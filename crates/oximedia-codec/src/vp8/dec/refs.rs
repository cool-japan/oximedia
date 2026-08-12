//! VP8 reference-frame surfaces and the decoded-picture buffer (RFC 6386
//! §9.7, §18).
//!
//! An inter frame predicts from one of three reference surfaces — "last",
//! "golden" and "altref". This module owns
//!
//! * [`RefSurface`]: a bordered YUV 4:2:0 surface laid out **exactly** like
//!   the in-progress reconstruction buffer ([`super::Planes`]) so a finished
//!   frame becomes a reference by a plain buffer move, with no re-striding;
//! * [`RefSurface::extend_borders`]: edge-pixel replication into the
//!   [`super::BORDER`]-pixel margin, which is what makes motion vectors that
//!   point outside the frame read defined pixels (RFC 6386 §18.2);
//! * [`Dpb`]: the three reference slots plus the end-of-frame update
//!   ([`Dpb::commit`]) that applies the frame header's refresh/copy flags.
//!
//! # Surface geometry
//!
//! [`super::Planes`] allocates macroblock-aligned planes with a symmetric
//! [`super::BORDER`] (= 32 px) margin:
//!
//! ```text
//! y_stride  = mb_cols * 16 + 2 * BORDER
//! y_origin  = BORDER * y_stride + BORDER      (offset of luma pixel (0,0))
//! y.len()   = y_stride * (mb_rows * 16 + 2 * BORDER)
//! ```
//!
//! and the chroma planes the same with `mb_cols * 8` / `mb_rows * 8`.
//! [`RefSurface`] mirrors that layout field-for-field.
//!
//! # What "inside the frame" means for motion compensation
//!
//! Border replication (and the bounds check in [`super::mc`]) works on the
//! **macroblock-aligned** area, not the visible crop: the decoder
//! reconstructs whole macroblocks, so the alignment padding holds real
//! reconstructed pixels that later frames legitimately predict from. The
//! reference decoder agrees — `predict_inter_emulated_edge` derives its
//! frame extent as `w = ctx->mb_cols * 16; h = ctx->mb_rows * 16;`
//! (rfc6386.txt lines 12281-12284), not from the visible width/height.
//! [`RefSurface::width`] / [`RefSurface::height`] keep the visible size for
//! the eventual crop; [`RefSurface::aligned_width`] /
//! [`RefSurface::aligned_height`] are what motion compensation sees.

//! Wave-2 package stub: populated by its implementation package (P3/P4/P5),
//! wired into the decode path and un-deadcoded at P6.
#![forbid(unsafe_code)]

use crate::error::{CodecError, CodecResult};
use std::sync::Arc;

use super::BORDER;

/// Which decoded-picture-buffer slot a macroblock (or a buffer-copy code)
/// refers to.
///
/// Deliberately named `RefSlot` rather than `RefFrame`: the per-macroblock
/// reference selector decoded in `dec::mode` is a *bitstream* concept, this
/// is a *storage* concept. P6 maps one onto the other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RefSlot {
    /// The previous frame (`LAST_FRAME`).
    Last,
    /// The golden frame (`GOLDEN_FRAME`).
    Golden,
    /// The alternate reference frame (`ALTREF_FRAME`).
    AltRef,
}

/// A bordered YUV 4:2:0 reference surface.
///
/// Field-for-field geometry twin of [`super::Planes`]; see the module docs.
pub(super) struct RefSurface {
    /// Luma plane, `y_stride * (aligned_height + 2 * BORDER)` bytes.
    pub(super) y: Vec<u8>,
    /// Chroma-blue plane.
    pub(super) u: Vec<u8>,
    /// Chroma-red plane.
    pub(super) v: Vec<u8>,
    /// Row length of the luma plane, borders included.
    pub(super) y_stride: usize,
    /// Row length of each chroma plane, borders included.
    pub(super) uv_stride: usize,
    /// Offset of luma pixel (0,0) inside [`RefSurface::y`].
    pub(super) y_origin: usize,
    /// Offset of chroma pixel (0,0) inside [`RefSurface::u`] / `v`.
    pub(super) uv_origin: usize,
    /// Visible luma width in pixels (may be smaller than `mb_cols * 16`).
    pub(super) width: usize,
    /// Visible luma height in pixels (may be smaller than `mb_rows * 16`).
    pub(super) height: usize,
    /// Macroblock columns.
    pub(super) mb_cols: usize,
    /// Macroblock rows.
    pub(super) mb_rows: usize,
}

impl RefSurface {
    /// Macroblock-aligned luma width — the extent motion compensation and
    /// border replication treat as "the frame" (rfc6386.txt lines
    /// 12281-12284).
    pub(super) fn aligned_width(&self) -> usize {
        self.mb_cols * 16
    }

    /// Macroblock-aligned luma height.
    pub(super) fn aligned_height(&self) -> usize {
        self.mb_rows * 16
    }

    /// Macroblock-aligned chroma width (`aligned_width / 2`).
    pub(super) fn aligned_uv_width(&self) -> usize {
        self.mb_cols * 8
    }

    /// Macroblock-aligned chroma height.
    pub(super) fn aligned_uv_height(&self) -> usize {
        self.mb_rows * 8
    }

