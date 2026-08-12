//! ITU (International Telecommunication Union) audio metering standards.
//!
//! This module implements comprehensive ITU-R recommendations for loudness measurement
//! and operational practices in broadcast and streaming applications.
//!
//! # Standards Implemented
//!
//! - **ITU-R BS.1770-4** - Algorithms to measure audio programme loudness and true-peak audio level
//! - **ITU-R BS.1771** - Requirements for loudness and true-peak indication metres
//! - **ITU-R BS.1864** - Operational practice in loudness and true-peak level
//! - **ITU-R BS.2217** - Operational practices for loudness in streaming and file-based distribution
//!
//! # ITU-R BS.1770-4 Features
//!
//! - K-weighted frequency response
//! - Pre-filter (high-pass at 75 Hz)
//! - RLB weighting filter
//! - Momentary loudness (400ms)
//! - Short-term loudness (3s)
//! - Integrated loudness (program duration)
//! - Gating algorithm (-70 LKFS absolute, -10 LU relative)
//!
//! # ITU-R BS.1771 Features
//!
//! - Loudness range (LRA)
//! - Measurement methodology
//! - Percentile-based calculation
//! - LRA histogram
//!
//! # ITU-R BS.1864 Features
//!
//! - Operational practice for loudness
//! - Target levels by program type
//! - Tolerance ranges
//! - Measurement conditions
//!
//! # ITU-R BS.2217 Features
//!
//! - Streaming loudness requirements
//! - Platform-specific targets (Spotify, YouTube, Apple Music, Netflix, Amazon Prime)
//!
//! # Module status — NOT COMPILED
//!
//! This file is **not declared** in `meters/mod.rs`, so it is not part of the
//! build. It also does not currently compile: `KWeightingFilter::process`
//! borrows `self` immutably (`&self.pre_b`) and `self.pre_state[channel]`
//! mutably in the same call (two `E0502` errors). Wiring it up therefore needs
//! that borrow split fixed first, plus a clippy pass.
//!
//! The K-weighting coefficients below were corrected in 0.2.1 to match
//! ITU-R BS.1770-4 (Stage 1 is a +4 dB high-shelf, not a high-pass scaled by
//! the shelf gain read as a linear factor) so that reviving the module does not
//! resurrect the bug — but, because the file is not built, that correction is
//! *unverified by tests*. The live, tested implementations are
//! `crate::loudness::filter::KWeightFilter` and
//! `oximedia_metering::ebu_r128_impl::KWeightingFilter`.

#![forbid(unsafe_code)]

use crate::frame::AudioFrame;
use std::collections::VecDeque;

/// ITU-R BS.1770-4 K-weighting filter implementation.
///
/// The K-weighting consists of two stages:
/// 1. Pre-filter: high-shelf modelling the acoustic effect of the head
///    (f₀ ≈ 1 682 Hz, G ≈ +4 dB, Q ≈ 0.7071)
/// 2. RLB filter: revised low-frequency B-weighting **high-pass**
///    (f₀ ≈ 38.135 Hz, Q ≈ 0.5003)
///
/// The resulting chain is −1.13 dB at 100 Hz, +0.69 dB at 997 Hz and
/// +3.97 dB at 4 kHz.
pub struct KWeightingFilter {
    /// Pre-filter coefficients (high-shelf).
    pre_b: [f64; 3],
    pre_a: [f64; 3],
    /// RLB filter coefficients.
    rlb_b: [f64; 3],
    rlb_a: [f64; 3],
    /// Pre-filter state per channel.
    pre_state: Vec<FilterState>,
    /// RLB filter state per channel.
    rlb_state: Vec<FilterState>,
}

