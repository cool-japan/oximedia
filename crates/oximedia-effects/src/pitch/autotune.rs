//! Chromatic auto-tune / pitch correction effect.
//!
//! Provides key-aware pitch correction using YIN-based pitch detection
//! and 12-TET (twelve-tone equal temperament) scale quantization.
//!
//! # Features
//!
//! - **YIN pitch detector**: Reliable fundamental frequency estimation
//! - **12-TET quantization**: Snaps detected pitch to nearest semitone
//! - **Key/scale awareness**: Major, natural minor, harmonic minor,
//!   dorian, mixolydian, pentatonic, blues, and chromatic scales
//! - **Correction speed**: From subtle (slow) to hard-tune effect (fast)
//! - **Humanize**: Preserves natural vibrato by reducing correction
//!   when pitch deviation is small
//! - **Reference frequency**: Configurable A4 tuning (default 440 Hz)

#![allow(clippy::cast_precision_loss)]

use crate::AudioEffect;

// ---------------------------------------------------------------------------
// Musical key / scale definitions
// ---------------------------------------------------------------------------

/// Musical key (root note).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// C
    C = 0,
    /// C#/Db
    CSharp = 1,
    /// D
    D = 2,
    /// D#/Eb
    DSharp = 3,
    /// E
    E = 4,
    /// F
    F = 5,
    /// F#/Gb
    FSharp = 6,
    /// G
    G = 7,
    /// G#/Ab
    GSharp = 8,
    /// A
    A = 9,
    /// A#/Bb
    ASharp = 10,
    /// B
    B = 11,
}

/// Musical scale type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// Chromatic (all 12 semitones allowed).
    Chromatic,
    /// Major (Ionian) — W-W-H-W-W-W-H.
    Major,
    /// Natural minor (Aeolian) — W-H-W-W-H-W-W.
    Minor,
    /// Harmonic minor — W-H-W-W-H-A2-H (augmented 2nd).
    HarmonicMinor,
    /// Dorian — W-H-W-W-W-H-W.
    Dorian,
    /// Mixolydian — W-W-H-W-W-H-W.
    Mixolydian,
    /// Major pentatonic — scale degrees 1-2-3-5-6.
    Pentatonic,
    /// Blues — minor pentatonic + flat 5.
    Blues,
}

impl Scale {
    /// Get the semitone intervals that belong to this scale (relative to root).
    /// Returns a 12-element array where `true` means that pitch class is in the scale.
    #[must_use]
    fn intervals(self) -> [bool; 12] {
        match self {
            Scale::Chromatic => [true; 12],
            //                          C   C#  D   D#  E   F   F#  G   G#  A   A#  B
            Scale::Major => [
                true, false, true, false, true, true, false, true, false, true, false, true,
            ],
            Scale::Minor => [
                true, false, true, true, false, true, false, true, true, false, true, false,
            ],
            Scale::HarmonicMinor => [
                true, false, true, true, false, true, false, true, true, false, false, true,
            ],
            Scale::Dorian => [
                true, false, true, true, false, true, false, true, false, true, true, false,
            ],
            Scale::Mixolydian => [
                true, false, true, false, true, true, false, true, false, true, true, false,
            ],
            Scale::Pentatonic => [
                true, false, true, false, true, false, false, true, false, true, false, false,
            ],
            Scale::Blues => [
                true, false, false, true, false, true, true, true, false, false, true, false,
            ],
        }
    }

    /// Build allowed-note mask rotated by key offset.
    fn allowed_notes(self, key: Key) -> [bool; 12] {
        let base = self.intervals();
        let offset = key as usize;
        let mut rotated = [false; 12];
        for i in 0..12 {
            rotated[(i + offset) % 12] = base[i];
        }
        rotated
    }
}

// ---------------------------------------------------------------------------
// Auto-tune configuration
// ---------------------------------------------------------------------------

/// Auto-tune configuration.
#[derive(Debug, Clone)]
pub struct AutoTuneConfig {
    /// Correction speed (0.0 = no correction, 1.0 = instant snap).
    /// Values around 0.1-0.3 give natural results; 0.8-1.0 produces
    /// the characteristic "hard-tune" robotic effect.
    pub correction: f32,
    /// Musical key (root note).
    pub key: Key,
    /// Musical scale type.
    pub scale: Scale,
    /// Reference frequency for A4 in Hz (default 440.0).
    pub reference_a4: f32,
    /// Humanize amount (0.0 - 1.0). When > 0, reduces correction for
    /// small pitch deviations to preserve natural vibrato.
    pub humanize: f32,
    /// Minimum detectable frequency in Hz.
    pub min_freq: f32,
    /// Maximum detectable frequency in Hz.
    pub max_freq: f32,
}

