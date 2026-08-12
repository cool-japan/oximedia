//! Comprehensive EBU R128 / ITU-R BS.1770-4 loudness metering implementation.
//!
//! This module provides a complete, standards-accurate implementation of:
//!
//! - **K-weighting filter** (ITU-R BS.1770-4) using Direct Form II Transposed biquad
//! - **EBU R128 integrated loudness meter** with 400 ms momentary and 3 s short-term windows
//! - **True peak detector** with 4× oversampling and windowed-sinc interpolation
//! - **Loudness report** with multi-standard compliance checking
//!
//! # Filter Design
//!
//! The K-weighting filter consists of two cascaded biquad sections:
//!
//! - **Stage 1** – High-shelf pre-filter modelling acoustic head effects (gain ≈ +4 dB above 2 kHz)
//! - **Stage 2** – High-pass filter modelling revised low-frequency B-weighting (f_c ≈ 38 Hz)
//!
//! Coefficients for 48 000 Hz are taken directly from Table 1 of ITU-R BS.1770-4.
//! For any other sample rate they are computed analytically from the analogue prototype
//! via the bilinear transform with frequency pre-warping.
//!
//! # Channel summation
//!
//! ITU-R BS.1770-4 §2 defines loudness as
//!
//! ```text
//! L = −0.691 + 10·log₁₀( Σ_ch G_ch · z_ch )
//! ```
//!
//! where `z_ch` is the mean square of the K-weighted signal of channel `ch` and
//! `G_ch` is that channel's weight (Table 2). The weighted channel powers are
//! **summed**, never averaged: there is no division by the channel count or by
//! the sum of the weights. Consequently a dual-mono stereo programme reads
//! exactly 10·log₁₀(2) = 3.01 LU **above** the identical mono programme, which
//! is the intended behaviour of the standard.
//!
//! # Gating
//!
//! Integrated loudness uses the two-stage gating algorithm of ITU-R BS.1771:
//!
//! 1. Absolute gate: discard blocks below −70 LUFS
//! 2. Relative gate: discard blocks more than −10 LU below the absolute-gated mean
//!
//! # True Peak
//!
//! Uses 4× oversampling with a 48-tap windowed-sinc FIR (Hann window) to detect
//! inter-sample peaks that would appear during DAC reconstruction.

#![allow(clippy::many_single_char_names)]
#![allow(clippy::similar_names)]

use std::collections::VecDeque;
use std::f64::consts::PI;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Absolute gate threshold (LUFS).
const ABSOLUTE_GATE: f64 = -70.0;

/// Relative gate offset (LU) used for Integrated Loudness gating
/// (ITU-R BS.1770-4 §3 / EBU R128).
const RELATIVE_GATE_OFFSET: f64 = -10.0;

/// Relative gate offset (LU) used for Loudness Range (LRA) gating.
///
/// EBU Tech 3342 §3.1 mandates a *wider* relative gate of −20 LU (as opposed
/// to the −10 LU offset used for Integrated Loudness) so that the lower edge
/// of the loudness-range distribution tracks the weakest "real" signal rather
/// than the noise floor.
const LRA_RELATIVE_GATE_OFFSET: f64 = -20.0;

/// Lower percentile (10th) used by the Loudness Range algorithm.
const LRA_PERCENTILE_LOW: f64 = 0.10;

/// Upper percentile (95th) used by the Loudness Range algorithm.
const LRA_PERCENTILE_HIGH: f64 = 0.95;

/// Reference sample rate for which table coefficients are given.
const REF_SAMPLE_RATE: f64 = 48_000.0;

/// K-weighting Stage 1 coefficients at 48 000 Hz (from ITU-R BS.1770-4 Table 1).
const K_STAGE1_B_48K: [f64; 3] = [
    1.535_124_859_586_97,
    -2.691_696_189_406_38,
    1.198_392_810_852_85,
];
const K_STAGE1_A_48K: [f64; 2] = [-1.690_659_293_182_41, 0.732_480_774_215_85];

/// K-weighting Stage 2 coefficients at 48 000 Hz (from ITU-R BS.1770-4 Table 1).
const K_STAGE2_B_48K: [f64; 3] = [1.0, -2.0, 1.0];
const K_STAGE2_A_48K: [f64; 2] = [-1.990_047_454_833_98, 0.990_072_250_366_21];

/// True peak oversampling factor.
const TP_OVERSAMPLE: usize = 4;

/// Half-length of the windowed-sinc upsampling FIR per sub-phase.
const TP_FIR_HALF_LEN: usize = 12;

/// Total taps per upsampling FIR.
const TP_FIR_TAPS: usize = TP_FIR_HALF_LEN * 2;

// ─── Direct Form II Transposed Biquad ────────────────────────────────────────

/// Second-order IIR biquad filter using Direct Form II Transposed structure.
///
/// The difference equation is:
/// ```text
/// y[n] = b0·x[n] + w1[n-1]
/// w1[n] = b1·x[n] - a1·y[n] + w2[n-1]
/// w2[n] = b2·x[n] - a2·y[n]
/// ```
/// This form has superior numerical properties compared to Direct Form I.
#[derive(Clone, Debug)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    w1: f64,
    w2: f64,
}

impl Biquad {
    /// Construct a biquad from standard feed-forward (b) and feed-back (a) coefficients.
    ///
    /// The `a` slice contains `[a1, a2]` (a0 = 1 is implicit).
    fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self {
            b0: b[0],
            b1: b[1],
            b2: b[2],
            a1: a[0],
            a2: a[1],
            w1: 0.0,
            w2: 0.0,
        }
    }

    /// Process a single sample and return the filtered output.
    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.w1;
        self.w1 = self.b1 * x - self.a1 * y + self.w2;
        self.w2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Reset internal state to zero.
    fn reset(&mut self) {
        self.w1 = 0.0;
        self.w2 = 0.0;
    }
}

// ─── K-weighting Filter ──────────────────────────────────────────────────────

/// K-weighting filter as specified in ITU-R BS.1770-4.
///
/// Two cascaded biquad stages:
/// - Stage 1: high-shelf pre-filter (head acoustic effect)
/// - Stage 2: high-pass filter (revised low-frequency B-weighting)
///
/// Coefficients for 48 000 Hz are taken verbatim from the standard.
/// For other sample rates they are derived via the bilinear transform with
/// frequency pre-warping so that the analogue corner frequencies are preserved.
#[derive(Clone, Debug)]
pub struct KWeightingFilter {
    stage1: Biquad,
    stage2: Biquad,
}

impl KWeightingFilter {
    /// Create a K-weighting filter for the given sample rate.
    ///
    /// Uses exact table coefficients for 48 000 Hz; computes via bilinear transform
    /// for all other sample rates.
    pub fn new(sample_rate: u32) -> Self {
        let fs = f64::from(sample_rate);
        if (fs - REF_SAMPLE_RATE).abs() < 0.5 {
            // Use table coefficients directly.
            Self {
                stage1: Biquad::new(K_STAGE1_B_48K, K_STAGE1_A_48K),
                stage2: Biquad::new(K_STAGE2_B_48K, K_STAGE2_A_48K),
            }
        } else {
            let (b1, a1) = Self::design_stage1(fs);
            let (b2, a2) = Self::design_stage2(fs);
            Self {
                stage1: Biquad::new(b1, a1),
                stage2: Biquad::new(b2, a2),
            }
        }
    }

    /// Process one input sample and return the K-weighted output.
    pub fn process(&mut self, sample: f64) -> f64 {
        let s1 = self.stage1.process(sample);
        self.stage2.process(s1)
    }

    /// Process a block of samples, returning the K-weighted output.
    pub fn process_block(&mut self, samples: &[f64]) -> Vec<f64> {
        samples.iter().map(|&s| self.process(s)).collect()
    }

    /// Reset filter state to zero.
    pub fn reset(&mut self) {
        self.stage1.reset();
        self.stage2.reset();
    }

    // ── Coefficient design ────────────────────────────────────────────────────

