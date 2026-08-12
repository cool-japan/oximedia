//! Font loading and glyph rasterization for the VFX text renderer.
//!
//! No font ships in this tree — fonts carry their own licences — so every font
//! used here comes from caller-supplied bytes or a caller-supplied path. A
//! [`TextRenderer`](super::render::TextRenderer) built without one renders
//! nothing and reports an honest error rather than approximating glyphs.
//!
//! Rasterization uses `fontdue`, the pure-Rust engine already used by
//! `oximedia-graph`'s timecode burn-in and by `oximedia-subtitle`.

use std::collections::HashMap;
use std::path::Path;

use fontdue::{Font as FontdueFont, FontSettings};

use crate::{VfxError, VfxResult};

use super::layout::LineMetrics;

/// A rasterized glyph: an 8-bit coverage bitmap plus its placement metrics.
///
/// Coverage is `0` for "no ink" and `255` for "fully covered"; intermediate
/// values are the anti-aliased edges produced by the rasterizer.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBitmap {
    /// Coverage values, row-major, exactly `width * height` bytes long.
    pub coverage: Vec<u8>,
    /// Bitmap width in whole pixels.
    pub width: usize,
    /// Bitmap height in whole pixels.
    pub height: usize,
    /// Whole-pixel offset from the pen position to the bitmap's left edge.
    ///
    /// Negative when the glyph reaches left of the pen (e.g. italic overhang).
    pub xmin: i32,
    /// Whole-pixel offset from the baseline to the bitmap's bottom edge.
    ///
    /// Negative for descenders, which sit below the baseline.
    pub ymin: i32,
    /// Advance to the next pen position, in pixels.
    pub advance_width: f32,
}

impl GlyphBitmap {
    /// An empty (zero-sized) glyph with the given advance, used for whitespace
    /// and for characters the font has no outline for.
    #[must_use]
    pub const fn blank(advance_width: f32) -> Self {
        Self {
            coverage: Vec::new(),
            width: 0,
            height: 0,
            xmin: 0,
            ymin: 0,
            advance_width,
        }
    }

    /// `true` when this glyph has no pixels to paint.
    #[must_use]
    pub const fn is_blank(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Top-left corner of this bitmap for a pen at `pen_x` sitting on the
    /// baseline `baseline_y`, in the same coordinate space as those inputs.
    ///
    /// The `y` term flips `fontdue`'s y-up glyph box into the y-down pixel
    /// space used by [`Frame`](crate::Frame): the bitmap's bottom edge is
    /// `ymin` above the baseline, so its top edge is `ymin + height` above it.
    #[must_use]
    pub fn top_left(&self, pen_x: f32, baseline_y: f32) -> (f32, f32) {
        (
            pen_x + self.xmin as f32,
            baseline_y - self.ymin as f32 - self.height as f32,
        )
    }

    /// Coverage at `(x, y)` within the bitmap, or `0` outside it.
    #[must_use]
    pub fn coverage_at(&self, x: usize, y: usize) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.coverage.get(y * self.width + x).copied().unwrap_or(0)
    }
}

/// A loaded font face.
pub struct FontFace {
    inner: FontdueFont,
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `fontdue::Font` carries no name metadata and is not `Debug`, so
        // report the glyph count — enough to tell two faces apart in a log.
        f.debug_struct("FontFace")
            .field("glyph_count", &self.inner.glyph_count())
            .finish()
    }
}

impl FontFace {
    /// Load a font face from TTF/OTF bytes supplied by the caller.
    ///
    /// # Errors
    ///
    /// Returns [`VfxError::TextRenderError`] if `data` is empty or is not a
    /// font `fontdue` can parse.
    pub fn from_bytes(data: &[u8]) -> VfxResult<Self> {
        if data.is_empty() {
            return Err(VfxError::TextRenderError(
                "font data is empty — supply the bytes of a TTF/OTF file".to_string(),
            ));
        }
        let inner = FontdueFont::from_bytes(data, FontSettings::default())
            .map_err(|e| VfxError::TextRenderError(format!("failed to parse font data: {e}")))?;
        Ok(Self { inner })
    }