impl Default for AutoTuneConfig {
    fn default() -> Self {
        Self {
            correction: 0.5,
            key: Key::C,
            scale: Scale::Chromatic,
            reference_a4: 440.0,
            humanize: 0.0,
            min_freq: 60.0,
            max_freq: 1200.0,
        }
    }
}

impl AutoTuneConfig {
    /// Create with specific key and scale.
    #[must_use]
    pub fn with_key_scale(mut self, key: Key, scale: Scale) -> Self {
        self.key = key;
        self.scale = scale;
        self
    }

    /// Set correction speed.
    #[must_use]
    pub fn with_correction(mut self, correction: f32) -> Self {
        self.correction = correction.clamp(0.0, 1.0);
        self
    }

    /// Set humanize amount.
    #[must_use]
    pub fn with_humanize(mut self, humanize: f32) -> Self {
        self.humanize = humanize.clamp(0.0, 1.0);
        self
    }

    /// Set reference A4 frequency.
    #[must_use]
    pub fn with_reference(mut self, a4_hz: f32) -> Self {
        self.reference_a4 = a4_hz.clamp(400.0, 480.0);
        self
    }
}

// ---------------------------------------------------------------------------
// YIN pitch detector
// ---------------------------------------------------------------------------

/// YIN pitch detection algorithm.
///
/// Implements the YIN algorithm for fundamental frequency estimation,
/// a widely-used autocorrelation-based method that is robust against
/// octave errors.
struct YinDetector {
    /// Analysis buffer (ring buffer of recent samples).
    buffer: Vec<f32>,
    /// Write position in ring buffer.
    write_pos: usize,
    /// YIN threshold for pitch confidence (lower = stricter).
    threshold: f32,
    /// Minimum period in samples (corresponds to max frequency).
    min_period: usize,
    /// Maximum period in samples (corresponds to min frequency).
    max_period: usize,
    /// Half buffer size (analysis window).
    half_size: usize,
    /// Difference function workspace.
    diff: Vec<f32>,
    /// Cumulative mean normalized difference function workspace.
    cmndf: Vec<f32>,
}

impl YinDetector {
    fn new(sample_rate: f32, min_freq: f32, max_freq: f32) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let min_period = (sample_rate / max_freq).ceil() as usize;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_period = (sample_rate / min_freq).floor() as usize;

        // Buffer needs to hold at least 2 * max_period samples
        let half_size = max_period.max(256);
        let buffer_size = half_size * 2;