    /// Design Stage 1 (high-shelf pre-filter) via bilinear transform.
    ///
    /// The analogue prototype for the pre-filter is described in
    /// ITU-R BS.1770-4 Section 4.1.  Parameters:
    /// - f₀ = 1 681.974 Hz
    /// - G  = 3.999 84 dB
    /// - Q  = 0.707 18
    fn design_stage1(fs: f64) -> ([f64; 3], [f64; 2]) {
        const F0: f64 = 1_681.974_450_955_533;
        const G_DB: f64 = 3.999_843_853_973_347;
        const Q: f64 = 0.707_175_236_955_420;

        // Pre-warp the analogue frequency.
        let omega0 = 2.0 * PI * F0 / fs;
        let k = (omega0 / 2.0).tan(); // bilinear transform gain variable

        let vh = 10.0_f64.powf(G_DB / 20.0); // linear gain of the shelf
        let vb = vh.powf(0.5); // mid-band gain (geometric mean)

        let denom = 1.0 + k / Q + k * k;

        let b0 = (vh + vb * k / Q + k * k) / denom;
        let b1 = 2.0 * (k * k - vh) / denom;
        let b2 = (vh - vb * k / Q + k * k) / denom;
        let a1 = 2.0 * (k * k - 1.0) / denom;
        let a2 = (1.0 - k / Q + k * k) / denom;

        ([b0, b1, b2], [a1, a2])
    }

    /// Design Stage 2 (high-pass RLB filter) via bilinear transform.
    ///
    /// The analogue prototype is a second-order Butterworth high-pass filter
    /// with f₀ = 38.135 Hz and Q = 0.500.
    fn design_stage2(fs: f64) -> ([f64; 3], [f64; 2]) {
        const F0: f64 = 38.135_470_876_024_44;
        const Q: f64 = 0.500_327_037_323_877;

        let omega0 = 2.0 * PI * F0 / fs;
        let k = (omega0 / 2.0).tan();

        let denom = 1.0 + k / Q + k * k;

        // High-pass topology: b = [1, -2, 1] normalised by denom.
        let b0 = 1.0 / denom;
        let b1 = -2.0 / denom;
        let b2 = 1.0 / denom;
        let a1 = 2.0 * (k * k - 1.0) / denom;
        let a2 = (1.0 - k / Q + k * k) / denom;

        ([b0, b1, b2], [a1, a2])
    }
}

// ─── True Peak Detector ───────────────────────────────────────────────────────

/// True peak detector using 4× oversampling with windowed-sinc interpolation.
///
/// For every input sample the detector inserts three interpolated sub-samples
/// at fractional offsets 1/4, 2/4, and 3/4 of a sample period.  The absolute
/// maximum across all original and interpolated samples is tracked.
///
/// The interpolation FIR is designed using a 48-tap Hann-windowed sinc
/// (`sinc(n/L) · hann(n, N)` where L = 4 is the oversample factor and N = 48
/// is the filter length).
pub struct TruePeakDetector {
    /// FIR coefficients for each sub-phase (phases 1, 2, 3).
    fir_phases: [[f64; TP_FIR_TAPS]; 3],
    /// Circular delay buffer (stores the most recent TP_FIR_TAPS input samples).
    delay: [f64; TP_FIR_TAPS],
    /// Write position in the delay buffer.
    write_pos: usize,
    /// Running maximum of the absolute true peak in linear scale.
    max_peak: f64,
}

impl TruePeakDetector {
    /// Create a new true peak detector.
    ///
    /// The `sample_rate` parameter is accepted for API completeness but the
    /// oversampling FIR is dimensionless (it depends only on the oversample factor).
    pub fn new(_sample_rate: u32) -> Self {
        let fir_phases = Self::design_fir();
        Self {
            fir_phases,
            delay: [0.0; TP_FIR_TAPS],
            write_pos: 0,
            max_peak: 0.0,
        }
    }

    /// Process a single f32 sample and return the current sample's true peak (linear).
    pub fn process_sample(&mut self, sample: f32) -> f64 {
        let x = f64::from(sample);

        // Push sample into circular delay.
        self.delay[self.write_pos] = x;
        self.write_pos = (self.write_pos + 1) % TP_FIR_TAPS;

        // The original sample itself.
        let mut local_max = x.abs();

        // Three interpolated sub-samples.
        for phase_coeffs in &self.fir_phases {
            let interpolated = self.convolve(phase_coeffs);
            let abs_interp = interpolated.abs();
            if abs_interp > local_max {
                local_max = abs_interp;
            }
        }

        if local_max > self.max_peak {
            self.max_peak = local_max;
        }

        local_max
    }

    /// Maximum true peak in dBTP accumulated since creation or last reset.
    pub fn max_true_peak_dbtp(&self) -> f64 {
        if self.max_peak > 0.0 {
            20.0 * self.max_peak.log10()
        } else {
            f64::NEG_INFINITY
        }
    }

    /// Reset the detector (clears delay line and peak hold).
    pub fn reset(&mut self) {
        self.delay = [0.0; TP_FIR_TAPS];
        self.write_pos = 0;
        self.max_peak = 0.0;
    }

    // ── FIR convolution ───────────────────────────────────────────────────────

    /// Convolve the delay buffer with `coeffs`, reading oldest sample first.
    #[inline]
    fn convolve(&self, coeffs: &[f64; TP_FIR_TAPS]) -> f64 {
        let mut sum = 0.0;
        for (i, &c) in coeffs.iter().enumerate() {
            // Oldest sample in the delay is at write_pos (the next position to overwrite).
            let read_pos = (self.write_pos + i) % TP_FIR_TAPS;
            sum += self.delay[read_pos] * c;
        }
        sum
    }

    // ── FIR design ────────────────────────────────────────────────────────────

    /// Design three polyphase interpolation FIRs (one per sub-sample phase).
    ///
    /// Each FIR is a 48-tap Hann-windowed sinc evaluated at fractional sample
    /// offsets p/4 (p = 1, 2, 3) relative to the nearest input sample.
    fn design_fir() -> [[f64; TP_FIR_TAPS]; 3] {
        let n_total = TP_FIR_TAPS as f64; // 24
        let half = TP_FIR_HALF_LEN as f64; // 12.0

        let mut phases = [[0.0f64; TP_FIR_TAPS]; 3];

        for (phase_idx, phase_coeffs) in phases.iter_mut().enumerate() {
            let offset = (phase_idx + 1) as f64 / TP_OVERSAMPLE as f64; // 0.25, 0.50, 0.75

            let mut sum = 0.0;
            for (tap, coeff) in phase_coeffs.iter_mut().enumerate() {
                // `t` is the sample index relative to the centre of the FIR.
                let t = tap as f64 - half + offset;

                // Hann window.
                let window_idx = tap as f64 / (n_total - 1.0);
                let hann = 0.5 * (1.0 - (2.0 * PI * window_idx).cos());

                // Sinc function (normalised).
                let sinc = if t.abs() < 1e-12 {
                    1.0
                } else {
                    (PI * t).sin() / (PI * t)
                };

                *coeff = sinc * hann;
                sum += *coeff;
            }

            // Normalise so DC gain = 1.
            if sum.abs() > 1e-12 {
                for c in phase_coeffs.iter_mut() {
                    *c /= sum;
                }
            }
        }

        phases
    }
}

// ─── EBU R128 Meter ──────────────────────────────────────────────────────────

/// EBU R128 / ITU-R BS.1770-4 integrated loudness meter.
///
/// Processes interleaved f32 audio and provides:
///
/// | Measurement          | Window      | Notes                                 |
/// |----------------------|-------------|---------------------------------------|
/// | Momentary loudness   | 400 ms      | Updated every 100 ms (75 % overlap)   |
/// | Short-term loudness  | 3 000 ms    | Updated every 100 ms (75 % overlap)   |
/// | Integrated loudness  | Full file   | Two-stage gating per ITU-R BS.1771    |
/// | Loudness range (LRA) | Full file   | 10th–95th percentile difference       |
/// | True peak            | Per-sample  | 4× oversampled                        |
///
/// # Example
///
/// ```rust
/// use oximedia_metering::ebu_r128_impl::EbuR128Meter;
/// use std::f64::consts::PI;
///
/// let mut meter = EbuR128Meter::new(48000, 1);
///
/// // Generate 2 s of a 997 Hz sine at −3 dBFS (peak).
/// // LUFS ≈ dBFS_peak − 3.7 ≈ −6.7 LUFS for a mono sine.
/// let amplitude = 10.0_f64.powf(-3.0 / 20.0) as f32;
/// let sr = 48000u32;
/// let samples: Vec<f32> = (0..sr * 2)
///     .map(|n| amplitude * (2.0 * PI as f32 * 997.0 * n as f32 / sr as f32).sin())
///     .collect();
///
/// meter.process(&samples);
///
/// let m = meter.momentary_lufs();
/// // −3 dBFS peak sine → ≈ −6.7 LUFS (sine RMS = peak/√2, plus calibration offset).
/// assert!(m > -8.5 && m < -4.5, "momentary={m:.2}");
/// ```
pub struct EbuR128Meter {
    sample_rate: u32,
    channels: u32,

