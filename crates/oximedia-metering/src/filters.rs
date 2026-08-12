//! K-weighting filters for ITU-R BS.1770-4 loudness measurement.
//!
//! This module implements the K-weighting filter chain consisting of:
//! - Stage 1: high-shelf pre-filter modelling the acoustic effect of the head
//!   (gain ≈ +4 dB above ≈ 2 kHz, f₀ ≈ 1 682 Hz, Q ≈ 0.7071)
//! - Stage 2: RLB (Revised Low-frequency B-weighting) **high-pass** filter
//!   (f₀ ≈ 38.135 Hz, Q ≈ 0.5003)
//!
//! Coefficients for 48 000 Hz are taken verbatim from Table 1 of
//! ITU-R BS.1770-4; for every other sample rate they are derived from the
//! analogue prototypes via the bilinear transform with frequency pre-warping.
//! The resulting chain magnitude response is −1.13 dB at 100 Hz, +0.69 dB at
//! 997 Hz and +3.97 dB at 4 kHz (see `test_k_weight_chain_matches_itu_table1`).

#![allow(clippy::many_single_char_names)]
#![allow(clippy::similar_names)]

use std::f64::consts::PI;

/// Reference sample rate for which the ITU-R BS.1770-4 Table 1 coefficients are given.
const REF_SAMPLE_RATE: f64 = 48_000.0;

/// K-weighting Stage 1 (high-shelf) feed-forward coefficients at 48 000 Hz
/// (ITU-R BS.1770-4 Table 1).
const K_STAGE1_B_48K: [f64; 3] = [
    1.535_124_859_586_97,
    -2.691_696_189_406_38,
    1.198_392_810_852_85,
];

/// K-weighting Stage 1 (high-shelf) feed-back coefficients at 48 000 Hz.
const K_STAGE1_A_48K: [f64; 2] = [-1.690_659_293_182_41, 0.732_480_774_215_85];

/// K-weighting Stage 2 (high-pass) feed-forward coefficients at 48 000 Hz
/// (ITU-R BS.1770-4 Table 1).
const K_STAGE2_B_48K: [f64; 3] = [1.0, -2.0, 1.0];

/// K-weighting Stage 2 (high-pass) feed-back coefficients at 48 000 Hz.
const K_STAGE2_A_48K: [f64; 2] = [-1.990_047_454_833_98, 0.990_072_250_366_21];

/// K-weighting filter for single channel.
///
/// Implements ITU-R BS.1770-4 K-weighting using cascaded biquad filters.
#[derive(Clone, Debug)]
pub struct KWeightFilter {
    sample_rate: f64,
    pre_filter: BiquadFilter,
    rlb_filter: BiquadFilter,
}

impl KWeightFilter {
    /// Create a new K-weighting filter.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Sample rate in Hz (8000-192000 Hz)
    pub fn new(sample_rate: f64) -> Self {
        let (pre_filter, rlb_filter) = if (sample_rate - REF_SAMPLE_RATE).abs() < 0.5 {
            // Exact ITU-R BS.1770-4 Table 1 coefficients.
            (
                BiquadFilter::from_ba(K_STAGE1_B_48K, K_STAGE1_A_48K),
                BiquadFilter::from_ba(K_STAGE2_B_48K, K_STAGE2_A_48K),
            )
        } else {
            let (b1, a1) = Self::design_pre_filter(sample_rate);
            let (b2, a2) = Self::design_rlb_filter(sample_rate);
            (BiquadFilter::from_ba(b1, a1), BiquadFilter::from_ba(b2, a2))
        };

        Self {
            sample_rate,
            pre_filter,
            rlb_filter,
        }
    }

    /// Process a single sample.
    ///
    /// # Arguments
    ///
    /// * `input` - Input sample
    ///
    /// # Returns
    ///
    /// K-weighted output sample
    pub fn process(&mut self, input: f64) -> f64 {
        let stage1 = self.pre_filter.process(input);
        self.rlb_filter.process(stage1)
    }

    /// Reset filter state to zero.
    pub fn reset(&mut self) {
        self.pre_filter.reset();
        self.rlb_filter.reset();
    }

