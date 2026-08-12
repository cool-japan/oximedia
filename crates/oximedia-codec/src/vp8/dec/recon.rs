//! VP8 macroblock reconstruction (RFC 6386 §12-§14).
//!
//! Applies intra prediction and inverse transforms to turn decoded
//! coefficients (from [`super::residual`]) into reconstructed pixels: whole
//! 16x16 luma, per-4x4 B_PRED luma, and 8x8 chroma.

use super::predict::{predict_block, predict_subblock, SubBlockEdge};
use super::transform::{add_residual, idct4x4, iwht4x4};
use super::Decoder;

impl Decoder<'_> {
    /// Reconstructs a whole-16x16-predicted luma macroblock.
    pub(super) fn reconstruct_y16(
        &mut self,
        mb_x: usize,
        mb_y: usize,
        y_mode: usize,
        has_y2: bool,
        coeffs: &mut [[i32; 16]; 25],
    ) {
        let stride = self.planes.y_stride;
        let off = self.planes.y_origin + mb_y * 16 * stride + mb_x * 16;
        let have_up = mb_y > 0;
        let have_left = mb_x > 0;
        // Whole-block prediction over the 16x16 luma region.
        predict_block(
            &mut self.planes.y,
            off,
            stride,
            16,
            y_mode,
            have_up,
            have_left,
        );

        self.add_luma_residual(mb_x, mb_y, has_y2, coeffs);
    }

    /// Inverse-transforms and adds the sixteen 4x4 luma residual blocks of a
    /// macroblock whose 16x16 prediction is already in the plane.
    ///
    /// Split out of [`Decoder::reconstruct_y16`] because an inter macroblock
    /// runs exactly this half — dixie.c `fixup_dc_coeffs` plus the residual
    /// add inside `recon_1_block` (rfc6386.txt lines 12787-12790,
    /// 12181-12213) — on top of a motion-compensated prediction instead of an
    /// intra one.
    pub(super) fn add_luma_residual(
        &mut self,
        mb_x: usize,
        mb_y: usize,
        has_y2: bool,
        coeffs: &mut [[i32; 16]; 25],
    ) {
        let stride = self.planes.y_stride;
        let off = self.planes.y_origin + mb_y * 16 * stride + mb_x * 16;

        // If a Y2 block was decoded, inverse-WHT it and scatter DCs
        // (RFC 6386 §14.3: the Y2 block carries the 16 luma DC values).
        if has_y2 {
            let mut y2 = coeffs[24];
            iwht4x4(&mut y2);
            for sb in 0..16 {
                coeffs[sb][0] = y2[sb];
            }
        }

        // Inverse-transform and add each 4x4 luma sub-block.
        for r in 0..4 {
            for col in 0..4 {
                let sb = r * 4 + col;
                let mut blk = coeffs[sb];
                idct4x4(&mut blk);
                let sb_off = off + r * 4 * stride + col * 4;
                add_residual(&mut self.planes.y, sb_off, stride, &blk);
            }
        }
    }

    /// Reconstructs a B_PRED (per-4x4) luma macroblock.
    pub(super) fn reconstruct_bpred(
        &mut self,
        mb_x: usize,
        mb_y: usize,
        bmodes: &[u8; 16],
        coeffs: &[[i32; 16]; 25],
    ) {
        let stride = self.planes.y_stride;
        let mb_off = self.planes.y_origin + mb_y * 16 * stride + mb_x * 16;

        // VP8 above-right quirk: every col==3 sub-block — on EVERY sub-block
        // row — sees the SAME four above-right samples: the pixels
        // above-right of the whole macroblock (libvpx decodeframe.c
        // "propagate the above right state"; libwebp frame_dec.c "replicate
        // the top-right samples on the rows below"). They are the top border
        // (127) on the first macroblock row, and replicate the last
        // above-row pixel at the frame's right edge.
        let mb_top_right: [i32; 4] = if mb_y == 0 {
            [127; 4]
        } else {
            let above_row = mb_off - stride;
            if mb_x + 1 < self.mb_cols {
                let p = &self.planes.y;
                [
                    i32::from(p[above_row + 16]),
                    i32::from(p[above_row + 17]),
                    i32::from(p[above_row + 18]),
                    i32::from(p[above_row + 19]),
                ]
            } else {
                [i32::from(self.planes.y[above_row + 15]); 4]
            }
        };

        // Each 4x4 sub-block is predicted then immediately reconstructed so
        // later sub-blocks see the correct neighbours.
        for r in 0..4 {
            for col in 0..4 {
                let sb = r * 4 + col;
                let sb_off = mb_off + r * 4 * stride + col * 4;
                let have_up = mb_y > 0 || r > 0;
                let have_left = mb_x > 0 || col > 0;
                // Interior columns read their above-right samples straight
                // from the plane (already reconstructed); the rightmost
                // column uses the macroblock's shared above-right samples.
                let top_right = if col == 3 { Some(&mb_top_right) } else { None };
                let edge = self.gather_subblock_edge(sb_off, stride, have_up, have_left, top_right);
                predict_subblock(
                    &mut self.planes.y,
                    sb_off,
                    stride,
                    usize::from(bmodes[sb]),
                    &edge,
                );
                let mut blk = coeffs[sb];
                idct4x4(&mut blk);
                add_residual(&mut self.planes.y, sb_off, stride, &blk);
            }
        }
    }

    /// Gathers the 8 above + 4 left + corner edge samples for a 4x4 block.
    ///
    /// `top_right` overrides the four above-right samples; it is provided
    /// for col==3 sub-blocks, which all share the macroblock's above-right
    /// pixels (see `reconstruct_bpred`). For interior columns (`None`) the
    /// above-right samples are read from the plane, which is always valid
    /// once `have_up` holds: they belong to this macroblock's own above row
    /// or an already-reconstructed sub-block.
    fn gather_subblock_edge(
        &self,
        sb_off: usize,
        stride: usize,
        have_up: bool,
        have_left: bool,
        top_right: Option<&[i32; 4]>,
    ) -> SubBlockEdge {
        let plane = &self.planes.y;
        let mut above = [127i32; 8];
        let mut left = [129i32; 4];
        let mut corner = if have_up && have_left {
            i32::from(plane[sb_off - stride - 1])
        } else if have_up {
            129
        } else {
            127
        };
        if have_up {
            for (c, a) in above.iter_mut().take(4).enumerate() {
                *a = i32::from(plane[sb_off - stride + c]);
            }
            match top_right {
                Some(tr) => above[4..8].copy_from_slice(tr),
                None => {
                    for c in 4..8 {
                        above[c] = i32::from(plane[sb_off - stride + c]);
                    }
                }
            }
        } else {
            corner = 127;
            // On the frame's top edge the above-right samples are the top
            // border value (127) as well; `above` already holds that, and
            // the shared macroblock samples agree ([127; 4] when mb_y == 0).
            if let Some(tr) = top_right {
                above[4..8].copy_from_slice(tr);
            }
        }
        if have_left {
            for (r, l) in left.iter_mut().enumerate() {
                *l = i32::from(plane[sb_off + r * stride - 1]);
            }
        }
        SubBlockEdge {
            above,
            left,
            corner,
        }
    }

    /// Reconstructs both chroma planes of a macroblock.
    pub(super) fn reconstruct_chroma(
        &mut self,
        mb_x: usize,
        mb_y: usize,
        uv_mode: usize,
        coeffs: &[[i32; 16]; 25],
    ) {
        let stride = self.planes.uv_stride;
        let off = self.planes.uv_origin + mb_y * 8 * stride + mb_x * 8;
        let have_up = mb_y > 0;
        let have_left = mb_x > 0;

        for plane_sel in 0..2 {
            let plane = if plane_sel == 0 {
                &mut self.planes.u
            } else {
                &mut self.planes.v
            };
            predict_block(plane, off, stride, 8, uv_mode, have_up, have_left);
        }
        self.add_chroma_residual(mb_x, mb_y, coeffs);
    }

    /// Inverse-transforms and adds the 2x(2x2) 4x4 chroma residual blocks of
    /// a macroblock whose 8x8 chroma prediction is already in the planes.
    ///
    /// The inter-frame half of [`Decoder::reconstruct_chroma`]; see
    /// [`Decoder::add_luma_residual`].
    pub(super) fn add_chroma_residual(
        &mut self,
        mb_x: usize,
        mb_y: usize,
        coeffs: &[[i32; 16]; 25],
    ) {
        let stride = self.planes.uv_stride;
        let off = self.planes.uv_origin + mb_y * 8 * stride + mb_x * 8;

        for (plane_sel, base) in [(0usize, 16usize), (1usize, 20usize)] {
            let plane = if plane_sel == 0 {
                &mut self.planes.u
            } else {
                &mut self.planes.v
            };
            for r in 0..2 {
                for col in 0..2 {
                    let sb = base + r * 2 + col;
                    let mut blk = coeffs[sb];
                    idct4x4(&mut blk);
                    let sb_off = off + r * 4 * stride + col * 4;
                    add_residual(plane, sb_off, stride, &blk);
                }
            }
        }
    }
}
