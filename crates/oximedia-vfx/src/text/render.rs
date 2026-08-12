//! Text rendering with effects.
//!
//! Glyphs are rasterized for real by `fontdue` (see [`super::font`]), laid out
//! by the metric-driven engine in [`super::layout`], and composited through the
//! coverage-mask pipeline in [`super::paint`]: drop shadow, then outline, then
//! fill.
//!
//! No font ships in this tree — fonts carry their own licences — so a renderer
//! built with [`TextRenderer::new`] has no font and reports an honest error for
//! non-empty text. Supply one with [`TextRenderer::with_font_bytes`],
//! [`TextRenderer::with_font_file`] or [`TextRenderer::set_font_bytes`].

use std::path::Path;

use crate::{Color, EffectParams, Frame, VfxError, VfxResult, VideoEffect};

use super::font::{FontFace, GlyphCache};
use super::layout::{self, TextAlign, TextLayout};
use super::paint::{self, CoverageMask};

/// Message reported when text is configured but no font was supplied.
const NO_FONT: &str = "text rendering needs a font: none supplied — build the renderer with \
                       TextRenderer::with_font_bytes/with_font_file, or call \
                       set_font_bytes/set_font_file (no font ships with OxiMedia)";

/// Text configuration.
#[derive(Debug, Clone)]
pub struct TextConfig {
    /// Text content. `\n` starts a new line.
    pub text: String,
    /// Font size in pixels.
    pub font_size: f32,
    /// Text color.
    pub color: Color,
    /// Position X (0.0 - 1.0), the horizontal centre of the text block.
    pub x: f32,
    /// Position Y (0.0 - 1.0), the vertical centre of the text block.
    pub y: f32,
    /// Outline width in pixels (`0.0` disables the outline pass).
    ///
    /// Clamped to [`paint::MAX_OUTLINE_RADIUS`].
    pub outline_width: f32,
    /// Outline color.
    pub outline_color: Color,
    /// Drop shadow offset X in pixels.
    pub shadow_x: f32,
    /// Drop shadow offset Y in pixels.
    pub shadow_y: f32,
    /// Shadow color.
    pub shadow_color: Color,
    /// Horizontal alignment of lines within the block.
    pub align: TextAlign,
    /// Word-wrap width as a fraction of the frame width (`None` = no wrap).
    pub max_width: Option<f32>,
    /// Multiplier on the font's own baseline-to-baseline distance.
    pub line_height: f32,
}

impl Default for TextConfig {
    fn default() -> Self {
        Self {
            text: String::new(),
            font_size: 48.0,
            color: Color::white(),
            x: 0.5,
            y: 0.5,
            outline_width: 0.0,
            outline_color: Color::black(),
            shadow_x: 0.0,
            shadow_y: 0.0,
            shadow_color: Color::black(),
            align: TextAlign::Left,
            max_width: None,
            line_height: 1.0,
        }
    }
}

impl TextConfig {
    /// Create a new text config.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// Set font size.
    #[must_use]
    pub const fn with_font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    /// Set text color.
    #[must_use]
    pub const fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Set position.
    #[must_use]
    pub fn with_position(mut self, x: f32, y: f32) -> Self {
        self.x = x.clamp(0.0, 1.0);
        self.y = y.clamp(0.0, 1.0);
        self
    }

    /// Set outline.
    #[must_use]
    pub const fn with_outline(mut self, width: f32, color: Color) -> Self {
        self.outline_width = width;
        self.outline_color = color;
        self
    }

    /// Set shadow.
    #[must_use]
    pub const fn with_shadow(mut self, offset_x: f32, offset_y: f32, color: Color) -> Self {
        self.shadow_x = offset_x;
        self.shadow_y = offset_y;
        self.shadow_color = color;
        self
    }

    /// Set horizontal alignment of the lines within the block.
    #[must_use]
    pub const fn with_align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    /// Set the word-wrap width, as a fraction of the frame width.
    #[must_use]
    pub fn with_max_width(mut self, fraction: f32) -> Self {
        self.max_width = Some(fraction.clamp(0.0, 1.0));
        self
    }

    /// Set the line-height multiplier (`1.0` = exactly what the font asks for).
    #[must_use]
    pub const fn with_line_height(mut self, multiplier: f32) -> Self {
        self.line_height = multiplier;
        self
    }

