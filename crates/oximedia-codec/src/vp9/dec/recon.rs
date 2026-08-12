//! VP9 frame reconstruction driver — key, intra-only **and** inter frames.
//!
//! Exact port of libvpx `vp9/decoder/vp9_decodeframe.c` (`decode_tiles` /
//! `decode_partition` / `decode_block` /
//! `predict_and_reconstruct_intra_block` / `reconstruct_inter_block`),
//! `vp9_decodemv.c` (`read_intra_frame_mode_info` and, through
//! [`super::modeinfo`], `read_inter_frame_mode_info`) and `vp9_detokenize.c`
//! (`decode_coefs`), for 8-bit profile-0 4:2:0 frames.
//!
//! # The two reconstruction orders are *not* interchangeable
//!
//! An intra block interleaves prediction and residual per transform block —
//! `predict_and_reconstruct_intra_block` predicts from the neighbouring
//! **reconstructed** pixels, so the previous transform block's residual has
//! to be in the frame buffer before the next one predicts.
//!
//! An inter block does the opposite: `dec_build_inter_predictors_sb` builds
//! the prediction for the *whole block, all three planes* in one pass, and
//! only then does the per-transform-block residual loop add to it
//! (`vp9_decodeframe.c:1050-1085`). Motion compensation reads the reference
//! frame, never this frame, so nothing forces the interleave — and merging
//! the two orders would change the picture, because a compound block's
//! second reference averages into what the first wrote.
//!
//! # `eobtotal == 0` rewrites the block's skip flag
//!
//! `if (!less8x8 && eobtotal == 0) mi->skip = 1;` (`:1083`) is easy to read
//! as a loop-filter-only tweak — the comment says "skip loopfilter" — but
//! libvpx's mode-info cells *alias* one `MODE_INFO`, so the rewrite is also
//! visible to the next block's `vp9_get_skip_context` and
//! `get_tx_size_context`. The specification agrees: it stores `Skips[][]`
//! only after the fix-up (§6.4.4 `decode_block`). This module therefore
//! replicates the block's mode info into the grid **after** the residual
//! loop, not before.
//!
//! **No committed fixture reaches that branch.** Across the eight inter
//! fixtures the driver decodes 3219 inter blocks, of which 693 are not
//! skipped and so run the residual loop (23188 coefficients in total) — but
//! `eobtotal == 0` never once holds for one of `BLOCK_8X8` or larger.
//! Disabling the assignment leaves every fixture byte-identical, and
//! replacing it with a `panic!` never fires: libvpx's encoder does not emit a
//! `skip == 0` block whose coefficients all quantize away, so the case is
//! legal-but-unproduced rather than impossible. The code is transcribed from
//! libvpx and the spec, not from evidence — a stream that does hit it is the
//! way to pin it, and until one exists this paragraph is the honest statement
//! of what is verified.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_possible_truncation)]

use super::booldec::BoolReader;
use super::counts::{FrameCounts, EOB_MODEL_TOKEN, ONE_TOKEN, TWO_TOKEN, ZERO_TOKEN};
use super::hdr::{
    parse_compressed_header, FrameProbs, HeaderCfg, ReferenceMode, TxMode, SWITCHABLE,
};
use super::interpred::{dec_build_inter_predictors_sb, FrameGeometry};
use super::itx::{inverse_transform_add, TxKind};
use super::lf::{loop_filter_frame, LoopFilterInfo};
use super::modeinfo::{
    frame_interp_filter, read_inter_frame_mode_info, read_tx_size, BlockPos, InterFrameCfg,
    Neighbours,
};
use super::mvref::TileBounds;
use super::pred::{predict_intra, PredMode};
use super::predctx::CompRefState;
use super::refs::{MvRefRow, Vp9RefSlot, LAST_FRAME, REFS_PER_FRAME};
use super::scan;
use super::tables;
use crate::error::{CodecError, CodecResult};
use crate::vp9::mv::MotionVector;
use crate::vp9::uncompressed::UncompressedHeader;

/// One reconstruction plane with MI-aligned dimensions.
///
/// `stride >= width` and `data` may be taller than `height`: the planes
/// [`decode_frame`] allocates carry a right/bottom margin of
/// [`PLANE_OVERHANG`] pixels, which a transform block at the picture edge is
/// allowed to be written into and which nothing ever reads back. Synthetic
/// buffers built by tests use the tight `stride == width`, `data.len() ==
/// stride * height` shape; every consumer must address rows through `stride`
/// and bound itself by `width` / `height` rather than by `data.len()`.
#[derive(Clone)]
pub struct PlaneBuf {
    /// Pixel data, at least `stride * height` bytes.
    pub data: Vec<u8>,
    /// Row stride, `>= width`.
    pub stride: usize,
    /// Aligned width in pixels.
    pub width: usize,
    /// Aligned height in pixels.
    pub height: usize,
}

/// Per-8x8 mode info (replicated over all covered grid cells like libvpx's
/// `mi_grid_visible` pointers).
///
/// This is libvpx's `MODE_INFO` (`vp9/common/vp9_blockd.h`) for both the
/// intra and inter paths: the intra decode writes the first block, the inter
/// decode (mode/MV parse, motion compensation, MV-candidate scans and the
/// loop-filter level lookup) writes and reads the second. The inter fields
/// are present now, defaulted to what `read_intra_frame_mode_info` produces,
/// so the shape is fixed for the packages that fill them.
#[derive(Clone, Copy)]
pub struct MiInfo {
    /// Block size (VP9 `BLOCK_SIZE` index).
    pub sb_type: u8,
    /// Skip flag (no residual).
    pub skip: bool,
    /// Transform size (0..=3).
    pub tx_size: u8,
    /// Segment id (0..=7).
    pub segment_id: u8,
    /// Luma prediction mode (`mi->mode`).
    pub mode: u8,
    /// Chroma prediction mode.
    pub uv_mode: u8,
    /// Sub-8x8 luma modes (`bmi[i].as_mode`).
    pub bmi: [u8; 4],
    /// Whether the block is inter-coded (`is_inter_block(mi)`, i.e.
    /// `ref_frame[0] > INTRA_FRAME`).
    pub is_inter: bool,
    /// Reference frames (`mi->ref_frame`): `-1` NONE, `0` INTRA, `1` LAST,
    /// `2` GOLDEN, `3` ALTREF — see [`super::refs`] for the named constants.
    pub ref_frame: [i8; 2],
    /// Block motion vectors, one per reference, 1/8-pel units (`mi->mv`).
    pub mv: [MotionVector; 2],
    /// Sub-8x8 motion vectors, `[sub-block][reference]`
    /// (`mi->bmi[i].as_mv`). Only the first `1`, `2` or `4` entries are
    /// meaningful, per `sb_type`.
    pub bmv: [[MotionVector; 2]; 4],
    /// Interpolation filter of an inter block (`mi->interp_filter`):
    /// `0` EIGHTTAP, `1` EIGHTTAP_SMOOTH, `2` EIGHTTAP_SHARP, `3` BILINEAR,
    /// `4` SWITCHABLE.
    pub interp_filter: u8,
    /// Whether this block's segment id was temporally predicted from the
    /// previous frame's map (`mi->seg_id_predicted`).
    pub seg_id_predicted: bool,
}

impl Default for MiInfo {
    /// The intra defaults, matching libvpx `read_intra_frame_mode_info`
    /// (`mi->ref_frame[0] = INTRA_FRAME; mi->ref_frame[1] = NONE;`) — so the
    /// intra decode path, which never assigns the inter fields, produces
    /// exactly the mode info libvpx does.
    fn default() -> Self {
        Self {
            sb_type: 0,
            skip: false,
            tx_size: 0,
            segment_id: 0,
            mode: 0,
            uv_mode: 0,
            bmi: [0; 4],
            is_inter: false,
            ref_frame: [super::refs::INTRA_FRAME, super::refs::NONE_FRAME],
            mv: [MotionVector::zero(); 2],
            bmv: [[MotionVector::zero(); 2]; 4],
            interp_filter: 0,
            seg_id_predicted: false,
        }
    }
}

