//! Baseline (sequential DCT, Huffman-coded) JPEG decoding.
//!
//! This is the decode half of [`crate::jpeg`]. It implements ITU-T T.81
//! process 1 (`SOF0`) and its extended-sequential sibling (`SOF1`, 8-bit),
//! including the parts a "simplified" decoder is tempted to skip:
//!
//! * **Per-component sampling factors** (`Hi × Vi`) and the interleaved MCU
//!   layout of T.81 §A.2.3 — this is what 4:2:0 and 4:2:2 are. Ignoring the
//!   factors and decoding every scan as 4:4:4 does not fail; it silently
//!   returns a picture that has nothing to do with the file.
//! * **Component → table bindings**: `Tq` from the frame header, `Td`/`Ta`
//!   from the scan header, rather than assuming the usual luma/chroma layout.
//! * **Quantization table ordering**: `DQT` elements arrive in zigzag scan
//!   order (T.81 §B.2.4.1) and are de-zigzagged here, once, at parse time.
//! * **Restart intervals**: `DRI` plus `RST`*n* with DC-predictor reset and
//!   bit-stream realignment (T.81 §F.2.1.3.1).
//!
//! Anything outside that envelope — progressive, arithmetic-coded, lossless,
//! hierarchical, 12-bit, or a chroma ratio this upsampler cannot express — is
//! rejected with a specific error. A redaction pipeline must never be handed a
//! plausible-looking image that is not the file it was given.

use super::entropy::{HuffDecodeTable, ScanReader};
use super::upsample;
use super::{extend, JpegFrame, ZIGZAG};
use crate::error::{ImageError, ImageResult};
use std::f32::consts::PI;

// Marker low bytes (the parent module exports the 16-bit forms used by the
// encoder; the decoder works one byte at a time).
const MARKER_SOF0: u8 = 0xC0;
const MARKER_SOF1: u8 = 0xC1;
const MARKER_DHT: u8 = 0xC4;
const MARKER_DQT: u8 = 0xDB;
const MARKER_DRI: u8 = 0xDD;
const MARKER_SOS: u8 = 0xDA;
const MARKER_EOI: u8 = 0xD9;
const MARKER_SOI: u8 = 0xD8;
const MARKER_TEM: u8 = 0x01;

/// Largest chroma upsampling ratio this decoder reproduces.
const MAX_UPSAMPLE_RATIO: usize = 2;

/// A component as declared by the frame header.
#[derive(Clone, Copy)]
struct FrameComponent {
    /// Component identifier, matched against the scan header's selectors.
    id: u8,
    /// Horizontal sampling factor `Hi`.
    h: usize,
    /// Vertical sampling factor `Vi`.
    v: usize,
    /// Quantization table selector `Tq`.
    quant_id: usize,
}

/// The frame geometry shared by every scan.
#[derive(Clone)]
struct Frame {
    width: usize,
    height: usize,
    h_max: usize,
    v_max: usize,
    mcus_x: usize,
    mcus_y: usize,
    components: Vec<FrameComponent>,
}

/// A component as referenced by a scan header.
struct ScanComponent {
    /// Index into [`Frame::components`] (and into the plane list).
    index: usize,
    /// DC Huffman table selector `Td`.
    dc_table: usize,
    /// AC Huffman table selector `Ta`.
    ac_table: usize,
}

/// One component's decoded samples.
pub(super) struct Plane {
    /// Horizontal sampling factor.
    h: usize,
    /// Vertical sampling factor.
    v: usize,
    /// True component extent, `ceil(width · Hi / Hmax)`.
    pub(super) width: usize,
    /// True component extent, `ceil(height · Vi / Vmax)`.
    pub(super) height: usize,
    /// Horizontal upsampling ratio `Hmax / Hi` (1 or 2).
    pub(super) scale_x: usize,
    /// Vertical upsampling ratio `Vmax / Vi` (1 or 2).
    pub(super) scale_y: usize,
    /// Row stride of the MCU-padded sample buffer.
    pub(super) stride: usize,
    /// Row count of the MCU-padded sample buffer.
    rows: usize,
    /// `stride · rows` samples.
    pub(super) samples: Vec<u8>,
    /// Quantization table selector for this component.
    quant_id: usize,
    /// Set once a scan has coded this component; a frame with an uncoded
    /// component is reported rather than filled in.
    decoded: bool,
}

