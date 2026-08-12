//! Loudness normalization processor.
//!
//! Implements multi-pass loudness normalization with optional
//! dynamic range compression and true peak limiting.

#![forbid(unsafe_code)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::too_many_arguments)]

use super::peak::TruePeakDetector;
use super::r128::R128Meter;
use super::sample_bytes;
use crate::frame::{AudioBuffer, AudioFrame};

/// Loudness normalization mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NormalizationMode {
    /// Simple linear gain adjustment only.
    LinearGain,
    /// Linear gain with true peak limiting.
    #[default]
    LimitedGain,
    /// Dynamic range compression followed by gain adjustment.
    DynamicCompression,
    /// Full processing: compression, gain, and limiting.
    Full,
}

/// Configuration for loudness normalization.
#[derive(Clone, Debug)]
pub struct NormalizationConfig {
    /// Target loudness in LUFS.
    pub target_lufs: f64,
    /// Maximum true peak in dBTP (e.g., -1.0).
    pub max_true_peak_dbtp: f64,
    /// Normalization mode.
    pub mode: NormalizationMode,
    /// Enable dynamic range compression.
    pub enable_compression: bool,
    /// Compression threshold in LU below target.
    pub compression_threshold_lu: f64,
    /// Compression ratio.
    pub compression_ratio: f64,
    /// Enable true peak limiting.
    pub enable_limiting: bool,
    /// Limiter attack time in milliseconds.
    pub limiter_attack_ms: f64,
    /// Limiter release time in milliseconds.
    pub limiter_release_ms: f64,
}

impl Default for NormalizationConfig {
    fn default() -> Self {
        Self {
            target_lufs: -23.0, // EBU R128 target
            max_true_peak_dbtp: -1.0,
            mode: NormalizationMode::LimitedGain,
            enable_compression: false,
            compression_threshold_lu: 10.0,
            compression_ratio: 3.0,
            enable_limiting: true,
            limiter_attack_ms: 1.0,
            limiter_release_ms: 100.0,
        }
    }
}

impl NormalizationConfig {
    /// Create config for EBU R128 compliance.
    #[must_use]
    pub fn ebu_r128() -> Self {
        Self {
            target_lufs: -23.0,
            max_true_peak_dbtp: -1.0,
            mode: NormalizationMode::LimitedGain,
            enable_compression: false,
            compression_threshold_lu: 10.0,
            compression_ratio: 3.0,
            enable_limiting: true,
            limiter_attack_ms: 0.5,
            limiter_release_ms: 100.0,
        }
    }

    /// Create config for ATSC A/85 compliance.
    #[must_use]
    pub fn atsc_a85() -> Self {
        Self {
            target_lufs: -24.0,
            max_true_peak_dbtp: -2.0,
            mode: NormalizationMode::LimitedGain,
            enable_compression: false,
            compression_threshold_lu: 10.0,
            compression_ratio: 3.0,
            enable_limiting: true,
            limiter_attack_ms: 1.0,
            limiter_release_ms: 100.0,
        }
    }

    /// Create config for streaming platforms (Spotify, YouTube, etc.).
    #[must_use]
    pub fn streaming() -> Self {
        Self {
            target_lufs: -14.0,
            max_true_peak_dbtp: -1.0,
            mode: NormalizationMode::LimitedGain,
            enable_compression: false,
            compression_threshold_lu: 8.0,
            compression_ratio: 2.5,
            enable_limiting: true,
            limiter_attack_ms: 0.1,
            limiter_release_ms: 80.0,
        }
    }

    /// Create config with custom target loudness.
    #[must_use]
    pub fn custom(target_lufs: f64) -> Self {
        Self {
            target_lufs,
            ..Default::default()
        }
    }
}

/// Loudness normalization processor.
///
/// Performs multi-pass analysis and processing:
/// 1. Analysis pass: Measure integrated loudness
/// 2. Calculate required gain
/// 3. Processing pass: Apply gain and limiting
pub struct LoudnessNormalizer {
    /// Normalization configuration.
    config: NormalizationConfig,
    /// Sample rate in Hz.
    sample_rate: f64,
    /// Number of channels.
    channels: usize,
}

impl LoudnessNormalizer {
    /// Create a new loudness normalizer.
    ///
    /// # Arguments
    ///
    /// * `config` - Normalization configuration
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of audio channels
    #[must_use]
    pub fn new(config: NormalizationConfig, sample_rate: f64, channels: usize) -> Self {
        Self {
            config,
            sample_rate,
            channels,
        }
    }

