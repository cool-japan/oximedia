//! Baseline DCT JPEG decoder (ITU-T T.81 process 1, SOF0) for lossy DNG.
//!
//! DNG 1.4 introduced *lossy* compression (TIFF compression `34892`): the raw
//! sensor data is gamma-encoded to 8-bit and then stored as an ordinary
//! baseline JPEG. The opcode list and `DefaultScale`/`BaselineExposure` tags
//! reverse the gamma; this module's job is purely to reproduce the 8-bit
//! sample planes from the JPEG datastream.
//!
//! This is a self-contained, dependency-free baseline decoder. It supports:
//!
//! * Huffman entropy coding (DC + AC), the only entropy mode of baseline JPEG.
//! * Arbitrary component sampling factors (`Hi × Vi`) and the interleaved MCU
//!   layout of T.81 §A.2.3.
//! * Restart (`RSTn`) markers with DC-predictor reset (T.81 §F.2.1.3.1).
//! * 8-bit precision (the only precision baseline JPEG permits).
//!
//! The output keeps each component as its own plane at the component's stored
//! resolution; chroma upsampling, if any, is the caller's responsibility — DNG
//! lossy tiles are written as single-component (mosaic) or `YCbCr` 4:4:4, and
//! [`decode_baseline_jpeg`] returns enough geometry to handle either.

use crate::error::{ImageError, ImageResult};

/// Standard JPEG zigzag order: maps coefficient scan index → natural index.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

// JPEG markers.
const MARKER_SOI: u8 = 0xD8;
const MARKER_EOI: u8 = 0xD9;
const MARKER_SOF0: u8 = 0xC0;
const MARKER_SOF1: u8 = 0xC1; // extended sequential — same Huffman decode path
const MARKER_DHT: u8 = 0xC4;
const MARKER_DQT: u8 = 0xDB;
const MARKER_DRI: u8 = 0xDD;
const MARKER_SOS: u8 = 0xDA;

/// A Huffman table with a flat fast-lookup over its canonical codes.
#[derive(Clone, Default)]
struct HuffTable {
    max_code: [i32; 17],
    min_code: [i32; 17],
    val_ptr: [usize; 17],
    values: Vec<u8>,
}

impl HuffTable {
    fn build(counts: &[u8; 16], values: Vec<u8>) -> Self {
        let mut table = HuffTable {
            values,
            ..Default::default()
        };
        let mut code: i32 = 0;
        let mut value_index = 0usize;
        for length in 1..=16usize {
            let count = counts[length - 1] as usize;
            if count == 0 {
                table.max_code[length] = -1;
            } else {
                table.val_ptr[length] = value_index;
                table.min_code[length] = code;
                code += count as i32;
                table.max_code[length] = code - 1;
                value_index += count;
            }
            code <<= 1;
        }
        table
    }

    fn decode_symbol(&self, reader: &mut BitReader<'_>) -> ImageResult<u8> {
        let mut code: i32 = 0;
        for length in 1..=16usize {
            code = (code << 1) | i32::from(reader.read_bit()?);
            let max = self.max_code[length];
            if max >= 0 && code <= max {
                let index = self.val_ptr[length] + (code - self.min_code[length]) as usize;
                return self
                    .values
                    .get(index)
                    .copied()
                    .ok_or_else(|| ImageError::compression("Baseline JPEG: bad Huffman symbol"));
            }
        }
        Err(ImageError::compression(
            "Baseline JPEG: Huffman code not found within 16 bits",
        ))
    }
}