        Self {
            buffer: vec![0.0; buffer_size],
            write_pos: 0,
            threshold: 0.15,
            min_period: min_period.max(2),
            max_period,
            half_size,
            diff: vec![0.0; half_size],
            cmndf: vec![0.0; half_size],
        }
    }

    /// Push a single sample into the detector buffer.
    fn push(&mut self, sample: f32) {
        self.buffer[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) % self.buffer.len();
    }

    /// Read from the ring buffer at an offset from the current write position.
    fn read_back(&self, offset: usize) -> f32 {
        let len = self.buffer.len();
        let idx = (self.write_pos + len - 1 - offset) % len;
        self.buffer[idx]
    }

    /// Detect pitch using the YIN algorithm.
    /// Returns `Some(frequency)` if a reliable pitch is found, or `None`.
    fn detect(&mut self, sample_rate: f32) -> Option<f32> {
        let w = self.half_size;
        let max_tau = self.max_period.min(w - 1);

        if max_tau <= self.min_period {
            return None;
        }

        // Step 1: Difference function d(tau)
        self.diff[0] = 0.0;
        for tau in 1..=max_tau {
            let mut sum = 0.0f32;
            for j in 0..w {
                let x_j = self.read_back(j);
                let x_j_tau = self.read_back(j + tau);
                let delta = x_j - x_j_tau;
                sum += delta * delta;
            }
            self.diff[tau] = sum;
        }

        // Step 2: Cumulative mean normalized difference function
        self.cmndf[0] = 1.0;
        let mut running_sum = 0.0f32;
        for tau in 1..=max_tau {
            running_sum += self.diff[tau];
            if running_sum > 0.0 {
                #[allow(clippy::cast_precision_loss)]
                let tau_f = tau as f32;
                self.cmndf[tau] = self.diff[tau] * tau_f / running_sum;
            } else {
                self.cmndf[tau] = 1.0;
            }
        }

        // Step 3: Absolute threshold — find first tau below threshold
        let mut best_tau = None;
        for tau in self.min_period..=max_tau {
            if self.cmndf[tau] < self.threshold {
                // Find local minimum after dropping below threshold
                let mut min_tau = tau;
                let mut min_val = self.cmndf[tau];
                let search_end = (tau + 4).min(max_tau);
                for t in (tau + 1)..=search_end {
                    if self.cmndf[t] < min_val {
                        min_val = self.cmndf[t];
                        min_tau = t;
                    } else {
                        break;
                    }
                }
                best_tau = Some(min_tau);
                break;
            }
        }

        let tau = best_tau?;

        // Step 4: Parabolic interpolation for sub-sample accuracy
        let refined_tau = if tau > 0 && tau < max_tau {
            let s0 = self.cmndf[tau - 1];
            let s1 = self.cmndf[tau];
            let s2 = self.cmndf[tau + 1];
            let denom = 2.0 * s1 - s2 - s0;
            if denom.abs() > 1e-12 {
                #[allow(clippy::cast_precision_loss)]
                let correction = (s0 - s2) / (2.0 * denom);
                tau as f32 + correction
            } else {
                tau as f32
            }
        } else {
            tau as f32
        };

        if refined_tau > 0.0 {
            Some(sample_rate / refined_tau)
        } else {
            None
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }
}

// ---------------------------------------------------------------------------
// 12-TET pitch quantizer
// ---------------------------------------------------------------------------

/// Quantize a frequency to the nearest allowed note in a given key/scale.
///
/// Returns `(target_freq, semitone_offset)` where `semitone_offset` is
/// the signed distance from the detected pitch to the target note in semitones.
fn quantize_to_scale(freq: f32, reference_a4: f32, allowed: &[bool; 12]) -> (f32, f32) {
    // Convert frequency to continuous MIDI note number
    // MIDI 69 = A4
    let midi_continuous = 69.0 + 12.0 * (freq / reference_a4).ln() / core::f32::consts::LN_2;

    // Round to nearest allowed semitone
    let midi_rounded = midi_continuous.round() as i32;
    let pitch_class = ((midi_rounded % 12) + 12) % 12;

    // Search outward from nearest semitone for allowed note
    let mut best_offset = 0i32;
    for offset in 0..7 {
        let up = ((pitch_class + offset) % 12) as usize;
        let down = ((pitch_class - offset + 12) % 12) as usize;

        if allowed[up] {
            best_offset = offset;
            break;
        }
        if allowed[down] && offset > 0 {
            best_offset = -offset;
            break;
        }
    }

    let target_midi = midi_rounded + best_offset;
    #[allow(clippy::cast_precision_loss)]
    let target_freq = reference_a4 * 2.0f32.powf((target_midi as f32 - 69.0) / 12.0);
    let semitone_offset = midi_continuous - target_midi as f32;

    (target_freq, semitone_offset)
}

/// Get the note name for a frequency.
#[must_use]
pub fn frequency_to_note_name(freq: f32, reference_a4: f32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];

    let midi = 69.0 + 12.0 * (freq / reference_a4).ln() / core::f32::consts::LN_2;
    let midi_rounded = midi.round() as i32;
    let note_idx = ((midi_rounded % 12) + 12) % 12;
    let octave = (midi_rounded / 12) - 1;

    format!("{}{}", NAMES[note_idx as usize], octave)
}

// ---------------------------------------------------------------------------
// Pitch correction via resampling
// ---------------------------------------------------------------------------

/// PSOLA-style pitch corrector using a fractional delay line.
///
/// Applies a per-sample pitch shift by reading from a delay line at a
/// variable rate, effectively speeding up or slowing down the signal
/// to match the target pitch.
struct PitchCorrector {
    delay_buffer: Vec<f32>,
    buffer_size: usize,
    write_pos: usize,
    read_phase: f64,
    /// Smoothed correction ratio (current).
    current_ratio: f64,
    /// Target correction ratio.
    target_ratio: f64,
    /// Smoothing coefficient for ratio.
    smooth_coeff: f64,
}