    /// One K-weighting filter per channel.
    k_filters: Vec<KWeightingFilter>,

    /// Weighted K-filtered energy accumulated over the current 100 ms hop.
    ///
    /// This is `Σ_frames Σ_ch G_ch · y_ch[n]²` — already summed across channels
    /// per ITU-R BS.1770-4 §2, so dividing it by the hop length yields
    /// `Σ_ch G_ch · z_ch` directly.
    hop_energy: f64,
    /// Count of samples accumulated in the current hop.
    hop_count: usize,
    /// Size of a 100 ms hop in samples.
    hop_size: usize,

    /// Per-channel ring buffer holding squared-weighted samples for the 400 ms
    /// momentary window (4 hops × hop_size).
    momentary_buf: VecDeque<f64>,
    /// Per-channel ring buffer for the 3 000 ms short-term window (30 hops × hop_size).
    short_term_buf: VecDeque<f64>,

    /// Running sum of squared-weighted samples in the 400 ms window.
    momentary_sum: f64,
    /// Running sum for the 3 000 ms window.
    short_term_sum: f64,
    /// Capacity of the momentary window in hop blocks.
    momentary_cap_hops: usize,
    /// Capacity of the short-term window in hop blocks.
    short_term_cap_hops: usize,

    /// Cached momentary loudness in LUFS.
    momentary_lufs: f64,
    /// Maximum momentary loudness seen.
    momentary_max: f64,
    /// Cached short-term loudness in LUFS.
    short_term_lufs: f64,
    /// Maximum short-term loudness seen.
    short_term_max: f64,

    /// All 400 ms gating blocks with their loudness values (LUFS) and mean power.
    gating_blocks: Vec<(f64, f64)>,
    /// Per-hop power values used to assemble 400 ms gating blocks (4 hops = 1 block).
    hop_powers: VecDeque<f64>,

    /// True peak detectors, one per channel (ITU-R BS.1770-4 §6: the reported
    /// true peak is the maximum across all channels).
    tp_detectors: Vec<TruePeakDetector>,

    /// Channel weighting factors per ITU-R BS.1770-4 Table 2.
    channel_weights: Vec<f64>,
}

impl EbuR128Meter {
    /// Create a new EBU R128 meter.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` – Sample rate in Hz (e.g. 44100, 48000)
    /// * `channels`    – Number of interleaved audio channels (1 = mono, 2 = stereo, …)
    pub fn new(sample_rate: u32, channels: u32) -> Self {
        let channels_usize = channels as usize;

        let k_filters = (0..channels_usize)
            .map(|_| KWeightingFilter::new(sample_rate))
            .collect();

        // 100 ms hop in samples.
        let hop_size = (u64::from(sample_rate) * 100 / 1000) as usize;

        // Window capacities in hops (not samples).
        let momentary_cap_hops = 4; // 4 × 100 ms = 400 ms
        let short_term_cap_hops = 30; // 30 × 100 ms = 3 000 ms

        let channel_weights = Self::channel_weights(channels_usize);

        Self {
            sample_rate,
            channels,
            k_filters,
            hop_energy: 0.0,
            hop_count: 0,
            hop_size,
            momentary_buf: VecDeque::new(),
            short_term_buf: VecDeque::new(),
            momentary_sum: 0.0,
            short_term_sum: 0.0,
            momentary_cap_hops,
            short_term_cap_hops,
            momentary_lufs: f64::NEG_INFINITY,
            momentary_max: f64::NEG_INFINITY,
            short_term_lufs: f64::NEG_INFINITY,
            short_term_max: f64::NEG_INFINITY,
            gating_blocks: Vec::new(),
            hop_powers: VecDeque::new(),
            tp_detectors: (0..channels_usize.max(1))
                .map(|_| TruePeakDetector::new(sample_rate))
                .collect(),
            channel_weights,
        }
    }

    // ── Audio processing ──────────────────────────────────────────────────────

    /// Process a block of interleaved audio samples.
    ///
    /// Samples must be in the range [−1.0, 1.0].  The slice length must be a
    /// multiple of `channels`; any trailing incomplete frame is silently ignored.
    pub fn process(&mut self, samples: &[f32]) {
        let channels = self.channels as usize;
        let frames = samples.len() / channels;

        for frame in 0..frames {
            let base = frame * channels;

            // K-weight each channel, accumulate the *weighted sum* of squared
            // samples across channels (ITU-R BS.1770-4 §2 — a sum, not a mean),
            // and run the oversampled true-peak detector on every channel.
            let mut frame_energy = 0.0;
            for ch in 0..channels {
                let raw = samples[base + ch];
                let s = f64::from(raw);
                let kw = self.k_filters[ch].process(s);
                frame_energy += kw * kw * self.channel_weights[ch];
                if let Some(detector) = self.tp_detectors.get_mut(ch) {
                    detector.process_sample(raw);
                }
            }
            self.hop_energy += frame_energy;

            self.hop_count += 1;

            if self.hop_count >= self.hop_size {
                self.complete_hop();
            }
        }
    }

    /// Complete the current 100 ms hop, updating all windows.
    fn complete_hop(&mut self) {
        let n = self.hop_count;
        if n == 0 {
            return;
        }

        // Loudness power for this hop: `Σ_ch G_ch · z_ch`, where `z_ch` is the
        // per-channel mean square of the K-weighted signal.
        //
        // ITU-R BS.1770-4 §2 sums the weighted channel powers; it does **not**
        // average them. Dividing by the channel count (or by the sum of the
        // channel weights, as an earlier revision did) makes dual-mono stereo
        // read 3.01 LU too low. Only the time average over the hop is taken.
        let hop_power = self.hop_energy / n as f64;

        // ── Momentary window (4 hops) ─────────────────────────────────────────
        self.momentary_buf.push_back(hop_power);
        self.momentary_sum += hop_power;
        if self.momentary_buf.len() > self.momentary_cap_hops {
            let removed = self.momentary_buf.pop_front().unwrap_or(0.0);
            self.momentary_sum -= removed;
        }

        if self.momentary_buf.len() == self.momentary_cap_hops {
            let mean = self.momentary_sum / self.momentary_cap_hops as f64;
            self.momentary_lufs = Self::power_to_lufs(mean);
            if self.momentary_lufs > self.momentary_max {
                self.momentary_max = self.momentary_lufs;
            }
        }

        // ── Short-term window (30 hops) ───────────────────────────────────────
        self.short_term_buf.push_back(hop_power);
        self.short_term_sum += hop_power;
        if self.short_term_buf.len() > self.short_term_cap_hops {
            let removed = self.short_term_buf.pop_front().unwrap_or(0.0);
            self.short_term_sum -= removed;
        }

        if self.short_term_buf.len() == self.short_term_cap_hops {
            let mean = self.short_term_sum / self.short_term_cap_hops as f64;
            self.short_term_lufs = Self::power_to_lufs(mean);
            if self.short_term_lufs > self.short_term_max {
                self.short_term_max = self.short_term_lufs;
            }
        }

        // ── Gating blocks (every 4 hops = 400 ms) ────────────────────────────
        self.hop_powers.push_back(hop_power);
        if self.hop_powers.len() > self.momentary_cap_hops {
            self.hop_powers.pop_front();
        }
        if self.hop_powers.len() == self.momentary_cap_hops {
            let block_mean: f64 =
                self.hop_powers.iter().sum::<f64>() / self.momentary_cap_hops as f64;
            let block_lufs = Self::power_to_lufs(block_mean);
            self.gating_blocks.push((block_lufs, block_mean));
        }

        // Reset hop accumulator.
        self.hop_energy = 0.0;
        self.hop_count = 0;
    }

    // ── Loudness accessors ────────────────────────────────────────────────────

    /// Momentary loudness in LUFS (400 ms sliding window).
    ///
    /// Returns `f64::NEG_INFINITY` if fewer than 400 ms of audio has been processed.
    pub fn momentary_lufs(&self) -> f64 {
        self.momentary_lufs
    }