    /// `true` when a drop shadow is configured.
    #[must_use]
    pub fn has_shadow(&self) -> bool {
        (self.shadow_x != 0.0 || self.shadow_y != 0.0)
            && self.shadow_x.is_finite()
            && self.shadow_y.is_finite()
            && self.shadow_color.a > 0
    }

    /// `true` when an outline is configured.
    #[must_use]
    pub fn has_outline(&self) -> bool {
        self.outline_width.is_finite() && self.outline_width > 0.0 && self.outline_color.a > 0
    }

    /// Effective outline radius in pixels, clamped to the supported maximum.
    #[must_use]
    pub fn outline_radius(&self) -> f32 {
        if self.has_outline() {
            self.outline_width.clamp(0.0, paint::MAX_OUTLINE_RADIUS)
        } else {
            0.0
        }
    }
}

/// Text renderer.
///
/// Rasterizes real glyphs from a caller-supplied font, lays them out (word
/// wrap, hard breaks, alignment, line height) and composites the block onto a
/// frame with an optional outline and drop shadow.
///
/// The frame position is the *centre* of the laid-out block, so the default
/// [`TextConfig`] position `(0.5, 0.5)` centres text on the frame.
pub struct TextRenderer {
    config: TextConfig,
    font: Option<GlyphCache>,
}

impl TextRenderer {
    /// Create a renderer with no font loaded.
    ///
    /// Rendering non-empty text through it reports an honest error; supply a
    /// font with [`Self::set_font_bytes`] or [`Self::set_font_file`], or build
    /// the renderer with [`Self::with_font_bytes`] / [`Self::with_font_file`].
    ///
    /// # Errors
    ///
    /// Currently infallible; the `Result` is kept so callers stay source
    /// compatible with font-loading constructors.
    pub fn new(config: TextConfig) -> VfxResult<Self> {
        Ok(Self { config, font: None })
    }

    /// Create a renderer from caller-supplied TTF/OTF bytes.
    ///
    /// # Errors
    ///
    /// Returns [`VfxError::TextRenderError`] if the bytes are not a parsable
    /// font.
    pub fn with_font_bytes(config: TextConfig, font_data: &[u8]) -> VfxResult<Self> {
        Ok(Self {
            config,
            font: Some(GlyphCache::new(FontFace::from_bytes(font_data)?)),
        })
    }

    /// Create a renderer from a font file path.
    ///
    /// # Errors
    ///
    /// Returns [`VfxError::TextRenderError`] if the file cannot be read or is
    /// not a parsable font.
    pub fn with_font_file(config: TextConfig, path: impl AsRef<Path>) -> VfxResult<Self> {
        Ok(Self {
            config,
            font: Some(GlyphCache::new(FontFace::from_file(path)?)),
        })
    }

    /// Load (or replace) the font from TTF/OTF bytes.
    ///
    /// # Errors
    ///
    /// Returns [`VfxError::TextRenderError`] if the bytes are not a parsable
    /// font; the previously loaded font, if any, is kept.
    pub fn set_font_bytes(&mut self, font_data: &[u8]) -> VfxResult<()> {
        self.font = Some(GlyphCache::new(FontFace::from_bytes(font_data)?));
        Ok(())
    }

    /// Load (or replace) the font from a file path.
    ///
    /// # Errors
    ///
    /// Returns [`VfxError::TextRenderError`] if the file cannot be read or is
    /// not a parsable font; the previously loaded font, if any, is kept.
    pub fn set_font_file(&mut self, path: impl AsRef<Path>) -> VfxResult<()> {
        self.font = Some(GlyphCache::new(FontFace::from_file(path)?));
        Ok(())
    }

    /// `true` when a font has been loaded.
    #[must_use]
    pub const fn has_font(&self) -> bool {
        self.font.is_some()
    }

    /// Number of glyphs currently cached (`0` when no font is loaded).
    #[must_use]
    pub fn cached_glyphs(&self) -> usize {
        self.font.as_ref().map_or(0, GlyphCache::len)
    }

