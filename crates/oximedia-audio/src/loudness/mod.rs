//! Loudness normalization for broadcast and streaming.
//!
//! This module provides comprehensive loudness measurement and normalization
//! according to international broadcast standards:
//!
//! - **EBU R128** - European Broadcasting Union loudness standard
//! - **ITU-R BS.1770-4** - International loudness measurement algorithm
//! - **ATSC A/85** - Advanced Television Systems Committee loudness standard
//!
//! # Features
//!
//! ## Loudness Measurement
//!
//! - Momentary loudness (400ms sliding window)
//! - Short-term loudness (3s sliding window)
//! - Integrated loudness (gated program loudness)
//! - Loudness range (LRA)
//! - True peak detection (prevents inter-sample clipping)
//!
//! ## K-Weighting Filter
//!
//! ITU-R BS.1770-4 K-weighting filter chain for perceptually accurate measurement:
//! - Pre-filter (high-pass at ~78 Hz)
//! - RLB filter (revised low-frequency B-weighting)
//!
//! ## Gating Algorithm
//!
//! Two-stage gating for integrated loudness:
//! - Absolute gate: -70 LUFS
//! - Relative gate: -10 LU below ungated loudness
//!
//! ## Normalization
//!
//! - Linear gain adjustment to target loudness
//! - True peak limiting for compliance
//! - Optional dynamic range compression
//! - Multi-pass analysis and processing
//! - Streaming support with pre-analyzed gain
//!
//! ## Reporting
//!
//! - Comprehensive measurement reports
//! - Compliance checking (EBU R128, ATSC A/85)
//! - Loudness history visualization
//! - Export to text, JSON, CSV
//!
//! # Example
//!
//! ```ignore
//! use oximedia_audio::loudness::{LoudnessMeter, LoudnessStandard};
//!
//! // Create a loudness meter
//! let mut meter = LoudnessMeter::new(LoudnessStandard::EbuR128, 48000.0, 2);
//!
//! // Process audio frames
//! for frame in audio_frames {
//!     let metrics = meter.measure(&frame);
//!     println!("Integrated: {:.1} LUFS", metrics.integrated_lufs);
//! }
//!
//! // Normalize to target loudness
//! let normalized = meter.normalize(&audio_frames, -23.0);
//!
//! // Generate report
//! let report = meter.report(duration_seconds);
//! println!("{}", report);
//! ```
//!
//! # Standards
//!
//! ## EBU R128
//!
//! - Target: -23 LUFS ±1 LU
//! - True peak: ≤ -1.0 dBTP
//! - Measurement: ITU-R BS.1770-4 algorithm
//!
//! ## ATSC A/85
//!
//! - Target: -24 LKFS ±2 dB
//! - True peak: typically ≤ -2.0 dBTP
//! - Measurement: Same as ITU-R BS.1770-4 (LKFS = LUFS)
//!
//! ## Streaming Platforms
//!
//! - Spotify: -14 LUFS
//! - YouTube: -14 LUFS
//! - Apple Music: -16 LUFS
//! - Tidal: -14 LUFS

#![forbid(unsafe_code)]

pub mod filter;
pub mod gate;
pub mod normalize;
pub mod peak;
pub mod r128;
pub mod report;
pub mod sample_bytes;

use crate::frame::AudioFrame;
use crate::AudioResult;

// Re-exports
pub use filter::{KWeightFilter, KWeightFilterBank};
pub use gate::{BlockAccumulator, GatingProcessor};
pub use normalize::{
    AutoGainConfig, AutoGainProcessor, BatchNormalizer, FileNormalizationStats, LoudnessNormalizer,
    NormalizationConfig, NormalizationMode, NormalizationParams, StreamingNormalizer,
};
pub use peak::{SamplePeakDetector, TruePeakDetector, TruePeakLimiter, TruePeakLimiterConfig};
pub use r128::{AtscA85Ext, AtscA85Meter, ComplianceStatus, R128Compliance, R128Meter};
pub use report::{
    AtscA85Compliance, EbuR128Compliance, LoudnessHistory, LoudnessReport, LoudnessStatistics,
    NormalizationReport,
};