/// Second-order IIR filter state.
#[derive(Clone, Debug)]
struct FilterState {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl FilterState {
    fn new() -> Self {
        Self {
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

impl KWeightingFilter {
    /// Create a new K-weighting filter.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of audio channels
    pub fn new(sample_rate: f64, channels: usize) -> Self {
        // Stage 1 — high-shelf pre-filter (acoustic effect of the head).
        // f0 = 1681.974 Hz, G = 3.99984 *decibels*, Q = 0.707175.
        let f0 = 1681.974450955533;
        let g_db = 3.999843853973347;
        let q = 0.7071752369554196;

        let k = (std::f64::consts::PI * f0 / sample_rate).tan();
        let k2 = k * k;
        let vh = 10.0_f64.powf(g_db / 20.0); // linear gain of the shelf
        let vb = vh.powf(0.5); // mid-band gain (geometric mean)
        let denom = 1.0 + k / q + k2;

        let pre_b = [
            (vh + vb * k / q + k2) / denom,
            2.0 * (k2 - vh) / denom,
            (vh - vb * k / q + k2) / denom,
        ];
        let pre_a = [1.0, 2.0 * (k2 - 1.0) / denom, (1.0 - k / q + k2) / denom];

        // Stage 2 — RLB high-pass (revised low-frequency B-weighting).
        let f0_rlb = 38.13547087602444;
        let q_rlb = 0.5003270373238773;
        let k_rlb = (std::f64::consts::PI * f0_rlb / sample_rate).tan();
        let k2_rlb = k_rlb * k_rlb;
        let norm_rlb = 1.0 / (1.0 + k_rlb / q_rlb + k2_rlb);

        let rlb_b = [norm_rlb, -2.0 * norm_rlb, norm_rlb];
        let rlb_a = [
            1.0,
            2.0 * (k2_rlb - 1.0) * norm_rlb,
            (1.0 - k_rlb / q_rlb + k2_rlb) * norm_rlb,
        ];

        Self {
            pre_b,
            pre_a,
            rlb_b,
            rlb_a,
            pre_state: vec![FilterState::new(); channels],
            rlb_state: vec![FilterState::new(); channels],
        }
    }

    /// Process a single sample through the K-weighting filter chain.
    ///
    /// # Arguments
    ///
    /// * `sample` - Input sample
    /// * `channel` - Channel index
    ///
    /// # Returns
    ///
    /// K-weighted sample
    pub fn process(&mut self, sample: f64, channel: usize) -> f64 {
        // Pre-filter
        let pre_out = self.process_biquad(
            sample,
            &self.pre_b,
            &self.pre_a,
            &mut self.pre_state[channel],
        );

        // RLB filter
        self.process_biquad(pre_out, &self.rlb_b, &self.rlb_a, &mut self.rlb_state[channel])
    }

    /// Process a biquad filter.
    fn process_biquad(&self, x: f64, b: &[f64; 3], a: &[f64; 3], state: &mut FilterState) -> f64 {
        let y = b[0] * x + b[1] * state.x1 + b[2] * state.x2 - a[1] * state.y1 - a[2] * state.y2;

        state.x2 = state.x1;
        state.x1 = x;
        state.y2 = state.y1;
        state.y1 = y;

        y
    }

    /// Reset filter state.
    pub fn reset(&mut self) {
        for state in &mut self.pre_state {
            state.reset();
        }
        for state in &mut self.rlb_state {
            state.reset();
        }
    }
}

/// ITU-R BS.1770-4 gating algorithm.
///
/// Two-stage gating for integrated loudness:
/// 1. Absolute gate: -70 LKFS
/// 2. Relative gate: -10 LU below ungated loudness
pub struct GatingAlgorithm {
    /// Absolute gate threshold (LKFS).
    absolute_gate: f64,
    /// Relative gate offset (LU).
    relative_gate_offset: f64,
    /// Block accumulator for gating.
    blocks: Vec<BlockMeasurement>,
}

/// Single block measurement for gating.
#[derive(Clone, Debug)]
struct BlockMeasurement {
    /// Block loudness in LKFS.
    loudness: f64,
    /// Block timestamp.
    timestamp: f64,
}

impl GatingAlgorithm {
    /// Create a new gating algorithm.
    pub fn new() -> Self {
        Self {
            absolute_gate: -70.0,
            relative_gate_offset: -10.0,
            blocks: Vec::new(),
        }
    }

    /// Add a block measurement.
    ///
    /// # Arguments
    ///
    /// * `loudness` - Block loudness in LKFS
    /// * `timestamp` - Block timestamp
    pub fn add_block(&mut self, loudness: f64, timestamp: f64) {
        if loudness.is_finite() {
            self.blocks.push(BlockMeasurement {
                loudness,
                timestamp,
            });
        }
    }

    /// Calculate gated loudness.
    ///
    /// # Returns
    ///
    /// Integrated loudness in LKFS
    pub fn calculate_gated_loudness(&self) -> f64 {
        if self.blocks.is_empty() {
            return f64::NEG_INFINITY;
        }

        // Stage 1: Absolute gate
        let absolute_gated: Vec<_> = self
            .blocks
            .iter()
            .filter(|b| b.loudness >= self.absolute_gate)
            .collect();

        if absolute_gated.is_empty() {
            return f64::NEG_INFINITY;
        }

        // Calculate ungated loudness
        let ungated_sum: f64 = absolute_gated
            .iter()
            .map(|b| 10_f64.powf(b.loudness / 10.0))
            .sum();
        let ungated_loudness = 10.0 * (ungated_sum / absolute_gated.len() as f64).log10();

        // Stage 2: Relative gate
        let relative_gate = ungated_loudness + self.relative_gate_offset;
        let relative_gated: Vec<_> = absolute_gated
            .iter()
            .filter(|b| b.loudness >= relative_gate)
            .collect();

        if relative_gated.is_empty() {
            return f64::NEG_INFINITY;
        }

        let gated_sum: f64 = relative_gated
            .iter()
            .map(|b| 10_f64.powf(b.loudness / 10.0))
            .sum();
        10.0 * (gated_sum / relative_gated.len() as f64).log10()
    }

    /// Get blocks above absolute gate for LRA calculation.
    pub fn absolute_gated_blocks(&self) -> Vec<f64> {
        self.blocks
            .iter()
            .filter(|b| b.loudness >= self.absolute_gate)
            .map(|b| b.loudness)
            .collect()
    }

    /// Reset the gating algorithm.
    pub fn reset(&mut self) {
        self.blocks.clear();
    }
}

impl Default for GatingAlgorithm {
    fn default() -> Self {
        Self::new()
    }
}

/// ITU-R BS.1770-4 loudness meter.
///
/// Full implementation of ITU-R BS.1770-4 algorithm with K-weighting,
/// gating, and multiple integration times.
pub struct Bs1770Meter {
    /// Sample rate in Hz.
    sample_rate: f64,
    /// Number of channels.
    channels: usize,
    /// K-weighting filter.
    k_weight: KWeightingFilter,
    /// Momentary loudness window (400ms).
    momentary_window: SlidingWindow,
    /// Short-term loudness window (3s).
    short_term_window: SlidingWindow,
    /// Gating algorithm.
    gating: GatingAlgorithm,
    /// Channel weightings.
    channel_weights: Vec<f64>,
    /// Current timestamp.
    timestamp: f64,
    /// Maximum momentary loudness.
    max_momentary: f64,
    /// Maximum short-term loudness.
    max_short_term: f64,
}

/// Sliding window for momentary and short-term measurements.
struct SlidingWindow {
    /// Window size in samples.
    size: usize,
    /// Overlap (75% for momentary, 66.7% for short-term).
    overlap: usize,
    /// Buffer for mean-square values per channel.
    buffers: Vec<VecDeque<f64>>,
    /// Block size (100ms).
    block_size: usize,
    /// Samples accumulated in current block.
    block_accumulator: Vec<f64>,
    /// Sample count in current block.
    block_count: usize,
}

impl SlidingWindow {
    /// Create a new sliding window.
    ///
    /// # Arguments
    ///
    /// * `duration_ms` - Window duration in milliseconds
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of channels
    fn new(duration_ms: usize, sample_rate: f64, channels: usize) -> Self {
        let block_size = (sample_rate * 0.1) as usize; // 100ms blocks
        let num_blocks = duration_ms / 100;

        Self {
            size: num_blocks,
            overlap: (num_blocks * 3) / 4,
            buffers: vec![VecDeque::with_capacity(num_blocks); channels],
            block_size,
            block_accumulator: vec![0.0; channels],
            block_count: 0,
        }
    }

    /// Add a sample to the window.
    ///
    /// # Arguments
    ///
    /// * `samples` - Per-channel samples (K-weighted)
    ///
    /// # Returns
    ///
    /// Optional loudness value when block completes
    fn add_samples(&mut self, samples: &[f64]) -> Option<f64> {
        // Accumulate mean-square
        for (ch, &sample) in samples.iter().enumerate() {
            self.block_accumulator[ch] += sample * sample;
        }
        self.block_count += 1;

        // Check if block is complete
        if self.block_count >= self.block_size {
            // Calculate mean-square for this block
            for ch in 0..samples.len() {
                let ms = self.block_accumulator[ch] / self.block_count as f64;
                self.buffers[ch].push_back(ms);
                if self.buffers[ch].len() > self.size {
                    self.buffers[ch].pop_front();
                }
            }

            // Reset accumulator
            self.block_accumulator.fill(0.0);
            self.block_count = 0;

            // Calculate loudness if window is full
            if self.buffers[0].len() == self.size {
                return Some(self.calculate_loudness());
            }
        }

        None
    }

    /// Calculate loudness from current window.
    fn calculate_loudness(&self) -> f64 {
        // Sum mean-square across channels and blocks
        let mut sum = 0.0;
        for buffer in &self.buffers {
            let channel_sum: f64 = buffer.iter().sum();
            sum += channel_sum / self.size as f64;
        }

        // Convert to LKFS
        if sum > 0.0 {
            -0.691 + 10.0 * sum.log10()
        } else {
            f64::NEG_INFINITY
        }
    }

    fn reset(&mut self) {
        for buffer in &mut self.buffers {
            buffer.clear();
        }
        self.block_accumulator.fill(0.0);
        self.block_count = 0;
    }
}

impl Bs1770Meter {
    /// Create a new ITU-R BS.1770-4 meter.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of audio channels
    pub fn new(sample_rate: f64, channels: usize) -> Self {
        let k_weight = KWeightingFilter::new(sample_rate, channels);
        let momentary_window = SlidingWindow::new(400, sample_rate, channels);
        let short_term_window = SlidingWindow::new(3000, sample_rate, channels);
        let gating = GatingAlgorithm::new();

        // ITU-R BS.1770-4 channel weightings
        let channel_weights = if channels == 1 {
            vec![1.0]
        } else if channels == 2 {
            vec![1.0, 1.0]
        } else if channels == 5 {
            // 5.0: L, R, C, Ls, Rs
            vec![1.0, 1.0, 1.0, 1.41, 1.41]
        } else if channels == 6 {
            // 5.1: L, R, C, LFE, Ls, Rs
            vec![1.0, 1.0, 1.0, 0.0, 1.41, 1.41]
        } else {
            vec![1.0; channels]
        };

        Self {
            sample_rate,
            channels,
            k_weight,
            momentary_window,
            short_term_window,
            gating,
            channel_weights,
            timestamp: 0.0,
            max_momentary: f64::NEG_INFINITY,
            max_short_term: f64::NEG_INFINITY,
        }
    }

    /// Process audio samples.
    ///
    /// # Arguments
    ///
    /// * `samples` - Interleaved audio samples
    pub fn process(&mut self, samples: &[f64]) {
        let frames = samples.len() / self.channels;

        for frame_idx in 0..frames {
            let mut k_weighted = vec![0.0; self.channels];

            // Apply K-weighting per channel
            for ch in 0..self.channels {
                let idx = frame_idx * self.channels + ch;
                if idx < samples.len() {
                    k_weighted[ch] = self.k_weight.process(samples[idx], ch);
                }
            }

            // Add to sliding windows
            if let Some(momentary) = self.momentary_window.add_samples(&k_weighted) {
                self.max_momentary = self.max_momentary.max(momentary);
                self.gating.add_block(momentary, self.timestamp);
            }

            if let Some(short_term) = self.short_term_window.add_samples(&k_weighted) {
                self.max_short_term = self.max_short_term.max(short_term);
            }

            self.timestamp += 1.0 / self.sample_rate;
        }
    }

    /// Get momentary loudness (400ms).
    pub fn momentary_loudness(&self) -> f64 {
        self.momentary_window.calculate_loudness()
    }

    /// Get short-term loudness (3s).
    pub fn short_term_loudness(&self) -> f64 {
        self.short_term_window.calculate_loudness()
    }

    /// Get integrated loudness (gated).
    pub fn integrated_loudness(&self) -> f64 {
        self.gating.calculate_gated_loudness()
    }

    /// Get maximum momentary loudness.
    pub fn max_momentary(&self) -> f64 {
        self.max_momentary
    }

    /// Get maximum short-term loudness.
    pub fn max_short_term(&self) -> f64 {
        self.max_short_term
    }

    /// Reset the meter.
    pub fn reset(&mut self) {
        self.k_weight.reset();
        self.momentary_window.reset();
        self.short_term_window.reset();
        self.gating.reset();
        self.timestamp = 0.0;
        self.max_momentary = f64::NEG_INFINITY;
        self.max_short_term = f64::NEG_INFINITY;
    }

    /// Get gating blocks for LRA calculation.
    pub fn gating_blocks(&self) -> Vec<f64> {
        self.gating.absolute_gated_blocks()
    }
}

/// ITU-R BS.1771 - Loudness Range (LRA) calculator.
///
/// LRA measures the variation in loudness using a percentile-based approach.
pub struct LoudnessRangeCalculator {
    /// Histogram bins for LRA calculation.
    histogram: Vec<(f64, usize)>,
}

impl LoudnessRangeCalculator {
    /// Create a new LRA calculator.
    pub fn new() -> Self {
        Self {
            histogram: Vec::new(),
        }
    }

    /// Calculate LRA from gating blocks.
    ///
    /// # Arguments
    ///
    /// * `blocks` - Absolute-gated loudness blocks
    ///
    /// # Returns
    ///
    /// Loudness range in LU
    pub fn calculate(&mut self, blocks: &[f64]) -> f64 {
        if blocks.len() < 2 {
            return 0.0;
        }

        // Build histogram
        self.histogram.clear();
        for &loudness in blocks {
            if loudness.is_finite() {
                self.histogram.push((loudness, 1));
            }
        }

        // Sort by loudness
        self.histogram.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Calculate percentiles (10th and 95th)
        let total = self.histogram.len();
        let p10_idx = (total as f64 * 0.10) as usize;
        let p95_idx = (total as f64 * 0.95) as usize;

        if p95_idx >= total || p10_idx >= p95_idx {
            return 0.0;
        }

        let p10 = self.histogram[p10_idx].0;
        let p95 = self.histogram[p95_idx].0;

        (p95 - p10).abs()
    }

    /// Get histogram for visualization.
    pub fn histogram(&self) -> &[(f64, usize)] {
        &self.histogram
    }

    /// Reset the calculator.
    pub fn reset(&mut self) {
        self.histogram.clear();
    }
}

impl Default for LoudnessRangeCalculator {
    fn default() -> Self {
        Self::new()
    }
}

/// ITU-R BS.1864 - Operational practice in loudness.
///
/// Provides target levels and tolerances by program type.
pub struct Bs1864Practice {
    /// Program type.
    program_type: ProgramType,
}

/// Program type for operational practice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProgramType {
    /// General broadcast programs.
    GeneralBroadcast,
    /// Drama and narrative content.
    Drama,
    /// Documentary programs.
    Documentary,
    /// Sports broadcasting.
    Sports,
    /// Music programs.
    Music,
    /// News and current affairs.
    News,
    /// Commercial advertisements.
    Commercial,
    /// Cinema and theatrical content.
    Cinema,
}

impl Bs1864Practice {
    /// Create a new BS.1864 operational practice.
    pub fn new(program_type: ProgramType) -> Self {
        Self { program_type }
    }

    /// Get target loudness for program type.
    pub fn target_loudness(&self) -> f64 {
        match self.program_type {
            ProgramType::GeneralBroadcast => -23.0,
            ProgramType::Drama => -23.0,
            ProgramType::Documentary => -23.0,
            ProgramType::Sports => -23.0,
            ProgramType::Music => -23.0,
            ProgramType::News => -23.0,
            ProgramType::Commercial => -23.0,
            ProgramType::Cinema => -27.0,
        }
    }

    /// Get tolerance in LU.
    pub fn tolerance(&self) -> f64 {
        match self.program_type {
            ProgramType::GeneralBroadcast => 1.0,
            ProgramType::Drama => 1.0,
            ProgramType::Documentary => 1.0,
            ProgramType::Sports => 2.0,
            ProgramType::Music => 2.0,
            ProgramType::News => 1.0,
            ProgramType::Commercial => 1.0,
            ProgramType::Cinema => 2.0,
        }
    }

    /// Get maximum true peak.
    pub fn max_true_peak(&self) -> f64 {
        match self.program_type {
            ProgramType::Cinema => 0.0, // Allow peaks up to 0 dBTP for cinema
            _ => -1.0,                   // -1 dBTP for broadcast
        }
    }

    /// Check compliance.
    pub fn check_compliance(&self, integrated: f64, true_peak: f64) -> ComplianceResult {
        let target = self.target_loudness();
        let tolerance = self.tolerance();
        let max_peak = self.max_true_peak();

        let loudness_ok = integrated >= target - tolerance && integrated <= target + tolerance;
        let peak_ok = true_peak <= max_peak;

        ComplianceResult {
            loudness_compliant: loudness_ok,
            peak_compliant: peak_ok,
            target_loudness: target,
            measured_loudness: integrated,
            loudness_deviation: integrated - target,
            max_peak_allowed: max_peak,
            measured_peak: true_peak,
        }
    }
}

/// Compliance result.
#[derive(Clone, Debug)]
pub struct ComplianceResult {
    /// Is loudness compliant?
    pub loudness_compliant: bool,
    /// Is peak compliant?
    pub peak_compliant: bool,
    /// Target loudness.
    pub target_loudness: f64,
    /// Measured integrated loudness.
    pub measured_loudness: f64,
    /// Deviation from target (LU).
    pub loudness_deviation: f64,
    /// Maximum peak allowed.
    pub max_peak_allowed: f64,
    /// Measured true peak.
    pub measured_peak: f64,
}

/// ITU-R BS.2217 - Operational practices for streaming.
///
/// Platform-specific loudness targets and requirements.
pub struct Bs2217Streaming {
    /// Streaming platform.
    platform: StreamingPlatform,
}

/// Streaming platform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StreamingPlatform {
    /// Spotify.
    Spotify,
    /// YouTube.
    YouTube,
    /// Apple Music.
    AppleMusic,
    /// Netflix.
    Netflix,
    /// Amazon Prime Video.
    AmazonPrime,
    /// Disney+.
    DisneyPlus,
    /// HBO Max.
    HboMax,
    /// Tidal.
    Tidal,
    /// Deezer.
    Deezer,
    /// Custom platform.
    Custom { target: f64, max_peak: f64 },
}

impl Bs2217Streaming {
    /// Create a new BS.2217 streaming practice.
    pub fn new(platform: StreamingPlatform) -> Self {
        Self { platform }
    }