    /// Set text content.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.config.text = text.into();
    }

    /// Get reference to config.
    #[must_use]
    pub fn config(&self) -> &TextConfig {
        &self.config
    }

    /// Get mutable reference to config.
    #[must_use]
    pub fn config_mut(&mut self) -> &mut TextConfig {
        &mut self.config
    }

    /// Lay out the configured text as it would be rendered onto a frame of
    /// `frame_width` pixels, without painting anything.
    ///
    /// # Errors
    ///
    /// Returns [`VfxError::TextRenderError`] if no font is loaded, or
    /// [`VfxError::InvalidParameter`] if the font size is not positive and
    /// finite.
    pub fn layout(&self, frame_width: u32) -> VfxResult<TextLayout> {
        let size = self.validated_font_size()?;
        let align = self.config.align;
        let line_height = self.config.line_height;
        let max_width = self.wrap_width(frame_width);
        let cache = self.font.as_ref().ok_or_else(no_font)?;
        let metrics = cache.font().line_metrics(size);
        let font = cache.font();
        Ok(layout::layout_block(
            &self.config.text,
            size,
            metrics,
            line_height,
            align,
            max_width,
            |c| font.advance_width(c, size),
        ))
    }

    /// Word-wrap width in pixels for a frame of `frame_width`.
    fn wrap_width(&self, frame_width: u32) -> Option<f32> {
        self.config
            .max_width
            .map(|fraction| fraction * frame_width as f32)
            .filter(|w| w.is_finite() && *w > 0.0)
    }

    /// Validate the configured font size.
    fn validated_font_size(&self) -> VfxResult<f32> {
        let size = self.config.font_size;
        if size.is_finite() && size > 0.0 {
            Ok(size)
        } else {
            Err(VfxError::InvalidParameter(format!(
                "font_size must be positive and finite, got {size}"
            )))
        }
    }

    /// Render the configured text onto `frame`, returning the number of pixels
    /// the text (including outline and shadow) actually changed.
    ///
    /// `0` means nothing visible landed on the frame — every character was
    /// whitespace or missing from the font, or the block fell outside the
    /// frame.
    ///
    /// # Errors
    ///
    /// Returns [`VfxError::TextRenderError`] if no font is loaded,
    /// [`VfxError::InvalidParameter`] for a non-positive font size, or
    /// [`VfxError::InvalidDimensions`] if the block is too large to rasterize.
    pub fn render_onto(&mut self, frame: &mut Frame) -> VfxResult<usize> {
        if self.config.text.is_empty() {
            return Ok(0);
        }
        let size = self.validated_font_size()?;
        if !self.has_font() {
            return Err(no_font());
        }

        let laid_out = self.layout(frame.width)?;
        if laid_out.is_empty() {
            return Ok(0);
        }

        let radius = self.config.outline_radius();
        let cache = self.font.as_mut().ok_or_else(no_font)?;
        let mask = paint::build_mask(
            cache,
            &laid_out.glyphs,
            size,
            laid_out.width,
            laid_out.height,
            radius,
        )?;

        let (block_x, block_y) = layout::anchor_top_left(
            frame.width,
            frame.height,
            laid_out.width,
            laid_out.height,
            self.config.x,
            self.config.y,
        );

        // The outline is a dilation of the fill mask, so the shadow — which is
        // a copy of the whole visible shape — must use the outlined mask when
        // there is one.
        let outline: Option<CoverageMask> = if radius > 0.0 {
            Some(paint::dilate(&mask, radius))
        } else {
            None
        };

        let mut painted = 0usize;
        if self.config.has_shadow() {
            let silhouette = outline.as_ref().unwrap_or(&mask);
            painted += paint::composite(
                frame,
                silhouette,
                block_x + self.config.shadow_x,
                block_y + self.config.shadow_y,
                self.config.shadow_color,
            );
        }
        if let Some(outline) = outline.as_ref() {
            painted +=
                paint::composite(frame, outline, block_x, block_y, self.config.outline_color);
        }
        painted += paint::composite(frame, &mask, block_x, block_y, self.config.color);
        Ok(painted)
    }
}

/// The honest "no font supplied" error.
fn no_font() -> VfxError {
    VfxError::TextRenderError(NO_FONT.to_string())
}

