//! ProRes frame container and frame header parsing (RDD 36 §6.2–6.4).

use thiserror::Error;

use super::quant::{DEFAULT_CHROMA_QUANT_MATRIX, DEFAULT_LUMA_QUANT_MATRIX};

/// Errors emitted by the ProRes frame parser.
#[derive(Debug, Error)]
pub enum FrameError {
    /// Buffer ended before the parser expected.
    #[error("truncated ProRes data: needed {needed} bytes at {context}, had {available}")]
    Truncated {
        /// Where the parser was looking when it ran out of bytes.
        context: &'static str,
        /// Bytes the parser asked for at that point.
        needed: usize,
        /// Bytes that remained in the buffer.
        available: usize,
    },

    /// Frame container didn't carry the 'icpf' four-character tag.
    #[error("bad frame container tag: expected 'icpf', got {0:?}")]
    BadContainerTag([u8; 4]),

    /// Frame header version byte wasn't 0 (the only spec-defined value).
    #[error("unsupported ProRes bitstream version: {0}")]
    BadVersion(u8),

    /// Encoder identifier was not one of the known 'apco' / 'apcs' /
    /// 'apcn' / 'apch' / 'ap4h' / 'ap4x' FourCCs.
    #[error("unknown ProRes profile identifier: {0:?}")]
    UnknownProfile([u8; 4]),

    /// Frame header declared a chroma_format value the spec doesn't define.
    #[error("invalid ProRes chroma_format code: {0}")]
    BadChromaFormat(u8),

    /// Frame header declared an interlace_mode value the spec doesn't define.
    #[error("invalid ProRes interlace_mode code: {0}")]
    BadInterlaceMode(u8),
}

/// Outer ProRes frame container: 4-byte size + 'icpf' + frame payload.
///
/// The size field counts itself, so the payload length is `size - 4`.
#[derive(Debug, Clone)]
pub struct FrameContainer<'a> {
    /// Total container length in bytes, including the 4-byte size field.
    pub total_size: u32,
    /// Bytes covered by the frame payload (i.e. everything after the
    /// `'icpf'` tag, up to the end of the container).
    pub payload: &'a [u8],
}

impl<'a> FrameContainer<'a> {
    /// Parse a frame container from the start of `buf`. Returns the
    /// container view plus the trailing bytes that follow it.
    pub fn parse(buf: &'a [u8]) -> Result<(Self, &'a [u8]), FrameError> {
        if buf.len() < 8 {
            return Err(FrameError::Truncated {
                context: "frame container header",
                needed: 8,
                available: buf.len(),
            });
        }
        let total_size = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let tag = [buf[4], buf[5], buf[6], buf[7]];
        if &tag != b"icpf" {
            return Err(FrameError::BadContainerTag(tag));
        }
        if (total_size as usize) > buf.len() {
            return Err(FrameError::Truncated {
                context: "frame container payload",
                needed: total_size as usize,
                available: buf.len(),
            });
        }
        let payload = &buf[8..total_size as usize];
        let rest = &buf[total_size as usize..];
        Ok((
            Self {
                total_size,
                payload,
            },
            rest,
        ))
    }
}

/// ProRes 422 profile — distinguishes the five 4:2:2 quality levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProResProfile {
    /// `apco` — 422 Proxy.
    Proxy,
    /// `apcs` — 422 LT.
    Lt,
    /// `apcn` — 422 Standard.
    Standard,
    /// `apch` — 422 HQ.
    Hq,
    /// `ap4h` — 4444.
    P4444,
    /// `ap4x` — 4444 XQ.
    P4444Xq,
}

impl ProResProfile {
    /// Try to recognize one of the known FourCCs.
    pub fn from_fourcc(fourcc: &[u8; 4]) -> Result<Self, FrameError> {
        Ok(match &fourcc[..] {
            b"apco" => Self::Proxy,
            b"apcs" => Self::Lt,
            b"apcn" => Self::Standard,
            b"apch" => Self::Hq,
            b"ap4h" => Self::P4444,
            b"ap4x" => Self::P4444Xq,
            _ => return Err(FrameError::UnknownProfile(*fourcc)),
        })
    }

    /// Human-readable name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Proxy => "ProRes 422 Proxy",
            Self::Lt => "ProRes 422 LT",
            Self::Standard => "ProRes 422 Standard",
            Self::Hq => "ProRes 422 HQ",
            Self::P4444 => "ProRes 4444",
            Self::P4444Xq => "ProRes 4444 XQ",
        }
    }

    /// True for 4:4:4 profiles (which carry an alpha channel option).
    #[must_use]
    pub fn is_4444(self) -> bool {
        matches!(self, Self::P4444 | Self::P4444Xq)
    }
}

