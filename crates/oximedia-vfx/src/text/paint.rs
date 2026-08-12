//! Coverage masks, outline dilation and alpha compositing for rendered text.
//!
//! Text is painted in three passes over a single 8-bit coverage mask built from
//! the laid-out glyphs: drop shadow, then outline, then fill. Working from one
//! mask (rather than re-rasterizing per pass) is what keeps the outline
//! concentric with the glyphs and the shadow an exact copy of the visible
//! shape.
//!
//! Every function here is pure over plain buffers, so masks, dilation and
//! blending are testable with hand-built synthetic glyphs — no font required.

use crate::{Color, Frame, VfxError, VfxResult};

use super::font::{GlyphBitmap, GlyphCache};
use super::layout::PlacedGlyph;

/// Widest outline supported, in pixels.
///
/// Dilation costs `O(width * height * radius^2)`, so the radius is clamped
/// here rather than letting an absurd `outline_width` stall a render.
pub const MAX_OUTLINE_RADIUS: f32 = 64.0;

/// Largest coverage mask that will be allocated, in pixels.
///
/// A text block bigger than this is a configuration error (a font size far
/// larger than the frame), reported honestly instead of attempting the
/// allocation.
pub const MAX_MASK_PIXELS: usize = 64 << 20;

/// An 8-bit coverage mask positioned relative to a text block's origin.
///
/// `0` is "no ink", `255` is "fully covered". The mask usually extends a
/// little beyond the block box: glyphs may overhang their advance, and outline
/// dilation needs padding to grow into.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverageMask {
    data: Vec<u8>,
    width: usize,
    height: usize,
    origin_x: f32,
    origin_y: f32,
}

impl CoverageMask {
    /// Allocate an empty mask whose top-left pixel sits at `(origin_x,
    /// origin_y)` relative to the block origin.
    ///
    /// # Errors
    ///
    /// Returns [`VfxError::InvalidDimensions`] if the mask would exceed
    /// [`MAX_MASK_PIXELS`].
    pub fn new(width: usize, height: usize, origin_x: f32, origin_y: f32) -> VfxResult<Self> {
        let pixels = width.saturating_mul(height);
        if pixels > MAX_MASK_PIXELS {
            return Err(VfxError::InvalidDimensions {
                width: width.min(u32::MAX as usize) as u32,
                height: height.min(u32::MAX as usize) as u32,
            });
        }
        Ok(Self {
            data: vec![0; pixels],
            width,
            height,
            origin_x,
            origin_y,
        })
    }

    /// Mask width in pixels.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Mask height in pixels.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Position of the mask's top-left pixel relative to the block origin.
    #[must_use]
    pub const fn origin(&self) -> (f32, f32) {
        (self.origin_x, self.origin_y)
    }

    /// The raw coverage buffer, row-major.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Coverage at `(x, y)`, or `0` outside the mask.
    #[must_use]
    pub fn coverage_at(&self, x: usize, y: usize) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.data.get(y * self.width + x).copied().unwrap_or(0)
    }

    /// `true` when the mask has no pixels at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Total number of pixels carrying any coverage.
    #[must_use]
    pub fn covered_pixels(&self) -> usize {
        self.data.iter().filter(|&&v| v > 0).count()
    }

    /// Composite one glyph bitmap into the mask with its top-left corner at
    /// `(x, y)` in mask space, keeping the maximum coverage where glyphs
    /// overlap (kerned pairs and accents must not brighten each other).
    pub fn blit_max(&mut self, glyph: &GlyphBitmap, x: i32, y: i32) {
        for gy in 0..glyph.height {
            let my = y + gy as i32;
            if my < 0 || my as usize >= self.height {
                continue;
            }
            let row = my as usize * self.width;
            for gx in 0..glyph.width {
                let mx = x + gx as i32;
                if mx < 0 || mx as usize >= self.width {
                    continue;
                }
                let cov = glyph.coverage_at(gx, gy);
                if cov == 0 {
                    continue;
                }
                let slot = &mut self.data[row + mx as usize];
                *slot = (*slot).max(cov);
            }
        }
    }
}

