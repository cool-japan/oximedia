//! AV1 intra-frame reconstruction driver.
//!
//! Exact port of the tile-group decode path of the AV1 spec for intra
//! frames: `decode_tile` (5.11.2), `decode_partition` (5.11.4),
//! `decode_block` (5.11.5), `intra_frame_mode_info` (5.11.10) and the
//! segment/skip/cdef/delta syntax it invokes, `read_tx_size` (5.11.16),
//! `residual`/`transform_block` (5.11.34/35), and the `reconstruct`
//! process (7.12.3) on top of [`super::coef`], [`super::pred`], and
//! [`super::itx`].

#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]

use super::cdfs::CdfCtx;
use super::coef::{self, CoefBlock, LevelCtxs, FILTER_INTRA_MODE_TO_INTRA_DIR};
use super::consts::{
    BLOCK_128X128, BLOCK_4X4, BLOCK_64X64, BLOCK_8X8, BLOCK_INVALID, DC_PRED, DELTA_LF_SMALL,
    DELTA_Q_SMALL, FRAME_LF_COUNT, MAX_ANGLE_DELTA, MAX_LOOP_FILTER, PARTITION_HORZ,
    PARTITION_HORZ_4, PARTITION_HORZ_A, PARTITION_HORZ_B, PARTITION_NONE, PARTITION_SPLIT,
    PARTITION_VERT, PARTITION_VERT_4, PARTITION_VERT_A, PARTITION_VERT_B, SEG_LVL_ALT_Q,
    SEG_LVL_SKIP, TX_16X32, TX_16X64, TX_32X16, TX_32X32, TX_32X64, TX_4X4, TX_64X16, TX_64X32,
    TX_64X64, UV_CFL_PRED, V_PRED,
};
use super::hdr::{FrameHdr, SeqHdr};
use super::itx::inverse_transform_2d;
use super::msac::Msac;
use super::pred::{is_directional_mode, predict_cfl, predict_intra, PredParams};
use super::tables_cdf_mode as tm;
use super::tables_conv::{
    AC_QLOOKUP, DC_QLOOKUP, INTRA_MODE_CONTEXT, MAX_TX_DEPTH, MAX_TX_SIZE_RECT, MI_HEIGHT_LOG2,
    MI_WIDTH_LOG2, NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE, PARTITION_SUBSIZE, SIZE_GROUP,
    SPLIT_TX_SIZE, SUBSAMPLED_SIZE, TX_HEIGHT, TX_WIDTH,
};
use crate::error::{CodecError, CodecResult};

/// One reconstruction plane with MI-aligned dimensions.
pub struct PlaneBuf {
    /// Pixel data, `stride * height` bytes.
    pub data: Vec<u8>,
    /// Row stride (== aligned width).
    pub stride: usize,
    /// Aligned width in pixels.
    pub width: usize,
    /// Aligned height in pixels.
    pub height: usize,
}

/// Decoded intra frame: planes + display dimensions.
pub struct DecodedIntraFrame {
    /// Y, U, V planes (MI-aligned).
    pub planes: [PlaneBuf; 3],
    /// Display width.
    pub width: usize,
    /// Display height.
    pub height: usize,
}

// --------------------------------------------------- filter-stage row bands
//
// Reconstruction itself is strictly sequential: intra prediction reads the
// already-reconstructed samples above and to the left of the current block,
// so neither rows nor blocks may be reordered. The two post-reconstruction
// stages that follow it are not: CDEF (spec 7.15) and loop restoration
// (spec 7.17) both read an immutable input frame (`CurrFrame` /
// `UpscaledCdefFrame` plus `UpscaledCurrFrame`) and write a *separate*
// output frame, and every output sample is produced by exactly one source
// block. Splitting the output into horizontal bands therefore reproduces
// the serial result byte for byte, whatever the band count and whatever
// order the bands are evaluated in. The deblocking loop filter (spec 7.14)
// has no such property — it filters in place and its horizontal-edge pass
// is only column-independent — so it stays serial.

/// A horizontal band of a three-plane output frame: one mutable row window
/// per plane plus the absolute plane row each window starts at.
///
/// [`OutBand::put`] lets the filter stages keep addressing samples in
/// absolute plane coordinates, exactly as the serial code does.
pub struct OutBand<'a> {
    rows: [&'a mut [u8]; 3],
    row0: [usize; 3],
    stride: [usize; 3],
}

impl OutBand<'_> {
    /// Stores one output sample at absolute plane coordinates `(x, y)`.
    #[inline]
    pub fn put(&mut self, plane: usize, y: usize, x: usize, v: u8) {
        self.rows[plane][(y - self.row0[plane]) * self.stride[plane] + x] = v;
    }
}

/// Splits three output planes into the contiguous luma-row ranges `bounds`.
///
/// `bounds` must be contiguous, start at 0, end at the luma plane height and
/// have every boundary divisible by `1 << sub_y`, so the chroma row counts
/// divide exactly. Returns one [`OutBand`] per range, in `bounds` order.
pub fn split_out_bands<'a>(
    out: &'a mut [Vec<u8>; 3],
    strides: [usize; 3],
    sub_y: usize,
    bounds: &[(usize, usize)],
) -> Vec<OutBand<'a>> {
    let [o0, o1, o2] = out;
    let mut rest: [&mut [u8]; 3] = [o0.as_mut_slice(), o1.as_mut_slice(), o2.as_mut_slice()];
    let mut next_row = [0usize; 3];
    let mut bands = Vec::with_capacity(bounds.len());
    for &(y0, y1) in bounds {
        debug_assert_eq!(y0, next_row[0], "bands must be contiguous from row 0");
        let mut rows: [&mut [u8]; 3] = [&mut [], &mut [], &mut []];
        let row0 = next_row;
        for plane in 0..3 {
            let shift = if plane == 0 { 0 } else { sub_y };
            let n_rows = (y1 >> shift) - (y0 >> shift);
            let take = n_rows * strides[plane];
            let all = core::mem::take(&mut rest[plane]);
            debug_assert!(take <= all.len(), "band exceeds the output plane");
            let (head, tail) = all.split_at_mut(core::cmp::min(take, all.len()));
            rest[plane] = tail;
            rows[plane] = head;
            next_row[plane] += n_rows;
        }
        bands.push(OutBand {
            rows,
            row0,
            stride: strides,
        });
    }
    bands
}

/// Splits `total` rows into `count` contiguous bands whose boundaries are
/// multiples of `align`. `total` must be a multiple of `align`; `count` is
/// clamped to `1..=total / align`.
///
/// Every returned band is non-empty, so `total == 0` yields no bands at all.
pub fn plan_bands(total: usize, align: usize, count: usize) -> Vec<(usize, usize)> {
    let align = align.max(1);
    debug_assert_eq!(total % align, 0, "band alignment must divide the height");
    let units = total / align;
    if units == 0 {
        return Vec::new();
    }
    let count = count.clamp(1, units);
    let mut bounds = Vec::with_capacity(count);
    let mut start = 0usize;
    for k in 0..count {
        let end = core::cmp::min(units * (k + 1) / count * align, total);
        bounds.push((start, end));
        start = end;
    }
    if let Some(last) = bounds.last_mut() {
        last.1 = total;
    }
    bounds
}

/// Band count for a frame-sized filter stage: 1 (fully serial, no rayon
/// involvement at all) for small frames and single-threaded pools, else the
/// work divided by `min_pixels_per_band`, capped at the global pool size.
///
/// The count only decides how the work is scheduled — the filtered output is
/// identical for every value, which the `filter_stage_bands_bit_exact` test
/// checks against the reference decodes.
pub fn auto_band_count(luma_pixels: usize, min_pixels_per_band: usize) -> usize {
    let threads = rayon::current_num_threads();
    if threads <= 1 {
        return 1;
    }
    (luma_pixels / min_pixels_per_band.max(1)).clamp(1, threads)
}

#[inline]
fn block_width(bsize: usize) -> usize {
    usize::from(NUM_4X4_BLOCKS_WIDE[bsize]) * 4
}
#[inline]
fn block_height(bsize: usize) -> usize {
    usize::from(NUM_4X4_BLOCKS_HIGH[bsize]) * 4
}

/// Frame-wide per-MI grids (all `mi_rows * mi_cols`).
struct Grids {
    mi_rows: usize,
    mi_cols: usize,
    y_modes: Vec<u8>,
    uv_modes: Vec<u8>,
    mi_sizes: Vec<u8>,
    skips: Vec<u8>,
    inter_tx_sizes: Vec<u8>,
    tx_sizes: Vec<u8>,
    seg_ids: Vec<u8>,
    tx_types: Vec<u8>,
    /// `LoopfilterTxSizes[plane]` at plane-subsampled MI resolution.
    lf_tx_sizes: [Vec<u8>; 3],
    /// `DeltaLFs` per MI (4 components).
    delta_lfs: Vec<[i8; FRAME_LF_COUNT]>,
    /// `cdef_idx` per MI (-1 = unset).
    cdef_idx: Vec<i16>,
}

