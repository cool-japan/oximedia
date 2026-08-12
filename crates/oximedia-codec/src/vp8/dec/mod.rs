//! Pure-Rust VP8 decoder — RFC 6386 §9-§18.
//!
//! This module is the end-to-end VP8 key-frame reconstruction pipeline,
//! ported from the production-verified `oximedia-image` `webp/vp8` decoder
//! (a WebP lossy still image *is* a single VP8 key frame, so that decoder is
//! a complete VP8 intra decoder). The port keeps the algorithms identical
//! and swaps the output surface: instead of RGBA for still images, it
//! returns the reconstructed YUV 4:2:0 planes that a video decoder emits.
//!
//! The per-macroblock decode loop:
//! 1. parse the macroblock prediction modes (16x16/B_PRED luma + 8x8 chroma),
//! 2. decode the DCT coefficient tokens for the 25 (Y2 + 16 Y + 4 U + 4 V)
//!    sub-blocks (RFC 6386 §13) — [`residual`],
//! 3. dequantise (§14.1), inverse-transform (§14.3), intra-predict (§12) and
//!    reconstruct each plane — [`recon`],
//! 4. run the in-loop deblocking filter (§15) — [`filter`].
//!
//! This file keeps the still-image entry point ([`decode_keyframe`]), the
//! per-macroblock loop (the `Decoder` struct and its mode/segment-id
//! parsing), and the shared plane/state types (`Planes`, `MbInfo`,
//! `DequantFactors`); the residual decode, reconstruction and loop-filter
//! passes live in the sibling `residual`, `recon` and `filter` modules, each
//! an `impl Decoder` block extending the same type.
//!
//! # Inter frames
//!
//! Inter frames reuse everything above — the same `Decoder`, residual decode,
//! reconstruction and loop-filter passes — and add the per-macroblock
//! prediction records ([`mode`], §16), motion-vector entropy decode ([`mv`],
//! §17), sub-pixel motion compensation ([`mc`], §18), reference-buffer
//! management ([`refs`], §9.7-§9.8) and the cross-frame state an inter frame
//! inherits ([`state`], §9.3-§9.10). [`inter::Vp8SequenceDecoder`] is the
//! driver that ties those together and decodes a whole sequence;
//! [`decode_keyframe`] stays as the stateless single-key-frame entry point
//! the still-image (WebP) caller uses.

mod bool_decoder;
mod filter;
mod header;
mod inter;
mod loopfilter;
mod mc;
mod mode;
mod mv;
mod predict;
mod recon;
mod refs;
mod residual;
mod state;
mod tables;
mod tables_inter;
#[cfg(test)]
mod testutil;
mod transform;

use crate::error::{CodecError, CodecResult};
use bool_decoder::BoolDecoder;
use header::Vp8Header;
pub(crate) use inter::Vp8SequenceDecoder;
use tables::{
    clamp_qindex, AC_QUANT, BMODE_TREE, B_DC_PRED, B_HE_PRED, B_PRED, B_TM_PRED, B_VE_PRED,
    DC_PRED, DC_QUANT, H_PRED, KF_BMODE_PROB, KF_UV_MODE_PROB, KF_YMODE_PROB, KF_YMODE_TREE,
    TM_PRED, UV_MODE_TREE, V_PRED,
};

/// Number of pixels of border padding kept around each plane.
///
/// VP8 prediction reads up to one pixel left / above and (for sub-block modes)
/// up to four pixels above-right; a one-macroblock guard keeps all reads in
/// bounds without per-pixel branching beyond the explicit availability flags.
const BORDER: usize = 32;

/// The fully-decoded VP8 key frame as tightly-packed YUV 4:2:0 planes.
pub(crate) struct DecodedImage {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Luma plane, `width * height` bytes, stride == `width`.
    pub y: Vec<u8>,
    /// Chroma-blue plane, `chroma_width() * chroma_height()` bytes.
    pub u: Vec<u8>,
    /// Chroma-red plane, `chroma_width() * chroma_height()` bytes.
    pub v: Vec<u8>,
}