    /// Consumes a finished reconstruction buffer, reusing its allocations.
    ///
    /// The zero-copy form of [`RefSurface::from_planes`]; borders are
    /// replicated before returning.
    ///
    /// # Errors
    /// Fails when `planes`'s geometry does not match `mb_cols`/`mb_rows`.
    pub(super) fn from_planes_owned(
        planes: super::Planes,
        width: usize,
        height: usize,
        mb_cols: usize,
        mb_rows: usize,
    ) -> CodecResult<Self> {
        let geom = SurfaceGeometry::new(mb_cols, mb_rows)?;
        geom.check_planes(&planes)?;
        let mut surface = Self {
            y: planes.y,
            u: planes.u,
            v: planes.v,
            y_stride: planes.y_stride,
            uv_stride: planes.uv_stride,
            y_origin: planes.y_origin,
            uv_origin: planes.uv_origin,
            width,
            height,
            mb_cols,
            mb_rows,
        };
        surface.extend_borders();
        Ok(surface)
    }

    /// Replicates the outermost row / column of each plane into the whole
    /// [`super::BORDER`]-pixel margin, corners included (RFC 6386 §18.2:
    /// motion vectors may point outside the frame, and the pixels they read
    /// there are the replicated edge pixels).
    ///
    /// Replication starts at the **macroblock-aligned** edge; see the module
    /// documentation for why.
    pub(super) fn extend_borders(&mut self) {
        let (aw, ah) = (self.aligned_width(), self.aligned_height());
        extend_plane(&mut self.y, self.y_stride, self.y_origin, aw, ah);
        let (cw, ch) = (self.aligned_uv_width(), self.aligned_uv_height());
        extend_plane(&mut self.u, self.uv_stride, self.uv_origin, cw, ch);
        extend_plane(&mut self.v, self.uv_stride, self.uv_origin, cw, ch);
    }
}

/// Derived buffer geometry for a macroblock-aligned bordered surface.
///
/// Exists so [`RefSurface`] and [`super::Planes`] can be checked against one
/// single description of the layout instead of two copies of the arithmetic.
struct SurfaceGeometry {
    y_stride: usize,
    uv_stride: usize,
    y_origin: usize,
    uv_origin: usize,
    y_len: usize,
    uv_len: usize,
}

impl SurfaceGeometry {
    /// Computes the layout for an `mb_cols` x `mb_rows` frame.
    fn new(mb_cols: usize, mb_rows: usize) -> CodecResult<Self> {
        if mb_cols == 0 || mb_rows == 0 {
            return Err(CodecError::InvalidBitstream(
                "VP8: zero macroblock dimension for reference surface".to_string(),
            ));
        }
        let overflow =
            || CodecError::InvalidBitstream("VP8: reference surface size overflow".to_string());
        let y_w = mb_cols.checked_mul(16).ok_or_else(overflow)?;
        let y_h = mb_rows.checked_mul(16).ok_or_else(overflow)?;
        let uv_w = mb_cols.checked_mul(8).ok_or_else(overflow)?;
        let uv_h = mb_rows.checked_mul(8).ok_or_else(overflow)?;
        let y_stride = y_w.checked_add(2 * BORDER).ok_or_else(overflow)?;
        let uv_stride = uv_w.checked_add(2 * BORDER).ok_or_else(overflow)?;
        let y_len = y_stride
            .checked_mul(y_h.checked_add(2 * BORDER).ok_or_else(overflow)?)
            .ok_or_else(overflow)?;
        let uv_len = uv_stride
            .checked_mul(uv_h.checked_add(2 * BORDER).ok_or_else(overflow)?)
            .ok_or_else(overflow)?;
        Ok(Self {
            y_stride,
            uv_stride,
            y_origin: BORDER * y_stride + BORDER,
            uv_origin: BORDER * uv_stride + BORDER,
            y_len,
            uv_len,
        })
    }

    /// Verifies that a reconstruction buffer really has this layout.
    fn check_planes(&self, planes: &super::Planes) -> CodecResult<()> {
        let ok = planes.y_stride == self.y_stride
            && planes.uv_stride == self.uv_stride
            && planes.y_origin == self.y_origin
            && planes.uv_origin == self.uv_origin
            && planes.y.len() == self.y_len
            && planes.u.len() == self.uv_len
            && planes.v.len() == self.uv_len;
        if ok {
            Ok(())
        } else {
            Err(CodecError::Internal(
                "VP8: reconstruction plane geometry does not match the reference surface"
                    .to_string(),
            ))
        }
    }
}