impl Grids {
    fn new(mi_rows: usize, mi_cols: usize) -> Self {
        let n = mi_rows * mi_cols;
        Self {
            mi_rows,
            mi_cols,
            y_modes: vec![0; n],
            uv_modes: vec![0; n],
            mi_sizes: vec![0; n],
            skips: vec![0; n],
            inter_tx_sizes: vec![0; n],
            tx_sizes: vec![0; n],
            seg_ids: vec![0; n],
            tx_types: vec![0; n],
            lf_tx_sizes: [vec![0; n], vec![0; n], vec![0; n]],
            delta_lfs: vec![[0; FRAME_LF_COUNT]; n],
            cdef_idx: vec![-1; n],
        }
    }
    #[inline]
    fn at(&self, r: usize, c: usize) -> usize {
        r * self.mi_cols + c
    }
}

/// Current-block state assembled by `intra_frame_mode_info`.
#[derive(Clone, Copy, Default)]
struct Block {
    mi_row: usize,
    mi_col: usize,
    mi_size: usize,
    has_chroma: bool,
    avail_u: bool,
    avail_l: bool,
    avail_u_chroma: bool,
    avail_l_chroma: bool,
    y_mode: usize,
    uv_mode: usize,
    angle_delta_y: i32,
    angle_delta_uv: i32,
    cfl_alpha_u: i32,
    cfl_alpha_v: i32,
    use_filter_intra: bool,
    filter_intra_mode: usize,
    skip: bool,
    segment_id: usize,
    lossless: bool,
    tx_size: usize,
    /// CFL luma coverage (plane-0 pixel coordinates).
    max_luma_w: usize,
    max_luma_h: usize,
}

/// The frame decoder.
struct Dec<'a> {
    seq: &'a SeqHdr,
    hdr: &'a FrameHdr,
    planes: [PlaneBuf; 3],
    grids: Grids,
    // Tile state
    msac: Msac<'a>,
    cdfs: CdfCtx,
    lc: LevelCtxs,
    mi_row_start: usize,
    mi_row_end: usize,
    mi_col_start: usize,
    mi_col_end: usize,
    /// `BlockDecoded[plane]` for the current superblock, with a one-entry
    /// border on all sides (offset +1).
    block_decoded: [Vec<u8>; 3],
    bd_stride: [usize; 3],
    current_q_index: u32,
    delta_lf: [i32; FRAME_LF_COUNT],
    read_deltas: bool,
    sub_x: bool,
    sub_y: bool,
    num_planes: usize,
    sb_size: usize,
    sb_size4: usize,
    lr_units: super::lr::LrUnitGrids,
    lr_refs: super::lr::LrRefs,
}

