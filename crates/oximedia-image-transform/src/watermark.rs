// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Watermark overlay for the image transform pipeline.
//!
//! Supports two watermark types:
//! - **Text watermarks** — rendered with a built-in 5x7 bitmap font and configurable
//!   color, opacity, and position.
//! - **Image watermarks** — overlay a [`PixelBuffer`] stamp with configurable opacity,
//!   position, and scaling.
//!
//! Watermarks are composited using alpha-aware blending, respecting both the
//! watermark opacity setting and the per-pixel alpha of image stamps.

use crate::processor::PixelBuffer;

// ============================================================================
// WatermarkPosition
// ============================================================================

/// Anchor position for a watermark overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatermarkPosition {
    /// Top-left corner.
    TopLeft,
    /// Top-center.
    TopCenter,
    /// Top-right corner.
    TopRight,
    /// Center-left.
    CenterLeft,
    /// Dead center.
    Center,
    /// Center-right.
    CenterRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-center.
    BottomCenter,
    /// Bottom-right corner.
    BottomRight,
}

impl Default for WatermarkPosition {
    fn default() -> Self {
        Self::BottomRight
    }
}

impl WatermarkPosition {
    /// Parse a position string (case-insensitive, accepts hyphens/underscores).
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "topleft" => Some(Self::TopLeft),
            "topcenter" | "topcentre" => Some(Self::TopCenter),
            "topright" => Some(Self::TopRight),
            "centerleft" | "centreleft" => Some(Self::CenterLeft),
            "center" | "centre" => Some(Self::Center),
            "centerright" | "centreright" => Some(Self::CenterRight),
            "bottomleft" => Some(Self::BottomLeft),
            "bottomcenter" | "bottomcentre" => Some(Self::BottomCenter),
            "bottomright" => Some(Self::BottomRight),
            _ => None,
        }
    }

    /// Compute (x, y) origin for a watermark of size (wm_w, wm_h) inside a
    /// canvas of size (canvas_w, canvas_h) with the given margin.
    fn origin(
        &self,
        canvas_w: u32,
        canvas_h: u32,
        wm_w: u32,
        wm_h: u32,
        margin: u32,
    ) -> (u32, u32) {
        let max_x = canvas_w.saturating_sub(wm_w + margin);
        let max_y = canvas_h.saturating_sub(wm_h + margin);
        let center_x = canvas_w.saturating_sub(wm_w) / 2;
        let center_y = canvas_h.saturating_sub(wm_h) / 2;

        match self {
            Self::TopLeft => (margin, margin),
            Self::TopCenter => (center_x, margin),
            Self::TopRight => (max_x, margin),
            Self::CenterLeft => (margin, center_y),
            Self::Center => (center_x, center_y),
            Self::CenterRight => (max_x, center_y),
            Self::BottomLeft => (margin, max_y),
            Self::BottomCenter => (center_x, max_y),
            Self::BottomRight => (max_x, max_y),
        }
    }
}

// ============================================================================
// WatermarkConfig
// ============================================================================

/// Configuration for a watermark overlay.
#[derive(Debug, Clone)]
pub struct WatermarkConfig {
    /// Anchor position on the canvas.
    pub position: WatermarkPosition,
    /// Global opacity (0.0 = invisible, 1.0 = fully opaque).
    pub opacity: f64,
    /// Margin from the anchor edge in pixels.
    pub margin: u32,
    /// Optional scale factor for image watermarks (1.0 = natural size).
    pub scale: f64,
}

impl Default for WatermarkConfig {
    fn default() -> Self {
        Self {
            position: WatermarkPosition::BottomRight,
            opacity: 0.5,
            margin: 10,
            scale: 1.0,
        }
    }
}

// ============================================================================
// Text watermark rendering
// ============================================================================

/// Render a text watermark onto a pixel buffer.
///
/// Uses a built-in 5x7 bitmap font. Text colour and opacity are configurable.
/// The watermark is composited using alpha blending.
pub fn apply_text_watermark(
    buffer: &mut PixelBuffer,
    text: &str,
    color: [u8; 4],
    config: &WatermarkConfig,
) {
    if text.is_empty() || buffer.width == 0 || buffer.height == 0 {
        return;
    }

    let glyph_w: u32 = 5;
    let glyph_h: u32 = 7;
    let spacing: u32 = 1;

    // Compute rendered text dimensions
    let char_count = text.len() as u32;
    let text_w = char_count * glyph_w + char_count.saturating_sub(1) * spacing;
    let text_h = glyph_h;

    if text_w == 0 || text_h == 0 {
        return;
    }

    let (origin_x, origin_y) =
        config
            .position
            .origin(buffer.width, buffer.height, text_w, text_h, config.margin);

    let opacity = config.opacity.clamp(0.0, 1.0);

    for (ci, ch) in text.chars().enumerate() {
        let glyph = get_glyph(ch);
        let gx = origin_x + ci as u32 * (glyph_w + spacing);

        for row in 0..glyph_h {
            let bits = glyph[row as usize];
            for col in 0..glyph_w {
                if (bits >> (4 - col)) & 1 == 1 {
                    let px = gx + col;
                    let py = origin_y + row;
                    blend_pixel(buffer, px, py, color, opacity);
                }
            }
        }
    }
}