/// Replicates the edges of one `w` x `h` plane into its whole border.
///
/// Left/right first (so the extended rows are already complete), then the
/// full extended top and bottom rows — which is what fills the corners.
fn extend_plane(buf: &mut [u8], stride: usize, origin: usize, w: usize, h: usize) {
    if w == 0 || h == 0 || stride < w + 2 * BORDER || origin < BORDER * stride + BORDER {
        // Geometry is validated by `SurfaceGeometry`; a mismatch here would
        // mean a caller built a surface by hand. Do nothing rather than
        // index out of bounds.
        return;
    }
    let row_start = origin - BORDER;
    // --- left / right ---
    for row in 0..h {
        let line = origin + row * stride;
        let Some(&left) = buf.get(line) else { return };
        let Some(&right) = buf.get(line + w - 1) else {
            return;
        };
        let Some(slice) = buf.get_mut(line - BORDER..line) else {
            return;
        };
        slice.fill(left);
        let Some(slice) = buf.get_mut(line + w..line + w + BORDER) else {
            return;
        };
        slice.fill(right);
    }
    // --- top / bottom (full extended rows, corners included) ---
    let ext = w + 2 * BORDER;
    let first = row_start;
    let last = row_start + (h - 1) * stride;
    for b in 1..=BORDER {
        let (Some(src), Some(dst)) = (first.checked_sub(b * stride), last.checked_add(b * stride))
        else {
            return;
        };
        if src + ext > buf.len() || dst + ext > buf.len() {
            return;
        }
        buf.copy_within(first..first + ext, src);
        buf.copy_within(last..last + ext, dst);
    }
}

/// The decoded-picture buffer: the three reference slots a VP8 inter frame
/// may predict from.
#[derive(Default)]
pub(super) struct Dpb {
    /// The previous frame.
    pub(super) last: Option<Arc<RefSurface>>,
    /// The golden frame.
    pub(super) golden: Option<Arc<RefSurface>>,
    /// The alternate reference frame.
    pub(super) altref: Option<Arc<RefSurface>>,
}

impl Dpb {
    /// An empty buffer (all three slots unset), as before the first key
    /// frame.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Borrows a reference slot.
    ///
    /// # Errors
    /// Fails when the slot has never been filled — a bitstream that predicts
    /// from a reference the stream never produced.
    pub(super) fn get(&self, slot: RefSlot) -> CodecResult<&Arc<RefSurface>> {
        let entry = match slot {
            RefSlot::Last => &self.last,
            RefSlot::Golden => &self.golden,
            RefSlot::AltRef => &self.altref,
        };
        entry.as_ref().ok_or_else(|| {
            CodecError::InvalidBitstream(format!(
                "VP8: frame references the {slot:?} buffer, which has never been decoded"
            ))
        })
    }

    /// Applies the end-of-frame reference-buffer update (RFC 6386 §9.7).
    ///
    /// # Copy codes
    ///
    /// Per `decode_reference_header` (rfc6386.txt lines 7793-7806) the copy
    /// codes are read only when the matching refresh flag is clear, and the
    /// update itself (rfc6386.txt lines 8211-8266) reads:
    ///
    /// | code | `copy_to_golden` | `copy_to_alt` |
    /// |------|------------------|---------------|
    /// | 0    | no copy          | no copy       |
    /// | 1    | golden <- last   | altref <- last|
    /// | 2    | golden <- altref | altref <- golden |
    ///
    /// Note the asymmetry: code 2 means "the *other* long-term reference" in
    /// both cases, so it is `altref` for golden and `golden` for altref.
    ///
    /// # Evaluation order — this is *not* a transactional swap
    ///
    /// The reference decoder performs the update **sequentially**, altref
    /// copy first, then golden copy, then the three refreshes (rfc6386.txt
    /// lines 8211-8266; libvpx `swap_frame_buffers` in
    /// `vp8/decoder/onyxd_if.c` is in the same order). So the "exchange
    /// golden and altref" bit pattern `copy_to_alt == 2 && copy_to_golden ==
    /// 2` does **not** exchange them: altref takes the old golden first, and
    /// the golden copy then reads that already-updated altref, leaving
    /// golden unchanged. Bit-exact decoding requires reproducing exactly
    /// that, so this function resolves the copies in the reference decoder's
    /// order (see the `copy_alt2_and_copy_golden2_is_not_a_swap` test).
    ///
    /// What *is* transactional is the write-back: every slot is resolved
    /// into a local before any field of `self` is assigned, so a partially
    /// applied update can never be observed and no slot can alias a
    /// half-updated neighbour.
    ///
    /// # Key frames
    ///
    /// A key frame fills all three slots with the new frame regardless of
    /// the flags — `decode_reference_header` forces `refresh_gf`,
    /// `refresh_arf` and `refresh_last` to 1 and both copy codes to 0 for a
    /// key frame (rfc6386.txt lines 7797-7806). `is_keyframe` re-applies
    /// that here so a caller cannot get it wrong.
    ///
    /// # Errors
    /// Fails when a copy code names a slot that has never been filled, or
    /// when a copy code is outside `0..=2`. Post-key-frame every slot is
    /// filled, so a well-formed stream never hits either.
    pub(super) fn commit(
        &mut self,
        new_frame: Arc<RefSurface>,
        is_keyframe: bool,
        refresh_last: bool,
        refresh_golden: bool,
        refresh_alt: bool,
        copy_to_golden: u8,
        copy_to_alt: u8,
    ) -> CodecResult<()> {
        if is_keyframe {
            self.last = Some(Arc::clone(&new_frame));
            self.golden = Some(Arc::clone(&new_frame));
            self.altref = Some(new_frame);
            return Ok(());
        }

        // Staged copies of the three slots, resolved in the reference
        // decoder's order; `self` is untouched until every branch has run.
        let mut staged_last = self.last.clone();
        let mut staged_golden = self.golden.clone();
        let mut staged_alt = self.altref.clone();

        // 1. altref copy (rfc6386.txt lines 8212-8223), read against the
        //    pre-update buffer.
        if !refresh_alt {
            match copy_to_alt {
                0 => {}
                1 => staged_alt = Some(Arc::clone(copy_source(&staged_last, "last", "altref")?)),
                2 => {
                    staged_alt = Some(Arc::clone(copy_source(&staged_golden, "golden", "altref")?));
                }
                other => return Err(bad_copy_code("altref", other)),
            }
        }

        // 2. golden copy (rfc6386.txt lines 8225-8246). Deliberately reads
        //    `staged_alt`, i.e. the altref slot *after* step 1 — that is what
        //    the reference decoder does.
        if !refresh_golden {
            match copy_to_golden {
                0 => {}
                1 => staged_golden = Some(Arc::clone(copy_source(&staged_last, "last", "golden")?)),
                2 => {
                    staged_golden = Some(Arc::clone(copy_source(&staged_alt, "altref", "golden")?));
                }
                other => return Err(bad_copy_code("golden", other)),
            }
        }

        // 3. refreshes from the frame just decoded (rfc6386.txt lines
        //    8248-8266). A refresh always wins over its copy code.
        if refresh_golden {
            staged_golden = Some(Arc::clone(&new_frame));
        }
        if refresh_alt {
            staged_alt = Some(Arc::clone(&new_frame));
        }
        if refresh_last {
            staged_last = Some(new_frame);
        }

        self.last = staged_last;
        self.golden = staged_golden;
        self.altref = staged_alt;
        Ok(())
    }
}