impl Dec<'_> {
    #[inline]
    fn mi_rows(&self) -> usize {
        self.grids.mi_rows
    }
    #[inline]
    fn mi_cols(&self) -> usize {
        self.grids.mi_cols
    }

    /// `is_inside` (spec 5.11.51): tile-relative availability.
    #[inline]
    fn is_inside(&self, r: isize, c: isize) -> bool {
        c >= self.mi_col_start as isize
            && c < self.mi_col_end as isize
            && r >= self.mi_row_start as isize
            && r < self.mi_row_end as isize
    }

    /// `get_qindex(0, segment_id)` at block-decode time (spec 7.12.2).
    fn get_qindex(&self, segment_id: usize) -> u32 {
        let seg = &self.hdr.seg;
        if seg.enabled && seg.feature_enabled[segment_id][SEG_LVL_ALT_Q] {
            let data = i64::from(seg.feature_data[segment_id][SEG_LVL_ALT_Q]);
            let qindex = if self.hdr.delta_q_present {
                i64::from(self.current_q_index) + data
            } else {
                i64::from(self.hdr.base_q_idx) + data
            };
            qindex.clamp(0, 255) as u32
        } else if self.hdr.delta_q_present {
            self.current_q_index
        } else {
            self.hdr.base_q_idx
        }
    }

    /// `get_dc_quant`/`get_ac_quant` (spec 7.12.2), 8-bit lookups.
    fn dc_quant(&self, plane: usize, segment_id: usize) -> i32 {
        let delta = match plane {
            0 => self.hdr.delta_q_y_dc,
            1 => self.hdr.delta_q_u_dc,
            _ => self.hdr.delta_q_v_dc,
        };
        let b = (i64::from(self.get_qindex(segment_id)) + i64::from(delta)).clamp(0, 255);
        i32::from(DC_QLOOKUP[0][b as usize])
    }
    fn ac_quant(&self, plane: usize, segment_id: usize) -> i32 {
        let delta = match plane {
            0 => 0,
            1 => self.hdr.delta_q_u_ac,
            _ => self.hdr.delta_q_v_ac,
        };
        let b = (i64::from(self.get_qindex(segment_id)) + i64::from(delta)).clamp(0, 255);
        i32::from(AC_QLOOKUP[0][b as usize])
    }

    // ---------------------------------------------------------------- tile

    /// `decode_tile()` (spec 5.11.2) for one tile.
    fn decode_tile(&mut self) -> CodecResult<()> {
        self.lc.clear_above();
        self.delta_lf = [0; FRAME_LF_COUNT];
        // RefSgrXqd / RefLrWiener per-tile init (spec 5.11.2).
        self.lr_refs = super::lr::LrRefs::new();
        let sb_size4 = self.sb_size4;
        let mut r = self.mi_row_start;
        while r < self.mi_row_end {
            self.lc.clear_left();
            let mut c = self.mi_col_start;
            while c < self.mi_col_end {
                self.read_deltas = self.hdr.delta_q_present;
                self.clear_cdef(r, c);
                self.clear_block_decoded_flags(r, c);
                self.read_lr(r, c);
                self.decode_partition(r, c, self.sb_size)?;
                c += sb_size4;
            }
            r += sb_size4;
        }
        self.msac.check_exit()
    }

    /// `read_lr` (spec 5.11.57): reads loop restoration unit parameters for
    /// the units whose top-left falls inside this superblock.
    fn read_lr(&mut self, r: usize, c: usize) {
        if self.hdr.allow_intrabc || !self.hdr.lr.uses_lr {
            return;
        }
        let w = self.sb_size4;
        let h = self.sb_size4;
        for plane in 0..self.num_planes {
            let frt = self.hdr.lr.frame_restoration_type[plane];
            if frt == super::consts::RESTORE_NONE {
                continue;
            }
            let (sub_x, sub_y) = self.plane_subsampling(plane);
            let sxu = usize::from(sub_x);
            let syu = usize::from(sub_y);
            let unit_size = self.hdr.lr.loop_restoration_size[plane] as usize;
            let unit_rows = self.lr_units.unit_rows[plane];
            let unit_cols = self.lr_units.unit_cols[plane];
            let unit_row_start = (r * (4 >> syu) + unit_size - 1) / unit_size;
            let unit_row_end = core::cmp::min(
                unit_rows,
                ((r + h) * (4 >> syu) + unit_size - 1) / unit_size,
            );
            // use_superres == 0 in this decoder (gated earlier):
            let numerator = 4 >> sxu;
            let denominator = unit_size;
            let unit_col_start = (c * numerator + denominator - 1) / denominator;
            let unit_col_end = core::cmp::min(
                unit_cols,
                ((c + w) * numerator + denominator - 1) / denominator,
            );
            for unit_row in unit_row_start..unit_row_end {
                for unit_col in unit_col_start..unit_col_end {
                    super::lr::read_lr_unit(
                        &mut self.msac,
                        &mut self.cdfs,
                        &mut self.lr_refs,
                        &mut self.lr_units,
                        frt,
                        plane,
                        unit_row,
                        unit_col,
                    );
                }
            }
        }
    }

    /// `clear_cdef` (spec 5.11.55).
    fn clear_cdef(&mut self, r: usize, c: usize) {
        let idx = self.grids.at(r, c);
        self.grids.cdef_idx[idx] = -1;
        if self.seq.use_128x128_superblock {
            let cdef_size4 = usize::from(NUM_4X4_BLOCKS_WIDE[BLOCK_64X64]);
            for (dr, dc) in [(0, cdef_size4), (cdef_size4, 0), (cdef_size4, cdef_size4)] {
                if r + dr < self.mi_rows() && c + dc < self.mi_cols() {
                    let idx = self.grids.at(r + dr, c + dc);
                    self.grids.cdef_idx[idx] = -1;
                }
            }
        }
    }

    /// `clear_block_decoded_flags` (spec 5.11.3).
    fn clear_block_decoded_flags(&mut self, r: usize, c: usize) {
        let sb_size4 = self.sb_size4;
        for plane in 0..self.num_planes {
            let (sub_x, sub_y) = self.plane_subsampling(plane);
            let sb_width4 = (self.mi_col_end - c) >> usize::from(sub_x);
            let sb_height4 = (self.mi_row_end - r) >> usize::from(sub_y);
            let sz4 = sb_size4 >> usize::from(if plane == 0 { false } else { sub_y });
            let sx4 = sb_size4 >> usize::from(if plane == 0 { false } else { sub_x });
            let stride = self.bd_stride[plane];
            // y in -1..=(sbSize4 >> subY), x in -1..=(sbSize4 >> subX).
            for y in 0..=(sz4 + 1) {
                for x in 0..=(sx4 + 1) {
                    let iy = y as isize - 1;
                    let ix = x as isize - 1;
                    let v = if iy < 0 && ix < sb_width4 as isize {
                        1
                    } else if ix < 0 && iy < sb_height4 as isize {
                        1
                    } else {
                        0
                    };
                    self.block_decoded[plane][y * stride + x] = v;
                }
            }
            // BlockDecoded[plane][sbSize4 >> subY][-1] = 0
            self.block_decoded[plane][(sz4 + 1) * stride] = 0;
        }
    }

    #[inline]
    fn plane_subsampling(&self, plane: usize) -> (bool, bool) {
        if plane == 0 {
            (false, false)
        } else {
            (self.sub_x, self.sub_y)
        }
    }

    #[inline]
    fn bd_get(&self, plane: usize, y: isize, x: isize) -> bool {
        let stride = self.bd_stride[plane];
        let iy = (y + 1) as usize;
        let ix = (x + 1) as usize;
        self.block_decoded[plane][iy * stride + ix] != 0
    }
    #[inline]
    fn bd_set(&mut self, plane: usize, y: isize, x: isize) {
        let stride = self.bd_stride[plane];
        let iy = (y + 1) as usize;
        let ix = (x + 1) as usize;
        self.block_decoded[plane][iy * stride + ix] = 1;
    }

    // ----------------------------------------------------------- partition

    /// `decode_partition` (spec 5.11.4).
    fn decode_partition(&mut self, r: usize, c: usize, b_size: usize) -> CodecResult<()> {
        if r >= self.mi_rows() || c >= self.mi_cols() {
            return Ok(());
        }
        let avail_u = self.is_inside(r as isize - 1, c as isize);
        let avail_l = self.is_inside(r as isize, c as isize - 1);
        let num4x4 = usize::from(NUM_4X4_BLOCKS_WIDE[b_size]);
        let half_block4x4 = num4x4 >> 1;
        let quarter_block4x4 = half_block4x4 >> 1;
        let has_rows = (r + half_block4x4) < self.mi_rows();
        let has_cols = (c + half_block4x4) < self.mi_cols();

        let partition = if b_size < BLOCK_8X8 {
            PARTITION_NONE
        } else if has_rows && has_cols {
            self.read_partition_symbol(r, c, b_size, avail_u, avail_l)?
        } else if has_cols {
            let split = self.read_split_or_rect(r, c, b_size, avail_u, avail_l, true)?;
            if split {
                PARTITION_SPLIT
            } else {
                PARTITION_HORZ
            }
        } else if has_rows {
            let split = self.read_split_or_rect(r, c, b_size, avail_u, avail_l, false)?;
            if split {
                PARTITION_SPLIT
            } else {
                PARTITION_VERT
            }
        } else {
            PARTITION_SPLIT
        };

        let sub_size = usize::from(PARTITION_SUBSIZE[partition][b_size]);
        if sub_size == BLOCK_INVALID {
            return Err(CodecError::InvalidBitstream(
                "AV1: invalid partition subsize".into(),
            ));
        }
        let split_size = usize::from(PARTITION_SUBSIZE[PARTITION_SPLIT][b_size]);

        match partition {
            p if p == PARTITION_NONE => self.decode_block(r, c, sub_size)?,
            p if p == PARTITION_HORZ => {
                self.decode_block(r, c, sub_size)?;
                if has_rows {
                    self.decode_block(r + half_block4x4, c, sub_size)?;
                }
            }
            p if p == PARTITION_VERT => {
                self.decode_block(r, c, sub_size)?;
                if has_cols {
                    self.decode_block(r, c + half_block4x4, sub_size)?;
                }
            }
            p if p == PARTITION_SPLIT => {
                self.decode_partition(r, c, sub_size)?;
                self.decode_partition(r, c + half_block4x4, sub_size)?;
                self.decode_partition(r + half_block4x4, c, sub_size)?;
                self.decode_partition(r + half_block4x4, c + half_block4x4, sub_size)?;
            }
            p if p == PARTITION_HORZ_A => {
                self.decode_block(r, c, split_size)?;
                self.decode_block(r, c + half_block4x4, split_size)?;
                self.decode_block(r + half_block4x4, c, sub_size)?;
            }
            p if p == PARTITION_HORZ_B => {
                self.decode_block(r, c, sub_size)?;
                self.decode_block(r + half_block4x4, c, split_size)?;
                self.decode_block(r + half_block4x4, c + half_block4x4, split_size)?;
            }
            p if p == PARTITION_VERT_A => {
                self.decode_block(r, c, split_size)?;
                self.decode_block(r + half_block4x4, c, split_size)?;
                self.decode_block(r, c + half_block4x4, sub_size)?;
            }
            p if p == PARTITION_VERT_B => {
                self.decode_block(r, c, sub_size)?;
                self.decode_block(r, c + half_block4x4, split_size)?;
                self.decode_block(r + half_block4x4, c + half_block4x4, split_size)?;
            }
            p if p == PARTITION_HORZ_4 => {
                for k in 0..4 {
                    let rr = r + quarter_block4x4 * k;
                    if k == 3 && rr >= self.mi_rows() {
                        break;
                    }
                    self.decode_block(rr, c, sub_size)?;
                }
            }
            _ => {
                // PARTITION_VERT_4
                for k in 0..4 {
                    let cc = c + quarter_block4x4 * k;
                    if k == 3 && cc >= self.mi_cols() {
                        break;
                    }
                    self.decode_block(r, cc, sub_size)?;
                }
            }
        }
        Ok(())
    }

    /// Partition ctx (spec 8.3.2 "partition").
    fn partition_ctx(
        &self,
        r: usize,
        c: usize,
        b_size: usize,
        avail_u: bool,
        avail_l: bool,
    ) -> usize {
        let bsl = usize::from(MI_WIDTH_LOG2[b_size]);
        let above = avail_u
            && usize::from(
                MI_WIDTH_LOG2[usize::from(self.grids.mi_sizes[self.grids.at(r - 1, c)])],
            ) < bsl;
        let left = avail_l
            && usize::from(
                MI_HEIGHT_LOG2[usize::from(self.grids.mi_sizes[self.grids.at(r, c - 1)])],
            ) < bsl;
        usize::from(left) * 2 + usize::from(above)
    }

    fn read_partition_symbol(
        &mut self,
        r: usize,
        c: usize,
        b_size: usize,
        avail_u: bool,
        avail_l: bool,
    ) -> CodecResult<usize> {
        let ctx = self.partition_ctx(r, c, b_size, avail_u, avail_l);
        let bsl = usize::from(MI_WIDTH_LOG2[b_size]);
        let sym = match bsl {
            1 => self.msac.read_symbol(&mut self.cdfs.partition_w8[ctx]),
            2 => self.msac.read_symbol(&mut self.cdfs.partition_w16[ctx]),
            3 => self.msac.read_symbol(&mut self.cdfs.partition_w32[ctx]),
            4 => self.msac.read_symbol(&mut self.cdfs.partition_w64[ctx]),
            _ => self.msac.read_symbol(&mut self.cdfs.partition_w128[ctx]),
        };
        Ok(sym)
    }

    /// `split_or_horz` / `split_or_vert` (spec 8.3.2): 2-ary symbol from a
    /// synthesized CDF; the underlying partition CDF is NOT adapted.
    fn read_split_or_rect(
        &mut self,
        r: usize,
        c: usize,
        b_size: usize,
        avail_u: bool,
        avail_l: bool,
        horz: bool,
    ) -> CodecResult<bool> {
        let ctx = self.partition_ctx(r, c, b_size, avail_u, avail_l);
        let bsl = usize::from(MI_WIDTH_LOG2[b_size]);
        let pcdf: &[u16] = match bsl {
            2 => &self.cdfs.partition_w16[ctx],
            3 => &self.cdfs.partition_w32[ctx],
            4 => &self.cdfs.partition_w64[ctx],
            _ => &self.cdfs.partition_w128[ctx],
        };
        #[inline]
        fn prob(cdf: &[u16], part: usize) -> u32 {
            u32::from(cdf[part]) - u32::from(cdf[part - 1])
        }
        let mut psum = if horz {
            prob(pcdf, PARTITION_VERT)
                + prob(pcdf, PARTITION_SPLIT)
                + prob(pcdf, PARTITION_HORZ_A)
                + prob(pcdf, PARTITION_VERT_A)
                + prob(pcdf, PARTITION_VERT_B)
        } else {
            prob(pcdf, PARTITION_HORZ)
                + prob(pcdf, PARTITION_SPLIT)
                + prob(pcdf, PARTITION_HORZ_A)
                + prob(pcdf, PARTITION_HORZ_B)
                + prob(pcdf, PARTITION_VERT_A)
        };
        if b_size != BLOCK_128X128 {
            psum += if horz {
                prob(pcdf, PARTITION_VERT_4)
            } else {
                prob(pcdf, PARTITION_HORZ_4)
            };
        }
        let mut cdf = [(1u16 << 15) - psum as u16, 1 << 15, 0];
        Ok(self.msac.read_symbol(&mut cdf) != 0)
    }

    // ---------------------------------------------------------------- block

    /// `decode_block` (spec 5.11.5).
    fn decode_block(&mut self, r: usize, c: usize, mi_size: usize) -> CodecResult<()> {
        let bw4 = usize::from(NUM_4X4_BLOCKS_WIDE[mi_size]);
        let bh4 = usize::from(NUM_4X4_BLOCKS_HIGH[mi_size]);
        let mut b = Block {
            mi_row: r,
            mi_col: c,
            mi_size,
            ..Block::default()
        };
        b.has_chroma = if bh4 == 1 && self.sub_y && (r & 1) == 0 {
            false
        } else if bw4 == 1 && self.sub_x && (c & 1) == 0 {
            false
        } else {
            self.num_planes > 1
        };
        b.avail_u = self.is_inside(r as isize - 1, c as isize);
        b.avail_l = self.is_inside(r as isize, c as isize - 1);
        b.avail_u_chroma = b.avail_u;
        b.avail_l_chroma = b.avail_l;
        if b.has_chroma {
            if self.sub_y && bh4 == 1 {
                b.avail_u_chroma = self.is_inside(r as isize - 2, c as isize);
            }
            if self.sub_x && bw4 == 1 {
                b.avail_l_chroma = self.is_inside(r as isize, c as isize - 2);
            }
        } else {
            b.avail_u_chroma = false;
            b.avail_l_chroma = false;
        }

        self.intra_frame_mode_info(&mut b)?;
        // palette_tokens(): PaletteSizeY/UV are always 0 here (palette
        // signalling errors out above), so nothing is read.
        self.read_block_tx_size(&mut b);

        if b.skip {
            self.lc
                .reset_block(r, c, bw4, bh4, b.has_chroma, self.sub_x, self.sub_y);
        }
        // Replicate mode grids (pre-residual subset).
        for y in 0..bh4 {
            for x in 0..bw4 {
                if r + y >= self.mi_rows() || c + x >= self.mi_cols() {
                    continue;
                }
                let idx = self.grids.at(r + y, c + x);
                self.grids.y_modes[idx] = b.y_mode as u8;
                if b.has_chroma {
                    self.grids.uv_modes[idx] = b.uv_mode as u8;
                }
            }
        }
        // compute_prediction() is a no-op for intra blocks (no interintra).
        self.residual(&mut b)?;
        for y in 0..bh4 {
            for x in 0..bw4 {
                if r + y >= self.mi_rows() || c + x >= self.mi_cols() {
                    continue;
                }
                let idx = self.grids.at(r + y, c + x);
                self.grids.skips[idx] = u8::from(b.skip);
                self.grids.tx_sizes[idx] = b.tx_size as u8;
                self.grids.mi_sizes[idx] = b.mi_size as u8;
                self.grids.seg_ids[idx] = b.segment_id as u8;
                for (i, d) in self.delta_lf.iter().enumerate() {
                    self.grids.delta_lfs[idx][i] = *d as i8;
                }
            }
        }
        Ok(())
    }

    /// `intra_frame_mode_info` (spec 5.11.10) plus the syntax it invokes.
    fn intra_frame_mode_info(&mut self, b: &mut Block) -> CodecResult<()> {
        b.skip = false;
        if self.hdr.seg.seg_id_pre_skip {
            self.intra_segment_id(b)?;
        }
        // skip_mode = 0 on intra frames.
        self.read_skip(b);
        if !self.hdr.seg.seg_id_pre_skip {
            self.intra_segment_id(b)?;
        }
        self.read_cdef(b);
        self.read_delta_qindex(b);
        self.read_delta_lf(b);
        self.read_deltas = false;

        if self.hdr.allow_intrabc {
            let use_intrabc = self.msac.read_symbol(&mut self.cdfs.intrabc) != 0;
            if use_intrabc {
                // TODO(0.2.x): intra block copy — needs in-frame MV
                // prediction and block copy with the intrabc delay rules.
                return Err(CodecError::UnsupportedFeature(
                    "AV1 intra block copy (use_intrabc) not implemented".into(),
                ));
            }
        }

        // intra_frame_y_mode with neighbor mode context.
        let above_mode = if b.avail_u {
            usize::from(self.grids.y_modes[self.grids.at(b.mi_row - 1, b.mi_col)])
        } else {
            DC_PRED
        };
        let left_mode = if b.avail_l {
            usize::from(self.grids.y_modes[self.grids.at(b.mi_row, b.mi_col - 1)])
        } else {
            DC_PRED
        };
        let above_ctx = usize::from(INTRA_MODE_CONTEXT[above_mode]);
        let left_ctx = usize::from(INTRA_MODE_CONTEXT[left_mode]);
        b.y_mode = self
            .msac
            .read_symbol(&mut self.cdfs.intra_frame_y_mode[above_ctx][left_ctx]);
        // intra_angle_info_y
        b.angle_delta_y = 0;
        if b.mi_size >= BLOCK_8X8 && is_directional_mode(b.y_mode) {
            let sym = self
                .msac
                .read_symbol(&mut self.cdfs.angle_delta[b.y_mode - V_PRED]);
            b.angle_delta_y = sym as i32 - MAX_ANGLE_DELTA as i32;
        }
        if b.has_chroma {
            let cfl_allowed = if b.lossless {
                usize::from(
                    SUBSAMPLED_SIZE[b.mi_size][usize::from(self.sub_x)][usize::from(self.sub_y)],
                ) == BLOCK_4X4
            } else {
                core::cmp::max(block_width(b.mi_size), block_height(b.mi_size)) <= 32
            };
            b.uv_mode = if cfl_allowed {
                self.msac
                    .read_symbol(&mut self.cdfs.uv_mode_cfl_allowed[b.y_mode])
            } else {
                self.msac
                    .read_symbol(&mut self.cdfs.uv_mode_cfl_not_allowed[b.y_mode])
            };
            if b.uv_mode == UV_CFL_PRED {
                self.read_cfl_alphas(b);
            }
            b.angle_delta_uv = 0;
            if b.mi_size >= BLOCK_8X8 && is_directional_mode(b.uv_mode) {
                let sym = self
                    .msac
                    .read_symbol(&mut self.cdfs.angle_delta[b.uv_mode - V_PRED]);
                b.angle_delta_uv = sym as i32 - MAX_ANGLE_DELTA as i32;
            }
        }
        // palette_mode_info(): flags must be read to stay in sync.
        if b.mi_size >= BLOCK_8X8
            && block_width(b.mi_size) <= 64
            && block_height(b.mi_size) <= 64
            && self.hdr.allow_screen_content_tools
        {
            let bsize_ctx =
                usize::from(MI_WIDTH_LOG2[b.mi_size]) + usize::from(MI_HEIGHT_LOG2[b.mi_size]) - 2;
            if b.y_mode == DC_PRED {
                // has_palette_y ctx: neighboring palette sizes are always 0
                // in this decoder (palette errors out), so ctx == 0.
                let has_palette_y = self
                    .msac
                    .read_symbol(&mut self.cdfs.palette_y_mode[bsize_ctx][0])
                    != 0;
                if has_palette_y {
                    // TODO(0.2.x): palette mode — color cache, delta-coded
                    // palettes and the diagonal color-index map.
                    return Err(CodecError::UnsupportedFeature(
                        "AV1 palette mode (has_palette_y) not implemented".into(),
                    ));
                }
            }
            if b.has_chroma && b.uv_mode == DC_PRED {
                let has_palette_uv = self.msac.read_symbol(&mut self.cdfs.palette_uv_mode[0]) != 0;
                if has_palette_uv {
                    return Err(CodecError::UnsupportedFeature(
                        "AV1 palette mode (has_palette_uv) not implemented".into(),
                    ));
                }
            }
        }
        // filter_intra_mode_info()
        b.use_filter_intra = false;
        if self.seq.enable_filter_intra
            && b.y_mode == DC_PRED
            && core::cmp::max(block_width(b.mi_size), block_height(b.mi_size)) <= 32
        {
            b.use_filter_intra = self
                .msac
                .read_symbol(&mut self.cdfs.filter_intra[b.mi_size])
                != 0;
            if b.use_filter_intra {
                b.filter_intra_mode = self.msac.read_symbol(&mut self.cdfs.filter_intra_mode);
            }
        }
        Ok(())
    }

    /// `intra_segment_id` + `read_segment_id` (spec 5.11.11/12).
    fn intra_segment_id(&mut self, b: &mut Block) -> CodecResult<()> {
        if self.hdr.seg.enabled {
            self.read_segment_id(b);
        } else {
            b.segment_id = 0;
        }
        b.lossless = self.hdr.lossless_array[b.segment_id];
        Ok(())
    }

    fn read_segment_id(&mut self, b: &mut Block) {
        let (r, c) = (b.mi_row, b.mi_col);
        let prev_ul = if b.avail_u && b.avail_l {
            i32::from(self.grids.seg_ids[self.grids.at(r - 1, c - 1)])
        } else {
            -1
        };
        let prev_u = if b.avail_u {
            i32::from(self.grids.seg_ids[self.grids.at(r - 1, c)])
        } else {
            -1
        };
        let prev_l = if b.avail_l {
            i32::from(self.grids.seg_ids[self.grids.at(r, c - 1)])
        } else {
            -1
        };
        let pred = if prev_u == -1 {
            if prev_l == -1 {
                0
            } else {
                prev_l
            }
        } else if prev_l == -1 {
            prev_u
        } else if prev_ul == prev_u {
            prev_u
        } else {
            prev_l
        };
        if b.skip {
            b.segment_id = pred as usize;
            return;
        }
        let ctx = if prev_ul < 0 {
            0
        } else if prev_ul == prev_u && prev_ul == prev_l {
            2
        } else if prev_ul == prev_u || prev_ul == prev_l || prev_u == prev_l {
            1
        } else {
            0
        };
        let diff = self.msac.read_symbol(&mut self.cdfs.segment_id[ctx]) as i32;
        let max = self.hdr.seg.last_active_seg_id as i32 + 1;
        b.segment_id = neg_deinterleave(diff, pred, max) as usize;
    }

    /// `read_skip` (spec 5.11.14).
    fn read_skip(&mut self, b: &mut Block) {
        let seg = &self.hdr.seg;
        if seg.seg_id_pre_skip && seg.enabled && seg.feature_enabled[b.segment_id][SEG_LVL_SKIP] {
            b.skip = true;
        } else {
            let mut ctx = 0usize;
            if b.avail_u {
                ctx += usize::from(self.grids.skips[self.grids.at(b.mi_row - 1, b.mi_col)]);
            }
            if b.avail_l {
                ctx += usize::from(self.grids.skips[self.grids.at(b.mi_row, b.mi_col - 1)]);
            }
            b.skip = self.msac.read_symbol(&mut self.cdfs.skip[ctx]) != 0;
        }
    }

    /// `read_cdef` (spec 5.11.56).
    fn read_cdef(&mut self, b: &Block) {
        if b.skip || self.hdr.coded_lossless || !self.seq.enable_cdef || self.hdr.allow_intrabc {
            return;
        }
        let cdef_size4 = usize::from(NUM_4X4_BLOCKS_WIDE[BLOCK_64X64]);
        let cdef_mask4 = !(cdef_size4 - 1);
        let r = b.mi_row & cdef_mask4;
        let c = b.mi_col & cdef_mask4;
        if self.grids.cdef_idx[self.grids.at(r, c)] == -1 {
            let v = self.msac.read_literal(self.hdr.cdef.bits) as i16;
            let w4 = usize::from(NUM_4X4_BLOCKS_WIDE[b.mi_size]);
            let h4 = usize::from(NUM_4X4_BLOCKS_HIGH[b.mi_size]);
            let mut i = r;
            while i < r + h4 {
                let mut j = c;
                while j < c + w4 {
                    if i < self.mi_rows() && j < self.mi_cols() {
                        let idx = self.grids.at(i, j);
                        self.grids.cdef_idx[idx] = v;
                    }
                    j += cdef_size4;
                }
                i += cdef_size4;
            }
        }
    }

    /// `read_delta_qindex` (spec 5.11.18).
    fn read_delta_qindex(&mut self, b: &Block) {
        let sb_size = if self.seq.use_128x128_superblock {
            BLOCK_128X128
        } else {
            BLOCK_64X64
        };
        if b.mi_size == sb_size && b.skip {
            return;
        }
        if self.read_deltas {
            let mut delta_q_abs = self.msac.read_symbol(&mut self.cdfs.delta_q) as u32;
            if delta_q_abs == DELTA_Q_SMALL as u32 {
                let delta_q_rem_bits = self.msac.read_literal(3) + 1;
                let delta_q_abs_bits = self.msac.read_literal(delta_q_rem_bits);
                delta_q_abs = delta_q_abs_bits + (1 << delta_q_rem_bits) + 1;
            }
            if delta_q_abs != 0 {
                let sign = self.msac.read_literal(1) != 0;
                let reduced = if sign {
                    -(delta_q_abs as i64)
                } else {
                    delta_q_abs as i64
                };
                let v = i64::from(self.current_q_index) + (reduced << self.hdr.delta_q_res);
                self.current_q_index = v.clamp(1, 255) as u32;
            }
        }
    }

    /// `read_delta_lf` (spec 5.11.19).
    fn read_delta_lf(&mut self, b: &Block) {
        let sb_size = if self.seq.use_128x128_superblock {
            BLOCK_128X128
        } else {
            BLOCK_64X64
        };
        if b.mi_size == sb_size && b.skip {
            return;
        }
        if self.read_deltas && self.hdr.delta_lf_present {
            let frame_lf_count = if self.hdr.delta_lf_multi {
                if self.num_planes > 1 {
                    FRAME_LF_COUNT
                } else {
                    FRAME_LF_COUNT - 2
                }
            } else {
                1
            };
            for i in 0..frame_lf_count {
                let sym = if self.hdr.delta_lf_multi {
                    self.msac.read_symbol(&mut self.cdfs.delta_lf_multi[i])
                } else {
                    self.msac.read_symbol(&mut self.cdfs.delta_lf)
                } as u32;
                let delta_lf_abs = if sym == DELTA_LF_SMALL as u32 {
                    let n = self.msac.read_literal(3) + 1;
                    let bits = self.msac.read_literal(n);
                    bits + (1 << n) + 1
                } else {
                    sym
                };
                if delta_lf_abs != 0 {
                    let sign = self.msac.read_literal(1) != 0;
                    let reduced = if sign {
                        -(delta_lf_abs as i64)
                    } else {
                        delta_lf_abs as i64
                    };
                    let v = i64::from(self.delta_lf[i]) + (reduced << self.hdr.delta_lf_res);
                    self.delta_lf[i] =
                        v.clamp(-(MAX_LOOP_FILTER as i64), MAX_LOOP_FILTER as i64) as i32;
                }
            }
        }
    }

    /// `read_cfl_alphas` (spec 5.11.45).
    fn read_cfl_alphas(&mut self, b: &mut Block) {
        let cfl_alpha_signs = self.msac.read_symbol(&mut self.cdfs.cfl_sign);
        let sign_u = (cfl_alpha_signs + 1) / 3;
        let sign_v = (cfl_alpha_signs + 1) % 3;
        // CFL_SIGN_ZERO = 0, CFL_SIGN_NEG = 1, CFL_SIGN_POS = 2 (spec 6.10.
        // cfl_alpha_signs semantics).
        b.cfl_alpha_u = if sign_u != 0 {
            let ctx = (sign_u - 1) * 3 + sign_v;
            let mut v = 1 + self.msac.read_symbol(&mut self.cdfs.cfl_alpha[ctx]) as i32;
            if sign_u == 1 {
                v = -v;
            }
            v
        } else {
            0
        };
        b.cfl_alpha_v = if sign_v != 0 {
            let ctx = (sign_v - 1) * 3 + sign_u;
            let mut v = 1 + self.msac.read_symbol(&mut self.cdfs.cfl_alpha[ctx]) as i32;
            if sign_v == 1 {
                v = -v;
            }
            v
        } else {
            0
        };
    }

    // ------------------------------------------------------------- tx size

    /// `read_block_tx_size` + `read_tx_size` (spec 5.11.15/16), intra path.
    fn read_block_tx_size(&mut self, b: &mut Block) {
        // Intra blocks always use the non-vartx path.
        self.read_tx_size(b, true);
        let bw4 = usize::from(NUM_4X4_BLOCKS_WIDE[b.mi_size]);
        let bh4 = usize::from(NUM_4X4_BLOCKS_HIGH[b.mi_size]);
        for row in b.mi_row..(b.mi_row + bh4).min(self.mi_rows()) {
            for col in b.mi_col..(b.mi_col + bw4).min(self.mi_cols()) {
                let idx = self.grids.at(row, col);
                self.grids.inter_tx_sizes[idx] = b.tx_size as u8;
            }
        }
    }

    fn read_tx_size(&mut self, b: &mut Block, allow_select: bool) {
        if b.lossless {
            b.tx_size = TX_4X4;
            return;
        }
        let max_rect_tx_size = usize::from(MAX_TX_SIZE_RECT[b.mi_size]);
        let max_tx_depth = usize::from(MAX_TX_DEPTH[b.mi_size]);
        b.tx_size = max_rect_tx_size;
        if b.mi_size > BLOCK_4X4 && allow_select && self.hdr.tx_mode_select {
            // tx_depth ctx (spec 8.3.2 "tx_depth").
            let max_tx_width = usize::from(TX_WIDTH[max_rect_tx_size]);
            let max_tx_height = usize::from(TX_HEIGHT[max_rect_tx_size]);
            // Intra frame: IsInters is always 0, so the neighbor dimensions
            // come from get_above_tx_width / get_left_tx_height.
            let above_w = if b.avail_u {
                usize::from(
                    TX_WIDTH[usize::from(
                        self.grids.inter_tx_sizes[self.grids.at(b.mi_row - 1, b.mi_col)],
                    )],
                )
            } else {
                0
            };
            let left_h = if b.avail_l {
                usize::from(
                    TX_HEIGHT[usize::from(
                        self.grids.inter_tx_sizes[self.grids.at(b.mi_row, b.mi_col - 1)],
                    )],
                )
            } else {
                0
            };
            let ctx = usize::from(above_w >= max_tx_width) + usize::from(left_h >= max_tx_height);
            let tx_depth = match max_tx_depth {
                4 => self.msac.read_symbol(&mut self.cdfs.tx_64x64[ctx]),
                3 => self.msac.read_symbol(&mut self.cdfs.tx_32x32[ctx]),
                2 => self.msac.read_symbol(&mut self.cdfs.tx_16x16[ctx]),
                _ => self.msac.read_symbol(&mut self.cdfs.tx_8x8[ctx]),
            };
            for _ in 0..tx_depth {
                b.tx_size = usize::from(SPLIT_TX_SIZE[b.tx_size]);
            }
        }
    }

    // ------------------------------------------------------------ residual

    /// `residual()` (spec 5.11.34).
    fn residual(&mut self, b: &mut Block) -> CodecResult<()> {
        let sb_mask = if self.seq.use_128x128_superblock {
            31
        } else {
            15
        };
        let width_chunks = core::cmp::max(1, block_width(b.mi_size) >> 6);
        let height_chunks = core::cmp::max(1, block_height(b.mi_size) >> 6);
        let mi_size_chunk = if width_chunks > 1 || height_chunks > 1 {
            BLOCK_64X64
        } else {
            b.mi_size
        };
        for chunk_y in 0..height_chunks {
            for chunk_x in 0..width_chunks {
                let mi_row_chunk = b.mi_row + (chunk_y << 4);
                let mi_col_chunk = b.mi_col + (chunk_x << 4);
                for plane in 0..(1 + usize::from(b.has_chroma) * 2) {
                    let tx_sz = if b.lossless {
                        TX_4X4
                    } else {
                        self.get_tx_size(plane, b.tx_size, b.mi_size)
                    };
                    let step_x = usize::from(TX_WIDTH[tx_sz]) >> 2;
                    let step_y = usize::from(TX_HEIGHT[tx_sz]) >> 2;
                    let (sub_x, sub_y) = self.plane_subsampling(plane);
                    let plane_sz =
                        coef::get_plane_residual_size(mi_size_chunk, plane, self.sub_x, self.sub_y);
                    let num4x4w = usize::from(NUM_4X4_BLOCKS_WIDE[plane_sz]);
                    let num4x4h = usize::from(NUM_4X4_BLOCKS_HIGH[plane_sz]);
                    let base_x_block = (b.mi_col >> usize::from(sub_x)) * 4;
                    let base_y_block = (b.mi_row >> usize::from(sub_y)) * 4;
                    let mut y = 0;
                    while y < num4x4h {
                        let mut x = 0;
                        while x < num4x4w {
                            self.transform_block(
                                b,
                                plane,
                                base_x_block,
                                base_y_block,
                                tx_sz,
                                x + ((chunk_x << 4) >> usize::from(sub_x)),
                                y + ((chunk_y << 4) >> usize::from(sub_y)),
                                sb_mask,
                            )?;
                            x += step_x;
                        }
                        y += step_y;
                    }
                }
                let _ = mi_row_chunk;
                let _ = mi_col_chunk;
            }
        }
        Ok(())
    }

    /// `get_tx_size( plane, txSz )` (spec 5.11.37).
    fn get_tx_size(&self, plane: usize, tx_sz: usize, mi_size: usize) -> usize {
        if plane == 0 {
            return tx_sz;
        }
        let plane_sz = coef::get_plane_residual_size(mi_size, plane, self.sub_x, self.sub_y);
        let uv_tx = usize::from(MAX_TX_SIZE_RECT[plane_sz]);
        if usize::from(TX_WIDTH[uv_tx]) == 64 || usize::from(TX_HEIGHT[uv_tx]) == 64 {
            if usize::from(TX_WIDTH[uv_tx]) == 16 {
                return TX_16X32;
            }
            if usize::from(TX_HEIGHT[uv_tx]) == 16 {
                return TX_32X16;
            }
            return TX_32X32;
        }
        uv_tx
    }

    /// `transform_block` (spec 5.11.35).
    #[allow(clippy::too_many_arguments)]
    fn transform_block(
        &mut self,
        b: &mut Block,
        plane: usize,
        base_x: usize,
        base_y: usize,
        tx_sz: usize,
        x: usize,
        y: usize,
        sb_mask: usize,
    ) -> CodecResult<()> {
        let start_x = base_x + 4 * x;
        let start_y = base_y + 4 * y;
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        let sxu = usize::from(sub_x);
        let syu = usize::from(sub_y);
        let row = (start_y << syu) >> 2;
        let col = (start_x << sxu) >> 2;
        let sub_block_mi_row = row & sb_mask;
        let sub_block_mi_col = col & sb_mask;
        let step_x = usize::from(TX_WIDTH[tx_sz]) >> 2;
        let step_y = usize::from(TX_HEIGHT[tx_sz]) >> 2;
        let max_x = (self.mi_cols() * 4) >> sxu;
        let max_y = (self.mi_rows() * 4) >> syu;
        if start_x >= max_x || start_y >= max_y {
            return Ok(());
        }

        // Intra prediction (palette is rejected earlier).
        let is_cfl = plane > 0 && b.uv_mode == UV_CFL_PRED;
        let mode = if plane == 0 {
            b.y_mode
        } else if is_cfl {
            DC_PRED
        } else {
            b.uv_mode
        };
        let log2w = u32::from(super::tables_conv::TX_WIDTH_LOG2[tx_sz]);
        let log2h = u32::from(super::tables_conv::TX_HEIGHT_LOG2[tx_sz]);
        let have_left = if plane == 0 {
            b.avail_l
        } else {
            b.avail_l_chroma
        } || x > 0;
        let have_above = if plane == 0 {
            b.avail_u
        } else {
            b.avail_u_chroma
        } || y > 0;
        let have_above_right = self.bd_get(
            plane,
            (sub_block_mi_row >> syu) as isize - 1,
            ((sub_block_mi_col >> sxu) + step_x) as isize,
        );
        let have_below_left = self.bd_get(
            plane,
            ((sub_block_mi_row >> syu) + step_y) as isize,
            (sub_block_mi_col >> sxu) as isize - 1,
        );
        let filter_type = self.get_filter_type(b, plane);
        let params = PredParams {
            have_left,
            have_above,
            have_above_right,
            have_below_left,
            mode,
            log2w,
            log2h,
            angle_delta: if plane == 0 {
                b.angle_delta_y
            } else {
                b.angle_delta_uv
            },
            filter_intra_mode: if plane == 0 && b.use_filter_intra {
                Some(b.filter_intra_mode)
            } else {
                None
            },
            enable_intra_edge_filter: self.seq.enable_intra_edge_filter,
            filter_type,
            max_x: max_x - 1,
            max_y: max_y - 1,
        };
        {
            let p = &mut self.planes[plane];
            predict_intra(&mut p.data, p.stride, start_x, start_y, &params);
        }
        if is_cfl {
            let alpha = if plane == 1 {
                b.cfl_alpha_u
            } else {
                b.cfl_alpha_v
            };
            let (luma, rest) = self.planes.split_at_mut(plane);
            let chroma = &mut rest[0];
            predict_cfl(
                &mut chroma.data,
                chroma.stride,
                &luma[0].data,
                luma[0].stride,
                start_x,
                start_y,
                log2w,
                log2h,
                alpha,
                self.sub_x,
                self.sub_y,
                b.max_luma_w,
                b.max_luma_h,
            );
        }
        if plane == 0 {
            b.max_luma_w = start_x + step_x * 4;
            b.max_luma_h = start_y + step_y * 4;
        }

        if !b.skip {
            let cb = CoefBlock {
                plane,
                start_x,
                start_y,
                tx_sz,
                mi_size: b.mi_size,
                lossless: b.lossless,
                qindex_gt0: if self.hdr.seg.enabled {
                    self.hdr.qindex_ignoring_deltaq(b.segment_id) > 0
                } else {
                    self.hdr.base_q_idx > 0
                },
                reduced_tx_set: self.hdr.reduced_tx_set,
                intra_dir: if b.use_filter_intra {
                    FILTER_INTRA_MODE_TO_INTRA_DIR[b.filter_intra_mode]
                } else {
                    b.y_mode
                },
                uv_mode: b.uv_mode,
                sub_x: self.sub_x,
                sub_y: self.sub_y,
                mi_cols: self.mi_cols(),
                mi_rows: self.mi_rows(),
            };
            let mut quant = [0i32; 1024];
            let (eob, plane_tx_type) = coef::decode_coeffs(
                &mut self.msac,
                &mut self.cdfs,
                &mut self.lc,
                &mut self.grids.tx_types,
                &cb,
                &mut quant,
            )?;
            if eob > 0 {
                self.reconstruct(b, plane, start_x, start_y, tx_sz, plane_tx_type, &quant);
            }
        }

        for i in 0..step_y {
            for j in 0..step_x {
                let gr = (row >> syu) + i;
                let gc = (col >> sxu) + j;
                if gr < self.mi_rows() && gc < self.mi_cols() {
                    let idx = gr * self.mi_cols() + gc;
                    self.grids.lf_tx_sizes[plane][idx] = tx_sz as u8;
                }
                self.bd_set(
                    plane,
                    ((sub_block_mi_row >> syu) + i) as isize,
                    ((sub_block_mi_col >> sxu) + j) as isize,
                );
            }
        }
        Ok(())
    }

    /// `get_filter_type` (spec 7.11.2.8).
    fn get_filter_type(&self, b: &Block, plane: usize) -> bool {
        let mut above_smooth = false;
        let mut left_smooth = false;
        let avail_above = if plane == 0 {
            b.avail_u
        } else {
            b.avail_u_chroma
        };
        let avail_left = if plane == 0 {
            b.avail_l
        } else {
            b.avail_l_chroma
        };
        if avail_above {
            let mut r = b.mi_row as isize - 1;
            let mut c = b.mi_col as isize;
            if plane > 0 {
                if self.sub_x && (b.mi_col & 1) == 0 {
                    c += 1;
                }
                if self.sub_y && (b.mi_row & 1) == 1 {
                    r -= 1;
                }
            }
            above_smooth = self.is_smooth(r as usize, c as usize, plane);
        }
        if avail_left {
            let mut r = b.mi_row as isize;
            let mut c = b.mi_col as isize - 1;
            if plane > 0 {
                if self.sub_x && (b.mi_col & 1) == 1 {
                    c -= 1;
                }
                if self.sub_y && (b.mi_row & 1) == 0 {
                    r += 1;
                }
            }
            left_smooth = self.is_smooth(r as usize, c as usize, plane);
        }
        above_smooth || left_smooth
    }

    fn is_smooth(&self, row: usize, col: usize, plane: usize) -> bool {
        use super::consts::{SMOOTH_H_PRED, SMOOTH_PRED, SMOOTH_V_PRED};
        if row >= self.mi_rows() || col >= self.mi_cols() {
            return false;
        }
        let mode = if plane == 0 {
            usize::from(self.grids.y_modes[self.grids.at(row, col)])
        } else {
            // Intra frame: every block is intra (RefFrames[..][0] ==
            // INTRA_FRAME), so the inter check of is_smooth never fires.
            usize::from(self.grids.uv_modes[self.grids.at(row, col)])
        };
        mode == SMOOTH_PRED || mode == SMOOTH_V_PRED || mode == SMOOTH_H_PRED
    }

    /// `reconstruct` (spec 7.12.3): dequantize, inverse transform, add.
    fn reconstruct(
        &mut self,
        b: &Block,
        plane: usize,
        x: usize,
        y: usize,
        tx_sz: usize,
        plane_tx_type: usize,
        quant: &[i32; 1024],
    ) {
        use super::consts::{
            ADST_FLIPADST, DCT_FLIPADST, FLIPADST_ADST, FLIPADST_DCT, FLIPADST_FLIPADST,
            H_FLIPADST, V_FLIPADST,
        };
        let dq_denom: i64 = if tx_sz == TX_32X32
            || tx_sz == TX_16X32
            || tx_sz == TX_32X16
            || tx_sz == TX_16X64
            || tx_sz == TX_64X16
        {
            2
        } else if tx_sz == TX_64X64 || tx_sz == TX_32X64 || tx_sz == TX_64X32 {
            4
        } else {
            1
        };
        let log2w = u32::from(super::tables_conv::TX_WIDTH_LOG2[tx_sz]);
        let log2h = u32::from(super::tables_conv::TX_HEIGHT_LOG2[tx_sz]);
        let w = 1usize << log2w;
        let h = 1usize << log2h;
        let tw = core::cmp::min(32, w);
        let th = core::cmp::min(32, h);
        let flip_ud = plane_tx_type == FLIPADST_DCT
            || plane_tx_type == FLIPADST_ADST
            || plane_tx_type == V_FLIPADST
            || plane_tx_type == FLIPADST_FLIPADST;
        let flip_lr = plane_tx_type == DCT_FLIPADST
            || plane_tx_type == ADST_FLIPADST
            || plane_tx_type == H_FLIPADST
            || plane_tx_type == FLIPADST_FLIPADST;

        let dc_q = i64::from(self.dc_quant(plane, b.segment_id));
        let ac_q = i64::from(self.ac_quant(plane, b.segment_id));
        // using_qmatrix is rejected at frame level (honest error), so q2 = q.
        let mut dequant = [0i32; 32 * 32];
        for i in 0..th {
            for j in 0..tw {
                let q = if i == 0 && j == 0 { dc_q } else { ac_q };
                let dq = i64::from(quant[i * tw + j]) * q;
                let sign: i64 = if dq < 0 { -1 } else { 1 };
                let dq2 = sign * ((dq.abs() & 0xFF_FFFF) / dq_denom);
                // `dq2` is masked to 24 bits before the sign is reapplied, so
                // it always lies in -0xFF_FFFF..=0xFF_FFFF and the narrowing
                // is exact; the fallback saturates rather than wrapping.
                let dq2 = i32::try_from(dq2).unwrap_or(if dq2 < 0 { i32::MIN } else { i32::MAX });
                let bd_clamp = 1i32 << (7 + 8);
                dequant[i * 32 + j] = dq2.clamp(-bd_clamp, bd_clamp - 1);
            }
        }
        let mut residual = vec![0i32; w * h];
        inverse_transform_2d(&dequant, &mut residual, tx_sz, plane_tx_type, b.lossless, 8);

        let p = &mut self.planes[plane];
        for i in 0..h {
            for j in 0..w {
                let xx = if flip_lr { w - j - 1 } else { j };
                let yy = if flip_ud { h - i - 1 } else { i };
                let px = y + yy;
                let pxx = x + xx;
                if px < p.height && pxx < p.width {
                    let cur = i32::from(p.data[px * p.stride + pxx]);
                    p.data[px * p.stride + pxx] = (cur + residual[i * w + j]).clamp(0, 255) as u8;
                }
            }
        }
    }
}