    /// Analyze audio and calculate required normalization parameters.
    ///
    /// # Arguments
    ///
    /// * `frames` - Slice of audio frames to analyze
    ///
    /// # Returns
    ///
    /// Normalization parameters
    pub fn analyze(&self, frames: &[AudioFrame]) -> NormalizationParams {
        let mut meter = R128Meter::new(self.sample_rate, self.channels);

        // Process all frames
        for frame in frames {
            let samples = self.extract_samples(frame);
            meter.process_interleaved(&samples);
        }

        let measured_lufs = meter.integrated_loudness();
        let measured_peak_dbtp = meter.true_peak_dbtp();
        let lra = meter.loudness_range();

        // Calculate gain adjustment
        let gain_db = if measured_lufs.is_finite() {
            self.config.target_lufs - measured_lufs
        } else {
            0.0
        };

        // Predict peak after gain
        let predicted_peak_dbtp = measured_peak_dbtp + gain_db;

        // Calculate limiting gain if needed
        let limiting_gain_db = if predicted_peak_dbtp > self.config.max_true_peak_dbtp {
            self.config.max_true_peak_dbtp - predicted_peak_dbtp
        } else {
            0.0
        };

        NormalizationParams {
            measured_lufs,
            target_lufs: self.config.target_lufs,
            gain_db,
            limiting_gain_db,
            total_gain_db: gain_db + limiting_gain_db,
            measured_peak_dbtp,
            predicted_peak_dbtp: predicted_peak_dbtp + limiting_gain_db,
            loudness_range: lra,
            frames_modified: 0,
        }
    }

    /// Normalize audio frames to target loudness, rewriting their samples.
    ///
    /// The measured gain is applied to the sample data of every frame **in
    /// place**, in the frame's own [`SampleFormat`](oximedia_core::SampleFormat)
    /// (all integer and float formats listed in
    /// [`super::sample_bytes`] are supported). If true peak limiting is enabled
    /// and the gained signal would exceed the configured ceiling, a brick-wall
    /// limiter is applied afterwards — also written back.
    ///
    /// [`NormalizationParams::frames_modified`] reports how many frames were
    /// actually rewritten, so a caller can detect frames whose sample format
    /// could not be written.
    ///
    /// # Arguments
    ///
    /// * `frames` - Mutable slice of audio frames to normalize
    ///
    /// # Returns
    ///
    /// Normalization parameters used
    pub fn normalize(&self, frames: &mut [AudioFrame]) -> NormalizationParams {
        // Analysis pass
        let mut params = self.analyze(frames);

        if params.total_gain_db.abs() < 0.01 {
            // No normalization needed; nothing is rewritten.
            return params;
        }

        // Processing pass
        let linear_gain = Self::db_to_linear(params.gain_db);

        let mut modified = 0_usize;
        for frame in frames.iter_mut() {
            if Self::apply_gain(frame, linear_gain) {
                modified += 1;
            }
        }

        // Apply limiting if needed and enabled
        if self.config.enable_limiting && params.limiting_gain_db < -0.1 {
            Self::apply_limiting(frames, self.config.max_true_peak_dbtp);
        }

        params.frames_modified = modified;
        params
    }

    /// Apply a linear gain to an audio frame, writing the result back into the
    /// frame's byte buffer.
    ///
    /// Returns `true` if the frame's samples were rewritten, `false` if the
    /// frame's sample format is not supported by [`super::sample_bytes`] (in
    /// which case the frame is left untouched rather than silently corrupted).
    fn apply_gain(frame: &mut AudioFrame, linear_gain: f64) -> bool {
        Self::map_frame_samples(frame, |sample| sample * linear_gain)
    }

    /// Apply true peak limiting to frames, writing the result back.
    fn apply_limiting(frames: &mut [AudioFrame], max_peak_dbtp: f64) {
        let max_peak_linear = TruePeakDetector::dbtp_to_linear(max_peak_dbtp);

        for frame in frames {
            Self::map_frame_samples(frame, |sample| {
                if sample.abs() > max_peak_linear {
                    sample.signum() * max_peak_linear
                } else {
                    sample
                }
            });
        }
    }

    /// Apply `op` to every sample of `frame`, decoding and re-encoding the raw
    /// bytes in the frame's own sample format.
    ///
    /// Returns `false` (leaving the frame untouched) if the format cannot be
    /// decoded or encoded.
    fn map_frame_samples<F: Fn(f64) -> f64>(frame: &mut AudioFrame, op: F) -> bool {
        let format = frame.format;

        match &mut frame.samples {
            AudioBuffer::Interleaved(data) => {
                let Some(mut samples) = sample_bytes::decode(data, format) else {
                    return false;
                };
                for sample in &mut samples {
                    *sample = op(*sample);
                }
                let Some(encoded) = sample_bytes::encode(&samples, format) else {
                    return false;
                };
                *data = encoded;
                true
            }
            AudioBuffer::Planar(planes) => {
                let mut all_ok = true;
                for plane in planes.iter_mut() {
                    let Some(mut samples) = sample_bytes::decode(plane, format) else {
                        all_ok = false;
                        continue;
                    };
                    for sample in &mut samples {
                        *sample = op(*sample);
                    }
                    if let Some(encoded) = sample_bytes::encode(&samples, format) {
                        *plane = encoded;
                    } else {
                        all_ok = false;
                    }
                }
                all_ok
            }
        }
    }