/// Borrows a buffer-copy source slot, or reports the empty slot honestly.
fn copy_source<'a>(
    slot: &'a Option<Arc<RefSurface>>,
    source: &str,
    target: &str,
) -> CodecResult<&'a Arc<RefSurface>> {
    slot.as_ref().ok_or_else(|| {
        CodecError::InvalidBitstream(format!(
            "VP8: buffer copy into {target} names the {source} buffer, which has never been decoded"
        ))
    })
}

/// Builds the error for a copy code outside `0..=2`.
fn bad_copy_code(target: &str, code: u8) -> CodecError {
    CodecError::InvalidBitstream(format!("VP8: invalid buffer-copy code {code} for {target}"))
}

#[cfg(test)]
mod tests {
    // Test-only constructors, visible to the whole `dec` test tree (the
    // sibling `mc` tests build fixtures with them too). The decode path only
    // ever builds a reference surface *from a just-reconstructed frame*
    // (`from_planes_owned`, the zero-copy form) — a VP8 sequence always opens
    // with a key frame, which fills all three slots — so a blank surface and
    // a cloning constructor have no production caller and are scoped to the
    // tests that do use them rather than left as unused production API.
    impl RefSurface {
        /// Allocates a blank surface for a `width` x `height` frame.
        ///
        /// The fill value (129) matches `super::super::Planes`'s initial fill so an
        /// as-yet-unwritten reference behaves like an unwritten reconstruction
        /// buffer. In practice the value never reaches the output: the first
        /// frame of a stream is a key frame, which fills all three slots.
        ///
        /// # Errors
        /// Fails when either dimension is zero or the allocation size overflows.
        pub(in crate::vp8::dec) fn new(width: usize, height: usize) -> CodecResult<Self> {
            if width == 0 || height == 0 {
                return Err(CodecError::InvalidBitstream(
                    "VP8: zero frame dimension for reference surface".to_string(),
                ));
            }
            let mb_cols = width.div_ceil(16);
            let mb_rows = height.div_ceil(16);
            Self::with_mb_dims(width, height, mb_cols, mb_rows)
        }

        /// Allocates a blank surface with explicit macroblock dimensions.
        ///
        /// # Errors
        /// Fails when a dimension is zero or the buffer size overflows `usize`.
        fn with_mb_dims(
            width: usize,
            height: usize,
            mb_cols: usize,
            mb_rows: usize,
        ) -> CodecResult<Self> {
            let geom = SurfaceGeometry::new(mb_cols, mb_rows)?;
            Ok(Self {
                y: vec![129u8; geom.y_len],
                u: vec![129u8; geom.uv_len],
                v: vec![129u8; geom.uv_len],
                y_stride: geom.y_stride,
                uv_stride: geom.uv_stride,
                y_origin: geom.y_origin,
                uv_origin: geom.uv_origin,
                width,
                height,
                mb_cols,
                mb_rows,
            })
        }