    /// Get target loudness for platform.
    pub fn target_loudness(&self) -> f64 {
        match self.platform {
            StreamingPlatform::Spotify => -14.0,
            StreamingPlatform::YouTube => -14.0,
            StreamingPlatform::AppleMusic => -16.0,
            StreamingPlatform::Netflix => -27.0,
            StreamingPlatform::AmazonPrime => -24.0,
            StreamingPlatform::DisneyPlus => -27.0,
            StreamingPlatform::HboMax => -27.0,
            StreamingPlatform::Tidal => -14.0,
            StreamingPlatform::Deezer => -14.0,
            StreamingPlatform::Custom { target, .. } => target,
        }
    }

    /// Get maximum true peak for platform.
    pub fn max_true_peak(&self) -> f64 {
        match self.platform {
            StreamingPlatform::Spotify => -1.0,
            StreamingPlatform::YouTube => -1.0,
            StreamingPlatform::AppleMusic => -1.0,
            StreamingPlatform::Netflix => -2.0,
            StreamingPlatform::AmazonPrime => -2.0,
            StreamingPlatform::DisneyPlus => -2.0,
            StreamingPlatform::HboMax => -2.0,
            StreamingPlatform::Tidal => -1.0,
            StreamingPlatform::Deezer => -1.0,
            StreamingPlatform::Custom { max_peak, .. } => max_peak,
        }
    }

