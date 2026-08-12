#![allow(dead_code)]
//! Tempo-based audio alignment for music synchronization.
//!
//! This module aligns audio streams by detecting and matching musical tempo,
//! beat positions, and rhythmic structures. It is particularly useful for
//! aligning multiple recordings of the same musical performance.

/// Tempo detection configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoConfig {
    /// Sample rate of the audio signal in Hz.
    pub sample_rate: u32,
    /// Minimum detectable BPM.
    pub min_bpm: f64,
    /// Maximum detectable BPM.
    pub max_bpm: f64,
    /// Analysis hop size in samples.
    pub hop_size: usize,
    /// Number of onset frames to accumulate before estimation.
    pub accumulation_frames: usize,
}

impl Default for TempoConfig {
    fn default() -> Self {
        Self {
            sample_rate: 44100,
            min_bpm: 40.0,
            max_bpm: 240.0,
            hop_size: 512,
            accumulation_frames: 256,
        }
    }
}

/// A detected beat position in an audio stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeatPosition {
    /// Time of the beat in seconds.
    pub time_secs: f64,
    /// Strength of the detected beat (0.0..1.0).
    pub strength: f64,
    /// Beat index (sequential, starting from 0).
    pub index: u32,
}

impl BeatPosition {
    /// Create a new beat position.
    #[must_use]
    pub fn new(time_secs: f64, strength: f64, index: u32) -> Self {
        Self {
            time_secs,
            strength: strength.clamp(0.0, 1.0),
            index,
        }
    }

    /// Compute the interval to the next beat.
    #[must_use]
    pub fn interval_to(&self, next: &Self) -> f64 {
        next.time_secs - self.time_secs
    }
}

/// Result of tempo estimation on an audio segment.
#[derive(Debug, Clone, PartialEq)]
pub struct TempoEstimate {
    /// Estimated tempo in BPM.
    pub bpm: f64,
    /// Confidence of the estimate (0.0..1.0).
    pub confidence: f64,
    /// Detected beat positions.
    pub beats: Vec<BeatPosition>,
    /// Alternative tempo candidates (e.g., half/double time).
    pub alternatives: Vec<f64>,
}

impl TempoEstimate {
    /// Create a new tempo estimate.
    #[must_use]
    pub fn new(bpm: f64, confidence: f64) -> Self {
        Self {
            bpm,
            confidence: confidence.clamp(0.0, 1.0),
            beats: Vec::new(),
            alternatives: Vec::new(),
        }
    }

    /// Compute the beat period in seconds from the detected BPM.
    #[must_use]
    pub fn beat_period_secs(&self) -> f64 {
        if self.bpm > 0.0 {
            60.0 / self.bpm
        } else {
            0.0
        }
    }

    /// Compute the mean inter-beat interval from detected beats.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn mean_ibi(&self) -> f64 {
        if self.beats.len() < 2 {
            return 0.0;
        }
        let total: f64 = self
            .beats
            .windows(2)
            .map(|w| w[1].time_secs - w[0].time_secs)
            .sum();
        total / (self.beats.len() - 1) as f64
    }

    /// Check whether this tempo is harmonically related to another tempo.
    #[must_use]
    pub fn is_harmonic_of(&self, other_bpm: f64) -> bool {
        if other_bpm <= 0.0 || self.bpm <= 0.0 {
            return false;
        }
        let ratio = self.bpm / other_bpm;
        let rounded = ratio.round();
        if rounded < 1.0 {
            return false;
        }
        (ratio - rounded).abs() < 0.05
    }
}

/// Onset detection function type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnsetFunction {
    /// Energy-based onset detection.
    Energy,
    /// Spectral flux onset detection.
    SpectralFlux,
    /// High-frequency content onset detection.
    HighFrequencyContent,
    /// Complex domain onset detection.
    ComplexDomain,
}

/// Onset envelope analyzer for beat tracking.
#[derive(Debug)]
pub struct OnsetAnalyzer {
    /// Configuration.
    config: TempoConfig,
    /// Type of onset function to use.
    onset_fn: OnsetFunction,
    /// Accumulated onset envelope.
    envelope: Vec<f64>,
}

