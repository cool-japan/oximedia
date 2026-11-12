// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! A minimal 5x7 bitmap font for scope labels.
//!
//! The upstream native `oximedia-scopes` font (`render.rs::FONT_5X7`) only
//! encodes the ten digits plus `.`/`%`/space, so any alphabetic label it tried
//! to draw was silently dropped. This port fixes that bug by covering **every
//! glyph the scope actually draws**: digits `0-9`, space, and the letters
//! `B C G I M Q R Y` used by the waveform IRE labels, the RGB / YCbCr parade
//! pane captions, and the vectorscope target / I-Q axis labels.
//!
//! Glyphs are stored **row-major**: seven `u8` rows, one per scanline, with the
//! low five bits describing the row (bit 4 = leftmost column). Unknown
//! characters return [`None`] and are rendered as blank cells so a stray label
//! never panics or produces garbage.

/// Pixel width of one glyph cell.
pub const GLYPH_WIDTH: u32 = 5;
/// Pixel height of one glyph cell.
pub const GLYPH_HEIGHT: u32 = 7;
/// Horizontal advance between glyph origins (5 pixels + 1 spacing column).
pub const GLYPH_ADVANCE: u32 = 6;

/// Returns the 7-row bitmap for `c`, or [`None`] for an unsupported character.
///
/// Each returned row's low [`GLYPH_WIDTH`] bits are the pixel pattern, most
/// significant of those bits being the leftmost column.
#[must_use]
pub fn glyph(c: char) -> Option<[u8; 7]> {
    let rows = match c {
        ' ' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        _ => return None,
    };
    Some(rows)
}

/// Pixel width a string will occupy when drawn with [`GLYPH_ADVANCE`] spacing.
#[must_use]
pub fn text_width(text: &str) -> u32 {
    let n = text.chars().count() as u32;
    if n == 0 {
        0
    } else {
        n * GLYPH_ADVANCE - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_label_letter_is_present() {
        for c in "0123456789 BCGIMQRY".chars() {
            assert!(glyph(c).is_some(), "missing glyph for {c:?}");
        }
    }

    #[test]
    fn unknown_glyphs_are_none() {
        assert!(glyph('Z').is_none());
        assert!(glyph('%').is_none());
    }

    #[test]
    fn glyph_rows_fit_five_bits() {
        for c in "0123456789BCGIMQRY".chars() {
            let g = glyph(c).expect("present");
            for row in g {
                assert!(row < 0x20, "row {row:#x} of {c:?} exceeds 5 bits");
            }
        }
    }

    #[test]
    fn text_width_matches_advance() {
        assert_eq!(text_width(""), 0);
        assert_eq!(text_width("R"), GLYPH_WIDTH);
        assert_eq!(text_width("CB"), GLYPH_ADVANCE + GLYPH_WIDTH);
    }
}
