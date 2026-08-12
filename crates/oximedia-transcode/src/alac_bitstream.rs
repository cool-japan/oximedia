// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Spec-shaped ALAC frame encoder/decoder: escape (uncompressed) *and*
//! compressed (adaptive Rice + LPC) element forms — **both verified
//! against a real reference decoder (FFmpeg)**, not just self-consistency.
//!
//! The workspace's other ALAC encoder (`oximedia_codec::alac`) round-trips
//! against its own decoder but its compressed elements are rejected by
//! reference decoders, which is why the transcode pipeline has always had
//! its own escape-only encoder here. This module extends that encoder with
//! a real compressed element form:
//!
//! - **Escape (uncompressed)** — every element written as raw 16-bit
//!   signed samples. Bit-exact lossless, decodable by any conformant ALAC
//!   decoder. Verified against FFmpeg.
//! - **Compressed** — adaptive Golomb/modified-Rice residual coding plus a
//!   sliding-window LPC predictor with sign-sign-LMS coefficient
//!   adaptation, following the format Apple published as open source
//!   (`ALACEncoder.cpp`/`ALACDecoder.cpp`, `dp_enc.c`/`dp_dec.c`,
//!   `ag_enc.c`/`ag_dec.c`) and cross-checked against FFmpeg's
//!   from-scratch reimplementation (`libavcodec/alac.c`, `alacenc.c`,
//!   `alacdsp.c`). **Verified against FFmpeg** (`test_ffmpeg_can_decode_compressed_output`
//!   and its mono sibling — both run for real, not skipped, wherever
//!   `ffmpeg` is on `PATH`, as it is in this session's environment).
//!
//! Two simplifications versus Apple's reference encoder, made deliberately
//! because they affect only *compression ratio*, never correctness or
//! interop (both this module's own decoder *and* FFmpeg's decode these
//! identically regardless):
//!
//! - LPC coefficients always start at zero and are shaped purely by the
//!   sign-sign-LMS adaptation loop as each block is coded, rather than
//!   being seeded per-block via Levinson-Durbin analysis.
//! - Stereo pairs always use ALAC's "mid/side" decorrelation mode
//!   (`shift=1`, `weight=1` — algebraically reversible for any input, not
//!   just content where it happens to compress best), never the
//!   independent-channel or left/side or right/side alternatives.
//!
//! `AlacStreamEncoder::encode_block` (used by `run_alac_job` in
//! `frame_level.rs` — the actual `.caf` output path) picks compressed or
//! escape per block automatically: it only emits compressed output when
//! this module's own decoder round-trips it back to the exact input *and*
//! the compressed bytes are smaller than the escape form, so output is
//! never fabricated and never larger than the escape-only baseline this
//! module shipped with before. That self-verification is a safety net
//! against a *regression* in this specific encoder configuration, not a
//! hedge on the FFmpeg finding above — the automatic policy's fixed
//! `(order=8, quant=9)` choice is exactly what the FFmpeg cross-check
//! tests exercise.
//!
//! One real bug this cross-check caught and fixed, left recorded on
//! `lpc_predict` and on `test_ffmpeg_can_decode_compressed_output`: an LPC
//! coefficient/tap index pairing that was internally self-consistent (this
//! module's encoder and decoder agreed with each other, so every
//! self-only test passed) but didn't match how FFmpeg's decoder applies
//! the same coefficient array — a class of bug self-consistency testing
//! structurally cannot catch, which is why the FFmpeg cross-check exists
//! at all rather than being redundant with the round-trip tests.
//!
//! # Element layout (both forms share this header)
//!
//! ```text
//! [3b element tag: SCE=0 mono / CPE=1 pair]
//! [4b element instance tag = 0]
//! [12b unused = 0]
//! [1b partial-frame flag]
//! [2b bytes shifted (this module always writes/expects 0)]
//! [1b escape: 1 = uncompressed, 0 = compressed]
//! [32b sample count, only when partial]
//! ---- escape form ----
//! [samples: sample-major, channel-interleaved, 16-bit signed]
//! ---- compressed form (only when 1 or 2 channels) ----
//! [8b stereo decorrelation shift] [8b stereo decorrelation weight]
//! per channel: [4b prediction type = 0] [4b LPC quant shift]
//!              [3b Rice history-mult modifier] [5b LPC order]
//!              [order x 16b signed LPC coefficients]
//! per channel: adaptive-Rice-coded residuals (order: same as coefficients)
//! ```
//!
//! The frame ends with the `END` tag (7) and zero padding to a byte
//! boundary.

use crate::flac_bitstream::BitWriter;
use crate::{Result, TranscodeError};

/// ALAC syntax element tags (AAC-style enumeration: SCE=0, CPE=1, END=7).
const ID_SCE: u64 = 0; // single channel element
const ID_CPE: u64 = 1; // channel pair element
const ID_END: u64 = 7;

/// `pb` (Rice history-adaptation multiplier) from the magic cookie —
/// Apple's documented default.
const PB: u32 = 40;
/// `mb` (Rice initial history) from the magic cookie — Apple's default.
const MB: u32 = 10;
/// `kb` (Rice parameter ceiling) from the magic cookie — Apple's default.
const KB: u32 = 14;
/// Per-channel 3-bit Rice-modifier field this module always writes:
/// `RICE_MODIFIER * PB / 4 == PB`, i.e. "no modification" — any value 0-7
/// is format-legal (it only scales history adaptation speed), this is
/// simply the conventional neutral choice.
const RICE_MODIFIER: u32 = 4;
/// Highest LPC order this module emits or decodes. Real ALAC reserves
/// order 31 as a special fixed first-order-difference predictor decoded
/// without reading any coefficients; this module does not implement that
/// sentinel (it never emits it, so this only matters for streams produced
/// by *other* encoders, which this module does not claim to decode).
const MAX_LPC_ORDER: u8 = 30;

/// A spec-compliant ALAC frame encoder for 16-bit interleaved PCM.
pub struct AlacStreamEncoder {
    sample_rate: u32,
    channels: u16,
    frame_length: u32,
}

impl AlacStreamEncoder {
    /// Bits per sample handled by this encoder.
    const BPS: u32 = 16;