    /// Load a font face from a file path supplied by the caller.
    ///
    /// # Errors
    ///
    /// Returns [`VfxError::TextRenderError`] if the file cannot be read or is
    /// not a font `fontdue` can parse.
    pub fn from_file(path: impl AsRef<Path>) -> VfxResult<Self> {
        let path = path.as_ref();
        let data = std::fs::read(path).map_err(|e| {
            VfxError::TextRenderError(format!("failed to read font file {}: {e}", path.display()))
        })?;
        Self::from_bytes(&data)
    }

    /// Vertical line metrics reported by the font at `size` pixels.
    ///
    /// Falls back to [`LineMetrics::fallback`] for fonts that carry no
    /// horizontal line metrics table.
    #[must_use]
    pub fn line_metrics(&self, size: f32) -> LineMetrics {
        self.inner.horizontal_line_metrics(size).map_or_else(
            || LineMetrics::fallback(size),
            |m| LineMetrics {
                ascent: m.ascent,
                descent: m.descent,
                line_gap: m.line_gap,
                new_line_size: m.new_line_size,
            },
        )
    }

    /// Advance width of `c` at `size` pixels, as reported by the font.
    #[must_use]
    pub fn advance_width(&self, c: char, size: f32) -> f32 {
        self.inner.metrics(c, size).advance_width
    }

    /// `true` when the font actually contains an outline for `c` (rather than
    /// falling back to `.notdef`).
    #[must_use]
    pub fn has_glyph(&self, c: char) -> bool {
        self.inner.lookup_glyph_index(c) != 0
    }

    /// Rasterize `c` at `size` pixels.
    #[must_use]
    pub fn rasterize(&self, c: char, size: f32) -> GlyphBitmap {
        let (metrics, coverage) = self.inner.rasterize(c, size);
        GlyphBitmap {
            coverage,
            width: metrics.width,
            height: metrics.height,
            xmin: metrics.xmin,
            ymin: metrics.ymin,
            advance_width: metrics.advance_width,
        }
    }
}

/// Cache key for a rasterized glyph: codepoint plus size quantized to 1/100 px.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    codepoint: u32,
    size_centi: u32,
}

impl GlyphKey {
    fn new(c: char, size: f32) -> Self {
        Self {
            codepoint: c as u32,
            // f32 -> u32 casts saturate in Rust, so NaN/negative sizes land on 0
            // instead of wrapping into a colliding key.
            size_centi: (size * 100.0) as u32,
        }
    }
}

/// A font face plus a cache of the glyphs already rasterized from it.
///
/// Rasterizing is the expensive part of text rendering, and video work re-draws
/// the same characters on every frame, so glyphs are cached per (character,
/// size) pair — the same strategy `oximedia-graph`'s timecode filter uses.
pub struct GlyphCache {
    font: FontFace,
    glyphs: HashMap<GlyphKey, GlyphBitmap>,
}

impl GlyphCache {
    /// Wrap a loaded font face in an empty cache.
    #[must_use]
    pub fn new(font: FontFace) -> Self {
        Self {
            font,
            glyphs: HashMap::new(),
        }
    }

    /// The underlying font face.
    #[must_use]
    pub const fn font(&self) -> &FontFace {
        &self.font
    }

    /// Rasterize `c` at `size` if it is not cached yet, then return it.
    pub fn glyph(&mut self, c: char, size: f32) -> &GlyphBitmap {
        let key = GlyphKey::new(c, size);
        self.glyphs
            .entry(key)
            .or_insert_with(|| self.font.rasterize(c, size))
    }