        /// Copies a finished reconstruction buffer into a new reference surface
        /// and replicates its borders.
        ///
        /// `planes` must have been allocated by `Decoder::new` for the same
        /// `mb_cols` x `mb_rows` frame; the geometry is re-derived and checked
        /// rather than trusted.
        ///
        /// # Errors
        /// Fails when `planes`'s geometry does not match `mb_cols`/`mb_rows`.
        pub(in crate::vp8::dec) fn from_planes(
            planes: &super::super::Planes,
            width: usize,
            height: usize,
            mb_cols: usize,
            mb_rows: usize,
        ) -> CodecResult<Self> {
            let geom = SurfaceGeometry::new(mb_cols, mb_rows)?;
            geom.check_planes(planes)?;
            let mut surface = Self {
                y: planes.y.clone(),
                u: planes.u.clone(),
                v: planes.v.clone(),
                y_stride: planes.y_stride,
                uv_stride: planes.uv_stride,
                y_origin: planes.y_origin,
                uv_origin: planes.uv_origin,
                width,
                height,
                mb_cols,
                mb_rows,
            };
            surface.extend_borders();
            Ok(surface)
        }
    }

    use super::*;

    /// Builds a surface whose macroblock-aligned area holds an asymmetric,
    /// position-dependent pattern (so a wrong replication direction cannot
    /// accidentally pass).
    fn patterned_surface(mb_cols: usize, mb_rows: usize) -> RefSurface {
        let Ok(mut s) = RefSurface::with_mb_dims(mb_cols * 16, mb_rows * 16, mb_cols, mb_rows)
        else {
            unreachable!("valid macroblock dimensions")
        };
        for row in 0..s.aligned_height() {
            for col in 0..s.aligned_width() {
                s.y[s.y_origin + row * s.y_stride + col] = ((row * 7 + col * 3 + 1) % 251) as u8;
            }
        }
        for row in 0..s.aligned_uv_height() {
            for col in 0..s.aligned_uv_width() {
                s.u[s.uv_origin + row * s.uv_stride + col] = ((row * 5 + col * 11 + 2) % 241) as u8;
                s.v[s.uv_origin + row * s.uv_stride + col] = ((row * 13 + col * 2 + 3) % 239) as u8;
            }
        }
        s
    }

    #[test]
    fn test_geometry_mirrors_planes() {
        let Ok(geom) = SurfaceGeometry::new(3, 2) else {
            unreachable!("valid dims")
        };
        // Same arithmetic as `Decoder::new` in dec/mod.rs.
        assert_eq!(geom.y_stride, 3 * 16 + 2 * BORDER);
        assert_eq!(geom.uv_stride, 3 * 8 + 2 * BORDER);
        assert_eq!(geom.y_origin, BORDER * geom.y_stride + BORDER);
        assert_eq!(geom.uv_origin, BORDER * geom.uv_stride + BORDER);
        assert_eq!(geom.y_len, geom.y_stride * (2 * 16 + 2 * BORDER));
        assert_eq!(geom.uv_len, geom.uv_stride * (2 * 8 + 2 * BORDER));
    }

    #[test]
    fn test_new_rejects_zero_dimension() {
        assert!(RefSurface::new(0, 16).is_err());
        assert!(RefSurface::new(16, 0).is_err());
    }

    #[test]
    fn test_extend_borders_leaves_interior_untouched() {
        let mut s = patterned_surface(2, 2);
        let before = s.y.clone();
        s.extend_borders();
        for row in 0..s.aligned_height() {
            for col in 0..s.aligned_width() {
                let i = s.y_origin + row * s.y_stride + col;
                assert_eq!(s.y[i], before[i], "interior pixel ({row},{col}) changed");
            }
        }
    }

    #[test]
    fn test_extend_borders_replicates_left_and_right() {
        let mut s = patterned_surface(2, 2);
        s.extend_borders();
        let (w, h) = (s.aligned_width(), s.aligned_height());
        for row in 0..h {
            let line = s.y_origin + row * s.y_stride;
            let left = s.y[line];
            let right = s.y[line + w - 1];
            for b in 1..=BORDER {
                assert_eq!(s.y[line - b], left, "left border row {row} offset {b}");
                assert_eq!(
                    s.y[line + w - 1 + b],
                    right,
                    "right border row {row} off {b}"
                );
            }
        }
    }

    #[test]
    fn test_extend_borders_replicates_top_and_bottom() {
        let mut s = patterned_surface(2, 2);
        s.extend_borders();
        let (w, h) = (s.aligned_width(), s.aligned_height());
        for col in 0..w {
            let top = s.y[s.y_origin + col];
            let bottom = s.y[s.y_origin + (h - 1) * s.y_stride + col];
            for b in 1..=BORDER {
                assert_eq!(
                    s.y[s.y_origin + col - b * s.y_stride],
                    top,
                    "top border col {col} offset {b}"
                );
                assert_eq!(
                    s.y[s.y_origin + (h - 1 + b) * s.y_stride + col],
                    bottom,
                    "bottom border col {col} offset {b}"
                );
            }
        }
    }

    #[test]
    fn test_extend_borders_fills_corners() {
        let mut s = patterned_surface(2, 2);
        s.extend_borders();
        let (w, h) = (s.aligned_width(), s.aligned_height());
        let tl = s.y[s.y_origin];
        let tr = s.y[s.y_origin + w - 1];
        let bl = s.y[s.y_origin + (h - 1) * s.y_stride];
        let br = s.y[s.y_origin + (h - 1) * s.y_stride + w - 1];
        for dy in 1..=BORDER {
            for dx in 1..=BORDER {
                let up = s.y_origin - dy * s.y_stride;
                let down = s.y_origin + (h - 1 + dy) * s.y_stride;
                assert_eq!(s.y[up - dx], tl, "top-left ({dx},{dy})");
                assert_eq!(s.y[up + w - 1 + dx], tr, "top-right ({dx},{dy})");
                assert_eq!(s.y[down - dx], bl, "bottom-left ({dx},{dy})");
                assert_eq!(s.y[down + w - 1 + dx], br, "bottom-right ({dx},{dy})");
            }
        }
    }