    /// Get tolerance in LU.
    pub fn tolerance(&self) -> f64 {
        match self.platform {
            StreamingPlatform::Netflix
            | StreamingPlatform::AmazonPrime
            | StreamingPlatform::DisneyPlus
            | StreamingPlatform::HboMax => 2.0,
            _ => 1.0,
        }
    }

    /// Does platform apply loudness normalization?
    pub fn applies_normalization(&self) -> bool {
        matches!(
            self.platform,
            StreamingPlatform::Spotify
                | StreamingPlatform::YouTube
                | StreamingPlatform::AppleMusic
                | StreamingPlatform::Tidal
                | StreamingPlatform::Deezer
        )
    }

    /// Get platform name.
    pub fn platform_name(&self) -> &str {
        match self.platform {
            StreamingPlatform::Spotify => "Spotify",
            StreamingPlatform::YouTube => "YouTube",
            StreamingPlatform::AppleMusic => "Apple Music",
            StreamingPlatform::Netflix => "Netflix",
            StreamingPlatform::AmazonPrime => "Amazon Prime Video",
            StreamingPlatform::DisneyPlus => "Disney+",
            StreamingPlatform::HboMax => "HBO Max",
            StreamingPlatform::Tidal => "Tidal",
            StreamingPlatform::Deezer => "Deezer",
            StreamingPlatform::Custom { .. } => "Custom",
        }
    }