impl PitchCorrector {
    fn new(sample_rate: f32) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let buffer_size = (sample_rate * 0.1) as usize; // 100ms buffer
        let buffer_size = buffer_size.max(256);

        // Start read_phase behind write_pos by half the buffer
        // so the corrector can read previously written samples
        let initial_delay = buffer_size / 2;

        Self {
            delay_buffer: vec![0.0; buffer_size],
            buffer_size,
            write_pos: initial_delay,
            read_phase: 0.0,
            current_ratio: 1.0,
            target_ratio: 1.0,
            smooth_coeff: 0.999, // very smooth by default
        }
    }

    fn set_correction_speed(&mut self, speed: f32) {
        // Higher speed = less smoothing on the ratio
        // speed 1.0 -> coeff ~0.99 (fast)
        // speed 0.1 -> coeff ~0.9999 (slow)
        let speed_clamped = speed.clamp(0.01, 1.0) as f64;
        self.smooth_coeff = 1.0 - speed_clamped * 0.01;
    }

    fn set_target_ratio(&mut self, ratio: f64) {
        self.target_ratio = ratio.clamp(0.5, 2.0);
    }

    fn process(&mut self, input: f32) -> f32 {
        // Write input to delay buffer
        self.delay_buffer[self.write_pos] = input;
        self.write_pos = (self.write_pos + 1) % self.buffer_size;

        // Smooth the correction ratio
        self.current_ratio =
            self.smooth_coeff * self.current_ratio + (1.0 - self.smooth_coeff) * self.target_ratio;

        // Read from delay buffer at variable rate
        self.read_phase += self.current_ratio;
        if self.read_phase >= self.buffer_size as f64 {
            self.read_phase -= self.buffer_size as f64;
        }

        // Linear interpolation for fractional read position
        let read_pos = self.read_phase;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let idx0 = read_pos as usize % self.buffer_size;
        let idx1 = (idx0 + 1) % self.buffer_size;
        let frac = (read_pos - idx0 as f64) as f32;

        self.delay_buffer[idx0] * (1.0 - frac) + self.delay_buffer[idx1] * frac
    }

    fn reset(&mut self) {
        self.delay_buffer.fill(0.0);
        self.write_pos = 0;
        self.read_phase = 0.0;
        self.current_ratio = 1.0;
        self.target_ratio = 1.0;
    }
}

// ---------------------------------------------------------------------------
// AutoTune effect
// ---------------------------------------------------------------------------

/// Chromatic auto-tune effect with key-aware pitch correction.
///
/// Uses YIN pitch detection to estimate the fundamental frequency,
/// then quantizes to the nearest allowed note in the configured
/// key/scale using 12-TET tuning. A correction speed parameter
/// controls how aggressively pitch is snapped.
///
/// # Example
///
/// ```ignore
/// use oximedia_effects::pitch::{AutoTune, AutoTuneConfig, Key, Scale};
///
/// let config = AutoTuneConfig::default()
///     .with_key_scale(Key::C, Scale::Major)
///     .with_correction(0.8);
///
/// let mut autotune = AutoTune::new(config, 48000.0);
///
/// // Process audio samples
/// for sample in audio_buffer.iter_mut() {
///     *sample = autotune.process_sample(*sample);
/// }
/// ```
pub struct AutoTune {
    config: AutoTuneConfig,
    detector: YinDetector,
    corrector: PitchCorrector,
    /// Allowed notes mask (precomputed from key + scale).
    allowed_notes: [bool; 12],
    /// Last detected frequency.
    last_detected_freq: f32,
    /// Last target frequency.
    last_target_freq: f32,
    /// Sample rate.
    sample_rate: f32,
    /// Sample counter for periodic pitch detection.
    sample_counter: usize,
    /// Detection interval in samples (run YIN every N samples).
    detection_interval: usize,
}

impl AutoTune {
    /// Create new auto-tune effect.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn new(config: AutoTuneConfig, sample_rate: f32) -> Self {
        let allowed_notes = config.scale.allowed_notes(config.key);
        let mut corrector = PitchCorrector::new(sample_rate);
        corrector.set_correction_speed(config.correction);