    /// Design the pre-filter (Stage 1) via the bilinear transform.
    ///
    /// This is the **high-shelf** stage of ITU-R BS.1770-4 §4.1 that models the
    /// acoustic effect of the head: f₀ = 1 681.974 Hz, G = 3.999 84 dB (a *shelf
    /// gain in decibels*, not a linear scale factor) and Q = 0.707 175.
    ///
    /// Returns `([b0, b1, b2], [a1, a2])` with `a0 = 1` implicit.
    fn design_pre_filter(sample_rate: f64) -> ([f64; 3], [f64; 2]) {
        const F0: f64 = 1_681.974_450_955_533;
        const G_DB: f64 = 3.999_843_853_973_347;
        const Q: f64 = 0.707_175_236_955_420;

        // Pre-warp the analogue corner frequency.
        let k = (PI * F0 / sample_rate).tan();
        let k_sq = k * k;

        let vh = 10.0_f64.powf(G_DB / 20.0); // linear gain of the shelf
        let vb = vh.powf(0.5); // mid-band gain (geometric mean)

        let denom = 1.0 + k / Q + k_sq;

        let b0 = (vh + vb * k / Q + k_sq) / denom;
        let b1 = 2.0 * (k_sq - vh) / denom;
        let b2 = (vh - vb * k / Q + k_sq) / denom;
        let a1 = 2.0 * (k_sq - 1.0) / denom;
        let a2 = (1.0 - k / Q + k_sq) / denom;

        ([b0, b1, b2], [a1, a2])
    }

    /// Design the RLB filter (Stage 2) via the bilinear transform.
    ///
    /// This is the **high-pass** stage (revised low-frequency B-weighting) of
    /// ITU-R BS.1770-4: a second-order high-pass with f₀ = 38.135 Hz and
    /// Q = 0.500 327, i.e. `b = [1, −2, 1] / denom`.
    ///
    /// Returns `([b0, b1, b2], [a1, a2])` with `a0 = 1` implicit.
    fn design_rlb_filter(sample_rate: f64) -> ([f64; 3], [f64; 2]) {
        const F0: f64 = 38.135_470_876_024_44;
        const Q: f64 = 0.500_327_037_323_877;

        let k = (PI * F0 / sample_rate).tan();
        let k_sq = k * k;

        let denom = 1.0 + k / Q + k_sq;

        let b0 = 1.0 / denom;
        let b1 = -2.0 / denom;
        let b2 = 1.0 / denom;
        let a1 = 2.0 * (k_sq - 1.0) / denom;
        let a2 = (1.0 - k / Q + k_sq) / denom;

        ([b0, b1, b2], [a1, a2])
    }

    /// Get sample rate.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
}

/// Second-order IIR biquad filter.
///
/// Implements the difference equation:
/// y[n] = b0*x[n] + b1*x[n-1] + b2*x[n-2] - a1*y[n-1] - a2*y[n-2]
#[derive(Clone, Debug)]
struct BiquadFilter {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl BiquadFilter {
    /// Create a new biquad filter with given coefficients.
    fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Create a biquad from `b = [b0, b1, b2]` and `a = [a1, a2]` (`a0 = 1` implicit).
    fn from_ba(b: [f64; 3], a: [f64; 2]) -> Self {
        Self::new(b[0], b[1], b[2], a[0], a[1])
    }

    /// Process a single sample through the filter.
    fn process(&mut self, input: f64) -> f64 {
        let output = self.b0 * input + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;

        // Update delay line
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;

        output
    }

    /// Reset filter state.
    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Multi-channel K-weighting filter bank.
///
/// Maintains independent filter states for each audio channel.
#[derive(Clone, Debug)]
pub struct KWeightFilterBank {
    filters: Vec<KWeightFilter>,
    sample_rate: f64,
    channels: usize,
}

impl KWeightFilterBank {
    /// Create a new filter bank.
    ///
    /// # Arguments
    ///
    /// * `channels` - Number of audio channels
    /// * `sample_rate` - Sample rate in Hz
    pub fn new(channels: usize, sample_rate: f64) -> Self {
        let filters = (0..channels)
            .map(|_| KWeightFilter::new(sample_rate))
            .collect();

        Self {
            filters,
            sample_rate,
            channels,
        }
    }

    /// Process interleaved multi-channel audio.
    ///
    /// # Arguments
    ///
    /// * `input` - Interleaved input samples [L, R, L, R, ...]
    /// * `channels` - Number of channels
    /// * `output` - Output buffer for filtered samples
    ///
    /// # Returns
    ///
    /// Number of frames processed
    pub fn process_interleaved(
        &mut self,
        input: &[f64],
        channels: usize,
        output: &mut [f64],
    ) -> usize {
        if channels != self.channels || output.len() < input.len() {
            return 0;
        }

        let frames = input.len() / channels;

        for frame in 0..frames {
            for ch in 0..channels {
                let idx = frame * channels + ch;
                output[idx] = self.filters[ch].process(input[idx]);
            }
        }

        frames
    }

    /// Process planar multi-channel audio.
    ///
    /// # Arguments
    ///
    /// * `channels` - Mutable slice of per-channel sample buffers
    pub fn process_planar(&mut self, channels: &mut [Vec<f64>]) {
        for (ch_idx, samples) in channels.iter_mut().enumerate() {
            if ch_idx < self.filters.len() {
                for sample in samples {
                    *sample = self.filters[ch_idx].process(*sample);
                }
            }
        }
    }