impl DecodedImage {
    /// Chroma plane width: `ceil(width / 2)` (4:2:0 subsampling).
    pub fn chroma_width(&self) -> u32 {
        self.width.div_ceil(2)
    }

    /// Chroma plane height: `ceil(height / 2)` (4:2:0 subsampling).
    pub fn chroma_height(&self) -> u32 {
        self.height.div_ceil(2)
    }
}

/// Decodes a complete VP8 key-frame payload into YUV 4:2:0 planes.
///
/// # Errors
/// Fails on malformed headers, truncated partitions, or non-key-frame input.
pub(crate) fn decode_keyframe(data: &[u8]) -> CodecResult<DecodedImage> {
    let (header, header_bd) = Vp8Header::parse(data)?;
    let mut decoder = Decoder::new(&header, header_bd, data)?;
    decoder.decode_all();
    Ok(decoder.finish())
}

/// Per-macroblock decoded state retained after the reconstruction pass.
#[derive(Clone, Copy)]
struct MbInfo {
    /// Whether the loop filter visits this macroblock's interior (sub-block)
    /// edges.
    ///
    /// dixie.c `filter_row_normal` / `filter_row_simple` (rfc6386.txt lines
    /// 9308-9310 and 9433-9435): `mbi->base.eob_mask || y_mode == SPLITMV ||
    /// y_mode == B_PRED`. Stored as the resolved boolean rather than as the
    /// mode, because "which modes always filter their interior" differs
    /// between the two frame types (a key frame has no `SPLITMV`) and the
    /// filter pass should not have to know which pass produced it.
    filter_inner: bool,
    /// Effective loop-filter level for this macroblock.
    filter_level: i32,
    /// Segment id (RFC 6386 §10), kept so a completed key frame can seed the
    /// persistent [`state::Vp8State::segment_map`] that later inter frames
    /// inherit when they do not retransmit the map.
    segment_id: u8,
}

/// Dequantisation factors for one segment (RFC 6386 §14.1).
#[derive(Clone, Copy, Default)]
struct DequantFactors {
    /// Luma DC factor.
    y_dc: i32,
    /// Luma AC factor.
    y_ac: i32,
    /// Y2 (WHT) DC factor.
    y2_dc: i32,
    /// Y2 (WHT) AC factor.
    y2_ac: i32,
    /// Chroma DC factor.
    uv_dc: i32,
    /// Chroma AC factor.
    uv_ac: i32,
}

/// Plane reconstruction buffers with padding borders.
struct Planes {
    /// Luma plane.
    y: Vec<u8>,
    /// Chroma-blue plane (subsampled 2x2).
    u: Vec<u8>,
    /// Chroma-red plane (subsampled 2x2).
    v: Vec<u8>,
    /// Stride (row length, including border) of the luma plane.
    y_stride: usize,
    /// Stride of each chroma plane.
    uv_stride: usize,
    /// Offset of luma pixel (0,0).
    y_origin: usize,
    /// Offset of chroma pixel (0,0).
    uv_origin: usize,
}

/// Internal decoder holding all per-frame mutable state.
struct Decoder<'a> {
    header: &'a Vp8Header,
    /// Boolean decoder over the first (header) partition — mode/segment data.
    bd: BoolDecoder<'a>,
    /// Boolean decoders, one per DCT-token partition.
    token_bd: Vec<BoolDecoder<'a>>,
    /// Macroblock columns.
    mb_cols: usize,
    /// Macroblock rows.
    mb_rows: usize,
    /// Reconstruction planes.
    planes: Planes,
    /// Per-MB info kept for the loop filter (row-major).
    mb_info: Vec<MbInfo>,
    /// Per-segment dequantisation factors.
    dequant: [DequantFactors; 4],
    /// 4x4 sub-block modes for the current macroblock row, used as the "above"
    /// context for the next row: `above_bmode[mb_col * 4 + col]`.
    above_bmode: Vec<u8>,
    /// "Above" non-zero-coefficient context for the entropy decoder.
    /// 9 entries per MB column: 4 Y + 2 U + 2 V + 1 Y2.
    above_nz: Vec<bool>,
}

