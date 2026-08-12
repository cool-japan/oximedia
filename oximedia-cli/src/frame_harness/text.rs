//! Real font-based text overlay: glyph layout plus alpha-compositing directly
//! onto packed planar Y4M frames.
//!
//! Shared by `subtitle burn` and `captions burn`, the CLI's two cue-burning
//! commands. Neither ships a font (fonts carry their own licences, see
//! [`super::font`]), so both require `--font <PATH>` and rasterise with the
//! same real `fontdue`-backed engine:
//! [`oximedia_subtitle::font::{Font, GlyphCache, SimpleLayoutEngine}`].
//!
//! This deliberately does *not* use `oximedia_subtitle::burn_in::{BitmapFont,
//! SubtitleBurnIn}`, which draws a built-in 8x12 bitmap-font approximation
//! rather than real glyphs rasterised from the user's font file — the same
//! honesty rule the timecode burn-in filter already follows.

use anyhow::{Context, Result};
use oximedia_container::demux::y4m::Y4mChroma;
use oximedia_subtitle::font::{CachedGlyph, Font, GlyphCache, GlyphPosition, SimpleLayoutEngine};

use super::{adapt, ChromaLayout, PlanarFrame};

/// An RGB colour for burned-in text, convertible to the full-range BT.709
/// YCbCr triple used when painting onto Y4M planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextColor {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl TextColor {
    /// Opaque white — the standard subtitle/caption colour.
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
    };

    /// Parse a `RRGGBB` (optionally prefixed with `#`) hex colour string.
    ///
    /// # Errors
    ///
    /// Returns an error if `s` is not exactly 6 hex digits.
    pub fn from_hex(s: &str) -> Result<Self> {
        let trimmed = s.trim().trim_start_matches('#');
        if trimmed.len() != 6 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!(
                "font color '{s}' must be a 6-digit hex RGB value, e.g. FFFFFF or #FFFFFF"
            );
        }
        let byte = |i: usize| -> Result<u8> {
            u8::from_str_radix(&trimmed[i..i + 2], 16)
                .with_context(|| format!("font color '{s}' is not valid hex"))
        };
        Ok(Self {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
        })
    }

    /// Convert to full-range BT.709 YCbCr.
    ///
    /// Uses the same coefficients as
    /// `oximedia_graph::filters::video::timecode::TimecodeFilter::composite_to_yuv`,
    /// so a colour looks the same whichever burn-in path produced it.
    #[must_use]
    fn to_yuv(self) -> (u8, u8, u8) {
        let r = f32::from(self.r);
        let g = f32::from(self.g);
        let b = f32::from(self.b);
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let u = -0.1146 * r - 0.3854 * g + 0.5 * b + 128.0;
        let v = 0.5 * r - 0.4542 * g - 0.0458 * b + 128.0;
        (
            y.round().clamp(0.0, 255.0) as u8,
            u.round().clamp(0.0, 255.0) as u8,
            v.round().clamp(0.0, 255.0) as u8,
        )
    }
}

/// Real glyph rasterisation and alpha-compositing engine for one loaded font.
pub struct TextRenderer {
    cache: GlyphCache,
    layout_engine: SimpleLayoutEngine,
}

