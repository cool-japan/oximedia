//! `FourCC` (Four Character Code) types and registry for codec/format identification.
//!
//! `FourCC` codes are four-byte identifiers used in multimedia containers and
//! codecs to identify stream types, pixel formats, and compression schemes.

#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt;

/// A four-character code (`FourCC`) identifier.
///
/// Used to identify codec types, pixel formats, and container formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FourCC([u8; 4]);

impl FourCC {
    /// Creates a `FourCC` from a 4-byte array.
    ///
    /// # Example
    /// ```
    /// use oximedia_core::fourcc::FourCC;
    /// let fcc = FourCC::from_bytes(*b"AV01");
    /// assert_eq!(fcc.as_bytes(), b"AV01");
    /// ```
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    /// Creates a `FourCC` from a string slice.
    ///
    /// Returns `None` if the string is not exactly 4 bytes (ASCII).
    ///
    /// # Example
    /// ```
    /// use oximedia_core::fourcc::FourCC;
    /// let fcc = FourCC::parse("VP90").expect("valid fourcc");
    /// assert!(fcc.is_video());
    /// ```
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 4 {
            return None;
        }
        Some(Self([b[0], b[1], b[2], b[3]]))
    }

    /// Returns the raw bytes of this `FourCC`.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }

    /// Returns `true` if this `FourCC` identifies a known video codec.
    ///
    /// Recognises patent-free video codecs and image formats: AV1, VP9, VP8,
    /// Theora, FFV1, WebP, GIF, PNG, TIFF, JPEG-XL, DNG, OpenEXR.
    #[must_use]
    pub fn is_video(&self) -> bool {
        matches!(
            &self.0,
            b"AV01"
                | b"VP90"
                | b"VP80"
                | b"theo"
                | b"FFV1"
                | b"WEBP"
                | b"GIF "
                | b"PNG "
                | b"TIFF"
                | b"jxl "
                | b"CDNG"
                | b"exr "
        )
    }

    /// Returns `true` if this `FourCC` identifies a known audio codec.
    ///
    /// Recognises patent-free audio codecs: Opus, Vorbis, FLAC, PCM
    /// variants, and MP3 (patents expired 2017).
    #[must_use]
    pub fn is_audio(&self) -> bool {
        matches!(
            &self.0,
            b"Opus" | b"Vorb" | b"fLaC" | b"araw" | b"sowt" | b"mp3 "
        )
    }

    /// Returns the numeric representation of this `FourCC` as a `u32`.
    #[must_use]
    pub fn as_u32(&self) -> u32 {
        u32::from_be_bytes(self.0)
    }
}

impl fmt::Display for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &b in &self.0 {
            if b.is_ascii_graphic() || b == b' ' {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, ".")?;
            }
        }
        Ok(())
    }
}

impl From<[u8; 4]> for FourCC {
    fn from(bytes: [u8; 4]) -> Self {
        Self::from_bytes(bytes)
    }
}