impl<'a> Decoder<'a> {
    /// Builds a decoder, allocating planes and setting up token partitions.
    fn new(header: &'a Vp8Header, bd: BoolDecoder<'a>, data: &'a [u8]) -> CodecResult<Self> {
        let mb_cols = (header.width as usize).div_ceil(16);
        let mb_rows = (header.height as usize).div_ceil(16);

        // --- token partition setup (RFC 6386 §9.5) ---
        let num_parts = header.num_token_partitions;
        let mut token_bd = Vec::with_capacity(num_parts);
        let part_table_start = header.partitions_start;
        // For N partitions there are (N-1) 3-byte size entries.
        let size_table_len = 3 * (num_parts.saturating_sub(1));
        let mut part_data_start = part_table_start
            .checked_add(size_table_len)
            .ok_or_else(|| CodecError::InvalidBitstream("VP8: partition table overflow".into()))?;
        if part_data_start > data.len() {
            return Err(CodecError::InvalidBitstream(
                "VP8: partition size table truncated".to_string(),
            ));
        }
        for i in 0..num_parts {
            let size = if i + 1 < num_parts {
                let off = part_table_start + 3 * i;
                usize::from(data[off])
                    | (usize::from(data[off + 1]) << 8)
                    | (usize::from(data[off + 2]) << 16)
            } else {
                // Last partition runs to the end of the payload.
                data.len().saturating_sub(part_data_start)
            };
            let end = part_data_start
                .checked_add(size)
                .ok_or_else(|| CodecError::InvalidBitstream("VP8: partition overflow".into()))?;
            if end > data.len() {
                return Err(CodecError::InvalidBitstream(
                    "VP8: token partition exceeds payload".to_string(),
                ));
            }
            token_bd.push(BoolDecoder::new(&data[part_data_start..end]));
            part_data_start = end;
        }
        if token_bd.is_empty() {
            return Err(CodecError::InvalidBitstream(
                "VP8: no token partitions".to_string(),
            ));
        }

        // --- plane allocation with borders ---
        let y_w = mb_cols * 16;
        let y_h = mb_rows * 16;
        let uv_w = mb_cols * 8;
        let uv_h = mb_rows * 8;
        let y_stride = y_w + 2 * BORDER;
        let uv_stride = uv_w + 2 * BORDER;
        let y_origin = BORDER * y_stride + BORDER;
        let uv_origin = BORDER * uv_stride + BORDER;
        let planes = Planes {
            y: vec![129u8; y_stride * (y_h + 2 * BORDER)],
            u: vec![129u8; uv_stride * (uv_h + 2 * BORDER)],
            v: vec![129u8; uv_stride * (uv_h + 2 * BORDER)],
            y_stride,
            uv_stride,
            y_origin,
            uv_origin,
        };

        // --- per-segment dequantisation factors (RFC 6386 §14.1) ---
        let dequant = build_dequant(header);

        Ok(Self {
            header,
            bd,
            token_bd,
            mb_cols,
            mb_rows,
            planes,
            mb_info: vec![
                MbInfo {
                    filter_inner: false,
                    filter_level: 0,
                    segment_id: 0,
                };
                mb_cols * mb_rows
            ],
            dequant,
            above_bmode: vec![B_DC_PRED as u8; mb_cols * 4],
            above_nz: vec![false; mb_cols * 9],
        })
    }

