//! Text animation effects.
//!
//! Each frame is rendered from a *derived* configuration: the pristine base
//! [`TextConfig`] the animation was built with is never mutated, and the
//! per-frame values (revealed characters, alpha, position, size) are computed
//! from it and the current progress. Deriving rather than mutating in place is
//! what makes the animations replayable — a typewriter that truncated its own
//! text, or a scale that multiplied its own font size, would degrade a little
//! further on every single frame.

use std::path::Path;

use crate::{EffectParams, Frame, VfxResult, VideoEffect};
use serde::{Deserialize, Serialize};

use super::render::{TextConfig, TextRenderer};

/// Text animation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnimationType {
    /// Typewriter effect.
    Typewriter,
    /// Fade in.
    FadeIn,
    /// Fade out.
    FadeOut,
    /// Slide in from left.
    SlideInLeft,
    /// Slide in from right.
    SlideInRight,
    /// Scale in.
    ScaleIn,
    /// Bounce in.
    BounceIn,
}

/// Text animation effect.
pub struct TextAnimation {
    animation_type: AnimationType,
    duration: f32,
    renderer: TextRenderer,
    /// Pristine configuration every frame is derived from.
    base: TextConfig,
    start_time: f64,
}

impl TextAnimation {
    /// Create a new text animation with no font loaded.
    ///
    /// Rendering non-empty text through it reports the underlying renderer's
    /// honest "no font supplied" error; see [`TextRenderer`].
    ///
    /// # Errors
    ///
    /// Returns an error if text renderer creation fails.
    pub fn new(animation_type: AnimationType, config: TextConfig) -> VfxResult<Self> {
        let base = config.clone();
        Ok(Self {
            animation_type,
            duration: 2.0,
            renderer: TextRenderer::new(config)?,
            base,
            start_time: 0.0,
        })
    }

    /// Create a text animation from caller-supplied TTF/OTF font bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes are not a parsable font.
    pub fn with_font_bytes(
        animation_type: AnimationType,
        config: TextConfig,
        font_data: &[u8],
    ) -> VfxResult<Self> {
        let base = config.clone();
        Ok(Self {
            animation_type,
            duration: 2.0,
            renderer: TextRenderer::with_font_bytes(config, font_data)?,
            base,
            start_time: 0.0,
        })
    }

    /// Create a text animation from a font file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not a parsable font.
    pub fn with_font_file(
        animation_type: AnimationType,
        config: TextConfig,
        path: impl AsRef<Path>,
    ) -> VfxResult<Self> {
        let base = config.clone();
        Ok(Self {
            animation_type,
            duration: 2.0,
            renderer: TextRenderer::with_font_file(config, path)?,
            base,
            start_time: 0.0,
        })
    }

    /// Load (or replace) the font from TTF/OTF bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes are not a parsable font.
    pub fn set_font_bytes(&mut self, font_data: &[u8]) -> VfxResult<()> {
        self.renderer.set_font_bytes(font_data)
    }

    /// Load (or replace) the font from a file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not a parsable font.
    pub fn set_font_file(&mut self, path: impl AsRef<Path>) -> VfxResult<()> {
        self.renderer.set_font_file(path)
    }

    /// `true` when a font has been loaded.
    #[must_use]
    pub fn has_font(&self) -> bool {
        self.renderer.has_font()
    }

    /// Set animation duration in seconds.
    #[must_use]
    pub const fn with_duration(mut self, duration: f32) -> Self {
        self.duration = duration;
        self
    }

    /// Set the time the animation starts at, in seconds.
    #[must_use]
    pub const fn with_start_time(mut self, start_time: f64) -> Self {
        self.start_time = start_time;
        self
    }

    /// The pristine configuration frames are derived from.
    #[must_use]
    pub const fn config(&self) -> &TextConfig {
        &self.base
    }

    /// Mutable access to the pristine configuration.
    ///
    /// Edits here affect every subsequent frame; the per-frame derived config
    /// is recomputed from this on each [`apply`](VideoEffect::apply).
    pub fn config_mut(&mut self) -> &mut TextConfig {
        &mut self.base
    }