    /// Look up an already-cached glyph without rasterizing.
    ///
    /// Used by the mask builder, which warms the cache in one pass and then
    /// reads it back immutably while measuring and blitting.
    #[must_use]
    pub fn peek(&self, c: char, size: f32) -> Option<&GlyphBitmap> {
        self.glyphs.get(&GlyphKey::new(c, size))
    }

    /// Number of cached glyphs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// `true` when nothing has been rasterized yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Drop every cached glyph.
    pub fn clear(&mut self) {
        self.glyphs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_bytes_rejects_empty_data() {
        let err = FontFace::from_bytes(&[]).expect_err("empty font data must be rejected");
        assert!(
            matches!(err, VfxError::TextRenderError(ref m) if m.contains("empty")),
            "expected an honest empty-data error, got {err:?}"
        );
    }

    #[test]
    fn from_bytes_rejects_non_font_data() {
        let err = FontFace::from_bytes(b"this is definitely not a TTF file")
            .expect_err("garbage must not parse as a font");
        assert!(matches!(err, VfxError::TextRenderError(_)), "got {err:?}");
    }

    #[test]
    fn from_file_reports_the_missing_path() {
        let missing = std::env::temp_dir().join("oximedia-vfx-no-such-font-file.ttf");
        let err = FontFace::from_file(&missing).expect_err("missing font file must error");
        let VfxError::TextRenderError(msg) = err else {
            panic!("expected TextRenderError");
        };
        assert!(
            msg.contains("oximedia-vfx-no-such-font-file.ttf"),
            "error must name the path it failed to read, got {msg}"
        );
    }

    #[test]
    fn blank_glyph_has_no_pixels_but_keeps_its_advance() {
        let g = GlyphBitmap::blank(7.5);
        assert!(g.is_blank());
        assert_eq!(g.advance_width, 7.5);
        assert_eq!(g.coverage_at(0, 0), 0);
    }

    #[test]
    fn top_left_flips_the_baseline_relative_box_into_y_down_space() {
        // A 4x6 glyph whose bottom edge sits 2px below the baseline
        // (ymin = -2) and which starts 1px left of the pen (xmin = -1).
        let g = GlyphBitmap {
            coverage: vec![0; 24],
            width: 4,
            height: 6,
            xmin: -1,
            ymin: -2,
            advance_width: 5.0,
        };
        let (x, y) = g.top_left(10.0, 20.0);
        assert_eq!(x, 9.0, "pen 10 with xmin -1 starts at 9");
        // top = baseline - ymin - height = 20 - (-2) - 6 = 16
        assert_eq!(y, 16.0, "bottom edge 2px below baseline, 6px tall");
    }

    #[test]
    fn coverage_at_is_bounds_checked() {
        let g = GlyphBitmap {
            coverage: vec![10, 20, 30, 40],
            width: 2,
            height: 2,
            xmin: 0,
            ymin: 0,
            advance_width: 2.0,
        };
        assert_eq!(g.coverage_at(0, 0), 10);
        assert_eq!(g.coverage_at(1, 1), 40);
        assert_eq!(g.coverage_at(2, 0), 0, "out of bounds reads as no coverage");
        assert_eq!(g.coverage_at(0, 9), 0, "out of bounds reads as no coverage");
    }

    #[test]
    fn glyph_keys_separate_sizes_and_codepoints() {
        assert_ne!(GlyphKey::new('a', 12.0), GlyphKey::new('a', 24.0));
        assert_ne!(GlyphKey::new('a', 12.0), GlyphKey::new('b', 12.0));
        assert_eq!(GlyphKey::new('a', 12.0), GlyphKey::new('a', 12.0));
    }

    #[test]
    fn glyph_key_saturates_instead_of_wrapping_on_bad_sizes() {
        // Negative and NaN sizes must not wrap into a key that collides with a
        // legitimate one; Rust float->int casts saturate, so both land on 0.
        assert_eq!(GlyphKey::new('a', -5.0).size_centi, 0);
        assert_eq!(GlyphKey::new('a', f32::NAN).size_centi, 0);
    }
}