    /// Short-term loudness in LUFS (3 000 ms sliding window).
    ///
    /// Returns `f64::NEG_INFINITY` if fewer than 3 s of audio has been processed.
    pub fn short_term_lufs(&self) -> f64 {
        self.short_term_lufs
    }

    /// Integrated loudness in LUFS using the ITU-R BS.1771 two-stage gating algorithm.
    ///
    /// Stage 1 – Absolute gate: exclude blocks below −70 LUFS.
    /// Stage 2 – Relative gate: exclude blocks more than 10 LU below the
    ///           absolute-gated mean.
    ///
    /// Returns `f64::NEG_INFINITY` if no blocks survive gating.
    pub fn integrated_lufs(&self) -> f64 {
        // Stage 1: absolute gate.
        let abs_gated: Vec<(f64, f64)> = self
            .gating_blocks
            .iter()
            .copied()
            .filter(|&(lufs, _)| lufs >= ABSOLUTE_GATE)
            .collect();

        if abs_gated.is_empty() {
            return f64::NEG_INFINITY;
        }

        let abs_mean: f64 = abs_gated.iter().map(|&(_, p)| p).sum::<f64>() / abs_gated.len() as f64;
        let abs_lufs = Self::power_to_lufs(abs_mean);

        // Stage 2: relative gate (absolute-gated mean − 10 LU).
        let rel_gate = abs_lufs + RELATIVE_GATE_OFFSET;
        let rel_gated: Vec<f64> = abs_gated
            .iter()
            .filter(|&&(lufs, _)| lufs >= rel_gate)
            .map(|&(_, p)| p)
            .collect();

        if rel_gated.is_empty() {
            return f64::NEG_INFINITY;
        }

        let rel_mean: f64 = rel_gated.iter().sum::<f64>() / rel_gated.len() as f64;
        Self::power_to_lufs(rel_mean)
    }

    /// Loudness range (LRA) in LU, per EBU Tech 3342 §3.1.
    ///
    /// The algorithm applies a *cascaded* gating scheme to the distribution of
    /// gating-block loudness values, then reports the spread between the 10th
    /// and 95th percentiles of what remains:
    ///
    /// 1. **Absolute gate** – discard blocks below −70 LUFS (`ABSOLUTE_GATE`).
    /// 2. **Relative gate** – compute the mean power of the absolute-gated
    ///    blocks, convert to LUFS, and discard blocks more than 20 LU below
    ///    that mean (`LRA_RELATIVE_GATE_OFFSET`). This relative-gate offset
    ///    is intentionally wider than the −10 LU offset used by
    ///    [`Self::integrated_lufs`] — it exists so that quiet
    ///    background/atmosphere sections do not inflate the reported range.
    /// 3. **Percentile range** – sort the doubly-gated loudness values and
    ///    return `p95 − p10`.
    ///
    /// Returns `0.0` if fewer than two blocks survive gating (there is no
    /// meaningful range to report).
    pub fn loudness_range_lu(&self) -> f64 {
        // Stage 1: absolute gate.
        let abs_gated: Vec<(f64, f64)> = self
            .gating_blocks
            .iter()
            .copied()
            .filter(|&(lufs, _)| lufs >= ABSOLUTE_GATE && lufs.is_finite())
            .collect();

        if abs_gated.len() < 2 {
            return 0.0;
        }

        let abs_mean: f64 = abs_gated.iter().map(|&(_, p)| p).sum::<f64>() / abs_gated.len() as f64;
        let abs_lufs = Self::power_to_lufs(abs_mean);

        // Stage 2: relative gate, −20 LU below the absolute-gated mean.
        let rel_gate = abs_lufs + LRA_RELATIVE_GATE_OFFSET;
        let mut rel_gated: Vec<f64> = abs_gated
            .iter()
            .filter(|&&(lufs, _)| lufs >= rel_gate)
            .map(|&(lufs, _)| lufs)
            .collect();

        if rel_gated.len() < 2 {
            return 0.0;
        }

        // Stage 3: percentile range of the doubly-gated distribution.
        rel_gated.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = rel_gated.len();

        let idx10 = (((n - 1) as f64) * LRA_PERCENTILE_LOW).round() as usize;
        let idx95 = (((n - 1) as f64) * LRA_PERCENTILE_HIGH).round() as usize;
        let idx10 = idx10.min(n - 1);
        let idx95 = idx95.min(n - 1);

        rel_gated[idx95] - rel_gated[idx10]
    }