    #[test]
    fn test_extend_borders_covers_chroma_planes() {
        let mut s = patterned_surface(2, 3);
        s.extend_borders();
        let (w, h) = (s.aligned_uv_width(), s.aligned_uv_height());
        for (plane, name) in [(&s.u, "u"), (&s.v, "v")] {
            for row in 0..h {
                let line = s.uv_origin + row * s.uv_stride;
                assert_eq!(plane[line - BORDER], plane[line], "{name} left row {row}");
                assert_eq!(
                    plane[line + w - 1 + BORDER],
                    plane[line + w - 1],
                    "{name} right row {row}"
                );
            }
            let top_left = plane[s.uv_origin];
            assert_eq!(
                plane[s.uv_origin - BORDER * s.uv_stride - BORDER],
                top_left,
                "{name} top-left corner"
            );
        }
    }

    #[test]
    fn test_extend_borders_is_idempotent() {
        let mut s = patterned_surface(2, 2);
        s.extend_borders();
        let once = s.y.clone();
        s.extend_borders();
        assert_eq!(s.y, once, "border extension must be idempotent");
    }

    /// Builds a reconstruction buffer exactly as `Decoder::new` does, with a
    /// position-dependent pattern in the macroblock-aligned area.
    fn patterned_planes(mb_cols: usize, mb_rows: usize) -> super::super::Planes {
        let y_stride = mb_cols * 16 + 2 * BORDER;
        let uv_stride = mb_cols * 8 + 2 * BORDER;
        let y_origin = BORDER * y_stride + BORDER;
        let uv_origin = BORDER * uv_stride + BORDER;
        let mut planes = super::super::Planes {
            y: vec![129u8; y_stride * (mb_rows * 16 + 2 * BORDER)],
            u: vec![129u8; uv_stride * (mb_rows * 8 + 2 * BORDER)],
            v: vec![129u8; uv_stride * (mb_rows * 8 + 2 * BORDER)],
            y_stride,
            uv_stride,
            y_origin,
            uv_origin,
        };
        for row in 0..mb_rows * 16 {
            for col in 0..mb_cols * 16 {
                planes.y[y_origin + row * y_stride + col] = ((row * 3 + col * 5 + 1) % 253) as u8;
            }
        }
        for row in 0..mb_rows * 8 {
            for col in 0..mb_cols * 8 {
                planes.u[uv_origin + row * uv_stride + col] = ((row + col * 7 + 2) % 251) as u8;
                planes.v[uv_origin + row * uv_stride + col] = ((row * 9 + col + 3) % 247) as u8;
            }
        }
        planes
    }

    #[test]
    fn test_from_planes_copies_pixels_and_extends_borders() {
        let planes = patterned_planes(3, 2);
        let Ok(surface) = RefSurface::from_planes(&planes, 40, 20, 3, 2) else {
            unreachable!("geometry matches")
        };
        assert_eq!(surface.width, 40);
        assert_eq!(surface.height, 20);
        assert_eq!(surface.aligned_width(), 48);
        assert_eq!(surface.aligned_height(), 32);
        assert_eq!(surface.y_stride, planes.y_stride);
        assert_eq!(surface.y_origin, planes.y_origin);
        // Interior is a verbatim copy...
        for row in 0..surface.aligned_height() {
            for col in 0..surface.aligned_width() {
                let i = surface.y_origin + row * surface.y_stride + col;
                assert_eq!(surface.y[i], planes.y[i], "luma ({row},{col})");
            }
        }
        // ...and the border is already replicated, unlike in `planes`.
        let line = surface.y_origin;
        assert_eq!(surface.y[line - 1], surface.y[line]);
        assert_eq!(
            surface.u[surface.uv_origin - 1],
            surface.u[surface.uv_origin]
        );
        assert_eq!(
            surface.v[surface.uv_origin - 1],
            surface.v[surface.uv_origin]
        );
    }

    #[test]
    fn test_from_planes_owned_matches_the_borrowing_form() {
        let planes = patterned_planes(2, 2);
        let Ok(copied) = RefSurface::from_planes(&planes, 32, 32, 2, 2) else {
            unreachable!("geometry matches")
        };
        let Ok(moved) = RefSurface::from_planes_owned(planes, 32, 32, 2, 2) else {
            unreachable!("geometry matches")
        };
        assert_eq!(copied.y, moved.y);
        assert_eq!(copied.u, moved.u);
        assert_eq!(copied.v, moved.v);
    }