/// Chroma subsampling format (RDD 36 §6.4 `chroma_format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaFormat {
    /// 4:2:2 — co-sited Cb/Cr at half horizontal resolution.
    Yuv422,
    /// 4:4:4 — full-resolution Cb/Cr.
    Yuv444,
}

impl ChromaFormat {
    fn from_code(code: u8) -> Result<Self, FrameError> {
        Ok(match code {
            2 => Self::Yuv422,
            3 => Self::Yuv444,
            other => return Err(FrameError::BadChromaFormat(other)),
        })
    }
}

/// Interlace mode (RDD 36 §6.4 `interlace_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterlaceMode {
    /// Progressive (full frame in one picture).
    Progressive,
    /// Top field first, two pictures per frame.
    TopFieldFirst,
    /// Bottom field first, two pictures per frame.
    BottomFieldFirst,
}

impl InterlaceMode {
    fn from_code(code: u8) -> Result<Self, FrameError> {
        Ok(match code {
            0 => Self::Progressive,
            1 => Self::TopFieldFirst,
            2 => Self::BottomFieldFirst,
            other => return Err(FrameError::BadInterlaceMode(other)),
        })
    }

    /// Number of pictures per frame: 1 for progressive, 2 for interlaced.
    #[must_use]
    pub fn pictures_per_frame(self) -> usize {
        if matches!(self, Self::Progressive) {
            1
        } else {
            2
        }
    }
}

/// Parsed ProRes frame header.
///
/// The header sits at the very start of the frame container's payload
/// and carries everything the decoder needs to know about the frame
/// before parsing pictures and slices.
#[derive(Debug, Clone)]
pub struct FrameHeader {
    /// Header size in bytes (including the 2-byte size field itself
    /// and any custom quant matrices).
    pub header_size: u16,
    /// Bitstream version. Always 0 in spec-conformant streams.
    pub version: u8,
    /// Profile (and quality level).
    pub profile: ProResProfile,
    /// Horizontal picture size in luma samples.
    pub width: u16,
    /// Vertical picture size in luma samples (or per-field for interlaced).
    pub height: u16,
    /// Chroma subsampling.
    pub chroma_format: ChromaFormat,
    /// Interlace mode.
    pub interlace_mode: InterlaceMode,
    /// 4-bit aspect_ratio_information code (RDD 36 Table 5).
    pub aspect_ratio_code: u8,
    /// 4-bit frame_rate_code (RDD 36 Table 6).
    pub frame_rate_code: u8,
    /// `color_primaries` code (ITU-T H.273).
    pub color_primaries: u8,
    /// `transfer_characteristic` code (ITU-T H.273).
    pub transfer_characteristic: u8,
    /// `matrix_coefficients` code (ITU-T H.273).
    pub matrix_coefficients: u8,
    /// Source pixel format hint (4 high bits of byte 14).
    pub source_pixel_format: u8,
    /// Alpha channel type (0 = none, 1 = 8-bit, 2 = 16-bit). Only
    /// meaningful for 4444 profiles.
    pub alpha_channel_type: u8,
    /// Effective luma quantization matrix (custom if signaled, default
    /// otherwise).
    pub luma_quant_matrix: [u8; 64],
    /// Effective chroma quantization matrix (custom if signaled,
    /// default otherwise).
    pub chroma_quant_matrix: [u8; 64],
}

impl FrameHeader {
    /// Pictures per frame implied by the interlace mode.
    #[must_use]
    pub fn pictures_per_frame(&self) -> usize {
        self.interlace_mode.pictures_per_frame()
    }
}