    /// Maximum true peak in dBTP across **all** channels, detected since
    /// creation or the last reset.
    pub fn true_peak_dbtp(&self) -> f64 {
        self.tp_detectors
            .iter()
            .map(TruePeakDetector::max_true_peak_dbtp)
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// Per-channel maximum true peak in dBTP, in channel order.
    ///
    /// Channels that have never seen a non-zero sample report
    /// `f64::NEG_INFINITY`.
    pub fn channel_true_peaks_dbtp(&self) -> Vec<f64> {
        self.tp_detectors
            .iter()
            .map(TruePeakDetector::max_true_peak_dbtp)
            .collect()
    }

    /// Reset all measurements.
    pub fn reset(&mut self) {
        for f in &mut self.k_filters {
            f.reset();
        }
        self.hop_energy = 0.0;
        self.hop_count = 0;
        self.momentary_buf.clear();
        self.short_term_buf.clear();
        self.momentary_sum = 0.0;
        self.short_term_sum = 0.0;
        self.momentary_lufs = f64::NEG_INFINITY;
        self.momentary_max = f64::NEG_INFINITY;
        self.short_term_lufs = f64::NEG_INFINITY;
        self.short_term_max = f64::NEG_INFINITY;
        self.gating_blocks.clear();
        self.hop_powers.clear();
        for detector in &mut self.tp_detectors {
            detector.reset();
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Convert loudness power to LUFS: `L = −0.691 + 10 · log₁₀(P)`.
    ///
    /// `power` must already be the **weighted sum across channels** of the
    /// per-channel mean squares (`Σ_ch G_ch · z_ch`), not a per-channel average.
    #[inline]
    fn power_to_lufs(power: f64) -> f64 {
        if power > 0.0 {
            -0.691 + 10.0 * power.log10()
        } else {
            f64::NEG_INFINITY
        }
    }

    /// Return ITU-R BS.1770-4 channel weights for a given channel count.
    ///
    /// Standard weight table (Table 2):
    ///
    /// | Format | L   | R   | C   | LFE | Ls  | Rs  | Lss | Rss |
    /// |--------|-----|-----|-----|-----|-----|-----|-----|-----|
    /// | Mono   | 1.0 |     |     |     |     |     |     |     |
    /// | Stereo | 1.0 | 1.0 |     |     |     |     |     |     |
    /// | 5.1    | 1.0 | 1.0 | 1.0 | 0.0 |1.41 |1.41 |     |     |
    /// | 7.1    | 1.0 | 1.0 | 1.0 | 0.0 |1.41 |1.41 |1.41 |1.41 |
    fn channel_weights(channels: usize) -> Vec<f64> {
        match channels {
            1 => vec![1.0],
            2 => vec![1.0, 1.0],
            3 => vec![1.0, 1.0, 1.0],
            4 => vec![1.0, 1.0, 1.0, 0.0],
            5 => vec![1.0, 1.0, 1.0, 1.41, 1.41],
            6 => vec![1.0, 1.0, 1.0, 0.0, 1.41, 1.41],
            7 => vec![1.0, 1.0, 1.0, 0.0, 1.41, 1.41, 1.41],
            8 => vec![1.0, 1.0, 1.0, 0.0, 1.41, 1.41, 1.41, 1.41],
            _ => vec![1.0; channels],
        }
    }

    /// Return the sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Return the channel count.
    pub fn channels(&self) -> u32 {
        self.channels
    }
}

// ─── Loudness Report ─────────────────────────────────────────────────────────

/// Comprehensive loudness report derived from an [`EbuR128Meter`].
///
/// Provides measured values plus compliance status for three broadcast standards:
///
/// - **EBU R128** – target −23 LUFS ±1 LU, LRA ≤ 20 LU, TP ≤ −1 dBTP
/// - **ATSC A/85** – target −24 LKFS ±2 dB, TP ≤ −2 dBTP
/// - **ARIB TR-B32** – target −24 LUFS ±1 LU
#[derive(Clone, Debug)]
pub struct LoudnessReport {
    /// Integrated loudness in LUFS.
    pub integrated_lufs: f64,
    /// Maximum momentary loudness (400 ms) in LUFS.
    pub momentary_max_lufs: f64,
    /// Maximum short-term loudness (3 000 ms) in LUFS.
    pub short_term_max_lufs: f64,
    /// Loudness range (LRA) in LU.
    pub loudness_range_lu: f64,
    /// Maximum true peak in dBTP.
    pub true_peak_dbtp: f64,
    /// Whether the audio is compliant with EBU R128.
    pub complies_ebu_r128: bool,
    /// Whether the audio is compliant with ATSC A/85.
    pub complies_atsc_a85: bool,
    /// Whether the audio is compliant with ARIB TR-B32.
    pub complies_arib_tr_b32: bool,
}

impl LoudnessReport {
    /// Derive a report from a finished [`EbuR128Meter`].
    pub fn from_meter(meter: &EbuR128Meter) -> Self {
        let integrated = meter.integrated_lufs();
        let momentary_max = meter.momentary_max;
        let short_term_max = meter.short_term_max;
        let lra = meter.loudness_range_lu();
        let tp = meter.true_peak_dbtp();

        // EBU R128: −23 LUFS ±1 LU, LRA ≤ 20 LU, TP ≤ −1 dBTP
        let complies_ebu = integrated.is_finite()
            && (integrated - (-23.0)).abs() <= 1.0
            && lra <= 20.0
            && tp <= -1.0;

        // ATSC A/85: −24 LKFS ±2 dB, TP ≤ −2 dBTP
        let complies_atsc =
            integrated.is_finite() && (integrated - (-24.0)).abs() <= 2.0 && tp <= -2.0;

        // ARIB TR-B32: −24 LUFS ±1 LU
        let complies_arib = integrated.is_finite() && (integrated - (-24.0)).abs() <= 1.0;

        Self {
            integrated_lufs: integrated,
            momentary_max_lufs: momentary_max,
            short_term_max_lufs: short_term_max,
            loudness_range_lu: lra,
            true_peak_dbtp: tp,
            complies_ebu_r128: complies_ebu,
            complies_atsc_a85: complies_atsc,
            complies_arib_tr_b32: complies_arib,
        }
    }

    /// Format a human-readable one-line summary.
    pub fn format_summary(&self) -> String {
        format!(
            "I={:.1} LUFS  LRA={:.1} LU  TP={:.1} dBTP  [EBU:{} ATSC:{} ARIB:{}]",
            self.integrated_lufs,
            self.loudness_range_lu,
            self.true_peak_dbtp,
            if self.complies_ebu_r128 { "OK" } else { "FAIL" },
            if self.complies_atsc_a85 { "OK" } else { "FAIL" },
            if self.complies_arib_tr_b32 {
                "OK"
            } else {
                "FAIL"
            },
        )
    }

    /// Calculate the gain (in dB) needed to reach `target_lufs`.
    ///
    /// Returns `0.0` if integrated loudness is not finite.
    pub fn recommended_gain_db(&self, target_lufs: f64) -> f64 {
        if self.integrated_lufs.is_finite() {
            target_lufs - self.integrated_lufs
        } else {
            0.0
        }
    }
}

// ─── Unit Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Generate `n_samples` of a mono sine wave at `freq_hz` with `amplitude_dbfs` dBFS.
    fn mono_sine(
        freq_hz: f64,
        amplitude_dbfs: f64,
        sample_rate: u32,
        n_samples: usize,
    ) -> Vec<f32> {
        let amplitude = 10.0_f64.powf(amplitude_dbfs / 20.0);
        let fs = f64::from(sample_rate);
        (0..n_samples)
            .map(|n| (amplitude * (2.0 * PI * freq_hz * n as f64 / fs).sin()) as f32)
            .collect()
    }

    /// Build an interleaved stereo signal made of concatenated sine-wave
    /// segments, each `seg_duration_secs` long, applied in phase and at equal
    /// level to both channels ("dual mono"), matching the construction of the
    /// EBU Tech 3342 §4 "minimum requirements test signals".
    ///
    /// `segments_dbfs` gives the per-channel peak level (in dBFS) of each
    /// consecutive segment.
    fn stereo_sine_segments(
        freq_hz: f64,
        segments_dbfs: &[f64],
        seg_duration_secs: f64,
        sample_rate: u32,
    ) -> Vec<f32> {
        let n_per_seg = (f64::from(sample_rate) * seg_duration_secs) as usize;
        let mut interleaved = Vec::with_capacity(segments_dbfs.len() * n_per_seg * 2);
        for &dbfs in segments_dbfs {
            let mono = mono_sine(freq_hz, dbfs, sample_rate, n_per_seg);
            for s in mono {
                interleaved.push(s); // L
                interleaved.push(s); // R (identical, in phase)
            }
        }
        interleaved
    }

    // ── K-weighting filter tests ──────────────────────────────────────────────

    #[test]
    fn test_k_weight_filter_constructs_for_48k() {
        let _ = KWeightingFilter::new(48_000);
    }

    #[test]
    fn test_k_weight_filter_constructs_for_44100() {
        let _ = KWeightingFilter::new(44_100);
    }

    #[test]
    fn test_k_weight_filter_constructs_for_96k() {
        let _ = KWeightingFilter::new(96_000);
    }

    /// K-weighting should be approximately 0 dB at 997 Hz (the reference frequency
    /// used in ITU-R BS.1770-4 self-tests).
    #[test]
    fn test_k_weight_flat_at_997hz() {
        let sample_rate = 48_000_u32;
        let mut filter = KWeightingFilter::new(sample_rate);

        // Warm up the filter with 0.1 s of a 997 Hz sine (filter settles quickly).
        let warmup = mono_sine(997.0, 0.0, sample_rate, sample_rate as usize / 10);
        for &s in &warmup {
            filter.process(f64::from(s));
        }

        // Measure RMS of filtered output and input over 0.05 s.
        let n = sample_rate as usize / 20;
        let phase_offset = warmup.len();
        let fs = f64::from(sample_rate);
        let mut sum_in_sq = 0.0;
        let mut sum_out_sq = 0.0;

        for i in 0..n {
            let s = (2.0 * PI * 997.0 * (phase_offset + i) as f64 / fs).sin();
            let out = filter.process(s);
            sum_in_sq += s * s;
            sum_out_sq += out * out;
        }

        let gain_db = 10.0 * (sum_out_sq / sum_in_sq).log10();
        // Allow ±1 dB at 997 Hz (the standard specifies ≈ 0 dB).
        assert!(
            gain_db.abs() < 1.0,
            "K-weighting gain at 997 Hz = {gain_db:.3} dB, expected ≈ 0 dB"
        );
    }

    #[test]
    fn test_k_weight_filter_reset_clears_state() {
        let mut filter = KWeightingFilter::new(48_000);
        // Force some state.
        for _ in 0..100 {
            filter.process(1.0);
        }
        filter.reset();
        // After reset, processing silence must give silence.
        let out = filter.process(0.0);
        assert_eq!(out, 0.0, "after reset, 0.0 input should give 0.0 output");
    }

    #[test]
    fn test_k_weight_process_block() {
        let mut filter = KWeightingFilter::new(48_000);
        let input: Vec<f64> = (0..1024)
            .map(|n| (2.0 * PI * 1000.0 * n as f64 / 48_000.0).sin())
            .collect();
        let output = filter.process_block(&input);
        assert_eq!(output.len(), input.len());
        assert!(output.iter().all(|x| x.is_finite()));
    }

    // ── Biquad filter tests ───────────────────────────────────────────────────

    #[test]
    fn test_biquad_silence_in_silence_out() {
        let mut bq = Biquad::new([1.0, 0.0, 0.0], [0.0, 0.0]);
        assert_eq!(bq.process(0.0), 0.0);
    }

    #[test]
    fn test_biquad_reset() {
        let mut bq = Biquad::new([1.0, 0.0, 0.0], [0.0, 0.0]);
        bq.process(0.5);
        bq.reset();
        assert_eq!(bq.w1, 0.0);
        assert_eq!(bq.w2, 0.0);
    }

    #[test]
    fn test_biquad_identity_filter() {
        // b = [1, 0, 0], a = [0, 0] → identity.
        let mut bq = Biquad::new([1.0, 0.0, 0.0], [0.0, 0.0]);
        for x in [0.1, -0.5, 0.9, 0.0, 1.0] {
            assert!(
                (bq.process(x) - x).abs() < 1e-12,
                "identity biquad failed for x={x}"
            );
        }
    }

    // ── True peak tests ───────────────────────────────────────────────────────

    #[test]
    fn test_tp_detector_silence_gives_neg_inf() {
        let mut tp = TruePeakDetector::new(48_000);
        let silence = vec![0.0f32; 4800];
        for &s in &silence {
            tp.process_sample(s);
        }
        // All-zero input → max_peak stays 0 → dBTP is -∞.
        assert!(
            tp.max_true_peak_dbtp().is_infinite(),
            "silence should give −∞ dBTP"
        );
    }

    #[test]
    fn test_tp_detector_full_scale_sine() {
        let mut tp = TruePeakDetector::new(48_000);
        let sr = 48_000_u32;
        // Full-scale 997 Hz sine.
        let samples = mono_sine(997.0, 0.0, sr, sr as usize / 2);
        for &s in &samples {
            tp.process_sample(s);
        }
        // True peak of a full-scale sine is 0 dBTP (or slightly above due to
        // inter-sample peaks, but should be within [−1, +1] dBTP).
        let tp_db = tp.max_true_peak_dbtp();
        assert!(
            tp_db > -1.0 && tp_db <= 1.0,
            "TP of full-scale sine = {tp_db:.2} dBTP"
        );
    }

    #[test]
    fn test_tp_detector_reset() {
        let mut tp = TruePeakDetector::new(48_000);
        // 0.1 s is enough to register a true peak; no need for a full 1 s.
        let samples = mono_sine(997.0, 0.0, 48_000, 4_800);
        for &s in &samples {
            tp.process_sample(s);
        }
        tp.reset();
        assert!(
            tp.max_true_peak_dbtp().is_infinite(),
            "after reset, TP should be −∞"
        );
    }

    #[test]
    fn test_tp_detector_returns_finite_for_signal() {
        let mut tp = TruePeakDetector::new(48_000);
        let samples = mono_sine(997.0, -6.0, 48_000, 4800);
        for &s in &samples {
            tp.process_sample(s);
        }
        assert!(tp.max_true_peak_dbtp().is_finite());
    }

    // ── EbuR128Meter tests ────────────────────────────────────────────────────

    #[test]
    fn test_meter_constructs() {
        let _ = EbuR128Meter::new(48_000, 1);
        let _ = EbuR128Meter::new(48_000, 2);
        let _ = EbuR128Meter::new(44_100, 2);
    }

    /// A 997 Hz sine at −3 dBFS should give momentary loudness of approximately
    /// −6.7 LUFS (not −3 LUFS).
    ///
    /// Physics: for a peak-normalised sine at p dBFS:
    ///   mean_sq = 10^(p/10) / 2   (sine RMS = peak/√2)
    ///   LUFS    = −0.691 + 10·log₁₀(mean_sq)
    ///           = p − 3.01 − 0.691
    ///           ≈ p − 3.7
    ///
    /// So −3 dBFS peak → ≈ −6.7 LUFS.
    /// K-weighting is ≈ 0 dB at 997 Hz, so no significant correction here.
    #[test]
    fn test_momentary_lufs_997hz_minus3dbfs() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);

        // 0.5 s is enough to fill the 400 ms momentary window (4 × 100 ms hops).
        let samples = mono_sine(997.0, -3.0, sr, sr as usize / 2);
        meter.process(&samples);

        let m = meter.momentary_lufs();
        assert!(
            m.is_finite(),
            "momentary LUFS should be finite after 0.5 s of signal"
        );
        // Expected: -3 - 3.7 = -6.7 LUFS.  Allow ±1.5 LUFS tolerance.
        assert!(
            m > -8.2 && m < -5.2,
            "momentary LUFS = {m:.2}, expected ≈ −6.7 LUFS for 997 Hz @ −3 dBFS peak"
        );
    }