#[cfg(test)]
impl Plane {
    /// Build a plane straight from samples, for the upsampler's unit tests.
    pub(super) fn for_upsample_test(
        width: usize,
        height: usize,
        scale_x: usize,
        scale_y: usize,
        stride: usize,
        rows: usize,
        samples: Vec<u8>,
    ) -> Self {
        Self {
            h: 1,
            v: 1,
            width,
            height,
            scale_x,
            scale_y,
            stride,
            rows,
            samples,
            quant_id: 0,
            decoded: true,
        }
    }
}

/// Decode a baseline JPEG datastream.
pub(super) fn decode(data: &[u8]) -> ImageResult<JpegFrame> {
    if data.len() < 4 {
        return Err(ImageError::invalid_format("JPEG data too short"));
    }
    if u16::from_be_bytes([data[0], data[1]]) != super::JPEG_SOI {
        return Err(ImageError::invalid_format("Not a JPEG file (missing SOI)"));
    }
    Decoder::new(data).run()
}

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    quant: [Option<[u16; 64]>; 4],
    dc_tables: [Option<HuffDecodeTable>; 4],
    ac_tables: [Option<HuffDecodeTable>; 4],
    frame: Option<Frame>,
    planes: Vec<Plane>,
    restart_interval: usize,
    scans: usize,
    cos_table: [[f32; 8]; 8],
}