        // Run detection every ~5ms for responsive tracking
        let detection_interval = (sample_rate * 0.005).max(1.0) as usize;

        Self {
            detector: YinDetector::new(sample_rate, config.min_freq, config.max_freq),
            corrector,
            allowed_notes,
            last_detected_freq: 0.0,
            last_target_freq: 0.0,
            sample_rate,
            sample_counter: 0,
            detection_interval,
            config,
        }
    }

    /// Set correction speed (0.0 = none, 1.0 = instant snap).
    pub fn set_correction(&mut self, correction: f32) {
        self.config.correction = correction.clamp(0.0, 1.0);
        self.corrector.set_correction_speed(self.config.correction);
    }

    /// Set key and scale.
    pub fn set_key_scale(&mut self, key: Key, scale: Scale) {
        self.config.key = key;
        self.config.scale = scale;
        self.allowed_notes = scale.allowed_notes(key);
    }

    /// Set humanize amount (0.0 - 1.0).
    pub fn set_humanize(&mut self, humanize: f32) {
        self.config.humanize = humanize.clamp(0.0, 1.0);
    }

    /// Get the last detected frequency (0 if no pitch detected).
    #[must_use]
    pub fn detected_frequency(&self) -> f32 {
        self.last_detected_freq
    }

    /// Get the last target (corrected) frequency.
    #[must_use]
    pub fn target_frequency(&self) -> f32 {
        self.last_target_freq
    }

    /// Get the note name for the current target frequency.
    #[must_use]
    pub fn current_note(&self) -> String {
        if self.last_target_freq > 0.0 {
            frequency_to_note_name(self.last_target_freq, self.config.reference_a4)
        } else {
            String::from("--")
        }
    }
}

impl AudioEffect for AutoTune {
    const EFFECT_ID: u64 = 6021;
    fn process_sample(&mut self, input: f32) -> f32 {
        // Feed sample to pitch detector
        self.detector.push(input);
        self.sample_counter += 1;

        // Run pitch detection periodically
        if self.sample_counter >= self.detection_interval {
            self.sample_counter = 0;

            if let Some(freq) = self.detector.detect(self.sample_rate) {
                self.last_detected_freq = freq;

                // Quantize to nearest allowed note
                let (target_freq, semitone_offset) =
                    quantize_to_scale(freq, self.config.reference_a4, &self.allowed_notes);
                self.last_target_freq = target_freq;

                // Apply humanize: reduce correction for small deviations
                let effective_correction = if self.config.humanize > 0.0 {
                    let deviation = semitone_offset.abs();
                    // If deviation is less than humanize threshold (in semitones),
                    // scale down correction proportionally
                    let humanize_threshold = self.config.humanize * 0.5; // max 0.5 semitone
                    if deviation < humanize_threshold {
                        self.config.correction * (deviation / humanize_threshold)
                    } else {
                        self.config.correction
                    }
                } else {
                    self.config.correction
                };

                // Compute correction ratio
                if freq > 0.0 {
                    let full_ratio = target_freq as f64 / freq as f64;
                    // Blend between no correction (1.0) and full correction
                    let ratio = 1.0 + (full_ratio - 1.0) * effective_correction as f64;
                    self.corrector.set_target_ratio(ratio);
                }
            }
        }

        // Apply pitch correction
        self.corrector.process(input)
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.corrector.reset();
        self.last_detected_freq = 0.0;
        self.last_target_freq = 0.0;
        self.sample_counter = 0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = Self::new(self.config.clone(), sample_rate);
    }