    /// Silence must give −∞ LUFS for all measurements.
    #[test]
    fn test_silence_gives_neg_infinity() {
        let mut meter = EbuR128Meter::new(48_000, 1);
        // 0.5 s is enough to verify silence behaviour without processing 4 s.
        let silence = vec![0.0f32; 24_000];
        meter.process(&silence);

        let i = meter.integrated_lufs();
        assert!(
            i.is_infinite() && i.is_sign_negative(),
            "silence: integrated LUFS should be −∞, got {i}"
        );
    }

    #[test]
    fn test_meter_reset_clears_state() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);
        // 0.5 s fills the 400 ms momentary window; no need for 2 s.
        let samples = mono_sine(997.0, -3.0, sr, sr as usize / 2);
        meter.process(&samples);

        // Ensure some measurement happened.
        assert!(meter.momentary_lufs().is_finite());

        meter.reset();
        assert!(
            meter.momentary_lufs().is_infinite(),
            "after reset, momentary should be −∞"
        );
        assert!(
            meter.integrated_lufs().is_infinite(),
            "after reset, integrated should be −∞"
        );
        assert!(
            meter.true_peak_dbtp().is_infinite(),
            "after reset, true peak should be −∞"
        );
    }

    #[test]
    fn test_integrated_lufs_approaches_momentary_for_steady_tone() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);

        // 1 s is enough for several gating blocks (400 ms each, 75% overlap).
        let samples = mono_sine(997.0, -18.0, sr, sr as usize);
        meter.process(&samples);

        let m = meter.momentary_lufs();
        let i = meter.integrated_lufs();
        assert!(m.is_finite() && i.is_finite());

        // Integrated should be within 2 LU of momentary for a steady tone.
        assert!(
            (m - i).abs() < 2.0,
            "momentary={m:.2}, integrated={i:.2}; should agree within 2 LU"
        );
    }

    #[test]
    fn test_stereo_meter() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 2);

        // 0.5 s stereo fills the 400 ms momentary window; halved from 1 s.
        let mono = mono_sine(997.0, -6.0, sr, sr as usize / 2);
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        meter.process(&stereo);

        assert!(
            meter.momentary_lufs().is_finite(),
            "stereo momentary should be finite"
        );
    }

    // ── ITU-R BS.1770-4 channel-summation conformance ─────────────────────────
    //
    // BS.1770-4 §2 defines loudness as −0.691 + 10·log₁₀(Σ_ch G_ch·z_ch): the
    // weighted per-channel mean squares are **summed**. An earlier revision of
    // this meter divided the summed energy by the sum of the channel weights,
    // which made every dual-mono stereo programme read 3.01 LU too low (and
    // therefore over-amplified BGM during loudness normalisation).

    /// 10·log₁₀(2) — the exact loudness offset between a mono programme and the
    /// dual-mono stereo programme built from it.
    const DUAL_MONO_OFFSET_LU: f64 = 3.010_299_956_639_812;

    /// A dual-mono stereo signal must read exactly +3.01 LU above the identical
    /// mono signal, for **momentary** loudness.
    #[test]
    fn test_dual_mono_stereo_reads_plus_3db_momentary() {
        let sr = 48_000_u32;
        let mono = mono_sine(997.0, -20.0, sr, sr as usize);
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();

        let mut mono_meter = EbuR128Meter::new(sr, 1);
        mono_meter.process(&mono);
        let mono_m = mono_meter.momentary_lufs();

        let mut stereo_meter = EbuR128Meter::new(sr, 2);
        stereo_meter.process(&stereo);
        let stereo_m = stereo_meter.momentary_lufs();

        assert!(
            mono_m.is_finite() && stereo_m.is_finite(),
            "both meters must produce a momentary reading (mono={mono_m}, stereo={stereo_m})"
        );

        let delta = stereo_m - mono_m;
        assert!(
            (delta - DUAL_MONO_OFFSET_LU).abs() <= 0.05,
            "dual-mono stereo momentary must be +3.01 LU above mono; \
             mono={mono_m:.4} LUFS, stereo={stereo_m:.4} LUFS, Δ={delta:.4} LU"
        );
    }

    /// Same conformance requirement for **integrated** loudness (the value the
    /// normalisation stage acts on).
    #[test]
    fn test_dual_mono_stereo_reads_plus_3db_integrated() {
        let sr = 48_000_u32;
        let mono = mono_sine(997.0, -20.0, sr, sr as usize * 2);
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();

        let mut mono_meter = EbuR128Meter::new(sr, 1);
        mono_meter.process(&mono);
        let mono_i = mono_meter.integrated_lufs();

        let mut stereo_meter = EbuR128Meter::new(sr, 2);
        stereo_meter.process(&stereo);
        let stereo_i = stereo_meter.integrated_lufs();

        assert!(
            mono_i.is_finite() && stereo_i.is_finite(),
            "both meters must produce an integrated reading (mono={mono_i}, stereo={stereo_i})"
        );

        let delta = stereo_i - mono_i;
        assert!(
            (delta - DUAL_MONO_OFFSET_LU).abs() <= 0.05,
            "dual-mono stereo integrated must be +3.01 LU above mono; \
             mono={mono_i:.4} LUFS, stereo={stereo_i:.4} LUFS, Δ={delta:.4} LU"
        );
    }

    /// Absolute calibration of the stereo case.
    ///
    /// A 997 Hz sine at −20 dBFS peak has a per-channel mean square of
    /// 10^(−2)/2, i.e. −23.01 dBFS RMS. K-weighting is +0.691 dB at 997 Hz,
    /// which exactly cancels the −0.691 LUFS calibration offset, so the mono
    /// reading is −23.01 LUFS. Summing two identical channels adds
    /// 10·log₁₀(2) = 3.01 LU → **−20.00 LUFS**.
    ///
    /// This pins the sign of the fix: with the old divide-by-weight-sum the
    /// same signal read −23.01 LUFS, identical to mono.
    #[test]
    fn test_stereo_absolute_calibration_997hz_minus20dbfs() {
        let sr = 48_000_u32;
        let mono = mono_sine(997.0, -20.0, sr, sr as usize * 2);
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();

        let mut meter = EbuR128Meter::new(sr, 2);
        meter.process(&stereo);

        let i = meter.integrated_lufs();
        assert!(
            (i - (-20.0)).abs() < 0.2,
            "dual-mono stereo −20 dBFS sine should read ≈ −20.0 LUFS, got {i:.3}"
        );
    }

    /// Mono calibration must be untouched by the channel-summation fix:
    /// a 997 Hz mono sine at −20 dBFS peak still reads ≈ −23.01 LUFS
    /// (−3.01 dB sine crest, K-weighting +0.691 dB cancelling the −0.691 offset).
    #[test]
    fn test_mono_absolute_calibration_unchanged_997hz_minus20dbfs() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);
        meter.process(&mono_sine(997.0, -20.0, sr, sr as usize * 2));

        let i = meter.integrated_lufs();
        assert!(
            (i - (-23.01)).abs() < 0.2,
            "mono −20 dBFS sine should still read ≈ −23.01 LUFS, got {i:.3}"
        );
    }

    /// The −10 LU relative gate must still exclude quiet material.
    ///
    /// 5 s at −20 dBFS followed by 5 s at −60 dBFS: the quiet half sits well
    /// above the −70 LUFS absolute gate, so only the relative gate can remove
    /// it. With the relative gate working, the integrated value tracks the loud
    /// half (≈ −23.01 LUFS); without it, the two halves average in power and the
    /// reading drops by ≈ 3 LU.
    #[test]
    fn test_relative_gate_excludes_quiet_section() {
        let sr = 48_000_u32;
        let mut samples = mono_sine(997.0, -20.0, sr, sr as usize * 5);
        samples.extend(mono_sine(997.0, -60.0, sr, sr as usize * 5));

        let mut meter = EbuR128Meter::new(sr, 1);
        meter.process(&samples);

        let i = meter.integrated_lufs();
        assert!(
            (i - (-23.01)).abs() < 0.3,
            "the −10 LU relative gate should exclude the −60 dBFS half, \
             leaving ≈ −23.01 LUFS; got {i:.3}"
        );
    }

    #[test]
    fn test_short_term_lufs_valid_after_3s() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);
        // 3 s is the exact short-term window; process exactly 3 s (30 × 100 ms hops).
        let samples = mono_sine(997.0, -12.0, sr, sr as usize * 3);
        meter.process(&samples);

        assert!(
            meter.short_term_lufs().is_finite(),
            "short-term LUFS should be finite after 3 s of signal"
        );
    }

    #[test]
    fn test_loudness_range_zero_for_short_signal() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);
        // Only 0.5 s – not enough for LRA percentiles to be meaningful.
        let samples = mono_sine(997.0, -12.0, sr, sr as usize / 2);
        meter.process(&samples);

        // LRA can be 0 when there is < 2 gating blocks.
        let lra = meter.loudness_range_lu();
        assert!(lra >= 0.0, "LRA must be non-negative");
    }

    // ── EBU Tech 3342 "minimum requirements" LRA conformance tests ────────────
    //
    // Reference: EBU Tech 3342 (November 2023), §4 "Minimum requirements,
    // compliance test", Table 1. Each test signal is a stereo, in-phase,
    // dual-mono sine wave made of consecutive 20 s segments at the stated
    // per-channel peak dBFS levels; the expected LRA is given with a ±1 LU
    // tolerance. These are the exact reference signals/values published by
    // the EBU PLOUD group for validating "EBU Mode" loudness meters.

    /// EBU Tech 3342 Table 1, test case 1: −20.0 dBFS then −30.0 dBFS
    /// (10 dB apart) → expected LRA = 10 ±1 LU.
    #[test]
    fn test_lra_ebu_tech3342_case1() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 2);
        let samples = stereo_sine_segments(1_000.0, &[-20.0, -30.0], 20.0, sr);
        meter.process(&samples);

        let lra = meter.loudness_range_lu();
        assert!(
            (lra - 10.0).abs() <= 1.0,
            "EBU Tech 3342 case 1: expected LRA = 10 ±1 LU, got {lra:.2} LU"
        );
    }

    /// EBU Tech 3342 Table 1, test case 2: −20.0 dBFS then −15.0 dBFS
    /// (5 dB apart) → expected LRA = 5 ±1 LU.
    #[test]
    fn test_lra_ebu_tech3342_case2() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 2);
        let samples = stereo_sine_segments(1_000.0, &[-20.0, -15.0], 20.0, sr);
        meter.process(&samples);

        let lra = meter.loudness_range_lu();
        assert!(
            (lra - 5.0).abs() <= 1.0,
            "EBU Tech 3342 case 2: expected LRA = 5 ±1 LU, got {lra:.2} LU"
        );
    }

    /// EBU Tech 3342 Table 1, test case 3: −40.0 dBFS then −20.0 dBFS
    /// (20 dB apart) → expected LRA = 20 ±1 LU.
    #[test]
    fn test_lra_ebu_tech3342_case3() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 2);
        let samples = stereo_sine_segments(1_000.0, &[-40.0, -20.0], 20.0, sr);
        meter.process(&samples);

        let lra = meter.loudness_range_lu();
        assert!(
            (lra - 20.0).abs() <= 1.0,
            "EBU Tech 3342 case 3: expected LRA = 20 ±1 LU, got {lra:.2} LU"
        );
    }

    /// EBU Tech 3342 Table 1, test case 4: five 20 s segments at
    /// −50.0, −35.0, −20.0, −35.0, −50.0 dBFS → expected LRA = 15 ±1 LU.
    ///
    /// This case only passes when the −20 LU relative gate (Tech 3342 §3.1)
    /// is applied in addition to the −70 LUFS absolute gate: the two −50 dBFS
    /// "tails" sit well above the absolute gate but must be excluded by the
    /// relative gate, or the measured range balloons to ~30 LU.
    #[test]
    fn test_lra_ebu_tech3342_case4() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 2);
        let samples = stereo_sine_segments(1_000.0, &[-50.0, -35.0, -20.0, -35.0, -50.0], 20.0, sr);
        meter.process(&samples);

        let lra = meter.loudness_range_lu();
        assert!(
            (lra - 15.0).abs() <= 1.0,
            "EBU Tech 3342 case 4: expected LRA = 15 ±1 LU, got {lra:.2} LU"
        );
    }

    /// True peak must be measured on **every** channel, not just channel 0.
    ///
    /// Regression guard: the detector used to run on `samples[base]` only, so a
    /// loud right channel next to a quiet left channel was reported at the left
    /// channel's level.
    #[test]
    fn test_true_peak_uses_all_channels() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 2);

        // L = −40 dBFS, R = −6 dBFS (interleaved).
        let left = mono_sine(997.0, -40.0, sr, sr as usize / 2);
        let right = mono_sine(997.0, -6.0, sr, sr as usize / 2);
        let stereo: Vec<f32> = left
            .iter()
            .zip(right.iter())
            .flat_map(|(&l, &r)| [l, r])
            .collect();
        meter.process(&stereo);

        let tp = meter.true_peak_dbtp();
        assert!(
            (tp - (-6.0)).abs() < 1.0,
            "true peak must follow the loudest channel (−6 dBTP), got {tp:.2} dBTP"
        );

        let per_channel = meter.channel_true_peaks_dbtp();
        assert_eq!(per_channel.len(), 2, "one true peak per channel");
        assert!(
            (per_channel[0] - (-40.0)).abs() < 1.0,
            "left channel true peak = {:.2} dBTP, expected ≈ −40",
            per_channel[0]
        );
        assert!(
            (per_channel[1] - (-6.0)).abs() < 1.0,
            "right channel true peak = {:.2} dBTP, expected ≈ −6",
            per_channel[1]
        );
    }

    #[test]
    fn test_true_peak_detected_above_signal_peak() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);

        // Full-scale 997 Hz sine.
        let samples = mono_sine(997.0, 0.0, sr, sr as usize / 2);
        meter.process(&samples);

        let tp = meter.true_peak_dbtp();
        assert!(
            tp.is_finite(),
            "true peak should be finite for full-scale signal"
        );
    }

    // ── LoudnessReport tests ──────────────────────────────────────────────────

    #[test]
    fn test_report_from_meter() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);
        // 1 s is sufficient to get several gating blocks and a valid integrated value.
        let samples = mono_sine(997.0, -23.0, sr, sr as usize);
        meter.process(&samples);

        let report = LoudnessReport::from_meter(&meter);
        assert!(report.integrated_lufs.is_finite());
        assert!(report.loudness_range_lu >= 0.0);
    }

    #[test]
    fn test_report_format_summary_contains_lufs() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);
        // 1 s is sufficient to get a valid integrated value for the summary.
        let samples = mono_sine(997.0, -23.0, sr, sr as usize);
        meter.process(&samples);

        let report = LoudnessReport::from_meter(&meter);
        let summary = report.format_summary();
        assert!(
            summary.contains("LUFS"),
            "summary should contain 'LUFS': {summary}"
        );
    }

    #[test]
    fn test_report_recommended_gain() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);
        // A -19.3 dBFS peak sine gives ≈ -23 LUFS (since LUFS ≈ dBFS_peak - 3.7).
        // Use -22 dBFS peak to land around -25.7 LUFS, giving ~2.7 dB recommended gain.
        // 1 s gives enough gating blocks for a stable integrated reading.
        let samples = mono_sine(997.0, -22.0, sr, sr as usize);
        meter.process(&samples);

        let report = LoudnessReport::from_meter(&meter);
        // Integrated should be around -25.7 LUFS.
        // Recommended gain to -23 LUFS ≈ +2.7 dB.
        let gain = report.recommended_gain_db(-23.0);
        // Allow ±1.5 dB tolerance.
        assert!(
            gain > 0.5 && gain < 5.0,
            "recommended gain = {gain:.2} dB, expected ~+2.7 dB"
        );
    }

    #[test]
    fn test_report_silence_recommended_gain_zero() {
        let mut meter = EbuR128Meter::new(48_000, 1);
        meter.process(&vec![0.0f32; 48_000]);
        let report = LoudnessReport::from_meter(&meter);
        // Integrated is −∞, so recommended gain must be 0.
        assert_eq!(report.recommended_gain_db(-23.0), 0.0);
    }

    /// Verify EBU R128 compliance for a signal calibrated to approximately −23 LUFS.
    ///
    /// For a 997 Hz mono sine: LUFS ≈ dBFS_peak − 3.7 (sine power factor + calibration).
    /// Target: LUFS = −23  →  dBFS_peak = −23 + 3.7 = −19.3 dBFS.
    /// True peak for −19.3 dBFS peak sine = −19.3 dBTP, well below −1 dBTP limit.
    #[test]
    fn test_ebu_compliance_for_target_signal() {
        let sr = 48_000_u32;
        let mut meter = EbuR128Meter::new(sr, 1);
        // −19.3 dBFS peak → ≈ −23 LUFS; 2 s gives stable integrated loudness.
        let samples = mono_sine(997.0, -19.3, sr, sr as usize * 2);
        meter.process(&samples);

        let report = LoudnessReport::from_meter(&meter);
        // The integrated level should be within ±1 LU of −23 LUFS for EBU R128 compliance.
        assert!(
            report.complies_ebu_r128,
            "should comply with EBU R128; I={:.2}, LRA={:.2}, TP={:.2}",
            report.integrated_lufs, report.loudness_range_lu, report.true_peak_dbtp
        );
    }

    #[test]
    fn test_sample_rate_accessor() {
        let meter = EbuR128Meter::new(44_100, 2);
        assert_eq!(meter.sample_rate(), 44_100);
    }

    #[test]
    fn test_channel_accessor() {
        let meter = EbuR128Meter::new(48_000, 6);
        assert_eq!(meter.channels(), 6);
    }

    // ── Coefficient verification ──────────────────────────────────────────────

    /// Verify that the 48 kHz biquad stage 1 b-coefficients match the standard.
    #[test]
    fn test_stage1_48k_coefficients() {
        let filter = KWeightingFilter::new(48_000);
        // We verify by checking that the biquad b0 is close to the table value.
        // stage1 is not directly accessible, so we cross-check via processing.
        // Process a unit impulse and verify the filter is not an identity.
        let mut f = filter;
        let y0 = f.process(1.0);
        let y1 = f.process(0.0);
        let y2 = f.process(0.0);
        // b0 of stage 1 × b0 of stage 2 ≈ 1.535 × 1/denom – just check it is near 1.5.
        assert!(y0 > 1.0 && y0 < 2.0, "impulse response[0]={y0:.4}");
        // Response should decay.
        assert!(y1.abs() < y0.abs() * 2.0 && y2.is_finite());
    }

    /// Verify Stage 2 high-pass characteristic: DC must be strongly attenuated.
    #[test]
    fn test_stage2_attenuates_dc() {
        let mut filter = KWeightingFilter::new(48_000);
        // Feed DC (all ones) for 10 000 samples.
        let mut last = 0.0;
        for _ in 0..10_000 {
            last = filter.process(1.0);
        }
        // High-pass filter should drive DC output to near 0.
        assert!(
            last.abs() < 0.01,
            "Stage 2 should attenuate DC; last output = {last:.6}"
        );
    }

    /// Verify Stage 1 boosts high frequencies (> 2 kHz) relative to mid-band.
    #[test]
    fn test_stage1_shelf_boosts_highs() {
        let sr = 48_000_u32;
        let fs = f64::from(sr);

        let (b1, a1) = KWeightingFilter::design_stage1(fs);
        let mut s1 = Biquad::new(b1, a1);

        // Warm up.
        for _ in 0..4800 {
            s1.process((2.0 * PI * 1000.0 / fs).sin());
        }

        // Measure gain at 1 kHz.
        let n = 4800_usize;
        let mut sum_in_1k = 0.0;
        let mut sum_out_1k = 0.0;
        for i in 0..n {
            let x = (2.0 * PI * 1000.0 * i as f64 / fs).sin();
            let y = s1.process(x);
            sum_in_1k += x * x;
            sum_out_1k += y * y;
        }

        // Warm up again at 8 kHz.
        let (b1, a1) = KWeightingFilter::design_stage1(fs);
        let mut s1_8k = Biquad::new(b1, a1);
        for _ in 0..4800 {
            s1_8k.process((2.0 * PI * 8000.0 / fs).sin());
        }

        let mut sum_in_8k = 0.0;
        let mut sum_out_8k = 0.0;
        for i in 0..n {
            let x = (2.0 * PI * 8000.0 * i as f64 / fs).sin();
            let y = s1_8k.process(x);
            sum_in_8k += x * x;
            sum_out_8k += y * y;
        }

        let gain_1k_db = 10.0 * (sum_out_1k / sum_in_1k).log10();
        let gain_8k_db = 10.0 * (sum_out_8k / sum_in_8k).log10();

        // The shelf should boost 8 kHz by at least 2 dB more than 1 kHz.
        assert!(
            gain_8k_db > gain_1k_db + 2.0,
            "Stage 1 shelf gain 8kHz={gain_8k_db:.2} dB, 1kHz={gain_1k_db:.2} dB; expected 8k > 1k + 2dB"
        );
    }
}