/// Frame-wide mode-info grid.
pub struct FrameMi {
    /// Grid height in MI units.
    pub rows: usize,
    /// Grid width in MI units.
    pub cols: usize,
    data: Vec<MiInfo>,
}

impl FrameMi {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            data: vec![MiInfo::default(); rows * cols],
        }
    }

    /// Mode info at `(row, col)`.
    #[must_use]
    pub fn get(&self, row: usize, col: usize) -> MiInfo {
        self.data[row * self.cols + col]
    }

    fn set(&mut self, row: usize, col: usize, mi: MiInfo) {
        self.data[row * self.cols + col] = mi;
    }
}

/// One decoded frame: planes, display dimensions, the per-MI motion-vector
/// record a following frame's temporal candidate scan reads, and the
/// transform mode its compressed header chose.
pub struct DecodedFrame {
    /// Y, U, V planes (MI-aligned).
    pub planes: [PlaneBuf; 3],
    /// Display width.
    pub width: usize,
    /// Display height.
    pub height: usize,
    /// libvpx `cm->cur_frame->mvs`: `mi_rows * mi_cols` entries, row-major,
    /// written by `vp9_read_mode_info`'s inter branch only. An intra frame
    /// leaves every entry at [`MvRefRow::default`], exactly as libvpx leaves
    /// its freshly allocated array untouched.
    pub mvs: Vec<MvRefRow>,
    /// libvpx `cm->tx_mode` — carried out of the decode because backward
    /// adaptation ([`super::adapt::adapt_mode_probs`]) gates the
    /// transform-size probabilities on it, long after the compressed header
    /// that decided it is gone.
    pub tx_mode: TxMode,
}

/// The frame-level inter inputs the reconstruction driver cannot derive from
/// the uncompressed header alone: the resolved reference slots and the
/// previous frame's motion vectors.
///
/// Absent (`None` at the [`decode_frame`] parameter) for a key or intra-only
/// frame, which names no reference.
#[derive(Clone, Copy)]
pub struct InterRefs<'a> {
    /// LAST / GOLDEN / ALTREF, in that order — libvpx `cm->frame_refs`,
    /// i.e. `dpb[hdr.ref_frame_idx[i]]`. A block naming reference `n`
    /// (`1..=3`) reads entry `n - 1`.
    ///
    /// An entry may be `None` when the stream names an unwritten
    /// decoded-picture-buffer slot; motion compensation reports that as an
    /// honest [`CodecError::InvalidBitstream`] if a block actually uses it,
    /// rather than this driver failing frames that never touch it.
    pub active: [Option<&'a Vp9RefSlot>; REFS_PER_FRAME],
    /// `cm->prev_frame->mvs`, `Some` exactly when `cm->use_prev_frame_mvs`
    /// holds ([`super::state::Vp9DecState::use_prev_frame_mvs`]).
    pub prev_mvs: Option<&'a [MvRefRow]>,
}

/// Right / bottom margin every reconstruction plane is allocated with, in
/// pixels — the part of libvpx's frame-buffer border a transform block can
/// legitimately be written into.
///
/// A block may overhang the MI grid: `decode_partition` only forces a split
/// when the block's *half* would fall outside (`has_rows` / `has_cols`), so a
/// 64x64 block whose right half is off-grid is legal, and its transform
/// blocks are stepped by `max_blocks_wide` / `max_blocks_high` — which count
/// 4x4 units to the **aligned** picture edge, not to the transform's own
/// extent. A 32x32 transform can therefore start at aligned-width minus 4 and
/// write 28 pixels past it. libvpx does exactly that, into
/// `vpx_realloc_frame_buffer`'s border, and never reads those pixels back:
/// every neighbour read in [`super::pred`] is clamped to the aligned picture
/// (`build_intra_predictors`' `frame_width` is `y_width`, the aligned width,
/// not `y_crop_width`), motion compensation clamps to the reference's display
/// rectangle, and the loop filter and the output crop both stop at the MI
/// grid.
///
/// Without the margin the same write either runs off the end of the buffer
/// (a panic — which is what the 100x68 `switch` fixture's inter frames do) or
/// — worse, because it is silent — wraps around a `stride == width` row and
/// corrupts the *next* row of an already-reconstructed block to its left. 32
/// covers the 28-pixel worst case with room to spare and matches the
/// granularity libvpx allocates at.
///
/// "Never read back" is measured, not merely argued: filling the margin with
/// a poison byte instead of zero leaves all eight inter fixtures
/// byte-identical.
const PLANE_OVERHANG: usize = 32;

/// Per-segment dequant pairs `[dc, ac]` for Y and UV.
struct Dequant {
    y: [[i64; 2]; 8],
    uv: [[i64; 2]; 8],
}

/// `vp9_dc_quant` / `vp9_ac_quant` (8-bit).
fn dc_q(qindex: i32) -> i64 {
    i64::from(tables::DC_QLOOKUP[qindex.clamp(0, 255) as usize])
}
fn ac_q(qindex: i32) -> i64 {
    i64::from(tables::AC_QLOOKUP[qindex.clamp(0, 255) as usize])
}

/// `vp9_get_qindex`.
fn seg_qindex(hdr: &UncompressedHeader, seg_id: usize) -> i32 {
    let base = i32::from(hdr.quant.base_q_idx);
    if hdr.seg.enabled && hdr.seg.feature_enabled[seg_id][0] {
        let data = i32::from(hdr.seg.feature_data[seg_id][0]);
        let q = if hdr.seg.abs_delta { data } else { base + data };
        q.clamp(0, 255)
    } else {
        base
    }
}

/// The frame-level inter parameters a block's mode info and motion
/// compensation read, gathered once per frame.
///
/// `Copy` so a `&mut self` method can lift it out of [`FrameState`] in one
/// move and then borrow the probability context and the counters
/// independently — every field is a scalar or a shared reference.
#[derive(Clone, Copy)]
struct InterState<'h> {
    /// `cm->reference_mode`, as the compressed header decided it.
    reference_mode: ReferenceMode,
    /// `cm->comp_fixed_ref` / `cm->comp_var_ref` / `cm->ref_frame_sign_bias`.
    comp: CompRefState,
    /// `cm->interp_filter`, already mapped through `LITERAL_TO_FILTER`.
    interp_filter: u8,
    /// `cm->allow_high_precision_mv`.
    allow_high_precision_mv: bool,
    /// The resolved reference slots and previous-frame motion vectors.
    refs: InterRefs<'h>,
    /// Frame geometry, as motion compensation needs it.
    geom: FrameGeometry,
}

impl<'h> InterState<'h> {
    /// The decoded-picture-buffer slot a block's `ref_frame[i]` names.
    ///
    /// `INTRA_FRAME` (0) and `NONE` (-1) name no slot and yield `None`, which
    /// is what a single-reference block's second entry must be.
    fn slot(&self, ref_frame: i8) -> Option<&'h Vp9RefSlot> {
        if ref_frame < LAST_FRAME {
            return None;
        }
        self.refs
            .active
            .get((ref_frame - LAST_FRAME) as usize)
            .copied()
            .flatten()
    }
}

