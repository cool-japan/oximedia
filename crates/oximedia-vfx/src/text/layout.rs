//! Pure text-layout math for the VFX text renderer.
//!
//! Everything in this module is a pure function over *metrics* — per-character
//! advance widths and per-font line metrics — never over a loaded font. Line
//! breaking, word wrapping, alignment and frame anchoring are therefore fully
//! testable without a font file, which matters because no font ships in this
//! tree (see [`super::font`]).
//!
//! Coordinates are y-down pixels relative to the block's own top-left origin;
//! [`anchor_top_left`] maps that origin onto a frame.

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// Horizontal alignment of the individual lines within a text block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    /// Lines start at the block's left edge.
    Left,
    /// Lines are centered within the block.
    Center,
    /// Lines end at the block's right edge.
    Right,
}

impl Default for TextAlign {
    fn default() -> Self {
        Self::Left
    }
}

/// Vertical line metrics of one font at one size, in pixels.
///
/// Follows the `fontdue` sign convention: `ascent` is positive above the
/// baseline and `descent` is negative below it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// Distance from the baseline up to the highest ascender.
    pub ascent: f32,
    /// Distance from the baseline down to the lowest descender (negative).
    pub descent: f32,
    /// Extra leading the font asks for between lines.
    pub line_gap: f32,
    /// Recommended baseline-to-baseline distance.
    pub new_line_size: f32,
}

impl LineMetrics {
    /// Metrics derived purely from `size`, for fonts that report no horizontal
    /// line metrics table.
    #[must_use]
    pub fn fallback(size: f32) -> Self {
        Self {
            ascent: size * 0.8,
            descent: -size * 0.2,
            line_gap: 0.0,
            new_line_size: size * 1.2,
        }
    }

    /// Baseline-to-baseline distance, falling back to `size * 1.2` when the
    /// font reports nothing usable.
    #[must_use]
    pub fn line_step(&self, size: f32) -> f32 {
        if self.new_line_size.is_finite() && self.new_line_size > 0.0 {
            self.new_line_size
        } else {
            Self::fallback(size).new_line_size
        }
    }

    /// Height of a single line (ascender to descender), falling back to
    /// `size` when the font reports nothing usable.
    #[must_use]
    pub fn line_height(&self, size: f32) -> f32 {
        let h = self.ascent - self.descent;
        if h.is_finite() && h > 0.0 {
            h
        } else {
            let f = Self::fallback(size);
            f.ascent - f.descent
        }
    }

    /// Distance from the top of a line box down to its baseline.
    #[must_use]
    pub fn baseline_offset(&self, size: f32) -> f32 {
        if self.ascent.is_finite() && self.ascent > 0.0 {
            self.ascent
        } else {
            Self::fallback(size).ascent
        }
    }
}

/// One character paired with the advance width the font reports for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharAdvance {
    /// The character.
    pub c: char,
    /// Its advance width in pixels.
    pub advance: f32,
}

impl CharAdvance {
    /// Pair a character with its advance.
    #[must_use]
    pub const fn new(c: char, advance: f32) -> Self {
        Self { c, advance }
    }
}

/// A glyph placed by the layout engine, relative to the block's top-left
/// origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    /// The character this position was computed for.
    pub c: char,
    /// Pen x position (the glyph's own bitmap may start left of this).
    pub pen_x: f32,
    /// Baseline y position for the line this glyph sits on.
    pub baseline_y: f32,
}

/// The result of laying out a text block.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayout {
    /// Every non-whitespace glyph, in reading order.
    pub glyphs: Vec<PlacedGlyph>,
    /// Width of the widest line, in pixels.
    pub width: f32,
    /// Total block height, in pixels.
    pub height: f32,
    /// Number of laid-out lines, including hard-break blank lines.
    pub line_count: usize,
}

impl TextLayout {
    /// An empty layout, produced for empty input.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            glyphs: Vec::new(),
            width: 0.0,
            height: 0.0,
            line_count: 0,
        }
    }

    /// `true` when nothing was placed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

/// Width of `items[range]` with trailing whitespace trimmed.
///
/// Trailing spaces are excluded because a wrapped line's break whitespace stays
/// attached to the line it broke from; counting it would make right-aligned and
/// centered lines drift.
#[must_use]
pub fn visible_width(items: &[CharAdvance], range: Range<usize>) -> f32 {
    let slice = items.get(range).unwrap_or(&[]);
    let end = slice
        .iter()
        .rposition(|it| !it.c.is_whitespace())
        .map_or(0, |i| i + 1);
    slice[..end].iter().map(|it| it.advance).sum()
}