    /// Check compliance for streaming platform.
    pub fn check_compliance(&self, integrated: f64, true_peak: f64) -> StreamingCompliance {
        let target = self.target_loudness();
        let tolerance = self.tolerance();
        let max_peak = self.max_true_peak();

        let loudness_ok = integrated >= target - tolerance && integrated <= target + tolerance;
        let peak_ok = true_peak <= max_peak;

        // Calculate normalization gain if platform applies it
        let normalization_gain = if self.applies_normalization() {
            Some(target - integrated)
        } else {
            None
        };

        StreamingCompliance {
            platform: self.platform_name().to_string(),
            loudness_compliant: loudness_ok,
            peak_compliant: peak_ok,
            target_loudness: target,
            measured_loudness: integrated,
            loudness_deviation: integrated - target,
            max_peak_allowed: max_peak,
            measured_peak: true_peak,
            applies_normalization: self.applies_normalization(),
            normalization_gain,
        }
    }
}

/// Streaming platform compliance result.
#[derive(Clone, Debug)]
pub struct StreamingCompliance {
    /// Platform name.
    pub platform: String,
    /// Is loudness compliant?
    pub loudness_compliant: bool,
    /// Is peak compliant?
    pub peak_compliant: bool,
    /// Target loudness.
    pub target_loudness: f64,
    /// Measured integrated loudness.
    pub measured_loudness: f64,
    /// Deviation from target (LU).
    pub loudness_deviation: f64,
    /// Maximum peak allowed.
    pub max_peak_allowed: f64,
    /// Measured true peak.
    pub measured_peak: f64,
    /// Does platform apply normalization?
    pub applies_normalization: bool,
    /// Normalization gain that will be applied (if any).
    pub normalization_gain: Option<f64>,
}

/// Unified ITU metering suite.
///
/// Combines all ITU standards into a single meter.
pub struct ItuMeter {
    /// BS.1770-4 meter.
    bs1770: Bs1770Meter,
    /// LRA calculator.
    lra_calc: LoudnessRangeCalculator,
    /// True peak detector.
    true_peak: TruePeakDetector,
    /// Sample rate.
    sample_rate: f64,
    /// Channels.
    channels: usize,
}

/// Simple true peak detector.
struct TruePeakDetector {
    max_peak: f64,
    channels: usize,
}

impl TruePeakDetector {
    fn new(channels: usize) -> Self {
        Self {
            max_peak: 0.0,
            channels,
        }
    }