    /// Decodes every macroblock row, then runs the loop filter.
    fn decode_all(&mut self) {
        for mb_y in 0..self.mb_rows {
            // "Left" sub-block mode context resets at the start of each MB row.
            let mut left_bmode = [B_DC_PRED as u8; 4];
            // "Left" non-zero context resets each row: 4 Y + 2 U + 2 V + 1 Y2.
            let mut left_nz = [false; 9];
            for mb_x in 0..self.mb_cols {
                self.decode_macroblock(mb_x, mb_y, &mut left_bmode, &mut left_nz);
            }
        }
        self.apply_loop_filter();
    }

    /// Decodes and reconstructs one macroblock.
    fn decode_macroblock(
        &mut self,
        mb_x: usize,
        mb_y: usize,
        left_bmode: &mut [u8; 4],
        left_nz: &mut [bool; 9],
    ) {
        let mb_idx = mb_y * self.mb_cols + mb_x;

        // --- segment id (RFC 6386 §10) ---
        let segment_id = self.read_segment_id();

        // --- mb_skip_coeff (RFC 6386 §11.1) ---
        let skip_coeff = if self.header.mb_no_skip_coeff {
            self.bd.get_bool(self.header.prob_skip_false)
        } else {
            false
        };

        // --- prediction modes (RFC 6386 §11.2-§11.4) ---
        // 4x4 sub-block modes for this MB, raster order (16 entries).
        let mut bmodes = [DC_PRED as u8; 16];
        let y_mode = self.read_kf_ymode();
        if y_mode == B_PRED {
            // Per-4x4 submodes with above/left context.
            for r in 0..4 {
                for c in 0..4 {
                    let above = if r == 0 {
                        usize::from(self.above_bmode[mb_x * 4 + c])
                    } else {
                        usize::from(bmodes[(r - 1) * 4 + c])
                    };
                    let left = if c == 0 {
                        usize::from(left_bmode[r])
                    } else {
                        usize::from(bmodes[r * 4 + c - 1])
                    };
                    let probs = &KF_BMODE_PROB[above][left];
                    let m = self.bd.read_tree(&BMODE_TREE, probs) as u8;
                    bmodes[r * 4 + c] = m;
                }
            }
        } else {
            // A whole-block luma mode implies a fixed equivalent submode for
            // the purposes of neighbouring B_PRED context (RFC 6386 §11.3).
            let implied = match y_mode {
                V_PRED => B_VE_PRED,
                H_PRED => B_HE_PRED,
                TM_PRED => B_TM_PRED,
                _ => B_DC_PRED,
            } as u8;
            bmodes = [implied; 16];
        }
        // Update above/left submode context for the next MB / row.
        for c in 0..4 {
            self.above_bmode[mb_x * 4 + c] = bmodes[12 + c];
        }
        for r in 0..4 {
            left_bmode[r] = bmodes[r * 4 + 3];
        }

        let uv_mode = self.read_kf_uvmode();

        // --- coefficient decode (RFC 6386 §13) ---
        // 25 sub-blocks: index 24 = Y2, 0..16 = Y, 16..20 = U, 20..24 = V.
        let mut coeffs = [[0i32; 16]; 25];
        let has_y2;
        let mut any_tokens = false;
        if skip_coeff {
            // Skipped MB: clear the non-zero context for the Y/U/V
            // sub-blocks. The Y2 context is reset only when this mode HAS a
            // Y2 block (non-B_PRED): a skipped Y2 counts as decoded
            // all-zero. For B_PRED (which never codes Y2) the Y2 context is
            // preserved — RFC 6386 reference decoder `reset_mb_context`
            // ("we have to preserve the context of the second order block
            // if this mode would not have updated it") and libvpx
            // `vp8_reset_mb_tokens_context`.
            for c in 0..8 {
                left_nz[c] = false;
                self.above_nz[mb_x * 9 + c] = false;
            }
            if y_mode != B_PRED {
                left_nz[8] = false;
                self.above_nz[mb_x * 9 + 8] = false;
            }
            has_y2 = y_mode != B_PRED;
        } else {
            let dq = self.dequant[segment_id as usize];
            let part = mb_y % self.token_bd.len();
            has_y2 = y_mode != B_PRED;
            any_tokens = self.decode_residuals(mb_x, has_y2, &dq, &mut coeffs, part, left_nz);
        }

        // --- reconstruction ---
        if y_mode == B_PRED {
            self.reconstruct_bpred(mb_x, mb_y, &bmodes, &coeffs);
        } else {
            self.reconstruct_y16(mb_x, mb_y, y_mode, has_y2, &mut coeffs);
        }
        self.reconstruct_chroma(mb_x, mb_y, uv_mode, &coeffs);

        // --- record loop-filter info ---
        // A key frame's macroblocks are all intra, i.e. reference-frame
        // delta slot 0, and only B_PRED takes a mode delta (slot 0) — dixie.c
        // lines 9192-9198.
        let mode_delta_slot = if y_mode == B_PRED { Some(0) } else { None };
        let filter_level = self.compute_mb_filter_level(segment_id, 0, mode_delta_slot);
        self.mb_info[mb_idx] = MbInfo {
            filter_inner: any_tokens || y_mode == B_PRED,
            filter_level,
            segment_id,
        };
    }