/// Parse the frame header at the start of a frame container's payload.
///
/// Returns the parsed header plus the slice of bytes immediately
/// following it (the first byte of the picture header).
pub fn parse_frame_header(payload: &[u8]) -> Result<(FrameHeader, &[u8]), FrameError> {
    // Minimum fixed header: 2-byte size + 18 bytes of fixed fields.
    if payload.len() < 20 {
        return Err(FrameError::Truncated {
            context: "frame header",
            needed: 20,
            available: payload.len(),
        });
    }

    let header_size = u16::from_be_bytes([payload[0], payload[1]]);
    if (header_size as usize) > payload.len() {
        return Err(FrameError::Truncated {
            context: "frame header (declared header_size)",
            needed: header_size as usize,
            available: payload.len(),
        });
    }

    // byte 2: bits 7..4 = bs_version (reserved zero), bits 3..0 = reserved.
    let version = payload[2] >> 4;
    if version != 0 {
        return Err(FrameError::BadVersion(version));
    }

    // bytes 3..7: encoder_identifier
    let mut fourcc = [0u8; 4];
    fourcc.copy_from_slice(&payload[3..7]);
    let profile = ProResProfile::from_fourcc(&fourcc)?;

    let width = u16::from_be_bytes([payload[7], payload[8]]);
    let height = u16::from_be_bytes([payload[9], payload[10]]);

    let chroma_byte = payload[11];
    let chroma_format = ChromaFormat::from_code((chroma_byte >> 6) & 0x3)?;
    let interlace_mode = InterlaceMode::from_code((chroma_byte >> 2) & 0x3)?;

    let ar_fr_byte = payload[12];
    let aspect_ratio_code = ar_fr_byte >> 4;
    let frame_rate_code = ar_fr_byte & 0x0F;

    let color_primaries = payload[13];
    let transfer_characteristic = payload[14];
    let matrix_coefficients = payload[15];

    let src_alpha_byte = payload[16];
    let source_pixel_format = src_alpha_byte >> 4;
    let alpha_channel_type = src_alpha_byte & 0x0F;

    // byte 17: reserved
    // byte 18: bit 7 = load_luma_quant, bit 6 = load_chroma_quant, rest reserved.
    let quant_flags = payload[18];
    let load_luma = quant_flags & 0x80 != 0;
    let load_chroma = quant_flags & 0x40 != 0;

    // byte 19 is reserved; matrices (if any) follow at byte 20.
    let mut cursor = 20usize;

    let luma_quant_matrix = if load_luma {
        if cursor + 64 > header_size as usize {
            return Err(FrameError::Truncated {
                context: "frame header luma quant matrix",
                needed: cursor + 64,
                available: header_size as usize,
            });
        }
        let mut m = [0u8; 64];
        m.copy_from_slice(&payload[cursor..cursor + 64]);
        cursor += 64;
        m
    } else {
        DEFAULT_LUMA_QUANT_MATRIX
    };

    let chroma_quant_matrix = if load_chroma {
        if cursor + 64 > header_size as usize {
            return Err(FrameError::Truncated {
                context: "frame header chroma quant matrix",
                needed: cursor + 64,
                available: header_size as usize,
            });
        }
        let mut m = [0u8; 64];
        m.copy_from_slice(&payload[cursor..cursor + 64]);
        m
    } else if load_luma {
        // RDD 36: if only luma is signaled, chroma reuses the luma matrix.
        luma_quant_matrix
    } else {
        DEFAULT_CHROMA_QUANT_MATRIX
    };

    let header = FrameHeader {
        header_size,
        version,
        profile,
        width,
        height,
        chroma_format,
        interlace_mode,
        aspect_ratio_code,
        frame_rate_code,
        color_primaries,
        transfer_characteristic,
        matrix_coefficients,
        source_pixel_format,
        alpha_channel_type,
        luma_quant_matrix,
        chroma_quant_matrix,
    };
    Ok((header, &payload[header_size as usize..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid frame header (20 bytes, no custom matrices)
    /// describing a 1920×1080 progressive ProRes Standard frame.
    fn minimal_frame_header() -> Vec<u8> {
        let mut h = Vec::with_capacity(20);
        h.extend_from_slice(&20u16.to_be_bytes()); // header_size = 20
        h.push(0x00); // version=0
        h.extend_from_slice(b"apcn"); // ProRes 422 Standard
        h.extend_from_slice(&1920u16.to_be_bytes());
        h.extend_from_slice(&1080u16.to_be_bytes());
        h.push(0x80); // chroma_format=2 (422) << 6
        h.push(0x00); // aspect_ratio=0, frame_rate=0
        h.push(1); // color_primaries
        h.push(1); // transfer
        h.push(1); // matrix
        h.push(0x00); // source_pixel_format=0, alpha_channel_type=0
        h.push(0x00); // reserved
        h.push(0x00); // no custom quant matrices
        h.push(0x00); // reserved
        assert_eq!(h.len(), 20);
        h
    }

    fn wrap_in_container(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + payload.len());
        let total = 8 + payload.len() as u32;
        out.extend_from_slice(&total.to_be_bytes());
        out.extend_from_slice(b"icpf");
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn container_parses_and_splits_payload() {
        let payload = b"AAAAA";
        let buf = wrap_in_container(payload);
        let (c, rest) = FrameContainer::parse(&buf).unwrap();
        assert_eq!(c.total_size, (8 + payload.len()) as u32);
        assert_eq!(c.payload, payload);
        assert!(rest.is_empty());
    }

    #[test]
    fn container_with_trailing_bytes() {
        let payload = b"BBBB";
        let mut buf = wrap_in_container(payload);
        buf.extend_from_slice(b"trailing");
        let (_, rest) = FrameContainer::parse(&buf).unwrap();
        assert_eq!(rest, b"trailing");
    }

    #[test]
    fn container_bad_tag_errors() {
        let mut buf = wrap_in_container(b"x");
        buf[4] = b'X';
        let err = FrameContainer::parse(&buf).unwrap_err();
        assert!(matches!(err, FrameError::BadContainerTag(_)));
    }

    #[test]
    fn container_short_buffer_errors() {
        assert!(FrameContainer::parse(b"\x00\x00\x00").is_err());
    }

    #[test]
    fn frame_header_parses_minimal() {
        let payload = minimal_frame_header();
        let (h, rest) = parse_frame_header(&payload).unwrap();
        assert_eq!(h.header_size, 20);
        assert_eq!(h.profile, ProResProfile::Standard);
        assert_eq!(h.width, 1920);
        assert_eq!(h.height, 1080);
        assert_eq!(h.chroma_format, ChromaFormat::Yuv422);
        assert_eq!(h.interlace_mode, InterlaceMode::Progressive);
        assert_eq!(h.color_primaries, 1);
        // Without explicit signaling, default matrices are used.
        assert_eq!(h.luma_quant_matrix, DEFAULT_LUMA_QUANT_MATRIX);
        assert_eq!(h.chroma_quant_matrix, DEFAULT_CHROMA_QUANT_MATRIX);
        assert!(rest.is_empty());
    }

    #[test]
    fn frame_header_with_custom_luma_matrix() {
        let mut hdr = minimal_frame_header();
        // Set load_luma = 1; bump header_size to 20 + 64.
        hdr[0] = 0;
        hdr[1] = 84; // 20 + 64 = 84
        hdr[18] = 0x80; // load_luma_quant
        // Append a recognisable custom matrix (0..64).
        let custom: [u8; 64] = std::array::from_fn(|i| i as u8 + 1);
        hdr.extend_from_slice(&custom);
        assert_eq!(hdr.len(), 84);

        let (h, rest) = parse_frame_header(&hdr).unwrap();
        assert_eq!(h.header_size, 84);
        assert_eq!(h.luma_quant_matrix, custom);
        // No chroma matrix signaled → reuses luma per RDD 36.
        assert_eq!(h.chroma_quant_matrix, custom);
        assert!(rest.is_empty());
    }

    #[test]
    fn frame_header_with_both_custom_matrices() {
        let mut hdr = minimal_frame_header();
        hdr[0] = 0;
        hdr[1] = 148; // 20 + 128 = 148
        hdr[18] = 0xC0; // load_luma + load_chroma
        let luma: [u8; 64] = std::array::from_fn(|i| (i + 10) as u8);
        let chroma: [u8; 64] = std::array::from_fn(|i| (i + 100) as u8);
        hdr.extend_from_slice(&luma);
        hdr.extend_from_slice(&chroma);
        assert_eq!(hdr.len(), 148);

        let (h, rest) = parse_frame_header(&hdr).unwrap();
        assert_eq!(h.luma_quant_matrix, luma);
        assert_eq!(h.chroma_quant_matrix, chroma);
        assert!(rest.is_empty());
    }

    #[test]
    fn frame_header_recognises_every_profile() {
        for (fourcc, expected) in [
            (b"apco", ProResProfile::Proxy),
            (b"apcs", ProResProfile::Lt),
            (b"apcn", ProResProfile::Standard),
            (b"apch", ProResProfile::Hq),
            (b"ap4h", ProResProfile::P4444),
            (b"ap4x", ProResProfile::P4444Xq),
        ] {
            let mut hdr = minimal_frame_header();
            hdr[3..7].copy_from_slice(fourcc);
            if matches!(expected, ProResProfile::P4444 | ProResProfile::P4444Xq) {
                // 4444 needs chroma_format = 3 (4:4:4).
                hdr[11] = 0xC0;
            }
            let (h, _) = parse_frame_header(&hdr).unwrap();
            assert_eq!(h.profile, expected);
        }
    }

    #[test]
    fn frame_header_rejects_unknown_profile() {
        let mut hdr = minimal_frame_header();
        hdr[3..7].copy_from_slice(b"xxxx");
        let err = parse_frame_header(&hdr).unwrap_err();
        assert!(matches!(err, FrameError::UnknownProfile(_)));
    }

    #[test]
    fn frame_header_rejects_nonzero_version() {
        let mut hdr = minimal_frame_header();
        hdr[2] = 0x10; // version=1
        let err = parse_frame_header(&hdr).unwrap_err();
        assert!(matches!(err, FrameError::BadVersion(1)));
    }

    #[test]
    fn interlace_mode_picture_count() {
        assert_eq!(InterlaceMode::Progressive.pictures_per_frame(), 1);
        assert_eq!(InterlaceMode::TopFieldFirst.pictures_per_frame(), 2);
        assert_eq!(InterlaceMode::BottomFieldFirst.pictures_per_frame(), 2);
    }

    #[test]
    fn profile_is_4444_classification() {
        assert!(ProResProfile::P4444.is_4444());
        assert!(ProResProfile::P4444Xq.is_4444());
        assert!(!ProResProfile::Standard.is_4444());
        assert!(!ProResProfile::Hq.is_4444());
    }
}