impl TextRenderer {
    /// Build a renderer from already-validated font bytes.
    ///
    /// Pair with [`super::font::load_font`], which validates `--font` and
    /// returns the raw bytes this constructor wraps.
    ///
    /// # Errors
    ///
    /// Returns an error if `font_bytes` is not a font `fontdue` can parse.
    pub fn new(font_bytes: Vec<u8>) -> Result<Self> {
        let font = Font::from_bytes(font_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to load font: {e}"))?;
        Ok(Self {
            cache: GlyphCache::new(font),
            layout_engine: SimpleLayoutEngine::new(),
        })
    }

    /// Lay out `text` at `size` points, optionally word-wrapped to
    /// `max_width` pixels. Hard line breaks (`\n`) in `text` are respected.
    ///
    /// Returns each glyph's character and its top-left pixel position
    /// relative to the block's own origin (not yet offset onto a frame).
    pub fn layout(&mut self, text: &str, size: f32, max_width: Option<f32>) -> Vec<GlyphPosition> {
        // Bind the font reference to a local first: `self.cache` and
        // `self.layout_engine` are disjoint fields, but borrowing
        // `self.cache.font()` as a call argument while simultaneously
        // borrowing `self.layout_engine` mutably as the receiver is clearer
        // (and more robustly accepted by the borrow checker) done in two
        // steps than inline.
        let font = self.cache.font();
        self.layout_engine.layout_text(font, text, size, max_width)
    }

    /// Pixel size `(width, height)` of a laid-out block.
    #[must_use]
    pub fn bounds(glyphs: &[GlyphPosition]) -> (f32, f32) {
        glyphs.iter().fold((0.0f32, 0.0f32), |(w, h), g| {
            (w.max(g.x + g.width), h.max(g.y + g.height))
        })
    }

    /// Rasterise and alpha-composite `glyphs` onto `frame`'s planes, offset
    /// so the block's origin lands at pixel `(anchor_x, anchor_y)`.
    ///
    /// Returns the number of luma samples actually painted (glyph coverage
    /// `> 0` and inside the frame). `0` means nothing visible landed on the
    /// frame — every glyph was whitespace/missing from the font, or the
    /// whole block fell outside the frame bounds.
    ///
    /// # Errors
    ///
    /// Returns an error if `frame`'s chroma format has no compositable
    /// [`oximedia_core::PixelFormat`] counterpart, or its dimensions are not
    /// divisible by the chroma subsampling factor (see
    /// [`adapt::require_compositable`]).
    pub fn paint(
        &mut self,
        frame: &mut PlanarFrame,
        glyphs: &[GlyphPosition],
        size: f32,
        anchor_x: i32,
        anchor_y: i32,
        color: TextColor,
    ) -> Result<usize> {
        adapt::require_compositable("text burn-in", &frame.layout)?;
        let yuv = color.to_yuv();

        let mut painted = 0usize;
        for g in glyphs {
            if g.c.is_whitespace() {
                continue;
            }
            let glyph = self.cache.get_glyph(g.c, size);
            if glyph.width == 0 || glyph.height == 0 {
                continue;
            }
            let gx = anchor_x + g.x.round() as i32;
            let gy = anchor_y + g.y.round() as i32;
            painted += blend_glyph(frame, glyph, gx, gy, yuv);
        }
        Ok(painted)
    }
}

/// Integer luma:chroma downsampling ratio `(horizontal, vertical)` for a Y4M
/// chroma format. Mirrors the match in
/// [`ChromaLayout::for_chroma`](super::ChromaLayout::for_chroma) exactly, so
/// it agrees with the plane dimensions that function derives.
const fn chroma_subsample(chroma: Y4mChroma) -> (usize, usize) {
    match chroma {
        Y4mChroma::C420jpeg | Y4mChroma::C420mpeg2 | Y4mChroma::C420paldv => (2, 2),
        Y4mChroma::C422 => (2, 1),
        Y4mChroma::C444 | Y4mChroma::C444alpha | Y4mChroma::Mono => (1, 1),
    }
}

/// Alpha-blend one rasterised glyph bitmap onto `frame`'s planes at pixel
/// position `(x, y)` (the top-left corner of the bitmap). Returns the number
/// of luma samples actually painted.
///
/// Operates directly on `frame.data` (rather than borrowing
/// `frame.luma_mut()` and `frame.plane_mut(1..)` separately) so luma and
/// chroma updates share one pass over the glyph's pixels without conflicting
/// mutable borrows.
fn blend_glyph(
    frame: &mut PlanarFrame,
    glyph: &CachedGlyph,
    x: i32,
    y: i32,
    yuv: (u8, u8, u8),
) -> usize {
    let layout: ChromaLayout = frame.layout;
    let (y_val, u_val, v_val) = yuv;
    let luma_w = layout.luma_w as i32;
    let luma_h = layout.luma_h as i32;
    let has_chroma = layout.chroma_len() > 0;
    let u_off = layout.plane_offset(1);
    let v_off = layout.plane_offset(2);
    let (h_sub, v_sub) = chroma_subsample(layout.chroma);

    let mut painted = 0usize;
    for gy in 0..glyph.height {
        let fy = y + gy as i32;
        if fy < 0 || fy >= luma_h {
            continue;
        }
        for gx in 0..glyph.width {
            let fx = x + gx as i32;
            if fx < 0 || fx >= luma_w {
                continue;
            }
            let alpha = glyph.bitmap[gy * glyph.width + gx];
            if alpha == 0 {
                continue;
            }
            let a = f32::from(alpha) / 255.0;
            let inv = 1.0 - a;

            let luma_idx = fy as usize * layout.luma_w + fx as usize;
            let old = f32::from(frame.data[luma_idx]);
            frame.data[luma_idx] = (f32::from(y_val) * a + old * inv).round() as u8;
            painted += 1;

            if has_chroma {
                let cx = ((fx as usize) / h_sub).min(layout.chroma_w.saturating_sub(1));
                let cy = ((fy as usize) / v_sub).min(layout.chroma_h.saturating_sub(1));
                let coff = cy * layout.chroma_w + cx;
                if let Some(uo) = u_off {
                    let idx = uo + coff;
                    let old = f32::from(frame.data[idx]);
                    frame.data[idx] = (f32::from(u_val) * a + old * inv).round() as u8;
                }
                if let Some(vo) = v_off {
                    let idx = vo + coff;
                    let old = f32::from(frame.data[idx]);
                    frame.data[idx] = (f32::from(v_val) * a + old * inv).round() as u8;
                }
            }
        }
    }
    painted
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_container::demux::y4m::Y4mChroma;

    fn flat_frame(w: usize, h: usize, y: u8) -> PlanarFrame {
        let layout =
            ChromaLayout::for_chroma(Y4mChroma::C420jpeg, w as u32, h as u32).expect("420 layout");
        let mut data = vec![128u8; layout.frame_size()];
        data[..layout.luma_len()].fill(y);
        PlanarFrame::new(layout, data).expect("frame")
    }

    #[test]
    fn hex_color_parses_rgb() {
        let c = TextColor::from_hex("FFAA00").expect("valid hex");
        assert_eq!(
            c,
            TextColor {
                r: 0xFF,
                g: 0xAA,
                b: 0x00
            }
        );
        let c2 = TextColor::from_hex("#00ff00").expect("valid hex with #");
        assert_eq!(
            c2,
            TextColor {
                r: 0,
                g: 0xFF,
                b: 0
            }
        );
    }

    #[test]
    fn hex_color_rejects_bad_input() {
        assert!(TextColor::from_hex("nothex").is_err());
        assert!(TextColor::from_hex("FFF").is_err());
        assert!(TextColor::from_hex("GGFFFF").is_err());
    }

    #[test]
    fn white_converts_to_near_255_luma() {
        let (y, u, v) = TextColor::WHITE.to_yuv();
        assert!(y > 250, "white must be near-peak luma, got {y}");
        assert!((i16::from(u) - 128).abs() < 3, "u should be ~128, got {u}");
        assert!((i16::from(v) - 128).abs() < 3, "v should be ~128, got {v}");
    }

    /// A synthetic glyph (a solid 4x4 block, alpha 255) painted at (2,2) must
    /// change exactly 16 luma samples and the co-sited chroma samples, and
    /// nothing outside that footprint.
    #[test]
    fn blend_glyph_paints_only_its_footprint() {
        let mut frame = flat_frame(16, 16, 100);
        let glyph = CachedGlyph {
            bitmap: vec![255u8; 16],
            width: 4,
            height: 4,
            offset_x: 0.0,
            offset_y: 0.0,
            advance_width: 4.0,
        };
        let painted = blend_glyph(&mut frame, &glyph, 2, 2, (255, 128, 128));
        assert_eq!(
            painted, 16,
            "a 4x4 fully-opaque glyph must paint 16 luma samples"
        );

        let luma_w = frame.layout.luma_w;
        let mut changed = 0usize;
        for row in 0..16 {
            for col in 0..16 {
                let inside = (2..6).contains(&col) && (2..6).contains(&row);
                let v = frame.luma()[row * luma_w + col];
                if inside {
                    assert!(
                        v > 200,
                        "inside the glyph footprint must be near-white, got {v}"
                    );
                    changed += 1;
                } else {
                    assert_eq!(v, 100, "outside the glyph footprint must be untouched");
                }
            }
        }
        assert_eq!(changed, 16);
    }

    #[test]
    fn blend_glyph_clips_at_frame_edges() {
        let mut frame = flat_frame(8, 8, 50);
        let glyph = CachedGlyph {
            bitmap: vec![255u8; 16],
            width: 4,
            height: 4,
            offset_x: 0.0,
            offset_y: 0.0,
            advance_width: 4.0,
        };
        // Anchored so half the glyph falls off the right/bottom edge.
        let painted = blend_glyph(&mut frame, &glyph, 6, 6, (255, 128, 128));
        assert_eq!(painted, 4, "only the 2x2 in-bounds corner should paint");
    }

    #[test]
    fn zero_alpha_glyph_paints_nothing() {
        let mut frame = flat_frame(8, 8, 77);
        let glyph = CachedGlyph {
            bitmap: vec![0u8; 16],
            width: 4,
            height: 4,
            offset_x: 0.0,
            offset_y: 0.0,
            advance_width: 4.0,
        };
        let painted = blend_glyph(&mut frame, &glyph, 0, 0, (255, 128, 128));
        assert_eq!(painted, 0);
        assert!(
            frame.luma().iter().all(|&v| v == 77),
            "frame must be untouched"
        );
    }
}