impl<'a> Decoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 2,
            quant: [None; 4],
            dc_tables: [None, None, None, None],
            ac_tables: [None, None, None, None],
            frame: None,
            planes: Vec::new(),
            restart_interval: 0,
            scans: 0,
            cos_table: build_cos_table(),
        }
    }

    fn run(mut self) -> ImageResult<JpegFrame> {
        while let Some(marker) = self.next_marker() {
            match marker {
                MARKER_DQT => self.read_dqt()?,
                MARKER_DHT => self.read_dht()?,
                MARKER_DRI => self.read_dri()?,
                MARKER_SOF0 | MARKER_SOF1 => self.read_sof()?,
                MARKER_SOS => self.read_sos()?,
                MARKER_EOI => break,
                // Standalone markers carry no length field.
                MARKER_SOI | MARKER_TEM | 0xD0..=0xD7 => {}
                // Every remaining SOF flavour is outside baseline. Saying so
                // beats decoding the entropy data as if it were baseline.
                0xC2 | 0xC3 | 0xC5..=0xCB | 0xCD..=0xCF => {
                    return Err(unsupported_sof(marker));
                }
                // DAC: arithmetic coding conditioning.
                0xCC => {
                    return Err(ImageError::invalid_format(
                        "JPEG: arithmetic coding is not supported (baseline Huffman only)",
                    ));
                }
                // DNL, APPn, COM, DHP, EXP, …: skip by declared length.
                _ => {
                    self.pos = self.segment_end()?;
                }
            }
        }
        self.finish()
    }

    /// Advance to the next marker, tolerating fill bytes and stray data.
    fn next_marker(&mut self) -> Option<u8> {
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF {
                let marker = self.data[self.pos + 1];
                if marker != 0x00 && marker != 0xFF {
                    self.pos += 2;
                    return Some(marker);
                }
            }
            self.pos += 1;
        }
        None
    }

    /// Absolute end offset of the segment whose length field starts at `pos`.
    fn segment_end(&self) -> ImageResult<usize> {
        let length = read_u16(self.data, self.pos)? as usize;
        if length < 2 {
            return Err(ImageError::invalid_format("JPEG: invalid segment length"));
        }
        self.pos
            .checked_add(length)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| ImageError::invalid_format("JPEG: segment overruns the file"))
    }

    fn read_dqt(&mut self) -> ImageResult<()> {
        let end = self.segment_end()?;
        self.pos += 2;
        while self.pos < end {
            let pq_tq = read_u8(self.data, self.pos)?;
            self.pos += 1;
            let precision = (pq_tq >> 4) & 0x0F;
            let id = (pq_tq & 0x0F) as usize;
            if id >= 4 {
                return Err(ImageError::invalid_format(
                    "JPEG: quantization table id out of range",
                ));
            }
            if precision > 1 {
                return Err(ImageError::invalid_format(
                    "JPEG: quantization table precision out of range",
                ));
            }
            // T.81 §B.2.4.1: element k is the k-th coefficient in *zigzag*
            // scan order. De-zigzag once, here, so that every later use can
            // index the table in natural (row-major) block order.
            let mut table = [0u16; 64];
            for k in 0..64usize {
                let value = if precision == 0 {
                    let value = u16::from(read_u8(self.data, self.pos)?);
                    self.pos += 1;
                    value
                } else {
                    let value = read_u16(self.data, self.pos)?;
                    self.pos += 2;
                    value
                };
                table[ZIGZAG[k] as usize] = value.max(1);
            }
            self.quant[id] = Some(table);
        }
        self.pos = end;
        Ok(())
    }

    fn read_dht(&mut self) -> ImageResult<()> {
        let end = self.segment_end()?;
        self.pos += 2;
        while self.pos < end {
            let tc_th = read_u8(self.data, self.pos)?;
            self.pos += 1;
            let class = (tc_th >> 4) & 0x0F;
            let id = (tc_th & 0x0F) as usize;
            if id >= 4 || class > 1 {
                return Err(ImageError::invalid_format(
                    "JPEG: Huffman table id or class out of range",
                ));
            }
            let mut counts = [0u8; 16];
            let mut total = 0usize;
            for slot in &mut counts {
                *slot = read_u8(self.data, self.pos)?;
                self.pos += 1;
                total += *slot as usize;
            }
            if self.pos + total > end {
                return Err(ImageError::invalid_format(
                    "JPEG: Huffman table overruns its segment",
                ));
            }
            let mut values = Vec::with_capacity(total);
            for _ in 0..total {
                values.push(read_u8(self.data, self.pos)?);
                self.pos += 1;
            }
            let table = HuffDecodeTable::build(&counts, values);
            if class == 0 {
                self.dc_tables[id] = Some(table);
            } else {
                self.ac_tables[id] = Some(table);
            }
        }
        self.pos = end;
        Ok(())
    }

    fn read_dri(&mut self) -> ImageResult<()> {
        let end = self.segment_end()?;
        self.pos += 2;
        self.restart_interval = read_u16(self.data, self.pos)? as usize;
        self.pos = end;
        Ok(())
    }

    fn read_sof(&mut self) -> ImageResult<()> {
        if self.frame.is_some() {
            return Err(ImageError::invalid_format(
                "JPEG: more than one frame header (hierarchical JPEG is unsupported)",
            ));
        }
        let end = self.segment_end()?;
        self.pos += 2;
        let body = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| ImageError::invalid_format("JPEG: SOF segment overruns the file"))?;
        // Shares the precision/dimension/component-count validation (and the
        // allocation-bomb ceiling) with the parent module's `SofHeader`.
        let header = super::parse_sof0(body)?;
        if header.precision != 8 {
            return Err(ImageError::invalid_format(format!(
                "JPEG: {}-bit sample precision is not baseline (only 8-bit is supported)",
                header.precision
            )));
        }
        let count = header.components as usize;
        let specs = body
            .get(6..6 + count * 3)
            .ok_or_else(|| ImageError::invalid_format("JPEG: SOF component list truncated"))?;

        let mut components = Vec::with_capacity(count);
        for spec in specs.chunks_exact(3) {
            let h = ((spec[1] >> 4) & 0x0F) as usize;
            let v = (spec[1] & 0x0F) as usize;
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                return Err(ImageError::invalid_format(format!(
                    "JPEG: component {} declares sampling factors {h}x{v} outside 1..=4",
                    spec[0]
                )));
            }
            let quant_id = spec[2] as usize;
            if quant_id >= 4 {
                return Err(ImageError::invalid_format(
                    "JPEG: component references a quantization table id out of range",
                ));
            }
            components.push(FrameComponent {
                id: spec[0],
                h,
                v,
                quant_id,
            });
        }
        if let Some(dup) = components
            .iter()
            .enumerate()
            .find(|(i, c)| components[..*i].iter().any(|o| o.id == c.id))
        {
            return Err(ImageError::invalid_format(format!(
                "JPEG: duplicate component id {} in the frame header",
                dup.1.id
            )));
        }

        let h_max = components.iter().map(|c| c.h).max().unwrap_or(1);
        let v_max = components.iter().map(|c| c.v).max().unwrap_or(1);
        for component in &components {
            check_ratio(h_max, component.h, component.id, 'h')?;
            check_ratio(v_max, component.v, component.id, 'v')?;
        }

        let width = header.width as usize;
        let height = header.height as usize;
        let mcus_x = width.div_ceil(8 * h_max);
        let mcus_y = height.div_ceil(8 * v_max);

        let mut planes = Vec::with_capacity(count);
        for component in &components {
            let stride = mcus_x * component.h * 8;
            let rows = mcus_y * component.v * 8;
            let len = crate::limits::checked_dims(stride, rows, 1, 1)
                .map_err(ImageError::InvalidFormat)?;
            planes.push(Plane {
                h: component.h,
                v: component.v,
                width: (width * component.h).div_ceil(h_max),
                height: (height * component.v).div_ceil(v_max),
                scale_x: h_max / component.h,
                scale_y: v_max / component.v,
                stride,
                rows,
                // 128 is the neutral level-shifted sample: a component that a
                // scan leaves partly uncovered stays mid-grey rather than
                // showing whatever was in memory.
                samples: vec![128u8; len],
                quant_id: component.quant_id,
                decoded: false,
            });
        }

        self.frame = Some(Frame {
            width,
            height,
            h_max,
            v_max,
            mcus_x,
            mcus_y,
            components,
        });
        self.planes = planes;
        self.pos = end;
        Ok(())
    }

    fn read_sos(&mut self) -> ImageResult<()> {
        let end = self.segment_end()?;
        self.pos += 2;
        // Cloned so the header parse below can keep mutating `self.pos`; a
        // frame header is at most four components.
        let frame = self
            .frame
            .clone()
            .ok_or_else(|| ImageError::invalid_format("JPEG: SOS before SOF0"))?;

        let count = read_u8(self.data, self.pos)? as usize;
        self.pos += 1;
        if count == 0 || count > frame.components.len() {
            return Err(ImageError::invalid_format(
                "JPEG: scan component count out of range",
            ));
        }
        let mut scan = Vec::with_capacity(count);
        for _ in 0..count {
            let selector = read_u8(self.data, self.pos)?;
            let td_ta = read_u8(self.data, self.pos + 1)?;
            self.pos += 2;
            let index = frame
                .components
                .iter()
                .position(|c| c.id == selector)
                .ok_or_else(|| {
                    ImageError::invalid_format(format!(
                        "JPEG: scan references component {selector}, which the frame does not declare"
                    ))
                })?;
            if scan.iter().any(|s: &ScanComponent| s.index == index) {
                return Err(ImageError::invalid_format(
                    "JPEG: scan lists the same component twice",
                ));
            }
            scan.push(ScanComponent {
                index,
                dc_table: ((td_ta >> 4) & 0x0F) as usize,
                ac_table: (td_ta & 0x0F) as usize,
            });
        }

        // Ss/Se/Ah/Al. Sequential scans always span the full spectrum with no
        // successive approximation; anything else is a progressive scan that
        // reached here without a SOF2 header.
        let ss = read_u8(self.data, self.pos)?;
        let se = read_u8(self.data, self.pos + 1)?;
        let ah_al = read_u8(self.data, self.pos + 2)?;
        if ss != 0 || se != 63 || ah_al != 0 {
            return Err(ImageError::invalid_format(format!(
                "JPEG: scan selects coefficients {ss}..={se} with approximation {ah_al:#04x}; \
                 only full sequential scans are supported"
            )));
        }
        self.pos = end;

        for sc in &scan {
            if self.planes[sc.index].decoded {
                return Err(ImageError::invalid_format(
                    "JPEG: component coded by more than one scan (progressive-style refinement)",
                ));
            }
        }

        let entropy_start = self.pos;
        let entropy_end = find_scan_end(self.data, entropy_start);
        let entropy = self
            .data
            .get(entropy_start..entropy_end)
            .ok_or_else(|| ImageError::invalid_format("JPEG: scan data overruns the file"))?;
        decode_scan(
            entropy,
            &frame,
            &scan,
            &mut self.planes,
            &self.quant,
            &self.dc_tables,
            &self.ac_tables,
            self.restart_interval,
            &self.cos_table,
        )?;
        for sc in &scan {
            self.planes[sc.index].decoded = true;
        }
        self.pos = entropy_end;
        self.scans += 1;
        Ok(())
    }

    fn finish(self) -> ImageResult<JpegFrame> {
        let frame = self
            .frame
            .ok_or_else(|| ImageError::invalid_format("JPEG: missing SOF0"))?;
        if self.scans == 0 {
            return Err(ImageError::invalid_format("JPEG: missing SOS"));
        }
        for (plane, component) in self.planes.iter().zip(frame.components.iter()) {
            if !plane.decoded {
                return Err(ImageError::invalid_format(format!(
                    "JPEG: component {} is never coded by any scan",
                    component.id
                )));
            }
        }
        let pixels = upsample::assemble(frame.width, frame.height, &self.planes)?;
        Ok(JpegFrame {
            width: frame.width as u32,
            height: frame.height as u32,
            components: self.planes.len() as u8,
            pixels,
        })
    }
}

