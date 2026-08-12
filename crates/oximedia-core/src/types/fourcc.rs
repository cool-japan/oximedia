//! Typed FourCC (Four Character Code) constants for codec and container identification.
//!
//! This module provides the [`FourCc`] value type and a comprehensive set of
//! typed constants for common multimedia FourCC codes used in ISOBMFF/MP4,
//! RIFF/AVI, and similar container formats.
//!
//! # Relationship to `crate::fourcc::FourCC`
//!
//! The legacy [`FourCC`](crate::fourcc::FourCC) type in `crate::fourcc` provides a
//! registry-oriented API (`FourCCRegistry`, `is_video()`, `is_audio()`, `parse()`).
//! This `FourCc` type is a simpler, `repr(transparent)` value type designed for
//! compile-time constant usage and `std::str::FromStr` integration.

use std::hash::{Hash, Hasher};

/// A four-byte multimedia identifier (Four Character Code).
///
/// `FourCc` is a `repr(transparent)` wrapper over `[u8; 4]`. It is `Copy`,
/// cheaply hashable, and can be constructed as a `const`.
///
/// # Examples
///
/// ```
/// use oximedia_core::types::fourcc::{FourCc, AV01, AVC1, MP4A};
///
/// // Parse from a 4-byte string
/// let av1: FourCc = "av01".parse().expect("valid fourcc");
/// assert_eq!(av1, AV01); // "av01" parses to the AV01 constant
///
/// // Named constants
/// assert_eq!(AVC1.as_bytes(), b"avc1");
/// assert_eq!(MP4A.as_bytes(), b"mp4a");
/// ```
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct FourCc(pub [u8; 4]);

impl FourCc {
    /// Creates a `FourCc` from a 4-byte array.
    ///
    /// This is a `const fn`, so it can be used in constant expressions.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::fourcc::FourCc;
    ///
    /// const MY_CODE: FourCc = FourCc::new(*b"avc1");
    /// assert_eq!(MY_CODE.as_bytes(), b"avc1");
    /// ```
    #[must_use]
    pub const fn new(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    /// Returns a reference to the underlying 4-byte array.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::fourcc::FourCc;
    ///
    /// let fcc = FourCc::new(*b"moov");
    /// assert_eq!(fcc.as_bytes(), b"moov");
    /// ```
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }

    /// Returns `true` if all bytes are ASCII graphic characters or spaces.
    ///
    /// Some FourCCs contain non-printable bytes (e.g. binary box types).
    /// This method identifies the common "human-readable" codes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::fourcc::FourCc;
    ///
    /// assert!(FourCc::new(*b"avc1").is_ascii_printable());
    /// assert!(!FourCc::new([0x00, 0x01, 0x02, 0x03]).is_ascii_printable());
    /// ```
    #[must_use]
    pub fn is_ascii_printable(&self) -> bool {
        self.0.iter().all(|b| b.is_ascii_graphic() || *b == b' ')
    }
}

impl Hash for FourCc {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl std::fmt::Display for FourCc {
    /// Renders printable bytes as-is; non-printable bytes as `\xHH`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for &b in &self.0 {
            if b.is_ascii_graphic() || b == b' ' {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "\\x{b:02X}")?;
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for FourCc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FourCc({} / {:08X})", self, u32::from_be_bytes(self.0))
    }
}

impl std::str::FromStr for FourCc {
    type Err = &'static str;

    /// Parses exactly 4 ASCII bytes into a `FourCc`.
    ///
    /// Returns `Err` if the string is not exactly 4 bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::fourcc::FourCc;
    ///
    /// let fcc: FourCc = "avc1".parse().expect("4-byte string");
    /// assert_eq!(fcc.as_bytes(), b"avc1");
    /// assert!("av1".parse::<FourCc>().is_err());
    /// ```
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let b = s.as_bytes();
        if b.len() == 4 {
            Ok(Self([b[0], b[1], b[2], b[3]]))
        } else {
            Err("FourCc requires exactly 4 bytes")
        }
    }
}