    #[test]
    fn test_from_planes_rejects_mismatched_geometry() {
        let planes = patterned_planes(3, 2);
        // Claiming the wrong macroblock dimensions must not silently
        // reinterpret the buffer.
        assert!(matches!(
            RefSurface::from_planes(&planes, 48, 32, 2, 2),
            Err(CodecError::Internal(_))
        ));
        assert!(matches!(
            RefSurface::from_planes(&planes, 48, 32, 3, 3),
            Err(CodecError::Internal(_))
        ));
        assert!(RefSurface::from_planes(&planes, 48, 32, 0, 2).is_err());
    }

    /// A surface tagged by its luma origin pixel, so the DPB tests can tell
    /// the frames apart.
    fn tagged(tag: u8) -> Arc<RefSurface> {
        let Ok(mut s) = RefSurface::new(16, 16) else {
            unreachable!("valid dims")
        };
        s.y[s.y_origin] = tag;
        Arc::new(s)
    }

    /// The tag of a slot, or `None` when the slot is empty.
    fn tag_of(slot: &Option<Arc<RefSurface>>) -> Option<u8> {
        slot.as_ref().map(|s| s.y[s.y_origin])
    }

    #[test]
    fn test_keyframe_fills_all_slots() {
        let mut dpb = Dpb::new();
        // Even with every flag cleared, a key frame fills all three.
        let Ok(()) = dpb.commit(tagged(7), true, false, false, false, 0, 0) else {
            unreachable!("keyframe commit cannot fail")
        };
        assert_eq!(tag_of(&dpb.last), Some(7));
        assert_eq!(tag_of(&dpb.golden), Some(7));
        assert_eq!(tag_of(&dpb.altref), Some(7));
    }

    #[test]
    fn test_refresh_flags_select_slots() {
        let mut dpb = Dpb::new();
        assert!(dpb.commit(tagged(1), true, true, true, true, 0, 0).is_ok());
        // Refresh last only.
        assert!(dpb
            .commit(tagged(2), false, true, false, false, 0, 0)
            .is_ok());
        assert_eq!(tag_of(&dpb.last), Some(2));
        assert_eq!(tag_of(&dpb.golden), Some(1));
        assert_eq!(tag_of(&dpb.altref), Some(1));
        // Refresh golden only.
        assert!(dpb
            .commit(tagged(3), false, false, true, false, 0, 0)
            .is_ok());
        assert_eq!(tag_of(&dpb.last), Some(2));
        assert_eq!(tag_of(&dpb.golden), Some(3));
        assert_eq!(tag_of(&dpb.altref), Some(1));
        // Refresh altref only.
        assert!(dpb
            .commit(tagged(4), false, false, false, true, 0, 0)
            .is_ok());
        assert_eq!(tag_of(&dpb.last), Some(2));
        assert_eq!(tag_of(&dpb.golden), Some(3));
        assert_eq!(tag_of(&dpb.altref), Some(4));
        // No flags at all: nothing moves.
        assert!(dpb
            .commit(tagged(5), false, false, false, false, 0, 0)
            .is_ok());
        assert_eq!(tag_of(&dpb.last), Some(2));
        assert_eq!(tag_of(&dpb.golden), Some(3));
        assert_eq!(tag_of(&dpb.altref), Some(4));
    }

    #[test]
    fn test_copy_codes_1_take_the_last_frame() {
        let mut dpb = Dpb::new();
        assert!(dpb.commit(tagged(1), true, true, true, true, 0, 0).is_ok());
        assert!(dpb
            .commit(tagged(2), false, true, false, false, 0, 0)
            .is_ok());
        // golden <- last, altref <- last.
        assert!(dpb
            .commit(tagged(3), false, false, false, false, 1, 1)
            .is_ok());
        assert_eq!(tag_of(&dpb.last), Some(2), "last must not move");
        assert_eq!(tag_of(&dpb.golden), Some(2));
        assert_eq!(tag_of(&dpb.altref), Some(2));
    }

    #[test]
    fn test_copy_code_2_targets_the_other_long_term_reference() {
        // golden <- altref (rfc6386.txt lines 8239-8246).
        let mut dpb = Dpb::new();
        assert!(dpb.commit(tagged(1), true, true, true, true, 0, 0).is_ok());
        assert!(dpb
            .commit(tagged(9), false, false, false, true, 0, 0)
            .is_ok());
        assert_eq!(tag_of(&dpb.altref), Some(9));
        assert!(dpb
            .commit(tagged(3), false, false, false, false, 2, 0)
            .is_ok());
        assert_eq!(tag_of(&dpb.golden), Some(9), "golden <- altref");

        // altref <- golden (rfc6386.txt lines 8218-8223).
        let mut dpb = Dpb::new();
        assert!(dpb.commit(tagged(1), true, true, true, true, 0, 0).is_ok());
        assert!(dpb
            .commit(tagged(8), false, false, true, false, 0, 0)
            .is_ok());
        assert_eq!(tag_of(&dpb.golden), Some(8));
        assert!(dpb
            .commit(tagged(3), false, false, false, false, 0, 2)
            .is_ok());
        assert_eq!(tag_of(&dpb.altref), Some(8), "altref <- golden");
    }