    /// Reset all channel filters.
    pub fn reset(&mut self) {
        for filter in &mut self.filters {
            filter.reset();
        }
    }

    /// Get number of channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Get sample rate.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_k_weight_filter_creates() {
        let filter = KWeightFilter::new(48000.0);
        assert_eq!(filter.sample_rate(), 48000.0);
    }

    #[test]
    fn test_k_weight_filter_processes() {
        let mut filter = KWeightFilter::new(48000.0);
        let output = filter.process(0.5);
        assert!(output.is_finite());
    }

    #[test]
    fn test_filter_bank_interleaved() {
        let mut bank = KWeightFilterBank::new(2, 48000.0);
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let mut output = vec![0.0; 4];

        let frames = bank.process_interleaved(&input, 2, &mut output);
        assert_eq!(frames, 2);
        assert!(output.iter().all(|&x| x.is_finite()));
    }

    #[test]
    fn test_filter_reset() {
        let mut filter = KWeightFilter::new(48000.0);
        filter.process(0.5);
        filter.reset();

        // After reset, state should be zero
        let output = filter.process(0.0);
        assert_eq!(output, 0.0);
    }

    // ── ITU-R BS.1770-4 conformance ───────────────────────────────────────────

    /// Reference magnitude of the complete K-weighting chain, evaluated
    /// analytically from the ITU-R BS.1770-4 Table 1 coefficients at 48 kHz:
    ///
    /// | f (Hz) | \|H(f)\| (dB) |
    /// |--------|---------------|
    /// | 100    | −1.1335       |
    /// | 997    | +0.6910       |
    /// | 4 000  | +3.9680       |
    const ITU_TABLE1_RESPONSE_DB: [(f64, f64); 3] =
        [(100.0, -1.1335), (997.0, 0.6910), (4000.0, 3.9680)];

    /// Measure the steady-state power gain of the K-weighting chain at `freq_hz`.
    ///
    /// A full second of signal is measured after a 200 ms warm-up, so at both
    /// 48 000 Hz and 44 100 Hz every test frequency spans a whole number of
    /// periods and the RMS ratio equals `|H(f)|` exactly.
    fn measure_chain_gain_db(sample_rate: f64, freq_hz: f64) -> f64 {
        let mut filter = KWeightFilter::new(sample_rate);

        let warmup = (sample_rate / 5.0) as usize;
        for i in 0..warmup {
            let x = (2.0 * PI * freq_hz * i as f64 / sample_rate).sin();
            filter.process(x);
        }

        let n = sample_rate as usize;
        let mut sum_in = 0.0;
        let mut sum_out = 0.0;
        for i in warmup..(warmup + n) {
            let x = (2.0 * PI * freq_hz * i as f64 / sample_rate).sin();
            let y = filter.process(x);
            sum_in += x * x;
            sum_out += y * y;
        }

        10.0 * (sum_out / sum_in).log10()
    }

    /// The K-weighting chain must match the ITU-R BS.1770-4 Table 1 response
    /// within 0.1 dB at 100 Hz, 997 Hz and 4 kHz.
    ///
    /// Regression guard for the historical bug where Stage 1 was implemented as
    /// a high-pass scaled by the *decibel* value 3.9998 used as a linear gain and
    /// Stage 2 as a +1 dB high-shelf; that chain was ≈ −35 dB at 100 Hz and
    /// ≈ +9 dB at 4–10 kHz.
    #[test]
    fn test_k_weight_chain_matches_itu_table1() {
        for (freq, expected_db) in ITU_TABLE1_RESPONSE_DB {
            let measured = measure_chain_gain_db(48_000.0, freq);
            assert!(
                (measured - expected_db).abs() <= 0.1,
                "K-weighting at {freq} Hz = {measured:.4} dB, expected {expected_db:.4} dB (ITU-R BS.1770-4 Table 1)"
            );
        }
    }

    /// The analytically designed coefficients used for non-48 kHz rates must
    /// track the Table 1 response to the same 0.1 dB tolerance.
    #[test]
    fn test_k_weight_chain_matches_itu_table1_at_44100() {
        for (freq, expected_db) in ITU_TABLE1_RESPONSE_DB {
            let measured = measure_chain_gain_db(44_100.0, freq);
            assert!(
                (measured - expected_db).abs() <= 0.1,
                "K-weighting at {freq} Hz (44.1 kHz) = {measured:.4} dB, expected {expected_db:.4} dB"
            );
        }
    }

    /// Stage 2 is a high-pass: sustained DC must decay to (near) zero.
    #[test]
    fn test_k_weight_attenuates_dc() {
        let mut filter = KWeightFilter::new(48_000.0);
        let mut last = 0.0;
        for _ in 0..10_000 {
            last = filter.process(1.0);
        }
        assert!(
            last.abs() < 0.01,
            "K-weighting must attenuate DC; last output = {last:.6}"
        );
    }
}