/// Loudness measurement standard.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum LoudnessStandard {
    /// EBU R128 (European Broadcasting Union).
    ///
    /// Target: -23 LUFS ±1 LU
    /// True peak: ≤ -1.0 dBTP
    #[default]
    EbuR128,

    /// ATSC A/85 (Advanced Television Systems Committee).
    ///
    /// Target: -24 LKFS ±2 dB
    /// True peak: ≤ -2.0 dBTP
    AtscA85,

    /// Spotify streaming platform.
    ///
    /// Target: -14 LUFS
    Spotify,

    /// YouTube streaming platform.
    ///
    /// Target: -14 LUFS
    YouTube,

    /// Apple Music streaming platform.
    ///
    /// Target: -16 LUFS
    AppleMusic,

    /// Amazon Music streaming platform.
    ///
    /// Target: -14 LUFS
    AmazonMusic,

    /// Tidal streaming platform.
    ///
    /// Target: -14 LUFS
    Tidal,

    /// Custom target loudness.
    Custom(f64),
}

impl LoudnessStandard {
    /// Get the target loudness in LUFS for this standard.
    #[must_use]
    pub fn target_lufs(&self) -> f64 {
        match self {
            Self::EbuR128 => -23.0,
            Self::AtscA85 => -24.0,
            Self::Spotify => -14.0,
            Self::YouTube => -14.0,
            Self::AppleMusic => -16.0,
            Self::AmazonMusic => -14.0,
            Self::Tidal => -14.0,
            Self::Custom(target) => *target,
        }
    }

    /// Get the maximum true peak in dBTP for this standard.
    #[must_use]
    pub fn max_true_peak_dbtp(&self) -> f64 {
        match self {
            Self::EbuR128 => -1.0,
            Self::AtscA85 => -2.0,
            Self::Spotify | Self::YouTube | Self::AppleMusic | Self::AmazonMusic | Self::Tidal => {
                -1.0
            }
            Self::Custom(_) => -1.0,
        }
    }

    /// Get the tolerance in LU for this standard.
    #[must_use]
    pub fn tolerance_lu(&self) -> f64 {
        match self {
            Self::EbuR128 => 1.0,
            Self::AtscA85 => 2.0,
            Self::Spotify | Self::YouTube | Self::AppleMusic | Self::AmazonMusic | Self::Tidal => {
                1.0
            }
            Self::Custom(_) => 1.0,
        }
    }

    /// Get the standard name as a string.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::EbuR128 => "EBU R128",
            Self::AtscA85 => "ATSC A/85",
            Self::Spotify => "Spotify",
            Self::YouTube => "YouTube",
            Self::AppleMusic => "Apple Music",
            Self::AmazonMusic => "Amazon Music",
            Self::Tidal => "Tidal",
            Self::Custom(_) => "Custom",
        }
    }
}

/// Loudness measurement metrics.
#[derive(Clone, Debug)]
pub struct LoudnessMetrics {
    /// Momentary loudness in LUFS (400ms).
    pub momentary_lufs: f64,
    /// Short-term loudness in LUFS (3s).
    pub short_term_lufs: f64,
    /// Integrated loudness in LUFS (entire program).
    pub integrated_lufs: f64,
    /// Loudness range in LU.
    pub loudness_range: f64,
    /// True peak in dBTP.
    pub true_peak_dbtp: f64,
    /// True peak (linear).
    pub true_peak_linear: f64,
    /// Maximum momentary loudness seen.
    pub max_momentary: f64,
    /// Maximum short-term loudness seen.
    pub max_short_term: f64,
}