/// `neg_deinterleave` (spec 5.11.13).
fn neg_deinterleave(diff: i32, r#ref: i32, max: i32) -> i32 {
    if r#ref == 0 {
        return diff;
    }
    if r#ref >= max - 1 {
        return max - diff - 1;
    }
    if 2 * r#ref < max {
        if diff <= 2 * r#ref {
            if diff & 1 != 0 {
                return r#ref + ((diff + 1) >> 1);
            }
            return r#ref - (diff >> 1);
        }
        diff
    } else {
        if diff <= 2 * (max - r#ref - 1) {
            if diff & 1 != 0 {
                return r#ref + ((diff + 1) >> 1);
            }
            return r#ref - (diff >> 1);
        }
        max - (diff + 1)
    }
}

/// Decodes all tiles of one intra frame and returns the reconstructed
/// (unfiltered-cropped) planes.
///
/// `tile_payloads` holds one coded byte range per tile in raster order.
///
/// # Errors
///
/// Honest `UnsupportedFeature` for surfaces beyond the current stage and
/// `InvalidBitstream` for malformed data.
pub(crate) fn decode_intra_frame(
    seq: &SeqHdr,
    hdr: &FrameHdr,
    tile_payloads: &[&[u8]],
) -> CodecResult<DecodedIntraFrame> {
    decode_intra_frame_banded(seq, hdr, tile_payloads, None)
}