/// Whole-frame decode state.
struct FrameState<'h> {
    hdr: &'h UncompressedHeader,
    /// The frame's working probability context (libvpx `cm->fc`), borrowed
    /// from the decoder state: it arrives holding
    /// `frame_contexts[frame_context_idx]` and is left holding that context
    /// plus this frame's compressed-header updates, which is what backward
    /// adaptation then merges the counts into.
    probs: &'h mut FrameProbs,
    /// Symbol counters (libvpx `xd->counts`), `None` for a frame that will
    /// not adapt — libvpx nulls the pointer for `frame_parallel_decoding_mode`
    /// (`vp9_decodeframe.c:1979-1980`) and every `if (counts)` guard in
    /// `vp9_detokenize.c` / `vp9_decodemv.c` keys off exactly that.
    counts: Option<&'h mut FrameCounts>,
    tx_mode: TxMode,
    lossless: bool,
    /// The frame-level inter state, `None` for a key / intra-only frame.
    /// This is exactly libvpx's `frame_is_intra_only(cm)` predicate,
    /// inverted: the partition probabilities, the mode-info reader and the
    /// reconstruction order all branch on it.
    inter: Option<InterState<'h>>,
    dequant: Dequant,
    mi: FrameMi,
    /// libvpx `cm->cur_frame->mvs`, filled by the inter mode-info path only.
    mvs: Vec<MvRefRow>,
    planes: [PlaneBuf; 3],
    /// Entropy contexts per plane, `2 * aligned_mi_cols` bytes each.
    above_ctx: [Vec<u8>; 3],
    /// Per-plane left entropy contexts (one superblock tall).
    left_ctx: [[u8; 16]; 3],
    /// Partition contexts.
    above_seg_ctx: Vec<u8>,
    left_seg_ctx: [u8; 8],
    /// Scratch dequantized-coefficient block (one 32x32 max).
    dqcoeff: Vec<i64>,
    ss_x: usize,
    ss_y: usize,
}

/// `get_tile_offset` (vp9_tile_common.c).
fn tile_offset(idx: usize, mis: usize, log2: usize) -> usize {
    let sb_units = (mis + 7) >> 3;
    let offset = ((idx * sb_units) >> log2) << 3;
    offset.min(mis)
}

/// Decodes one VP9 frame — key, intra-only or inter — to planes.
///
/// Scope: 8-bit, 4:2:0 (profile 0). The caller validates profile/bit-depth
/// before calling.
///
/// `probs` is the working probability context (libvpx `cm->fc`) *already
/// loaded* from `frame_contexts[frame_context_idx]`; the compressed header's
/// `diff_update_prob` passes are applied to it in place, so it comes back
/// holding exactly what the frame decoded against — which is what backward
/// adaptation and the conditional `refresh_frame_context` save then consume.
/// For a key frame this is always the specification defaults (its
/// `setup_past_independence` reset guarantees it); for an intra-only or inter
/// frame it is whatever a previous frame adapted, which is the whole reason
/// it is a parameter rather than a local.
///
/// `counts` accumulates the symbol counters backward adaptation consumes, and
/// is `None` for a frame that will not adapt (libvpx's null `xd->counts`).
/// Counting never influences decode: every increment is on a counter no
/// decode path reads.
///
/// `refs` must be `Some` for a frame that is not
/// [`UncompressedHeader::is_intra_only`] and `None` for one that is — the two
/// select libvpx's `frame_is_intra_only(cm)` branches throughout, and a
/// mismatch would read the wrong syntax rather than merely mispredicting.
/// Passing the wrong one is refused, not silently accommodated.
///
/// # Errors
///
/// * [`CodecError::InvalidParameter`] when `refs` disagrees with the header's
///   frame type.
/// * [`CodecError::UnsupportedFeature`] for a frame feature this decoder does
///   not implement (inter-frame segmentation; reference scaling).
/// * [`CodecError::InvalidBitstream`] on malformed data.
pub fn decode_frame(
    hdr: &UncompressedHeader,
    frame_data: &[u8],
    probs: &mut FrameProbs,
    counts: Option<&mut FrameCounts>,
    refs: Option<InterRefs<'_>>,
) -> CodecResult<DecodedFrame> {
    let frame_is_intra_only = hdr.is_intra_only();
    if frame_is_intra_only != refs.is_none() {
        return Err(CodecError::InvalidParameter(format!(
            "VP9 decode_frame: frame_is_intra_only is {frame_is_intra_only} but \
             reference state was {} — the two select different bitstream syntax, \
             so they must agree",
            if refs.is_some() {
                "supplied"
            } else {
                "withheld"
            }
        )));
    }

    let width = hdr.width as usize;
    let height = hdr.height as usize;
    let mi_cols = (width + 7) >> 3;
    let mi_rows = (height + 7) >> 3;
    let aligned_mi_cols = (mi_cols + 7) & !7;

    // Compressed header slice.
    let ch_start = hdr.uncompressed_header_bytes;
    let ch_end = ch_start + usize::from(hdr.compressed_header_size);
    if ch_end > frame_data.len() {
        return Err(CodecError::InvalidBitstream(
            "VP9: compressed header extends past frame data".into(),
        ));
    }
    let lossless = hdr.quant.lossless();
    let compressed = parse_compressed_header(
        &frame_data[ch_start..ch_end],
        &HeaderCfg::from_header(hdr),
        probs,
    )?;
    let tx_mode = compressed.tx_mode;

    let (ss_x, ss_y) = (
        usize::from(hdr.subsampling_x),
        usize::from(hdr.subsampling_y),
    );

    // `vp9_setup_compound_reference_mode` is called from
    // `read_compressed_header` only when the frame codes a non-single
    // reference mode; deriving it unconditionally is safe because nothing
    // reads it under `SINGLE_REFERENCE` (`read_ref_frames` takes the
    // single-reference branch, and `get_reference_mode_context` is reached
    // only from `REFERENCE_MODE_SELECT`).
    let inter = refs.map(|refs| InterState {
        reference_mode: compressed.reference_mode,
        comp: CompRefState::from_sign_bias(&hdr.ref_frame_sign_bias),
        // The header stores the *raw* two-bit literal; `read_interp_filter`'s
        // permutation is applied here, exactly once, and swaps EIGHTTAP with
        // EIGHTTAP_SMOOTH on every block of a non-switchable frame if it is
        // skipped.
        interp_filter: frame_interp_filter(hdr.interp_filter),
        allow_high_precision_mv: hdr.allow_high_precision_mv,
        refs,
        geom: FrameGeometry {
            mi_rows,
            mi_cols,
            width,
            height,
            ss_x,
            ss_y,
        },
    });

    // Segment dequant tables (setup_segmentation_dequant).
    let mut dequant = Dequant {
        y: [[0; 2]; 8],
        uv: [[0; 2]; 8],
    };
    for seg in 0..8 {
        let q = seg_qindex(hdr, seg);
        dequant.y[seg][0] = dc_q(q + hdr.quant.y_dc_delta);
        dequant.y[seg][1] = ac_q(q);
        dequant.uv[seg][0] = dc_q(q + hdr.quant.uv_dc_delta);
        dequant.uv[seg][1] = ac_q(q + hdr.quant.uv_ac_delta);
    }

    let y_w = mi_cols * 8;
    let y_h = mi_rows * 8;
    let mk_plane = |w: usize, h: usize| PlaneBuf {
        // `stride > width` and the extra rows: libvpx's frame buffers carry a
        // border (`vpx_realloc_frame_buffer`'s `VP9_DEC_BORDER_IN_PIXELS`),
        // and a transform block is allowed to *write* past the MI-aligned
        // picture into it. See [`PLANE_OVERHANG`].
        data: vec![0u8; (w + PLANE_OVERHANG) * (h + PLANE_OVERHANG)],
        stride: w + PLANE_OVERHANG,
        width: w,
        height: h,
    };
    let planes = [
        mk_plane(y_w, y_h),
        mk_plane(y_w >> ss_x, y_h >> ss_y),
        mk_plane(y_w >> ss_x, y_h >> ss_y),
    ];

    let mut st = FrameState {
        hdr,
        probs,
        counts,
        tx_mode,
        lossless,
        inter,
        dequant,
        mi: FrameMi::new(mi_rows, mi_cols),
        mvs: vec![MvRefRow::default(); mi_rows * mi_cols],
        planes,
        above_ctx: [
            vec![0u8; 2 * aligned_mi_cols],
            vec![0u8; 2 * aligned_mi_cols],
            vec![0u8; 2 * aligned_mi_cols],
        ],
        left_ctx: [[0u8; 16]; 3],
        above_seg_ctx: vec![0u8; aligned_mi_cols],
        left_seg_ctx: [0u8; 8],
        dqcoeff: vec![0i64; 32 * 32],
        ss_x,
        ss_y,
    };

    // Tile buffers (get_tile_buffers): 4-byte big-endian sizes, last tile
    // implicit.
    let tile_cols = 1usize << hdr.tile_cols_log2;
    let tile_rows = 1usize << hdr.tile_rows_log2;
    let mut tile_readers: Vec<BoolReader<'_>> = Vec::with_capacity(tile_cols * tile_rows);
    {
        let mut data = &frame_data[ch_end..];
        for tr in 0..tile_rows {
            for tc in 0..tile_cols {
                let is_last = tr == tile_rows - 1 && tc == tile_cols - 1;
                let size = if is_last {
                    data.len()
                } else {
                    if data.len() < 4 {
                        return Err(CodecError::InvalidBitstream(
                            "VP9: truncated tile length".into(),
                        ));
                    }
                    let sz = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
                    data = &data[4..];
                    if sz > data.len() {
                        return Err(CodecError::InvalidBitstream(
                            "VP9: corrupt tile size".into(),
                        ));
                    }
                    sz
                };
                if size == 0 {
                    return Err(CodecError::InvalidBitstream("VP9: empty tile".into()));
                }
                let reader = BoolReader::new(&data[..size]).ok_or_else(|| {
                    CodecError::InvalidBitstream("VP9 tile: invalid marker bit".into())
                })?;
                tile_readers.push(reader);
                data = &data[size..];
            }
        }
    }

    // Tile decode loop (decode_tiles order).
    for tile_row in 0..tile_rows {
        let mi_row_start = tile_offset(tile_row, mi_rows, hdr.tile_rows_log2 as usize);
        let mi_row_end = tile_offset(tile_row + 1, mi_rows, hdr.tile_rows_log2 as usize);
        let mut mi_row = mi_row_start;
        while mi_row < mi_row_end {
            for tile_col in 0..tile_cols {
                let tile = TileBounds {
                    mi_col_start: tile_offset(tile_col, mi_cols, hdr.tile_cols_log2 as usize),
                    mi_col_end: tile_offset(tile_col + 1, mi_cols, hdr.tile_cols_log2 as usize),
                };
                let r = &mut tile_readers[tile_row * tile_cols + tile_col];
                st.left_ctx = [[0u8; 16]; 3];
                st.left_seg_ctx = [0u8; 8];
                let mut mi_col = tile.mi_col_start;
                while mi_col < tile.mi_col_end {
                    st.decode_partition(r, tile, mi_row, mi_col, 12, 4)?;
                    mi_col += 8;
                }
                if r.has_error() {
                    return Err(CodecError::InvalidBitstream(
                        "VP9 tile data overran its partition".into(),
                    ));
                }
            }
            mi_row += 8;
        }
    }

    // Loop filter.
    if hdr.loop_filter.filter_level != 0 {
        let mut seg_lf = [(false, 0i16); 8];
        for s in 0..8 {
            seg_lf[s] = (
                hdr.seg.enabled && hdr.seg.feature_enabled[s][1],
                hdr.seg.feature_data[s][1],
            );
        }
        // The full per-reference / per-mode delta table, not the intra-only
        // `LoopFilterInfo::new` shortcut: an inter frame reads
        // `lvl[seg][LAST|GOLDEN|ALTREF][mode_bucket]`, which that shortcut
        // fills with zeros. `hdr.loop_filter.{ref,mode}_deltas` are the
        // *merged* persistent deltas — `Vp9DecState::apply_loop_filter_deltas`
        // writes them back into the header before the tiles are decoded, so a
        // frame that signals no delta update still filters with the deltas an
        // earlier frame installed.
        let lfi = LoopFilterInfo::new_with_deltas(
            hdr.loop_filter.filter_level,
            hdr.loop_filter.sharpness,
            hdr.loop_filter.delta_enabled,
            hdr.loop_filter.ref_deltas,
            hdr.loop_filter.mode_deltas,
            hdr.seg.enabled,
            hdr.seg.abs_delta,
            &seg_lf,
        );
        loop_filter_frame(&mut st.planes, (ss_x, ss_y), &st.mi, &lfi);
    }

    Ok(DecodedFrame {
        planes: st.planes,
        width,
        height,
        mvs: st.mvs,
        tx_mode,
    })
}