impl From<[u8; 4]> for FourCc {
    fn from(bytes: [u8; 4]) -> Self {
        Self::new(bytes)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Video codec FourCC constants (ISOBMFF / MP4 track handler)
// ─────────────────────────────────────────────────────────────────────────────

/// H.264 / MPEG-4 AVC with in-band parameter sets (ISOBMFF `avc1`).
pub const AVC1: FourCc = FourCc::new(*b"avc1");

/// H.265 / HEVC with in-band parameter sets (ISOBMFF `hvc1`).
pub const HVC1: FourCc = FourCc::new(*b"hvc1");

/// H.265 / HEVC with out-of-band parameter sets (ISOBMFF `hev1`).
pub const HEV1: FourCc = FourCc::new(*b"hev1");

/// AV1 video (ISOBMFF `av01`).
pub const AV01: FourCc = FourCc::new(*b"av01");

/// VP8 video (WebM/ISOBMFF `vp08`).
pub const VP08: FourCc = FourCc::new(*b"vp08");

/// VP9 video (WebM/ISOBMFF `vp09`).
pub const VP09: FourCc = FourCc::new(*b"vp09");

/// Motion JPEG as stored in AVI (`MJPG`, upper-case convention).
pub const MJPG_AVI: FourCc = FourCc::new(*b"MJPG");

/// Motion JPEG as stored in MP4 (`mjpg`, lower-case convention).
pub const MJPG_MP4: FourCc = FourCc::new(*b"mjpg");

/// APV intra-frame professional codec (`apv1`).
pub const APV1: FourCc = FourCc::new(*b"apv1");

/// JPEG-XL ISOBMFF brand / sample entry (`jxl `, note trailing space).
pub const JXL_: FourCc = FourCc::new(*b"jxl ");

/// FFV1 lossless video codec (`FFV1`).
pub const FFV1: FourCc = FourCc::new(*b"FFV1");

// ─────────────────────────────────────────────────────────────────────────────
// Audio codec FourCC constants
// ─────────────────────────────────────────────────────────────────────────────

/// AAC / MPEG-4 Audio (ISOBMFF `mp4a`).
pub const MP4A: FourCc = FourCc::new(*b"mp4a");

/// Opus audio (ISOBMFF `Opus`, RFC 7845 case-sensitive).
pub const OPUS: FourCc = FourCc::new(*b"Opus");

/// FLAC audio (`fLaC`, the FLAC stream marker — 4-byte prefix of the native signature).
pub const FLAC_: FourCc = FourCc::new(*b"fLaC");

/// Vorbis audio (`Vorb` — 4-byte prefix used in ISOBMFF).
pub const VRBIS: FourCc = FourCc::new(*b"Vorb");

// ─────────────────────────────────────────────────────────────────────────────
// ISOBMFF / MP4 box type constants
// ─────────────────────────────────────────────────────────────────────────────

/// `ftyp` — File Type Box (ISOBMFF §4.3).
pub const FTYP: FourCc = FourCc::new(*b"ftyp");

/// `moov` — Movie Container Box.
pub const MOOV: FourCc = FourCc::new(*b"moov");

/// `mdat` — Media Data Box.
pub const MDAT: FourCc = FourCc::new(*b"mdat");

/// `moof` — Movie Fragment Box.
pub const MOOF: FourCc = FourCc::new(*b"moof");

/// `trak` — Track Box.
pub const TRAK: FourCc = FourCc::new(*b"trak");

/// `mdia` — Media Box.
pub const MDIA: FourCc = FourCc::new(*b"mdia");

/// `minf` — Media Information Box.
pub const MINF: FourCc = FourCc::new(*b"minf");

/// `stbl` — Sample Table Box.
pub const STBL: FourCc = FourCc::new(*b"stbl");

/// `stsd` — Sample Description Box.
pub const STSD: FourCc = FourCc::new(*b"stsd");

/// `sidx` — Segment Index Box (MPEG-DASH).
pub const SIDX: FourCc = FourCc::new(*b"sidx");

// ─────────────────────────────────────────────────────────────────────────────
// RIFF / AVI constants
// ─────────────────────────────────────────────────────────────────────────────

/// `RIFF` — RIFF chunk header.
pub const RIFF: FourCc = FourCc::new(*b"RIFF");

/// `LIST` — RIFF LIST chunk.
pub const LIST: FourCc = FourCc::new(*b"LIST");

/// `AVI ` — AVI container brand (trailing space is part of the code).
pub const AVI_: FourCc = FourCc::new(*b"AVI ");

/// `AVIX` — Extended AVI chunk (OpenDML).
pub const AVIX: FourCc = FourCc::new(*b"AVIX");

/// `idx1` — AVI legacy index chunk.
pub const IDX1: FourCc = FourCc::new(*b"idx1");

/// `movi` — AVI movie data chunk.
pub const MOVI: FourCc = FourCc::new(*b"movi");

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── Construction and basic accessors ──────────────────────────────────────

    #[test]
    fn test_new_roundtrip() {
        let fcc = FourCc::new(*b"avc1");
        assert_eq!(fcc.as_bytes(), b"avc1");
    }

    #[test]
    fn test_from_array() {
        let fcc: FourCc = (*b"avc1").into();
        assert_eq!(fcc.as_bytes(), b"avc1");
    }

    #[test]
    fn test_is_ascii_printable_true() {
        assert!(FourCc::new(*b"avc1").is_ascii_printable());
        assert!(FourCc::new(*b"moov").is_ascii_printable());
        assert!(FourCc::new(*b"AVI ").is_ascii_printable()); // space is OK
    }

    #[test]
    fn test_is_ascii_printable_false() {
        assert!(!FourCc::new([0x00, 0x01, 0x02, 0x03]).is_ascii_printable());
        assert!(!FourCc::new([b'a', 0x09, b'c', b'1']).is_ascii_printable()); // tab not allowed
    }

    // ── Display / Debug ───────────────────────────────────────────────────────

    #[test]
    fn test_display_printable() {
        assert_eq!(format!("{}", FourCc::new(*b"avc1")), "avc1");
        assert_eq!(format!("{}", FourCc::new(*b"moov")), "moov");
        assert_eq!(format!("{}", FourCc::new(*b"AVI ")), "AVI ");
    }

    #[test]
    fn test_display_non_printable() {
        let fcc = FourCc::new([0x01, b'a', 0xFF, b'Z']);
        let s = format!("{fcc}");
        assert!(s.contains("\\x01"));
        assert!(s.contains('a'));
        assert!(s.contains("\\xFF"));
        assert!(s.contains('Z'));
    }

    #[test]
    fn test_debug_format() {
        let fcc = FourCc::new(*b"avc1");
        let dbg = format!("{fcc:?}");
        assert!(dbg.contains("FourCc("));
        assert!(dbg.contains("avc1"));
        // Also contains the hex u32 representation
        let hex = format!("{:08X}", u32::from_be_bytes(*b"avc1"));
        assert!(dbg.contains(&hex));
    }

    // ── FromStr ───────────────────────────────────────────────────────────────

    #[test]
    fn test_from_str_valid() {
        let fcc: FourCc = "avc1".parse().expect("4-byte ASCII");
        assert_eq!(fcc.as_bytes(), b"avc1");
    }

    #[test]
    fn test_from_str_too_short() {
        assert!("av1".parse::<FourCc>().is_err());
    }

    #[test]
    fn test_from_str_too_long() {
        assert!("avc12".parse::<FourCc>().is_err());
    }

    #[test]
    fn test_from_str_empty() {
        assert!("".parse::<FourCc>().is_err());
    }

    #[test]
    fn test_from_str_display_roundtrip() {
        let codes = [b"avc1", b"moov", b"mdat", b"RIFF", b"idx1", b"fLaC"];
        for code in &codes {
            let fcc = FourCc::new(**code);
            let s = format!("{fcc}");
            // Display is all-printable for these, so round-trip via parse works
            let parsed: FourCc = s.parse().expect("should round-trip");
            assert_eq!(fcc, parsed, "Round-trip failed for {s}");
        }
    }

    // ── Equality and hash ─────────────────────────────────────────────────────

    #[test]
    fn test_equality() {
        let a = FourCc::new(*b"av01");
        let b: FourCc = "av01".parse().expect("parse");
        assert_eq!(a, b);
    }

    #[test]
    fn test_inequality() {
        let a = FourCc::new(*b"av01");
        let b = FourCc::new(*b"av1 ");
        assert_ne!(a, b);
    }

    #[test]
    fn test_hash_consistency() {
        let mut map: HashMap<FourCc, &str> = HashMap::new();
        map.insert(FourCc::new(*b"avc1"), "H.264");
        map.insert(FourCc::new(*b"av01"), "AV1");
        assert_eq!(map[&AVC1], "H.264");
        assert_eq!(map[&AV01], "AV1");
    }

    // ── Typed video constants ─────────────────────────────────────────────────

    #[test]
    fn test_video_constant_bytes() {
        assert_eq!(AVC1.as_bytes(), b"avc1");
        assert_eq!(HVC1.as_bytes(), b"hvc1");
        assert_eq!(HEV1.as_bytes(), b"hev1");
        assert_eq!(AV01.as_bytes(), b"av01");
        assert_eq!(VP08.as_bytes(), b"vp08");
        assert_eq!(VP09.as_bytes(), b"vp09");
        assert_eq!(MJPG_AVI.as_bytes(), b"MJPG");
        assert_eq!(MJPG_MP4.as_bytes(), b"mjpg");
        assert_eq!(APV1.as_bytes(), b"apv1");
        assert_eq!(JXL_.as_bytes(), b"jxl ");
        assert_eq!(FFV1.as_bytes(), b"FFV1");
    }

    // ── Typed audio constants ─────────────────────────────────────────────────

    #[test]
    fn test_audio_constant_bytes() {
        assert_eq!(MP4A.as_bytes(), b"mp4a");
        assert_eq!(OPUS.as_bytes(), b"Opus");
        assert_eq!(FLAC_.as_bytes(), b"fLaC");
        assert_eq!(VRBIS.as_bytes(), b"Vorb");
    }

    // ── ISOBMFF box type constants ────────────────────────────────────────────

    #[test]
    fn test_isobmff_box_bytes() {
        assert_eq!(FTYP.as_bytes(), b"ftyp");
        assert_eq!(MOOV.as_bytes(), b"moov");
        assert_eq!(MDAT.as_bytes(), b"mdat");
        assert_eq!(MOOF.as_bytes(), b"moof");
        assert_eq!(TRAK.as_bytes(), b"trak");
        assert_eq!(MDIA.as_bytes(), b"mdia");
        assert_eq!(MINF.as_bytes(), b"minf");
        assert_eq!(STBL.as_bytes(), b"stbl");
        assert_eq!(STSD.as_bytes(), b"stsd");
        assert_eq!(SIDX.as_bytes(), b"sidx");
    }

    // ── RIFF/AVI constants ────────────────────────────────────────────────────

    #[test]
    fn test_riff_avi_bytes() {
        assert_eq!(RIFF.as_bytes(), b"RIFF");
        assert_eq!(LIST.as_bytes(), b"LIST");
        assert_eq!(AVI_.as_bytes(), b"AVI ");
        assert_eq!(AVIX.as_bytes(), b"AVIX");
        assert_eq!(IDX1.as_bytes(), b"idx1");
        assert_eq!(MOVI.as_bytes(), b"movi");
    }

    // ── MJPG AVI vs MP4 are distinct ─────────────────────────────────────────

    #[test]
    fn test_mjpg_variants_distinct() {
        assert_ne!(MJPG_AVI, MJPG_MP4);
        assert_eq!(MJPG_AVI.as_bytes(), b"MJPG");
        assert_eq!(MJPG_MP4.as_bytes(), b"mjpg");
    }

    // ── All printable constants are ASCII-printable ───────────────────────────

    #[test]
    fn test_all_named_constants_printable() {
        let constants = [
            AVC1, HVC1, HEV1, AV01, VP08, VP09, MJPG_AVI, MJPG_MP4, APV1, JXL_, FFV1, MP4A, OPUS,
            FLAC_, VRBIS, FTYP, MOOV, MDAT, MOOF, TRAK, MDIA, MINF, STBL, STSD, SIDX, RIFF, LIST,
            AVI_, AVIX, IDX1, MOVI,
        ];
        for c in &constants {
            assert!(
                c.is_ascii_printable(),
                "{c:?} should be ASCII-printable but is not"
            );
        }
    }
}
