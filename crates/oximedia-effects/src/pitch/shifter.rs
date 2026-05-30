//! Pitch shifting effect.

#![allow(dead_code)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::{utils::FractionalDelayLine, utils::InterpolationMode, AudioEffect};

/// Pitch shifter configuration.
#[derive(Debug, Clone)]
pub struct PitchShifterConfig {
    /// Pitch shift in semitones (-24.0 to +24.0).
    pub semitones: f32,
    /// Fine tune in cents (-100.0 to +100.0).
    pub cents: f32,
    /// Wet/dry mix (0.0 - 1.0).
    pub mix: f32,
}

impl Default for PitchShifterConfig {
    fn default() -> Self {
        Self {
            semitones: 0.0,
            cents: 0.0,
            mix: 1.0,
        }
    }
}

/// Simple pitch shifter using time-domain method (PSOLA-like).
///
/// Note: This is a simplified implementation. Production systems would use
/// more sophisticated algorithms like phase vocoder or PSOLA.
pub struct PitchShifter {
    delay: FractionalDelayLine,
    phase: f32,
    config: PitchShifterConfig,
    #[allow(dead_code)]
    sample_rate: f32,
}

impl PitchShifter {
    /// Create new pitch shifter.
    #[must_use]
    pub fn new(config: PitchShifterConfig, sample_rate: f32) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let delay_size = (sample_rate * 0.1) as usize; // 100ms buffer

        Self {
            delay: FractionalDelayLine::new(delay_size, InterpolationMode::Linear),
            phase: 0.0,
            config,
            sample_rate,
        }
    }

    /// Set pitch shift in semitones.
    pub fn set_semitones(&mut self, semitones: f32) {
        self.config.semitones = semitones.clamp(-24.0, 24.0);
    }

    /// Set fine tune in cents.
    pub fn set_cents(&mut self, cents: f32) {
        self.config.cents = cents.clamp(-100.0, 100.0);
    }

    fn pitch_ratio(&self) -> f32 {
        let total_semitones = self.config.semitones + self.config.cents / 100.0;
        2.0_f32.powf(total_semitones / 12.0)
    }
}

impl AudioEffect for PitchShifter {
    const EFFECT_ID: u64 = 6024;

    fn process_sample(&mut self, input: f32) -> f32 {
        let ratio = self.pitch_ratio();

        // Write to delay line
        self.delay.write(input);

        // Read with variable delay based on pitch ratio
        let base_delay = 1000.0; // Base delay in samples
        let mod_delay = base_delay * (1.0 + 0.1 * self.phase.sin());

        let shifted = self.delay.read(mod_delay);

        // Update phase
        self.phase += 0.01 * (ratio - 1.0);
        if self.phase > std::f32::consts::TAU {
            self.phase -= std::f32::consts::TAU;
        }

        // Mix
        shifted * self.config.mix + input * (1.0 - self.config.mix)
    }

    fn reset(&mut self) {
        self.delay.clear();
        self.phase = 0.0;
    }
}

/// Pitch shifting algorithm selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitchAlgorithm {
    /// Simple resampling (changes tempo too).
    Resample,
    /// Phase vocoder (frequency domain).
    PhaseVocoder,
    /// Simplified WSOLA (Waveform Similarity Overlap-Add).
    WsolaLite,
}

/// Advanced pitch shifter with multiple algorithm support.
///
/// Provides static methods for offline pitch shifting:
/// - `shift_resample`: Speed-based resampling with linear interpolation
/// - `shift_wsola`: Simplified WSOLA overlap-add approach
pub struct AdvancedPitchShifter {
    /// Number of semitones to shift.
    pub semitones: f32,
    /// Algorithm to use.
    pub algorithm: PitchAlgorithm,
}

impl AdvancedPitchShifter {
    /// Create a new advanced pitch shifter.
    #[must_use]
    pub fn new(semitones: f32, algorithm: PitchAlgorithm) -> Self {
        Self {
            semitones,
            algorithm,
        }
    }

    /// Process a buffer of samples using the configured algorithm.
    #[must_use]
    pub fn process(&self, samples: &[f32]) -> Vec<f32> {
        match self.algorithm {
            PitchAlgorithm::Resample => Self::shift_resample(samples, self.semitones),
            PitchAlgorithm::PhaseVocoder => {
                // Phase vocoder requires FFT; fall back to resample for lite version
                Self::shift_resample(samples, self.semitones)
            }
            PitchAlgorithm::WsolaLite => Self::shift_wsola(samples, self.semitones, 1024),
        }
    }