impl FrameState<'_> {
    /// `decode_partition` (n4x4_l2 is the block width log2 in 4x4 units).
    fn decode_partition(
        &mut self,
        r: &mut BoolReader<'_>,
        tile: TileBounds,
        mi_row: usize,
        mi_col: usize,
        bsize: u8,
        n4x4_l2: usize,
    ) -> CodecResult<()> {
        if mi_row >= self.mi.rows || mi_col >= self.mi.cols {
            return Ok(());
        }
        let n8x8_l2 = n4x4_l2 - 1;
        let num_8x8 = 1usize << n8x8_l2;
        let hbs = num_8x8 >> 1;
        let has_rows = (mi_row + hbs) < self.mi.rows;
        let has_cols = (mi_col + hbs) < self.mi.cols;

        // `read_partition` (`vp9_decodeframe.c:1150-1170`). The probability
        // table is `dec_set_partition_probs`' choice (`:1136-1141`):
        //
        // ```c
        // xd->partition_probs =
        //     frame_is_intra_only(cm) ? &vp9_kf_partition_probs[0]
        //                             : (const vpx_prob (*)[PARTITION_TYPES - 1])
        //                                   cm->fc->partition_prob;
        // ```
        //
        // Copied out rather than borrowed so the counter update below can
        // take `self.counts` mutably.
        let ctx = {
            let above = (self.above_seg_ctx[mi_col] >> n8x8_l2) & 1;
            let left = (self.left_seg_ctx[mi_row & 7] >> n8x8_l2) & 1;
            usize::from(left) * 2 + usize::from(above) + n8x8_l2 * 4
        };
        let probs: [u8; 3] = if self.inter.is_none() {
            tables::KF_PARTITION_PROBS[ctx]
        } else {
            self.probs.partition[ctx]
        };
        let probs = &probs;
        let partition: u8 = if has_rows && has_cols {
            r.read_tree(&tables::PARTITION_TREE, probs)
        } else if !has_rows && has_cols {
            if r.read_bool(probs[1]) {
                3
            } else {
                1
            }
        } else if has_rows {
            if r.read_bool(probs[2]) {
                3
            } else {
                2
            }
        } else {
            3
        };
        // `read_partition`'s count (vp9_decodeframe.c:1167), which libvpx
        // takes on every branch including the implicit PARTITION_SPLIT that
        // costs no bits.
        if let Some(cnt) = self.counts.as_deref_mut() {
            cnt.partition[ctx][partition as usize] += 1;
        }

        let subsize = tables::SUBSIZE_LOOKUP[partition as usize][bsize as usize];
        if subsize < 0 {
            return Err(CodecError::InvalidBitstream(
                "VP9: invalid partition subsize".into(),
            ));
        }
        let subsize = subsize as u8;

        if hbs == 0 {
            // 8x8 splits into sub-8x8 block types.
            self.decode_block(r, tile, mi_row, mi_col, subsize, 1, 1)?;
        } else {
            match partition {
                0 => self.decode_block(r, tile, mi_row, mi_col, subsize, n4x4_l2, n4x4_l2)?,
                1 => {
                    self.decode_block(r, tile, mi_row, mi_col, subsize, n4x4_l2, n8x8_l2)?;
                    if has_rows {
                        self.decode_block(
                            r,
                            tile,
                            mi_row + hbs,
                            mi_col,
                            subsize,
                            n4x4_l2,
                            n8x8_l2,
                        )?;
                    }
                }
                2 => {
                    self.decode_block(r, tile, mi_row, mi_col, subsize, n8x8_l2, n4x4_l2)?;
                    if has_cols {
                        self.decode_block(
                            r,
                            tile,
                            mi_row,
                            mi_col + hbs,
                            subsize,
                            n8x8_l2,
                            n4x4_l2,
                        )?;
                    }
                }
                _ => {
                    self.decode_partition(r, tile, mi_row, mi_col, subsize, n8x8_l2)?;
                    self.decode_partition(r, tile, mi_row, mi_col + hbs, subsize, n8x8_l2)?;
                    self.decode_partition(r, tile, mi_row + hbs, mi_col, subsize, n8x8_l2)?;
                    self.decode_partition(r, tile, mi_row + hbs, mi_col + hbs, subsize, n8x8_l2)?;
                }
            }
        }

        // dec_update_partition_context
        if bsize >= 3 && (bsize == 3 || partition != 3) {
            let pc = tables::PARTITION_CONTEXT_LOOKUP[subsize as usize];
            let above_end = (mi_col + num_8x8).min(self.above_seg_ctx.len());
            for v in &mut self.above_seg_ctx[mi_col..above_end] {
                *v = pc[0];
            }
            let left_start = mi_row & 7;
            for v in &mut self.left_seg_ctx[left_start..(left_start + num_8x8).min(8)] {
                *v = pc[1];
            }
        }
        Ok(())
    }

    /// `decode_block` (`vp9_decodeframe.c:1030-1090`) — both frame types.
    fn decode_block(
        &mut self,
        r: &mut BoolReader<'_>,
        tile: TileBounds,
        mi_row: usize,
        mi_col: usize,
        bsize: u8,
        bwl: usize,
        bhl: usize,
    ) -> CodecResult<()> {
        let bw = 1usize << (bwl - 1);
        let bh = 1usize << (bhl - 1);
        let x_mis = bw.min(self.mi.cols - mi_col);
        let y_mis = bh.min(self.mi.rows - mi_row);

        let above_mi = if mi_row > 0 {
            Some(self.mi.get(mi_row - 1, mi_col))
        } else {
            None
        };
        let left_mi = if mi_col > tile.mi_col_start {
            Some(self.mi.get(mi_row, mi_col - 1))
        } else {
            None
        };

        if bsize >= 3 && (self.ss_x != 0 || self.ss_y != 0) {
            let uv_subsize = tables::SS_SIZE_LOOKUP[bsize as usize][self.ss_x][self.ss_y];
            if uv_subsize < 0 {
                return Err(CodecError::InvalidBitstream(
                    "VP9: invalid uv block size".into(),
                ));
            }
        }

        // `vp9_read_mode_info` (`vp9_decodemv.c:826-848`) — the two frame
        // types read entirely different syntax here.
        let mut mi = match self.inter {
            None => self.read_intra_frame_mode_info(r, above_mi.as_ref(), left_mi.as_ref(), bsize),
            Some(inter) => {
                let cfg = InterFrameCfg {
                    tx_mode: self.tx_mode,
                    reference_mode: inter.reference_mode,
                    interp_filter: inter.interp_filter,
                    allow_high_precision_mv: inter.allow_high_precision_mv,
                    comp: inter.comp,
                    seg: &self.hdr.seg,
                    // The candidate scan is bounded by the *current tile*'s
                    // columns, not the frame's (libvpx `xd->tile`).
                    tile,
                    prev_mvs: inter.refs.prev_mvs,
                };
                read_inter_frame_mode_info(
                    r,
                    &self.mi,
                    &cfg,
                    self.probs,
                    self.counts.as_deref_mut(),
                    Neighbours {
                        above: above_mi.as_ref(),
                        left: left_mi.as_ref(),
                    },
                    BlockPos {
                        mi_row,
                        mi_col,
                        bsize,
                    },
                )?
            }
        };

        // `dec_reset_skip_context` (`:1023-1028`) — on the flag as *read*,
        // before the `eobtotal` rewrite below can touch it.
        if mi.skip {
            let above_len = 2 * self.mi_cols_al();
            for plane in 0..3 {
                let (px, py) = self.plane_ss(plane);
                let n4_w = (bw << 1) >> px;
                let n4_h = (bh << 1) >> py;
                let a0 = (mi_col * 2) >> px;
                let l0 = ((mi_row * 2) & 15) >> py;
                for v in &mut self.above_ctx[plane][a0..(a0 + n4_w).min(above_len)] {
                    *v = 0;
                }
                for v in &mut self.left_ctx[plane][l0..(l0 + n4_h).min(16)] {
                    *v = 0;
                }
            }
        }

        if mi.is_inter {
            // Prediction first, for the whole block and all three planes
            // (`dec_build_inter_predictors_sb`, `:1065`) — see the module
            // docs on why this may not be folded into the residual loop.
            let inter = self.inter.ok_or_else(|| {
                CodecError::InvalidBitstream(
                    "VP9: inter block on a frame with no reference state".into(),
                )
            })?;
            let block_refs = [inter.slot(mi.ref_frame[0]), inter.slot(mi.ref_frame[1])];
            dec_build_inter_predictors_sb(
                &mut self.planes,
                &mi,
                block_refs,
                &inter.geom,
                mi_row,
                mi_col,
            )?;

            // Reconstruction (`:1068-1084`): a skipped inter block codes no
            // residual at all, so the plane loop does not run for one.
            if !mi.skip {
                let eobtotal =
                    self.residual_loop(r, &mi, tile, mi_row, mi_col, bsize, bwl, bhl, None)?;
                // `if (!less8x8 && eobtotal == 0) mi->skip = 1;` (`:1083`) —
                // see the module docs: this is visible to the loop filter
                // *and* to the next block's skip / tx-size contexts.
                if bsize >= 3 && eobtotal == 0 {
                    mi.skip = true;
                }
            }
        } else {
            // Intra: prediction and residual interleave per transform block,
            // and the loop runs even for a skipped block because the
            // prediction still has to be written.
            self.residual_loop(
                r,
                &mi,
                tile,
                mi_row,
                mi_col,
                bsize,
                bwl,
                bhl,
                Some((above_mi.is_some(), left_mi.is_some())),
            )?;
        }

        // `set_offsets`' mode-info replication (`:990-996`). libvpx points
        // every covered grid cell at one `MODE_INFO` *before* the mode read,
        // so the cells see every later mutation; copying them here, after the
        // last one, is the same thing for a decoder that stores by value.
        for y in 0..y_mis {
            for x in 0..x_mis {
                self.mi.set(mi_row + y, mi_col + x, mi);
            }
        }

        // `vp9_read_mode_info`'s `MV_REF` save (`vp9_decodemv.c:836-846`),
        // inter frames only: an intra frame leaves the array as allocated.
        // Indexed with the frame's real `mi_cols`, which is what the next
        // frame's temporal scan and `Vp9RefSlot::mv_at` both assume.
        if self.inter.is_some() {
            let record = MvRefRow {
                ref_frame: mi.ref_frame,
                mv: mi.mv,
            };
            let cols = self.mi.cols;
            for y in 0..y_mis {
                for x in 0..x_mis {
                    self.mvs[(mi_row + y) * cols + mi_col + x] = record;
                }
            }
        }
        Ok(())
    }

    /// `read_intra_frame_mode_info` (`vp9_decodemv.c:192-233`), the key /
    /// intra-only frame path: segment id from the tree, skip, transform size,
    /// then the Y modes conditioned on the *neighbouring block modes* (which
    /// is what distinguishes it from the inter frame's intra block path in
    /// [`super::modeinfo`], where the Y mode uses `fc->y_mode_prob`).
    fn read_intra_frame_mode_info(
        &mut self,
        r: &mut BoolReader<'_>,
        above_mi: Option<&MiInfo>,
        left_mi: Option<&MiInfo>,
        bsize: u8,
    ) -> MiInfo {
        let seg = &self.hdr.seg;
        let segment_id: u8 = if seg.enabled && seg.update_map {
            r.read_tree(&tables::SEGMENT_TREE, &seg.tree_probs)
        } else {
            0
        };

        // read_skip
        let skip = if seg.enabled && seg.feature_enabled[segment_id as usize][3] {
            // SEG_LVL_SKIP forces the flag without coding a bit, so libvpx
            // counts nothing here (vp9_decodemv.c:179-189).
            true
        } else {
            let ctx = usize::from(above_mi.is_some_and(|m| m.skip))
                + usize::from(left_mi.is_some_and(|m| m.skip));
            let skip = r.read_bool(self.probs.skip[ctx]);
            if let Some(cnt) = self.counts.as_deref_mut() {
                cnt.skip[ctx][usize::from(skip)] += 1;
            }
            skip
        };

        // `read_tx_size(cm, xd, 1, r)` (`:207`) — the shared port, so the two
        // frame types cannot drift apart on the context derivation or the
        // counter indexing.
        let tx_size = read_tx_size(
            r,
            self.probs,
            self.counts.as_deref_mut(),
            above_mi,
            left_mi,
            bsize,
            self.tx_mode,
            true,
        );

        // Keyframe Y modes with above/left block-mode contexts.
        let above_block_mode = |cur: &MiInfo, b: usize| -> u8 {
            if b == 0 || b == 1 {
                match above_mi {
                    Some(m) => {
                        if m.sb_type < 3 {
                            m.bmi[b + 2]
                        } else {
                            m.mode
                        }
                    }
                    None => 0,
                }
            } else {
                cur.bmi[b - 2]
            }
        };
        let left_block_mode = |cur: &MiInfo, b: usize| -> u8 {
            if b == 0 || b == 2 {
                match left_mi {
                    Some(m) => {
                        if m.sb_type < 3 {
                            m.bmi[b + 1]
                        } else {
                            m.mode
                        }
                    }
                    None => 0,
                }
            } else {
                cur.bmi[b - 1]
            }
        };
        let mut mi = MiInfo {
            sb_type: bsize,
            skip,
            tx_size,
            segment_id,
            ..MiInfo::default()
        };
        let read_mode = |r: &mut BoolReader<'_>, cur: &MiInfo, b: usize| -> u8 {
            let a = above_block_mode(cur, b) as usize;
            let l = left_block_mode(cur, b) as usize;
            r.read_tree(&tables::INTRA_MODE_TREE, &tables::KF_Y_MODE_PROBS[a][l])
        };
        match bsize {
            0 => {
                // BLOCK_4X4
                for i in 0..4 {
                    mi.bmi[i] = read_mode(r, &mi, i);
                }
                mi.mode = mi.bmi[3];
            }
            1 => {
                // BLOCK_4X8
                let m0 = read_mode(r, &mi, 0);
                mi.bmi[0] = m0;
                mi.bmi[2] = m0;
                let m1 = read_mode(r, &mi, 1);
                mi.bmi[1] = m1;
                mi.bmi[3] = m1;
                mi.mode = m1;
            }
            2 => {
                // BLOCK_8X4
                let m0 = read_mode(r, &mi, 0);
                mi.bmi[0] = m0;
                mi.bmi[1] = m0;
                let m2 = read_mode(r, &mi, 2);
                mi.bmi[2] = m2;
                mi.bmi[3] = m2;
                mi.mode = m2;
            }
            _ => {
                mi.mode = read_mode(r, &mi, 0);
            }
        }
        mi.uv_mode = r.read_tree(
            &tables::INTRA_MODE_TREE,
            &tables::KF_UV_MODE_PROBS[mi.mode as usize],
        );
        mi
    }

    /// The per-plane, per-transform-block loop both reconstruction paths run
    /// (`vp9_decodeframe.c:1040-1062` for intra, `:1069-1081` for inter):
    /// identical geometry, different body.
    ///
    /// `intra_avail` is `Some((has_above_mi, has_left_mi))` for the intra
    /// path — the neighbour availability `vp9_predict_intra_block` needs —
    /// and `None` for the inter path, which predicts from the reference frame
    /// and has no such notion. Returns `eobtotal`, which only the inter path
    /// reads.
    fn residual_loop(
        &mut self,
        r: &mut BoolReader<'_>,
        mi: &MiInfo,
        tile: TileBounds,
        mi_row: usize,
        mi_col: usize,
        bsize: u8,
        bwl: usize,
        bhl: usize,
        intra_avail: Option<(bool, bool)>,
    ) -> CodecResult<usize> {
        let bw = 1usize << (bwl - 1);
        let bh = 1usize << (bhl - 1);
        let mb_to_right_edge = ((self.mi.cols as i64) - (bw as i64) - (mi_col as i64)) * 64;
        let mb_to_bottom_edge = ((self.mi.rows as i64) - (bh as i64) - (mi_row as i64)) * 64;
        let mut eobtotal = 0usize;
        for plane in 0..3 {
            let (px, py) = self.plane_ss(plane);
            let n4_w = (bw << 1) >> px;
            let n4_h = (bh << 1) >> py;
            let n4_wl = bwl - px;
            let tx = if plane == 0 {
                mi.tx_size
            } else {
                tables::UV_TXSIZE_LOOKUP[bsize as usize][mi.tx_size as usize][px][py]
            };
            let step = 1usize << tx;
            let max_blocks_wide = if mb_to_right_edge >= 0 {
                n4_w
            } else {
                (n4_w as i64 + (mb_to_right_edge >> (5 + px as i64))) as usize
            };
            let max_blocks_high = if mb_to_bottom_edge >= 0 {
                n4_h
            } else {
                (n4_h as i64 + (mb_to_bottom_edge >> (5 + py as i64))) as usize
            };
            // xd->max_blocks_wide is 0 unless the block crosses the edge.
            let limit_w = if mb_to_right_edge >= 0 {
                0
            } else {
                max_blocks_wide
            };
            let limit_h = if mb_to_bottom_edge >= 0 {
                0
            } else {
                max_blocks_high
            };

            let mut row = 0usize;
            while row < max_blocks_high {
                let mut col = 0usize;
                while col < max_blocks_wide {
                    match intra_avail {
                        Some((has_above_mi, has_left_mi)) => self.predict_and_reconstruct(
                            r,
                            mi,
                            tile,
                            plane,
                            mi_row,
                            mi_col,
                            row,
                            col,
                            tx,
                            n4_wl,
                            limit_w,
                            limit_h,
                            mb_to_right_edge < 0,
                            mb_to_bottom_edge < 0,
                            has_above_mi,
                            has_left_mi,
                        )?,
                        None => {
                            eobtotal += self.reconstruct_inter_block(
                                r, mi, plane, mi_row, mi_col, row, col, tx, limit_w, limit_h,
                            );
                        }
                    }
                    col += step;
                }
                row += step;
            }
        }
        Ok(eobtotal)
    }

    /// `reconstruct_inter_block` (`vp9_decodeframe.c:1010-1021`).
    ///
    /// Two differences from the intra body, both mandatory:
    /// the transform type is always `DCT_DCT` (so the scan order is always
    /// `vp9_default_scan_orders[tx_size]`, never the row/column scans an
    /// intra mode selects), and the coefficient probabilities and counters
    /// are indexed with `ref = 1`.
    fn reconstruct_inter_block(
        &mut self,
        r: &mut BoolReader<'_>,
        mi: &MiInfo,
        plane: usize,
        mi_row: usize,
        mi_col: usize,
        row: usize,
        col: usize,
        tx: u8,
        limit_w: usize,
        limit_h: usize,
    ) -> usize {
        let (scan_tbl, nb_tbl) = select_scan(tx, TxKind::DctDct);
        let eob = self.decode_block_tokens(
            r,
            plane,
            mi.segment_id as usize,
            true,
            mi_row,
            mi_col,
            col,
            row,
            tx,
            scan_tbl,
            nb_tbl,
            limit_w,
            limit_h,
        );
        if eob > 0 {
            let (px, py) = self.plane_ss(plane);
            let x0 = ((mi_col * 8) >> px) + 4 * col;
            let y0 = ((mi_row * 8) >> py) + 4 * row;
            let plane_buf = &mut self.planes[plane];
            let off = y0 * plane_buf.stride + x0;
            inverse_transform_add(
                tx as usize,
                TxKind::DctDct,
                self.lossless,
                &self.dqcoeff,
                &mut plane_buf.data,
                off,
                plane_buf.stride,
            );
            let n = (4usize << tx) * (4usize << tx);
            self.dqcoeff[..n].fill(0);
        }
        eob
    }

    fn plane_ss(&self, plane: usize) -> (usize, usize) {
        if plane == 0 {
            (0, 0)
        } else {
            (self.ss_x, self.ss_y)
        }
    }

    fn mi_cols_al(&self) -> usize {
        (self.mi.cols + 7) & !7
    }

    /// `predict_and_reconstruct_intra_block`.
    fn predict_and_reconstruct(
        &mut self,
        r: &mut BoolReader<'_>,
        mi: &MiInfo,
        _tile: TileBounds,
        plane: usize,
        mi_row: usize,
        mi_col: usize,
        row: usize,
        col: usize,
        tx: u8,
        n4_wl: usize,
        limit_w: usize,
        limit_h: usize,
        edge_slow_x: bool,
        edge_slow_y: bool,
        has_above_mi: bool,
        has_left_mi: bool,
    ) -> CodecResult<()> {
        let (px, py) = self.plane_ss(plane);
        let mut mode_idx = if plane == 0 { mi.mode } else { mi.uv_mode };
        if mi.sb_type < 3 && plane == 0 {
            mode_idx = mi.bmi[(row << 1) + col];
        }
        let mode = PredMode::from_index(mode_idx);

        let bs = 4usize << tx;
        let x0 = ((mi_col * 8) >> px) + 4 * col;
        let y0 = ((mi_row * 8) >> py) + 4 * row;

        // vp9_predict_intra_block availability.
        let have_top = row > 0 || has_above_mi;
        let have_left = col > 0 || has_left_mi;
        let txw = 1usize << tx;
        let have_right = (col + txw) < (1usize << n4_wl);

        {
            let plane_buf = &mut self.planes[plane];
            predict_intra(
                &mut plane_buf.data,
                plane_buf.stride,
                x0,
                y0,
                bs,
                mode,
                have_top,
                have_left,
                have_right,
                edge_slow_x,
                edge_slow_y,
                plane_buf.width,
                plane_buf.height,
            );
        }

        if !mi.skip {
            let tx_type = if plane > 0 || self.lossless {
                TxKind::DctDct
            } else {
                mode_to_tx_type(mode_idx)
            };
            let (scan_tbl, nb_tbl) = select_scan(tx, tx_type);
            let eob = self.decode_block_tokens(
                r,
                plane,
                mi.segment_id as usize,
                false,
                mi_row,
                mi_col,
                col,
                row,
                tx,
                scan_tbl,
                nb_tbl,
                limit_w,
                limit_h,
            );
            if eob > 0 {
                let plane_buf = &mut self.planes[plane];
                let off = y0 * plane_buf.stride + x0;
                inverse_transform_add(
                    tx as usize,
                    tx_type,
                    self.lossless,
                    &self.dqcoeff,
                    &mut plane_buf.data,
                    off,
                    plane_buf.stride,
                );
                // Clear the used coefficients (libvpx zeroes eob-dependent
                // spans; clearing the whole tx block is equivalent).
                let n = (4usize << tx) * (4usize << tx);
                self.dqcoeff[..n].fill(0);
            }
        }
        Ok(())
    }

    /// `vp9_decode_block_tokens`.
    ///
    /// `is_inter` is libvpx's `ref` argument — `is_inter_block(xd->mi[0])`,
    /// which selects the `[tx][plane_type][ref]` half of both the coefficient
    /// probabilities and their counters.
    fn decode_block_tokens(
        &mut self,
        r: &mut BoolReader<'_>,
        plane: usize,
        seg_id: usize,
        is_inter: bool,
        mi_row: usize,
        mi_col: usize,
        x: usize,
        y: usize,
        tx: u8,
        scan_tbl: &[i16],
        nb_tbl: &[i16],
        limit_w: usize,
        limit_h: usize,
    ) -> usize {
        let (px, py) = self.plane_ss(plane);
        let a0 = ((mi_col * 2) >> px) + x;
        let l0 = (((mi_row * 2) & 15) >> py) + y;
        let n = 1usize << tx; // tx size in 4x4 units

        let ctx = {
            let a_any = self.above_ctx[plane][a0..a0 + n].iter().any(|&v| v != 0);
            let l_any = self.left_ctx[plane][l0..l0 + n].iter().any(|&v| v != 0);
            usize::from(a_any) + usize::from(l_any)
        };

        let dequant = if plane == 0 {
            self.dequant.y[seg_id]
        } else {
            self.dequant.uv[seg_id]
        };
        let plane_type = usize::from(plane > 0);

        let eob = decode_coefs(
            r,
            &*self.probs,
            plane_type,
            usize::from(is_inter),
            tx as usize,
            dequant,
            ctx,
            scan_tbl,
            nb_tbl,
            &mut self.dqcoeff,
            self.counts.as_deref_mut(),
        );

        // Context update with edge truncation (get_ctx_shift semantics: the
        // `((eob > 0) * 0x0101...) >> ctx_shift` little-endian store sets the
        // first `limit - x` bytes and zeroes the rest).
        let a_valid = if limit_w != 0 && n + x > limit_w {
            limit_w - x
        } else {
            n
        };
        let l_valid = if limit_h != 0 && n + y > limit_h {
            limit_h - y
        } else {
            n
        };
        let v = u8::from(eob > 0);
        for i in 0..n {
            self.above_ctx[plane][a0 + i] = if i < a_valid { v } else { 0 };
        }
        for i in 0..n {
            self.left_ctx[plane][l0 + i] = if i < l_valid { v } else { 0 };
        }
        eob
    }
}

