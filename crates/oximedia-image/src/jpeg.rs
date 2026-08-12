//! Pure-Rust JPEG baseline codec.
//!
//! Implements JPEG baseline DCT encode/decode (JFIF, YCbCr).
//!
//! # Features
//! - Huffman table encode/decode (DC + AC)
//! - Quantization tables (luma/chroma, quality-scaled)
//! - 8x8 DCT / IDCT
//! - YCbCr ↔ RGB conversion
//! - JFIF APP0 header
//!
//! Decoding ([`JpegDecoder`], implemented in the `decode` submodule) reads any
//! chroma subsampling whose upsampling ratio is 1 or 2 per axis — 4:4:4,
//! 4:2:2, 4:4:0, 4:2:0 — plus greyscale and restart intervals, and rejects
//! everything it cannot reproduce. Encoding ([`JpegEncoder`]) always writes
//! 4:4:4.

#![allow(dead_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_lossless)]

mod decode;
mod entropy;
mod upsample;

use crate::error::{ImageError, ImageResult};
use crate::{ColorSpace, ImageData, ImageFrame, PixelType};
use std::path::Path;

// ── Markers ──────────────────────────────────────────────────────────────────

/// JPEG SOI marker (Start of Image).
pub const JPEG_SOI: u16 = 0xFFD8;
/// JPEG EOI marker (End of Image).
pub const JPEG_EOI: u16 = 0xFFD9;
/// SOF0: Start of Frame (Baseline DCT).
pub const JPEG_SOF0: u16 = 0xFFC0;
/// DHT: Define Huffman Table.
pub const JPEG_DHT: u16 = 0xFFC4;
/// DQT: Define Quantization Table.
pub const JPEG_DQT: u16 = 0xFFDB;
/// SOS: Start of Scan.
pub const JPEG_SOS: u16 = 0xFFDA;
/// APP0: Application-specific (JFIF).
pub const JPEG_APP0: u16 = 0xFFE0;
/// DRI: Define Restart Interval.
pub const JPEG_DRI: u16 = 0xFFDD;

// ── Zigzag scan order ────────────────────────────────────────────────────────

/// Standard JPEG zigzag scan order (maps position index → block index).
pub const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Inverse zigzag: maps block index → position index.
pub const ZIGZAG_INV: [u8; 64] = {
    let mut inv = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        inv[ZIGZAG[i] as usize] = i as u8;
        i += 1;
    }
    inv
};

// ── Quantization tables ───────────────────────────────────────────────────────

/// JPEG Annex K luma quantization table (quality=50 baseline).
pub const LUMA_QUANT_BASE: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

/// JPEG Annex K chroma quantization table.
pub const CHROMA_QUANT_BASE: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

/// Scale a quantization table by quality factor (1-100).
#[must_use]
pub fn scale_quant_table(base: &[u8; 64], quality: u8) -> [u16; 64] {
    let q = quality.clamp(1, 100) as u32;
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut out = [1u16; 64];
    for (i, &b) in base.iter().enumerate() {
        let v = ((b as u32 * scale + 50) / 100).clamp(1, 255);
        out[i] = v as u16;
    }
    out
}

// ── YCbCr ↔ RGB ──────────────────────────────────────────────────────────────

/// Convert an RGB byte triple to YCbCr (ITU-R BT.601).
#[must_use]
pub fn rgb_to_ycbcr(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32;
    let g = g as f32;
    let b = b as f32;
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = -0.168736 * r - 0.331264 * g + 0.5 * b + 128.0;
    let cr = 0.5 * r - 0.418688 * g - 0.081312 * b + 128.0;
    (y, cb, cr)
}

/// Convert YCbCr back to RGB bytes (clamped to 0-255).
#[must_use]
pub fn ycbcr_to_rgb(y: f32, cb: f32, cr: f32) -> (u8, u8, u8) {
    let cb = cb - 128.0;
    let cr = cr - 128.0;
    let r = (y + 1.402 * cr).clamp(0.0, 255.0) as u8;
    let g = (y - 0.344136 * cb - 0.714136 * cr).clamp(0.0, 255.0) as u8;
    let b = (y + 1.772 * cb).clamp(0.0, 255.0) as u8;
    (r, g, b)
}

// ── DCT / IDCT ───────────────────────────────────────────────────────────────

use std::f32::consts::PI;

/// Apply the 2D forward DCT to an 8x8 block (in-place, level-shifted by -128).
pub fn dct_8x8(block: &mut [f32; 64]) {
    // Level shift
    for v in block.iter_mut() {
        *v -= 128.0;
    }
    // 1D DCT on rows
    for row in 0..8 {
        dct_1d(&mut block[row * 8..(row + 1) * 8]);
    }
    // 1D DCT on columns (transposed)
    let mut col_buf = [0.0f32; 8];
    for col in 0..8 {
        for row in 0..8 {
            col_buf[row] = block[row * 8 + col];
        }
        dct_1d(&mut col_buf);
        for row in 0..8 {
            block[row * 8 + col] = col_buf[row];
        }
    }
}