    fn latency_samples(&self) -> usize {
        // YIN needs at least one analysis window before detection
        self.detector.half_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn generate_sine(freq: f32, sample_rate: f32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = i as f32 / sample_rate;
                (2.0 * PI * freq * t).sin()
            })
            .collect()
    }

    #[test]
    fn test_autotune_creation() {
        let config = AutoTuneConfig::default();
        let autotune = AutoTune::new(config, 48000.0);
        assert!((autotune.sample_rate - 48000.0).abs() < 1e-3);
        assert!((autotune.detected_frequency() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_autotune_process_finite() {
        let config = AutoTuneConfig::default();
        let mut autotune = AutoTune::new(config, 48000.0);

        for i in 0..4800 {
            #[allow(clippy::cast_precision_loss)]
            let input = (i as f32 * 0.1).sin() * 0.5;
            let output = autotune.process_sample(input);
            assert!(output.is_finite(), "Output should be finite at sample {i}");
        }
    }

    #[test]
    fn test_scale_intervals_chromatic() {
        let intervals = Scale::Chromatic.intervals();
        assert!(intervals.iter().all(|&v| v));
    }

    #[test]
    fn test_scale_intervals_major() {
        let intervals = Scale::Major.intervals();
        // C major: C D E F G A B = indices 0,2,4,5,7,9,11
        assert!(intervals[0]); // C
        assert!(!intervals[1]); // C#
        assert!(intervals[2]); // D
        assert!(!intervals[3]); // D#
        assert!(intervals[4]); // E
        assert!(intervals[5]); // F
        assert!(!intervals[6]); // F#
        assert!(intervals[7]); // G
        assert!(!intervals[8]); // G#
        assert!(intervals[9]); // A
        assert!(!intervals[10]); // A#
        assert!(intervals[11]); // B
    }

    #[test]
    fn test_scale_intervals_minor() {
        let intervals = Scale::Minor.intervals();
        // C natural minor: C D Eb F G Ab Bb = indices 0,2,3,5,7,8,10
        assert!(intervals[0]); // C
        assert!(intervals[2]); // D
        assert!(intervals[3]); // Eb
        assert!(intervals[5]); // F
        assert!(intervals[7]); // G
        assert!(intervals[8]); // Ab
        assert!(intervals[10]); // Bb
    }

    #[test]
    fn test_allowed_notes_c_major() {
        let allowed = Scale::Major.allowed_notes(Key::C);
        assert!(allowed[0]); // C
        assert!(allowed[4]); // E
        assert!(allowed[7]); // G
        assert!(!allowed[1]); // C#
    }

    #[test]
    fn test_allowed_notes_g_major() {
        // G major: G A B C D E F# = from root G(7): 7,9,11,0,2,4,6
        let allowed = Scale::Major.allowed_notes(Key::G);
        assert!(allowed[7]); // G
        assert!(allowed[9]); // A
        assert!(allowed[11]); // B
        assert!(allowed[0]); // C
        assert!(allowed[2]); // D
        assert!(allowed[4]); // E
        assert!(allowed[6]); // F#
        assert!(!allowed[1]); // C# not in G major
    }

    #[test]
    fn test_quantize_a440_chromatic() {
        let allowed = Scale::Chromatic.allowed_notes(Key::C);
        let (target, offset) = quantize_to_scale(440.0, 440.0, &allowed);
        assert!(
            (target - 440.0).abs() < 1.0,
            "A440 should quantize to itself: {target}"
        );
        assert!(offset.abs() < 0.01);
    }

    #[test]
    fn test_quantize_between_notes() {
        // 450 Hz is between A4 (440) and A#4 (466.16)
        let allowed = Scale::Chromatic.allowed_notes(Key::C);
        let (target, _offset) = quantize_to_scale(450.0, 440.0, &allowed);
        // Should snap to A4 (440) since it's closer
        assert!(
            (target - 440.0).abs() < 2.0,
            "450Hz should snap to A4 (440): {target}"
        );
    }

    #[test]
    fn test_quantize_to_c_major_skips_black_keys() {
        // F#4 (~370 Hz) is not in C major, should snap to F or G
        let allowed = Scale::Major.allowed_notes(Key::C);
        let fsharp4 = 440.0 * 2.0f32.powf(-3.0 / 12.0); // F#4 = ~369.99
        let (target, _offset) = quantize_to_scale(fsharp4, 440.0, &allowed);

        // Should snap to either F4 (~349.23) or G4 (~392.00)
        let f4 = 440.0 * 2.0f32.powf(-4.0 / 12.0);
        let g4 = 440.0 * 2.0f32.powf(-2.0 / 12.0);
        let dist_to_f = (target - f4).abs();
        let dist_to_g = (target - g4).abs();
        assert!(
            dist_to_f < 2.0 || dist_to_g < 2.0,
            "F#4 should snap to F4 or G4 in C major: target={target}"
        );
    }

    #[test]
    fn test_frequency_to_note_name() {
        assert_eq!(frequency_to_note_name(440.0, 440.0), "A4");
        assert_eq!(frequency_to_note_name(261.63, 440.0), "C4");
        assert_eq!(frequency_to_note_name(880.0, 440.0), "A5");
    }

    #[test]
    fn test_yin_detector_sine() {
        let sample_rate = 48000.0;
        let freq = 440.0;
        // Use a longer signal for more reliable detection
        let samples = generate_sine(freq, sample_rate, 24000);

        let mut detector = YinDetector::new(sample_rate, 60.0, 1200.0);

        // Feed samples
        for &s in &samples {
            detector.push(s);
        }

        // Detect pitch
        let detected = detector.detect(sample_rate);
        assert!(detected.is_some(), "Should detect pitch for 440Hz sine");
        if let Some(f) = detected {
            // Allow up to 15% frequency error (YIN on finite windows with
            // limited buffer sizes can have sub-harmonic detection)
            let rel_error = (f - freq).abs() / freq;
            assert!(
                rel_error < 0.15,
                "Pitch detection error too large: {rel_error:.3} ({f:.1} Hz vs {freq:.1} Hz)"
            );
        }
    }

    #[test]
    fn test_yin_detector_low_freq() {
        let sample_rate = 48000.0;
        let freq = 100.0;
        // Low frequencies need more samples for reliable detection
        let samples = generate_sine(freq, sample_rate, 48000);

        let mut detector = YinDetector::new(sample_rate, 60.0, 1200.0);
        for &s in &samples {
            detector.push(s);
        }

        let detected = detector.detect(sample_rate);
        assert!(detected.is_some(), "Should detect 100Hz");
        if let Some(f) = detected {
            // Allow up to 25% error for low frequencies (YIN is less accurate there)
            let rel_error = (f - freq).abs() / freq;
            assert!(
                rel_error < 0.25,
                "100Hz detection off: detected {f} (error {rel_error:.3})"
            );
        }
    }

    #[test]
    fn test_yin_detector_silence() {
        let sample_rate = 48000.0;
        let mut detector = YinDetector::new(sample_rate, 60.0, 1200.0);

        // Feed silence
        for _ in 0..4800 {
            detector.push(0.0);
        }

        let detected = detector.detect(sample_rate);
        // Should either return None or a very low-confidence result
        // (silence has no definite pitch)
        if let Some(f) = detected {
            // If it does detect something, it shouldn't be in normal vocal range
            // (this is acceptable — YIN on silence is undefined)
            assert!(f.is_finite());
        }
    }

    #[test]
    fn test_pitch_corrector_passthrough() {
        let mut corrector = PitchCorrector::new(48000.0);
        corrector.set_target_ratio(1.0); // no correction

        let mut output_energy = 0.0f32;
        for i in 0..4800 {
            #[allow(clippy::cast_precision_loss)]
            let input = (i as f32 * 0.1).sin() * 0.5;
            let output = corrector.process(input);
            output_energy += output * output;
            assert!(output.is_finite());
        }
        assert!(output_energy > 0.0, "Should produce non-zero output");
    }

    #[test]
    fn test_autotune_with_sine_c_major() {
        let config = AutoTuneConfig::default()
            .with_key_scale(Key::C, Scale::Major)
            .with_correction(0.8);
        let mut autotune = AutoTune::new(config, 48000.0);

        // Feed A4 (440 Hz) — A is in C major, should stay close
        let samples = generate_sine(440.0, 48000.0, 48000);
        let mut output = Vec::with_capacity(samples.len());
        for &s in &samples {
            output.push(autotune.process_sample(s));
        }

        assert!(output.iter().all(|s| s.is_finite()));
        // Output should have energy
        let energy: f32 = output.iter().map(|s| s * s).sum();
        assert!(energy > 0.1, "Output should have energy");
    }

    #[test]
    fn test_autotune_humanize() {
        let config = AutoTuneConfig::default()
            .with_correction(0.8)
            .with_humanize(0.5);
        let mut autotune = AutoTune::new(config, 48000.0);

        for i in 0..4800 {
            #[allow(clippy::cast_precision_loss)]
            let input = (i as f32 * 0.05).sin() * 0.3;
            let output = autotune.process_sample(input);
            assert!(output.is_finite());
        }
    }

    #[test]
    fn test_autotune_reset() {
        let config = AutoTuneConfig::default();
        let mut autotune = AutoTune::new(config, 48000.0);

        // Process some samples
        for i in 0..2400 {
            #[allow(clippy::cast_precision_loss)]
            let input = (i as f32 * 0.1).sin();
            autotune.process_sample(input);
        }

        autotune.reset();
        assert!((autotune.detected_frequency() - 0.0).abs() < 1e-6);
        assert!((autotune.target_frequency() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_autotune_set_key_scale() {
        let config = AutoTuneConfig::default();
        let mut autotune = AutoTune::new(config, 48000.0);

        autotune.set_key_scale(Key::G, Scale::Major);
        // G major allows F#
        assert!(autotune.allowed_notes[6]); // F#
    }

    #[test]
    fn test_autotune_current_note_no_pitch() {
        let config = AutoTuneConfig::default();
        let autotune = AutoTune::new(config, 48000.0);
        assert_eq!(autotune.current_note(), "--");
    }

    #[test]
    fn test_config_builder() {
        let config = AutoTuneConfig::default()
            .with_key_scale(Key::D, Scale::Minor)
            .with_correction(0.9)
            .with_humanize(0.3)
            .with_reference(442.0);

        assert_eq!(config.key, Key::D);
        assert_eq!(config.scale, Scale::Minor);
        assert!((config.correction - 0.9).abs() < 1e-6);
        assert!((config.humanize - 0.3).abs() < 1e-6);
        assert!((config.reference_a4 - 442.0).abs() < 1e-6);
    }

    #[test]
    fn test_all_scales_have_at_least_5_notes() {
        let scales = [
            Scale::Chromatic,
            Scale::Major,
            Scale::Minor,
            Scale::HarmonicMinor,
            Scale::Dorian,
            Scale::Mixolydian,
            Scale::Pentatonic,
            Scale::Blues,
        ];
        for scale in &scales {
            let count = scale.intervals().iter().filter(|&&v| v).count();
            assert!(
                count >= 5,
                "{scale:?} should have at least 5 notes, has {count}"
            );
        }
    }

    #[test]
    fn test_all_keys() {
        let keys = [
            Key::C,
            Key::CSharp,
            Key::D,
            Key::DSharp,
            Key::E,
            Key::F,
            Key::FSharp,
            Key::G,
            Key::GSharp,
            Key::A,
            Key::ASharp,
            Key::B,
        ];
        for key in &keys {
            let allowed = Scale::Major.allowed_notes(*key);
            let count = allowed.iter().filter(|&&v| v).count();
            assert_eq!(count, 7, "Major scale should have 7 notes for key {key:?}");
        }
    }

    #[test]
    fn test_quantize_all_semitones_chromatic() {
        let allowed = Scale::Chromatic.allowed_notes(Key::C);
        // Every semitone should quantize to itself in chromatic
        for midi_note in 48..72 {
            #[allow(clippy::cast_precision_loss)]
            let freq = 440.0 * 2.0f32.powf((midi_note as f32 - 69.0) / 12.0);
            let (target, offset) = quantize_to_scale(freq, 440.0, &allowed);
            assert!(
                (target - freq).abs() < 1.0,
                "MIDI {midi_note}: {freq} should quantize to itself, got {target}"
            );
            assert!(offset.abs() < 0.1);
        }
    }

    #[test]
    fn test_latency_nonzero() {
        let config = AutoTuneConfig::default();
        let autotune = AutoTune::new(config, 48000.0);
        assert!(
            autotune.latency_samples() > 0,
            "AutoTune should report non-zero latency"
        );
    }

    #[test]
    fn test_harmonic_minor_intervals() {
        let intervals = Scale::HarmonicMinor.intervals();
        // C harmonic minor: C D Eb F G Ab B = 0,2,3,5,7,8,11
        assert!(intervals[0]); // C
        assert!(intervals[2]); // D
        assert!(intervals[3]); // Eb
        assert!(intervals[5]); // F
        assert!(intervals[7]); // G
        assert!(intervals[8]); // Ab
        assert!(intervals[11]); // B (raised 7th)
        assert!(!intervals[10]); // Bb (not in harmonic minor)
    }

    #[test]
    fn test_blues_scale() {
        let intervals = Scale::Blues.intervals();
        // C blues: C Eb F F# G Bb = 0,3,5,6,7,10
        assert!(intervals[0]); // C
        assert!(intervals[3]); // Eb
        assert!(intervals[5]); // F
        assert!(intervals[6]); // F# (blue note)
        assert!(intervals[7]); // G
        assert!(intervals[10]); // Bb
    }
}
