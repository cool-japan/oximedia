//! Tests for patent violation detection in `error.rs`.
//!
//! Verifies that:
//! 1. `OxiError::PatentViolation` is correctly created, identified, and displayed.
//! 2. All known patent-encumbered codec names are rejected by `CodecId::from_str()`,
//!    since those names do not appear in OxiMedia's Green List.

use oximedia_core::{
    error::{OxiError, OxiResult},
    types::CodecId,
};

// ─────────────────────────────────────────────────────────────────────────────
// OxiError::PatentViolation construction and introspection
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_patent_violation_variant_creation_h264() {
    let err = OxiError::patent_violation("H.264");
    assert!(
        err.is_patent_violation(),
        "H.264 should produce PatentViolation"
    );
    let msg = format!("{err}");
    assert!(msg.contains("H.264"), "display must include codec name");
}

#[test]
fn test_patent_violation_variant_creation_h265() {
    let err = OxiError::patent_violation("H.265");
    assert!(err.is_patent_violation());
    let msg = format!("{err}");
    assert!(msg.contains("H.265"));
}

#[test]
fn test_patent_violation_variant_creation_aac() {
    let err = OxiError::patent_violation("AAC");
    assert!(err.is_patent_violation());
    assert!(format!("{err}").contains("AAC"));
}

#[test]
fn test_patent_violation_variant_creation_ac3() {
    let err = OxiError::patent_violation("AC-3");
    assert!(err.is_patent_violation());
    assert!(format!("{err}").contains("AC-3"));
}

#[test]
fn test_patent_violation_variant_creation_dts() {
    let err = OxiError::patent_violation("DTS");
    assert!(err.is_patent_violation());
    assert!(format!("{err}").contains("DTS"));
}

#[test]
fn test_patent_violation_variant_creation_hevc() {
    let err = OxiError::patent_violation("HEVC");
    assert!(err.is_patent_violation());
    assert!(format!("{err}").contains("HEVC"));
}

#[test]
fn test_patent_violation_variant_creation_vc1() {
    let err = OxiError::patent_violation("VC-1");
    assert!(err.is_patent_violation());
    assert!(format!("{err}").contains("VC-1"));
}

#[test]
fn test_patent_violation_variant_creation_mp4a() {
    let err = OxiError::patent_violation("mp4a");
    assert!(err.is_patent_violation());
    assert!(format!("{err}").contains("mp4a"));
}