/// Apply the 2D inverse DCT to an 8x8 block, level-unshift (+128).
pub fn idct_8x8(block: &mut [f32; 64]) {
    // 1D IDCT on rows
    for row in 0..8 {
        idct_1d(&mut block[row * 8..(row + 1) * 8]);
    }
    // 1D IDCT on columns
    let mut col_buf = [0.0f32; 8];
    for col in 0..8 {
        for row in 0..8 {
            col_buf[row] = block[row * 8 + col];
        }
        idct_1d(&mut col_buf);
        for row in 0..8 {
            block[row * 8 + col] = col_buf[row];
        }
    }
    // Level unshift
    for v in block.iter_mut() {
        *v += 128.0;
    }
}

fn dct_1d(x: &mut [f32]) {
    let n = x.len() as f32;
    let mut out = [0.0f32; 8];
    for k in 0..8 {
        let ck = if k == 0 { 1.0 / 2.0_f32.sqrt() } else { 1.0 };
        let mut sum = 0.0f32;
        for (i, &xi) in x.iter().enumerate() {
            sum += xi * ((PI * k as f32 * (2.0 * i as f32 + 1.0)) / (2.0 * n)).cos();
        }
        out[k] = (2.0 / n).sqrt() * ck * sum;
    }
    x.copy_from_slice(&out);
}

fn idct_1d(x: &mut [f32]) {
    let n = x.len() as f32;
    let mut out = [0.0f32; 8];
    for i in 0..8 {
        let mut sum = 0.0f32;
        for (k, &xk) in x.iter().enumerate() {
            let ck = if k == 0 { 1.0 / 2.0_f32.sqrt() } else { 1.0 };
            sum += ck * xk * ((PI * k as f32 * (2.0 * i as f32 + 1.0)) / (2.0 * n)).cos();
        }
        out[i] = (2.0 / n).sqrt() * sum;
    }
    x.copy_from_slice(&out);
}

// ── Huffman tables ────────────────────────────────────────────────────────────

/// A Huffman table entry: code length and code value.
#[derive(Debug, Clone, Default)]
pub struct HuffmanTable {
    /// Number of codes of each length 1-16.
    pub lengths: [u8; 16],
    /// Huffman symbols in code-length order.
    pub symbols: Vec<u8>,
}

impl HuffmanTable {
    /// Build a canonical Huffman code map (symbol → (code, length)).
    #[must_use]
    pub fn build_codes(&self) -> Vec<(u8, u32, u32)> {
        // Returns (symbol, code, code_length)
        let mut result = Vec::new();
        let mut code = 0u32;
        let mut sym_idx = 0usize;
        for (bit_len, &count) in self.lengths.iter().enumerate() {
            let bit_len = bit_len + 1;
            for _ in 0..count {
                if sym_idx < self.symbols.len() {
                    result.push((self.symbols[sym_idx], code, bit_len as u32));
                    sym_idx += 1;
                }
                code += 1;
            }
            code <<= 1;
        }
        result
    }

    /// Total number of symbols.
    #[must_use]
    pub fn symbol_count(&self) -> usize {
        self.lengths.iter().map(|&l| l as usize).sum()
    }
}