/// Render an image watermark (stamp) onto a pixel buffer.
///
/// The stamp is scaled by `config.scale` using nearest-neighbour (to keep it fast
/// for overlay purposes). Alpha from both the stamp and `config.opacity` is
/// respected during compositing.
pub fn apply_image_watermark(
    buffer: &mut PixelBuffer,
    stamp: &PixelBuffer,
    config: &WatermarkConfig,
) {
    if stamp.width == 0 || stamp.height == 0 || buffer.width == 0 || buffer.height == 0 {
        return;
    }

    let scaled_w = (stamp.width as f64 * config.scale).round().max(1.0) as u32;
    let scaled_h = (stamp.height as f64 * config.scale).round().max(1.0) as u32;

    let (origin_x, origin_y) = config.position.origin(
        buffer.width,
        buffer.height,
        scaled_w,
        scaled_h,
        config.margin,
    );

    let opacity = config.opacity.clamp(0.0, 1.0);

    for dy in 0..scaled_h {
        for dx in 0..scaled_w {
            // Nearest-neighbour sample from stamp
            let sx = (dx as f64 / config.scale).min(stamp.width.saturating_sub(1) as f64) as u32;
            let sy = (dy as f64 / config.scale).min(stamp.height.saturating_sub(1) as f64) as u32;

            if let Some(sp) = stamp.get_pixel(sx, sy) {
                let color = if stamp.channels >= 4 {
                    [sp[0], sp[1], sp[2], sp[3]]
                } else if stamp.channels >= 3 {
                    [sp[0], sp[1], sp[2], 255]
                } else {
                    [sp[0], sp[0], sp[0], 255]
                };

                let px = origin_x + dx;
                let py = origin_y + dy;
                blend_pixel(buffer, px, py, color, opacity);
            }
        }
    }
}

// ============================================================================
// Alpha blending helper
// ============================================================================

