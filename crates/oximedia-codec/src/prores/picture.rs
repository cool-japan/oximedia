//! ProRes picture and slice header parsing (RDD 36 §6.5).
//!
//! After the frame header, the bitstream contains one **picture** per
//! field (so progressive frames have one picture, interlaced frames have
//! two). Each picture is a header followed by a sequence of **slices**;
//! a slice covers a horizontal strip of macroblocks (default 8 MBs wide,
//! though the encoder can widen them).
//!
//! Picture layout:
//!
//! ```text
//!  ┌────────────────────────┐
//!  │ picture_header_size    │  1 byte
//!  │ picture_size           │  4 bytes BE (incl. header + data)
//!  │ slice_count            │  2 bytes BE
//!  │ log2_slice_mb_width    │  1 byte (4 high bits) + reserved
//!  ├────────────────────────┤
//!  │ slice_offset[0..n]     │  optional table of n × 2-byte offsets
//!  │                        │  (only if picture_header_size > 8)
//!  ├────────────────────────┤
//!  │ slice 0                │
//!  │ slice 1                │
//!  │   …                    │
//!  └────────────────────────┘
//! ```
//!
//! Each slice starts with a 6 (8 for 4444 + alpha) byte header:
//!
//! ```text
//!  slice_header_size   : 4 bits (high nibble of byte 0)
//!  ─────────────────── : 4 bits reserved
//!  quant_scale         : 8 bits (1..=224)
//!  luma_data_size      : 16 bits BE
//!  cb_data_size        : 16 bits BE
//!  cr_data_size        : 16 bits BE
//!  [alpha_data_size]   : 16 bits BE  ← only if alpha is present
//! ```

use super::frame::FrameError;

/// Parsed picture header.
#[derive(Debug, Clone)]
pub struct PictureHeader {
    /// Length in bytes of this header (including itself).
    pub header_size: u8,
    /// Total picture size in bytes (header + all slices).
    pub picture_size: u32,
    /// Number of slices in this picture.
    pub slice_count: u16,
    /// Log2 of slice macroblock-width. Default 3 ⇒ 8-MB-wide slices.
    pub log2_slice_mb_width: u8,
}

/// Parsed slice header.
#[derive(Debug, Clone, Copy)]
pub struct SliceHeader {
    /// Header length in bytes (typically 6, or 8 for 4444 + alpha).
    pub header_size: u8,
    /// Quantization scale (1–224) applied to this slice's coefficients.
    pub quant_scale: u8,
    /// Compressed size of the luma plane data for this slice.
    pub luma_data_size: u16,
    /// Compressed size of the Cb plane data.
    pub cb_data_size: u16,
    /// Compressed size of the Cr plane data.
    pub cr_data_size: u16,
    /// Compressed size of the alpha plane data, if alpha is enabled.
    pub alpha_data_size: Option<u16>,
}

impl SliceHeader {
    /// Sum of all compressed plane sizes — i.e. the byte length of the
    /// slice's *data* portion (excluding the header).
    #[must_use]
    pub fn data_size(&self) -> usize {
        usize::from(self.luma_data_size)
            + usize::from(self.cb_data_size)
            + usize::from(self.cr_data_size)
            + self.alpha_data_size.map_or(0, usize::from)
    }
}

/// Parse the picture header at the start of `payload`. Returns the
/// header plus the bytes that follow it (the first slice header or the
/// optional slice-offset table).
pub fn parse_picture_header(payload: &[u8]) -> Result<(PictureHeader, &[u8]), FrameError> {
    if payload.len() < 8 {
        return Err(FrameError::Truncated {
            context: "picture header",
            needed: 8,
            available: payload.len(),
        });
    }
    let header_size = payload[0];
    if (header_size as usize) < 8 {
        return Err(FrameError::Truncated {
            context: "picture header (declared header_size < 8)",
            needed: 8,
            available: header_size as usize,
        });
    }
    let picture_size = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
    let slice_count = u16::from_be_bytes([payload[5], payload[6]]);
    let log2_slice_mb_width = payload[7] >> 4;

    Ok((
        PictureHeader {
            header_size,
            picture_size,
            slice_count,
            log2_slice_mb_width,
        },
        &payload[header_size as usize..],
    ))
}