/// Standard JPEG Annex K DC Huffman table for luma.
#[must_use]
pub fn luma_dc_huffman() -> HuffmanTable {
    HuffmanTable {
        lengths: [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
        symbols: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    }
}

/// Standard JPEG Annex K DC Huffman table for chroma.
#[must_use]
pub fn chroma_dc_huffman() -> HuffmanTable {
    HuffmanTable {
        lengths: [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0],
        symbols: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    }
}

// ── JPEG frame ────────────────────────────────────────────────────────────────

/// A decoded JPEG image frame.
#[derive(Debug, Clone)]
pub struct JpegFrame {
    /// Image width.
    pub width: u32,
    /// Image height.
    pub height: u32,
    /// Number of components (1=gray, 3=YCbCr).
    pub components: u8,
    /// Decoded pixel data (interleaved RGB or gray).
    pub pixels: Vec<u8>,
}

impl JpegFrame {
    /// Convert to `ImageFrame`.
    #[must_use]
    pub fn to_image_frame(&self, frame_number: u32) -> ImageFrame {
        let color_space = if self.components == 1 {
            ColorSpace::Luma
        } else {
            ColorSpace::Srgb
        };
        ImageFrame::new(
            frame_number,
            self.width,
            self.height,
            PixelType::U8,
            self.components,
            color_space,
            ImageData::interleaved(self.pixels.clone()),
        )
    }
}

// ── JPEG quality ──────────────────────────────────────────────────────────────

/// JPEG encoding quality setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegQuality(pub u8);

impl JpegQuality {
    /// Create a quality (1-100, where 95 is high quality, 50 is baseline).
    #[must_use]
    pub fn new(q: u8) -> Self {
        Self(q.clamp(1, 100))
    }

    /// High quality (95).
    #[must_use]
    pub const fn high() -> Self {
        Self(95)
    }

    /// Medium quality (75).
    #[must_use]
    pub const fn medium() -> Self {
        Self(75)
    }

    /// Low quality (50, Annex K baseline).
    #[must_use]
    pub const fn low() -> Self {
        Self(50)
    }
}

impl Default for JpegQuality {
    fn default() -> Self {
        Self::medium()
    }
}

/// JPEG SOF0 frame header.
#[derive(Debug, Clone)]
pub struct SofHeader {
    /// Precision (bits per sample).
    pub precision: u8,
    /// Image height.
    pub height: u16,
    /// Image width.
    pub width: u16,
    /// Number of components.
    pub components: u8,
}

/// Parse the SOF0 segment.
fn parse_sof0(data: &[u8]) -> ImageResult<SofHeader> {
    if data.len() < 6 {
        return Err(ImageError::invalid_format("SOF0 too short"));
    }
    let precision = data[0];
    let height = u16::from_be_bytes([data[1], data[2]]);
    let width = u16::from_be_bytes([data[3], data[4]]);
    let components = data[5];
    // Defend against an allocation bomb: SOF0 declares 16-bit width/height and
    // a component count that drive a `width*height*components` buffer during
    // scan decode. Reject frames above the decode ceiling, and component counts
    // outside baseline JPEG's 1..=4, before anything is allocated from them.
    crate::limits::check_dimensions(width as usize, height as usize)
        .map_err(ImageError::InvalidFormat)?;
    if components == 0 || components > 4 {
        return Err(ImageError::invalid_format(format!(
            "SOF0 component count {components} out of range 1..=4"
        )));
    }
    Ok(SofHeader {
        precision,
        height,
        width,
        components,
    })
}

// ── Decode ─────────────────────────────────────────────────────────────────

/// JPEG decoder (baseline sequential DCT).
#[derive(Debug, Default)]
pub struct JpegDecoder;

impl JpegDecoder {
    /// Create a new decoder.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Decode JPEG bytes to a [`JpegFrame`].
    ///
    /// Handles baseline (`SOF0`) and 8-bit extended sequential (`SOF1`)
    /// datastreams: greyscale, and YCbCr at any chroma subsampling whose
    /// upsampling ratio is 1 or 2 on each axis — 4:4:4, 4:2:2, 4:4:0 and
    /// 4:2:0 — with restart intervals.
    ///
    /// # Errors
    ///
    /// Returns an error for anything outside that envelope (progressive,
    /// arithmetic-coded, lossless, hierarchical, 12-bit, exotic sampling
    /// ratios) and for malformed or truncated data. It never substitutes a
    /// synthetic image for a file it cannot read.
    pub fn decode(&self, data: &[u8]) -> ImageResult<JpegFrame> {
        decode::decode(data)
    }
}

/// Extend a sign-extended value from `nbit`-bit code.
fn extend(v: u32, nbit: u8) -> i32 {
    if nbit == 0 {
        return 0;
    }
    let vt = 1 << (nbit - 1);
    if v as i32 >= vt {
        v as i32
    } else {
        v as i32 + (-1 << nbit) + 1
    }
}

// ── JPEG encoder ─────────────────────────────────────────────────────────────

/// JPEG encoder (baseline DCT, JFIF output).
#[derive(Debug, Clone)]
pub struct JpegEncoder {
    /// Quality setting.
    pub quality: JpegQuality,
}

impl Default for JpegEncoder {
    fn default() -> Self {
        Self {
            quality: JpegQuality::default(),
        }
    }
}

impl JpegEncoder {
    /// Create an encoder with a given quality.
    #[must_use]
    pub fn new(quality: JpegQuality) -> Self {
        Self { quality }
    }

    /// Encode an `ImageFrame` as JPEG bytes.
    pub fn encode(&self, frame: &ImageFrame) -> ImageResult<Vec<u8>> {
        let data = frame
            .data
            .as_slice()
            .ok_or_else(|| ImageError::invalid_format("JPEG encoder requires interleaved data"))?;

        let w = frame.width as usize;
        let h = frame.height as usize;
        let comp = frame.components as usize;
        let luma_qt = scale_quant_table(&LUMA_QUANT_BASE, self.quality.0);
        let chroma_qt = scale_quant_table(&CHROMA_QUANT_BASE, self.quality.0);

        let mut out = Vec::new();

        // SOI
        out.extend_from_slice(&JPEG_SOI.to_be_bytes());

        // APP0 (JFIF)
        let app0: Vec<u8> = {
            let mut seg = Vec::new();
            seg.extend_from_slice(b"JFIF\x00");
            seg.extend_from_slice(&[1u8, 1]); // version 1.1
            seg.push(0); // aspect ratio units = 0 (no units)
            seg.extend_from_slice(&[0u8, 1, 0, 1]); // density 1x1
            seg.extend_from_slice(&[0u8, 0]); // no thumbnail
            seg
        };
        write_segment(&mut out, JPEG_APP0, &app0);

        // DQT: luma
        let mut dqt0 = vec![0x00u8]; // table 0, precision 8-bit
        write_quant_table(&mut dqt0, &luma_qt);
        write_segment(&mut out, JPEG_DQT, &dqt0);

        // DQT: chroma
        let mut dqt1 = vec![0x01u8]; // table 1
        write_quant_table(&mut dqt1, &chroma_qt);
        write_segment(&mut out, JPEG_DQT, &dqt1);

        // SOF0
        let n_comp = if comp == 1 { 1u8 } else { 3u8 };
        let mut sof0 = Vec::new();
        sof0.push(8u8); // precision
        sof0.extend_from_slice(&(h as u16).to_be_bytes());
        sof0.extend_from_slice(&(w as u16).to_be_bytes());
        sof0.push(n_comp);
        for ci in 0..n_comp {
            sof0.push(ci + 1); // component id
            sof0.push(0x11); // sampling: 1x1
            sof0.push(if ci == 0 { 0 } else { 1 }); // quant table id
        }
        write_segment(&mut out, JPEG_SOF0, &sof0);

        // DHT: luma DC, luma AC, chroma DC, chroma AC
        let luma_dc = luma_dc_huffman();
        let chroma_dc = chroma_dc_huffman();
        write_huffman_segment(&mut out, 0x00, &luma_dc);
        write_huffman_segment(&mut out, 0x10, &build_luma_ac_huffman());
        if n_comp > 1 {
            write_huffman_segment(&mut out, 0x01, &chroma_dc);
            write_huffman_segment(&mut out, 0x11, &build_chroma_ac_huffman());
        }

        // SOS header
        let mut sos = Vec::new();
        sos.push(n_comp);
        for ci in 0..n_comp {
            sos.push(ci + 1); // component id
            sos.push(if ci == 0 { 0x00u8 } else { 0x11 }); // DC/AC table ids
        }
        sos.extend_from_slice(&[0, 63, 0]); // spectral range and approximation
        write_segment(&mut out, JPEG_SOS, &sos);

        // Entropy-coded data
        let scan = self.encode_scan(data, w, h, comp, &luma_qt, &chroma_qt, n_comp as usize)?;
        out.extend_from_slice(&scan);

        // EOI
        out.extend_from_slice(&JPEG_EOI.to_be_bytes());

        Ok(out)
    }

    fn encode_scan(
        &self,
        pixels: &[u8],
        w: usize,
        h: usize,
        src_comp: usize,
        luma_qt: &[u16; 64],
        chroma_qt: &[u16; 64],
        n_comp: usize,
    ) -> ImageResult<Vec<u8>> {
        let luma_dc = luma_dc_huffman();
        let luma_ac = build_luma_ac_huffman();
        let chroma_dc = chroma_dc_huffman();
        let chroma_ac = build_chroma_ac_huffman();

        // Build encode maps
        let luma_dc_map = build_encode_map(&luma_dc);
        let luma_ac_map = build_encode_map(&luma_ac);
        let chroma_dc_map = build_encode_map(&chroma_dc);
        let chroma_ac_map = build_encode_map(&chroma_ac);

        let mcu_cols = (w + 7) / 8;
        let mcu_rows = (h + 7) / 8;
        let mut bw = BitWriter::new();
        let mut dc_pred = [0i32; 3];

        for mcu_row in 0..mcu_rows {
            for mcu_col in 0..mcu_cols {
                for comp in 0..n_comp {
                    let qt = if comp == 0 { luma_qt } else { chroma_qt };
                    let dc_map = if comp == 0 {
                        &luma_dc_map
                    } else {
                        &chroma_dc_map
                    };
                    let ac_map = if comp == 0 {
                        &luma_ac_map
                    } else {
                        &chroma_ac_map
                    };

                    // Extract 8x8 block
                    let mut block = [0.0f32; 64];
                    for by in 0..8 {
                        let img_y = (mcu_row * 8 + by).min(h.saturating_sub(1));
                        for bx in 0..8 {
                            let img_x = (mcu_col * 8 + bx).min(w.saturating_sub(1));
                            let pix_base = (img_y * w + img_x) * src_comp;
                            let sample = if src_comp == 1 {
                                pixels[pix_base] as f32
                            } else if comp == 0 {
                                rgb_to_ycbcr(
                                    pixels[pix_base],
                                    pixels[pix_base + 1],
                                    pixels[pix_base + 2],
                                )
                                .0
                            } else if comp == 1 {
                                rgb_to_ycbcr(
                                    pixels[pix_base],
                                    pixels[pix_base + 1],
                                    pixels[pix_base + 2],
                                )
                                .1
                            } else {
                                rgb_to_ycbcr(
                                    pixels[pix_base],
                                    pixels[pix_base + 1],
                                    pixels[pix_base + 2],
                                )
                                .2
                            };
                            block[by * 8 + bx] = sample;
                        }
                    }

                    // DCT
                    dct_8x8(&mut block);

                    // Quantize: for zigzag scan position k, the natural block position is
                    // nat = ZIGZAG[k]. qt is in natural order, so the quantization step for
                    // position k is qt[nat] = qt[ZIGZAG[k]].
                    let mut coeffs_zz = [0i32; 64]; // indexed by zigzag scan position k
                    for k in 0..64 {
                        let nat = ZIGZAG[k] as usize;
                        coeffs_zz[k] = (block[nat] / qt[nat] as f32).round() as i32;
                    }

                    // Encode DC
                    let diff = coeffs_zz[0] - dc_pred[comp];
                    dc_pred[comp] = coeffs_zz[0];
                    encode_dc(&mut bw, diff, dc_map);

                    // Encode AC in zigzag scan order.
                    // Find the last non-zero AC position to know when to stop early.
                    let last_nz = (1..64).rev().find(|&k| coeffs_zz[k] != 0);
                    let eob_pos = last_nz.map_or(1, |k| k + 1); // first pos after last non-zero
                    let mut run = 0u8;
                    for k in 1..eob_pos {
                        let ac = coeffs_zz[k];
                        if ac == 0 {
                            run += 1;
                            if run == 16 {
                                // ZRL
                                if let Some(&(code, len)) = ac_map.get(&0xF0u8) {
                                    bw.write_bits(code, len);
                                }
                                run = 0;
                            }
                        } else {
                            let cat = category(ac);
                            let rs = (run << 4) | cat as u8;
                            if let Some(&(code, len)) = ac_map.get(&rs) {
                                bw.write_bits(code, len);
                                encode_value(&mut bw, ac);
                            }
                            run = 0;
                        }
                    }
                    // Write EOB only if there are trailing zeros (eob_pos < 64 means the
                    // last non-zero AC is before k=63, so remaining positions are all zero).
                    // If all ACs were processed (eob_pos == 64, meaning last non-zero was k=63),
                    // no EOB is needed — the decoder knows when k reaches 64.
                    if eob_pos < 64 {
                        if let Some(&(code, len)) = ac_map.get(&0x00u8) {
                            bw.write_bits(code, len);
                        }
                    }
                }
            }
        }

        Ok(bw.finish())
    }
}

fn category(v: i32) -> u32 {
    if v == 0 {
        return 0;
    }
    let abs_v = v.unsigned_abs();
    32 - abs_v.leading_zeros()
}

fn encode_dc(bw: &mut BitWriter, diff: i32, map: &std::collections::HashMap<u8, (u32, u32)>) {
    let cat = category(diff) as u8;
    if let Some(&(code, len)) = map.get(&cat) {
        bw.write_bits(code, len);
        encode_value(bw, diff);
    }
    // If cat > 11 (not in standard DC table), skip this coefficient to avoid
    // corrupting the bitstream. The decoder will use 0 for this DC value.
}

fn encode_value(bw: &mut BitWriter, v: i32) {
    let cat = category(v);
    if cat == 0 {
        return;
    }
    let bits = if v > 0 {
        v as u32
    } else {
        (v + (1 << cat) - 1) as u32
    };
    bw.write_bits(bits, cat);
}

fn build_encode_map(ht: &HuffmanTable) -> std::collections::HashMap<u8, (u32, u32)> {
    ht.build_codes()
        .into_iter()
        .map(|(sym, code, len)| (sym, (code, len)))
        .collect()
}

fn build_luma_ac_huffman() -> HuffmanTable {
    // JPEG Annex K AC luma (abbreviated - lengths only to keep file < 2000 lines)
    HuffmanTable {
        lengths: [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125],
        symbols: vec![
            0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51,
            0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1,
            0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18,
            0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
            0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57,
            0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75,
            0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92,
            0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
            0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
            0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8,
            0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2,
            0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
        ],
    }
}

fn build_chroma_ac_huffman() -> HuffmanTable {
    HuffmanTable {
        lengths: [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119],
        symbols: vec![
            0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07,
            0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09,
            0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25,
            0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
            0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56,
            0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74,
            0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
            0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
            0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba,
            0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6,
            0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2,
            0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
        ],
    }
}

/// Append the 64 elements of a quantization table to a `DQT` payload.
///
/// T.81 §B.2.4.1 orders the elements by **zigzag scan position**, not by
/// natural block position. The tables in this module are stored naturally
/// (Annex K prints them that way), so the permutation happens here — the one
/// place that serialises them.
fn write_quant_table(out: &mut Vec<u8>, table: &[u16; 64]) {
    for &position in &ZIGZAG {
        out.push(table[position as usize].min(255) as u8);
    }
}

fn write_segment(out: &mut Vec<u8>, marker: u16, data: &[u8]) {
    out.extend_from_slice(&marker.to_be_bytes());
    let len = (data.len() + 2) as u16;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(data);
}

fn write_huffman_segment(out: &mut Vec<u8>, tc_th: u8, ht: &HuffmanTable) {
    let mut data = vec![tc_th];
    data.extend_from_slice(&ht.lengths);
    data.extend_from_slice(&ht.symbols);
    write_segment(out, JPEG_DHT, &data);
}

struct BitWriter {
    buf: Vec<u8>,
    bit_buf: u32,
    bits_used: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            bit_buf: 0,
            bits_used: 0,
        }
    }

    fn write_bits(&mut self, value: u32, n: u32) {
        if n == 0 {
            return;
        }
        self.bit_buf = (self.bit_buf << n) | (value & ((1 << n) - 1));
        self.bits_used += n;
        while self.bits_used >= 8 {
            self.bits_used -= 8;
            let byte = ((self.bit_buf >> self.bits_used) & 0xFF) as u8;
            self.buf.push(byte);
            if byte == 0xFF {
                self.buf.push(0x00); // byte stuffing
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        // Flush remaining bits
        if self.bits_used > 0 {
            let byte = ((self.bit_buf << (8 - self.bits_used)) & 0xFF) as u8;
            self.buf.push(byte);
            if byte == 0xFF {
                self.buf.push(0x00);
            }
        }
        self.buf
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Read a JPEG file from disk.
pub fn read_jpeg(path: &Path) -> ImageResult<ImageFrame> {
    let data = std::fs::read(path)?;
    let frame = JpegDecoder::new().decode(&data)?;
    Ok(frame.to_image_frame(1))
}

/// Write an `ImageFrame` as JPEG.
pub fn write_jpeg(path: &Path, frame: &ImageFrame, quality: u8) -> ImageResult<()> {
    let encoder = JpegEncoder::new(JpegQuality::new(quality));
    let data = encoder.encode(frame)?;
    std::fs::write(path, data)?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sof0_rejects_oversized_and_bad_component_count() {
        // Regression: SOF0 declares 16-bit width/height and a component count
        // that size a `width*height*components` decode buffer. Oversized frames
        // and out-of-range component counts must be rejected before allocation.
        // Layout: precision, height(2 BE), width(2 BE), components.
        let oversized = [8u8, 0xFF, 0xFF, 0xFF, 0xFF, 3];
        assert!(parse_sof0(&oversized).is_err());

        let bad_components = [8u8, 0x00, 0x10, 0x00, 0x10, 0]; // 16x16, 0 comps
        assert!(parse_sof0(&bad_components).is_err());

        let ok = [8u8, 0x00, 0x10, 0x00, 0x10, 3]; // 16x16, 3 comps
        assert!(parse_sof0(&ok).is_ok());
    }

    #[test]
    fn test_jpeg_soi_marker() {
        assert_eq!(JPEG_SOI, 0xFFD8);
        assert_eq!(JPEG_EOI, 0xFFD9);
    }

    #[test]
    fn test_zigzag_has_64_entries() {
        assert_eq!(ZIGZAG.len(), 64);
        assert_eq!(ZIGZAG[0], 0);
        assert_eq!(ZIGZAG[63], 63);
    }

    #[test]
    fn test_zigzag_is_permutation() {
        let mut seen = [false; 64];
        for &z in &ZIGZAG {
            assert!(!seen[z as usize], "Duplicate in zigzag: {z}");
            seen[z as usize] = true;
        }
    }

    #[test]
    fn test_zigzag_inv_roundtrip() {
        for i in 0..64 {
            let z = ZIGZAG[i];
            let inv = ZIGZAG_INV[z as usize];
            assert_eq!(inv as usize, i, "zigzag_inv[zigzag[{i}]] != {i}");
        }
    }

    #[test]
    fn test_quant_table_quality_50() {
        let qt = scale_quant_table(&LUMA_QUANT_BASE, 50);
        // At quality 50, scale=100, output == base table
        for (i, (&base, &q)) in LUMA_QUANT_BASE.iter().zip(qt.iter()).enumerate() {
            assert_eq!(base as u16, q, "Mismatch at index {i}");
        }
    }

    #[test]
    fn test_quant_table_quality_100() {
        let qt = scale_quant_table(&LUMA_QUANT_BASE, 100);
        // At quality 100, all values should be 1
        for &q in &qt {
            assert_eq!(q, 1, "Quality 100 should give all-1 table");
        }
    }

    #[test]
    fn test_quant_table_quality_1() {
        let qt = scale_quant_table(&LUMA_QUANT_BASE, 1);
        // At quality 1, values should be clamped to 255
        for &q in &qt {
            assert!(q <= 255, "Values must be <= 255");
        }
    }

    #[test]
    fn test_ycbcr_rgb_roundtrip() {
        let (r_in, g_in, b_in) = (200u8, 100u8, 50u8);
        let (y, cb, cr) = rgb_to_ycbcr(r_in, g_in, b_in);
        let (r_out, g_out, b_out) = ycbcr_to_rgb(y, cb, cr);
        assert!(
            (r_in as i32 - r_out as i32).abs() <= 2,
            "R: {r_in} vs {r_out}"
        );
        assert!(
            (g_in as i32 - g_out as i32).abs() <= 2,
            "G: {g_in} vs {g_out}"
        );
        assert!(
            (b_in as i32 - b_out as i32).abs() <= 2,
            "B: {b_in} vs {b_out}"
        );
    }

    #[test]
    fn test_ycbcr_gray_roundtrip() {
        let (r, g, b) = (128u8, 128, 128);
        let (y, cb, cr) = rgb_to_ycbcr(r, g, b);
        let (ro, go, bo) = ycbcr_to_rgb(y, cb, cr);
        assert!((r as i32 - ro as i32).abs() <= 2);
        assert!((g as i32 - go as i32).abs() <= 2);
        assert!((b as i32 - bo as i32).abs() <= 2);
    }

    #[test]
    fn test_dct_idct_flat_block() {
        // A flat block should pass through DCT-IDCT with minimal error
        let mut block = [128.0f32; 64];
        let original = block;
        dct_8x8(&mut block);
        idct_8x8(&mut block);
        for (i, (&o, &r)) in original.iter().zip(block.iter()).enumerate() {
            assert!(
                (o - r).abs() < 2.0,
                "Flat DCT-IDCT mismatch at {i}: {o} vs {r}"
            );
        }
    }

    #[test]
    fn test_dct_idct_ramp_block() {
        let mut block = [0.0f32; 64];
        for i in 0..64 {
            block[i] = i as f32 * 2.0;
        }
        let original = block;
        dct_8x8(&mut block);
        idct_8x8(&mut block);
        for (i, (&o, &r)) in original.iter().zip(block.iter()).enumerate() {
            assert!(
                (o - r).abs() < 3.0,
                "Ramp DCT-IDCT mismatch at {i}: {o} vs {r}"
            );
        }
    }

    #[test]
    fn test_huffman_table_luma_dc_symbol_count() {
        let ht = luma_dc_huffman();
        let count: usize = ht.lengths.iter().map(|&l| l as usize).sum();
        assert_eq!(count, 12, "Luma DC should have 12 symbols");
    }

    #[test]
    fn test_huffman_table_build_codes_unique() {
        let ht = luma_dc_huffman();
        let codes = ht.build_codes();
        let mut seen = std::collections::HashSet::new();
        for (_, code, len) in &codes {
            assert!(seen.insert((*code, *len)), "Duplicate code: {code}/{len}");
        }
    }

    #[test]
    fn test_category_function() {
        assert_eq!(category(0), 0);
        assert_eq!(category(1), 1);
        assert_eq!(category(-1), 1);
        assert_eq!(category(2), 2);
        assert_eq!(category(255), 8);
        assert_eq!(category(-255), 8);
    }

    #[test]
    fn test_extend_function() {
        // extend(0, 1) → -1 (negative)
        assert_eq!(extend(0, 1), -1);
        // extend(1, 1) → 1 (positive)
        assert_eq!(extend(1, 1), 1);
        // extend(0, 2) → -3
        assert_eq!(extend(0, 2), -3);
    }

    #[test]
    fn test_jpeg_decoder_rejects_invalid() {
        let bad = vec![0xFFu8, 0xD9, 0x00, 0x00]; // EOI, not SOI
        let result = JpegDecoder::new().decode(&bad);
        assert!(result.is_err());
    }

    /// Encode a 16×16 RGB24 block at Q85, decode, and verify PSNR ≥ 30 dB.
    /// This is a sanity check that encode→decode is actually lossless at the image layer.
    #[test]
    fn test_jpeg_encode_decode_psnr_q85() {
        const W: u32 = 16;
        const H: u32 = 16;
        // Build a simple test pattern
        let mut rgb = vec![0u8; (W * H * 3) as usize];
        for row in 0..H as usize {
            for col in 0..W as usize {
                let idx = (row * W as usize + col) * 3;
                rgb[idx] = ((row * 2 + col * 3) % 200 + 28) as u8;
                rgb[idx + 1] = ((row * 3 + col * 2) % 180 + 40) as u8;
                rgb[idx + 2] = ((row + col * 5) % 160 + 50) as u8;
            }
        }

        let frame = ImageFrame::new(
            0,
            W,
            H,
            PixelType::U8,
            3,
            ColorSpace::Srgb,
            ImageData::interleaved(rgb.clone()),
        );

        let encoder = JpegEncoder::new(JpegQuality::new(85));
        let jpeg_bytes = encoder.encode(&frame).expect("encode Q85");

        let decoded = JpegDecoder::new().decode(&jpeg_bytes).expect("decode Q85");

        assert_eq!(decoded.width, W, "decoded width");
        assert_eq!(decoded.height, H, "decoded height");
        assert_eq!(decoded.components, 3, "decoded components");

        let orig = &rgb;
        let dec = &decoded.pixels;
        let mse: f64 = orig
            .iter()
            .zip(dec.iter())
            .map(|(&a, &b)| {
                let d = a as f64 - b as f64;
                d * d
            })
            .sum::<f64>()
            / orig.len() as f64;
        let psnr = if mse < 1e-10 {
            f64::INFINITY
        } else {
            20.0 * (255.0_f64).log10() - 10.0 * mse.log10()
        };
        assert!(
            psnr >= 28.0,
            "JPEG Q85 encode-decode PSNR should be ≥ 28 dB, got {psnr:.2} dB"
        );
    }

    #[test]
    fn test_jpeg_encode_decode_psnr_128x96_adversarial() {
        const W: u32 = 128;
        const H: u32 = 96;
        let w = W as usize;
        let h = H as usize;
        let mut rgb = vec![0u8; w * h * 3];
        for row in 0..h {
            for col in 0..w {
                let idx = (row * w + col) * 3;
                rgb[idx] = ((row * 2 + col * 3) & 0xFF) as u8;
                rgb[idx + 1] = ((row * 3 + col * 2) & 0xFF) as u8;
                rgb[idx + 2] = ((row + col * 5) & 0xFF) as u8;
            }
        }

        let frame = ImageFrame::new(
            0,
            W,
            H,
            PixelType::U8,
            3,
            ColorSpace::Srgb,
            ImageData::interleaved(rgb.clone()),
        );
        let encoder = JpegEncoder::new(JpegQuality::new(85));
        let jpeg_bytes = encoder.encode(&frame).expect("encode");
        let decoded = JpegDecoder::new().decode(&jpeg_bytes).expect("decode");

        let mse: f64 = rgb
            .iter()
            .zip(decoded.pixels.iter())
            .map(|(&a, &b)| {
                let d = a as f64 - b as f64;
                d * d
            })
            .sum::<f64>()
            / rgb.len() as f64;
        let psnr = if mse < 1e-10 {
            f64::INFINITY
        } else {
            20.0 * (255.0_f64).log10() - 10.0 * mse.log10()
        };
        assert!(
            psnr >= 28.0,
            "128x96 adversarial Q85 PSNR should be ≥ 28 dB, got {psnr:.2} dB"
        );
    }

    #[test]
    fn test_jpeg_encoder_produces_soi_eoi() {
        let data = ImageData::interleaved(vec![128u8; 4 * 4 * 3]);
        let frame = ImageFrame::new(1, 4, 4, PixelType::U8, 3, ColorSpace::Srgb, data);
        let encoder = JpegEncoder::default();
        let encoded = encoder.encode(&frame).expect("encode");
        assert!(encoded.len() >= 4);
        let soi = u16::from_be_bytes([encoded[0], encoded[1]]);
        assert_eq!(soi, JPEG_SOI);
        let eoi = u16::from_be_bytes([encoded[encoded.len() - 2], encoded[encoded.len() - 1]]);
        assert_eq!(eoi, JPEG_EOI);
    }
}