/// Greedy word wrap of one hard line into ranges over `items`.
///
/// Breaks at whitespace when possible and mid-word only when a single word is
/// itself wider than `max_width`. `None` (or a non-positive width) means no
/// wrapping: the whole slice is one line.
#[must_use]
pub fn wrap_line(items: &[CharAdvance], max_width: Option<f32>) -> Vec<Range<usize>> {
    let mut lines: Vec<Range<usize>> = Vec::new();
    let Some(max_w) = max_width.filter(|w| w.is_finite() && *w > 0.0) else {
        // No wrapping: one line covering everything.
        lines.push(0..items.len());
        return lines;
    };

    let mut start = 0usize;
    let mut width = 0.0f32;
    // Index just past the most recent whitespace run — the next legal break.
    let mut break_at: Option<usize> = None;
    let mut i = 0usize;

    while i < items.len() {
        let item = items[i];
        if item.c.is_whitespace() {
            // Whitespace never forces a break: it is allowed to overhang the
            // wrap width, exactly as it does in a browser or a subtitle box.
            width += item.advance;
            i += 1;
            break_at = Some(i);
            continue;
        }

        if width + item.advance > max_w && i > start {
            let brk = match break_at {
                Some(b) if b > start && b <= i => b,
                // No whitespace to break at: split mid-word rather than
                // letting one long token overflow unboundedly.
                _ => i,
            };
            lines.push(start..brk);
            start = brk;
            while start < items.len() && items[start].c.is_whitespace() {
                start += 1;
            }
            width = 0.0;
            break_at = None;
            i = start;
            continue;
        }

        width += item.advance;
        i += 1;
    }

    if start < items.len() {
        lines.push(start..items.len());
    } else if lines.is_empty() {
        lines.push(0..0);
    }
    lines
}

/// Horizontal offset of a line of `line_width` inside a block of
/// `block_width`, for the given alignment.
#[must_use]
pub fn align_offset(align: TextAlign, block_width: f32, line_width: f32) -> f32 {
    match align {
        TextAlign::Left => 0.0,
        TextAlign::Center => (block_width - line_width) * 0.5,
        TextAlign::Right => block_width - line_width,
    }
}

/// Lay out `text` into positioned glyphs.
///
/// `advance` supplies the per-character advance width at `size` — in
/// production that is the font, in tests it is a stub, which is the whole
/// point of keeping this function metric-driven.
///
/// `line_height_scale` multiplies the font's own baseline-to-baseline step;
/// `1.0` means "exactly what the font asks for". `max_width` is in pixels.
/// Hard breaks (`\n`, with an optional preceding `\r`) always start a new line.
pub fn layout_block(
    text: &str,
    size: f32,
    metrics: LineMetrics,
    line_height_scale: f32,
    align: TextAlign,
    max_width: Option<f32>,
    mut advance: impl FnMut(char) -> f32,
) -> TextLayout {
    if text.is_empty() {
        return TextLayout::empty();
    }

    // 1. Measure every character once, grouped by hard line.
    let paragraphs: Vec<Vec<CharAdvance>> = text
        .split('\n')
        .map(|para| {
            para.strip_suffix('\r')
                .unwrap_or(para)
                .chars()
                .map(|c| CharAdvance::new(c, advance(c)))
                .collect()
        })
        .collect();

    // 2. Word-wrap each hard line, keeping the flattened line list in order.
    let mut lines: Vec<(usize, Range<usize>, f32)> = Vec::new();
    for (para_idx, items) in paragraphs.iter().enumerate() {
        for range in wrap_line(items, max_width) {
            let w = visible_width(items, range.clone());
            lines.push((para_idx, range, w));
        }
    }

    // 3. Block box.
    let block_width = lines.iter().map(|(_, _, w)| *w).fold(0.0_f32, f32::max);
    let scale = if line_height_scale.is_finite() && line_height_scale > 0.0 {
        line_height_scale
    } else {
        1.0
    };
    let step = metrics.line_step(size) * scale;
    let first_baseline = metrics.baseline_offset(size);
    let line_count = lines.len();
    let block_height = if line_count == 0 {
        0.0
    } else {
        (line_count - 1) as f32 * step + metrics.line_height(size)
    };

    // 4. Place the glyphs.
    let mut glyphs = Vec::new();
    for (line_idx, (para_idx, range, line_width)) in lines.into_iter().enumerate() {
        let items = &paragraphs[para_idx];
        let mut pen_x = align_offset(align, block_width, line_width);
        let baseline_y = first_baseline + line_idx as f32 * step;
        for item in items.get(range).unwrap_or(&[]) {
            if !item.c.is_whitespace() {
                glyphs.push(PlacedGlyph {
                    c: item.c,
                    pen_x,
                    baseline_y,
                });
            }
            pen_x += item.advance;
        }
    }

    TextLayout {
        glyphs,
        width: block_width,
        height: block_height,
        line_count,
    }
}