impl Default for LoudnessMetrics {
    fn default() -> Self {
        Self {
            momentary_lufs: f64::NEG_INFINITY,
            short_term_lufs: f64::NEG_INFINITY,
            integrated_lufs: f64::NEG_INFINITY,
            loudness_range: 0.0,
            true_peak_dbtp: f64::NEG_INFINITY,
            true_peak_linear: 0.0,
            max_momentary: f64::NEG_INFINITY,
            max_short_term: f64::NEG_INFINITY,
        }
    }
}

/// Unified loudness meter supporting multiple standards.
///
/// This is the main entry point for loudness measurement and normalization.
pub struct LoudnessMeter {
    /// Loudness standard being used.
    standard: LoudnessStandard,
    /// Internal R128 meter.
    meter: R128Meter,
    /// Loudness history for visualization.
    history: LoudnessHistory,
    /// Sample rate in Hz.
    sample_rate: f64,
    /// Number of channels.
    channels: usize,
}

impl LoudnessMeter {
    /// Create a new loudness meter.
    ///
    /// # Arguments
    ///
    /// * `standard` - Loudness standard to use
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of audio channels
    #[must_use]
    pub fn new(standard: LoudnessStandard, sample_rate: f64, channels: usize) -> Self {
        let meter = R128Meter::new(sample_rate, channels);
        let history = LoudnessHistory::new(0.1); // 100ms sample interval

        Self {
            standard,
            meter,
            history,
            sample_rate,
            channels,
        }
    }

    /// Measure loudness from an audio frame.
    ///
    /// # Arguments
    ///
    /// * `frame` - Audio frame to measure
    ///
    /// # Returns
    ///
    /// Current loudness metrics
    pub fn measure(&mut self, frame: &AudioFrame) -> LoudnessMetrics {
        // Extract samples from frame
        let samples = self.extract_samples(frame);

        // Process with meter
        self.meter.process_interleaved(&samples);

        // Update history
        let timestamp = frame.timestamp.to_seconds();
        self.history.add_sample(
            self.meter.momentary_loudness(),
            self.meter.short_term_loudness(),
            timestamp,
        );

        // Return current metrics
        self.get_metrics()
    }

    /// Get current loudness metrics without processing new frames.
    #[must_use]
    pub fn get_metrics(&self) -> LoudnessMetrics {
        LoudnessMetrics {
            momentary_lufs: self.meter.momentary_loudness(),
            short_term_lufs: self.meter.short_term_loudness(),
            integrated_lufs: self.meter.integrated_loudness(),
            loudness_range: self.meter.loudness_range(),
            true_peak_dbtp: self.meter.true_peak_dbtp(),
            true_peak_linear: self.meter.true_peak_linear(),
            max_momentary: self.meter.max_momentary(),
            max_short_term: self.meter.max_short_term(),
        }
    }

    /// Normalize audio frames to target loudness.
    ///
    /// # Arguments
    ///
    /// * `frames` - Mutable slice of audio frames
    /// * `target_lufs` - Target loudness in LUFS
    ///
    /// # Returns
    ///
    /// Normalization parameters applied
    pub fn normalize(&self, frames: &mut [AudioFrame], target_lufs: f64) -> NormalizationParams {
        let config = NormalizationConfig {
            target_lufs,
            max_true_peak_dbtp: self.standard.max_true_peak_dbtp(),
            ..Default::default()
        };

        let normalizer = LoudnessNormalizer::new(config, self.sample_rate, self.channels);
        normalizer.normalize(frames)
    }

    /// Normalize to the standard's target loudness.
    ///
    /// # Arguments
    ///
    /// * `frames` - Mutable slice of audio frames
    ///
    /// # Returns
    ///
    /// Normalization parameters applied
    pub fn normalize_to_standard(&self, frames: &mut [AudioFrame]) -> NormalizationParams {
        self.normalize(frames, self.standard.target_lufs())
    }

    /// Generate a comprehensive loudness report.
    ///
    /// # Arguments
    ///
    /// * `duration_seconds` - Total duration in seconds
    ///
    /// # Returns
    ///
    /// Loudness report
    #[must_use]
    pub fn report(&self, duration_seconds: f64) -> LoudnessReport {
        LoudnessReport::from_meter(&self.meter, duration_seconds)
    }