/// MSB-first entropy bit reader with byte de-stuffing.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buffer: u32,
    bits_in_buffer: u32,
    hit_marker: Option<u8>,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            bit_buffer: 0,
            bits_in_buffer: 0,
            hit_marker: None,
        }
    }

    fn next_byte(&mut self) -> Option<u8> {
        if self.pos >= self.data.len() {
            return None;
        }
        let byte = self.data[self.pos];
        if byte != 0xFF {
            self.pos += 1;
            return Some(byte);
        }
        if self.pos + 1 >= self.data.len() {
            self.pos += 1;
            return None;
        }
        let marker = self.data[self.pos + 1];
        if marker == 0x00 {
            self.pos += 2;
            Some(0xFF)
        } else if marker == 0xFF {
            self.pos += 1;
            self.next_byte()
        } else {
            self.hit_marker = Some(marker);
            None
        }
    }

    fn read_bit(&mut self) -> ImageResult<u8> {
        if self.bits_in_buffer == 0 {
            // Past the end of entropy data, baseline JPEG pads with 1-bits.
            let byte = self.next_byte().unwrap_or(0xFF);
            self.bit_buffer = u32::from(byte);
            self.bits_in_buffer = 8;
        }
        self.bits_in_buffer -= 1;
        Ok(((self.bit_buffer >> self.bits_in_buffer) & 1) as u8)
    }

    fn read_bits(&mut self, n: u8) -> ImageResult<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Ok(value)
    }

    /// Reset for the next restart interval and consume the `RSTn` marker.
    fn restart(&mut self) -> ImageResult<()> {
        self.bit_buffer = 0;
        self.bits_in_buffer = 0;
        let marker = self.hit_marker.take();
        match marker {
            Some(m) if (0xD0..=0xD7).contains(&m) => {
                self.pos += 2;
                Ok(())
            }
            _ => {
                // Search forward for the RSTn marker.
                while self.pos + 1 < self.data.len() {
                    if self.data[self.pos] == 0xFF {
                        let m = self.data[self.pos + 1];
                        if (0xD0..=0xD7).contains(&m) {
                            self.pos += 2;
                            return Ok(());
                        }
                        if m != 0x00 && m != 0xFF {
                            return Err(ImageError::compression(
                                "Baseline JPEG: expected restart marker",
                            ));
                        }
                    }
                    self.pos += 1;
                }
                Err(ImageError::compression(
                    "Baseline JPEG: restart marker not found",
                ))
            }
        }
    }
}

/// Sign-extend a magnitude-category value to a signed coefficient (T.81 F.12).
fn extend(value: u32, size: u8) -> i32 {
    if size == 0 {
        return 0;
    }
    let v = value as i32;
    if v < (1 << (size - 1)) {
        v - (1 << size) + 1
    } else {
        v
    }
}

/// 8×8 inverse DCT (separable, floating point, AAN-equivalent accuracy).
///
/// `block` holds dequantized coefficients in natural (row-major) order on
/// entry and the level-shifted spatial samples (`+128`) on exit.
fn idct_8x8(block: &mut [f32; 64]) {
    use std::f32::consts::PI;

    // Precomputed cosine basis: `COS[u][x] = cos((2x+1)·u·π/16) · C(u)`.
    // C(0) = 1/√2, C(u>0) = 1.
    let mut cos_table = [[0.0f32; 8]; 8];
    for (u, row) in cos_table.iter_mut().enumerate() {
        let cu = if u == 0 { 1.0 / 2.0f32.sqrt() } else { 1.0 };
        for (x, slot) in row.iter_mut().enumerate() {
            *slot = cu * (((2 * x + 1) as f32) * (u as f32) * PI / 16.0).cos();
        }
    }

    // Row pass.
    let mut tmp = [0.0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0.0f32;
            for u in 0..8 {
                sum += cos_table[u][x] * block[y * 8 + u];
            }
            tmp[y * 8 + x] = sum * 0.5;
        }
    }
    // Column pass.
    for x in 0..8 {
        for y in 0..8 {
            let mut sum = 0.0f32;
            for v in 0..8 {
                sum += cos_table[v][y] * tmp[v * 8 + x];
            }
            block[y * 8 + x] = sum * 0.5 + 128.0;
        }
    }
}

/// A component as declared in the SOF0 header.
#[derive(Clone, Copy)]
struct Component {
    id: u8,
    h: u8,
    v: u8,
    quant_id: usize,
}