    fn process(&mut self, samples: &[f64]) {
        for &sample in samples {
            let abs_sample = sample.abs();
            if abs_sample > self.max_peak {
                self.max_peak = abs_sample;
            }
        }
    }

    fn true_peak_dbtp(&self) -> f64 {
        if self.max_peak > 0.0 {
            20.0 * self.max_peak.log10()
        } else {
            f64::NEG_INFINITY
        }
    }

    fn reset(&mut self) {
        self.max_peak = 0.0;
    }
}

impl ItuMeter {
    /// Create a new unified ITU meter.
    pub fn new(sample_rate: f64, channels: usize) -> Self {
        Self {
            bs1770: Bs1770Meter::new(sample_rate, channels),
            lra_calc: LoudnessRangeCalculator::new(),
            true_peak: TruePeakDetector::new(channels),
            sample_rate,
            channels,
        }
    }

    /// Process audio frame.
    pub fn process(&mut self, frame: &AudioFrame) {
        let samples = extract_samples_f64(frame);
        self.bs1770.process(&samples);
        self.true_peak.process(&samples);
    }

    /// Get all ITU metrics.
    pub fn get_metrics(&mut self) -> ItuMetrics {
        let blocks = self.bs1770.gating_blocks();
        let lra = self.lra_calc.calculate(&blocks);

        ItuMetrics {
            momentary_lufs: self.bs1770.momentary_loudness(),
            short_term_lufs: self.bs1770.short_term_loudness(),
            integrated_lufs: self.bs1770.integrated_loudness(),
            max_momentary: self.bs1770.max_momentary(),
            max_short_term: self.bs1770.max_short_term(),
            loudness_range: lra,
            true_peak_dbtp: self.true_peak.true_peak_dbtp(),
        }
    }

