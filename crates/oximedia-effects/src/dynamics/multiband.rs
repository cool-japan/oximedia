//! Multi-band compressor with Linkwitz-Riley crossovers.
//!
//! Splits the signal into three bands (low/mid/high) using 4th-order
//! Linkwitz-Riley crossover filters, then applies independent compression
//! to each band before summing to the output.
//!
//! Linkwitz-Riley crossovers are constructed by cascading two 2nd-order
//! Butterworth filters. They have the property of summing to unity gain at
//! all frequencies when low + high bands are summed (flat phase response
//! at crossover).

#![allow(clippy::cast_precision_loss)]

use std::f32::consts::PI;

use crate::{
    compressor::{CompressorConfig, GainComputerState, LevelDetector},
    AudioEffect,
};

/// Second-order Butterworth low-pass or high-pass filter (biquad).
#[derive(Clone)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    s1: f32, // state
    s2: f32, // state
}

impl Biquad {
    /// Identity (bypass) filter.
    #[allow(dead_code)]
    fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// 2nd-order Butterworth low-pass.
    fn butterworth_lp(cutoff_hz: f32, sample_rate: f32) -> Self {
        let omega = 2.0 * PI * cutoff_hz / sample_rate;
        let cos_w = omega.cos();
        let sin_w = omega.sin();
        let alpha = sin_w / 2.0_f32.sqrt(); // Q = 1/sqrt(2) for Butterworth

        let b0 = (1.0 - cos_w) / 2.0;
        let b1 = 1.0 - cos_w;
        let b2 = (1.0 - cos_w) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// 2nd-order Butterworth high-pass.
    fn butterworth_hp(cutoff_hz: f32, sample_rate: f32) -> Self {
        let omega = 2.0 * PI * cutoff_hz / sample_rate;
        let cos_w = omega.cos();
        let sin_w = omega.sin();
        let alpha = sin_w / 2.0_f32.sqrt();

        let b0 = (1.0 + cos_w) / 2.0;
        let b1 = -(1.0 + cos_w);
        let b2 = (1.0 + cos_w) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Process a single sample via Direct-Form-II transposed.
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.s1;
        self.s1 = self.b1 * x - self.a1 * y + self.s2;
        self.s2 = self.b2 * x - self.a2 * y;
        y
    }

    fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

/// 4th-order Linkwitz-Riley filter (two cascaded 2nd-order Butterworth stages).
#[derive(Clone)]
struct LinkwitzRiley {
    stage1: Biquad,
    stage2: Biquad,
}

impl LinkwitzRiley {
    /// 4th-order Linkwitz-Riley low-pass at `cutoff_hz`.
    fn lp(cutoff_hz: f32, sample_rate: f32) -> Self {
        Self {
            stage1: Biquad::butterworth_lp(cutoff_hz, sample_rate),
            stage2: Biquad::butterworth_lp(cutoff_hz, sample_rate),
        }
    }

    /// 4th-order Linkwitz-Riley high-pass at `cutoff_hz`.
    fn hp(cutoff_hz: f32, sample_rate: f32) -> Self {
        Self {
            stage1: Biquad::butterworth_hp(cutoff_hz, sample_rate),
            stage2: Biquad::butterworth_hp(cutoff_hz, sample_rate),
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.stage2.process(self.stage1.process(x))
    }

    fn reset(&mut self) {
        self.stage1.reset();
        self.stage2.reset();
    }
}

/// Per-band compressor with its own level detector and gain computer.
struct BandCompressor {
    detector: LevelDetector,
    gain_computer: GainComputerState,
    config: CompressorConfig,
    smoothed_gr_db: f32,
    attack_coeff: f32,
    release_coeff: f32,
    makeup_linear: f32,
}

impl BandCompressor {
    fn new(config: CompressorConfig, sample_rate: f32) -> Self {
        let attack_coeff = Self::time_coeff(config.attack_ms, sample_rate);
        let release_coeff = Self::time_coeff(config.release_ms, sample_rate);
        let makeup_linear = Self::db_to_linear(config.makeup_gain_db);

        Self {
            detector: LevelDetector::new(),
            gain_computer: GainComputerState::new(),
            config,
            smoothed_gr_db: 0.0,
            attack_coeff,
            release_coeff,
            makeup_linear,
        }
    }

    fn time_coeff(time_ms: f32, sample_rate: f32) -> f32 {
        let samples = time_ms * sample_rate / 1000.0;
        if samples > 0.0 {
            1.0 - (-2.2_f32 / samples).exp()
        } else {
            1.0
        }
    }

    fn db_to_linear(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }

    fn linear_to_db(linear: f32) -> f32 {
        20.0 * linear.max(1e-10).log10()
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let level = self
            .detector
            .process(x, self.attack_coeff, self.release_coeff);
        let level_db = Self::linear_to_db(level);
        let gr_db = self.gain_computer.compute_gain(level_db, &self.config);

        // Smooth gain reduction with separate attack/release ballistics
        if gr_db < self.smoothed_gr_db {
            self.smoothed_gr_db += self.attack_coeff * (gr_db - self.smoothed_gr_db);
        } else {
            self.smoothed_gr_db += self.release_coeff * (gr_db - self.smoothed_gr_db);
        }

        let gain = Self::db_to_linear(self.smoothed_gr_db);
        x * gain * self.makeup_linear
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.smoothed_gr_db = 0.0;
    }

    /// Update config and recompute coefficients.
    fn set_config(&mut self, config: CompressorConfig, sample_rate: f32) {
        self.attack_coeff = Self::time_coeff(config.attack_ms, sample_rate);
        self.release_coeff = Self::time_coeff(config.release_ms, sample_rate);
        self.makeup_linear = Self::db_to_linear(config.makeup_gain_db);
        self.config = config;
    }
}

/// Configuration for the multi-band compressor.
#[derive(Debug, Clone)]
pub struct MultibandCompressorConfig {
    /// Low crossover frequency in Hz (low/mid boundary).
    pub crossover_low_hz: f32,
    /// High crossover frequency in Hz (mid/high boundary).
    pub crossover_high_hz: f32,
    /// Compressor settings for the low band.
    pub low_band: CompressorConfig,
    /// Compressor settings for the mid band.
    pub mid_band: CompressorConfig,
    /// Compressor settings for the high band.
    pub high_band: CompressorConfig,
}

impl Default for MultibandCompressorConfig {
    fn default() -> Self {
        Self {
            crossover_low_hz: 200.0,
            crossover_high_hz: 2000.0,
            low_band: CompressorConfig {
                threshold_db: -20.0,
                ratio: 4.0,
                attack_ms: 30.0,
                release_ms: 200.0,
                knee_db: 6.0,
                makeup_gain_db: 2.0,
            },
            mid_band: CompressorConfig {
                threshold_db: -18.0,
                ratio: 3.0,
                attack_ms: 10.0,
                release_ms: 100.0,
                knee_db: 6.0,
                makeup_gain_db: 2.0,
            },
            high_band: CompressorConfig {
                threshold_db: -16.0,
                ratio: 2.0,
                attack_ms: 5.0,
                release_ms: 60.0,
                knee_db: 4.0,
                makeup_gain_db: 1.0,
            },
        }
    }
}

impl MultibandCompressorConfig {
    /// Mastering preset: gentle, transparent multi-band.
    #[must_use]
    pub fn mastering() -> Self {
        Self {
            crossover_low_hz: 150.0,
            crossover_high_hz: 3500.0,
            low_band: CompressorConfig {
                threshold_db: -20.0,
                ratio: 2.5,
                attack_ms: 40.0,
                release_ms: 300.0,
                knee_db: 8.0,
                makeup_gain_db: 1.0,
            },
            mid_band: CompressorConfig {
                threshold_db: -22.0,
                ratio: 2.0,
                attack_ms: 15.0,
                release_ms: 150.0,
                knee_db: 8.0,
                makeup_gain_db: 1.0,
            },
            high_band: CompressorConfig {
                threshold_db: -18.0,
                ratio: 1.5,
                attack_ms: 5.0,
                release_ms: 80.0,
                knee_db: 6.0,
                makeup_gain_db: 0.5,
            },
        }
    }

    /// Broadcast preset: heavy limiting for loudness compliance.
    #[must_use]
    pub fn broadcast() -> Self {
        Self {
            crossover_low_hz: 250.0,
            crossover_high_hz: 4000.0,
            low_band: CompressorConfig {
                threshold_db: -12.0,
                ratio: 6.0,
                attack_ms: 10.0,
                release_ms: 100.0,
                knee_db: 4.0,
                makeup_gain_db: 3.0,
            },
            mid_band: CompressorConfig {
                threshold_db: -12.0,
                ratio: 5.0,
                attack_ms: 5.0,
                release_ms: 80.0,
                knee_db: 4.0,
                makeup_gain_db: 3.0,
            },
            high_band: CompressorConfig {
                threshold_db: -10.0,
                ratio: 4.0,
                attack_ms: 2.0,
                release_ms: 50.0,
                knee_db: 2.0,
                makeup_gain_db: 2.0,
            },
        }
    }
}

/// Three-band compressor using Linkwitz-Riley crossover filters.
///
/// Signal flow:
/// ```text
///         ┌──[LR LP @ low_xover]──► low band compressor  ──┐
/// input ──┤                                                   ├──► output
///         ├──[LR HP @ low_xover + LR LP @ high_xover]─► mid ──┤
///         └──[LR HP @ high_xover]──► high band compressor ──┘
/// ```
pub struct MultibandCompressor {
    /// Linkwitz-Riley low-pass for low band extraction.
    lp_low: LinkwitzRiley,
    /// Linkwitz-Riley high-pass for low/mid split.
    hp_low: LinkwitzRiley,
    /// Linkwitz-Riley low-pass for mid band ceiling.
    lp_high: LinkwitzRiley,
    /// Linkwitz-Riley high-pass for high band extraction.
    hp_high: LinkwitzRiley,
    /// Per-band compressor instances.
    compressor_low: BandCompressor,
    compressor_mid: BandCompressor,
    compressor_high: BandCompressor,
    /// Audio sample rate.
    sample_rate: f32,
    /// Wet/dry mix (0.0 = dry, 1.0 = wet).
    wet_mix: f32,
}

impl MultibandCompressor {
    /// Create a new multi-band compressor.
    ///
    /// # Arguments
    /// * `config` - Crossover frequencies and per-band compressor settings.
    /// * `sample_rate` - Audio sample rate in Hz.
    #[must_use]
    pub fn new(config: MultibandCompressorConfig, sample_rate: f32) -> Self {
        let xl = config.crossover_low_hz.clamp(20.0, sample_rate * 0.45);
        let xh = config.crossover_high_hz.clamp(xl + 1.0, sample_rate * 0.45);

        Self {
            lp_low: LinkwitzRiley::lp(xl, sample_rate),
            hp_low: LinkwitzRiley::hp(xl, sample_rate),
            lp_high: LinkwitzRiley::lp(xh, sample_rate),
            hp_high: LinkwitzRiley::hp(xh, sample_rate),
            compressor_low: BandCompressor::new(config.low_band, sample_rate),
            compressor_mid: BandCompressor::new(config.mid_band, sample_rate),
            compressor_high: BandCompressor::new(config.high_band, sample_rate),
            sample_rate,
            wet_mix: 1.0,
        }
    }

    /// Process a buffer of mono samples in-place.
    pub fn process_buffer(&mut self, buffer: &mut [f32]) {
        for sample in buffer.iter_mut() {
            *sample = self.process_one(*sample);
        }
    }

    /// Process a single sample through all three bands.
    #[inline]
    pub fn process_one(&mut self, x: f32) -> f32 {
        // Split into three bands using Linkwitz-Riley crossovers
        let low = self.lp_low.process(x);
        let hp_from_low = self.hp_low.process(x);
        let mid = self.lp_high.process(hp_from_low);
        let high = self.hp_high.process(hp_from_low);

        // Apply independent compression to each band
        let low_out = self.compressor_low.process(low);
        let mid_out = self.compressor_mid.process(mid);
        let high_out = self.compressor_high.process(high);

        // Sum bands (Linkwitz-Riley guarantees flat summed response)
        low_out + mid_out + high_out
    }

    /// Set wet/dry mix ratio.
    pub fn set_wet_mix(&mut self, wet: f32) {
        self.wet_mix = wet.clamp(0.0, 1.0);
    }

    /// Get wet/dry mix ratio.
    #[must_use]
    pub fn wet_mix(&self) -> f32 {
        self.wet_mix
    }

    /// Update low band compressor settings.
    pub fn set_low_band(&mut self, config: CompressorConfig) {
        self.compressor_low.set_config(config, self.sample_rate);
    }

    /// Update mid band compressor settings.
    pub fn set_mid_band(&mut self, config: CompressorConfig) {
        self.compressor_mid.set_config(config, self.sample_rate);
    }

    /// Update high band compressor settings.
    pub fn set_high_band(&mut self, config: CompressorConfig) {
        self.compressor_high.set_config(config, self.sample_rate);
    }

    /// Get current gain reduction in dB for each band (low, mid, high).
    #[must_use]
    pub fn gain_reduction_db(&self) -> (f32, f32, f32) {
        (
            -self.compressor_low.smoothed_gr_db,
            -self.compressor_mid.smoothed_gr_db,
            -self.compressor_high.smoothed_gr_db,
        )
    }
}

impl AudioEffect for MultibandCompressor {
    const EFFECT_ID: u64 = 6014;

    fn process_sample(&mut self, input: f32) -> f32 {
        let processed = self.process_one(input);
        let wet = self.wet_mix;
        processed * wet + input * (1.0 - wet)
    }

    fn reset(&mut self) {
        self.lp_low.reset();
        self.hp_low.reset();
        self.lp_high.reset();
        self.hp_high.reset();
        self.compressor_low.reset();
        self.compressor_mid.reset();
        self.compressor_high.reset();
    }

    fn wet_mix(&self) -> f32 {
        self.wet_mix
    }

    fn set_wet_mix(&mut self, wet: f32) {
        self.wet_mix = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sine(freq_hz: f32, sample_rate: f32, num_samples: usize) -> Vec<f32> {
        use std::f32::consts::TAU;
        (0..num_samples)
            .map(|i| (i as f32 * TAU * freq_hz / sample_rate).sin())
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    #[test]
    fn test_linkwitz_riley_lp_output_finite() {
        let mut lp = LinkwitzRiley::lp(500.0, 48000.0);
        for _ in 0..256 {
            let y = lp.process(0.5);
            assert!(y.is_finite());
        }
    }

    #[test]
    fn test_linkwitz_riley_hp_output_finite() {
        let mut hp = LinkwitzRiley::hp(500.0, 48000.0);
        for _ in 0..256 {
            let y = hp.process(0.5);
            assert!(y.is_finite());
        }
    }

    #[test]
    fn test_linkwitz_riley_sum_near_unity() {
        // LR LP + LR HP at same crossover should sum to ~unity (flat frequency response)
        let sr = 48000.0;
        let xover = 1000.0;
        let mut lp = LinkwitzRiley::lp(xover, sr);
        let mut hp = LinkwitzRiley::hp(xover, sr);

        // Use a frequency well away from crossover
        let input = make_sine(100.0, sr, 2048);
        let lp_out: Vec<f32> = input.iter().map(|&x| lp.process(x)).collect();
        let hp_out: Vec<f32> = input.iter().map(|&x| hp.process(x)).collect();
        let sum: Vec<f32> = lp_out
            .iter()
            .zip(hp_out.iter())
            .map(|(&l, &h)| l + h)
            .collect();

        // After settling (skip first 256 samples), sum RMS should be close to input RMS
        let in_rms = rms(&input[256..]);
        let sum_rms = rms(&sum[256..]);
        assert!(
            (sum_rms - in_rms).abs() < 0.1,
            "LR sum should be near unity: in={in_rms}, sum={sum_rms}"
        );
    }

    #[test]
    fn test_multiband_compressor_output_finite() {
        let config = MultibandCompressorConfig::default();
        let mut mbc = MultibandCompressor::new(config, 48000.0);

        for _ in 0..1024 {
            let out = mbc.process_one(0.5);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_multiband_compressor_audioeffect_trait() {
        let config = MultibandCompressorConfig::default();
        let mut mbc = MultibandCompressor::new(config, 48000.0);

        let out = mbc.process_sample(0.3);
        assert!(out.is_finite());
    }

    #[test]
    fn test_multiband_compressor_process_buffer() {
        let config = MultibandCompressorConfig::default();
        let mut mbc = MultibandCompressor::new(config, 48000.0);

        let mut buf = vec![0.4f32; 512];
        mbc.process_buffer(&mut buf);
        assert!(buf.iter().all(|&s| s.is_finite()));
    }

    #[test]
    fn test_multiband_compressor_reset() {
        let config = MultibandCompressorConfig::default();
        let mut mbc = MultibandCompressor::new(config, 48000.0);

        let mut buf = vec![0.9f32; 512];
        mbc.process_buffer(&mut buf);
        mbc.reset();

        // After reset, state is zero
        let out = mbc.process_one(0.0);
        assert_eq!(out, 0.0);
    }

    #[test]
    fn test_multiband_compressor_reduces_loud_signal() {
        let config = MultibandCompressorConfig {
            low_band: CompressorConfig {
                threshold_db: -6.0,
                ratio: 10.0,
                attack_ms: 1.0,
                release_ms: 50.0,
                knee_db: 0.0,
                makeup_gain_db: 0.0,
            },
            mid_band: CompressorConfig {
                threshold_db: -6.0,
                ratio: 10.0,
                attack_ms: 1.0,
                release_ms: 50.0,
                knee_db: 0.0,
                makeup_gain_db: 0.0,
            },
            high_band: CompressorConfig {
                threshold_db: -6.0,
                ratio: 10.0,
                attack_ms: 1.0,
                release_ms: 50.0,
                knee_db: 0.0,
                makeup_gain_db: 0.0,
            },
            ..Default::default()
        };
        let mut mbc = MultibandCompressor::new(config, 48000.0);

        // Process a loud signal for long enough to trigger compression
        let input = vec![0.9f32; 4096];
        let mut output = input.clone();
        mbc.process_buffer(&mut output);

        let in_rms = rms(&input[2048..]);
        let out_rms = rms(&output[2048..]);
        assert!(
            out_rms < in_rms,
            "Multi-band compressor should reduce loud signal: in={in_rms}, out={out_rms}"
        );
    }

    #[test]
    fn test_multiband_wet_dry_mix() {
        let config = MultibandCompressorConfig::default();
        let mut mbc = MultibandCompressor::new(config, 48000.0);

        assert_eq!(mbc.wet_mix(), 1.0);
        mbc.set_wet_mix(0.5);
        assert_eq!(mbc.wet_mix(), 0.5);
        mbc.set_wet_mix(2.0);
        assert_eq!(mbc.wet_mix(), 1.0);
    }

    #[test]
    fn test_multiband_gain_reduction_db() {
        let config = MultibandCompressorConfig::default();
        let mut mbc = MultibandCompressor::new(config, 48000.0);

        for _ in 0..512 {
            mbc.process_one(0.9);
        }

        let (low_gr, mid_gr, high_gr) = mbc.gain_reduction_db();
        assert!(low_gr.is_finite());
        assert!(mid_gr.is_finite());
        assert!(high_gr.is_finite());
    }

    #[test]
    fn test_mastering_preset() {
        let config = MultibandCompressorConfig::mastering();
        let mut mbc = MultibandCompressor::new(config, 48000.0);
        let input: Vec<f32> = make_sine(440.0, 48000.0, 2048);
        let mut buf = input.clone();
        mbc.process_buffer(&mut buf);
        assert!(buf.iter().all(|&s| s.is_finite()));
    }

    #[test]
    fn test_broadcast_preset() {
        let config = MultibandCompressorConfig::broadcast();
        let mut mbc = MultibandCompressor::new(config, 48000.0);
        let mut buf = vec![0.7f32; 2048];
        mbc.process_buffer(&mut buf);
        assert!(buf.iter().all(|&s| s.is_finite()));
    }

    #[test]
    fn test_band_compressor_process_finite() {
        let config = CompressorConfig::standard();
        let mut bc = BandCompressor::new(config, 48000.0);
        for _ in 0..512 {
            let out = bc.process(0.5);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_biquad_butterworth_lp_finite() {
        let mut bq = Biquad::butterworth_lp(1000.0, 48000.0);
        for _ in 0..256 {
            let y = bq.process(0.5);
            assert!(y.is_finite());
        }
    }

    #[test]
    fn test_biquad_butterworth_hp_finite() {
        let mut bq = Biquad::butterworth_hp(1000.0, 48000.0);
        for _ in 0..256 {
            let y = bq.process(0.5);
            assert!(y.is_finite());
        }
    }
}