/// A component as referenced by the SOS scan header.
#[derive(Clone, Copy)]
struct ScanComponent {
    frame_index: usize,
    dc_table: usize,
    ac_table: usize,
}

/// One decoded component plane.
pub struct ComponentPlane {
    /// Plane width in samples (`ceil(image_w · Hi / Hmax)` rounded to MCU).
    pub width: u32,
    /// Plane height in samples.
    pub height: u32,
    /// Horizontal sampling factor.
    pub h: u8,
    /// Vertical sampling factor.
    pub v: u8,
    /// Row-major 8-bit samples, `width · height` long.
    pub samples: Vec<u8>,
}

/// A fully decoded baseline JPEG image.
pub struct BaselineJpegImage {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Decoded component planes (1 = grey/mosaic, 3 = YCbCr).
    pub planes: Vec<ComponentPlane>,
}

/// Decode a baseline (SOF0) JPEG datastream into component planes.
///
/// # Errors
///
/// Returns an error for non-baseline JPEGs, malformed segments, or corrupt
/// entropy data.
pub fn decode_baseline_jpeg(data: &[u8]) -> ImageResult<BaselineJpegImage> {
    if data.len() < 2 || data[0] != 0xFF || data[1] != MARKER_SOI {
        return Err(ImageError::compression("Baseline JPEG: missing SOI"));
    }

    let mut pos = 2usize;
    let mut quant: [Option<[u16; 64]>; 4] = [None, None, None, None];
    let mut dc_tables: [Option<HuffTable>; 4] = Default::default();
    let mut ac_tables: [Option<HuffTable>; 4] = Default::default();
    let mut frame: Option<(u16, u16, Vec<Component>)> = None;
    let mut restart_interval: u32 = 0;

    loop {
        // Find next marker.
        while pos + 1 < data.len() && !(data[pos] == 0xFF && data[pos + 1] != 0x00) {
            pos += 1;
        }
        if pos + 1 >= data.len() {
            return Err(ImageError::compression("Baseline JPEG: no SOS marker"));
        }
        let marker = data[pos + 1];
        pos += 2;
        match marker {
            0xFF => {} // fill byte, keep scanning
            MARKER_DQT => {
                let end = read_segment_end(data, pos)?;
                pos += 2;
                while pos < end {
                    let pq_tq = read_u8(data, pos)?;
                    pos += 1;
                    let precision = (pq_tq >> 4) & 0x0F;
                    let table_id = (pq_tq & 0x0F) as usize;
                    if table_id >= 4 {
                        return Err(ImageError::compression(
                            "Baseline JPEG: quant table id out of range",
                        ));
                    }
                    let mut table = [0u16; 64];
                    for slot in &mut table {
                        if precision == 0 {
                            *slot = u16::from(read_u8(data, pos)?);
                            pos += 1;
                        } else {
                            *slot = read_u16(data, pos)?;
                            pos += 2;
                        }
                    }
                    quant[table_id] = Some(table);
                }
                pos = end;
            }
            MARKER_DHT => {
                let end = read_segment_end(data, pos)?;
                pos += 2;
                while pos < end {
                    let tc_th = read_u8(data, pos)?;
                    pos += 1;
                    let class = (tc_th >> 4) & 0x0F;
                    let table_id = (tc_th & 0x0F) as usize;
                    if table_id >= 4 {
                        return Err(ImageError::compression(
                            "Baseline JPEG: Huffman table id out of range",
                        ));
                    }
                    let mut counts = [0u8; 16];
                    let mut total = 0usize;
                    for slot in &mut counts {
                        *slot = read_u8(data, pos)?;
                        pos += 1;
                        total += *slot as usize;
                    }
                    let mut values = Vec::with_capacity(total);
                    for _ in 0..total {
                        values.push(read_u8(data, pos)?);
                        pos += 1;
                    }
                    let table = HuffTable::build(&counts, values);
                    if class == 0 {
                        dc_tables[table_id] = Some(table);
                    } else {
                        ac_tables[table_id] = Some(table);
                    }
                }
                pos = end;
            }
            MARKER_DRI => {
                let end = read_segment_end(data, pos)?;
                pos += 2;
                restart_interval = u32::from(read_u16(data, pos)?);
                pos = end;
            }
            MARKER_SOF0 | MARKER_SOF1 => {
                let end = read_segment_end(data, pos)?;
                pos += 2;
                let precision = read_u8(data, pos)?;
                pos += 1;
                if precision != 8 {
                    return Err(ImageError::unsupported(format!(
                        "Lossy DNG: baseline JPEG precision {precision} (only 8-bit supported)"
                    )));
                }
                let height = read_u16(data, pos)?;
                pos += 2;
                let width = read_u16(data, pos)?;
                pos += 2;
                let count = read_u8(data, pos)? as usize;
                pos += 1;
                if !(1..=4).contains(&count) {
                    return Err(ImageError::unsupported(
                        "Lossy DNG: unsupported JPEG component count",
                    ));
                }
                let mut components = Vec::with_capacity(count);
                for _ in 0..count {
                    let id = read_u8(data, pos)?;
                    let sampling = read_u8(data, pos + 1)?;
                    let quant_id = read_u8(data, pos + 2)? as usize;
                    pos += 3;
                    components.push(Component {
                        id,
                        h: (sampling >> 4) & 0x0F,
                        v: sampling & 0x0F,
                        quant_id: quant_id.min(3),
                    });
                }
                frame = Some((width, height, components));
                pos = end;
            }
            // Progressive / arithmetic / lossless / hierarchical are not baseline.
            0xC2 | 0xC3 | 0xC5..=0xCB | 0xCD..=0xCF => {
                return Err(ImageError::unsupported(
                    "Lossy DNG: JPEG is not baseline (SOF0); progressive/lossless unsupported",
                ));
            }
            MARKER_SOS => {
                let (width, height, components) = frame
                    .ok_or_else(|| ImageError::compression("Baseline JPEG: SOS before SOF0"))?;
                let end = read_segment_end(data, pos)?;
                pos += 2;
                let scan_count = read_u8(data, pos)? as usize;
                pos += 1;
                if scan_count == 0 || scan_count > components.len() {
                    return Err(ImageError::compression(
                        "Baseline JPEG: invalid scan component count",
                    ));
                }
                let mut scan = Vec::with_capacity(scan_count);
                for _ in 0..scan_count {
                    let selector = read_u8(data, pos)?;
                    let td_ta = read_u8(data, pos + 1)?;
                    pos += 2;
                    let frame_index = components
                        .iter()
                        .position(|c| c.id == selector)
                        .ok_or_else(|| {
                            ImageError::compression(
                                "Baseline JPEG: scan references unknown component",
                            )
                        })?;
                    scan.push(ScanComponent {
                        frame_index,
                        dc_table: ((td_ta >> 4) & 0x0F) as usize,
                        ac_table: (td_ta & 0x0F) as usize,
                    });
                }
                pos = end; // skip Ss/Se/Ah/Al (fixed for baseline)
                let entropy = &data[pos..];
                return decode_scan(
                    width,
                    height,
                    &components,
                    &scan,
                    &quant,
                    &dc_tables,
                    &ac_tables,
                    restart_interval,
                    entropy,
                );
            }
            MARKER_EOI => {
                return Err(ImageError::compression(
                    "Baseline JPEG: EOI before scan data",
                ));
            }
            _ => {
                // APPn, COM, etc.
                let end = read_segment_end(data, pos)?;
                pos = end;
            }
        }
    }
}

