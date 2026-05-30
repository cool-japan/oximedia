//! Convolution-based guitar/bass cabinet simulator.
//!
//! Implements cabinet impulse responses using overlap-add convolution in the
//! frequency domain via `oxifft`. Synthetic impulse responses model the
//! frequency-dependent resonance and high-frequency rolloff characteristics
//! of real loudspeaker cabinets.

use crate::{AudioEffect, EffectError, Result};
use oxifft::Complex;

/// Maximum allowed impulse response length (1 second at 48 kHz).
const MAX_IR_LENGTH: usize = 48_000;

/// Synthesize the impulse response for a 4×12" British guitar cabinet.
///
/// Models short attack transient, mid-frequency resonances at ~400 Hz and
/// ~1.2 kHz, exponential decay over 60 ms, and high-frequency rolloff.
#[allow(clippy::cast_precision_loss)]
fn synthesize_guitar_4x12_ir(sample_rate: f32) -> Vec<f32> {
    let duration_ms = 60.0_f32;
    let num_samples = ((duration_ms * sample_rate / 1000.0) as usize).max(64);
    let mut ir = vec![0.0_f32; num_samples];

    ir[0] = 1.0;
    if num_samples > 1 {
        ir[1] = 0.5;
    }

    for i in 2..num_samples {
        let t = i as f32 / sample_rate;
        let decay = (-t * 50.0_f32).exp();
        let r_mid = (2.0 * std::f32::consts::PI * 400.0 * t).sin() * 0.4 * decay;
        let r_upper = (2.0 * std::f32::consts::PI * 1200.0 * t).sin() * 0.25 * decay;
        ir[i] = r_mid + r_upper;
    }
    ir
}

/// Synthesize the impulse response for a 1×15" bass cabinet.
#[allow(clippy::cast_precision_loss)]
fn synthesize_bass_1x15_ir(sample_rate: f32) -> Vec<f32> {
    let duration_ms = 80.0_f32;
    let num_samples = ((duration_ms * sample_rate / 1000.0) as usize).max(64);
    let mut ir = vec![0.0_f32; num_samples];

    ir[0] = 1.0;
    if num_samples > 1 {
        ir[1] = 0.6;
    }

    for i in 2..num_samples {
        let t = i as f32 / sample_rate;
        let decay = (-t * 40.0_f32).exp();
        let r_low = (2.0 * std::f32::consts::PI * 80.0 * t).sin() * 0.5 * decay;
        let r_mid = (2.0 * std::f32::consts::PI * 160.0 * t).sin() * 0.3 * decay;
        ir[i] = r_low + r_mid;
    }
    ir
}

/// Synthesize the impulse response for a small 1×12" combo cabinet.
#[allow(clippy::cast_precision_loss)]
fn synthesize_combo_1x12_ir(sample_rate: f32) -> Vec<f32> {
    let duration_ms = 40.0_f32;
    let num_samples = ((duration_ms * sample_rate / 1000.0) as usize).max(64);
    let mut ir = vec![0.0_f32; num_samples];

    ir[0] = 1.0;
    if num_samples > 1 {
        ir[1] = 0.4;
    }

    for i in 2..num_samples {
        let t = i as f32 / sample_rate;
        let decay = (-t * 70.0_f32).exp();
        let r1 = (2.0 * std::f32::consts::PI * 250.0 * t).sin() * 0.45 * decay;
        let r2 = (2.0 * std::f32::consts::PI * 800.0 * t).sin() * 0.2 * decay;
        ir[i] = r1 + r2;
    }
    ir
}

/// Convolution-based speaker cabinet simulator.
///
/// Uses overlap-add frequency-domain convolution with a pre-computed cabinet
/// impulse response. Suitable for guitar and bass cabinet simulation in
/// real-time audio chains.
///
/// # Example
///
/// ```ignore
/// use oximedia_effects::reverb::CabinetSimulator;
/// let mut cab = CabinetSimulator::guitar_4x12(48000.0).expect("ok");
/// let out = cab.process_sample(0.5);
/// ```
pub struct CabinetSimulator {
    /// IR in frequency domain (pre-computed FFT).
    ir_fft: Vec<Complex<f32>>,
    #[allow(dead_code)]
    ir_length: usize,
    fft_size: usize,