/// `intra_mode_to_tx_type_lookup` (vp9_reconintra.c).
fn mode_to_tx_type(mode: u8) -> TxKind {
    match mode {
        1 | 5 | 8 => TxKind::AdstDct, // V, D117, D63
        2 | 6 | 7 => TxKind::DctAdst, // H, D153, D207
        4 | 9 => TxKind::AdstAdst,    // D135, TM
        _ => TxKind::DctDct,          // DC, D45
    }
}

/// `vp9_scan_orders[tx_size][tx_type]` (scan, neighbors).
fn select_scan(tx: u8, kind: TxKind) -> (&'static [i16], &'static [i16]) {
    match (tx, kind) {
        (0, TxKind::AdstDct) => (&scan::ROW_SCAN_4X4, &scan::ROW_SCAN_4X4_NB),
        (0, TxKind::DctAdst) => (&scan::COL_SCAN_4X4, &scan::COL_SCAN_4X4_NB),
        (0, _) => (&scan::DEFAULT_SCAN_4X4, &scan::DEFAULT_SCAN_4X4_NB),
        (1, TxKind::AdstDct) => (&scan::ROW_SCAN_8X8, &scan::ROW_SCAN_8X8_NB),
        (1, TxKind::DctAdst) => (&scan::COL_SCAN_8X8, &scan::COL_SCAN_8X8_NB),
        (1, _) => (&scan::DEFAULT_SCAN_8X8, &scan::DEFAULT_SCAN_8X8_NB),
        (2, TxKind::AdstDct) => (&scan::ROW_SCAN_16X16, &scan::ROW_SCAN_16X16_NB),
        (2, TxKind::DctAdst) => (&scan::COL_SCAN_16X16, &scan::COL_SCAN_16X16_NB),
        (2, _) => (&scan::DEFAULT_SCAN_16X16, &scan::DEFAULT_SCAN_16X16_NB),
        _ => (&scan::DEFAULT_SCAN_32X32, &scan::DEFAULT_SCAN_32X32_NB),
    }
}