    /// Pitch shift via resampling (changes duration).
    ///
    /// Speed factor = 2^(semitones/12). Uses linear interpolation.
    #[must_use]
    pub fn shift_resample(samples: &[f32], semitones: f32) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }
        let speed = 2.0_f32.powf(semitones / 12.0);
        let output_len = ((samples.len() as f32) / speed).round() as usize;
        let output_len = output_len.max(1);
        let mut output = Vec::with_capacity(output_len);

        for i in 0..output_len {
            let src_pos = i as f32 * speed;
            let idx = src_pos as usize;
            let frac = src_pos - idx as f32;

            let s0 = if idx < samples.len() {
                samples[idx]
            } else {
                0.0
            };
            let s1 = if idx + 1 < samples.len() {
                samples[idx + 1]
            } else {
                0.0
            };
            output.push(s0 + frac * (s1 - s0));
        }

        output
    }

    /// Pitch shift using simplified WSOLA (Waveform Similarity Overlap-Add).
    ///
    /// Stretches first, then resamples back to original length.
    #[must_use]
    pub fn shift_wsola(samples: &[f32], semitones: f32, frame_size: usize) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }
        let speed = 2.0_f32.powf(semitones / 12.0);
        // Stretch factor: how much we expand time before resampling
        let stretch = 1.0 / speed;
        let hop = frame_size / 4;
        let stretched_len = ((samples.len() as f32) * stretch).round() as usize;
        let stretched_len = stretched_len.max(frame_size);

        // Create stretched signal by copying overlapping frames
        let mut stretched = vec![0.0f32; stretched_len];
        let mut counts = vec![0u32; stretched_len];

        let mut src_pos = 0usize;
        let mut dst_pos = 0usize;
        let src_hop = (hop as f32 * speed).round() as usize;
        let src_hop = src_hop.max(1);

        while dst_pos + frame_size <= stretched_len && src_pos + frame_size <= samples.len() {
            // Simple Hanning window
            for k in 0..frame_size {
                let window = 0.5
                    * (1.0
                        - (2.0 * std::f32::consts::PI * k as f32 / (frame_size - 1) as f32).cos());
                stretched[dst_pos + k] += samples[src_pos + k] * window;
                counts[dst_pos + k] += 1;
            }
            src_pos = (src_pos + src_hop).min(samples.len().saturating_sub(frame_size));
            dst_pos += hop;
        }

        // Normalize by overlap count
        for (s, &c) in stretched.iter_mut().zip(counts.iter()) {
            if c > 0 {
                *s /= c as f32;
            }
        }

        // Resample back to original length
        let target_len = samples.len();
        Self::resample_linear(&stretched, target_len)
    }

    /// Linear resampling to a target length.
    fn resample_linear(samples: &[f32], target_len: usize) -> Vec<f32> {
        if samples.is_empty() || target_len == 0 {
            return Vec::new();
        }
        let ratio = (samples.len() - 1) as f32 / (target_len - 1).max(1) as f32;
        let mut output = Vec::with_capacity(target_len);
        for i in 0..target_len {
            let src_pos = i as f32 * ratio;
            let idx = src_pos as usize;
            let frac = src_pos - idx as f32;
            let s0 = if idx < samples.len() {
                samples[idx]
            } else {
                0.0
            };
            let s1 = if idx + 1 < samples.len() {
                samples[idx + 1]
            } else {
                s0
            };
            output.push(s0 + frac * (s1 - s0));
        }
        output
    }
}

/// Formant preserving processor for pitch shifting.
///
/// In a full implementation this would use cepstral liftering to separate
/// the spectral envelope from the pitch content. This simplified version
/// applies a smoothing filter that approximates formant preservation.
pub struct FormantPreserver {
    /// Shift in Hz (positive = upward).
    pub shift_hz: f32,
    /// Smoothing window size.
    window_size: usize,
}

impl FormantPreserver {
    /// Create a new formant preserver.
    #[must_use]
    pub fn new(shift_hz: f32) -> Self {
        Self {
            shift_hz,
            window_size: 64,
        }
    }

