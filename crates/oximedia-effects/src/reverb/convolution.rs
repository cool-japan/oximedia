//! Convolution reverb using impulse responses.
//!
//! Provides realistic room acoustics by convolving the input signal with
//! a recorded impulse response (IR) of a real or synthetic space.

use crate::{AudioEffect, EffectError, Result};
use oxifft::Complex;

/// Convolution reverb using frequency-domain convolution.
///
/// Implements partitioned convolution for efficient processing of long
/// impulse responses.
pub struct ConvolutionReverb {
    // Impulse response
    ir_fft: Vec<Complex<f32>>,
    #[allow(dead_code)]
    ir_length: usize,

    // FFT planner
    fft_size: usize,
    input_buffer: Vec<f32>,
    input_fft: Vec<Complex<f32>>,
    output_buffer: Vec<f32>,

    // Tail buffer (for overlap-add)
    tail_buffer: Vec<f32>,

    // Processing position
    input_pos: usize,
    output_pos: usize,

    // Parameters
    wet: f32,
    dry: f32,

    #[allow(dead_code)]
    sample_rate: f32,
}

impl ConvolutionReverb {
    /// Create a new convolution reverb.
    ///
    /// # Arguments
    ///
    /// * `impulse_response` - The impulse response samples
    /// * `sample_rate` - Audio sample rate
    ///
    /// # Errors
    ///
    /// Returns an error if the impulse response is empty or too long.
    pub fn new(impulse_response: &[f32], sample_rate: f32) -> Result<Self> {
        if impulse_response.is_empty() {
            return Err(EffectError::InvalidParameter(
                "Impulse response cannot be empty".into(),
            ));
        }

        if impulse_response.len() > 100_000 {
            return Err(EffectError::InvalidParameter(
                "Impulse response too long (max 100k samples)".into(),
            ));
        }

        let ir_length = impulse_response.len();

        // Choose FFT size (next power of 2 >= IR length * 2)
        let fft_size = (ir_length * 2).next_power_of_two();

        // Convert impulse response to frequency domain
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
            input_buffer: vec![0.0; fft_size],
            input_fft: vec![Complex::new(0.0, 0.0); fft_size],
            output_buffer: vec![0.0; fft_size],
            tail_buffer: vec![0.0; fft_size],
            input_pos: 0,
            output_pos: 0,
            wet: 0.5,
            dry: 0.5,
            sample_rate,
        })
    }

    /// Set wet level (0.0 - 1.0).
    pub fn set_wet(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
    }

    /// Set dry level (0.0 - 1.0).
    pub fn set_dry(&mut self, dry: f32) {
        self.dry = dry.clamp(0.0, 1.0);
    }

    /// Process a block of samples.
    fn process_block(&mut self) {
        // Copy input to complex buffer
        for (i, &sample) in self.input_buffer.iter().enumerate() {
            self.input_fft[i] = Complex::new(sample, 0.0);
        }

        // Forward FFT
        let fft_result = oxifft::fft(&self.input_fft);

        // Complex multiplication (convolution in frequency domain)
        let result_fft_freq: Vec<Complex<f32>> = fft_result
            .iter()
            .zip(self.ir_fft.iter())
            .map(|(&a, &b)| a * b)
            .collect();

        // Inverse FFT
        let result_fft = oxifft::ifft(&result_fft_freq);

        // Extract real part and normalize
        #[allow(clippy::cast_precision_loss)]
        let scale = 1.0 / self.fft_size as f32;

        for (i, val) in result_fft.iter().enumerate().take(self.fft_size) {
            self.output_buffer[i] = val.re * scale;
        }

        // Overlap-add with tail from previous block
        for i in 0..self.fft_size {
            self.output_buffer[i] += self.tail_buffer[i];
        }

        // Save second half as tail for next block
        for i in 0..self.fft_size / 2 {
            self.tail_buffer[i] = self.output_buffer[self.fft_size / 2 + i];
        }
        for i in self.fft_size / 2..self.fft_size {
            self.tail_buffer[i] = 0.0;
        }

        self.output_pos = 0;
    }
}

impl AudioEffect for ConvolutionReverb {
    const EFFECT_ID: u64 = 6027;
    fn process_sample(&mut self, input: f32) -> f32 {
        // Store input
        self.input_buffer[self.input_pos] = input;
        self.input_pos += 1;

        // When we have a full block, process it
        if self.input_pos >= self.fft_size / 2 {
            self.process_block();
            self.input_pos = 0;
            // Clear second half of input buffer for next block
            for i in self.fft_size / 2..self.fft_size {
                self.input_buffer[i] = 0.0;
            }
        }

        // Get output sample
        let wet_sample = if self.output_pos < self.output_buffer.len() {
            self.output_buffer[self.output_pos]
        } else {
            0.0
        };

        self.output_pos += 1;

        // Mix wet and dry
        wet_sample * self.wet + input * self.dry
    }

    fn reset(&mut self) {
        self.input_buffer.fill(0.0);
        self.output_buffer.fill(0.0);
        self.tail_buffer.fill(0.0);
        self.input_fft.fill(Complex::new(0.0, 0.0));
        self.input_pos = 0;
        self.output_pos = 0;
    }

    fn latency_samples(&self) -> usize {
        self.fft_size / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convolution_reverb_creation() {
        let ir = vec![1.0, 0.5, 0.25, 0.125]; // Simple exponential decay
        let reverb = ConvolutionReverb::new(&ir, 48000.0);
        assert!(reverb.is_ok());
    }

    #[test]
    fn test_convolution_reverb_empty_ir() {
        let ir: Vec<f32> = vec![];
        let result = ConvolutionReverb::new(&ir, 48000.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_convolution_reverb_process() {
        let ir = vec![1.0; 100]; // Simple IR
        let mut reverb = ConvolutionReverb::new(&ir, 48000.0).expect("test expectation failed");

        // Process impulse
        let output = reverb.process_sample(1.0);
        // Output might be delayed due to block processing
        assert!(output.is_finite());

        // Process more samples
        for _ in 0..1000 {
            let out = reverb.process_sample(0.0);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_convolution_wet_dry() {
        let ir = vec![0.5; 50];
        let mut reverb = ConvolutionReverb::new(&ir, 48000.0).expect("test expectation failed");

        reverb.set_wet(0.0);
        reverb.set_dry(1.0);

        // Process samples - with dry=1 and wet=0, should eventually get mostly dry signal
        for _ in 0..100 {
            reverb.process_sample(1.0);
        }

        let output = reverb.process_sample(1.0);
        // With wet=0, dry=1, output should be close to input
        assert!((output - 1.0).abs() < 0.5);
    }
}