    /// Reads the per-MB segment id from the segment-id tree.
    fn read_segment_id(&mut self) -> u8 {
        if !self.header.segment.enabled || !self.header.segment.update_map {
            return 0;
        }
        let probs = &self.header.segment.tree_probs;
        // 2-level binary tree over 4 segments.
        if self.bd.get_bool(probs[0]) {
            if self.bd.get_bool(probs[2]) {
                3
            } else {
                2
            }
        } else if self.bd.get_bool(probs[1]) {
            1
        } else {
            0
        }
    }

    /// Reads the 16x16 luma mode for a key-frame macroblock.
    fn read_kf_ymode(&mut self) -> usize {
        self.bd.read_tree(&KF_YMODE_TREE, &KF_YMODE_PROB) as usize
    }

    /// Reads the 8x8 chroma mode for a key-frame macroblock.
    fn read_kf_uvmode(&mut self) -> usize {
        self.bd.read_tree(&UV_MODE_TREE, &KF_UV_MODE_PROB) as usize
    }

    /// Extracts the visible region of the reconstructed planes as tight
    /// YUV 4:2:0 buffers (crops the macroblock-alignment padding).
    fn finish(self) -> DecodedImage {
        self.crop_image()
    }

    /// [`Decoder::finish`] without consuming the decoder, so an inter-frame
    /// caller can also hand the (uncropped, bordered) planes to the
    /// decoded-picture buffer afterwards.
    fn crop_image(&self) -> DecodedImage {
        let w = self.header.width as usize;
        let h = self.header.height as usize;
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);

        let mut y = vec![0u8; w * h];
        for row in 0..h {
            let src = self.planes.y_origin + row * self.planes.y_stride;
            y[row * w..(row + 1) * w].copy_from_slice(&self.planes.y[src..src + w]);
        }
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        for row in 0..ch {
            let src = self.planes.uv_origin + row * self.planes.uv_stride;
            u[row * cw..(row + 1) * cw].copy_from_slice(&self.planes.u[src..src + cw]);
            v[row * cw..(row + 1) * cw].copy_from_slice(&self.planes.v[src..src + cw]);
        }

        DecodedImage {
            width: self.header.width,
            height: self.header.height,
            y,
            u,
            v,
        }
    }
}