/// Read a big-endian segment length and return the absolute end offset.
fn read_segment_end(data: &[u8], pos: usize) -> ImageResult<usize> {
    let length = read_u16(data, pos)? as usize;
    if length < 2 {
        return Err(ImageError::compression(
            "Baseline JPEG: invalid segment length",
        ));
    }
    pos.checked_add(length)
        .filter(|&e| e <= data.len())
        .ok_or_else(|| ImageError::compression("Baseline JPEG: segment overruns data"))
}

fn read_u8(data: &[u8], pos: usize) -> ImageResult<u8> {
    data.get(pos)
        .copied()
        .ok_or_else(|| ImageError::compression("Baseline JPEG: unexpected end of data"))
}

fn read_u16(data: &[u8], pos: usize) -> ImageResult<u16> {
    if pos + 2 > data.len() {
        return Err(ImageError::compression(
            "Baseline JPEG: unexpected end of data",
        ));
    }
    Ok(u16::from_be_bytes([data[pos], data[pos + 1]]))
}

/// Decode the interleaved baseline scan into per-component planes.
#[allow(clippy::too_many_arguments)]
fn decode_scan(
    width: u16,
    height: u16,
    components: &[Component],
    scan: &[ScanComponent],
    quant: &[Option<[u16; 64]>; 4],
    dc_tables: &[Option<HuffTable>; 4],
    ac_tables: &[Option<HuffTable>; 4],
    restart_interval: u32,
    entropy: &[u8],
) -> ImageResult<BaselineJpegImage> {
    let width = width as usize;
    let height = height as usize;
    if width == 0 || height == 0 {
        return Err(ImageError::compression("Baseline JPEG: zero-sized image"));
    }
    // Defend against an allocation bomb: width/height drive the MCU-padded
    // per-component plane allocations below. Reject frames above the decode
    // ceiling before any plane is sized.
    crate::limits::check_dimensions(width, height).map_err(ImageError::InvalidFormat)?;

    // Maximum sampling factors define the MCU geometry.
    let h_max = scan
        .iter()
        .map(|s| components[s.frame_index].h)
        .max()
        .unwrap_or(1)
        .max(1) as usize;
    let v_max = scan
        .iter()
        .map(|s| components[s.frame_index].v)
        .max()
        .unwrap_or(1)
        .max(1) as usize;

    let mcu_width = 8 * h_max;
    let mcu_height = 8 * v_max;
    let mcus_x = width.div_ceil(mcu_width);
    let mcus_y = height.div_ceil(mcu_height);

    // Allocate one plane per scan component, padded to whole MCUs.
    struct PlaneBuf {
        width: usize,
        height: usize,
        h: u8,
        v: u8,
        quant: [u16; 64],
        samples: Vec<u8>,
    }
    let mut planes: Vec<PlaneBuf> = Vec::with_capacity(scan.len());
    for sc in scan {
        let fc = &components[sc.frame_index];
        let h = fc.h.max(1) as usize;
        let v = fc.v.max(1) as usize;
        let plane_w = mcus_x * h * 8;
        let plane_h = mcus_y * v * 8;
        let qt = quant[fc.quant_id].ok_or_else(|| {
            ImageError::compression("Baseline JPEG: component references undefined quant table")
        })?;
        planes.push(PlaneBuf {
            width: plane_w,
            height: plane_h,
            h: fc.h.max(1),
            v: fc.v.max(1),
            quant: qt,
            samples: vec![128u8; plane_w * plane_h],
        });
    }

    let mut reader = BitReader::new(entropy);
    let mut dc_pred = vec![0i32; scan.len()];
    let mut mcu_in_interval: u32 = 0;

    for my in 0..mcus_y {
        for mx in 0..mcus_x {
            if restart_interval != 0 && mcu_in_interval == restart_interval {
                reader.restart()?;
                for p in &mut dc_pred {
                    *p = 0;
                }
                mcu_in_interval = 0;
            }
            // Each MCU contains, per component, Hi×Vi data units in raster order.
            for (ci, sc) in scan.iter().enumerate() {
                let dc_table = dc_tables[sc.dc_table].as_ref().ok_or_else(|| {
                    ImageError::compression("Baseline JPEG: undefined DC Huffman table")
                })?;
                let ac_table = ac_tables[sc.ac_table].as_ref().ok_or_else(|| {
                    ImageError::compression("Baseline JPEG: undefined AC Huffman table")
                })?;
                let plane = &mut planes[ci];
                let comp_h = plane.h as usize;
                let comp_v = plane.v as usize;

                for by in 0..comp_v {
                    for bx in 0..comp_h {
                        let mut block = [0f32; 64];
                        decode_block(
                            &mut reader,
                            dc_table,
                            ac_table,
                            &plane.quant,
                            &mut dc_pred[ci],
                            &mut block,
                        )?;
                        idct_8x8(&mut block);

                        // Place the 8×8 block into the component plane.
                        let origin_x = (mx * comp_h + bx) * 8;
                        let origin_y = (my * comp_v + by) * 8;
                        for yy in 0..8 {
                            let py = origin_y + yy;
                            if py >= plane.height {
                                continue;
                            }
                            let row = py * plane.width;
                            for xx in 0..8 {
                                let px = origin_x + xx;
                                if px >= plane.width {
                                    continue;
                                }
                                let value = block[yy * 8 + xx].round().clamp(0.0, 255.0);
                                plane.samples[row + px] = value as u8;
                            }
                        }
                    }
                }
            }
            mcu_in_interval += 1;
        }
    }

    // Crop each plane to the component's true (unpadded) extent.
    let mut out_planes = Vec::with_capacity(planes.len());
    for plane in planes {
        let comp_w = (width * plane.h as usize).div_ceil(h_max);
        let comp_h = (height * plane.v as usize).div_ceil(v_max);
        let mut cropped = vec![0u8; comp_w * comp_h];
        for y in 0..comp_h {
            let src = y * plane.width;
            let dst = y * comp_w;
            cropped[dst..dst + comp_w].copy_from_slice(&plane.samples[src..src + comp_w]);
        }
        out_planes.push(ComponentPlane {
            width: comp_w as u32,
            height: comp_h as u32,
            h: plane.h,
            v: plane.v,
            samples: cropped,
        });
    }

    Ok(BaselineJpegImage {
        width: width as u32,
        height: height as u32,
        planes: out_planes,
    })
}