/// Multiple distinct codec names can all create PatentViolation errors.
#[test]
fn test_all_known_patent_encumbered_names_create_violation_error() {
    let encumbered: &[&str] = &[
        "H.264",
        "H.265",
        "AAC",
        "AC-3",
        "E-AC-3",
        "DTS",
        "DTS-HD",
        "HEVC",
        "AVC",
        "VC-1",
        "VC1",
        "WMV3",
        "mp4a",
        "MPEG-4 Visual",
        "H.264/AVC",
        "H.265/HEVC",
        "H.266/VVC",
    ];
    for name in encumbered {
        let err = OxiError::patent_violation(*name);
        assert!(
            err.is_patent_violation(),
            "patent_violation(\"{name}\") should create PatentViolation, got: {err:?}"
        );
        let display = format!("{err}");
        assert!(
            display.contains(name),
            "display for \"{name}\" should include the codec name; got: {display}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// is_patent_violation() predicate is false for all other error variants
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_is_patent_violation_false_for_eof() {
    assert!(!OxiError::Eof.is_patent_violation());
}

#[test]
fn test_is_patent_violation_false_for_codec_error() {
    assert!(!OxiError::codec("some failure").is_patent_violation());
}

#[test]
fn test_is_patent_violation_false_for_unsupported() {
    assert!(!OxiError::unsupported("unknown format").is_patent_violation());
}

#[test]
fn test_is_patent_violation_false_for_parse_error() {
    assert!(!OxiError::parse(0, "bad magic").is_patent_violation());
}

#[test]
fn test_is_patent_violation_false_for_buffer_too_small() {
    assert!(!OxiError::buffer_too_small(256, 128).is_patent_violation());
}

#[test]
fn test_is_patent_violation_false_for_invalid_data() {
    assert!(!OxiError::invalid_data("truncated header").is_patent_violation());
}

#[test]
fn test_is_patent_violation_false_for_unexpected_eof() {
    assert!(!OxiError::UnexpectedEof.is_patent_violation());
}

#[test]
fn test_is_patent_violation_false_for_unknown_format() {
    assert!(!OxiError::UnknownFormat.is_patent_violation());
}

// ─────────────────────────────────────────────────────────────────────────────
// OxiResult<T> with PatentViolation propagation
// ─────────────────────────────────────────────────────────────────────────────

fn reject_encumbered(name: &str) -> OxiResult<CodecId> {
    Err(OxiError::patent_violation(name))
}

#[test]
fn test_patent_violation_propagates_through_result() {
    let result = reject_encumbered("H.264");
    let err = result.expect_err("should be an error");
    assert!(err.is_patent_violation());
    assert!(format!("{err}").contains("H.264"));
}

// ─────────────────────────────────────────────────────────────────────────────
// CodecId::from_str rejects patent-encumbered codec names
//
// The Green List policy means these names must not parse to any CodecId.
// A failed parse returns OxiError::Unsupported (not PatentViolation, since
// the rejection happens at the string-parsing level, not the bitstream level).
// ─────────────────────────────────────────────────────────────────────────────

/// All of these are patent-encumbered and must NOT appear in the Green List.
const ENCUMBERED_PARSE_NAMES: &[&str] = &[
    // H.264 / AVC
    "h264", "h.264", "avc", "avc1", // H.265 / HEVC
    "h265", "h.265", "hevc",
    // AAC. `CodecId::Aac` exists as an *identification-only* variant (stream
    // inspectors and platform extensions must be able to name what they found),
    // but it stays unreachable from a string so no name-driven code path can
    // select AAC. See `types::codec_id`'s `FromStr` docs.
    "aac", "he-aac", "aac-lc", "mp4a", // AC-3 / E-AC-3 (Dolby)
    "ac3", "ac-3", "eac3", "eac-3", // DTS
    "dts", "dtshd", // VC-1 / WMV
    "vc1", "vc-1", "wmv3", // MPEG-4 Part 2 (Xvid / DivX)
    "mp4v", "mpeg4", "xvid", "divx", // H.266 / VVC (very recent)
    "vvc", "h266", "h.266",
];

#[test]
fn test_encumbered_codec_names_are_not_parseable_as_codec_id() {
    for name in ENCUMBERED_PARSE_NAMES {
        let result = name.parse::<CodecId>();
        assert!(
            result.is_err(),
            "CodecId::from_str(\"{name}\") should fail — \"{name}\" is patent-encumbered \
             or not on the Green List; unexpectedly parsed as: {:?}",
            result
        );
    }
}

/// Case-insensitive check: uppercase variants must also be rejected.
#[test]
fn test_encumbered_codec_names_rejected_case_insensitive() {
    let upper_samples: &[&str] = &["H264", "H265", "AAC", "HEVC", "AC3", "DTS", "VC1", "XVID"];
    for name in upper_samples {
        assert!(
            name.parse::<CodecId>().is_err(),
            "CodecId::from_str(\"{name}\") should be rejected"
        );
    }
}

/// Green List codecs must still parse correctly (no false positives).
#[test]
fn test_green_list_codecs_are_not_rejected() {
    let green_list: &[(&str, CodecId)] = &[
        ("av1", CodecId::Av1),
        ("vp9", CodecId::Vp9),
        ("vp8", CodecId::Vp8),
        ("theora", CodecId::Theora),
        ("opus", CodecId::Opus),
        ("vorbis", CodecId::Vorbis),
        ("flac", CodecId::Flac),
        ("pcm", CodecId::Pcm),
        ("webvtt", CodecId::WebVtt),
        ("srt", CodecId::Srt),
        ("ffv1", CodecId::Ffv1),
        ("webp", CodecId::WebP),
        ("gif", CodecId::Gif),
        ("png", CodecId::Png),
    ];
    for (name, expected) in green_list {
        let parsed = name
            .parse::<CodecId>()
            .unwrap_or_else(|e| panic!("Green List codec \"{name}\" should parse: {e}"));
        assert_eq!(parsed, *expected, "Green List codec \"{name}\" mismatch");
    }
}