/// Copy `input` into `output`, verifying they describe the same frame.
///
/// # Errors
///
/// Returns [`VfxError::InvalidDimensions`] if the frames differ in size, or
/// [`VfxError::BufferSizeMismatch`] if either buffer does not match its own
/// declared dimensions.
fn copy_frame(input: &Frame, output: &mut Frame) -> VfxResult<()> {
    if input.width != output.width || input.height != output.height {
        return Err(VfxError::InvalidDimensions {
            width: output.width,
            height: output.height,
        });
    }
    let expected = input.byte_size();
    if input.data.len() != expected {
        return Err(VfxError::BufferSizeMismatch {
            expected,
            actual: input.data.len(),
        });
    }
    if output.data.len() != expected {
        return Err(VfxError::BufferSizeMismatch {
            expected,
            actual: output.data.len(),
        });
    }
    output.data.copy_from_slice(&input.data);
    Ok(())
}

impl VideoEffect for TextRenderer {
    fn name(&self) -> &'static str {
        "Text Renderer"
    }

    fn description(&self) -> &'static str {
        "Real glyph rasterization with multi-line layout, outline and drop shadow"
    }

    fn apply(
        &mut self,
        input: &Frame,
        output: &mut Frame,
        _params: &EffectParams,
    ) -> VfxResult<()> {
        copy_frame(input, output)?;
        if self.config.text.is_empty() {
            // Nothing to rasterize: an honest identity pass-through.
            return Ok(());
        }
        self.render_onto(output)?;
        Ok(())
    }

    fn reset(&mut self) {
        if let Some(cache) = self.font.as_mut() {
            cache.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font path supplied by the environment, or `None`.
    ///
    /// No font ships in this tree, so pixel-level tests are opt-in:
    /// `OXIMEDIA_TEST_FONT=/path/to/DejaVuSans.ttf cargo test -p oximedia-vfx`.
    fn test_font() -> Option<std::path::PathBuf> {
        std::env::var_os("OXIMEDIA_TEST_FONT").map(std::path::PathBuf::from)
    }

    fn flat_frame(w: u32, h: u32, rgba: [u8; 4]) -> Frame {
        let mut f = Frame::new(w, h).expect("should succeed in test");
        f.clear(rgba);
        f
    }

    #[test]
    fn test_text_config() {
        let config = TextConfig::new("Hello")
            .with_font_size(36.0)
            .with_color(Color::rgb(255, 0, 0))
            .with_position(0.5, 0.5);

        assert_eq!(config.text, "Hello");
        assert_eq!(config.font_size, 36.0);
    }

    #[test]
    fn test_text_renderer_nonempty_text_returns_honest_err_not_fake_rectangles() {
        // With no font supplied, non-empty text must not be silently
        // "rendered" as solid-color rectangles; it must report an honest error
        // instead of fabricating success.
        let config = TextConfig::new("Test").with_font_size(24.0);
        let mut renderer = TextRenderer::new(config).expect("should succeed in test");

        let input = Frame::new(200, 100).expect("should succeed in test");
        let mut output = Frame::new(200, 100).expect("should succeed in test");
        let params = EffectParams::new();
        let err = renderer
            .apply(&input, &mut output, &params)
            .expect_err("text rendering without a font must fail honestly");

        assert!(
            matches!(err, VfxError::TextRenderError(_)),
            "expected VfxError::TextRenderError, got {err:?}"
        );
    }

    #[test]
    fn test_text_renderer_empty_text_is_identity_passthrough() {
        // Empty text has nothing to rasterize, so it is honestly a no-op:
        // output must exactly equal input, and this must succeed.
        let config = TextConfig::new("");
        let mut renderer = TextRenderer::new(config).expect("should succeed in test");

        let input = Frame::new(8, 6).expect("should succeed in test");
        let mut output = Frame::new(8, 6).expect("should succeed in test");
        let params = EffectParams::new();
        renderer
            .apply(&input, &mut output, &params)
            .expect("empty text must be a harmless identity pass-through");

        for y in 0..output.height {
            for x in 0..output.width {
                assert_eq!(
                    output.get_pixel(x, y).unwrap_or([0, 0, 0, 0]),
                    input.get_pixel(x, y).unwrap_or([0, 0, 0, 0]),
                    "identity pass-through must match input exactly at ({x},{y})"
                );
            }
        }
    }

    // ── font-free behaviour ──────────────────────────────────────────────

    #[test]
    fn no_font_error_names_the_constructors_that_supply_one() {
        let mut renderer =
            TextRenderer::new(TextConfig::new("hi")).expect("should succeed in test");
        assert!(!renderer.has_font());
        let mut frame = flat_frame(32, 32, [0, 0, 0, 255]);
        let VfxError::TextRenderError(msg) = renderer
            .render_onto(&mut frame)
            .expect_err("no font must error")
        else {
            panic!("expected TextRenderError");
        };
        assert!(msg.contains("with_font_bytes"), "unhelpful message: {msg}");
        assert!(msg.contains("set_font_bytes"), "unhelpful message: {msg}");
        assert!(
            !msg.contains("not yet implemented") && !msg.contains("TODO"),
            "the renderer is implemented; the message must say a font is missing: {msg}"
        );
    }

    #[test]
    fn invalid_font_bytes_are_rejected_and_leave_the_renderer_font_free() {
        let mut renderer =
            TextRenderer::new(TextConfig::new("hi")).expect("should succeed in test");
        assert!(renderer.set_font_bytes(b"not a font").is_err());
        assert!(
            !renderer.has_font(),
            "a failed load must not pretend to succeed"
        );
    }

    #[test]
    fn empty_text_renders_nothing_even_without_a_font() {
        let mut renderer = TextRenderer::new(TextConfig::new("")).expect("should succeed in test");
        let mut frame = flat_frame(8, 8, [1, 2, 3, 255]);
        assert_eq!(renderer.render_onto(&mut frame).expect("no-op"), 0);
    }

    #[test]
    fn mismatched_frame_sizes_are_rejected() {
        let mut renderer = TextRenderer::new(TextConfig::new("")).expect("should succeed in test");
        let input = Frame::new(8, 8).expect("should succeed in test");
        let mut output = Frame::new(8, 4).expect("should succeed in test");
        let err = renderer
            .apply(&input, &mut output, &EffectParams::new())
            .expect_err("size mismatch must be reported, not silently cropped");
        assert!(
            matches!(err, VfxError::InvalidDimensions { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn corrupt_frame_buffer_is_reported() {
        let mut renderer = TextRenderer::new(TextConfig::new("")).expect("should succeed in test");
        let mut input = Frame::new(8, 8).expect("should succeed in test");
        input.data.truncate(4);
        let mut output = Frame::new(8, 8).expect("should succeed in test");
        let err = renderer
            .apply(&input, &mut output, &EffectParams::new())
            .expect_err("a short buffer must be reported");
        assert!(
            matches!(err, VfxError::BufferSizeMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn non_positive_font_size_is_rejected() {
        for bad in [0.0_f32, -12.0, f32::NAN, f32::INFINITY] {
            let mut renderer =
                TextRenderer::new(TextConfig::new("hi").with_font_size(bad)).expect("test");
            let mut frame = flat_frame(16, 16, [0, 0, 0, 255]);
            let err = renderer
                .render_onto(&mut frame)
                .expect_err("a non-positive or non-finite font size must be rejected");
            assert!(
                matches!(err, VfxError::InvalidParameter(_)),
                "font size {bad} gave {err:?}"
            );
        }
    }

    #[test]
    fn config_flags_reflect_the_configured_effects() {
        let plain = TextConfig::new("x");
        assert!(!plain.has_outline());
        assert!(!plain.has_shadow());
        assert_eq!(plain.outline_radius(), 0.0);

        let decorated = TextConfig::new("x")
            .with_outline(3.0, Color::black())
            .with_shadow(2.0, 2.0, Color::black());
        assert!(decorated.has_outline());
        assert!(decorated.has_shadow());
        assert_eq!(decorated.outline_radius(), 3.0);

        let invisible = TextConfig::new("x")
            .with_outline(3.0, Color::new(0, 0, 0, 0))
            .with_shadow(2.0, 2.0, Color::new(0, 0, 0, 0));
        assert!(
            !invisible.has_outline(),
            "a transparent outline is no outline"
        );
        assert!(!invisible.has_shadow(), "a transparent shadow is no shadow");
    }

    #[test]
    fn outline_radius_is_clamped_to_the_supported_maximum() {
        let cfg = TextConfig::new("x").with_outline(1.0e6, Color::black());
        assert_eq!(cfg.outline_radius(), paint::MAX_OUTLINE_RADIUS);
    }

    #[test]
    fn wrap_width_scales_with_the_frame() {
        let renderer = TextRenderer::new(TextConfig::new("x").with_max_width(0.5))
            .expect("should succeed in test");
        assert_eq!(renderer.wrap_width(800), Some(400.0));

        let none = TextRenderer::new(TextConfig::new("x")).expect("should succeed in test");
        assert_eq!(none.wrap_width(800), None);

        let zero = TextRenderer::new(TextConfig::new("x").with_max_width(0.0))
            .expect("should succeed in test");
        assert_eq!(
            zero.wrap_width(800),
            None,
            "a zero wrap width means no wrap"
        );
    }

    #[test]
    fn layout_without_a_font_is_an_honest_error() {
        let renderer = TextRenderer::new(TextConfig::new("hi")).expect("test");
        assert!(matches!(
            renderer.layout(640),
            Err(VfxError::TextRenderError(_))
        ));
    }

    #[test]
    fn renderer_is_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TextRenderer>();
    }

    #[test]
    fn description_does_not_claim_to_be_unimplemented() {
        let renderer = TextRenderer::new(TextConfig::new("")).expect("test");
        let desc = renderer.description();
        assert!(
            !desc.contains("not yet implemented"),
            "stale honesty note: {desc}"
        );
        assert!(!desc.contains("TODO"), "stale honesty note: {desc}");
    }

    // ── real-font pixel tests (opt-in via OXIMEDIA_TEST_FONT) ────────────

    #[test]
    fn renders_real_glyphs_onto_a_frame() {
        let Some(font) = test_font() else {
            eprintln!(
                "skipping: no font in this tree — re-run with \
                 OXIMEDIA_TEST_FONT=/path/to/font.ttf (e.g. a DejaVuSans.ttf)."
            );
            return;
        };
        let config = TextConfig::new("Hg")
            .with_font_size(48.0)
            .with_color(Color::white());
        let mut renderer = TextRenderer::with_font_file(config, &font).expect("font must load");

        let input = flat_frame(256, 128, [0, 0, 0, 255]);
        let mut output = Frame::new(256, 128).expect("frame");
        renderer
            .apply(&input, &mut output, &EffectParams::new())
            .expect("rendering with a real font must succeed");

        let changed = output
            .data
            .chunks_exact(4)
            .zip(input.data.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > 0, "real glyphs must actually change pixels");
        assert!(
            renderer.cached_glyphs() >= 2,
            "both characters must have been rasterized"
        );
    }

    #[test]
    fn rendered_text_is_centred_on_the_configured_position() {
        let Some(font) = test_font() else {
            eprintln!("skipping: re-run with OXIMEDIA_TEST_FONT=/path/to/font.ttf.");
            return;
        };
        let mut renderer = TextRenderer::with_font_file(
            TextConfig::new("O")
                .with_font_size(64.0)
                .with_position(0.25, 0.5),
            &font,
        )
        .expect("font must load");

        let mut frame = flat_frame(256, 128, [0, 0, 0, 255]);
        assert!(renderer.render_onto(&mut frame).expect("render") > 0);

        let mut sum_x = 0f64;
        let mut count = 0f64;
        for y in 0..frame.height {
            for x in 0..frame.width {
                if frame.get_pixel(x, y).unwrap_or([0; 4])[0] > 40 {
                    sum_x += f64::from(x);
                    count += 1.0;
                }
            }
        }
        assert!(count > 0.0, "nothing was painted");
        let centroid = sum_x / count;
        assert!(
            (centroid - 64.0).abs() < 16.0,
            "text should sit near x = 0.25 * 256 = 64, centroid was {centroid}"
        );
    }

    #[test]
    fn outline_and_shadow_paint_more_pixels_than_plain_fill() {
        let Some(font) = test_font() else {
            eprintln!("skipping: re-run with OXIMEDIA_TEST_FONT=/path/to/font.ttf.");
            return;
        };
        let bytes = std::fs::read(&font).expect("font file must be readable");

        let mut plain =
            TextRenderer::with_font_bytes(TextConfig::new("Hg").with_font_size(48.0), &bytes)
                .expect("font must load");
        let mut decorated = TextRenderer::with_font_bytes(
            TextConfig::new("Hg")
                .with_font_size(48.0)
                .with_outline(3.0, Color::rgb(255, 0, 0))
                .with_shadow(6.0, 6.0, Color::rgb(0, 0, 255)),
            &bytes,
        )
        .expect("font must load");

        let mut plain_frame = flat_frame(256, 128, [0, 0, 0, 255]);
        let mut decorated_frame = flat_frame(256, 128, [0, 0, 0, 255]);
        let plain_painted = plain.render_onto(&mut plain_frame).expect("render");
        let decorated_painted = decorated.render_onto(&mut decorated_frame).expect("render");

        assert!(plain_painted > 0);
        assert!(
            decorated_painted > plain_painted,
            "outline + shadow must cover more than the fill alone ({decorated_painted} vs {plain_painted})"
        );

        let has_outline_colour = decorated_frame
            .data
            .chunks_exact(4)
            .any(|p| p[0] > 200 && p[1] < 60 && p[2] < 60);
        let has_shadow_colour = decorated_frame
            .data
            .chunks_exact(4)
            .any(|p| p[2] > 200 && p[0] < 60 && p[1] < 60);
        assert!(
            has_outline_colour,
            "the outline colour must appear on the frame"
        );
        assert!(
            has_shadow_colour,
            "the shadow colour must appear on the frame"
        );
    }

    #[test]
    fn word_wrap_and_alignment_change_the_laid_out_block() {
        let Some(font) = test_font() else {
            eprintln!("skipping: re-run with OXIMEDIA_TEST_FONT=/path/to/font.ttf.");
            return;
        };
        let bytes = std::fs::read(&font).expect("font file must be readable");
        let text = "the quick brown fox jumps over the lazy dog";

        let unwrapped =
            TextRenderer::with_font_bytes(TextConfig::new(text).with_font_size(24.0), &bytes)
                .expect("font must load");
        let wrapped = TextRenderer::with_font_bytes(
            TextConfig::new(text)
                .with_font_size(24.0)
                .with_max_width(0.3),
            &bytes,
        )
        .expect("font must load");

        let flat = unwrapped.layout(640).expect("layout");
        let boxed = wrapped.layout(640).expect("layout");
        assert_eq!(flat.line_count, 1);
        assert!(boxed.line_count > 1, "0.3 * 640px must force wrapping");
        assert!(boxed.width < flat.width, "wrapping must narrow the block");
        assert!(boxed.height > flat.height, "wrapping must deepen the block");
        assert_eq!(
            boxed.glyphs.len(),
            flat.glyphs.len(),
            "wrapping must not drop glyphs"
        );
    }

    #[test]
    fn fade_alpha_dims_the_rendered_text() {
        let Some(font) = test_font() else {
            eprintln!("skipping: re-run with OXIMEDIA_TEST_FONT=/path/to/font.ttf.");
            return;
        };
        let bytes = std::fs::read(&font).expect("font file must be readable");
        let brightest = |renderer: &mut TextRenderer| -> u8 {
            let mut frame = flat_frame(200, 100, [0, 0, 0, 255]);
            renderer.render_onto(&mut frame).expect("render");
            frame.data.chunks_exact(4).map(|p| p[0]).max().unwrap_or(0)
        };

        let mut opaque = TextRenderer::with_font_bytes(
            TextConfig::new("W")
                .with_font_size(48.0)
                .with_color(Color::white()),
            &bytes,
        )
        .expect("font must load");
        let mut faded = TextRenderer::with_font_bytes(
            TextConfig::new("W")
                .with_font_size(48.0)
                .with_color(Color::new(255, 255, 255, 64)),
            &bytes,
        )
        .expect("font must load");

        let full = brightest(&mut opaque);
        let dim = brightest(&mut faded);
        assert!(full > 200, "opaque white text must be bright, got {full}");
        assert!(
            dim < full / 2,
            "alpha 64/255 must dim the text ({dim} vs {full})"
        );
    }

    #[test]
    fn reset_drops_the_glyph_cache_but_keeps_the_font() {
        let Some(font) = test_font() else {
            eprintln!("skipping: re-run with OXIMEDIA_TEST_FONT=/path/to/font.ttf.");
            return;
        };
        let mut renderer =
            TextRenderer::with_font_file(TextConfig::new("abc").with_font_size(20.0), &font)
                .expect("font must load");
        let mut frame = flat_frame(128, 64, [0, 0, 0, 255]);
        renderer.render_onto(&mut frame).expect("render");
        assert!(renderer.cached_glyphs() > 0);
        renderer.reset();
        assert_eq!(renderer.cached_glyphs(), 0);
        assert!(renderer.has_font(), "reset must not unload the font");
    }
}