/// Build the coverage mask for a laid-out text block.
///
/// `pad` pixels of empty margin are reserved on every side for outline
/// dilation to grow into, and the mask is expanded further wherever a glyph's
/// bitmap overhangs the block box.
///
/// # Errors
///
/// Returns [`VfxError::InvalidDimensions`] if the resulting mask would exceed
/// [`MAX_MASK_PIXELS`].
pub fn build_mask(
    cache: &mut GlyphCache,
    glyphs: &[PlacedGlyph],
    size: f32,
    block_width: f32,
    block_height: f32,
    pad: f32,
) -> VfxResult<CoverageMask> {
    // Pass 1: rasterize everything once so the measuring and blitting passes
    // can read the cache immutably.
    for g in glyphs {
        let _ = cache.glyph(g.c, size);
    }

    // Pass 2: union of the block box and every glyph bitmap.
    let mut min_x = 0.0_f32;
    let mut min_y = 0.0_f32;
    let mut max_x = block_width.max(0.0);
    let mut max_y = block_height.max(0.0);
    for g in glyphs {
        let Some(bitmap) = cache.peek(g.c, size) else {
            continue;
        };
        if bitmap.is_blank() {
            continue;
        }
        let (gx, gy) = bitmap.top_left(g.pen_x, g.baseline_y);
        min_x = min_x.min(gx);
        min_y = min_y.min(gy);
        max_x = max_x.max(gx + bitmap.width as f32);
        max_y = max_y.max(gy + bitmap.height as f32);
    }

    let pad = pad.max(0.0);
    let origin_x = (min_x - pad).floor();
    let origin_y = (min_y - pad).floor();
    let width = ((max_x + pad).ceil() - origin_x).max(0.0) as usize;
    let height = ((max_y + pad).ceil() - origin_y).max(0.0) as usize;

    let mut mask = CoverageMask::new(width, height, origin_x, origin_y)?;

    // Pass 3: blit.
    for g in glyphs {
        let Some(bitmap) = cache.peek(g.c, size) else {
            continue;
        };
        if bitmap.is_blank() {
            continue;
        }
        let (gx, gy) = bitmap.top_left(g.pen_x, g.baseline_y);
        mask.blit_max(
            bitmap,
            (gx - origin_x).round() as i32,
            (gy - origin_y).round() as i32,
        );
    }

    Ok(mask)
}

/// Grow a coverage mask by `radius` pixels using a circular structuring
/// element — the outline pass.
///
/// The result keeps the input's dimensions and origin, so callers must reserve
/// padding up front (see [`build_mask`]). `radius` is clamped to
/// [`MAX_OUTLINE_RADIUS`].
#[must_use]
pub fn dilate(mask: &CoverageMask, radius: f32) -> CoverageMask {
    let radius = radius.clamp(0.0, MAX_OUTLINE_RADIUS);
    if radius <= 0.0 || mask.is_empty() {
        return mask.clone();
    }

    let r = radius.ceil() as i32;
    let r_sq = radius * radius;
    // Offsets inside the disc, precomputed once instead of per pixel.
    let mut offsets: Vec<(i32, i32)> = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            if (dx * dx + dy * dy) as f32 <= r_sq {
                offsets.push((dx, dy));
            }
        }
    }

    let mut out = mask.clone();
    for y in 0..mask.height {
        for x in 0..mask.width {
            let mut best = 0u8;
            for &(dx, dy) in &offsets {
                let sx = x as i32 + dx;
                let sy = y as i32 + dy;
                if sx < 0 || sy < 0 {
                    continue;
                }
                let cov = mask.coverage_at(sx as usize, sy as usize);
                if cov > best {
                    best = cov;
                    if best == u8::MAX {
                        break;
                    }
                }
            }
            out.data[y * mask.width + x] = best;
        }
    }
    out
}

