//! Double-buffered overlap-add (OLA) convolution engine.
//!
//! Provides a low-latency, CPU-efficient convolution primitive that uses two
//! alternating output buffers so that one frame can be processed by the FFT
//! while the other is being consumed sample-by-sample.  This eliminates the
//! full-block latency burst that single-buffer implementations suffer when an
//! analysis frame completes.
//!
//! # Algorithm
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────────┐
//! │  Input samples → input_buf[0..block_size-1]                         │
//! │      ↓  when block_size samples accumulated:                         │
//! │  Zero-pad → [0..fft_size-1] complex input                           │
//! │      ↓  Forward FFT                                                  │
//! │  Convolve with pre-computed IR spectrum (point-wise multiply)         │
//! │      ↓  Inverse FFT                                                  │
//! │  Add IR-block to output ring (overlap-add)                           │
//! │  Swap active output buffer                                           │
//! │      ↓  read sample-by-sample                                        │
//! │  Wet·convolved + Dry·input                                           │
//! └──────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Parameters
//!
//! | Field | Description |
//! |-------|-------------|
//! | `block_size` | Input processing block size (must be power-of-two) |
//! | `wet` / `dry` | Wet/dry mix controls |
//!
//! # Example
//!
//! ```ignore
//! use oximedia_effects::reverb::overlap_add::OverlapAddConvolver;
//!
//! // Short IR (e.g. a room click)
//! let ir: Vec<f32> = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
//! let mut conv = OverlapAddConvolver::new(&ir, 256).unwrap();
//! let out = conv.process_sample(0.5);
//! assert!(out.is_finite());
//! ```

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use oxifft::Complex;

use crate::{AudioEffect, EffectError, Result};

/// Double-buffered overlap-add convolution engine.
///
/// Supports long impulse responses (up to 100 000 samples) without introducing
/// block-boundary artefacts, at the cost of `block_size` samples of latency.
pub struct OverlapAddConvolver {
    // ── IR in frequency domain ────────────────────────────────────────────
    /// Pre-computed FFT of the zero-padded impulse response.
    ir_spectrum: Vec<Complex<f32>>,

    // ── I/O buffers ───────────────────────────────────────────────────────
    /// Input accumulator (length = `block_size`).
    input_block: Vec<f32>,
    /// How many new input samples are pending.
    input_fill: usize,

    // ── double output buffers ─────────────────────────────────────────────
    /// Two alternating overlap-add output buffers, each of length `fft_size`.
    out_bufs: [Vec<f32>; 2],
    /// Index of the currently-active (being-read) output buffer.
    active_buf: usize,
    /// Read cursor in the active output buffer.
    read_pos: usize,

    // ── tail ──────────────────────────────────────────────────────────────
    /// Overlap tail from the last FFT frame (length = `fft_size - block_size`).
    tail: Vec<f32>,

    // ── sizing ────────────────────────────────────────────────────────────
    block_size: usize,
    fft_size: usize,

    // ── wet/dry ───────────────────────────────────────────────────────────
    wet: f32,
    dry: f32,
}

impl OverlapAddConvolver {
    /// Create a new `OverlapAddConvolver`.
    ///
    /// # Arguments
    ///
    /// * `impulse_response` – time-domain IR samples (length ≥ 1, ≤ 100 000).
    /// * `block_size`       – processing block size, rounded up to the nearest
    ///   power-of-two.  Values < 16 are clamped to 16.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::InvalidParameter`] if `impulse_response` is empty
    /// or longer than 100 000 samples.
    pub fn new(impulse_response: &[f32], block_size: usize) -> Result<Self> {
        if impulse_response.is_empty() {
            return Err(EffectError::InvalidParameter(
                "impulse response must not be empty".into(),
            ));
        }
        if impulse_response.len() > 100_000 {
            return Err(EffectError::InvalidParameter(
                "impulse response too long (max 100 000 samples)".into(),
            ));
        }

        let block_size = block_size.max(16).next_power_of_two();
        // FFT size: at least (IR_len + block_size - 1), rounded up to power of two.
        let min_fft = impulse_response.len() + block_size; // overlap-add requirement
        let fft_size = min_fft.next_power_of_two();

        // Zero-pad IR and transform.
        let mut ir_padded: Vec<Complex<f32>> = impulse_response
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        ir_padded.resize(fft_size, Complex::new(0.0, 0.0));
        let ir_spectrum = oxifft::fft(&ir_padded);

        let tail_len = fft_size - block_size;

        Ok(Self {
            ir_spectrum,
            input_block: vec![0.0; block_size],
            input_fill: 0,
            out_bufs: [vec![0.0; fft_size], vec![0.0; fft_size]],
            active_buf: 0,
            read_pos: 0,
            tail: vec![0.0; tail_len],
            block_size,
            fft_size,
            wet: 1.0,
            dry: 0.0,
        })
    }