    /// Creates an encoder with the given frames-per-packet.
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::InvalidInput`] for out-of-range parameters.
    pub fn new(sample_rate: u32, channels: u16, frame_length: u32) -> Result<Self> {
        if !(1..=8).contains(&channels) {
            return Err(TranscodeError::InvalidInput(format!(
                "ALAC supports 1-8 channels, got {channels}"
            )));
        }
        if sample_rate == 0 || frame_length == 0 {
            return Err(TranscodeError::InvalidInput(
                "ALAC sample rate and frame length must be non-zero".into(),
            ));
        }
        Ok(Self {
            sample_rate,
            channels,
            frame_length,
        })
    }

    /// The 24-byte `ALACSpecificConfig` magic cookie for this stream.
    #[must_use]
    pub fn magic_cookie(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24);
        out.extend_from_slice(&self.frame_length.to_be_bytes());
        out.push(0); // compatible version
        out.push(Self::BPS as u8); // bit depth
        out.push(PB as u8);
        out.push(MB as u8);
        out.push(KB as u8);
        out.push(self.channels as u8);
        out.extend_from_slice(&255u16.to_be_bytes()); // maxRun
        out.extend_from_slice(&0u32.to_be_bytes()); // maxFrameBytes (unknown)
        out.extend_from_slice(&0u32.to_be_bytes()); // avgBitRate (unknown)
        out.extend_from_slice(&self.sample_rate.to_be_bytes());
        out
    }

    /// Encode one block of interleaved i16 samples into a complete ALAC
    /// frame, choosing the smaller of the escape and compressed forms.
    ///
    /// Compressed output is only used when it (a) is smaller than escape
    /// and (b) round-trips exactly through this module's own decoder —
    /// otherwise this falls back to the escape form, so output is never
    /// larger than the escape-only baseline and never unverified-lossy.
    /// Block sizes 1..=`frame_length` per channel.
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::CodecError`] on empty/ragged/oversized
    /// blocks.
    pub fn encode_block(&self, interleaved: &[i16]) -> Result<Vec<u8>> {
        let escape = self.encode_block_escape(interleaved)?;

        if (1..=2).contains(&self.channels) {
            if let Ok(compressed) = self.encode_block_compressed_with_params(interleaved, 8, 9) {
                if compressed.len() < escape.len() {
                    let decoder = AlacStreamDecoder::new(self.channels, self.frame_length)?;
                    if let Ok(round_tripped) = decoder.decode_block(&compressed) {
                        if round_tripped == interleaved {
                            return Ok(compressed);
                        }
                    }
                }
            }
        }
        Ok(escape)
    }

    /// Validates a block and returns `(channels, num_samples, partial)`.
    fn validate_block(&self, interleaved: &[i16]) -> Result<(usize, usize, bool)> {
        let ch = usize::from(self.channels);
        if interleaved.is_empty() || interleaved.len() % ch != 0 {
            return Err(TranscodeError::CodecError(format!(
                "ALAC block of {} samples is not a multiple of {ch} channels",
                interleaved.len()
            )));
        }
        let num_samples = interleaved.len() / ch;
        if num_samples > self.frame_length as usize {
            return Err(TranscodeError::CodecError(format!(
                "ALAC block of {num_samples} samples exceeds the {}-sample frame length",
                self.frame_length
            )));
        }
        let partial = num_samples != self.frame_length as usize;
        Ok((ch, num_samples, partial))
    }

    /// Encode one block using the escape (uncompressed) element form —
    /// the mux-side default; see the module doc for why.
    fn encode_block_escape(&self, interleaved: &[i16]) -> Result<Vec<u8>> {
        let (ch, num_samples, partial) = self.validate_block(interleaved)?;

        let mut bw = BitWriter::new();
        // Group channels into stereo pairs with a trailing mono element.
        let mut c = 0usize;
        while c < ch {
            let pair = c + 1 < ch;
            write_element_header(&mut bw, pair, partial, num_samples, true);
            let width = if pair { 2 } else { 1 };
            for s in 0..num_samples {
                for k in 0..width {
                    let sample = interleaved[s * ch + c + k];
                    bw.write_bits(sample as u64, Self::BPS);
                }
            }
            c += width;
        }
        bw.write_bits(ID_END, 3);
        Ok(bw.into_bytes())
    }

    /// Encode one block using the compressed (adaptive Rice + LPC)
    /// element form with an explicit `(order, quant)` choice, for callers
    /// that want direct control (this module's own automatic policy in
    /// [`encode_block`](Self::encode_block), and tests exercising a range
    /// of predictor/Rice parameters).
    ///
    /// Only 1 or 2 channels are supported (matching the single-element
    /// case this module actually uses/verifies); higher channel counts
    /// return an error rather than silently falling back.
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::CodecError`] for invalid blocks, and
    /// [`TranscodeError::Unsupported`] for `order`/`quant` out of range or
    /// more than 2 channels.
    pub fn encode_block_compressed_with_params(
        &self,
        interleaved: &[i16],
        order: u8,
        quant: u32,
    ) -> Result<Vec<u8>> {
        let (ch, num_samples, partial) = self.validate_block(interleaved)?;
        if ch > 2 {
            return Err(TranscodeError::Unsupported(format!(
                "ALAC compressed encode only supports 1-2 channels, got {ch}"
            )));
        }
        validate_order_quant(order, quant)?;

        let pair = ch == 2;
        let mut channel_samples: Vec<Vec<i64>> = vec![Vec::with_capacity(num_samples); ch];
        for s in 0..num_samples {
            for (k, chan) in channel_samples.iter_mut().enumerate() {
                chan.push(i64::from(interleaved[s * ch + k]));
            }
        }

        let mut bw = BitWriter::new();
        write_element_header(&mut bw, pair, partial, num_samples, false);

        let (decorr_shift, decorr_weight, mixed) = if pair {
            let (left, right) = (&channel_samples[0], &channel_samples[1]);
            let mut mid = Vec::with_capacity(num_samples);
            let mut side = Vec::with_capacity(num_samples);
            for k in 0..num_samples {
                mid.push((left[k] + right[k]) >> 1);
                side.push(left[k] - right[k]);
            }
            (1u32, 1u32, vec![mid, side])
        } else {
            (0u32, 0u32, channel_samples)
        };
        bw.write_bits(u64::from(decorr_shift), 8);
        bw.write_bits(u64::from(decorr_weight), 8);

        let write_sample_size = if pair { 17 } else { 16 };
        let order_usize = usize::from(order);

        let mut residuals_per_channel = Vec::with_capacity(mixed.len());
        for samples in &mixed {
            residuals_per_channel.push(lpc_encode(samples, order_usize, quant));
        }

        for _ in &mixed {
            bw.write_bits(0, 4); // prediction type: always the plain single-pass filter
            bw.write_bits(u64::from(quant), 4);
            bw.write_bits(u64::from(RICE_MODIFIER), 3);
            bw.write_bits(order as u64, 5);
            for _ in 0..order {
                bw.write_bits(0i16 as u16 as u64, 16); // coefficients always start at zero
            }
        }

        let history_mult = RICE_MODIFIER * PB / 4;
        for residual in &residuals_per_channel {
            rice_encode_channel(&mut bw, residual, write_sample_size, KB, history_mult);
        }

        bw.write_bits(ID_END, 3);
        Ok(bw.into_bytes())
    }
}

/// Writes the element header shared by both forms.
fn write_element_header(
    bw: &mut BitWriter,
    pair: bool,
    partial: bool,
    num_samples: usize,
    escape: bool,
) {
    bw.write_bits(if pair { ID_CPE } else { ID_SCE }, 3);
    bw.write_bits(0, 4); // element instance tag
    bw.write_bits(0, 12); // unused, must be zero
    bw.write_bits(u64::from(partial), 1);
    bw.write_bits(0, 2); // bytes shifted: this module never uses extra_bits
    bw.write_bits(u64::from(escape), 1);
    if partial {
        bw.write_bits(num_samples as u64, 32);
    }
}

fn validate_order_quant(order: u8, quant: u32) -> Result<()> {
    if order > MAX_LPC_ORDER {
        return Err(TranscodeError::Unsupported(format!(
            "ALAC compressed encode: LPC order {order} exceeds the {MAX_LPC_ORDER} \
             this module supports (order 31's fixed-predictor sentinel is not \
             implemented)"
        )));
    }
    if !(1..=15).contains(&quant) {
        return Err(TranscodeError::Unsupported(format!(
            "ALAC compressed encode: LPC quant shift {quant} must be 1-15"
        )));
    }
    Ok(())
}

// ─── LPC prediction (encode: residuals from samples; decode: the inverse) ─────
//
// Both directions share `lpc_predict`/`adapt_coefs`: the encoder knows the
// residual immediately (it has both the sample and the prediction), and
// because this codec is lossless, the decoder's reconstructed residual is
// bit-identical — so calling the *same* adaptation function with the same
// arguments keeps both sides' coefficient state in lockstep without any
// separate "encoder version" of the adaptation math to keep in sync by hand.

/// Sliding-window LPC prediction for the sample at `history[i]`, given
/// already-known samples/reconstructions `history[..i]`. `coefs[0]` weighs
/// the *oldest* tap in the window (`history[i-order]`); `coefs[order-1]`
/// the *newest* (`history[i-1]`); `d = history[i-order-1]` is the window's
/// baseline. This ascending (oldest-to-newest) pairing is not arbitrary —
/// it must match how a real ALAC decoder (FFmpeg's `lpc_prediction`,
/// `pred[j] = buffer_out[i-order+j]`) applies the coefficient array it
/// reads off the wire, or coefficient adaptation (below) silently diverges
/// between encoder and decoder from the very first non-zero update
/// onward, one implementation's "coefs[j]" governing a different tap than
/// the other's. Verified against `ffmpeg` in
/// `test_ffmpeg_can_decode_compressed_output` — see that test's doc
/// comment for the bug this exact convention fixed.
fn lpc_predict(coefs: &[i64], history: &[i64], i: usize, d: i64, quant: u32) -> i64 {
    let order = coefs.len();
    let mut val: i64 = 0;
    for (j, &coef) in coefs.iter().enumerate() {
        let tap = history[i - order + j];
        val += (tap - d) * coef;
    }
    val = (val + (1i64 << (quant - 1))) >> quant;
    val + d
}

/// Sign-sign-LMS coefficient adaptation after coding/reconstructing the
/// sample at `history[i]` with residual `residual` (encoder: the true
/// residual; decoder: the identical reconstructed residual).
fn adapt_coefs(coefs: &mut [i64], history: &[i64], i: usize, d: i64, quant: u32, residual: i64) {
    let order = coefs.len();
    let error_sign = residual.signum();
    if error_sign == 0 {
        return;
    }
    let mut e = residual;
    for (j, coef) in coefs.iter_mut().enumerate() {
        if e.signum() != error_sign {
            break;
        }
        let tap = history[i - order + j];
        let diff = d - tap;
        let sign_val = diff.signum() * error_sign;
        *coef -= sign_val;
        let scaled = diff * sign_val;
        e -= (scaled >> quant) * (j as i64 + 1);
    }
}

/// Computes LPC residuals for one channel's samples. `order == 0` means no
/// prediction (residual = sample). For `order >= 1`, the first `order`
/// samples after the very first use a plain first-difference "warm-up"
/// (matching the format), then the sliding-window predictor above takes
/// over with coefficients starting at zero and adapting per-sample.
fn lpc_encode(samples: &[i64], order: usize, quant: u32) -> Vec<i64> {
    let n = samples.len();
    let mut residual = vec![0i64; n];
    if n == 0 {
        return residual;
    }
    residual[0] = samples[0];
    if order == 0 || n == 1 {
        for i in 1..n {
            residual[i] = samples[i];
        }
        return residual;
    }

    let warm = order.min(n - 1);
    for i in 1..=warm {
        residual[i] = samples[i] - samples[i - 1];
    }
    if order + 1 >= n {
        return residual;
    }

    let mut coefs = vec![0i64; order];
    for i in (order + 1)..n {
        let d = samples[i - order - 1];
        let predicted = lpc_predict(&coefs, samples, i, d, quant);
        let r = samples[i] - predicted;
        residual[i] = r;
        adapt_coefs(&mut coefs, samples, i, d, quant, r);
    }
    residual
}

/// Reconstructs one channel's samples from LPC residuals — the exact
/// inverse of [`lpc_encode`] given the same `order`/`quant` and the
/// transmitted initial `coefs` (this module's own encoder always
/// transmits all-zero initial coefficients, but the decoder honors
/// whatever was actually sent).
fn lpc_decode(residual: &[i64], order: usize, quant: u32, mut coefs: Vec<i64>) -> Vec<i64> {
    let n = residual.len();
    let mut samples = vec![0i64; n];
    if n == 0 {
        return samples;
    }
    samples[0] = residual[0];
    if order == 0 || n == 1 {
        for i in 1..n {
            samples[i] = residual[i];
        }
        return samples;
    }

    let warm = order.min(n - 1);
    for i in 1..=warm {
        samples[i] = samples[i - 1] + residual[i];
    }
    if order + 1 >= n {
        return samples;
    }

    for i in (order + 1)..n {
        let d = samples[i - order - 1];
        let predicted = lpc_predict(&coefs, &samples, i, d, quant);
        let r = residual[i];
        samples[i] = predicted + r;
        adapt_coefs(&mut coefs, &samples, i, d, quant, r);
    }
    samples
}

// ─── Adaptive Golomb/modified-Rice entropy coding ──────────────────────────────

/// Standard zigzag mapping (0, -1, 1, -2, 2, … → 0, 1, 2, 3, 4, …),
/// algebraically identical to ALAC's `-2x-1` / `(x>>1)^-(x&1)` formulation.
fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(x: u64) -> i64 {
    ((x >> 1) as i64) ^ -((x & 1) as i64)
}

/// `floor(log2(x))`, with `log2_floor(0) == 0` (matches FFmpeg's `av_log2`
/// convention, which the Rice parameter formulas below are stated in
/// terms of).
fn log2_floor(x: u64) -> u32 {
    if x == 0 {
        0
    } else {
        x.ilog2()
    }
}

/// Rice parameter for the next residual, from the running history.
fn rice_k_from_history(history: i64, rice_limit: u32) -> u32 {
    let base = (history >> 9) + 3;
    log2_floor(base.max(0) as u64).clamp(1, rice_limit)
}

/// Rice parameter for a zero-run block-size codeword.
fn rice_k_block(history: i64, rice_limit: u32) -> u32 {
    let h = history.max(0);
    let log2h = i64::from(log2_floor(h as u64));
    let k = 7 - log2h + ((h + 16) >> 6);
    k.clamp(1, i64::from(rice_limit)) as u32
}

/// Updates the running history estimate after coding zigzag value `x`.
fn update_history(history: i64, x: u64, mult: u32) -> i64 {
    if x > 0xFFFF {
        return 0xFFFF;
    }
    let m = i64::from(mult);
    let xi = x as i64;
    (history + xi * m - ((history * m) >> 9)).max(0)
}

/// Modified Golomb-Rice codeword: unary quotient over divisor `2^k - 1`
/// (not `2^k`), with a one-bit-shorter codeword for a zero remainder, and
/// a 9-one-bits escape (followed by a `write_sample_size`-bit literal) for
/// quotients above 8. Mirrors Apple/FFmpeg's `ag_enc`/`encode_scalar`.
fn encode_scalar(bw: &mut BitWriter, x: u64, k: u32, write_sample_size: u32, rice_limit: u32) {
    let k = k.clamp(1, rice_limit);
    let divisor = (1u64 << k) - 1;
    let q = x / divisor;
    let r = x % divisor;
    if q > 8 {
        bw.write_bits(0x1FF, 9); // nine one-bits: escape marker (no stop bit)
        bw.write_bits(x, write_sample_size);
    } else {
        if q > 0 {
            bw.write_bits((1u64 << q) - 1, q as u32);
        }
        bw.write_bits(0, 1); // unary stop bit
        if k != 1 {
            if r > 0 {
                bw.write_bits(r + 1, k);
            } else {
                bw.write_bits(0, k - 1);
            }
        }
    }
}

/// The exact inverse of [`encode_scalar`].
fn decode_scalar(
    br: &mut BitReader<'_>,
    k: u32,
    write_sample_size: u32,
    rice_limit: u32,
) -> Result<u64> {
    let k = k.clamp(1, rice_limit);
    let mut q: u32 = 0;
    while q < 9 {
        if br.read_bits(1)? == 1 {
            q += 1;
        } else {
            break;
        }
    }
    if q > 8 {
        br.read_bits(write_sample_size)
    } else if k != 1 {
        let extrabits = br.peek_bits(k)?;
        if extrabits > 1 {
            let x = (u64::from(q) << k) - u64::from(q) + extrabits - 1;
            br.skip_bits(k)?;
            Ok(x)
        } else {
            br.skip_bits(k - 1)?;
            Ok((u64::from(q) << k) - u64::from(q))
        }
    } else {
        Ok(u64::from(q))
    }
}

/// Encodes one channel's residual stream: adaptive Rice parameter from a
/// running history estimate, with a run-length shortcut for stretches of
/// exact zeros (silence codes far smaller than one Rice codeword per
/// sample). Mirrors Apple/FFmpeg's `dyn_comp`/`alac_entropy_coder`.
fn rice_encode_channel(
    bw: &mut BitWriter,
    residuals: &[i64],
    write_sample_size: u32,
    rice_limit: u32,
    history_mult: u32,
) {
    let mut history = i64::from(MB);
    let mut sign_modifier: u64 = 0;
    let n = residuals.len();
    let mut i = 0usize;
    while i < n {
        let k = rice_k_from_history(history, rice_limit);
        let x = zigzag(residuals[i]);
        i += 1;
        encode_scalar(
            bw,
            x.saturating_sub(sign_modifier),
            k,
            write_sample_size,
            rice_limit,
        );
        sign_modifier = 0;
        history = update_history(history, x, history_mult);

        if history < 128 && i < n {
            let k2 = rice_k_block(history, rice_limit);
            let mut block_size: u64 = 0;
            while i < n && residuals[i] == 0 {
                i += 1;
                block_size += 1;
            }
            encode_scalar(bw, block_size, k2, 16, rice_limit);
            sign_modifier = u64::from(block_size <= 0xFFFF);
            history = 0;
        }
    }
}

/// The exact inverse of [`rice_encode_channel`].
fn rice_decode_channel(
    br: &mut BitReader<'_>,
    n: usize,
    write_sample_size: u32,
    rice_limit: u32,
    history_mult: u32,
) -> Result<Vec<i64>> {
    let mut history = i64::from(MB);
    let mut sign_modifier: u64 = 0;
    let mut out = vec![0i64; n];
    let mut i = 0usize;
    while i < n {
        let k = rice_k_from_history(history, rice_limit);
        let mut x = decode_scalar(br, k, write_sample_size, rice_limit)?;
        x += sign_modifier;
        sign_modifier = 0;
        out[i] = unzigzag(x);
        history = update_history(history, x, history_mult);
        i += 1;

        if history < 128 && i < n {
            let k2 = rice_k_block(history, rice_limit);
            let block_size = decode_scalar(br, k2, 16, rice_limit)?;
            let block_size = block_size.min((n - i) as u64) as usize;
            i += block_size; // `out` is already zero-initialized
            sign_modifier = u64::from(block_size <= 0xFFFF);
            history = 0;
        }
    }
    Ok(out)
}

// ─── MSB-first bit reader (ALAC/FLAC bit order) ────────────────────────────────

struct BitReader<'a> {
    data: &'a [u8],
    /// Bit position from the start of `data`.
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bits_left(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    /// Reads `n` bits without advancing the position.
    fn peek_bits(&self, n: u32) -> Result<u64> {
        let n = n as usize;
        if n == 0 {
            return Ok(0);
        }
        if self.bits_left() < n {
            return Err(TranscodeError::CodecError(
                "ALAC decode: bitstream exhausted".into(),
            ));
        }
        let mut value = 0u64;
        let mut pos = self.pos;
        let mut remaining = n;
        while remaining > 0 {
            let byte = self.data[pos / 8];
            let bit_off = pos % 8;
            let avail = 8 - bit_off;
            let take = avail.min(remaining);
            let shifted = (u64::from(byte) >> (avail - take)) & ((1u64 << take) - 1);
            value = (value << take) | shifted;
            pos += take;
            remaining -= take;
        }
        Ok(value)
    }

    fn skip_bits(&mut self, n: u32) -> Result<()> {
        let n = n as usize;
        if self.bits_left() < n {
            return Err(TranscodeError::CodecError(
                "ALAC decode: bitstream exhausted".into(),
            ));
        }
        self.pos += n;
        Ok(())
    }

    fn read_bits(&mut self, n: u32) -> Result<u64> {
        let v = self.peek_bits(n)?;
        self.skip_bits(n)?;
        Ok(v)
    }

    fn read_sbits(&mut self, n: u32) -> Result<i64> {
        let v = self.read_bits(n)?;
        if n == 0 {
            return Ok(0);
        }
        let sign = 1u64 << (n - 1);
        Ok(if v & sign != 0 {
            (v as i64) - (1i64 << n)
        } else {
            v as i64
        })
    }
}

// ─── AlacStreamDecoder ──────────────────────────────────────────────────────────

/// Decodes ALAC frames written by [`AlacStreamEncoder`] — both the escape
/// and compressed element forms, dispatching per element on the header's
/// escape bit, exactly like a real ALAC decoder must.
pub struct AlacStreamDecoder {
    channels: u16,
    frame_length: u32,
}

impl AlacStreamDecoder {
    /// Creates a decoder for a stream with the given channel count and
    /// (non-partial) frame length — the same values passed to
    /// [`AlacStreamEncoder::new`].
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::InvalidInput`] for out-of-range parameters.
    pub fn new(channels: u16, frame_length: u32) -> Result<Self> {
        if !(1..=8).contains(&channels) {
            return Err(TranscodeError::InvalidInput(format!(
                "ALAC supports 1-8 channels, got {channels}"
            )));
        }
        if frame_length == 0 {
            return Err(TranscodeError::InvalidInput(
                "ALAC frame length must be non-zero".into(),
            ));
        }
        Ok(Self {
            channels,
            frame_length,
        })
    }

    /// Decodes one complete ALAC frame back to interleaved i16 PCM.
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::CodecError`] on a truncated/malformed
    /// bitstream, and [`TranscodeError::Unsupported`] for element features
    /// this decoder does not implement (nonzero `extra_bits`, an
    /// unrecognized element tag, or a reconstructed sample that overflows
    /// `i16` — which a genuine round trip of this encoder's own output
    /// never produces).
    pub fn decode_block(&self, data: &[u8]) -> Result<Vec<i16>> {
        let mut br = BitReader::new(data);
        let ch_total = usize::from(self.channels);
        let mut channel_streams: Vec<Vec<i64>> = Vec::with_capacity(ch_total);
        let mut num_samples = self.frame_length as usize;

        while channel_streams.len() < ch_total {
            let tag = br.read_bits(3)?;
            let pair = match tag {
                ID_SCE => false,
                ID_CPE => true,
                _ => {
                    return Err(TranscodeError::Unsupported(format!(
                        "ALAC decode: unsupported element tag {tag}"
                    )))
                }
            };
            br.skip_bits(4)?; // element instance tag
            br.skip_bits(12)?; // unused
            let partial = br.read_bits(1)? != 0;
            let extra_bits_code = br.read_bits(2)?;
            let escape = br.read_bits(1)? != 0;
            if extra_bits_code != 0 {
                return Err(TranscodeError::Unsupported(
                    "ALAC decode: nonzero extra_bits (bytes-shifted samples) is not \
                     supported by this decoder"
                        .into(),
                ));
            }
            if partial {
                num_samples = br.read_bits(32)? as usize;
            }

            let n_ch = if pair { 2 } else { 1 };
            if channel_streams.len() + n_ch > ch_total {
                return Err(TranscodeError::CodecError(
                    "ALAC decode: element channel count exceeds the stream's channel count".into(),
                ));
            }

            if escape {
                let mut chans: Vec<Vec<i64>> = vec![Vec::with_capacity(num_samples); n_ch];
                for _ in 0..num_samples {
                    for chan in &mut chans {
                        chan.push(br.read_sbits(AlacStreamEncoder::BPS)?);
                    }
                }
                channel_streams.extend(chans);
            } else {
                let decorr_shift = br.read_bits(8)?;
                let decorr_weight = br.read_bits(8)?;
                let write_sample_size = if pair { 17 } else { 16 };

                let mut orders = Vec::with_capacity(n_ch);
                let mut quants = Vec::with_capacity(n_ch);
                let mut rice_mods = Vec::with_capacity(n_ch);
                let mut coefs_per_channel = Vec::with_capacity(n_ch);
                for _ in 0..n_ch {
                    br.skip_bits(4)?; // prediction type (only 0 is meaningful here)
                    let quant = br.read_bits(4)? as u32;
                    let rice_mod = br.read_bits(3)? as u32;
                    let order = br.read_bits(5)? as usize;
                    let mut coefs = Vec::with_capacity(order);
                    for _ in 0..order {
                        coefs.push(br.read_sbits(16)?);
                    }
                    orders.push(order);
                    quants.push(quant);
                    rice_mods.push(rice_mod);
                    coefs_per_channel.push(coefs);
                }

                let mut mixed: Vec<Vec<i64>> = Vec::with_capacity(n_ch);
                for ci in 0..n_ch {
                    let history_mult = rice_mods[ci] * PB / 4;
                    let residual = rice_decode_channel(
                        &mut br,
                        num_samples,
                        write_sample_size,
                        KB,
                        history_mult,
                    )?;
                    mixed.push(lpc_decode(
                        &residual,
                        orders[ci],
                        quants[ci].max(1),
                        coefs_per_channel[ci].clone(),
                    ));
                }

                if pair {
                    let (mid, side) = (&mixed[0], &mixed[1]);
                    let mut left = Vec::with_capacity(num_samples);
                    let mut right = Vec::with_capacity(num_samples);
                    for k in 0..num_samples {
                        let (a, b) = (mid[k], side[k]);
                        let a = if decorr_weight == 0 {
                            a
                        } else {
                            a - ((b * decorr_weight as i64) >> decorr_shift)
                        };
                        let b = if decorr_weight == 0 { b } else { b + a };
                        left.push(b);
                        right.push(a);
                    }
                    channel_streams.push(left);
                    channel_streams.push(right);
                } else {
                    channel_streams.push(mixed.into_iter().next().unwrap_or_default());
                }
            }
        }

        let tail = br.read_bits(3)?;
        if tail != ID_END {
            return Err(TranscodeError::CodecError(format!(
                "ALAC decode: expected END tag after channels, got {tail}"
            )));
        }

        let mut out = vec![0i16; num_samples * ch_total];
        for (ci, stream) in channel_streams.iter().enumerate() {
            for (k, &v) in stream.iter().enumerate() {
                let sample = i16::try_from(v).map_err(|_| {
                    TranscodeError::CodecError(format!(
                        "ALAC decode: reconstructed sample {v} at channel {ci} \
                         index {k} overflows i16"
                    ))
                })?;
                out[k * ch_total + ci] = sample;
            }
        }
        Ok(out)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_magic_cookie_layout() {
        let enc = AlacStreamEncoder::new(48_000, 2, 4_096).expect("encoder");
        let cookie = enc.magic_cookie();
        assert_eq!(cookie.len(), 24);
        assert_eq!(&cookie[0..4], &4096u32.to_be_bytes());
        assert_eq!(cookie[5], 16, "bit depth");
        assert_eq!(cookie[9], 2, "channels");
        assert_eq!(&cookie[20..24], &48_000u32.to_be_bytes());
    }

    #[test]
    fn test_full_block_escape_size_is_deterministic() {
        // Full 4096-sample stereo block, escape coding:
        // per pair element: 3+4+12+1+2+1 = 23 bits header, then
        // 4096*2*16 bits samples; plus END(3) and padding.
        let enc = AlacStreamEncoder::new(44_100, 2, 4_096).expect("encoder");
        let block = vec![0i16; 4_096 * 2];
        let frame = enc.encode_block_escape(&block).expect("encode");
        let bits: usize = 23 + 4_096 * 2 * 16 + 3;
        assert_eq!(frame.len(), bits.div_ceil(8));
    }

    #[test]
    fn test_partial_block_has_length_field() {
        let enc = AlacStreamEncoder::new(44_100, 1, 4_096).expect("encoder");
        let frame_full = enc.encode_block_escape(&vec![0i16; 4_096]).expect("full");
        let frame_short = enc.encode_block_escape(&vec![0i16; 100]).expect("short");
        // Short block: 23 + 32 + 100*16 + 3 bits.
        let bits: usize = 23 + 32 + 100 * 16 + 3;
        assert_eq!(frame_short.len(), bits.div_ceil(8));
        assert!(frame_full.len() > frame_short.len());
    }

    #[test]
    fn test_rejects_bad_blocks() {
        let enc = AlacStreamEncoder::new(44_100, 2, 4_096).expect("encoder");
        assert!(enc.encode_block_escape(&[]).is_err());
        assert!(enc.encode_block_escape(&[1i16, 2, 3]).is_err());
        assert!(enc
            .encode_block_escape(&vec![0i16; (4_096 + 1) * 2])
            .is_err());
    }

    // ── Compressed element: round-trip vs the uncompressed form ────────────

    fn sine_pcm(freq_hz: f64, sample_rate: u32, channels: u16, frames: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(frames * usize::from(channels));
        for i in 0..frames {
            let t = i as f64 / f64::from(sample_rate);
            for c in 0..channels {
                let phase = f64::from(c) * 0.4;
                let s = (2.0 * std::f64::consts::PI * freq_hz * t + phase).sin();
                out.push((s * 12_000.0) as i16);
            }
        }
        out
    }

    fn noise_pcm(seed: u32, channels: u16, frames: usize) -> Vec<i16> {
        let mut state = seed.max(1);
        let mut out = Vec::with_capacity(frames * usize::from(channels));
        for _ in 0..frames * usize::from(channels) {
            // xorshift32 — deterministic, no external dependency.
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            out.push((state % 30_000) as i16 - 15_000);
        }
        out
    }

    #[test]
    fn test_compressed_roundtrip_multiple_orders_and_quants_mono() {
        let sr = 44_100u32;
        let frames = 1_500usize;
        let pcm = sine_pcm(440.0, sr, 1, frames);
        let enc = AlacStreamEncoder::new(sr, 1, 4_096).expect("encoder");
        let dec = AlacStreamDecoder::new(1, 4_096).expect("decoder");

        for &order in &[0u8, 1, 2, 4, 8, 16, 30] {
            for &quant in &[4u32, 9, 14] {
                let frame = enc
                    .encode_block_compressed_with_params(&pcm, order, quant)
                    .unwrap_or_else(|e| panic!("order={order} quant={quant}: encode failed: {e}"));
                let decoded = dec
                    .decode_block(&frame)
                    .unwrap_or_else(|e| panic!("order={order} quant={quant}: decode failed: {e}"));
                assert_eq!(
                    decoded, pcm,
                    "order={order} quant={quant}: compressed round-trip must be sample-exact"
                );
            }
        }
    }

    #[test]
    fn test_compressed_roundtrip_multiple_orders_and_quants_stereo() {
        let sr = 48_000u32;
        let frames = 2_000usize;
        let pcm = sine_pcm(220.0, sr, 2, frames);
        let enc = AlacStreamEncoder::new(sr, 2, 4_096).expect("encoder");
        let dec = AlacStreamDecoder::new(2, 4_096).expect("decoder");

        for &order in &[0u8, 1, 3, 8, 12] {
            for &quant in &[5u32, 9, 12] {
                let frame = enc
                    .encode_block_compressed_with_params(&pcm, order, quant)
                    .unwrap_or_else(|e| panic!("order={order} quant={quant}: encode failed: {e}"));
                let decoded = dec
                    .decode_block(&frame)
                    .unwrap_or_else(|e| panic!("order={order} quant={quant}: decode failed: {e}"));
                assert_eq!(
                    decoded, pcm,
                    "order={order} quant={quant}: stereo compressed round-trip must be sample-exact"
                );
            }
        }
    }

    #[test]
    fn test_compressed_roundtrip_noise_and_silence_and_short_block() {
        let sr = 44_100u32;
        let enc = AlacStreamEncoder::new(sr, 2, 4_096).expect("encoder");
        let dec = AlacStreamDecoder::new(2, 4_096).expect("decoder");

        // Noise: worst case for a predictor (near-zero correlation) —
        // exercises the escape-codeword path (large residuals) heavily.
        let noise = noise_pcm(0xC0FFEE, 2, 2_048);
        let frame = enc
            .encode_block_compressed_with_params(&noise, 8, 9)
            .expect("encode noise");
        assert_eq!(dec.decode_block(&frame).expect("decode noise"), noise);

        // Exact silence: exercises the zero-run block-size path heavily.
        let silence = vec![0i16; 2_048 * 2];
        let frame = enc
            .encode_block_compressed_with_params(&silence, 8, 9)
            .expect("encode silence");
        assert_eq!(dec.decode_block(&frame).expect("decode silence"), silence);
        assert!(
            frame.len() < 64,
            "silence must compress to nearly nothing, got {} bytes",
            frame.len()
        );

        // Short (partial) final block.
        let short = sine_pcm(660.0, sr, 2, 37);
        let frame = enc
            .encode_block_compressed_with_params(&short, 4, 9)
            .expect("encode short block");
        assert_eq!(dec.decode_block(&frame).expect("decode short block"), short);
    }

    #[test]
    fn test_compressed_smaller_than_escape_for_tonal_content() {
        let sr = 44_100u32;
        let pcm = sine_pcm(300.0, sr, 2, 4_096);
        let enc = AlacStreamEncoder::new(sr, 2, 4_096).expect("encoder");
        let escape = enc.encode_block_escape(&pcm).expect("escape");
        let compressed = enc
            .encode_block_compressed_with_params(&pcm, 8, 9)
            .expect("compressed");
        assert!(
            compressed.len() < escape.len(),
            "compressed ({} bytes) should beat escape ({} bytes) for a tone",
            compressed.len(),
            escape.len()
        );
    }

    #[test]
    fn test_encode_block_prefers_verified_compressed_output() {
        let sr = 44_100u32;
        let pcm = sine_pcm(300.0, sr, 2, 4_096);
        let enc = AlacStreamEncoder::new(sr, 2, 4_096).expect("encoder");
        let auto = enc.encode_block(&pcm).expect("auto encode");
        let escape = enc.encode_block_escape(&pcm).expect("escape");
        assert!(
            auto.len() < escape.len(),
            "encode_block must pick the smaller, self-verified compressed form"
        );
        let dec = AlacStreamDecoder::new(2, 4_096).expect("decoder");
        assert_eq!(dec.decode_block(&auto).expect("decode auto"), pcm);
    }

    #[test]
    fn test_encode_block_falls_back_to_escape_for_many_channels() {
        // Compressed encode is scoped to 1-2 channels; encode_block must
        // still succeed (via the escape form) for higher channel counts.
        let enc = AlacStreamEncoder::new(48_000, 6, 512).expect("encoder");
        let pcm = sine_pcm(150.0, 48_000, 6, 512);
        let out = enc.encode_block(&pcm).expect("encode 6ch");
        let escape_only = enc.encode_block_escape(&pcm).expect("escape 6ch");
        assert_eq!(out, escape_only, "6ch must use the escape path unchanged");
    }

    #[test]
    fn test_compressed_rejects_bad_order_and_quant() {
        let enc = AlacStreamEncoder::new(44_100, 1, 4_096).expect("encoder");
        let pcm = vec![0i16; 512];
        assert!(enc
            .encode_block_compressed_with_params(&pcm, 31, 9)
            .is_err());
        assert!(enc.encode_block_compressed_with_params(&pcm, 8, 0).is_err());
        assert!(enc
            .encode_block_compressed_with_params(&pcm, 8, 16)
            .is_err());
    }

    #[test]
    fn test_decoder_rejects_channel_count_mismatch_gracefully() {
        // A stereo-encoded frame handed to a mono decoder must error, not
        // panic or silently truncate.
        let enc = AlacStreamEncoder::new(44_100, 2, 512).expect("encoder");
        let pcm = sine_pcm(440.0, 44_100, 2, 512);
        let frame = enc
            .encode_block_compressed_with_params(&pcm, 4, 9)
            .expect("encode");
        let dec = AlacStreamDecoder::new(1, 512).expect("mono decoder");
        assert!(dec.decode_block(&frame).is_err());
    }

    /// Test-only helper: mux one already-encoded ALAC frame (either element
    /// form) into a CAF file exactly as `run_alac_job` in `frame_level.rs`
    /// does, run system `ffmpeg` over it, and return the decoded PCM (or
    /// `None` if `ffmpeg` is not installed, so callers can skip rather than
    /// fail). Shared by [`ffmpeg_decode_compressed_frame`] and
    /// [`ffmpeg_decode_escape_frame`] so both element forms are checked
    /// against the exact same mux/decode path.
    async fn ffmpeg_decode_alac_frame(
        sr: u32,
        channels: u16,
        frame_length: u32,
        frame: &[u8],
        magic_cookie: Vec<u8>,
        tag: &str,
    ) -> Option<Vec<i16>> {
        use oximedia_container::{Muxer, Packet, PacketFlags};
        use oximedia_core::{CodecId, Rational, Timestamp};
        use std::process::Command;

        if Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("ffmpeg not installed; skipping reference-decoder cross-check");
            return None;
        }

        let path = std::env::temp_dir().join(format!(
            "oximedia_test_alac_ffmpeg_{tag}_{}.caf",
            std::process::id()
        ));
        let mut muxer =
            crate::raw_sinks::CafAlacFileMuxer::new(path.clone(), sr, channels, magic_cookie);
        muxer.set_total_frames(u64::from(frame_length));
        let stream = oximedia_container::StreamInfo::new(0, CodecId::Alac, Rational::new(1, 1_000));
        muxer.add_stream(stream).expect("add stream");
        muxer.write_header().await.expect("write header");
        let pkt = Packet::new(
            0,
            bytes::Bytes::copy_from_slice(frame),
            Timestamp::new(0, Rational::new(1, 1_000)),
            PacketFlags::KEYFRAME,
        );
        muxer.write_packet(&pkt).await.expect("write packet");
        muxer.write_trailer().await.expect("write trailer");

        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "s16le", "-acodec", "pcm_s16le", "-"])
            .output()
            .expect("run ffmpeg");
        let _ = std::fs::remove_file(&path);

        assert!(
            output.status.success(),
            "ffmpeg failed to decode our ALAC output: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(
            output
                .stdout
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect(),
        )
    }

    /// Encodes `pcm` with the compressed element form, then decodes the
    /// result through [`ffmpeg_decode_alac_frame`].
    async fn ffmpeg_decode_compressed_frame(
        sr: u32,
        channels: u16,
        frame_length: u32,
        pcm: &[i16],
        order: u8,
        quant: u32,
        tag: &str,
    ) -> Option<Vec<i16>> {
        let enc = AlacStreamEncoder::new(sr, channels, frame_length).expect("encoder");
        let frame = enc
            .encode_block_compressed_with_params(pcm, order, quant)
            .expect("compressed encode");
        ffmpeg_decode_alac_frame(sr, channels, frame_length, &frame, enc.magic_cookie(), tag).await
    }

    /// Encodes `pcm` with the escape (uncompressed) element form, then
    /// decodes the result through [`ffmpeg_decode_alac_frame`] — the
    /// counterpart to [`ffmpeg_decode_compressed_frame`] that backs this
    /// module's "escape form verified against FFmpeg" doc claim with an
    /// actual reference-decoder run, not just the format-layout reasoning.
    async fn ffmpeg_decode_escape_frame(
        sr: u32,
        channels: u16,
        frame_length: u32,
        pcm: &[i16],
        tag: &str,
    ) -> Option<Vec<i16>> {
        let enc = AlacStreamEncoder::new(sr, channels, frame_length).expect("encoder");
        let frame = enc.encode_block_escape(pcm).expect("escape encode");
        ffmpeg_decode_alac_frame(sr, channels, frame_length, &frame, enc.magic_cookie(), tag).await
    }

    /// Cross-checks this module's compressed output against a real
    /// external decoder (system `ffmpeg`) — the reference-conformance bar
    /// the module doc references. Skips (does not fail) when `ffmpeg` is
    /// not installed, so the crate's test suite never depends on it.
    ///
    /// **Verified this session** (`ffmpeg version 7.1.1`): stereo
    /// compressed output — mid/side decorrelation, LPC order 8, quant 9 —
    /// muxed into CAF exactly as `run_alac_job` does, decoded correctly by
    /// `ffmpeg`, sample-exact. This caught and fixed a real bug: the LPC
    /// tap/coefficient-index pairing this module used initially
    /// (`coefs[0]` ↔ newest tap) was self-consistent — this module's own
    /// encoder and decoder agreed with each other and every self-test
    /// passed — but did not match how FFmpeg's decoder actually applies
    /// `lpc_coefs[j]` to `history[i-order+j]` (oldest-to-newest ascending).
    /// Since coefficients start at zero, the very first prediction was
    /// unaffected, but every adaptation step after that nudged "the same"
    /// coefficient index based on a different tap on each side, producing
    /// a slow, growing drift (bit-exact for the first ~100 samples of a
    /// tone, then off by 1-2 and climbing) — self-consistency tests can
    /// never catch this class of bug because both sides of the pair are
    /// "wrong" the same way. `lpc_predict`/`adapt_coefs` now index
    /// `history[i-order+j]`, matching FFmpeg exactly.
    #[tokio::test]
    async fn test_ffmpeg_can_decode_compressed_output() {
        let sr = 44_100u32;
        let channels = 2u16;
        let pcm = sine_pcm(300.0, sr, channels, 4_096);
        let enc = AlacStreamEncoder::new(sr, channels, 4_096).expect("encoder");
        let compressed_len = enc
            .encode_block_compressed_with_params(&pcm, 8, 9)
            .expect("compressed encode")
            .len();
        assert!(
            compressed_len < enc.encode_block_escape(&pcm).expect("escape").len(),
            "sanity: compressed must actually be compressed for this input"
        );

        let Some(decoded_pcm) =
            ffmpeg_decode_compressed_frame(sr, channels, 4_096, &pcm, 8, 9, "stereo").await
        else {
            return;
        };
        assert_eq!(
            decoded_pcm, pcm,
            "ffmpeg-decoded PCM must match the source sample-exact (ALAC is lossless)"
        );
    }

    /// Same cross-check, mono (no stereo decorrelation involved) — this is
    /// the configuration that first exposed the tap-ordering bug described
    /// on [`test_ffmpeg_can_decode_compressed_output`], so it stays as its
    /// own regression test rather than folding into the stereo one.
    #[tokio::test]
    async fn test_ffmpeg_can_decode_compressed_output_mono() {
        let sr = 44_100u32;
        let channels = 1u16;
        let pcm = sine_pcm(300.0, sr, channels, 4_096);

        let Some(decoded_pcm) =
            ffmpeg_decode_compressed_frame(sr, channels, 4_096, &pcm, 8, 9, "mono").await
        else {
            return;
        };
        assert_eq!(
            decoded_pcm, pcm,
            "ffmpeg-decoded mono PCM must match the source sample-exact"
        );
    }

    /// Cross-checks the **escape** (uncompressed) element form against
    /// `ffmpeg`, mirroring [`test_ffmpeg_can_decode_compressed_output`] —
    /// this is what the module doc's "escape form ... verified against
    /// FFmpeg" claim actually rests on. Stereo, so both a `CPE` element and
    /// the raw 16-bit sample-major/channel-interleaved layout are
    /// exercised, not just a single-channel `SCE`.
    #[tokio::test]
    async fn test_ffmpeg_can_decode_escape_output() {
        let sr = 48_000u32;
        let channels = 2u16;
        let pcm = sine_pcm(300.0, sr, channels, 4_096);

        let Some(decoded_pcm) =
            ffmpeg_decode_escape_frame(sr, channels, 4_096, &pcm, "escape_stereo").await
        else {
            return;
        };
        assert_eq!(
            decoded_pcm, pcm,
            "ffmpeg-decoded escape-form PCM must match the source sample-exact"
        );
    }
}