/// Decode and dequantize one 8×8 block into natural-order coefficients.
fn decode_block(
    reader: &mut BitReader<'_>,
    dc_table: &HuffTable,
    ac_table: &HuffTable,
    quant: &[u16; 64],
    dc_pred: &mut i32,
    block: &mut [f32; 64],
) -> ImageResult<()> {
    // DC coefficient: magnitude category then differential value.
    let dc_size = dc_table.decode_symbol(reader)?;
    if dc_size > 16 {
        return Err(ImageError::compression(
            "Baseline JPEG: DC magnitude category out of range",
        ));
    }
    let dc_diff = if dc_size == 0 {
        0
    } else {
        extend(reader.read_bits(dc_size)?, dc_size)
    };
    *dc_pred += dc_diff;
    block[0] = (*dc_pred * i32::from(quant[0])) as f32;

    // AC coefficients: run/size pairs in zigzag order.
    let mut k = 1usize;
    while k < 64 {
        let rs = ac_table.decode_symbol(reader)?;
        let run = (rs >> 4) as usize;
        let size = rs & 0x0F;
        if size == 0 {
            if run == 15 {
                // ZRL: skip 16 zero coefficients.
                k += 16;
                continue;
            }
            // EOB: the rest of the block is zero.
            break;
        }
        k += run;
        if k >= 64 {
            break;
        }
        let coeff = extend(reader.read_bits(size)?, size);
        // `quant` holds the DQT segment as it was read, i.e. in zigzag scan
        // order (T.81 §B.2.4.1), so the divisor for scan position `k` is
        // `quant[k]` — not `quant[ZIGZAG[k]]`, which pairs a coefficient with
        // another frequency's step size and silently distorts every block.
        let natural = ZIGZAG[k];
        block[natural] = (coeff * i32::from(quant[k])) as f32;
        k += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jpeg::{JpegEncoder, JpegQuality};
    use crate::{ColorSpace, ImageData, ImageFrame, PixelType};

    #[test]
    fn extend_matches_t81() {
        assert_eq!(extend(0, 1), -1);
        assert_eq!(extend(1, 1), 1);
        assert_eq!(extend(0, 3), -7);
        assert_eq!(extend(7, 3), 7);
        assert_eq!(extend(0, 0), 0);
    }

    #[test]
    fn idct_of_dc_only_is_flat() {
        // A block whose only non-zero coefficient is the DC term must produce
        // a constant spatial block.
        let mut block = [0f32; 64];
        block[0] = 8.0 * 64.0; // DC coefficient scaled
        idct_8x8(&mut block);
        let first = block[0];
        for &v in &block {
            assert!((v - first).abs() < 1e-3, "IDCT of DC-only not flat");
        }
    }

    #[test]
    fn decode_grayscale_baseline_jpeg() {
        // Encode a grey 16×16 image with the crate's baseline JPEG encoder,
        // then decode it back with this decoder.
        let w = 16u32;
        let h = 16u32;
        let pixels: Vec<u8> = (0..(w * h)).map(|i| ((i * 7) % 200 + 20) as u8).collect();
        let frame = ImageFrame::new(
            0,
            w,
            h,
            PixelType::U8,
            1,
            ColorSpace::Luma,
            ImageData::interleaved(pixels.clone()),
        );
        let encoder = JpegEncoder::new(JpegQuality::high());
        let jpeg = encoder.encode(&frame).expect("encode");

        let decoded = decode_baseline_jpeg(&jpeg).expect("decode");
        assert_eq!(decoded.width, w);
        assert_eq!(decoded.height, h);
        assert_eq!(decoded.planes.len(), 1);
        let plane = &decoded.planes[0];
        assert_eq!(plane.width, w);
        assert_eq!(plane.height, h);

        // Lossy: allow a generous tolerance, but the structure must survive.
        let mut max_err = 0i32;
        for (a, b) in plane.samples.iter().zip(pixels.iter()) {
            max_err = max_err.max((i32::from(*a) - i32::from(*b)).abs());
        }
        assert!(max_err < 40, "grayscale JPEG round-trip error {max_err}");
    }

    #[test]
    fn decode_color_baseline_jpeg() {
        // RGB gradient encoded as YCbCr baseline JPEG.
        let w = 16u32;
        let h = 16u32;
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                rgb.push((x * 16) as u8);
                rgb.push((y * 16) as u8);
                rgb.push(128);
            }
        }
        let frame = ImageFrame::new(
            0,
            w,
            h,
            PixelType::U8,
            3,
            ColorSpace::Srgb,
            ImageData::interleaved(rgb),
        );
        let encoder = JpegEncoder::new(JpegQuality::high());
        let jpeg = encoder.encode(&frame).expect("encode");
        let decoded = decode_baseline_jpeg(&jpeg).expect("decode");
        assert_eq!(decoded.width, w);
        assert_eq!(decoded.height, h);
        // 3 component planes (Y, Cb, Cr).
        assert_eq!(decoded.planes.len(), 3);
        for plane in &decoded.planes {
            assert!(!plane.samples.is_empty());
        }
    }

    #[test]
    fn rejects_truncated_data() {
        assert!(decode_baseline_jpeg(&[0xFF, MARKER_SOI]).is_err());
        assert!(decode_baseline_jpeg(&[]).is_err());
    }

    #[test]
    fn rejects_non_baseline_sof() {
        // SOI then SOF2 (progressive) must be rejected.
        let data = vec![0xFF, MARKER_SOI, 0xFF, 0xC2, 0x00, 0x02];
        assert!(decode_baseline_jpeg(&data).is_err());
    }
}