/// Reject a sampling ratio this decoder cannot upsample exactly.
fn check_ratio(max: usize, factor: usize, id: u8, axis: char) -> ImageResult<()> {
    if max % factor != 0 || max / factor > MAX_UPSAMPLE_RATIO {
        return Err(ImageError::invalid_format(format!(
            "JPEG: component {id} needs a {max}:{factor} {axis} upsampling ratio, \
             which this decoder does not implement"
        )));
    }
    Ok(())
}

/// Name the non-baseline coding process a `SOF` marker selects.
fn unsupported_sof(marker: u8) -> ImageError {
    let process = match marker {
        0xC2 => "progressive DCT (SOF2)",
        0xC3 => "lossless (SOF3)",
        0xC5 => "differential sequential DCT (SOF5)",
        0xC6 => "differential progressive DCT (SOF6)",
        0xC7 => "differential lossless (SOF7)",
        0xC9 => "arithmetic-coded sequential DCT (SOF9)",
        0xCA => "arithmetic-coded progressive DCT (SOF10)",
        0xCB => "arithmetic-coded lossless (SOF11)",
        0xCD => "differential arithmetic sequential DCT (SOF13)",
        0xCE => "differential arithmetic progressive DCT (SOF14)",
        0xCF => "differential arithmetic lossless (SOF15)",
        _ => "a reserved coding process",
    };
    ImageError::invalid_format(format!(
        "JPEG: {process} is not supported; this decoder implements baseline sequential DCT only"
    ))
}