    /// Extract samples from an audio frame as interleaved f64.
    ///
    /// Planar frames are interleaved channel-by-channel; the shortest plane
    /// bounds the number of frames produced.
    fn extract_samples(&self, frame: &AudioFrame) -> Vec<f64> {
        let format = frame.format;

        match &frame.samples {
            AudioBuffer::Interleaved(data) => {
                sample_bytes::decode(data, format).unwrap_or_default()
            }
            AudioBuffer::Planar(planes) => {
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

    /// Convert dB to linear gain.
    #[must_use]
    pub fn db_to_linear(db: f64) -> f64 {
        10.0_f64.powf(db / 20.0)
    }

    /// Convert linear gain to dB.
    #[must_use]
    pub fn linear_to_db(linear: f64) -> f64 {
        if linear <= 0.0 {
            f64::NEG_INFINITY
        } else {
            20.0 * linear.log10()
        }
    }
}

/// Normalization parameters from analysis.
#[derive(Clone, Debug)]
pub struct NormalizationParams {
    /// Measured integrated loudness in LUFS.
    pub measured_lufs: f64,
    /// Target loudness in LUFS.
    pub target_lufs: f64,
    /// Calculated gain adjustment in dB.
    pub gain_db: f64,
    /// Additional limiting gain in dB (negative).
    pub limiting_gain_db: f64,
    /// Total gain to apply in dB.
    pub total_gain_db: f64,
    /// Measured true peak in dBTP.
    pub measured_peak_dbtp: f64,
    /// Predicted true peak after normalization in dBTP.
    pub predicted_peak_dbtp: f64,
    /// Measured loudness range in LU.
    pub loudness_range: f64,
    /// Number of frames whose sample data was actually rewritten by
    /// [`LoudnessNormalizer::normalize`].
    ///
    /// Zero after a pure [`LoudnessNormalizer::analyze`] call, and zero when the
    /// required gain is below the 0.01 dB no-op threshold.
    pub frames_modified: usize,
}

impl NormalizationParams {
    /// Check if normalization will clip.
    #[must_use]
    pub fn will_clip(&self, max_peak_dbtp: f64) -> bool {
        self.predicted_peak_dbtp > max_peak_dbtp
    }

    /// Get loudness difference from target.
    #[must_use]
    pub fn loudness_delta(&self) -> f64 {
        self.measured_lufs - self.target_lufs
    }
}

/// Single-pass loudness normalizer with streaming support.
///
/// Applies a fixed gain without analysis (requires pre-analyzed gain).
pub struct StreamingNormalizer {
    /// Linear gain to apply.
    linear_gain: f64,
    /// Maximum peak level (linear).
    max_peak: f64,
    /// True peak detector.
    peak_detector: TruePeakDetector,
}

impl StreamingNormalizer {
    /// Create a new streaming normalizer.
    ///
    /// # Arguments
    ///
    /// * `gain_db` - Gain to apply in dB
    /// * `max_peak_dbtp` - Maximum true peak in dBTP
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of channels
    #[must_use]
    pub fn new(gain_db: f64, max_peak_dbtp: f64, sample_rate: f64, channels: usize) -> Self {
        let linear_gain = LoudnessNormalizer::db_to_linear(gain_db);
        let max_peak = TruePeakDetector::dbtp_to_linear(max_peak_dbtp);
        let peak_detector = TruePeakDetector::new(sample_rate, channels);

        Self {
            linear_gain,
            max_peak,
            peak_detector,
        }
    }

    /// Process a buffer of interleaved samples.
    ///
    /// # Arguments
    ///
    /// * `samples` - Mutable interleaved samples
    pub fn process_interleaved(&mut self, samples: &mut [f64]) {
        // Apply gain
        for sample in samples.iter_mut() {
            *sample *= self.linear_gain;

            // Simple limiting
            if sample.abs() > self.max_peak {
                *sample = sample.signum() * self.max_peak;
            }
        }

        // Update peak detector
        self.peak_detector.process_interleaved(samples);
    }

    /// Process planar samples.
    ///
    /// # Arguments
    ///
    /// * `channels` - Mutable slice of per-channel sample buffers
    pub fn process_planar(&mut self, channels: &mut [Vec<f64>]) {
        for ch_samples in channels {
            for sample in ch_samples.iter_mut() {
                *sample *= self.linear_gain;

                // Simple limiting
                if sample.abs() > self.max_peak {
                    *sample = sample.signum() * self.max_peak;
                }
            }
        }
    }

    /// Get the current true peak in dBTP.
    #[must_use]
    pub fn current_peak_dbtp(&self) -> f64 {
        TruePeakDetector::linear_to_dbtp(
            self.peak_detector
                .get_all_peaks()
                .iter()
                .fold(0.0, |a, &b| a.max(b)),
        )
    }

    /// Reset peak detector.
    pub fn reset(&mut self) {
        self.peak_detector.reset();
    }
}

/// Batch loudness normalizer for processing multiple files to consistent loudness.
pub struct BatchNormalizer {
    /// Target loudness in LUFS.
    target_lufs: f64,
    /// Maximum true peak in dBTP.
    max_peak_dbtp: f64,
    /// Normalization statistics for all files.
    file_stats: Vec<FileNormalizationStats>,
}

impl BatchNormalizer {
    /// Create a new batch normalizer.
    ///
    /// # Arguments
    ///
    /// * `target_lufs` - Target loudness in LUFS
    /// * `max_peak_dbtp` - Maximum true peak in dBTP
    #[must_use]
    pub fn new(target_lufs: f64, max_peak_dbtp: f64) -> Self {
        Self {
            target_lufs,
            max_peak_dbtp,
            file_stats: Vec::new(),
        }
    }

    /// Analyze a file and add to batch.
    ///
    /// # Arguments
    ///
    /// * `file_id` - File identifier
    /// * `measured_lufs` - Measured integrated loudness
    /// * `measured_peak_dbtp` - Measured true peak
    pub fn add_file(&mut self, file_id: String, measured_lufs: f64, measured_peak_dbtp: f64) {
        let gain_db = self.target_lufs - measured_lufs;
        let predicted_peak = measured_peak_dbtp + gain_db;

        let limiting_gain = if predicted_peak > self.max_peak_dbtp {
            self.max_peak_dbtp - predicted_peak
        } else {
            0.0
        };

        self.file_stats.push(FileNormalizationStats {
            file_id,
            measured_lufs,
            measured_peak_dbtp,
            gain_db,
            limiting_gain_db: limiting_gain,
            total_gain_db: gain_db + limiting_gain,
        });
    }

    /// Get normalization parameters for a file.
    ///
    /// # Arguments
    ///
    /// * `file_id` - File identifier
    #[must_use]
    pub fn get_file_params(&self, file_id: &str) -> Option<&FileNormalizationStats> {
        self.file_stats.iter().find(|s| s.file_id == file_id)
    }

    /// Get all file statistics.
    #[must_use]
    pub fn all_stats(&self) -> &[FileNormalizationStats] {
        &self.file_stats
    }

    /// Calculate album/playlist normalization gain.
    ///
    /// Uses the loudest file to determine gain for all files.
    #[must_use]
    pub fn calculate_album_gain(&self) -> f64 {
        if self.file_stats.is_empty() {
            return 0.0;
        }

        // Find loudest file
        let max_lufs = self
            .file_stats
            .iter()
            .map(|s| s.measured_lufs)
            .fold(f64::NEG_INFINITY, f64::max);

        self.target_lufs - max_lufs
    }
}

/// Normalization statistics for a single file.
#[derive(Clone, Debug)]
pub struct FileNormalizationStats {
    /// File identifier.
    pub file_id: String,
    /// Measured integrated loudness in LUFS.
    pub measured_lufs: f64,
    /// Measured true peak in dBTP.
    pub measured_peak_dbtp: f64,
    /// Calculated gain in dB.
    pub gain_db: f64,
    /// Limiting gain in dB.
    pub limiting_gain_db: f64,
    /// Total gain in dB.
    pub total_gain_db: f64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Auto-gain processor
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for [`AutoGainProcessor`].
#[derive(Debug, Clone)]
pub struct AutoGainConfig {
    /// Target output level in dBFS (e.g. −6.0 for −6 dBFS).
    pub target_db: f64,
    /// Attack time constant in seconds (how fast gain is increased).
    pub attack_secs: f64,
    /// Release time constant in seconds (how fast gain is reduced).
    pub release_secs: f64,
    /// Sample rate in Hz.
    pub sample_rate: f64,
    /// Maximum allowed gain in dB (prevents extreme boosting of silence).
    pub max_gain_db: f64,
    /// Minimum allowed gain in dB (prevents extreme attenuation).
    pub min_gain_db: f64,
}

impl Default for AutoGainConfig {
    fn default() -> Self {
        Self {
            target_db: -6.0,
            attack_secs: 0.1,
            release_secs: 1.0,
            sample_rate: 48_000.0,
            max_gain_db: 20.0,
            min_gain_db: -40.0,
        }
    }
}

/// Real-time auto-gain processor.
///
/// Continuously adjusts the output gain so that the short-term RMS level of
/// the programme material tracks a configurable target level.  The processor
/// uses separate attack and release time constants so that gain *reduction*
/// (when the signal is too loud) happens faster than gain *boost* (when the
/// signal is too quiet), which gives a natural sounding result and avoids the
/// "pumping" artefact of a badly tuned AGC.
///
/// # Algorithm
///
/// 1. For each block of samples, compute the short-term RMS level.
/// 2. Calculate the ideal gain to reach `target_db`.
/// 3. Smooth the gain using attack/release coefficients.
/// 4. Clamp the gain to `[min_gain_db, max_gain_db]`.
/// 5. Apply the smoothed linear gain to the output.
pub struct AutoGainProcessor {
    config: AutoGainConfig,
    /// Current smoothed gain (linear, not dB).
    current_gain: f64,
    /// Attack coefficient (0..1, closer to 1 = slower).
    attack_coeff: f64,
    /// Release coefficient (0..1, closer to 1 = slower).
    release_coeff: f64,
}

impl AutoGainProcessor {
    /// Create a new auto-gain processor.
    #[must_use]
    pub fn new(config: AutoGainConfig) -> Self {
        let attack_coeff = Self::time_to_coeff(config.attack_secs, config.sample_rate);
        let release_coeff = Self::time_to_coeff(config.release_secs, config.sample_rate);
        Self {
            current_gain: 1.0,
            attack_coeff,
            release_coeff,
            config,
        }
    }

    /// Compute a one-pole IIR time constant coefficient.
    fn time_to_coeff(time_secs: f64, sample_rate: f64) -> f64 {
        if time_secs <= 0.0 || sample_rate <= 0.0 {
            return 0.0;
        }
        (-1.0_f64 / (time_secs * sample_rate)).exp()
    }

    /// Convert dB to linear.
    #[inline]
    fn db_to_linear(db: f64) -> f64 {
        10.0_f64.powf(db / 20.0)
    }

    /// Convert linear to dB (returns -∞ for zero).
    #[inline]
    fn linear_to_db(linear: f64) -> f64 {
        if linear <= 0.0 {
            f64::NEG_INFINITY
        } else {
            20.0 * linear.log10()
        }
    }

    /// Process a block of interleaved `f64` samples in-place.
    ///
    /// The block size determines the granularity of the RMS measurement: use
    /// a size of around 128–1 024 samples for most applications.
    pub fn process_block(&mut self, samples: &mut [f64]) {
        if samples.is_empty() {
            return;
        }

        // Compute short-term RMS of the input block.
        let sum_sq: f64 = samples.iter().map(|&s| s * s).sum();
        let rms = (sum_sq / samples.len() as f64).sqrt();

        // Compute the ideal gain to reach the target.
        let target_linear = Self::db_to_linear(self.config.target_db);
        let ideal_gain = if rms > 1e-10 {
            (target_linear / rms).clamp(
                Self::db_to_linear(self.config.min_gain_db),
                Self::db_to_linear(self.config.max_gain_db),
            )
        } else {
            // Signal is effectively silent — clamp to max gain to avoid explosion.
            Self::db_to_linear(self.config.max_gain_db)
        };

        // Smooth towards ideal_gain with separate attack/release.
        if ideal_gain < self.current_gain {
            // Gain needs to fall — use release coefficient (gain reduction = fast).
            self.current_gain =
                self.release_coeff * self.current_gain + (1.0 - self.release_coeff) * ideal_gain;
        } else {
            // Gain needs to rise — use attack coefficient (gain boost = slow).
            self.current_gain =
                self.attack_coeff * self.current_gain + (1.0 - self.attack_coeff) * ideal_gain;
        }

        // Apply the smoothed gain.
        for s in samples.iter_mut() {
            *s *= self.current_gain;
        }
    }

    /// Process a block of interleaved `f32` samples in-place.
    #[allow(clippy::cast_possible_truncation)]
    pub fn process_block_f32(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }

        let sum_sq: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
        let rms = (sum_sq / samples.len() as f64).sqrt();

        let target_linear = Self::db_to_linear(self.config.target_db);
        let ideal_gain = if rms > 1e-10 {
            (target_linear / rms).clamp(
                Self::db_to_linear(self.config.min_gain_db),
                Self::db_to_linear(self.config.max_gain_db),
            )
        } else {
            Self::db_to_linear(self.config.max_gain_db)
        };

        if ideal_gain < self.current_gain {
            self.current_gain =
                self.release_coeff * self.current_gain + (1.0 - self.release_coeff) * ideal_gain;
        } else {
            self.current_gain =
                self.attack_coeff * self.current_gain + (1.0 - self.attack_coeff) * ideal_gain;
        }

        for s in samples.iter_mut() {
            *s = (*s as f64 * self.current_gain) as f32;
        }
    }

    /// Get the current gain in dB.
    #[must_use]
    pub fn current_gain_db(&self) -> f64 {
        Self::linear_to_db(self.current_gain)
    }

    /// Get the current linear gain.
    #[must_use]
    pub fn current_gain(&self) -> f64 {
        self.current_gain
    }

    /// Reset the processor to unity gain.
    pub fn reset(&mut self) {
        self.current_gain = 1.0;
    }

    /// Update the target output level (dBFS) at runtime.
    pub fn set_target_db(&mut self, target_db: f64) {
        self.config.target_db = target_db;
    }

    /// Update the attack time constant.
    pub fn set_attack(&mut self, attack_secs: f64) {
        self.config.attack_secs = attack_secs;
        self.attack_coeff = Self::time_to_coeff(attack_secs, self.config.sample_rate);
    }

    /// Update the release time constant.
    pub fn set_release(&mut self, release_secs: f64) {
        self.config.release_secs = release_secs;
        self.release_coeff = Self::time_to_coeff(release_secs, self.config.sample_rate);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LoudnessNormalizer tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod normalizer_tests {
    use super::*;
    use crate::frame::ChannelLayout;
    use oximedia_core::SampleFormat;
    use std::f64::consts::PI;

    const SAMPLE_RATE: f64 = 48_000.0;

    /// Build a stereo (dual-mono) sine as a sequence of 0.5 s audio frames in
    /// `format`.
    fn sine_frames(
        peak_dbfs: f64,
        duration_secs: f64,
        format: SampleFormat,
        planar: bool,
    ) -> Vec<AudioFrame> {
        let amplitude = 10.0_f64.powf(peak_dbfs / 20.0);
        let per_frame = (SAMPLE_RATE * 0.5) as usize;
        let total = (SAMPLE_RATE * duration_secs) as usize;

        let mut frames = Vec::new();
        let mut start = 0_usize;
        while start < total {
            let end = (start + per_frame).min(total);
            let mono: Vec<f64> = (start..end)
                .map(|n| amplitude * (2.0 * PI * 997.0 * n as f64 / SAMPLE_RATE).sin())
                .collect();

            let mut frame = AudioFrame::new(format, SAMPLE_RATE as u32, ChannelLayout::Stereo);
            frame.samples = if planar {
                let plane = sample_bytes::encode(&mono, format).expect("encodable");
                AudioBuffer::Planar(vec![plane.clone(), plane])
            } else {
                let interleaved: Vec<f64> = mono.iter().flat_map(|&s| [s, s]).collect();
                AudioBuffer::Interleaved(
                    sample_bytes::encode(&interleaved, format).expect("encodable"),
                )
            };
            frames.push(frame);
            start = end;
        }
        frames
    }

    /// Re-measure the integrated loudness of a frame sequence with the
    /// (corrected) K-weighted R128 meter.
    fn measure_lufs(frames: &[AudioFrame], normalizer: &LoudnessNormalizer) -> f64 {
        let mut meter = R128Meter::new(SAMPLE_RATE, 2);
        for frame in frames {
            meter.process_interleaved(&normalizer.extract_samples(frame));
        }
        meter.integrated_loudness()
    }

    fn normalizer_for(target_lufs: f64) -> LoudnessNormalizer {
        let config = NormalizationConfig {
            target_lufs,
            max_true_peak_dbtp: -1.0,
            mode: NormalizationMode::LinearGain,
            enable_limiting: false,
            ..Default::default()
        };
        LoudnessNormalizer::new(config, SAMPLE_RATE, 2)
    }

    /// `normalize()` must actually rewrite the frame bytes, and the re-measured
    /// integrated loudness must land on target.
    ///
    /// Regression guard: `apply_gain` used to decode the samples, scale them in
    /// a temporary `Vec`, and drop it — returning correct-looking parameters
    /// while leaving the audio byte-identical.
    #[test]
    fn test_normalize_rewrites_samples_and_hits_target() {
        let normalizer = normalizer_for(-23.0);
        let mut frames = sine_frames(-30.0, 4.0, SampleFormat::F32, false);
        let original = frames.clone();

        let before = measure_lufs(&frames, &normalizer);
        assert!(before.is_finite(), "input loudness must be measurable");

        let params = normalizer.normalize(&mut frames);
        assert!(
            params.gain_db.abs() > 1.0,
            "a -30 dBFS sine needs real gain to reach -23 LUFS, got {:.2} dB",
            params.gain_db
        );
        assert_eq!(
            params.frames_modified,
            frames.len(),
            "every frame must be rewritten"
        );

        // The bytes must have changed.
        let mut changed = 0;
        for (before_frame, after_frame) in original.iter().zip(frames.iter()) {
            if let (AudioBuffer::Interleaved(a), AudioBuffer::Interleaved(b)) =
                (&before_frame.samples, &after_frame.samples)
            {
                if a != b {
                    changed += 1;
                }
            }
        }
        assert_eq!(changed, frames.len(), "all frames must differ after gain");

        let after = measure_lufs(&frames, &normalizer);
        assert!(
            (after - (-23.0)).abs() <= 0.5,
            "re-measured loudness {after:.2} LUFS should be within 0.5 LU of -23.0 (was {before:.2})"
        );
    }

    /// The write-back must work for every integer and float sample format,
    /// interleaved and planar.
    #[test]
    fn test_normalize_all_sample_formats() {
        let formats = [
            (SampleFormat::S16, false),
            (SampleFormat::S16p, true),
            (SampleFormat::S24, false),
            (SampleFormat::S24p, true),
            (SampleFormat::S32, false),
            (SampleFormat::F32, false),
            (SampleFormat::F32p, true),
            (SampleFormat::F64, false),
            (SampleFormat::U8, false),
        ];

        for (format, planar) in formats {
            let normalizer = normalizer_for(-20.0);
            let mut frames = sine_frames(-26.0, 4.0, format, planar);
            let original = frames.clone();

            let params = normalizer.normalize(&mut frames);
            assert_eq!(
                params.frames_modified,
                frames.len(),
                "{format:?}: every frame must be rewritten"
            );

            let mut any_changed = false;
            for (a, b) in original.iter().zip(frames.iter()) {
                match (&a.samples, &b.samples) {
                    (AudioBuffer::Interleaved(x), AudioBuffer::Interleaved(y)) => {
                        any_changed |= x != y;
                    }
                    (AudioBuffer::Planar(x), AudioBuffer::Planar(y)) => {
                        any_changed |= x != y;
                    }
                    _ => panic!("{format:?}: buffer kind changed"),
                }
            }
            assert!(any_changed, "{format:?}: samples were not modified");

            let after = measure_lufs(&frames, &normalizer);
            // 8-bit PCM quantisation noise dominates at -20 LUFS, so it gets a
            // wider window than the 0.5 LU used for the other formats.
            let tolerance = if format == SampleFormat::U8 { 1.5 } else { 0.5 };
            assert!(
                (after - (-20.0)).abs() <= tolerance,
                "{format:?}: re-measured {after:.2} LUFS, expected -20.0 ±{tolerance}"
            );
        }
    }

    /// Turning a loud signal down must also work (negative gain).
    #[test]
    fn test_normalize_attenuates_loud_input() {
        let normalizer = normalizer_for(-23.0);
        let mut frames = sine_frames(-6.0, 4.0, SampleFormat::F32, false);

        let params = normalizer.normalize(&mut frames);
        assert!(
            params.gain_db < -5.0,
            "expected attenuation, got {:.2} dB",
            params.gain_db
        );

        let after = measure_lufs(&frames, &normalizer);
        assert!(
            (after - (-23.0)).abs() <= 0.5,
            "re-measured {after:.2} LUFS, expected -23.0 ±0.5"
        );
    }

    /// True peak limiting must be written back too.
    #[test]
    fn test_normalize_with_limiting_clamps_samples() {
        // A dual-mono 997 Hz sine measures ≈ its peak dBFS in LUFS (ITU-R
        // BS.1770-4 sums the per-channel mean squares), so a 0 LUFS target on a
        // -20 dBFS source needs ≈ +20 dB and drives the true peak past the
        // -1 dBTP ceiling — exactly the case the limiter exists for.
        let config = NormalizationConfig {
            target_lufs: 0.0,
            max_true_peak_dbtp: -1.0,
            mode: NormalizationMode::LimitedGain,
            enable_limiting: true,
            ..Default::default()
        };
        let normalizer = LoudnessNormalizer::new(config, SAMPLE_RATE, 2);
        let mut frames = sine_frames(-20.0, 4.0, SampleFormat::F32, false);

        let params = normalizer.normalize(&mut frames);
        assert!(
            params.limiting_gain_db < -0.1,
            "limiting should engage, got {:.3} dB",
            params.limiting_gain_db
        );

        let ceiling = TruePeakDetector::dbtp_to_linear(-1.0);
        for frame in &frames {
            for sample in normalizer.extract_samples(frame) {
                assert!(
                    sample.abs() <= ceiling + 1e-6,
                    "sample {sample} exceeds the -1 dBTP ceiling {ceiling}"
                );
            }
        }
    }

    /// A signal already at target must not be touched.
    #[test]
    fn test_normalize_noop_when_on_target() {
        let normalizer = normalizer_for(-23.0);
        let mut frames = sine_frames(-30.0, 4.0, SampleFormat::F32, false);
        normalizer.normalize(&mut frames);

        let snapshot = frames.clone();
        let params = normalizer.normalize(&mut frames);
        assert!(
            params.gain_db.abs() < 0.5,
            "second pass should need almost no gain, got {:.3} dB",
            params.gain_db
        );

        if params.total_gain_db.abs() < 0.01 {
            assert_eq!(params.frames_modified, 0, "no-op must not rewrite frames");
            for (a, b) in snapshot.iter().zip(frames.iter()) {
                match (&a.samples, &b.samples) {
                    (AudioBuffer::Interleaved(x), AudioBuffer::Interleaved(y)) => {
                        assert_eq!(x, y, "no-op must leave bytes untouched");
                    }
                    _ => panic!("unexpected buffer kind"),
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AutoGain tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod auto_gain_tests {
    use super::*;

    fn make_agc() -> AutoGainProcessor {
        AutoGainProcessor::new(AutoGainConfig::default())
    }

    #[test]
    fn test_agc_creation() {
        let agc = make_agc();
        assert!((agc.current_gain() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_agc_reset() {
        let mut agc = make_agc();
        // Drive it up
        let mut block = vec![0.001_f64; 512];
        for _ in 0..200 {
            agc.process_block(&mut block);
        }
        agc.reset();
        assert!((agc.current_gain() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_agc_output_finite() {
        let mut agc = make_agc();
        let mut block: Vec<f64> = (0..256).map(|i| (i as f64 * 0.01).sin() * 0.5).collect();
        agc.process_block(&mut block);
        for &s in &block {
            assert!(s.is_finite(), "output must be finite");
        }
    }

    #[test]
    fn test_agc_boosts_quiet_signal() {
        let mut agc = AutoGainProcessor::new(AutoGainConfig {
            target_db: -6.0,
            attack_secs: 0.01,
            release_secs: 0.1,
            sample_rate: 48_000.0,
            max_gain_db: 40.0,
            min_gain_db: -40.0,
        });

        // Very quiet signal at -60 dBFS (rms ≈ 0.001)
        let mut block = vec![0.001_f64; 1024];
        for _ in 0..100 {
            agc.process_block(&mut block);
        }
        // After many blocks the gain should be > 1
        assert!(agc.current_gain() > 1.0, "AGC should boost quiet signal");
    }

    #[test]
    fn test_agc_reduces_loud_signal() {
        let mut agc = AutoGainProcessor::new(AutoGainConfig {
            target_db: -20.0,
            attack_secs: 0.01,
            release_secs: 0.1,
            sample_rate: 48_000.0,
            max_gain_db: 40.0,
            min_gain_db: -40.0,
        });

        // Very loud signal at 0 dBFS
        let mut block = vec![1.0_f64; 1024];
        for _ in 0..100 {
            agc.process_block(&mut block);
        }
        // Gain should be < 1 to attenuate
        assert!(agc.current_gain() < 1.0, "AGC should attenuate loud signal");
    }

    #[test]
    fn test_agc_current_gain_db_unity() {
        let agc = make_agc();
        assert!((agc.current_gain_db() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_agc_set_target_db() {
        let mut agc = make_agc();
        agc.set_target_db(-14.0);
        assert!((agc.current_gain() - 1.0).abs() < 1e-10); // gain unchanged immediately
    }

    #[test]
    fn test_agc_f32_output_finite() {
        let mut agc = make_agc();
        let mut block: Vec<f32> = (0..256).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        agc.process_block_f32(&mut block);
        for &s in &block {
            assert!(s.is_finite(), "f32 output must be finite");
        }
    }

    #[test]
    fn test_agc_empty_block_no_panic() {
        let mut agc = make_agc();
        let mut empty: Vec<f64> = Vec::new();
        agc.process_block(&mut empty); // must not panic
        assert!((agc.current_gain() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_agc_max_gain_clamped() {
        let mut agc = AutoGainProcessor::new(AutoGainConfig {
            target_db: 0.0,
            attack_secs: 0.001,
            release_secs: 0.001,
            sample_rate: 48_000.0,
            max_gain_db: 6.0,
            min_gain_db: -40.0,
        });
        // Push with silence many times
        let mut block = vec![1e-20_f64; 512];
        for _ in 0..1000 {
            agc.process_block(&mut block);
        }
        let max_linear = 10.0_f64.powf(6.0 / 20.0);
        assert!(
            agc.current_gain() <= max_linear + 1e-6,
            "gain must not exceed max; got {}",
            agc.current_gain()
        );
    }
}