impl TryFrom<&str> for FourCC {
    type Error = &'static str;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::parse(s).ok_or("FourCC string must be exactly 4 ASCII bytes")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Typed FourCC constants for all supported codecs
// ─────────────────────────────────────────────────────────────────────────────

/// AV1 video codec (`AV01`).
pub const FOURCC_AV1: FourCC = FourCC::from_bytes(*b"AV01");
/// VP9 video codec (`VP90`).
pub const FOURCC_VP9: FourCC = FourCC::from_bytes(*b"VP90");
/// VP8 video codec (`VP80`).
pub const FOURCC_VP8: FourCC = FourCC::from_bytes(*b"VP80");
/// Theora video codec (`theo`).
pub const FOURCC_THEORA: FourCC = FourCC::from_bytes(*b"theo");
/// FFV1 lossless video codec (`FFV1`).
pub const FOURCC_FFV1: FourCC = FourCC::from_bytes(*b"FFV1");
/// WebP image codec (`WEBP`).
pub const FOURCC_WEBP: FourCC = FourCC::from_bytes(*b"WEBP");
/// GIF image format (`GIF ` — trailing space to pad to 4 bytes).
pub const FOURCC_GIF: FourCC = FourCC::from_bytes(*b"GIF ");
/// PNG lossless image format (`PNG `).
pub const FOURCC_PNG: FourCC = FourCC::from_bytes(*b"PNG ");
/// TIFF image format (`TIFF`).
pub const FOURCC_TIFF: FourCC = FourCC::from_bytes(*b"TIFF");
/// JPEG-XL image codec (`jxl `).
pub const FOURCC_JPEGXL: FourCC = FourCC::from_bytes(*b"jxl ");
/// DNG RAW image format (`CDNG`).
pub const FOURCC_DNG: FourCC = FourCC::from_bytes(*b"CDNG");
/// OpenEXR HDR image format (`exr `).
pub const FOURCC_OPENEXR: FourCC = FourCC::from_bytes(*b"exr ");
/// Opus audio codec (`Opus`).
pub const FOURCC_OPUS: FourCC = FourCC::from_bytes(*b"Opus");
/// Vorbis audio codec (`Vorb`).
pub const FOURCC_VORBIS: FourCC = FourCC::from_bytes(*b"Vorb");
/// FLAC lossless audio codec (`fLaC`).
pub const FOURCC_FLAC: FourCC = FourCC::from_bytes(*b"fLaC");
/// PCM uncompressed audio — big-endian/signed (`araw`).
pub const FOURCC_PCM: FourCC = FourCC::from_bytes(*b"araw");
/// PCM little-endian audio (`sowt`).
pub const FOURCC_PCM_LE: FourCC = FourCC::from_bytes(*b"sowt");
/// MP3 audio codec — MPEG-1/2 Layer III patents expired (`mp3 `).
pub const FOURCC_MP3: FourCC = FourCC::from_bytes(*b"mp3 ");
/// WebVTT subtitle format (`wvtt`).
pub const FOURCC_WEBVTT: FourCC = FourCC::from_bytes(*b"wvtt");
/// SRT subtitle format (`SRT `).
pub const FOURCC_SRT: FourCC = FourCC::from_bytes(*b"SRT ");
/// ASS/SSA subtitle format (`SSA `).
pub const FOURCC_ASS: FourCC = FourCC::from_bytes(*b"SSA ");

/// A registry mapping `FourCC` codes to human-readable names.
///
/// Provides lookup and registration of `FourCC` codes used within a pipeline.
#[derive(Debug, Default)]
pub struct FourCCRegistry {
    entries: HashMap<FourCC, String>,
}

impl FourCCRegistry {
    /// Creates a new, empty `FourCCRegistry`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Creates a registry pre-populated with well-known patent-free codecs.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();
        for (code, name) in Self::known_video_codecs() {
            reg.register(code, name);
        }
        for (code, name) in Self::known_audio_codecs() {
            reg.register(code, name);
        }
        reg
    }

    /// Registers a `FourCC` with an associated name.
    pub fn register(&mut self, code: FourCC, name: impl Into<String>) {
        self.entries.insert(code, name.into());
    }

    /// Looks up the name for a `FourCC`, returning `None` if not registered.
    #[must_use]
    pub fn lookup(&self, code: &FourCC) -> Option<&str> {
        self.entries.get(code).map(String::as_str)
    }

    /// Returns the total number of registered `FourCC` entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the registry contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns well-known patent-free video codec FourCC/name pairs.
    ///
    /// Includes all video codecs and image formats with typed constants defined
    /// in this module.
    #[must_use]
    pub fn known_video_codecs() -> Vec<(FourCC, &'static str)> {
        vec![
            (FOURCC_AV1, "AV1"),
            (FOURCC_VP9, "VP9"),
            (FOURCC_VP8, "VP8"),
            (FOURCC_THEORA, "Theora"),
            (FOURCC_FFV1, "FFV1"),
            (FOURCC_WEBP, "WebP"),
            (FOURCC_GIF, "GIF"),
            (FOURCC_PNG, "PNG"),
            (FOURCC_TIFF, "TIFF"),
            (FOURCC_JPEGXL, "JPEG-XL"),
            (FOURCC_DNG, "DNG"),
            (FOURCC_OPENEXR, "OpenEXR"),
        ]
    }