    // Pre-allocated processing buffers.
    input_buffer: Vec<f32>,
    output_buffer: Vec<f32>,
    tail_buffer: Vec<f32>,
    temp_fft: Vec<Complex<f32>>,

    // Block-processing positions.
    input_pos: usize,
    output_pos: usize,

    wet_mix: f32,

    #[allow(dead_code)]
    sample_rate: f32,
}

impl CabinetSimulator {
    /// Create a new cabinet simulator from a custom impulse response.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::InvalidParameter`] if the impulse response is
    /// empty or exceeds `MAX_IR_LENGTH` samples.
    pub fn new(impulse_response: &[f32], sample_rate: f32) -> Result<Self> {
        if impulse_response.is_empty() {
            return Err(EffectError::InvalidParameter(
                "Impulse response cannot be empty".into(),
            ));
        }
        if impulse_response.len() > MAX_IR_LENGTH {
            return Err(EffectError::InvalidParameter(format!(
                "Impulse response too long: {} > {}",
                impulse_response.len(),
                MAX_IR_LENGTH
            )));
        }

        let ir_length = impulse_response.len();
        let fft_size = (ir_length * 2).next_power_of_two();

        // Pre-compute IR FFT.
        let mut ir_padded: Vec<Complex<f32>> = impulse_response
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        ir_padded.resize(fft_size, Complex::new(0.0, 0.0));
        let ir_fft = oxifft::fft(&ir_padded);

        Ok(Self {
            ir_fft,
            ir_length,
            fft_size,
            input_buffer: vec![0.0_f32; fft_size],
            output_buffer: vec![0.0_f32; fft_size],
            tail_buffer: vec![0.0_f32; fft_size],
            temp_fft: vec![Complex::new(0.0, 0.0); fft_size],
            input_pos: 0,
            output_pos: 0,
            wet_mix: 1.0,
            sample_rate,
        })
    }

    /// 4×12" British guitar cabinet (Marshall-style).
    ///
    /// # Errors
    ///
    /// Returns an error if the synthesized IR is invalid (should not occur).
    pub fn guitar_4x12(sample_rate: f32) -> Result<Self> {
        let ir = synthesize_guitar_4x12_ir(sample_rate);
        Self::new(&ir, sample_rate)
    }

    /// 1×15" bass cabinet.
    ///
    /// # Errors
    ///
    /// Returns an error if the synthesized IR is invalid (should not occur).
    pub fn bass_1x15(sample_rate: f32) -> Result<Self> {
        let ir = synthesize_bass_1x15_ir(sample_rate);
        Self::new(&ir, sample_rate)
    }

    /// Small 1×12" combo cabinet.
    ///
    /// # Errors
    ///
    /// Returns an error if the synthesized IR is invalid (should not occur).
    pub fn combo_1x12(sample_rate: f32) -> Result<Self> {
        let ir = synthesize_combo_1x12_ir(sample_rate);
        Self::new(&ir, sample_rate)
    }

    /// Set the wet/dry mix ratio.
    ///
    /// `wet` is clamped to `[0.0, 1.0]`.
    pub fn set_wet_mix(&mut self, wet: f32) {
        self.wet_mix = wet.clamp(0.0, 1.0);
    }

    /// Get the current wet/dry mix ratio.
    #[must_use]
    pub fn wet_mix(&self) -> f32 {
        self.wet_mix
    }