/// [`decode_intra_frame`] with an explicit row-band count for the CDEF and
/// loop-restoration stages. `None` (the production path) picks the count
/// from the frame size and the rayon pool; a forced count only changes how
/// the work is scheduled, never the decoded samples.
pub(crate) fn decode_intra_frame_banded(
    seq: &SeqHdr,
    hdr: &FrameHdr,
    tile_payloads: &[&[u8]],
    forced_bands: Option<usize>,
) -> CodecResult<DecodedIntraFrame> {
    // Stage gates (all honest errors; each names its missing surface).
    if seq.bit_depth != 8 || seq.mono_chrome || !seq.subsampling_x || !seq.subsampling_y {
        // TODO(0.2.x): 10/12-bit (highbd pipeline), monochrome, 4:2:2/4:4:4.
        return Err(CodecError::UnsupportedFeature(format!(
            "AV1 keyframe decode supports 8-bit 4:2:0 (profile 0); got {}-bit mono={} ss=({},{})",
            seq.bit_depth, seq.mono_chrome, seq.subsampling_x, seq.subsampling_y
        )));
    }
    if hdr.use_superres {
        // TODO(0.2.x): horizontal superres upscaling (spec 7.16).
        return Err(CodecError::UnsupportedFeature(
            "AV1 super-resolution upscaling not implemented".into(),
        ));
    }
    if hdr.using_qmatrix {
        // TODO(0.2.x): quantizer matrices (spec 7.12.2 Quantizer_Matrix).
        return Err(CodecError::UnsupportedFeature(
            "AV1 quantizer matrices (using_qmatrix) not implemented".into(),
        ));
    }
    if hdr.apply_grain {
        // TODO(0.2.x): film grain synthesis on output (spec 7.18.3).
        return Err(CodecError::UnsupportedFeature(
            "AV1 film grain synthesis not implemented".into(),
        ));
    }
    let mi_rows = hdr.mi_rows as usize;
    let mi_cols = hdr.mi_cols as usize;
    let luma_w = mi_cols * 4;
    let luma_h = mi_rows * 4;
    let sub_x = seq.subsampling_x;
    let sub_y = seq.subsampling_y;
    let chroma_w = luma_w >> usize::from(sub_x);
    let chroma_h = luma_h >> usize::from(sub_y);
    let mk_plane = |w: usize, h: usize| PlaneBuf {
        data: vec![0; w * h],
        stride: w,
        width: w,
        height: h,
    };

    let sb_size = if seq.use_128x128_superblock {
        BLOCK_128X128
    } else {
        BLOCK_64X64
    };
    let sb_size4 = usize::from(NUM_4X4_BLOCKS_WIDE[sb_size]);
    let bd_dims = |sub: bool| (sb_size4 >> usize::from(sub)) + 3;

    let num_tiles = (hdr.tiles.tile_cols * hdr.tiles.tile_rows) as usize;
    if tile_payloads.len() != num_tiles {
        return Err(CodecError::InvalidBitstream(format!(
            "AV1: expected {} tiles, got {} tile payloads",
            num_tiles,
            tile_payloads.len()
        )));
    }

    let mut dec = Dec {
        seq,
        hdr,
        planes: [
            mk_plane(luma_w, luma_h),
            mk_plane(chroma_w, chroma_h),
            mk_plane(chroma_w, chroma_h),
        ],
        grids: Grids::new(mi_rows, mi_cols),
        msac: Msac::new(&[], false),
        cdfs: CdfCtx::new(hdr.base_q_idx),
        lc: LevelCtxs::new(mi_cols, mi_rows),
        mi_row_start: 0,
        mi_row_end: mi_rows,
        mi_col_start: 0,
        mi_col_end: mi_cols,
        block_decoded: [
            vec![0; bd_dims(false) * bd_dims(false)],
            vec![0; bd_dims(sub_x) * bd_dims(sub_y)],
            vec![0; bd_dims(sub_x) * bd_dims(sub_y)],
        ],
        bd_stride: [bd_dims(false), bd_dims(sub_x), bd_dims(sub_x)],
        current_q_index: hdr.base_q_idx,
        delta_lf: [0; FRAME_LF_COUNT],
        read_deltas: false,
        sub_x,
        sub_y,
        num_planes: seq.num_planes as usize,
        sb_size,
        sb_size4,
        lr_units: super::lr::LrUnitGrids::new(hdr, sub_x, sub_y, seq.num_planes as usize),
        lr_refs: super::lr::LrRefs::new(),
    };

    for tile_num in 0..num_tiles {
        let tile_row = tile_num / hdr.tiles.tile_cols as usize;
        let tile_col = tile_num % hdr.tiles.tile_cols as usize;
        dec.mi_row_start = hdr.tiles.mi_row_starts[tile_row] as usize;
        dec.mi_row_end = hdr.tiles.mi_row_starts[tile_row + 1] as usize;
        dec.mi_col_start = hdr.tiles.mi_col_starts[tile_col] as usize;
        dec.mi_col_end = hdr.tiles.mi_col_starts[tile_col + 1] as usize;
        dec.current_q_index = hdr.base_q_idx;
        // Per-tile CDF copy of the frame-initial state (spec 8.2.2).
        dec.cdfs = CdfCtx::new(hdr.base_q_idx);
        dec.msac = Msac::new(tile_payloads[tile_num], !hdr.disable_cdf_update);
        dec.decode_tile()?;
    }

    // Deblocking loop filter (spec 7.14).
    if hdr.lf.level[0] != 0 || hdr.lf.level[1] != 0 {
        let input = super::lf::LfInput {
            hdr,
            sub_x,
            sub_y,
            num_planes: seq.num_planes as usize,
            mi_rows,
            mi_cols,
            mi_sizes: &dec.grids.mi_sizes,
            skips: &dec.grids.skips,
            seg_ids: &dec.grids.seg_ids,
            delta_lfs: &dec.grids.delta_lfs,
            lf_tx_sizes: &dec.grids.lf_tx_sizes,
        };
        super::lf::loop_filter_frame(&mut dec.planes, &input);
    }

    // Loop restoration reads deblocked samples outside the current stripe
    // (spec 7.17.6); keep a pre-CDEF copy when LR is active.
    let pre_cdef: Option<[PlaneBuf; 3]> = if hdr.lr.uses_lr {
        Some([
            PlaneBuf {
                data: dec.planes[0].data.clone(),
                stride: dec.planes[0].stride,
                width: dec.planes[0].width,
                height: dec.planes[0].height,
            },
            PlaneBuf {
                data: dec.planes[1].data.clone(),
                stride: dec.planes[1].stride,
                width: dec.planes[1].width,
                height: dec.planes[1].height,
            },
            PlaneBuf {
                data: dec.planes[2].data.clone(),
                stride: dec.planes[2].stride,
                width: dec.planes[2].width,
                height: dec.planes[2].height,
            },
        ])
    } else {
        None
    };

    // CDEF (spec 7.15): applied whenever enable_cdef produced parameters
    // (a cdef_idx grid is only populated in that case; idx == -1 blocks and
    // all-skip blocks pass through unchanged).
    if seq.enable_cdef && !hdr.coded_lossless && !hdr.allow_intrabc {
        let input = super::cdef::CdefInput {
            hdr,
            sub_x,
            sub_y,
            num_planes: seq.num_planes as usize,
            mi_rows,
            mi_cols,
            skips: &dec.grids.skips,
            cdef_idx: &dec.grids.cdef_idx,
        };
        super::cdef::cdef_frame(&mut dec.planes, &input, forced_bands);
    }

    // Loop restoration (spec 7.17). No superres in this decoder, so
    // UpscaledCurrFrame == the deblocked frame and UpscaledCdefFrame == the
    // CDEF output.
    if hdr.lr.uses_lr {
        if let Some(ref pre) = pre_cdef {
            let apply = super::lr::LrApply {
                hdr,
                sub_x,
                sub_y,
                num_planes: seq.num_planes as usize,
                pre_cdef: pre,
                grids: &dec.lr_units,
            };
            super::lr::loop_restore_frame(&mut dec.planes, &apply, forced_bands);
        }
    }

    Ok(DecodedIntraFrame {
        planes: dec.planes,
        width: hdr.frame_width as usize,
        height: hdr.frame_height as usize,
    })
}

#[cfg(test)]
#[path = "recon_tests.rs"]
mod tests;