/// Top-left corner, in frame pixels, of a `block_width` x `block_height` text
/// block whose centre is anchored at the normalized frame position `(x, y)`.
///
/// `(0.5, 0.5)` — the [`TextConfig`](super::render::TextConfig) default —
/// therefore centres the block on the frame.
#[must_use]
pub fn anchor_top_left(
    frame_width: u32,
    frame_height: u32,
    block_width: f32,
    block_height: f32,
    x: f32,
    y: f32,
) -> (f32, f32) {
    (
        x.clamp(0.0, 1.0) * frame_width as f32 - block_width * 0.5,
        y.clamp(0.0, 1.0) * frame_height as f32 - block_height * 0.5,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub "font" where every character advances 10px — layout math is
    /// checked against exact integers, no font file involved.
    fn uniform(_c: char) -> f32 {
        10.0
    }

    fn items(text: &str) -> Vec<CharAdvance> {
        text.chars().map(|c| CharAdvance::new(c, 10.0)).collect()
    }

    /// 10px advances, 12px line step, baseline 8px down, 10px line height.
    fn stub_metrics() -> LineMetrics {
        LineMetrics {
            ascent: 8.0,
            descent: -2.0,
            line_gap: 0.0,
            new_line_size: 12.0,
        }
    }

    // ── line metrics ─────────────────────────────────────────────────────

    /// `size * 1.2` is not exact in `f32`, so proportional metrics are checked
    /// to within a thousandth of a pixel rather than bit-for-bit.
    fn assert_close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < 1.0e-3,
            "{what}: expected ~{expected}, got {actual}"
        );
    }

    #[test]
    fn fallback_metrics_are_proportional_to_size() {
        let m = LineMetrics::fallback(100.0);
        assert_close(m.ascent, 80.0, "ascent");
        assert_close(m.descent, -20.0, "descent");
        assert_close(m.new_line_size, 120.0, "new_line_size");
        assert_close(m.line_height(100.0), 100.0, "line height");
    }

    #[test]
    fn degenerate_metrics_fall_back_instead_of_collapsing() {
        let zero = LineMetrics {
            ascent: 0.0,
            descent: 0.0,
            line_gap: 0.0,
            new_line_size: 0.0,
        };
        assert_close(
            zero.line_step(50.0),
            60.0,
            "0 step must fall back to 1.2*size",
        );
        assert_close(zero.line_height(50.0), 50.0, "line height");
        assert_close(zero.baseline_offset(50.0), 40.0, "baseline offset");
    }

    // ── visible width ────────────────────────────────────────────────────

    #[test]
    fn visible_width_sums_advances() {
        assert_eq!(visible_width(&items("abc"), 0..3), 30.0);
    }

    #[test]
    fn visible_width_trims_trailing_whitespace_only() {
        // "ab  " -> only "ab" counts; " ab" keeps its leading space.
        assert_eq!(visible_width(&items("ab  "), 0..4), 20.0);
        assert_eq!(visible_width(&items(" ab"), 0..3), 30.0);
    }

    #[test]
    fn visible_width_of_empty_range_is_zero() {
        assert_eq!(visible_width(&items("abc"), 0..0), 0.0);
        assert_eq!(visible_width(&[], 0..0), 0.0);
    }

    #[test]
    fn visible_width_of_out_of_bounds_range_is_zero() {
        assert_eq!(visible_width(&items("ab"), 0..99), 0.0);
    }

    // ── wrapping ─────────────────────────────────────────────────────────

    /// Assert that `lines` is exactly one range equal to `expected`.
    fn assert_single_line(lines: &[Range<usize>], expected: Range<usize>) {
        assert_eq!(lines.len(), 1, "expected one line, got {lines:?}");
        assert_eq!(lines[0], expected);
    }

    #[test]
    fn no_max_width_means_one_line() {
        assert_single_line(&wrap_line(&items("hello world"), None), 0..11);
        assert_single_line(&wrap_line(&items("hello world"), Some(0.0)), 0..11);
        assert_single_line(&wrap_line(&items("hello world"), Some(f32::NAN)), 0..11);
    }

    #[test]
    fn wraps_at_whitespace() {
        // 10px/char, 45px wide: "abc" (30) fits, "abc def" (70) does not.
        let it = items("abc def");
        let lines = wrap_line(&it, Some(45.0));
        assert_eq!(lines, vec![0..4, 4..7], "break must consume the space");
        assert_eq!(visible_width(&it, lines[0].clone()), 30.0);
        assert_eq!(visible_width(&it, lines[1].clone()), 30.0);
    }

    #[test]
    fn wraps_long_word_mid_word_rather_than_overflowing() {
        // A single 6-char token at 10px/char in a 25px box has no whitespace
        // to break at, so it must split mid-word.
        let lines = wrap_line(&items("abcdef"), Some(25.0));
        assert_eq!(lines, vec![0..2, 2..4, 4..6]);
    }

    #[test]
    fn wrapping_skips_the_whitespace_run_at_a_break() {
        // Two spaces between words: neither should start the next line.
        let it = items("ab  cd");
        let lines = wrap_line(&it, Some(25.0));
        assert_eq!(lines, vec![0..4, 4..6]);
        assert_eq!(it[lines[1].start].c, 'c');
    }

    #[test]
    fn trailing_whitespace_may_overhang_the_wrap_width() {
        // "ab   " is 50px wide in a 25px box but must not spawn blank lines.
        assert_single_line(&wrap_line(&items("ab   "), Some(25.0)), 0..5);
    }

    #[test]
    fn wrapping_empty_input_yields_one_empty_line() {
        assert_single_line(&wrap_line(&[], Some(50.0)), 0..0);
    }

    #[test]
    fn every_character_survives_wrapping() {
        // Regression guard: the ranges must tile the input without dropping
        // anything but the whitespace deliberately skipped at breaks.
        let text = "the quick brown fox jumps over the lazy dog";
        let it = items(text);
        for max in [15.0_f32, 35.0, 55.0, 95.0, 200.0] {
            let lines = wrap_line(&it, Some(max));
            let kept: String = lines
                .iter()
                .flat_map(|r| it[r.clone()].iter().map(|i| i.c))
                .collect();
            let stripped: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            let kept_stripped: String = kept.chars().filter(|c| !c.is_whitespace()).collect();
            assert_eq!(
                kept_stripped, stripped,
                "wrapping at {max} must not drop or duplicate characters"
            );
        }
    }

    // ── alignment ────────────────────────────────────────────────────────

    #[test]
    fn align_offsets_are_exact() {
        assert_eq!(align_offset(TextAlign::Left, 100.0, 40.0), 0.0);
        assert_eq!(align_offset(TextAlign::Center, 100.0, 40.0), 30.0);
        assert_eq!(align_offset(TextAlign::Right, 100.0, 40.0), 60.0);
    }

    #[test]
    fn default_align_is_left() {
        assert_eq!(TextAlign::default(), TextAlign::Left);
    }

    // ── block layout ─────────────────────────────────────────────────────

    #[test]
    fn empty_text_lays_out_to_nothing() {
        let l = layout_block(
            "",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Left,
            None,
            uniform,
        );
        assert!(l.is_empty());
        assert_eq!(l.line_count, 0);
        assert_eq!(l.width, 0.0);
        assert_eq!(l.height, 0.0);
    }

    #[test]
    fn single_line_advances_by_the_font_metric() {
        let l = layout_block(
            "abc",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Left,
            None,
            uniform,
        );
        assert_eq!(l.line_count, 1);
        assert_eq!(l.width, 30.0);
        assert_eq!(l.height, 10.0, "one line = ascent - descent");
        let pens: Vec<f32> = l.glyphs.iter().map(|g| g.pen_x).collect();
        assert_eq!(pens, vec![0.0, 10.0, 20.0], "pen must advance per glyph");
        assert!(l.glyphs.iter().all(|g| g.baseline_y == 8.0));
    }

    #[test]
    fn whitespace_advances_the_pen_but_places_no_glyph() {
        let l = layout_block(
            "a b",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Left,
            None,
            uniform,
        );
        assert_eq!(l.glyphs.len(), 2, "the space must not become a glyph");
        assert_eq!(l.glyphs[0].c, 'a');
        assert_eq!(l.glyphs[1].c, 'b');
        assert_eq!(l.glyphs[1].pen_x, 20.0, "space still advances the pen");
    }

    #[test]
    fn hard_breaks_start_new_lines_and_stack_baselines() {
        let l = layout_block(
            "ab\ncd",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Left,
            None,
            uniform,
        );
        assert_eq!(l.line_count, 2);
        assert_eq!(l.width, 20.0);
        assert_eq!(l.height, 22.0, "one 12px step plus a 10px line box");
        assert_eq!(l.glyphs[0].baseline_y, 8.0);
        assert_eq!(l.glyphs[2].baseline_y, 20.0, "second baseline is 8 + 12");
        assert_eq!(l.glyphs[2].pen_x, 0.0, "second line restarts at the left");
    }

    #[test]
    fn crlf_is_treated_as_a_single_hard_break() {
        let l = layout_block(
            "ab\r\ncd",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Left,
            None,
            uniform,
        );
        assert_eq!(l.line_count, 2);
        assert_eq!(l.glyphs.len(), 4, "the CR must not become a glyph");
    }

    #[test]
    fn blank_hard_line_still_occupies_a_line() {
        let l = layout_block(
            "a\n\nb",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Left,
            None,
            uniform,
        );
        assert_eq!(l.line_count, 3);
        assert_eq!(l.glyphs.len(), 2);
        assert_eq!(
            l.glyphs[1].baseline_y,
            8.0 + 24.0,
            "b sits on the third baseline"
        );
    }

    #[test]
    fn line_height_scale_stretches_the_baseline_step() {
        let l = layout_block(
            "a\nb",
            10.0,
            stub_metrics(),
            2.0,
            TextAlign::Left,
            None,
            uniform,
        );
        assert_eq!(l.glyphs[1].baseline_y, 8.0 + 24.0, "12px step doubled");
        assert_eq!(l.height, 34.0);
    }

    #[test]
    fn non_positive_line_height_scale_falls_back_to_one() {
        let l = layout_block(
            "a\nb",
            10.0,
            stub_metrics(),
            0.0,
            TextAlign::Left,
            None,
            uniform,
        );
        assert_eq!(l.glyphs[1].baseline_y, 20.0);
    }

    #[test]
    fn centered_and_right_aligned_lines_shift_by_the_width_difference() {
        // "abcd" (40px) and "a" (10px): the short line shifts.
        let centered = layout_block(
            "abcd\na",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Center,
            None,
            uniform,
        );
        assert_eq!(centered.glyphs[0].pen_x, 0.0, "widest line is not shifted");
        assert_eq!(centered.glyphs[4].pen_x, 15.0, "(40 - 10) / 2");

        let right = layout_block(
            "abcd\na",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Right,
            None,
            uniform,
        );
        assert_eq!(right.glyphs[4].pen_x, 30.0, "40 - 10");
    }

    #[test]
    fn word_wrap_feeds_through_to_placement() {
        let l = layout_block(
            "abc def",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Left,
            Some(45.0),
            uniform,
        );
        assert_eq!(l.line_count, 2);
        assert_eq!(l.width, 30.0, "block narrows to the wrapped line width");
        assert_eq!(l.glyphs[3].c, 'd');
        assert_eq!(l.glyphs[3].pen_x, 0.0, "wrapped line restarts at the left");
        assert_eq!(l.glyphs[3].baseline_y, 20.0);
    }

    #[test]
    fn advances_come_from_the_supplied_metric_not_a_constant() {
        // A proportional stub: 'i' is narrow, 'W' is wide.
        let l = layout_block(
            "iWi",
            10.0,
            stub_metrics(),
            1.0,
            TextAlign::Left,
            None,
            |c| if c == 'W' { 20.0 } else { 4.0 },
        );
        let pens: Vec<f32> = l.glyphs.iter().map(|g| g.pen_x).collect();
        assert_eq!(pens, vec![0.0, 4.0, 24.0]);
        assert_eq!(l.width, 28.0);
    }

    // ── anchoring ────────────────────────────────────────────────────────

    #[test]
    fn centre_anchor_centres_the_block() {
        let (x, y) = anchor_top_left(200, 100, 40.0, 20.0, 0.5, 0.5);
        assert_eq!(x, 80.0, "(200 / 2) - (40 / 2)");
        assert_eq!(y, 40.0, "(100 / 2) - (20 / 2)");
    }

    #[test]
    fn anchor_positions_are_clamped_to_the_frame() {
        let (x, y) = anchor_top_left(200, 100, 40.0, 20.0, -3.0, 9.0);
        assert_eq!(x, -20.0, "clamped to 0.0 * 200 - 20");
        assert_eq!(y, 90.0, "clamped to 1.0 * 100 - 10");
    }
}