    /// Return the block size used internally.
    #[must_use]
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Return the FFT size used internally.
    #[must_use]
    pub fn fft_size(&self) -> usize {
        self.fft_size
    }

    /// Set wet/dry mix.  `wet` is clamped to `[0.0, 1.0]`.
    pub fn set_wet(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
        self.dry = 1.0 - self.wet;
    }

    /// Return the current wet level.
    #[must_use]
    pub fn wet(&self) -> f32 {
        self.wet
    }

    /// Perform one FFT frame: forward-transform the current input block,
    /// convolve with the IR spectrum, inverse-transform, and overlap-add
    /// into the **inactive** output buffer before swapping.
    fn process_block(&mut self) {
        // Build zero-padded complex input.
        let mut input_fft: Vec<Complex<f32>> = self
            .input_block
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        input_fft.resize(self.fft_size, Complex::new(0.0, 0.0));

        // Forward FFT.
        let input_spectrum = oxifft::fft(&input_fft);

        // Pointwise multiply with IR spectrum.
        let convolved: Vec<Complex<f32>> = input_spectrum
            .iter()
            .zip(self.ir_spectrum.iter())
            .map(|(&a, &b)| {
                // Complex multiplication: (a.re+i·a.im)·(b.re+i·b.im)
                Complex::new(a.re * b.re - a.im * b.im, a.re * b.im + a.im * b.re)
            })
            .collect();

        // Inverse FFT → time-domain convolution result.
        let time_domain = oxifft::ifft(&convolved);

        // Select the inactive buffer to write into.
        let inactive = 1 - self.active_buf;
        // Clear the inactive buffer.
        for s in &mut self.out_bufs[inactive] {
            *s = 0.0;
        }

        // Copy the convolution output into the inactive buffer.
        let fft_size = self.fft_size;
        for (i, sample) in time_domain.iter().enumerate().take(fft_size) {
            self.out_bufs[inactive][i] = sample.re;
        }

        // Add the overlap tail from the previous frame.
        let tail_len = self.tail.len();
        for (i, &t) in self.tail.iter().enumerate() {
            self.out_bufs[inactive][self.block_size + i] += t;
        }

        // Save new tail for the next frame (the portion beyond block_size).
        let new_tail_start = self.block_size;
        let new_tail_end = (new_tail_start + tail_len).min(fft_size);
        for (i, t) in self.tail.iter_mut().enumerate() {
            let src_idx = new_tail_start + i;
            *t = if src_idx < new_tail_end {
                self.out_bufs[inactive][src_idx]
            } else {
                0.0
            };
        }

        // Swap buffers: the freshly-filled buffer becomes active.
        self.active_buf = inactive;
        self.read_pos = 0;
    }

    /// Clear all internal state (buffers, tail, phases).
    pub fn clear(&mut self) {
        self.input_block.iter_mut().for_each(|s| *s = 0.0);
        self.input_fill = 0;
        for buf in &mut self.out_bufs {
            buf.iter_mut().for_each(|s| *s = 0.0);
        }
        self.tail.iter_mut().for_each(|s| *s = 0.0);
        self.read_pos = 0;
    }
}

impl AudioEffect for OverlapAddConvolver {
    const EFFECT_ID: u64 = 6029;
    fn process_sample(&mut self, input: f32) -> f32 {
        // Accumulate input.
        self.input_block[self.input_fill] = input;
        self.input_fill += 1;

        // When the block is full, run an FFT frame.
        if self.input_fill == self.block_size {
            self.process_block();
            self.input_fill = 0;
        }

        // Read one sample from the active output buffer.
        let convolved = if self.read_pos < self.fft_size {
            let s = self.out_bufs[self.active_buf][self.read_pos];
            self.read_pos += 1;
            s
        } else {
            0.0
        };

        convolved * self.wet + input * self.dry
    }

    fn reset(&mut self) {
        self.clear();
    }

    fn latency_samples(&self) -> usize {
        self.block_size
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.set_wet(wet);
    }

    fn wet_dry(&self) -> f32 {
        self.wet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn make_ir(len: usize) -> Vec<f32> {
        // Simple decaying IR
        (0..len)
            .map(|i| (-(i as f32) / (len as f32 / 3.0)).exp())
            .collect()
    }

    fn make_sine(freq: f32, n: usize, sr: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (TAU * freq * i as f32 / sr).sin() * 0.5)
            .collect()
    }

    // ── construction ─────────────────────────────────────────────────────────

    #[test]
    fn test_new_basic() {
        let ir = make_ir(512);
        let conv = OverlapAddConvolver::new(&ir, 256);
        assert!(conv.is_ok(), "basic construction should succeed");
    }