/// Builds the per-segment dequantisation factor table (RFC 6386 §14.1).
fn build_dequant(header: &Vp8Header) -> [DequantFactors; 4] {
    let q = &header.quant;
    let seg = &header.segment;
    let mut out = [DequantFactors::default(); 4];
    for (s, df) in out.iter_mut().enumerate() {
        // Base AC quantiser index, optionally adjusted per segment.
        let base = if seg.enabled {
            if seg.abs_delta {
                seg.quantizer[s]
            } else {
                q.y_ac_qi + seg.quantizer[s]
            }
        } else {
            q.y_ac_qi
        };

        let y_ac_idx = clamp_qindex(base);
        let y_dc_idx = clamp_qindex(base + q.y_dc_delta);
        let y2_dc_idx = clamp_qindex(base + q.y2_dc_delta);
        let y2_ac_idx = clamp_qindex(base + q.y2_ac_delta);
        let uv_dc_idx = clamp_qindex(base + q.uv_dc_delta);
        let uv_ac_idx = clamp_qindex(base + q.uv_ac_delta);

        // Y2 scaling factors per RFC 6386 §14.1: DC x2, AC x155/100 (min 8).
        let y2_dc = DC_QUANT[y2_dc_idx] * 2;
        let mut y2_ac = AC_QUANT[y2_ac_idx] * 155 / 100;
        if y2_ac < 8 {
            y2_ac = 8;
        }
        // Chroma DC factor is capped at 132.
        let mut uv_dc = DC_QUANT[uv_dc_idx];
        if uv_dc > 132 {
            uv_dc = 132;
        }

        *df = DequantFactors {
            y_dc: DC_QUANT[y_dc_idx],
            y_ac: AC_QUANT[y_ac_idx],
            y2_dc,
            y2_ac,
            uv_dc,
            uv_ac: AC_QUANT[uv_ac_idx],
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_garbage() {
        // Not a valid VP8 payload.
        assert!(decode_keyframe(&[0u8; 4]).is_err());
    }

    #[test]
    fn test_rejects_bad_start_code() {
        // 32-byte payload with a valid key-frame tag but no start code.
        let mut data = vec![0u8; 32];
        let part_size: u32 = 16;
        let tag = part_size << 5; // key frame, partition size
        data[0] = (tag & 0xFF) as u8;
        data[1] = ((tag >> 8) & 0xFF) as u8;
        data[2] = ((tag >> 16) & 0xFF) as u8;
        // start code bytes left as 0 => invalid
        assert!(decode_keyframe(&data).is_err());
    }

    #[test]
    fn test_rejects_inter_frame_payload() {
        let mut data = vec![0u8; 32];
        data[0] = 0x11; // bit0 = 1 => inter frame
        assert!(decode_keyframe(&data).is_err());
    }

    #[test]
    fn test_build_dequant_no_segments() {
        let mut header = make_minimal_header();
        header.quant.y_ac_qi = 20;
        let dq = build_dequant(&header);
        assert_eq!(dq[0].y_ac, AC_QUANT[20]);
        assert_eq!(dq[0].y_dc, DC_QUANT[20]);
        // Y2 DC is 2x the DC table (RFC 6386 §14.1).
        assert_eq!(dq[0].y2_dc, DC_QUANT[20] * 2);
    }

    #[test]
    fn test_dequant_uv_dc_capped() {
        let mut header = make_minimal_header();
        header.quant.y_ac_qi = 127; // max index
        let dq = build_dequant(&header);
        assert!(dq[0].uv_dc <= 132, "chroma DC must be capped at 132");
    }

    /// Builds a minimal header for dequant tests.
    fn make_minimal_header() -> Vp8Header {
        Vp8Header {
            width: 16,
            height: 16,
            version: 0,
            horizontal_scale: 0,
            vertical_scale: 0,
            color_space: 0,
            clamping_required: true,
            segment: header::SegmentHeader::default(),
            loop_filter: header::LoopFilterHeader::default(),
            quant: header::QuantHeader::default(),
            coeff_probs: tables::DEFAULT_COEFF_PROBS,
            mb_no_skip_coeff: false,
            prob_skip_false: 0,
            partitions_start: 10,
            first_partition_size: 1,
            num_token_partitions: 1,
            // Additive inter-frame fields (package P2): irrelevant for this
            // key-frame-only dequant test helper.
            is_keyframe: true,
            inter: header::InterFrameHeader::default(),
        }
    }
}