impl OnsetAnalyzer {
    /// Create a new onset analyzer.
    #[must_use]
    pub fn new(config: TempoConfig, onset_fn: OnsetFunction) -> Self {
        Self {
            config,
            onset_fn,
            envelope: Vec::new(),
        }
    }

    /// Compute the onset envelope from audio samples.
    #[allow(clippy::cast_precision_loss)]
    pub fn compute_envelope(&mut self, samples: &[f32]) {
        self.envelope.clear();
        if samples.is_empty() || self.config.hop_size == 0 {
            return;
        }
        let hop = self.config.hop_size;
        let num_frames = samples.len() / hop;

        for i in 0..num_frames {
            let start = i * hop;
            let end = (start + hop).min(samples.len());
            let frame = &samples[start..end];

            let value = match self.onset_fn {
                OnsetFunction::Energy => {
                    frame.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / frame.len() as f64
                }
                OnsetFunction::SpectralFlux => {
                    // Simplified: use absolute differences between consecutive samples
                    if frame.len() < 2 {
                        0.0
                    } else {
                        frame
                            .windows(2)
                            .map(|w| f64::from(w[1] - w[0]).abs())
                            .sum::<f64>()
                            / (frame.len() - 1) as f64
                    }
                }
                OnsetFunction::HighFrequencyContent => {
                    // Simplified: weight by sample position within frame
                    frame
                        .iter()
                        .enumerate()
                        .map(|(j, &s)| (j as f64 + 1.0) * f64::from(s).abs())
                        .sum::<f64>()
                        / frame.len() as f64
                }
                OnsetFunction::ComplexDomain => {
                    // Simplified: combination of energy and flux
                    let energy: f64 = frame.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>()
                        / frame.len() as f64;
                    let flux: f64 = if frame.len() < 2 {
                        0.0
                    } else {
                        frame
                            .windows(2)
                            .map(|w| f64::from(w[1] - w[0]).abs())
                            .sum::<f64>()
                            / (frame.len() - 1) as f64
                    };
                    (energy + flux) / 2.0
                }
            };
            self.envelope.push(value);
        }
    }

    /// Return a reference to the computed onset envelope.
    #[must_use]
    pub fn envelope(&self) -> &[f64] {
        &self.envelope
    }

    /// Pick peaks in the onset envelope above a threshold.
    #[must_use]
    pub fn pick_peaks(&self, threshold: f64) -> Vec<usize> {
        let mut peaks = Vec::new();
        if self.envelope.len() < 3 {
            return peaks;
        }
        for i in 1..self.envelope.len() - 1 {
            if self.envelope[i] > threshold
                && self.envelope[i] > self.envelope[i - 1]
                && self.envelope[i] >= self.envelope[i + 1]
            {
                peaks.push(i);
            }
        }
        peaks
    }
}

/// Tempo-based alignment result between two audio streams.
#[derive(Debug, Clone, PartialEq)]
pub struct TempoAlignResult {
    /// Estimated offset in seconds (stream B relative to stream A).
    pub offset_secs: f64,
    /// Tempo of stream A in BPM.
    pub tempo_a: f64,
    /// Tempo of stream B in BPM.
    pub tempo_b: f64,
    /// Confidence of the alignment (0.0..1.0).
    pub confidence: f64,
    /// Number of matched beat pairs.
    pub matched_beats: usize,
}

impl TempoAlignResult {
    /// Create a new tempo alignment result.
    #[must_use]
    pub fn new(
        offset_secs: f64,
        tempo_a: f64,
        tempo_b: f64,
        confidence: f64,
        matched_beats: usize,
    ) -> Self {
        Self {
            offset_secs,
            tempo_a,
            tempo_b,
            confidence: confidence.clamp(0.0, 1.0),
            matched_beats,
        }
    }