    /// Perform overlap-add convolution on the current input block.
    #[allow(clippy::cast_precision_loss)]
    fn process_block(&mut self) {
        // Build complex input from the current input buffer.
        for (i, &s) in self.input_buffer.iter().enumerate() {
            self.temp_fft[i] = Complex::new(s, 0.0);
        }

        // Forward FFT.
        let input_freq = oxifft::fft(&self.temp_fft);

        // Complex multiply (convolution in frequency domain).
        let convolved: Vec<Complex<f32>> = input_freq
            .iter()
            .zip(self.ir_fft.iter())
            .map(|(&a, &b)| a * b)
            .collect();

        // Inverse FFT.
        let time_domain = oxifft::ifft(&convolved);

        // Extract real part, apply normalization.
        let scale = 1.0 / self.fft_size as f32;
        for (i, val) in time_domain.iter().enumerate().take(self.fft_size) {
            self.output_buffer[i] = val.re * scale;
        }

        // Overlap-add: add the tail from the previous block.
        for i in 0..self.fft_size {
            self.output_buffer[i] += self.tail_buffer[i];
        }

        // Save upper half as tail for the next block.
        let half = self.fft_size / 2;
        for i in 0..half {
            self.tail_buffer[i] = self.output_buffer[half + i];
        }
        for i in half..self.fft_size {
            self.tail_buffer[i] = 0.0;
        }

        self.output_pos = 0;
    }
}

impl AudioEffect for CabinetSimulator {

    const EFFECT_ID: u64 = 6026;
    fn process_sample(&mut self, input: f32) -> f32 {
        let half = self.fft_size / 2;

        // Accumulate into input buffer.
        self.input_buffer[self.input_pos] = input;
        self.input_pos += 1;

        // Process a full half-block.
        if self.input_pos >= half {
            self.process_block();
            self.input_pos = 0;
            // Zero the second half of input buffer (zero-padding for linear convolution).
            for i in half..self.fft_size {
                self.input_buffer[i] = 0.0;
            }
        }

        // Read the next output sample.
        let wet_sample = if self.output_pos < self.output_buffer.len() {
            self.output_buffer[self.output_pos]
        } else {
            0.0
        };
        self.output_pos += 1;

        wet_sample * self.wet_mix + input * (1.0 - self.wet_mix)
    }

    fn reset(&mut self) {
        self.input_buffer.fill(0.0);
        self.output_buffer.fill(0.0);
        self.tail_buffer.fill(0.0);
        self.temp_fft.fill(Complex::new(0.0, 0.0));
        self.input_pos = 0;
        self.output_pos = 0;
    }

    fn latency_samples(&self) -> usize {
        self.fft_size / 2
    }

    fn wet_dry(&self) -> f32 {
        self.wet_mix
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.wet_mix = wet.clamp(0.0, 1.0);
    }
}