    /// The bit pattern that *looks* like "exchange golden and altref" is not
    /// an exchange: the reference decoder applies the altref copy first and
    /// the golden copy then reads the already-updated altref (rfc6386.txt
    /// lines 8211-8246; libvpx `swap_frame_buffers`). Both slots end up
    /// holding the old golden frame.
    #[test]
    fn test_copy_alt2_and_copy_golden2_is_not_a_swap() {
        let mut dpb = Dpb::new();
        assert!(dpb.commit(tagged(1), true, true, true, true, 0, 0).is_ok());
        assert!(dpb
            .commit(tagged(10), false, false, true, false, 0, 0)
            .is_ok()); // golden = 10
        assert!(dpb
            .commit(tagged(20), false, false, false, true, 0, 0)
            .is_ok()); // altref = 20
        assert_eq!(tag_of(&dpb.golden), Some(10));
        assert_eq!(tag_of(&dpb.altref), Some(20));

        assert!(dpb
            .commit(tagged(30), false, false, false, false, 2, 2)
            .is_ok());
        assert_eq!(tag_of(&dpb.altref), Some(10), "altref takes the old golden");
        assert_eq!(
            tag_of(&dpb.golden),
            Some(10),
            "golden reads the already-updated altref, so it does not change"
        );
    }

    /// The distinction the sequential order creates: `altref <- last` then
    /// `golden <- altref` chains the last frame into *both*, whereas a
    /// transactional read would have left golden at its old value.
    #[test]
    fn test_copy_chain_differs_from_transactional_reads() {
        let mut dpb = Dpb::new();
        assert!(dpb.commit(tagged(1), true, true, true, true, 0, 0).is_ok());
        assert!(dpb
            .commit(tagged(40), false, true, false, false, 0, 0)
            .is_ok()); // last = 40
        assert!(dpb
            .commit(tagged(50), false, false, false, true, 0, 0)
            .is_ok()); // altref = 50
        assert_eq!(tag_of(&dpb.golden), Some(1));

        // copy_to_alt = 1 (altref <- last), copy_to_golden = 2 (golden <- altref).
        assert!(dpb
            .commit(tagged(60), false, false, false, false, 2, 1)
            .is_ok());
        assert_eq!(tag_of(&dpb.altref), Some(40));
        assert_eq!(
            tag_of(&dpb.golden),
            Some(40),
            "golden must see the altref updated earlier in the same commit, not the old 50"
        );
    }

    #[test]
    fn test_refresh_overrides_its_copy_code() {
        // A well-formed bitstream never codes both, but `commit` is
        // fuzz-reachable, so the precedence must be defined.
        let mut dpb = Dpb::new();
        assert!(dpb.commit(tagged(1), true, true, true, true, 0, 0).is_ok());
        assert!(dpb
            .commit(tagged(70), false, true, false, false, 0, 0)
            .is_ok()); // last = 70
        assert!(dpb
            .commit(tagged(80), false, false, true, true, 1, 1)
            .is_ok());
        assert_eq!(tag_of(&dpb.golden), Some(80), "refresh wins over copy");
        assert_eq!(tag_of(&dpb.altref), Some(80), "refresh wins over copy");
    }

    #[test]
    fn test_copy_from_empty_slot_is_an_error() {
        let mut dpb = Dpb::new();
        // Nothing has ever been decoded: golden <- last is impossible.
        let err = dpb.commit(tagged(1), false, false, false, false, 1, 0);
        assert!(matches!(err, Err(CodecError::InvalidBitstream(_))));
        // And the failed commit must not have modified the buffer.
        assert!(dpb.last.is_none() && dpb.golden.is_none() && dpb.altref.is_none());
    }

    #[test]
    fn test_invalid_copy_code_is_an_error() {
        let mut dpb = Dpb::new();
        assert!(dpb.commit(tagged(1), true, true, true, true, 0, 0).is_ok());
        assert!(matches!(
            dpb.commit(tagged(2), false, false, false, false, 3, 0),
            Err(CodecError::InvalidBitstream(_))
        ));
        assert!(matches!(
            dpb.commit(tagged(2), false, false, false, false, 0, 3),
            Err(CodecError::InvalidBitstream(_))
        ));
    }

    #[test]
    fn test_get_reports_missing_slots() {
        let mut dpb = Dpb::new();
        assert!(matches!(
            dpb.get(RefSlot::Golden),
            Err(CodecError::InvalidBitstream(_))
        ));
        assert!(dpb.commit(tagged(5), true, true, true, true, 0, 0).is_ok());
        for slot in [RefSlot::Last, RefSlot::Golden, RefSlot::AltRef] {
            let Ok(s) = dpb.get(slot) else {
                unreachable!("slot filled by the key frame")
            };
            assert_eq!(s.y[s.y_origin], 5);
        }
    }

    #[test]
    fn test_slots_share_one_allocation() {
        // A key frame points all three slots at the same surface; the DPB
        // must not have cloned the pixel data three times.
        let mut dpb = Dpb::new();
        let frame = tagged(3);
        assert!(dpb
            .commit(Arc::clone(&frame), true, true, true, true, 0, 0)
            .is_ok());
        assert_eq!(Arc::strong_count(&frame), 4, "one local + three slots");
        let (Some(l), Some(g)) = (dpb.last.as_ref(), dpb.golden.as_ref()) else {
            unreachable!("filled by the key frame")
        };
        assert!(Arc::ptr_eq(l, g));
    }
}