/// `get_coef_context` (vp9_scan.h).
#[inline]
fn coef_context(nb: &[i16], token_cache: &[u8; 1024], c: usize) -> usize {
    ((1 + u32::from(token_cache[nb[2 * c] as usize])
        + u32::from(token_cache[nb[2 * c + 1] as usize]))
        >> 1) as usize
}

/// `decode_coefs` (vp9_detokenize.c:113-259), 8-bit.
///
/// `ref_idx` is libvpx's `ref` — `is_inter_block(xd->mi[0])`, `0` for an
/// intra block and `1` for an inter one. It selects the same half of the
/// probability array and of every counter, and the two must agree: reading
/// against the intra probabilities while counting into the inter bucket
/// would decode correctly this frame and corrupt the next one that loads the
/// adapted context.
///
/// # Counting
///
/// `counts` mirrors libvpx's `xd->counts`: `None` disables every counter,
/// exactly as libvpx's `if (counts)` guards do for a frame-parallel frame.
/// The increment points are libvpx's, in libvpx's order, and are the subtle
/// part of this function:
///
/// * `eob_branch[band][ctx]` is incremented **before** the EOB read
///   (`vp9_detokenize.c:163`), so it counts how often that node was *asked*,
///   not how often it terminated the block. `adapt_coef_probs` needs both
///   halves — `neob` and `eob_branch - neob` — and only this ordering can
///   supply the denominator.
/// * `EOB_MODEL_TOKEN` is counted when the EOB read breaks the loop
///   (`:165`), against the same `band`/`ctx` the branch was read with.
/// * `ZERO_TOKEN` is counted inside the zero run (`:170`) **before** `c`
///   advances, so it lands on the position that coded the zero — and the
///   run's later iterations re-derive `band`/`ctx` for the next position,
///   which is what makes each zero land in a different bucket.
/// * `TWO_TOKEN` is counted on entering the `ONE_CONTEXT_NODE`-set branch
///   (`:187`), i.e. for *every* token of magnitude two or more, before the
///   Pareto tail is read — it is a model-token count, not a literal "value
///   was 2" count.
/// * `ONE_TOKEN` is counted in the else branch (`:233`).
///
/// The two loop exits that count nothing are equally load-bearing: a zero run
/// that reaches `max_eob` returns immediately (`:174-178`, "zero tokens at the
/// end (no eob token)"), and an outer loop that reaches `max_eob` after a
/// coded token simply falls out — neither reads an EOB branch, so neither may
/// count one.
fn decode_coefs(
    r: &mut BoolReader<'_>,
    probs: &FrameProbs,
    plane_type: usize,
    ref_idx: usize,
    tx: usize,
    dq: [i64; 2],
    mut ctx: usize,
    scan_tbl: &[i16],
    nb_tbl: &[i16],
    dqcoeff: &mut [i64],
    mut counts: Option<&mut FrameCounts>,
) -> usize {
    let max_eob = 16usize << (tx << 1);
    let coef_probs = &probs.coef[tx][plane_type][ref_idx];
    let band_translate: &[u8] = if tx == 0 {
        &tables::COEFBAND_TRANS_4X4
    } else {
        &tables::COEFBAND_TRANS_8X8PLUS
    };
    let dq_shift = i64::from(tx == 3);
    let mut token_cache = [0u8; 1024];
    let mut dqv = dq[0];
    let mut c = 0usize;

    while c < max_eob {
        let mut band = band_translate[c] as usize;
        let mut prob = &coef_probs[band][ctx];

        // `if (counts) ++eob_branch_count[band][ctx];` -- before the read.
        if let Some(cnt) = counts.as_deref_mut() {
            cnt.eob_branch[tx][plane_type][ref_idx][band][ctx] += 1;
        }

        // EOB_CONTEXT_NODE
        if !r.read_bool(prob[0]) {
            if let Some(cnt) = counts.as_deref_mut() {
                cnt.coef[tx][plane_type][ref_idx][band][ctx][EOB_MODEL_TOKEN] += 1;
            }
            break;
        }

        // ZERO_CONTEXT_NODE run
        while !r.read_bool(prob[1]) {
            if let Some(cnt) = counts.as_deref_mut() {
                cnt.coef[tx][plane_type][ref_idx][band][ctx][ZERO_TOKEN] += 1;
            }
            dqv = dq[1];
            token_cache[scan_tbl[c] as usize] = 0;
            c += 1;
            if c >= max_eob {
                return c; // zero tokens at the end (no eob token)
            }
            ctx = coef_context(nb_tbl, &token_cache, c);
            band = band_translate[c] as usize;
            prob = &coef_probs[band][ctx];
        }

        // ONE_CONTEXT_NODE
        let v: i64;
        if r.read_bool(prob[2]) {
            if let Some(cnt) = counts.as_deref_mut() {
                cnt.coef[tx][plane_type][ref_idx][band][ctx][TWO_TOKEN] += 1;
            }
            // Probabilities are always >= 1 on valid streams (defaults and
            // inv_remap_prob both guarantee it); max(1) guards the index.
            let p = &tables::PARETO8_FULL[usize::from(prob[2].max(1)) - 1];
            if r.read_bool(p[0]) {
                if r.read_bool(p[3]) {
                    token_cache[scan_tbl[c] as usize] = 5;
                    let val: i64 = if r.read_bool(p[5]) {
                        if r.read_bool(p[7]) {
                            67 + read_coeff(r, &tables::CAT6_PROB)
                        } else {
                            35 + read_coeff(r, &tables::CAT5_PROB)
                        }
                    } else if r.read_bool(p[6]) {
                        19 + read_coeff(r, &tables::CAT4_PROB)
                    } else {
                        11 + read_coeff(r, &tables::CAT3_PROB)
                    };
                    v = (val * dqv) >> dq_shift;
                } else {
                    token_cache[scan_tbl[c] as usize] = 4;
                    let val: i64 = if r.read_bool(p[4]) {
                        7 + read_coeff(r, &tables::CAT2_PROB)
                    } else {
                        5 + read_coeff(r, &tables::CAT1_PROB)
                    };
                    v = (val * dqv) >> dq_shift;
                }
            } else if r.read_bool(p[1]) {
                token_cache[scan_tbl[c] as usize] = 3;
                v = ((3 + i64::from(r.read_bool(p[2]))) * dqv) >> dq_shift;
            } else {
                token_cache[scan_tbl[c] as usize] = 2;
                v = (2 * dqv) >> dq_shift;
            }
        } else {
            if let Some(cnt) = counts.as_deref_mut() {
                cnt.coef[tx][plane_type][ref_idx][band][ctx][ONE_TOKEN] += 1;
            }
            token_cache[scan_tbl[c] as usize] = 1;
            v = dqv >> dq_shift;
        }

        // Sign; store with libvpx's (tran_low_t) int16 truncation.
        let signed = if r.read_bool(128) { -v } else { v };
        dqcoeff[scan_tbl[c] as usize] = i64::from(signed as i16);
        c += 1;
        ctx = coef_context(nb_tbl, &token_cache, c);
        dqv = dq[1];
    }

    c
}

/// `read_coeff` (extra-bit categories, MSB first).
fn read_coeff(r: &mut BoolReader<'_>, cat_probs: &[u8]) -> i64 {
    let mut val: i64 = 0;
    for &p in cat_probs {
        val = (val << 1) | i64::from(r.read_bool(p));
    }
    val
}

#[cfg(test)]
mod tests;