    /// Return the tempo ratio between the two streams.
    #[must_use]
    pub fn tempo_ratio(&self) -> f64 {
        if self.tempo_b > 0.0 {
            self.tempo_a / self.tempo_b
        } else {
            0.0
        }
    }

    /// Check whether the two tempos are approximately equal.
    #[must_use]
    pub fn tempos_match(&self, tolerance_bpm: f64) -> bool {
        (self.tempo_a - self.tempo_b).abs() < tolerance_bpm
    }
}

/// Align two sets of beat positions by finding the best offset.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn align_beats(
    beats_a: &[BeatPosition],
    beats_b: &[BeatPosition],
    tolerance_secs: f64,
) -> TempoAlignResult {
    if beats_a.is_empty() || beats_b.is_empty() {
        return TempoAlignResult::new(0.0, 0.0, 0.0, 0.0, 0);
    }

    // Estimate tempos from beat intervals
    let tempo_a = estimate_bpm_from_beats(beats_a);
    let tempo_b = estimate_bpm_from_beats(beats_b);

    // Try each possible offset by pairing first beats
    let mut best_offset = 0.0;
    let mut best_count = 0_usize;

    for a_beat in beats_a.iter().take(beats_a.len().min(8)) {
        for b_beat in beats_b.iter().take(beats_b.len().min(8)) {
            let candidate_offset = a_beat.time_secs - b_beat.time_secs;
            let count = count_matched_beats(beats_a, beats_b, candidate_offset, tolerance_secs);
            if count > best_count {
                best_count = count;
                best_offset = candidate_offset;
            }
        }
    }

    let max_possible = beats_a.len().min(beats_b.len());
    let confidence = if max_possible > 0 {
        (best_count as f64 / max_possible as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    TempoAlignResult::new(best_offset, tempo_a, tempo_b, confidence, best_count)
}

/// Count how many beats match between two beat sequences given an offset.
fn count_matched_beats(
    beats_a: &[BeatPosition],
    beats_b: &[BeatPosition],
    offset_secs: f64,
    tolerance_secs: f64,
) -> usize {
    let mut count = 0;
    for a in beats_a {
        let shifted = a.time_secs - offset_secs;
        for b in beats_b {
            if (shifted - b.time_secs).abs() < tolerance_secs {
                count += 1;
                break;
            }
        }
    }
    count
}

/// Estimate BPM from a series of beat positions.
#[allow(clippy::cast_precision_loss)]
fn estimate_bpm_from_beats(beats: &[BeatPosition]) -> f64 {
    if beats.len() < 2 {
        return 0.0;
    }
    let (Some(first), Some(last)) = (beats.first(), beats.last()) else {
        // Unreachable given the length check above; avoids any panic path.
        return 0.0;
    };
    let total_time = last.time_secs - first.time_secs;
    if total_time <= 0.0 {
        return 0.0;
    }
    let intervals = (beats.len() - 1) as f64;
    let avg_interval = total_time / intervals;
    if avg_interval > 0.0 {
        60.0 / avg_interval
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tempo_config_default() {
        let cfg = TempoConfig::default();
        assert_eq!(cfg.sample_rate, 44100);
        assert!((cfg.min_bpm - 40.0).abs() < f64::EPSILON);
        assert!((cfg.max_bpm - 240.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_beat_position_interval() {
        let a = BeatPosition::new(1.0, 0.9, 0);
        let b = BeatPosition::new(1.5, 0.8, 1);
        assert!((a.interval_to(&b) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_beat_position_strength_clamped() {
        let bp = BeatPosition::new(0.0, 2.0, 0);
        assert!((bp.strength - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_tempo_estimate_beat_period() {
        let te = TempoEstimate::new(120.0, 0.9);
        assert!((te.beat_period_secs() - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_tempo_estimate_zero_bpm() {
        let te = TempoEstimate::new(0.0, 0.0);
        assert!((te.beat_period_secs()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_tempo_estimate_mean_ibi() {
        let mut te = TempoEstimate::new(120.0, 0.9);
        te.beats.push(BeatPosition::new(0.0, 1.0, 0));
        te.beats.push(BeatPosition::new(0.5, 1.0, 1));
        te.beats.push(BeatPosition::new(1.0, 1.0, 2));
        assert!((te.mean_ibi() - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_tempo_estimate_mean_ibi_single() {
        let mut te = TempoEstimate::new(120.0, 0.9);
        te.beats.push(BeatPosition::new(0.0, 1.0, 0));
        assert!((te.mean_ibi()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_is_harmonic_double_time() {
        let te = TempoEstimate::new(120.0, 0.9);
        assert!(te.is_harmonic_of(60.0));
        assert!(te.is_harmonic_of(120.0));
    }

    #[test]
    fn test_is_harmonic_not_related() {
        let te = TempoEstimate::new(120.0, 0.9);
        assert!(!te.is_harmonic_of(73.0));
    }

    #[test]
    fn test_onset_analyzer_energy() {
        let config = TempoConfig {
            sample_rate: 44100,
            hop_size: 4,
            ..TempoConfig::default()
        };
        let mut analyzer = OnsetAnalyzer::new(config, OnsetFunction::Energy);
        let samples = vec![0.5_f32, 0.3, 0.1, 0.0, 0.8, 0.6, 0.4, 0.2];
        analyzer.compute_envelope(&samples);
        assert_eq!(analyzer.envelope().len(), 2);
        assert!(analyzer.envelope()[0] > 0.0);
    }

    #[test]
    fn test_onset_analyzer_empty() {
        let config = TempoConfig::default();
        let mut analyzer = OnsetAnalyzer::new(config, OnsetFunction::Energy);
        analyzer.compute_envelope(&[]);
        assert!(analyzer.envelope().is_empty());
    }

    #[test]
    fn test_pick_peaks() {
        let config = TempoConfig {
            sample_rate: 44100,
            hop_size: 1,
            ..TempoConfig::default()
        };
        let mut analyzer = OnsetAnalyzer::new(config, OnsetFunction::Energy);
        // Manually set envelope with a clear peak
        let samples: Vec<f32> = vec![0.0, 0.1, 0.5, 0.9, 0.5, 0.1, 0.0];
        analyzer.compute_envelope(&samples);
        let peaks = analyzer.pick_peaks(0.01);
        assert!(!peaks.is_empty());
    }

    #[test]
    fn test_align_beats_exact_match() {
        let beats_a: Vec<BeatPosition> = (0..4)
            .map(|i| BeatPosition::new(i as f64 * 0.5, 1.0, i))
            .collect();
        let beats_b: Vec<BeatPosition> = (0..4)
            .map(|i| BeatPosition::new(i as f64 * 0.5, 1.0, i))
            .collect();
        let result = align_beats(&beats_a, &beats_b, 0.05);
        assert!(result.offset_secs.abs() < 0.06);
        assert!(result.matched_beats >= 3);
    }

    #[test]
    fn test_align_beats_with_offset() {
        let beats_a: Vec<BeatPosition> = (0..4)
            .map(|i| BeatPosition::new(i as f64 * 0.5 + 1.0, 1.0, i))
            .collect();
        let beats_b: Vec<BeatPosition> = (0..4)
            .map(|i| BeatPosition::new(i as f64 * 0.5, 1.0, i))
            .collect();
        let result = align_beats(&beats_a, &beats_b, 0.05);
        assert!((result.offset_secs - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_align_beats_empty() {
        let result = align_beats(&[], &[], 0.05);
        assert_eq!(result.matched_beats, 0);
        assert!((result.confidence).abs() < f64::EPSILON);
    }

    #[test]
    fn test_tempo_align_result_ratio() {
        let r = TempoAlignResult::new(0.0, 120.0, 60.0, 0.9, 8);
        assert!((r.tempo_ratio() - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_tempo_align_result_match() {
        let r = TempoAlignResult::new(0.0, 120.0, 120.5, 0.9, 8);
        assert!(r.tempos_match(1.0));
        assert!(!r.tempos_match(0.1));
    }
}