    /// Returns well-known patent-free audio codec FourCC/name pairs.
    ///
    /// Includes all audio codecs with typed constants defined in this module.
    #[must_use]
    pub fn known_audio_codecs() -> Vec<(FourCC, &'static str)> {
        vec![
            (FOURCC_OPUS, "Opus"),
            (FOURCC_VORBIS, "Vorbis"),
            (FOURCC_FLAC, "FLAC"),
            (FOURCC_PCM, "PCM"),
            (FOURCC_PCM_LE, "PCM-LE"),
            (FOURCC_MP3, "MP3"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_bytes_roundtrip() {
        let fcc = FourCC::from_bytes(*b"AV01");
        assert_eq!(fcc.as_bytes(), b"AV01");
    }

    #[test]
    fn test_from_str_valid() {
        let fcc = FourCC::parse("VP90").expect("should succeed");
        assert_eq!(fcc.as_bytes(), b"VP90");
    }

    #[test]
    fn test_from_str_too_short() {
        assert!(FourCC::parse("VP9").is_none());
    }

    #[test]
    fn test_from_str_too_long() {
        assert!(FourCC::parse("VP901").is_none());
    }

    #[test]
    fn test_is_video_av1() {
        let fcc = FourCC::from_bytes(*b"AV01");
        assert!(fcc.is_video());
    }

    #[test]
    fn test_is_video_vp9() {
        let fcc = FourCC::from_bytes(*b"VP90");
        assert!(fcc.is_video());
    }

    #[test]
    fn test_is_video_vp8() {
        let fcc = FourCC::from_bytes(*b"VP80");
        assert!(fcc.is_video());
    }

    #[test]
    fn test_is_video_theora() {
        let fcc = FourCC::from_bytes(*b"theo");
        assert!(fcc.is_video());
    }

    #[test]
    fn test_is_audio_opus() {
        let fcc = FourCC::from_bytes(*b"Opus");
        assert!(fcc.is_audio());
        assert!(!fcc.is_video());
    }

    #[test]
    fn test_is_audio_flac() {
        let fcc = FourCC::from_bytes(*b"fLaC");
        assert!(fcc.is_audio());
    }

    #[test]
    fn test_display_ascii() {
        let fcc = FourCC::from_bytes(*b"AV01");
        assert_eq!(format!("{fcc}"), "AV01");
    }

    #[test]
    fn test_display_non_ascii() {
        // Non-printable bytes should render as '.'
        let fcc = FourCC::from_bytes([0x01, 0x02, 0x03, 0x04]);
        let s = format!("{fcc}");
        assert!(s.chars().all(|c| c == '.'));
    }

    #[test]
    fn test_as_u32() {
        let fcc = FourCC::from_bytes(*b"AV01");
        assert_eq!(fcc.as_u32(), u32::from_be_bytes(*b"AV01"));
    }

    #[test]
    fn test_from_array_trait() {
        let fcc: FourCC = (*b"VP80").into();
        assert!(fcc.is_video());
    }

    #[test]
    fn test_try_from_str_ok() {
        let fcc = FourCC::try_from("Opus").expect("try_from should succeed");
        assert!(fcc.is_audio());
    }

    #[test]
    fn test_try_from_str_err() {
        assert!(FourCC::try_from("XY").is_err());
    }

    #[test]
    fn test_registry_register_lookup() {
        let mut reg = FourCCRegistry::new();
        let code = FourCC::from_bytes(*b"AV01");
        reg.register(code, "AV1 Video");
        assert_eq!(reg.lookup(&code), Some("AV1 Video"));
    }

    #[test]
    fn test_registry_lookup_missing() {
        let reg = FourCCRegistry::new();
        let code = FourCC::from_bytes(*b"XXXX");
        assert!(reg.lookup(&code).is_none());
    }

    #[test]
    fn test_registry_len_and_empty() {
        let mut reg = FourCCRegistry::new();
        assert!(reg.is_empty());
        reg.register(FourCC::from_bytes(*b"AV01"), "AV1");
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn test_registry_with_defaults_not_empty() {
        let reg = FourCCRegistry::with_defaults();
        assert!(!reg.is_empty());
    }

    #[test]
    fn test_known_video_codecs_count() {
        // AV1, VP9, VP8, Theora, FFV1, WebP, GIF, PNG, TIFF, JPEG-XL, DNG, OpenEXR = 12
        assert_eq!(FourCCRegistry::known_video_codecs().len(), 12);
    }

    #[test]
    fn test_known_audio_codecs_count() {
        // Opus, Vorbis, FLAC, PCM, PCM-LE, MP3 = 6
        assert_eq!(FourCCRegistry::known_audio_codecs().len(), 6);
    }

    #[test]
    fn test_known_video_codecs_all_is_video() {
        for (fcc, _) in FourCCRegistry::known_video_codecs() {
            assert!(fcc.is_video(), "{fcc} should be video");
        }
    }

    #[test]
    fn test_equality() {
        let a = FourCC::from_bytes(*b"AV01");
        let b = FourCC::parse("AV01").expect("should succeed");
        assert_eq!(a, b);
    }

    // ── Typed constant tests ─────────────────────────────────────────────────

    #[test]
    fn test_typed_video_constants() {
        assert!(FOURCC_AV1.is_video());
        assert!(FOURCC_VP9.is_video());
        assert!(FOURCC_VP8.is_video());
        assert!(FOURCC_THEORA.is_video());
        assert!(FOURCC_FFV1.is_video());
        assert!(FOURCC_WEBP.is_video());
        assert!(FOURCC_GIF.is_video());
        assert!(FOURCC_PNG.is_video());
        assert!(FOURCC_TIFF.is_video());
        assert!(FOURCC_JPEGXL.is_video());
        assert!(FOURCC_DNG.is_video());
        assert!(FOURCC_OPENEXR.is_video());
    }

    #[test]
    fn test_typed_audio_constants() {
        assert!(FOURCC_OPUS.is_audio());
        assert!(FOURCC_VORBIS.is_audio());
        assert!(FOURCC_FLAC.is_audio());
        assert!(FOURCC_PCM.is_audio());
        assert!(FOURCC_PCM_LE.is_audio());
        assert!(FOURCC_MP3.is_audio());
    }

    #[test]
    fn test_typed_constants_match_from_bytes() {
        assert_eq!(FOURCC_AV1, FourCC::from_bytes(*b"AV01"));
        assert_eq!(FOURCC_VP9, FourCC::from_bytes(*b"VP90"));
        assert_eq!(FOURCC_OPUS, FourCC::from_bytes(*b"Opus"));
        assert_eq!(FOURCC_FLAC, FourCC::from_bytes(*b"fLaC"));
    }

    #[test]
    fn test_typed_subtitle_constants() {
        // Subtitle FourCCs are not video or audio
        assert!(!FOURCC_WEBVTT.is_video());
        assert!(!FOURCC_WEBVTT.is_audio());
        assert!(!FOURCC_SRT.is_video());
        assert!(!FOURCC_ASS.is_audio());
    }

    #[test]
    fn test_video_constants_not_audio() {
        assert!(!FOURCC_AV1.is_audio());
        assert!(!FOURCC_VP9.is_audio());
        assert!(!FOURCC_FFV1.is_audio());
    }

    #[test]
    fn test_audio_constants_not_video() {
        assert!(!FOURCC_OPUS.is_video());
        assert!(!FOURCC_FLAC.is_video());
        assert!(!FOURCC_MP3.is_video());
    }

    #[test]
    fn test_known_audio_codecs_all_is_audio() {
        for (fcc, _) in FourCCRegistry::known_audio_codecs() {
            assert!(fcc.is_audio(), "{fcc} should be audio");
        }
    }

    #[test]
    fn test_registry_with_defaults_contains_all_codecs() {
        let reg = FourCCRegistry::with_defaults();
        // Should contain all 12 video + 6 audio = 18 entries
        assert_eq!(reg.len(), 18);
        assert_eq!(reg.lookup(&FOURCC_AV1), Some("AV1"));
        assert_eq!(reg.lookup(&FOURCC_OPUS), Some("Opus"));
        assert_eq!(reg.lookup(&FOURCC_JPEGXL), Some("JPEG-XL"));
        assert_eq!(reg.lookup(&FOURCC_MP3), Some("MP3"));
    }
}