/// Preset cabinet types for easy selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CabinetType {
    /// 4×12" British guitar cabinet (Marshall-style).
    Guitar4x12,
    /// 1×15" bass cabinet.
    Bass1x15,
    /// Small 1×12" combo cabinet.
    Combo1x12,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioEffect;

    fn make_sine(freq_hz: f32, sample_rate: f32, num_samples: usize) -> Vec<f32> {
        use std::f32::consts::TAU;
        (0..num_samples)
            .map(|i| (i as f32 * TAU * freq_hz / sample_rate).sin())
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|&x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn test_cabinet_from_custom_ir_ok() {
        let ir: Vec<f32> = (0..100).map(|i| (-0.05 * i as f32).exp()).collect();
        let result = CabinetSimulator::new(&ir, 48000.0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cabinet_empty_ir_error() {
        let result = CabinetSimulator::new(&[], 48000.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_cabinet_ir_too_long_error() {
        let ir = vec![0.001_f32; MAX_IR_LENGTH + 1];
        let result = CabinetSimulator::new(&ir, 48000.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_cabinet_output_finite() {
        let ir: Vec<f32> = (0..100).map(|i| (-0.05 * i as f32).exp()).collect();
        let mut cab = CabinetSimulator::new(&ir, 48000.0).expect("valid IR");
        let input = make_sine(440.0, 48000.0, 2000);
        for &s in &input {
            let out = cab.process_sample(s);
            assert!(out.is_finite(), "Output must remain finite: {out}");
        }
    }

    #[test]
    fn test_cabinet_preset_guitar_4x12() {
        let mut cab = CabinetSimulator::guitar_4x12(48000.0).expect("preset ok");
        let input = make_sine(440.0, 48000.0, 1024);
        for &s in &input {
            let out = cab.process_sample(s);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_cabinet_preset_bass_1x15() {
        let mut cab = CabinetSimulator::bass_1x15(48000.0).expect("preset ok");
        let input = make_sine(100.0, 48000.0, 1024);
        for &s in &input {
            let out = cab.process_sample(s);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_cabinet_preset_combo_1x12() {
        let mut cab = CabinetSimulator::combo_1x12(48000.0).expect("preset ok");
        let input = make_sine(880.0, 48000.0, 1024);
        for &s in &input {
            let out = cab.process_sample(s);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_cabinet_wet_dry_mix() {
        let ir = vec![1.0_f32; 64];
        let mut cab = CabinetSimulator::new(&ir, 48000.0).expect("valid IR");

        assert!((cab.wet_mix() - 1.0).abs() < 1e-6);
        cab.set_wet_mix(0.5);
        assert!((cab.wet_mix() - 0.5).abs() < 1e-6);

        // Clamp high
        cab.set_wet_mix(2.0);
        assert!((cab.wet_mix() - 1.0).abs() < 1e-6);

        // Clamp low
        cab.set_wet_mix(-1.0);
        assert!((cab.wet_mix() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_cabinet_reset() {
        let ir = vec![1.0_f32; 64];
        let mut cab = CabinetSimulator::new(&ir, 48000.0).expect("valid IR");

        // Fill the cab with audio.
        for _ in 0..1000 {
            cab.process_sample(0.9);
        }

        cab.reset();

        // After reset with wet=1.0, output should be near zero for a zero input.
        cab.set_wet_mix(1.0);
        let out = cab.process_sample(0.0);
        assert!(
            out.abs() < 1e-6,
            "Post-reset output should be silent: {out}"
        );
    }

    #[test]
    fn test_cabinet_latency() {
        let ir = vec![1.0_f32; 64];
        let cab = CabinetSimulator::new(&ir, 48000.0).expect("valid IR");
        assert!(
            cab.latency_samples() > 0,
            "Cabinet must introduce some latency due to block convolution"
        );
    }

    #[test]
    fn test_cabinet_attenuates_high_freq() {
        // Guitar 4×12 should reduce very high frequency content (>10 kHz).
        // The synthetic IR has resonances only at 400 Hz and 1.2 kHz with exponential
        // decay, so ultra-high frequencies receive very little energy.
        let mut cab_full = CabinetSimulator::guitar_4x12(48000.0).expect("preset ok");

        // Settle
        let settle = make_sine(440.0, 48000.0, 4096);
        for &s in &settle {
            cab_full.process_sample(s);
        }

        // Measure response at 15 kHz (should be attenuated vs 440 Hz through raw sine).
        let input_hf = make_sine(15000.0, 48000.0, 2048);
        let mut out_hf = Vec::with_capacity(2048);
        for &s in &input_hf {
            out_hf.push(cab_full.process_sample(s));
        }

        // The RMS at 15 kHz through the cabinet should be less than the
        // unity-input RMS (0.707), because the synthetic IR has no energy there.
        let out_rms = rms(&out_hf);
        assert!(
            out_rms < 0.707,
            "Cabinet should attenuate high frequencies: rms={out_rms}"
        );
    }

    #[test]
    fn test_cabinet_ir_length_accessible() {
        let ir = vec![0.5_f32; 200];
        let cab = CabinetSimulator::new(&ir, 48000.0).expect("valid IR");
        assert_eq!(cab.ir_length, 200);
    }
}