/// Offset of the marker that terminates the entropy-coded data starting at
/// `start`. Stuffed bytes and `RST`*n* belong to the scan and are skipped.
fn find_scan_end(data: &[u8], start: usize) -> usize {
    let mut i = start;
    while i + 1 < data.len() {
        if data[i] == 0xFF {
            let next = data[i + 1];
            if next != 0x00 && next != 0xFF && !(0xD0..=0xD7).contains(&next) {
                return i;
            }
        }
        i += 1;
    }
    data.len()
}

/// Decode one scan into the component planes.
#[allow(clippy::too_many_arguments)]
fn decode_scan(
    entropy: &[u8],
    frame: &Frame,
    scan: &[ScanComponent],
    planes: &mut [Plane],
    quant: &[Option<[u16; 64]>; 4],
    dc_tables: &[Option<HuffDecodeTable>; 4],
    ac_tables: &[Option<HuffDecodeTable>; 4],
    restart_interval: usize,
    cos_table: &[[f32; 8]; 8],
) -> ImageResult<()> {
    // T.81 §A.2: a scan carrying one component is *not* interleaved — its data
    // units run in raster order over that component's own block grid, with no
    // MCU padding. Multi-component scans use the interleaved MCU layout.
    let interleaved = scan.len() > 1;
    let (units_x, units_y) = if interleaved {
        (frame.mcus_x, frame.mcus_y)
    } else {
        let plane = &planes[scan[0].index];
        (plane.width.div_ceil(8), plane.height.div_ceil(8))
    };

    let mut tables = Vec::with_capacity(scan.len());
    for sc in scan {
        let dc = dc_tables[sc.dc_table].as_ref().ok_or_else(|| {
            ImageError::invalid_format(format!(
                "JPEG: scan uses DC Huffman table {} before it is defined",
                sc.dc_table
            ))
        })?;
        let ac = ac_tables[sc.ac_table].as_ref().ok_or_else(|| {
            ImageError::invalid_format(format!(
                "JPEG: scan uses AC Huffman table {} before it is defined",
                sc.ac_table
            ))
        })?;
        let quant_id = planes[sc.index].quant_id;
        let qt = quant[quant_id].as_ref().ok_or_else(|| {
            ImageError::invalid_format(format!(
                "JPEG: component references quantization table {quant_id} before it is defined"
            ))
        })?;
        tables.push((dc, ac, qt));
    }

    let mut reader = ScanReader::new(entropy);
    let mut dc_pred = vec![0i32; scan.len()];
    let mut units_since_restart = 0usize;
    let mut restart_index = 0u8;

    for uy in 0..units_y {
        for ux in 0..units_x {
            if restart_interval != 0 && units_since_restart == restart_interval {
                reader.restart(restart_index)?;
                restart_index = (restart_index + 1) & 0x07;
                for pred in &mut dc_pred {
                    *pred = 0;
                }
                units_since_restart = 0;
            }
            for (si, sc) in scan.iter().enumerate() {
                let plane = &mut planes[sc.index];
                let (blocks_h, blocks_v) = if interleaved {
                    (plane.h, plane.v)
                } else {
                    (1, 1)
                };
                let (dc_table, ac_table, qt) = tables[si];
                for by in 0..blocks_v {
                    for bx in 0..blocks_h {
                        let (block_x, block_y) = if interleaved {
                            (ux * plane.h + bx, uy * plane.v + by)
                        } else {
                            (ux, uy)
                        };
                        decode_block(
                            &mut reader,
                            dc_table,
                            ac_table,
                            qt,
                            &mut dc_pred[si],
                            cos_table,
                            plane,
                            block_x,
                            block_y,
                        )?;
                    }
                }
            }
            units_since_restart += 1;
        }
    }
    Ok(())
}