/// Parse a slice header at the start of `payload`.
///
/// `has_alpha` should be `true` when the surrounding frame's
/// `alpha_channel_type` is non-zero (4444 + alpha streams).
pub fn parse_slice_header(
    payload: &[u8],
    has_alpha: bool,
) -> Result<(SliceHeader, &[u8]), FrameError> {
    let min_size = if has_alpha { 8 } else { 6 };
    if payload.len() < min_size {
        return Err(FrameError::Truncated {
            context: "slice header",
            needed: min_size,
            available: payload.len(),
        });
    }
    // High nibble of byte 0 is slice_header_size (in bytes / 1).
    let header_size = payload[0] >> 4;
    let quant_scale = payload[1];
    let luma_data_size = u16::from_be_bytes([payload[2], payload[3]]);
    let cb_data_size = u16::from_be_bytes([payload[4], payload[5]]);
    let cr_data_size = u16::from_be_bytes([payload[6], payload[7]]);
    let (alpha_data_size, hdr_bytes) = if has_alpha {
        if payload.len() < 10 {
            return Err(FrameError::Truncated {
                context: "slice header alpha size",
                needed: 10,
                available: payload.len(),
            });
        }
        (
            Some(u16::from_be_bytes([payload[8], payload[9]])),
            10usize,
        )
    } else {
        (None, 8usize)
    };

    Ok((
        SliceHeader {
            header_size,
            quant_scale,
            luma_data_size,
            cb_data_size,
            cr_data_size,
            alpha_data_size,
        },
        &payload[hdr_bytes..],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture_header_bytes(slice_count: u16, log2_mb_w: u8) -> Vec<u8> {
        let mut h = Vec::with_capacity(8);
        h.push(8); // header_size = 8 (no offset table)
        h.extend_from_slice(&1000u32.to_be_bytes()); // picture_size
        h.extend_from_slice(&slice_count.to_be_bytes());
        h.push(log2_mb_w << 4);
        h
    }

    fn slice_header_bytes(
        quant: u8,
        luma: u16,
        cb: u16,
        cr: u16,
        alpha: Option<u16>,
    ) -> Vec<u8> {
        let hdr_size_nibble = if alpha.is_some() { 10 } else { 8 };
        let mut h = vec![(hdr_size_nibble as u8) << 4, quant];
        h.extend_from_slice(&luma.to_be_bytes());
        h.extend_from_slice(&cb.to_be_bytes());
        h.extend_from_slice(&cr.to_be_bytes());
        if let Some(a) = alpha {
            h.extend_from_slice(&a.to_be_bytes());
        }
        h
    }

    #[test]
    fn picture_header_round_trip() {
        let bytes = picture_header_bytes(60, 3);
        let (h, rest) = parse_picture_header(&bytes).unwrap();
        assert_eq!(h.header_size, 8);
        assert_eq!(h.picture_size, 1000);
        assert_eq!(h.slice_count, 60);
        assert_eq!(h.log2_slice_mb_width, 3);
        assert!(rest.is_empty());
    }

    #[test]
    fn picture_header_with_trailing_bytes_returns_remainder() {
        let mut bytes = picture_header_bytes(8, 3);
        bytes.extend_from_slice(b"data");
        let (_, rest) = parse_picture_header(&bytes).unwrap();
        assert_eq!(rest, b"data");
    }

    #[test]
    fn slice_header_no_alpha() {
        let mut buf = slice_header_bytes(50, 200, 100, 100, None);
        buf.extend_from_slice(b"payload");
        let (s, rest) = parse_slice_header(&buf, false).unwrap();
        assert_eq!(s.header_size, 8);
        assert_eq!(s.quant_scale, 50);
        assert_eq!(s.luma_data_size, 200);
        assert_eq!(s.cb_data_size, 100);
        assert_eq!(s.cr_data_size, 100);
        assert_eq!(s.alpha_data_size, None);
        assert_eq!(s.data_size(), 400);
        assert_eq!(rest, b"payload");
    }

    #[test]
    fn slice_header_with_alpha() {
        let mut buf = slice_header_bytes(80, 300, 150, 150, Some(75));
        buf.extend_from_slice(b"xyz");
        let (s, rest) = parse_slice_header(&buf, true).unwrap();
        assert_eq!(s.header_size, 10);
        assert_eq!(s.alpha_data_size, Some(75));
        assert_eq!(s.data_size(), 300 + 150 + 150 + 75);
        assert_eq!(rest, b"xyz");
    }

    #[test]
    fn slice_header_truncated_errors() {
        assert!(parse_slice_header(&[0u8; 4], false).is_err());
    }

    #[test]
    fn picture_header_too_short_errors() {
        assert!(parse_picture_header(&[0u8; 3]).is_err());
    }
}