/// Blend a single pixel onto the buffer with alpha compositing.
fn blend_pixel(buffer: &mut PixelBuffer, x: u32, y: u32, color: [u8; 4], opacity: f64) {
    if x >= buffer.width || y >= buffer.height {
        return;
    }

    let ch = buffer.channels as usize;
    let idx = (y as usize * buffer.width as usize + x as usize) * ch;
    if idx + ch > buffer.data.len() {
        return;
    }

    let src_alpha = (color[3] as f64 / 255.0) * opacity;
    let inv_alpha = 1.0 - src_alpha;
    let color_ch = ch.min(3);

    for c in 0..color_ch {
        let dst = buffer.data[idx + c] as f64;
        let src = color[c] as f64;
        buffer.data[idx + c] = (src * src_alpha + dst * inv_alpha)
            .round()
            .clamp(0.0, 255.0) as u8;
    }

    // Update alpha channel if present
    if ch >= 4 {
        let dst_a = buffer.data[idx + 3] as f64 / 255.0;
        let out_a = src_alpha + dst_a * inv_alpha;
        buffer.data[idx + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}

// ============================================================================
// Built-in 5x7 bitmap font (ASCII printable subset)
// ============================================================================

/// Get the 5x7 bitmap glyph for a character.
///
/// Returns an array of 7 bytes; each byte encodes a row where bits 4..0
/// correspond to columns 0..4 (MSB = leftmost column).
fn get_glyph(ch: char) -> [u8; 7] {
    match ch {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01110, 0b10001, 0b10000, 0b01110, 0b00001, 0b10001, 0b01110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        'a'..='z' => get_glyph(ch.to_ascii_uppercase()),
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        ' ' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100,
        ],
        '!' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '_' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111,
        ],
        ':' => [
            0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '@' => [
            0b01110, 0b10001, 0b10111, 0b10101, 0b10111, 0b10000, 0b01110,
        ],
        '#' => [
            0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        _ => [
            0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111,
        ], // box
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_solid_buffer(width: u32, height: u32, color: [u8; 4]) -> PixelBuffer {
        let mut buf = PixelBuffer::new(width, height, 4);
        for y in 0..height {
            for x in 0..width {
                buf.set_pixel(x, y, &color);
            }
        }
        buf
    }

    // ── WatermarkPosition ──

    #[test]
    fn test_position_parse() {
        assert_eq!(
            WatermarkPosition::from_str_loose("top-left"),
            Some(WatermarkPosition::TopLeft)
        );
        assert_eq!(
            WatermarkPosition::from_str_loose("bottom_right"),
            Some(WatermarkPosition::BottomRight)
        );
        assert_eq!(
            WatermarkPosition::from_str_loose("Center"),
            Some(WatermarkPosition::Center)
        );
        assert_eq!(
            WatermarkPosition::from_str_loose("TOPCENTER"),
            Some(WatermarkPosition::TopCenter)
        );
        assert_eq!(WatermarkPosition::from_str_loose("invalid"), None);
    }

    #[test]
    fn test_position_default() {
        assert_eq!(WatermarkPosition::default(), WatermarkPosition::BottomRight);
    }

    #[test]
    fn test_position_origin_top_left() {
        let (x, y) = WatermarkPosition::TopLeft.origin(100, 100, 20, 10, 5);
        assert_eq!((x, y), (5, 5));
    }

    #[test]
    fn test_position_origin_bottom_right() {
        let (x, y) = WatermarkPosition::BottomRight.origin(100, 100, 20, 10, 5);
        assert_eq!(x, 75); // 100 - 20 - 5
        assert_eq!(y, 85); // 100 - 10 - 5
    }

    #[test]
    fn test_position_origin_center() {
        let (x, y) = WatermarkPosition::Center.origin(100, 100, 20, 10, 5);
        assert_eq!(x, 40); // (100 - 20) / 2
        assert_eq!(y, 45); // (100 - 10) / 2
    }

    // ── Config defaults ──

    #[test]
    fn test_config_defaults() {
        let config = WatermarkConfig::default();
        assert_eq!(config.position, WatermarkPosition::BottomRight);
        assert!((config.opacity - 0.5).abs() < f64::EPSILON);
        assert_eq!(config.margin, 10);
        assert!((config.scale - 1.0).abs() < f64::EPSILON);
    }

    // ── Text watermark ──

    #[test]
    fn test_text_watermark_modifies_pixels() {
        let mut buf = make_solid_buffer(100, 100, [255, 255, 255, 255]);
        let original = buf.data.clone();
        apply_text_watermark(
            &mut buf,
            "TEST",
            [0, 0, 0, 255],
            &WatermarkConfig::default(),
        );
        // At least some pixels should have changed
        assert_ne!(buf.data, original);
    }

    #[test]
    fn test_text_watermark_empty_string_noop() {
        let mut buf = make_solid_buffer(50, 50, [128, 128, 128, 255]);
        let original = buf.data.clone();
        apply_text_watermark(&mut buf, "", [0, 0, 0, 255], &WatermarkConfig::default());
        assert_eq!(buf.data, original);
    }

    #[test]
    fn test_text_watermark_zero_opacity_noop() {
        let mut buf = make_solid_buffer(100, 100, [128, 128, 128, 255]);
        let original = buf.data.clone();
        let config = WatermarkConfig {
            opacity: 0.0,
            ..WatermarkConfig::default()
        };
        apply_text_watermark(&mut buf, "HELLO", [0, 0, 0, 255], &config);
        assert_eq!(buf.data, original);
    }

    #[test]
    fn test_text_watermark_full_opacity() {
        let mut buf = make_solid_buffer(100, 50, [255, 255, 255, 255]);
        let config = WatermarkConfig {
            opacity: 1.0,
            position: WatermarkPosition::TopLeft,
            margin: 0,
            ..WatermarkConfig::default()
        };
        apply_text_watermark(&mut buf, "A", [0, 0, 0, 255], &config);
        // "A" glyph row 0 = 0b01110: columns 1,2,3 are set
        // At (1, 0) the pixel should be black
        let p = buf.get_pixel(1, 0).expect("pixel");
        assert_eq!(p[0], 0);
        assert_eq!(p[1], 0);
        assert_eq!(p[2], 0);
    }

    #[test]
    fn test_text_watermark_small_buffer() {
        // Buffer smaller than watermark margin: should not panic
        let mut buf = make_solid_buffer(5, 5, [128, 128, 128, 255]);
        apply_text_watermark(
            &mut buf,
            "VERY LONG TEXT",
            [0, 0, 0, 255],
            &WatermarkConfig::default(),
        );
    }

    #[test]
    fn test_text_watermark_positions() {
        for pos in [
            WatermarkPosition::TopLeft,
            WatermarkPosition::TopCenter,
            WatermarkPosition::TopRight,
            WatermarkPosition::CenterLeft,
            WatermarkPosition::Center,
            WatermarkPosition::CenterRight,
            WatermarkPosition::BottomLeft,
            WatermarkPosition::BottomCenter,
            WatermarkPosition::BottomRight,
        ] {
            let mut buf = make_solid_buffer(200, 200, [255, 255, 255, 255]);
            let config = WatermarkConfig {
                position: pos,
                opacity: 1.0,
                margin: 5,
                ..WatermarkConfig::default()
            };
            apply_text_watermark(&mut buf, "AB", [0, 0, 0, 255], &config);
            // Just verify it does not panic
        }
    }

    // ── Image watermark ──

    #[test]
    fn test_image_watermark_modifies_pixels() {
        let mut buf = make_solid_buffer(100, 100, [255, 255, 255, 255]);
        let stamp = make_solid_buffer(20, 20, [255, 0, 0, 200]);
        let original = buf.data.clone();
        apply_image_watermark(&mut buf, &stamp, &WatermarkConfig::default());
        assert_ne!(buf.data, original);
    }

    #[test]
    fn test_image_watermark_empty_stamp_noop() {
        let mut buf = make_solid_buffer(100, 100, [255, 255, 255, 255]);
        let stamp = PixelBuffer::new(0, 0, 4);
        let original = buf.data.clone();
        apply_image_watermark(&mut buf, &stamp, &WatermarkConfig::default());
        assert_eq!(buf.data, original);
    }

    #[test]
    fn test_image_watermark_scaled() {
        let mut buf = make_solid_buffer(200, 200, [255, 255, 255, 255]);
        let stamp = make_solid_buffer(10, 10, [0, 0, 0, 255]);
        let config = WatermarkConfig {
            scale: 2.0,
            opacity: 1.0,
            position: WatermarkPosition::TopLeft,
            margin: 0,
        };
        apply_image_watermark(&mut buf, &stamp, &config);
        // Pixel at (0,0) should be black (stamp scaled to 20x20)
        let p = buf.get_pixel(0, 0).expect("pixel");
        assert_eq!(p[0], 0);
    }

    #[test]
    fn test_image_watermark_half_opacity() {
        let mut buf = make_solid_buffer(100, 100, [0, 0, 0, 255]);
        let stamp = make_solid_buffer(10, 10, [200, 200, 200, 255]);
        let config = WatermarkConfig {
            opacity: 0.5,
            position: WatermarkPosition::TopLeft,
            margin: 0,
            ..WatermarkConfig::default()
        };
        apply_image_watermark(&mut buf, &stamp, &config);
        let p = buf.get_pixel(0, 0).expect("pixel");
        // 200 * 0.5 + 0 * 0.5 = 100
        assert!((p[0] as i32 - 100).abs() <= 1);
    }

    // ── Blend pixel ──

    #[test]
    fn test_blend_pixel_full_alpha() {
        let mut buf = make_solid_buffer(10, 10, [100, 100, 100, 255]);
        blend_pixel(&mut buf, 5, 5, [200, 50, 0, 255], 1.0);
        let p = buf.get_pixel(5, 5).expect("pixel");
        assert_eq!(p[0], 200);
        assert_eq!(p[1], 50);
        assert_eq!(p[2], 0);
    }

    #[test]
    fn test_blend_pixel_out_of_bounds() {
        let mut buf = make_solid_buffer(10, 10, [100, 100, 100, 255]);
        let original = buf.data.clone();
        blend_pixel(&mut buf, 20, 20, [200, 50, 0, 255], 1.0);
        assert_eq!(buf.data, original);
    }

    // ── Glyph ──

    #[test]
    fn test_glyph_lowercase_maps_to_uppercase() {
        assert_eq!(get_glyph('a'), get_glyph('A'));
        assert_eq!(get_glyph('z'), get_glyph('Z'));
    }

    #[test]
    fn test_glyph_space_is_blank() {
        let g = get_glyph(' ');
        for row in g {
            assert_eq!(row, 0);
        }
    }

    #[test]
    fn test_glyph_unknown_char_is_box() {
        let g = get_glyph('\u{2603}'); // snowman
                                       // Box glyph: top row is all 1s
        assert_eq!(g[0], 0b11111);
    }

    // ── RGB buffer watermark ──

    #[test]
    fn test_text_watermark_rgb_buffer() {
        let data = vec![255u8; 100 * 100 * 3];
        let mut buf = PixelBuffer::from_rgb(data, 100, 100).expect("valid");
        let config = WatermarkConfig {
            position: WatermarkPosition::TopLeft,
            opacity: 1.0,
            margin: 0,
            ..WatermarkConfig::default()
        };
        apply_text_watermark(&mut buf, "X", [0, 0, 0, 255], &config);
        // Should modify some pixels without panicking
        assert_eq!(buf.channels, 3);
    }
}