    /// Set text content.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.base.text = text.into();
    }

    /// Normalized progress through the animation at `time`.
    ///
    /// A zero or non-finite duration means "already finished" rather than a
    /// NaN that would silently poison every derived value.
    #[must_use]
    pub fn get_progress(&self, time: f64) -> f32 {
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return 1.0;
        }
        let elapsed = (time - self.start_time) as f32;
        (elapsed / self.duration).clamp(0.0, 1.0)
    }

    /// Derive the configuration for a given progress from the pristine base.
    #[must_use]
    pub fn config_at(&self, progress: f32) -> TextConfig {
        let progress = progress.clamp(0.0, 1.0);
        let mut config = self.base.clone();
        match self.animation_type {
            AnimationType::Typewriter => {
                // Count *characters*, not bytes: `len()` would reveal the whole
                // string immediately for any multi-byte text.
                let total = self.base.text.chars().count();
                let shown = ((total as f32) * progress) as usize;
                config.text = self.base.text.chars().take(shown.min(total)).collect();
            }
            AnimationType::FadeIn => {
                config.color.a = (f32::from(self.base.color.a) * progress).round() as u8;
            }
            AnimationType::FadeOut => {
                config.color.a = (f32::from(self.base.color.a) * (1.0 - progress)).round() as u8;
            }
            AnimationType::SlideInLeft => {
                // Travel from the left frame edge to the configured position.
                config.x = self.base.x * progress;
            }
            AnimationType::SlideInRight => {
                // Travel from the right frame edge to the configured position.
                config.x = 1.0 - (1.0 - self.base.x) * progress;
            }
            AnimationType::ScaleIn => {
                config.font_size = self.base.font_size * progress.max(0.01);
            }
            AnimationType::BounceIn => {
                let bounce = if progress < 0.5 {
                    2.0 * progress * progress
                } else {
                    1.0 - (-2.0 * progress + 2.0).powi(2) / 2.0
                };
                config.font_size = self.base.font_size * bounce.max(0.01);
            }
        }
        config
    }
}