    /// Check compliance with the selected standard.
    #[must_use]
    pub fn check_compliance(&self) -> ComplianceStatus {
        let integrated = self.meter.integrated_loudness();
        let target = self.standard.target_lufs();
        let tolerance = self.standard.tolerance_lu();

        if integrated.is_infinite() {
            ComplianceStatus::Unknown
        } else if integrated >= target - tolerance && integrated <= target + tolerance {
            ComplianceStatus::Compliant
        } else if integrated > target + tolerance {
            ComplianceStatus::TooLoud(integrated - target)
        } else {
            ComplianceStatus::TooQuiet(target - integrated)
        }
    }

    /// Get the loudness history for visualization.
    #[must_use]
    pub fn history(&self) -> &LoudnessHistory {
        &self.history
    }

    /// Reset the meter.
    pub fn reset(&mut self) {
        self.meter.reset();
        self.history = LoudnessHistory::new(0.1);
    }

    /// Get the loudness standard being used.
    #[must_use]
    pub fn standard(&self) -> LoudnessStandard {
        self.standard
    }

    /// Set the loudness standard.
    pub fn set_standard(&mut self, standard: LoudnessStandard) {
        self.standard = standard;
    }

    /// Extract samples from an audio frame as interleaved f64.
    ///
    /// Decoding honours the frame's own
    /// [`SampleFormat`](oximedia_core::SampleFormat); planar frames are
    /// interleaved channel-by-channel.
    fn extract_samples(&self, frame: &AudioFrame) -> Vec<f64> {
        let format = frame.format;

        match &frame.samples {
            crate::frame::AudioBuffer::Interleaved(data) => {
                sample_bytes::decode(data, format).unwrap_or_default()
            }
            crate::frame::AudioBuffer::Planar(planes) => {
                if planes.is_empty() {
                    return Vec::new();
                }

                let decoded: Vec<Vec<f64>> = planes
                    .iter()
                    .map(|plane| sample_bytes::decode(plane, format).unwrap_or_default())
                    .collect();

                let channels = decoded.len();
                let frames = decoded.iter().map(Vec::len).min().unwrap_or(0);
                let mut interleaved = Vec::with_capacity(frames * channels);

                for frame_idx in 0..frames {
                    for plane in &decoded {
                        interleaved.push(plane[frame_idx]);
                    }
                }

                interleaved
            }
        }
    }
}

/// Quick loudness measurement function.
///
/// Measure loudness of audio frames without creating a persistent meter.
///
/// # Arguments
///
/// * `frames` - Slice of audio frames
/// * `sample_rate` - Sample rate in Hz
/// * `channels` - Number of channels
///
/// # Returns
///
/// Loudness metrics
pub fn measure_loudness(
    frames: &[AudioFrame],
    sample_rate: f64,
    channels: usize,
) -> AudioResult<LoudnessMetrics> {
    let mut meter = LoudnessMeter::new(LoudnessStandard::EbuR128, sample_rate, channels);

    for frame in frames {
        meter.measure(frame);
    }

    Ok(meter.get_metrics())
}

/// Quick normalization function.
///
/// Normalize audio frames to target loudness.
///
/// # Arguments
///
/// * `frames` - Mutable slice of audio frames
/// * `target_lufs` - Target loudness in LUFS
/// * `sample_rate` - Sample rate in Hz
/// * `channels` - Number of channels
///
/// # Returns
///
/// Normalization parameters
pub fn normalize_loudness(
    frames: &mut [AudioFrame],
    target_lufs: f64,
    sample_rate: f64,
    channels: usize,
) -> AudioResult<NormalizationParams> {
    let meter = LoudnessMeter::new(LoudnessStandard::Custom(target_lufs), sample_rate, channels);
    Ok(meter.normalize(frames, target_lufs))
}