/// Alpha-blend one source colour over one destination RGBA pixel.
///
/// `coverage` is the mask value; the colour's own alpha scales it, which is
/// what makes [`AnimationType::FadeIn`](super::animate::AnimationType::FadeIn)
/// and `FadeOut` real rather than decorative.
#[must_use]
pub fn blend_pixel(dst: [u8; 4], color: Color, coverage: u8) -> [u8; 4] {
    let alpha = (f32::from(coverage) / 255.0) * (f32::from(color.a) / 255.0);
    if alpha <= 0.0 {
        return dst;
    }
    let inv = 1.0 - alpha;
    let mix = |src: u8, dst: u8| -> u8 {
        (f32::from(src) * alpha + f32::from(dst) * inv)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [
        mix(color.r, dst[0]),
        mix(color.g, dst[1]),
        mix(color.b, dst[2]),
        (alpha * 255.0 + f32::from(dst[3]) * inv)
            .round()
            .clamp(0.0, 255.0) as u8,
    ]
}

/// Composite a coverage mask onto a frame in `color`.
///
/// `(block_x, block_y)` is the frame position of the text block's origin; the
/// mask's own origin offset is applied on top of it. Returns the number of
/// pixels actually touched — `0` means nothing visible landed on the frame.
pub fn composite(
    frame: &mut Frame,
    mask: &CoverageMask,
    block_x: f32,
    block_y: f32,
    color: Color,
) -> usize {
    if mask.is_empty() || color.a == 0 {
        return 0;
    }
    let (origin_x, origin_y) = mask.origin();
    let left = (block_x + origin_x).round() as i32;
    let top = (block_y + origin_y).round() as i32;

    let mut painted = 0usize;
    for my in 0..mask.height() {
        let fy = top + my as i32;
        if fy < 0 || fy as u32 >= frame.height {
            continue;
        }
        for mx in 0..mask.width() {
            let fx = left + mx as i32;
            if fx < 0 || fx as u32 >= frame.width {
                continue;
            }
            let coverage = mask.coverage_at(mx, my);
            if coverage == 0 {
                continue;
            }
            let Some(dst) = frame.get_pixel(fx as u32, fy as u32) else {
                continue;
            };
            let blended = blend_pixel(dst, color, coverage);
            if blended != dst {
                painted += 1;
            }
            frame.set_pixel(fx as u32, fy as u32, blended);
        }
    }
    painted
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A solid `w` x `h` synthetic glyph, fully opaque — stands in for a
    /// rasterized one so mask/outline/shadow behaviour is testable font-free.
    fn solid_glyph(w: usize, h: usize) -> GlyphBitmap {
        GlyphBitmap {
            coverage: vec![255; w * h],
            width: w,
            height: h,
            xmin: 0,
            ymin: 0,
            advance_width: w as f32,
        }
    }

    fn mask_with(width: usize, height: usize, set: &[(usize, usize)]) -> CoverageMask {
        let mut m = CoverageMask::new(width, height, 0.0, 0.0).expect("small mask");
        for &(x, y) in set {
            m.data[y * width + x] = 255;
        }
        m
    }

    fn flat_frame(w: u32, h: u32, rgba: [u8; 4]) -> Frame {
        let mut f = Frame::new(w, h).expect("frame");
        f.clear(rgba);
        f
    }

    // ── mask basics ──────────────────────────────────────────────────────

    #[test]
    fn oversized_mask_is_rejected_not_allocated() {
        let err = CoverageMask::new(usize::MAX, 2, 0.0, 0.0)
            .expect_err("an absurd mask must not be allocated");
        assert!(
            matches!(err, VfxError::InvalidDimensions { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn blit_max_places_the_glyph_at_the_requested_corner() {
        let mut m = CoverageMask::new(8, 8, 0.0, 0.0).expect("mask");
        m.blit_max(&solid_glyph(2, 2), 3, 4);
        assert_eq!(m.covered_pixels(), 4);
        assert_eq!(m.coverage_at(3, 4), 255);
        assert_eq!(m.coverage_at(4, 5), 255);
        assert_eq!(m.coverage_at(2, 4), 0);
        assert_eq!(m.coverage_at(5, 5), 0);
    }

    #[test]
    fn blit_max_clips_at_the_mask_edges() {
        let mut m = CoverageMask::new(4, 4, 0.0, 0.0).expect("mask");
        m.blit_max(&solid_glyph(3, 3), -1, 2);
        // Columns 0..2 (x = -1 is clipped) and rows 2..4 survive: 2 x 2.
        assert_eq!(m.covered_pixels(), 4);
        assert_eq!(m.coverage_at(0, 2), 255);
        assert_eq!(m.coverage_at(1, 3), 255);
    }

    #[test]
    fn blit_max_keeps_the_stronger_coverage_on_overlap() {
        let mut m = CoverageMask::new(4, 4, 0.0, 0.0).expect("mask");
        let faint = GlyphBitmap {
            coverage: vec![80; 4],
            width: 2,
            height: 2,
            xmin: 0,
            ymin: 0,
            advance_width: 2.0,
        };
        m.blit_max(&solid_glyph(2, 2), 0, 0);
        m.blit_max(&faint, 0, 0);
        assert_eq!(m.coverage_at(0, 0), 255, "overlap must not dim the glyph");
        m.blit_max(&faint, 2, 2);
        assert_eq!(m.coverage_at(2, 2), 80);
    }

    // ── dilation (outline) ───────────────────────────────────────────────

    #[test]
    fn dilate_by_one_grows_a_single_pixel_into_a_plus() {
        let m = mask_with(5, 5, &[(2, 2)]);
        let out = dilate(&m, 1.0);
        assert_eq!(out.covered_pixels(), 5, "radius 1 disc = 5 pixels");
        for &(x, y) in &[(2usize, 1usize), (1, 2), (2, 2), (3, 2), (2, 3)] {
            assert_eq!(out.coverage_at(x, y), 255, "({x},{y}) must be filled");
        }
        assert_eq!(out.coverage_at(1, 1), 0, "corners stay outside radius 1");
    }

    #[test]
    fn dilate_covers_the_original_shape() {
        let m = mask_with(9, 9, &[(4, 4), (5, 4)]);
        let out = dilate(&m, 2.0);
        assert_eq!(out.coverage_at(4, 4), 255);
        assert_eq!(out.coverage_at(5, 4), 255);
        assert!(
            out.covered_pixels() > m.covered_pixels(),
            "dilation must grow the shape"
        );
    }

    #[test]
    fn dilate_preserves_dimensions_and_origin() {
        let m = CoverageMask::new(6, 7, -2.0, -3.0).expect("mask");
        let out = dilate(&m, 3.0);
        assert_eq!((out.width(), out.height()), (6, 7));
        assert_eq!(out.origin(), (-2.0, -3.0));
    }

    #[test]
    fn zero_and_negative_radius_are_identity() {
        let m = mask_with(5, 5, &[(2, 2)]);
        assert_eq!(dilate(&m, 0.0), m);
        assert_eq!(dilate(&m, -4.0), m);
    }

    #[test]
    fn dilate_radius_is_clamped_to_the_documented_maximum() {
        // A wildly oversized radius must still terminate and stay in bounds.
        let m = mask_with(3, 3, &[(1, 1)]);
        let out = dilate(&m, 1.0e9);
        assert_eq!(out.covered_pixels(), 9, "everything within the tiny mask");
    }

    #[test]
    fn dilate_propagates_partial_coverage_unchanged() {
        let mut m = CoverageMask::new(3, 3, 0.0, 0.0).expect("mask");
        m.data[4] = 120;
        let out = dilate(&m, 1.0);
        assert_eq!(
            out.coverage_at(1, 0),
            120,
            "dilation copies, never brightens"
        );
        assert_eq!(out.coverage_at(1, 1), 120);
    }

    // ── blending ─────────────────────────────────────────────────────────

    #[test]
    fn zero_coverage_leaves_the_pixel_untouched() {
        let dst = [10, 20, 30, 255];
        assert_eq!(blend_pixel(dst, Color::white(), 0), dst);
    }

    #[test]
    fn full_coverage_opaque_colour_replaces_the_pixel() {
        let out = blend_pixel([10, 20, 30, 255], Color::rgb(200, 100, 50), 255);
        assert_eq!(out, [200, 100, 50, 255]);
    }

    #[test]
    fn half_coverage_mixes_evenly() {
        let out = blend_pixel([0, 0, 0, 255], Color::rgb(200, 200, 200), 128);
        // 128/255 ~= 0.502 -> ~100
        assert!(
            (99..=101).contains(&out[0]),
            "expected ~100, got {}",
            out[0]
        );
    }

    /// Colour alpha must scale coverage, otherwise the fade animations are
    /// silently cosmetic.
    #[test]
    fn colour_alpha_scales_coverage() {
        let opaque = blend_pixel([0, 0, 0, 255], Color::new(255, 255, 255, 255), 255);
        let half = blend_pixel([0, 0, 0, 255], Color::new(255, 255, 255, 128), 255);
        let clear = blend_pixel([0, 0, 0, 255], Color::new(255, 255, 255, 0), 255);
        assert_eq!(opaque[0], 255);
        assert!(
            (126..=130).contains(&half[0]),
            "expected ~128, got {}",
            half[0]
        );
        assert_eq!(clear, [0, 0, 0, 255], "alpha 0 must paint nothing");
    }

    #[test]
    fn blending_onto_transparent_accumulates_alpha() {
        let out = blend_pixel([0, 0, 0, 0], Color::rgb(255, 255, 255), 255);
        assert_eq!(out[3], 255, "opaque ink must make the pixel opaque");
    }

    // ── compositing ──────────────────────────────────────────────────────

    #[test]
    fn composite_paints_only_the_mask_footprint() {
        let mut frame = flat_frame(16, 16, [0, 0, 0, 255]);
        let mut mask = CoverageMask::new(4, 4, 0.0, 0.0).expect("mask");
        mask.blit_max(&solid_glyph(4, 4), 0, 0);

        let painted = composite(&mut frame, &mask, 2.0, 3.0, Color::white());
        assert_eq!(painted, 16, "a 4x4 opaque mask paints 16 pixels");

        for y in 0..16u32 {
            for x in 0..16u32 {
                let inside = (2..6).contains(&x) && (3..7).contains(&y);
                let px = frame.get_pixel(x, y).expect("in bounds");
                if inside {
                    assert_eq!(px, [255, 255, 255, 255], "({x},{y}) must be painted");
                } else {
                    assert_eq!(px, [0, 0, 0, 255], "({x},{y}) must be untouched");
                }
            }
        }
    }

    #[test]
    fn composite_applies_the_mask_origin_offset() {
        let mut frame = flat_frame(8, 8, [0, 0, 0, 255]);
        let mut mask = CoverageMask::new(2, 2, -3.0, -3.0).expect("mask");
        mask.blit_max(&solid_glyph(2, 2), 0, 0);
        composite(&mut frame, &mask, 4.0, 4.0, Color::white());
        // block origin (4,4) + mask origin (-3,-3) = (1,1)
        assert_eq!(
            frame.get_pixel(1, 1).expect("in bounds"),
            [255, 255, 255, 255]
        );
        assert_eq!(frame.get_pixel(4, 4).expect("in bounds"), [0, 0, 0, 255]);
    }

    #[test]
    fn composite_clips_at_frame_edges() {
        let mut frame = flat_frame(8, 8, [0, 0, 0, 255]);
        let mut mask = CoverageMask::new(4, 4, 0.0, 0.0).expect("mask");
        mask.blit_max(&solid_glyph(4, 4), 0, 0);
        let painted = composite(&mut frame, &mask, 6.0, 6.0, Color::white());
        assert_eq!(painted, 4, "only the 2x2 in-bounds corner may paint");
    }

    #[test]
    fn composite_off_frame_paints_nothing() {
        let mut frame = flat_frame(8, 8, [7, 7, 7, 255]);
        let mut mask = CoverageMask::new(4, 4, 0.0, 0.0).expect("mask");
        mask.blit_max(&solid_glyph(4, 4), 0, 0);
        assert_eq!(
            composite(&mut frame, &mask, -50.0, -50.0, Color::white()),
            0
        );
        assert!(frame
            .data
            .chunks_exact(4)
            .all(|p| p == [7u8, 7, 7, 255].as_slice()));
    }

    #[test]
    fn fully_transparent_colour_paints_nothing() {
        let mut frame = flat_frame(8, 8, [7, 7, 7, 255]);
        let mut mask = CoverageMask::new(4, 4, 0.0, 0.0).expect("mask");
        mask.blit_max(&solid_glyph(4, 4), 0, 0);
        let painted = composite(&mut frame, &mask, 0.0, 0.0, Color::new(255, 255, 255, 0));
        assert_eq!(painted, 0);
        assert!(frame
            .data
            .chunks_exact(4)
            .all(|p| p == [7u8, 7, 7, 255].as_slice()));
    }

    /// Shadow-then-fill ordering: the offset copy must sit *under* the fill.
    #[test]
    fn shadow_pass_lands_at_the_offset_and_stays_below_the_fill() {
        let mut frame = flat_frame(16, 16, [0, 0, 0, 255]);
        let mut mask = CoverageMask::new(4, 4, 0.0, 0.0).expect("mask");
        mask.blit_max(&solid_glyph(4, 4), 0, 0);

        // Shadow first, offset by (+2, +2), then the fill at the origin.
        composite(
            &mut frame,
            &mask,
            4.0 + 2.0,
            4.0 + 2.0,
            Color::rgb(255, 0, 0),
        );
        composite(&mut frame, &mask, 4.0, 4.0, Color::rgb(0, 255, 0));

        assert_eq!(
            frame.get_pixel(4, 4).expect("in bounds"),
            [0, 255, 0, 255],
            "fill-only region"
        );
        assert_eq!(
            frame.get_pixel(9, 9).expect("in bounds"),
            [255, 0, 0, 255],
            "shadow-only region"
        );
        assert_eq!(
            frame.get_pixel(6, 6).expect("in bounds"),
            [0, 255, 0, 255],
            "overlap must show the fill, not the shadow"
        );
    }

    /// Outline-then-fill ordering: a dilated mask painted first must show a
    /// ring of outline colour around the fill.
    #[test]
    fn outline_pass_surrounds_the_fill() {
        let mut frame = flat_frame(16, 16, [0, 0, 0, 255]);
        let mut mask = CoverageMask::new(8, 8, 0.0, 0.0).expect("mask");
        mask.blit_max(&solid_glyph(2, 2), 3, 3);
        let outline = dilate(&mask, 2.0);

        composite(&mut frame, &outline, 4.0, 4.0, Color::rgb(255, 0, 0));
        composite(&mut frame, &mask, 4.0, 4.0, Color::rgb(0, 255, 0));

        // Glyph body at mask (3,3)..(5,5) -> frame (7,7)..(9,9).
        assert_eq!(
            frame.get_pixel(7, 7).expect("in bounds"),
            [0, 255, 0, 255],
            "the body must be fill coloured"
        );
        assert_eq!(
            frame.get_pixel(5, 7).expect("in bounds"),
            [255, 0, 0, 255],
            "two pixels left of the body must be outline coloured"
        );
    }

    #[test]
    fn build_mask_of_no_glyphs_is_empty_but_valid() {
        let mask = CoverageMask::new(0, 0, 0.0, 0.0).expect("empty mask");
        assert!(mask.is_empty());
        assert_eq!(mask.covered_pixels(), 0);
        let mut frame = flat_frame(4, 4, [1, 2, 3, 255]);
        assert_eq!(composite(&mut frame, &mask, 0.0, 0.0, Color::white()), 0);
    }
}