    /// Apply formant preservation to a pitch-shifted signal.
    ///
    /// Applies a mild spectral smoothing that approximates formant correction.
    #[must_use]
    pub fn apply(&self, samples: &[f32], sample_rate: u32) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }
        // Smoothing window based on shift magnitude relative to Nyquist
        let nyquist = sample_rate as f32 / 2.0;
        let normalized_shift = (self.shift_hz.abs() / nyquist).clamp(0.0, 1.0);
        let smooth_samples = (self.window_size as f32 * normalized_shift) as usize;

        if smooth_samples < 2 {
            return samples.to_vec();
        }

        // Simple moving average as placeholder for spectral envelope correction
        let half = smooth_samples / 2;
        let n = samples.len();
        let mut output = Vec::with_capacity(n);

        for i in 0..n {
            let start = i.saturating_sub(half);
            let end = (i + half + 1).min(n);
            let sum: f32 = samples[start..end].iter().sum();
            let count = (end - start) as f32;
            output.push(sum / count);
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pitch_shifter() {
        let config = PitchShifterConfig::default();
        let mut shifter = PitchShifter::new(config, 48000.0);

        let output = shifter.process_sample(0.5);
        assert!(output.is_finite());
    }

    #[test]
    fn test_pitch_ratio() {
        let config = PitchShifterConfig {
            semitones: 12.0,
            ..Default::default()
        };
        let shifter = PitchShifter::new(config, 48000.0);
        let ratio = shifter.pitch_ratio();
        assert!((ratio - 2.0).abs() < 0.01); // 12 semitones = 2x
    }

    #[test]
    fn test_pitch_algorithm_variants() {
        let _ = PitchAlgorithm::Resample;
        let _ = PitchAlgorithm::PhaseVocoder;
        let _ = PitchAlgorithm::WsolaLite;
    }

    #[test]
    fn test_advanced_shifter_new() {
        let shifter = AdvancedPitchShifter::new(5.0, PitchAlgorithm::Resample);
        assert!((shifter.semitones - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_shift_resample_octave_up() {
        let samples: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin()).collect();
        let shifted = AdvancedPitchShifter::shift_resample(&samples, 12.0);
        // 12 semitones up → half as many output samples
        assert!(shifted.len() < samples.len() / 2 + 10);
        assert!(shifted.iter().all(|&s| s.is_finite()));
    }

    #[test]
    fn test_shift_resample_octave_down() {
        let samples: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin()).collect();
        let shifted = AdvancedPitchShifter::shift_resample(&samples, -12.0);
        // 12 semitones down → twice as many output samples
        assert!(shifted.len() > samples.len());
        assert!(shifted.iter().all(|&s| s.is_finite()));
    }

    #[test]
    fn test_shift_resample_zero_semitones() {
        let samples = vec![0.1f32, 0.2, 0.3, 0.4, 0.5];
        let shifted = AdvancedPitchShifter::shift_resample(&samples, 0.0);
        assert_eq!(shifted.len(), samples.len());
        for (&a, &b) in samples.iter().zip(shifted.iter()) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn test_shift_resample_empty() {
        let shifted = AdvancedPitchShifter::shift_resample(&[], 5.0);
        assert!(shifted.is_empty());
    }

    #[test]
    fn test_shift_wsola_output_length() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.01).sin()).collect();
        let shifted = AdvancedPitchShifter::shift_wsola(&samples, 5.0, 512);
        // WSOLA should return approximately the same length
        assert_eq!(shifted.len(), samples.len());
        assert!(shifted.iter().all(|&s| s.is_finite()));
    }

    #[test]
    fn test_shift_wsola_empty() {
        let shifted = AdvancedPitchShifter::shift_wsola(&[], 3.0, 512);
        assert!(shifted.is_empty());
    }

    #[test]
    fn test_advanced_shifter_process_resample() {
        let shifter = AdvancedPitchShifter::new(7.0, PitchAlgorithm::Resample);
        let samples: Vec<f32> = (0..512).map(|i| (i as f32 * 0.1).sin()).collect();
        let output = shifter.process(&samples);
        assert!(output.iter().all(|&s| s.is_finite()));
    }

    #[test]
    fn test_advanced_shifter_process_wsola() {
        let shifter = AdvancedPitchShifter::new(3.0, PitchAlgorithm::WsolaLite);
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.01).sin()).collect();
        let output = shifter.process(&samples);
        assert_eq!(output.len(), samples.len());
        assert!(output.iter().all(|&s| s.is_finite()));
    }

    #[test]
    fn test_formant_preserver_new() {
        let fp = FormantPreserver::new(200.0);
        assert!((fp.shift_hz - 200.0).abs() < 1e-6);
    }

    #[test]
    fn test_formant_preserver_apply() {
        let fp = FormantPreserver::new(100.0);
        let samples: Vec<f32> = (0..512).map(|i| (i as f32 * 0.1).sin()).collect();
        let output = fp.apply(&samples, 48000);
        assert_eq!(output.len(), samples.len());
        assert!(output.iter().all(|&s| s.is_finite()));
    }

    #[test]
    fn test_formant_preserver_empty() {
        let fp = FormantPreserver::new(100.0);
        let output = fp.apply(&[], 48000);
        assert!(output.is_empty());
    }
}