/// Decode, dequantize, inverse-transform and store one 8×8 data unit.
#[allow(clippy::too_many_arguments)]
fn decode_block(
    reader: &mut ScanReader<'_>,
    dc_table: &HuffDecodeTable,
    ac_table: &HuffDecodeTable,
    quant: &[u16; 64],
    dc_pred: &mut i32,
    cos_table: &[[f32; 8]; 8],
    plane: &mut Plane,
    block_x: usize,
    block_y: usize,
) -> ImageResult<()> {
    let mut block = [0f32; 64];

    // DC: magnitude category, then the differential value (T.81 §F.2.2.1).
    let dc_size = dc_table.decode_symbol(reader)?;
    if dc_size > 15 {
        return Err(ImageError::invalid_format(
            "JPEG: DC magnitude category out of range",
        ));
    }
    let dc_diff = if dc_size == 0 {
        0
    } else {
        extend(reader.read_bits(dc_size)?, dc_size)
    };
    *dc_pred += dc_diff;
    block[0] = (*dc_pred * i32::from(quant[0])) as f32;

    // AC: run/size pairs walking the zigzag scan (T.81 §F.2.2.2).
    let mut k = 1usize;
    while k < 64 {
        let rs = ac_table.decode_symbol(reader)?;
        let run = (rs >> 4) as usize;
        let size = rs & 0x0F;
        if size == 0 {
            if run == 15 {
                k += 16; // ZRL: sixteen zero coefficients.
                continue;
            }
            break; // EOB: the rest of the block is zero.
        }
        k += run;
        if k >= 64 {
            return Err(ImageError::invalid_format(
                "JPEG: AC coefficient run past the end of the block",
            ));
        }
        let coeff = extend(reader.read_bits(size)?, size);
        let natural = ZIGZAG[k] as usize;
        block[natural] = (coeff * i32::from(quant[natural])) as f32;
        k += 1;
    }

    idct_8x8(&mut block, cos_table);

    let origin_x = block_x * 8;
    let origin_y = block_y * 8;
    for yy in 0..8usize {
        let py = origin_y + yy;
        if py >= plane.rows {
            break;
        }
        let row = py * plane.stride;
        for xx in 0..8usize {
            let px = origin_x + xx;
            if px >= plane.stride {
                break;
            }
            plane.samples[row + px] = block[yy * 8 + xx].round().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(())
}

/// `COS[u][x] = C(u)·cos((2x+1)·u·π/16)`, with `C(0) = 1/√2`.
fn build_cos_table() -> [[f32; 8]; 8] {
    let mut table = [[0.0f32; 8]; 8];
    for (u, row) in table.iter_mut().enumerate() {
        let cu = if u == 0 { 1.0 / 2.0f32.sqrt() } else { 1.0 };
        for (x, slot) in row.iter_mut().enumerate() {
            *slot = cu * (((2 * x + 1) as f32) * (u as f32) * PI / 16.0).cos();
        }
    }
    table
}

/// Separable 8×8 inverse DCT with the level shift folded in.
///
/// Equivalent to [`crate::jpeg::idct_8x8`] but takes the cosine basis as an
/// argument: the scan loop builds it once instead of once per data unit.
fn idct_8x8(block: &mut [f32; 64], cos_table: &[[f32; 8]; 8]) {
    let mut tmp = [0.0f32; 64];
    for y in 0..8usize {
        for x in 0..8usize {
            let mut sum = 0.0f32;
            for u in 0..8usize {
                sum += cos_table[u][x] * block[y * 8 + u];
            }
            tmp[y * 8 + x] = sum * 0.5;
        }
    }
    for x in 0..8usize {
        for y in 0..8usize {
            let mut sum = 0.0f32;
            for v in 0..8usize {
                sum += cos_table[v][y] * tmp[v * 8 + x];
            }
            block[y * 8 + x] = sum * 0.5 + 128.0;
        }
    }
}

fn read_u8(data: &[u8], pos: usize) -> ImageResult<u8> {
    data.get(pos)
        .copied()
        .ok_or_else(|| ImageError::invalid_format("JPEG: unexpected end of data"))
}

fn read_u16(data: &[u8], pos: usize) -> ImageResult<u16> {
    let hi = read_u8(data, pos)?;
    let lo = read_u8(data, pos + 1)?;
    Ok(u16::from_be_bytes([hi, lo]))
}