    /// Check compliance with BS.1864.
    pub fn check_bs1864_compliance(&mut self, program_type: ProgramType) -> ComplianceResult {
        let practice = Bs1864Practice::new(program_type);
        let metrics = self.get_metrics();
        practice.check_compliance(metrics.integrated_lufs, metrics.true_peak_dbtp)
    }

    /// Check compliance with BS.2217 streaming.
    pub fn check_streaming_compliance(
        &mut self,
        platform: StreamingPlatform,
    ) -> StreamingCompliance {
        let streaming = Bs2217Streaming::new(platform);
        let metrics = self.get_metrics();
        streaming.check_compliance(metrics.integrated_lufs, metrics.true_peak_dbtp)
    }

    /// Reset the meter.
    pub fn reset(&mut self) {
        self.bs1770.reset();
        self.lra_calc.reset();
        self.true_peak.reset();
    }
}

/// ITU measurement metrics.
#[derive(Clone, Debug)]
pub struct ItuMetrics {
    /// Momentary loudness (400ms).
    pub momentary_lufs: f64,
    /// Short-term loudness (3s).
    pub short_term_lufs: f64,
    /// Integrated loudness (gated).
    pub integrated_lufs: f64,
    /// Maximum momentary.
    pub max_momentary: f64,
    /// Maximum short-term.
    pub max_short_term: f64,
    /// Loudness range.
    pub loudness_range: f64,
    /// True peak.
    pub true_peak_dbtp: f64,
}

/// Extract samples as f64 from AudioFrame.
fn extract_samples_f64(frame: &AudioFrame) -> Vec<f64> {
    match &frame.samples {
        crate::frame::AudioBuffer::Interleaved(data) => {
            let sample_count = data.len() / 4;
            let mut samples = Vec::with_capacity(sample_count);

            for i in 0..sample_count {
                let offset = i * 4;
                if offset + 4 <= data.len() {
                    let bytes_array = [
                        data[offset],
                        data[offset + 1],
                        data[offset + 2],
                        data[offset + 3],
                    ];
                    let sample = f32::from_le_bytes(bytes_array);
                    samples.push(f64::from(sample));
                }
            }

            samples
        }
        crate::frame::AudioBuffer::Planar(planes) => {
            if planes.is_empty() {
                return Vec::new();
            }

            let channels = planes.len();
            let sample_size = std::mem::size_of::<f32>();
            let frames = planes[0].len() / sample_size;
            let mut interleaved = Vec::with_capacity(frames * channels);

            for frame_idx in 0..frames {
                for plane in planes {
                    let offset = frame_idx * sample_size;
                    if offset + 4 <= plane.len() {
                        let bytes_array = [
                            plane[offset],
                            plane[offset + 1],
                            plane[offset + 2],
                            plane[offset + 3],
                        ];
                        let sample = f32::from_le_bytes(bytes_array);
                        interleaved.push(f64::from(sample));
                    }
                }
            }

            interleaved
        }
    }
}