impl VideoEffect for TextAnimation {
    fn name(&self) -> &'static str {
        "Text Animation"
    }

    fn description(&self) -> &'static str {
        "Animated text with various effects"
    }

    fn apply(&mut self, input: &Frame, output: &mut Frame, params: &EffectParams) -> VfxResult<()> {
        let progress = self.get_progress(params.time);
        let frame_config = self.config_at(progress);
        *self.renderer.config_mut() = frame_config;
        self.renderer.apply(input, output, params)
    }

    fn reset(&mut self) {
        let base = self.base.clone();
        *self.renderer.config_mut() = base;
        self.renderer.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    fn anim(kind: AnimationType, config: TextConfig) -> TextAnimation {
        TextAnimation::new(kind, config)
            .expect("should succeed in test")
            .with_duration(1.0)
    }

    #[test]
    fn test_text_animation_nonempty_text_propagates_honest_render_err() {
        // The underlying TextRenderer has no font loaded (none ships in this
        // tree, see oximedia_vfx::text::font), so animating non-empty text
        // must propagate that honest error rather than the animation layer
        // masking it as success.
        let config = TextConfig::new("Animated").with_font_size(32.0);
        let mut animation = TextAnimation::new(AnimationType::FadeIn, config)
            .expect("should succeed in test")
            .with_duration(1.0);

        let input = Frame::new(200, 100).expect("should succeed in test");
        let mut output = Frame::new(200, 100).expect("should succeed in test");
        let params = EffectParams::new().with_time(0.5);
        let err = animation
            .apply(&input, &mut output, &params)
            .expect_err("animating non-empty text without a font must fail honestly");
        assert!(matches!(err, crate::VfxError::TextRenderError(_)));
    }

    #[test]
    fn test_text_animation_empty_text_succeeds_as_passthrough() {
        let config = TextConfig::new("").with_font_size(32.0);
        let mut animation = TextAnimation::new(AnimationType::FadeIn, config)
            .expect("should succeed in test")
            .with_duration(1.0);

        let input = Frame::new(200, 100).expect("should succeed in test");
        let mut output = Frame::new(200, 100).expect("should succeed in test");
        let params = EffectParams::new().with_time(0.5);
        animation
            .apply(&input, &mut output, &params)
            .expect("empty text animation must succeed as an identity pass-through");
    }

    // ── progress ─────────────────────────────────────────────────────────

    #[test]
    fn progress_is_clamped_to_the_duration() {
        let a = anim(AnimationType::FadeIn, TextConfig::new("x")).with_duration(2.0);
        assert_eq!(a.get_progress(-5.0), 0.0);
        assert_eq!(a.get_progress(0.0), 0.0);
        assert_eq!(a.get_progress(1.0), 0.5);
        assert_eq!(a.get_progress(2.0), 1.0);
        assert_eq!(a.get_progress(99.0), 1.0);
    }

    #[test]
    fn progress_honours_the_start_time() {
        let a = anim(AnimationType::FadeIn, TextConfig::new("x"))
            .with_duration(2.0)
            .with_start_time(10.0);
        assert_eq!(a.get_progress(10.0), 0.0);
        assert_eq!(a.get_progress(11.0), 0.5);
        assert_eq!(a.get_progress(12.0), 1.0);
    }

    /// A zero duration must not produce NaN (which `clamp` propagates).
    #[test]
    fn zero_or_invalid_duration_reads_as_finished() {
        for bad in [0.0_f32, -1.0, f32::NAN] {
            let a = anim(AnimationType::FadeIn, TextConfig::new("x")).with_duration(bad);
            let p = a.get_progress(0.5);
            assert!(
                p.is_finite(),
                "duration {bad} produced a non-finite progress"
            );
            assert_eq!(p, 1.0, "duration {bad} should read as finished");
        }
    }

    // ── derived configuration ────────────────────────────────────────────

    #[test]
    fn typewriter_reveals_characters_progressively() {
        let a = anim(AnimationType::Typewriter, TextConfig::new("abcd"));
        assert_eq!(a.config_at(0.0).text, "");
        assert_eq!(a.config_at(0.5).text, "ab");
        assert_eq!(a.config_at(1.0).text, "abcd");
    }

    /// Regression: character-counting, not byte-counting. With `len()` a
    /// multi-byte string revealed itself instantly.
    #[test]
    fn typewriter_counts_characters_not_bytes() {
        // 4 characters, 12 UTF-8 bytes.
        let a = anim(AnimationType::Typewriter, TextConfig::new("日本語字"));
        assert_eq!(a.config_at(0.0).text, "");
        assert_eq!(a.config_at(0.5).text, "日本");
        assert_eq!(a.config_at(1.0).text, "日本語字");
    }

    /// Regression: the animation used to truncate its own config, so the full
    /// text was destroyed by the first partial frame and never came back.
    #[test]
    fn typewriter_does_not_consume_its_own_text() {
        let mut a = anim(AnimationType::Typewriter, TextConfig::new("abcd"));
        let input = Frame::new(16, 16).expect("frame");
        let mut output = Frame::new(16, 16).expect("frame");
        // Render a mid-animation frame (errors: no font — irrelevant here).
        let _ = a.apply(&input, &mut output, &EffectParams::new().with_time(0.25));
        assert_eq!(
            a.config().text,
            "abcd",
            "the base text must survive a frame"
        );
        assert_eq!(
            a.config_at(1.0).text,
            "abcd",
            "the finished frame must still show the whole string"
        );
    }

    /// Regression: `font_size *= progress` compounded across frames, so a
    /// scaled-in title shrank towards zero the longer it played.
    #[test]
    fn scale_in_does_not_compound_across_frames() {
        let mut a = anim(
            AnimationType::ScaleIn,
            TextConfig::new("x").with_font_size(100.0),
        );
        let input = Frame::new(16, 16).expect("frame");
        let mut output = Frame::new(16, 16).expect("frame");
        for step in 0..5 {
            let t = f64::from(step) * 0.1;
            let _ = a.apply(&input, &mut output, &EffectParams::new().with_time(t));
        }
        assert_eq!(a.config().font_size, 100.0, "the base size must not decay");
        assert_eq!(a.config_at(0.5).font_size, 50.0);
        assert_eq!(a.config_at(1.0).font_size, 100.0);
    }

    #[test]
    fn scale_in_never_reaches_a_zero_font_size() {
        let a = anim(
            AnimationType::ScaleIn,
            TextConfig::new("x").with_font_size(40.0),
        );
        let size = a.config_at(0.0).font_size;
        assert!(size > 0.0, "a zero font size would be rejected downstream");
    }

    #[test]
    fn bounce_in_overshoots_then_settles_at_the_base_size() {
        let a = anim(
            AnimationType::BounceIn,
            TextConfig::new("x").with_font_size(50.0),
        );
        assert!(a.config_at(0.0).font_size > 0.0);
        assert_eq!(a.config_at(1.0).font_size, 50.0);
        assert!(
            a.config_at(0.5).font_size < 50.0,
            "the bounce curve is below 1.0 at its midpoint"
        );
    }

    #[test]
    fn fade_in_and_out_interpolate_the_configured_alpha() {
        let cfg = TextConfig::new("x").with_color(Color::new(255, 255, 255, 200));
        let fade_in = anim(AnimationType::FadeIn, cfg.clone());
        assert_eq!(fade_in.config_at(0.0).color.a, 0);
        assert_eq!(fade_in.config_at(0.5).color.a, 100);
        assert_eq!(fade_in.config_at(1.0).color.a, 200);

        let fade_out = anim(AnimationType::FadeOut, cfg);
        assert_eq!(fade_out.config_at(0.0).color.a, 200);
        assert_eq!(fade_out.config_at(1.0).color.a, 0);
    }

    #[test]
    fn fades_do_not_compound_across_frames() {
        let mut a = anim(
            AnimationType::FadeOut,
            TextConfig::new("x").with_color(Color::new(255, 255, 255, 255)),
        );
        let input = Frame::new(16, 16).expect("frame");
        let mut output = Frame::new(16, 16).expect("frame");
        for step in 0..4 {
            let t = f64::from(step) * 0.2;
            let _ = a.apply(&input, &mut output, &EffectParams::new().with_time(t));
        }
        assert_eq!(a.config().color.a, 255, "the base alpha must not decay");
        assert_eq!(a.config_at(0.0).color.a, 255);
    }

    #[test]
    fn slides_start_at_a_frame_edge_and_end_at_the_configured_position() {
        let cfg = TextConfig::new("x").with_position(0.5, 0.5);

        let left = anim(AnimationType::SlideInLeft, cfg.clone());
        assert_eq!(left.config_at(0.0).x, 0.0, "starts at the left edge");
        assert_eq!(left.config_at(1.0).x, 0.5, "ends at the configured x");

        let right = anim(AnimationType::SlideInRight, cfg);
        assert_eq!(right.config_at(0.0).x, 1.0, "starts at the right edge");
        assert_eq!(right.config_at(1.0).x, 0.5, "ends at the configured x");
    }

    #[test]
    fn slides_leave_the_vertical_position_alone() {
        let cfg = TextConfig::new("x").with_position(0.5, 0.25);
        let left = anim(AnimationType::SlideInLeft, cfg);
        assert_eq!(left.config_at(0.3).y, 0.25);
    }

    #[test]
    fn config_at_clamps_out_of_range_progress() {
        let a = anim(AnimationType::Typewriter, TextConfig::new("abcd"));
        assert_eq!(a.config_at(-1.0).text, "");
        assert_eq!(a.config_at(7.0).text, "abcd");
    }

    #[test]
    fn typewriter_on_empty_text_stays_empty() {
        let a = anim(AnimationType::Typewriter, TextConfig::new(""));
        assert_eq!(a.config_at(0.5).text, "");
        assert_eq!(a.config_at(1.0).text, "");
    }

    #[test]
    fn set_text_updates_the_base_the_animation_derives_from() {
        let mut a = anim(AnimationType::Typewriter, TextConfig::new("old"));
        a.set_text("brand new");
        assert_eq!(a.config().text, "brand new");
        assert_eq!(a.config_at(1.0).text, "brand new");
    }

    #[test]
    fn reset_restores_the_renderer_to_the_base_config() {
        let mut a = anim(AnimationType::Typewriter, TextConfig::new("abcd"));
        let input = Frame::new(16, 16).expect("frame");
        let mut output = Frame::new(16, 16).expect("frame");
        let _ = a.apply(&input, &mut output, &EffectParams::new().with_time(0.25));
        a.reset();
        assert_eq!(a.config().text, "abcd");
    }

    #[test]
    fn animation_is_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TextAnimation>();
    }

    #[test]
    fn font_loading_errors_are_reported_and_leave_the_animation_font_free() {
        let mut a = anim(AnimationType::FadeIn, TextConfig::new("x"));
        assert!(!a.has_font());
        assert!(a.set_font_bytes(b"not a font").is_err());
        assert!(!a.has_font());
    }
}