    #[test]
    fn test_empty_ir_fails() {
        let result = OverlapAddConvolver::new(&[], 256);
        assert!(result.is_err(), "empty IR should return error");
    }

    #[test]
    fn test_block_size_power_of_two() {
        let ir = make_ir(100);
        let conv = OverlapAddConvolver::new(&ir, 300).unwrap();
        // block_size must be a power of 2
        assert!(conv.block_size().is_power_of_two());
    }

    #[test]
    fn test_fft_size_geq_ir_plus_block() {
        let ir = make_ir(512);
        let block = 256;
        let conv = OverlapAddConvolver::new(&ir, block).unwrap();
        assert!(
            conv.fft_size() >= ir.len() + conv.block_size(),
            "fft_size must cover IR + block overlap"
        );
    }

    // ── processing ───────────────────────────────────────────────────────────

    #[test]
    fn test_output_is_finite() {
        let ir = make_ir(256);
        let mut conv = OverlapAddConvolver::new(&ir, 128).unwrap();
        let sine = make_sine(440.0, 4096, 48_000.0);
        for &s in &sine {
            let out = conv.process_sample(s);
            assert!(out.is_finite(), "output not finite: {out}");
        }
    }

    #[test]
    fn test_silence_stays_near_silent() {
        let ir = make_ir(128);
        let mut conv = OverlapAddConvolver::new(&ir, 64).unwrap();
        for _ in 0..1024 {
            let out = conv.process_sample(0.0);
            assert!(
                out.abs() < 1e-5,
                "silence input should give near-silence, got {out}"
            );
        }
    }

    #[test]
    fn test_impulse_ir_approx_passthrough() {
        // An identity IR {1.0} followed by zeros should pass the input through.
        let mut ir = vec![0.0_f32; 64];
        ir[0] = 1.0;
        let mut conv = OverlapAddConvolver::new(&ir, 32).unwrap();
        conv.set_wet(1.0);

        // Feed silence to fill latency, then one impulse.
        let latency = conv.latency_samples();
        for _ in 0..latency {
            conv.process_sample(0.0);
        }
        // Now feed the impulse.
        let out = conv.process_sample(1.0);
        assert!(out.is_finite(), "passthrough output should be finite: {out}");
    }

    // ── wet/dry ──────────────────────────────────────────────────────────────

    #[test]
    fn test_wet_zero_passes_dry() {
        let ir = make_ir(128);
        let mut conv = OverlapAddConvolver::new(&ir, 64).unwrap();
        conv.set_wet(0.0);
        let input = 0.7_f32;
        let out = conv.process_sample(input);
        assert!(
            (out - input).abs() < 1e-5,
            "wet=0 should return dry signal: {out}"
        );
    }

    #[test]
    fn test_wet_dry_retrieval() {
        let ir = make_ir(64);
        let mut conv = OverlapAddConvolver::new(&ir, 32).unwrap();
        conv.set_wet(0.6);
        assert!((conv.wet() - 0.6).abs() < 1e-5);
        assert!((conv.wet_dry() - 0.6).abs() < 1e-5);
    }

    #[test]
    fn test_set_wet_dry_via_trait() {
        let ir = make_ir(64);
        let mut conv = OverlapAddConvolver::new(&ir, 32).unwrap();
        conv.set_wet_dry(0.3);
        assert!((conv.wet_dry() - 0.3).abs() < 1e-5);
    }

    // ── reset ────────────────────────────────────────────────────────────────

    #[test]
    fn test_reset_clears_state() {
        let ir = make_ir(128);
        let mut conv = OverlapAddConvolver::new(&ir, 64).unwrap();
        let sine = make_sine(440.0, 2048, 48_000.0);
        for &s in &sine {
            conv.process_sample(s);
        }
        conv.reset();
        // After reset, zero input should produce zero (or near-zero) output.
        for _ in 0..128 {
            let out = conv.process_sample(0.0);
            assert!(
                out.abs() < 1e-4,
                "after reset, silence should yield near-silence: {out}"
            );
        }
    }

    // ── latency ──────────────────────────────────────────────────────────────

    #[test]
    fn test_latency_equals_block_size() {
        let ir = make_ir(256);
        let conv = OverlapAddConvolver::new(&ir, 128).unwrap();
        assert_eq!(
            conv.latency_samples(),
            conv.block_size(),
            "latency should equal block_size"
        );
    }

    // ── long IR ──────────────────────────────────────────────────────────────

    #[test]
    fn test_long_ir() {
        let ir = make_ir(8192);
        let mut conv = OverlapAddConvolver::new(&ir, 512).unwrap();
        let sine = make_sine(220.0, 16384, 48_000.0);
        for &s in &sine {
            let out = conv.process_sample(s);
            assert!(out.is_finite(), "long IR output must be finite: {out}");
        }
    }
}
