// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Round-trip integration tests for the canonical [`TransformParams`]
//! serializer.
//!
//! These tests prove that
//! `parse_transform_string → TransformParams::serialize → parse_transform_string`
//! is idempotent and that malformed / out-of-range inputs are handled per the
//! crate's defined behaviour (clamp / ignore / `Err`) — never a panic.

use oximedia_image_transform::parser::parse_transform_string;
use oximedia_image_transform::transform::{
    Color, FitMode, Gravity, MetadataMode, OutputFormat, OutputOptions, Rotation, TransformParams,
};

/// Helper: parse a transform string, asserting success with a descriptive message.
fn parse(s: &str) -> TransformParams {
    parse_transform_string(s).unwrap_or_else(|e| panic!("expected `{s}` to parse, got {e:?}"))
}

// ---------------------------------------------------------------------------
// Basic round-trip
// ---------------------------------------------------------------------------

#[test]
fn basic_round_trip_preserves_all_fields() {
    let original = parse("w=800,h=600,q=85");
    let serialized = original.serialize();
    let reparsed = parse(&serialized);

    assert_eq!(original.width, reparsed.width);
    assert_eq!(original.height, reparsed.height);
    assert_eq!(original.quality, reparsed.quality);
    assert_eq!(original.format, reparsed.format);
    assert_eq!(original.fit, reparsed.fit);
    // Strongest assertion: full structural equality.
    assert_eq!(original, reparsed);
}

#[test]
fn rich_param_set_round_trips_exactly() {
    // Four-sided geometry uses the comma-free `x` form so it survives the
    // top-level comma split (`trim=10x5x10x5`, `border=...x...:color`).
    let s = "width=1024,height=768,quality=72,format=webp,fit=cover,gravity=top-left,\
             sharpen=2,blur=5,brightness=0.25,contrast=-0.3,gamma=2.2,rotate=90,\
             dpr=2,metadata=keep,anim=false,background=ff8800,compression=best,\
             onerror=redirect,trim=10x5x10x5,border=3x4x3x4:112233,pad=0.1x0.2x0.1x0.2";
    let original = parse(s);
    let reparsed = parse(&original.serialize());
    assert_eq!(original, reparsed);
}

// ---------------------------------------------------------------------------
// Defaults are not emitted
// ---------------------------------------------------------------------------

#[test]
fn default_params_serialize_to_empty_string() {
    let p = TransformParams::default();
    assert_eq!(p.serialize(), "");
    // Empty string re-parses to the default.
    assert_eq!(parse(""), p);
}

#[test]
fn default_valued_fields_are_omitted() {
    // Explicitly set several fields to their default value alongside one real
    // change; the serializer must drop every default and emit only `width`.
    let mut p = TransformParams::default();
    p.width = Some(640);
    p.quality = 85; // default
    p.format = OutputFormat::Auto; // default
    p.fit = FitMode::ScaleDown; // default
    p.gravity = Gravity::Center; // default
    p.gamma = 1.0; // default
    p.dpr = 1.0; // default
    p.anim = true; // default
    p.metadata = MetadataMode::None; // default

    let serialized = p.serialize();
    assert_eq!(
        serialized, "width=640",
        "only the non-default field survives"
    );

    let reparsed = parse(&serialized);
    assert_eq!(p, reparsed);
}

#[test]
fn serialized_string_is_compact() {
    let original = parse("w=800,h=600");
    let serialized = original.serialize();
    // No default tokens leaked in.
    assert!(!serialized.contains("quality"));
    assert!(!serialized.contains("format"));
    assert!(!serialized.contains("fit"));
    assert!(!serialized.contains("dpr"));
    assert_eq!(serialized, "width=800,height=600");
}

// ---------------------------------------------------------------------------
// Unknown keys dropped
// ---------------------------------------------------------------------------

#[test]
fn unknown_keys_are_dropped_on_round_trip() {
    let original = parse("unknown=value,w=100");
    // Unknown key ignored on parse.
    assert_eq!(original.width, Some(100));

    let serialized = original.serialize();
    assert!(
        !serialized.contains("unknown"),
        "serialized form must not carry the unknown key: {serialized}"
    );
    assert_eq!(serialized, "width=100");

    let reparsed = parse(&serialized);
    assert_eq!(original, reparsed);
}

#[test]
fn multiple_unknown_keys_dropped() {
    let original = parse("foo=1,w=320,bar=baz,h=240,future_param=42");
    assert_eq!(original.width, Some(320));
    assert_eq!(original.height, Some(240));
    let serialized = original.serialize();
    for ghost in ["foo", "bar", "baz", "future_param"] {
        assert!(
            !serialized.contains(ghost),
            "{ghost} leaked into {serialized}"
        );
    }
    assert_eq!(parse(&serialized), original);
}

// ---------------------------------------------------------------------------
// All format variants round-trip
// ---------------------------------------------------------------------------

#[test]
fn all_format_variants_round_trip() {
    let formats = [
        OutputFormat::Auto,
        OutputFormat::Avif,
        OutputFormat::WebP,
        OutputFormat::Jpeg,
        OutputFormat::Png,
        OutputFormat::Gif,
        OutputFormat::Baseline,
        OutputFormat::Json,
    ];
    for fmt in formats {
        let mut p = TransformParams::default();
        p.width = Some(512);
        p.format = fmt;
        let serialized = p.serialize();
        let reparsed = parse(&serialized);
        assert_eq!(
            reparsed.format, fmt,
            "format {fmt:?} did not round-trip via `{serialized}`"
        );
        assert_eq!(p, reparsed, "full struct mismatch for format {fmt:?}");
    }
}

#[test]
fn all_fit_modes_round_trip() {
    let modes = [
        FitMode::ScaleDown,
        FitMode::Contain,
        FitMode::Cover,
        FitMode::Crop,
        FitMode::Pad,
        FitMode::Fill,
    ];
    for fit in modes {
        let mut p = TransformParams::default();
        p.width = Some(400);
        p.fit = fit;
        assert_eq!(parse(&p.serialize()), p, "fit {fit:?} did not round-trip");
    }
}

#[test]
fn all_rotation_variants_round_trip() {
    let rotations = [
        Rotation::Deg0,
        Rotation::Deg90,
        Rotation::Deg180,
        Rotation::Deg270,
        Rotation::Auto,
    ];
    for r in rotations {
        let mut p = TransformParams::default();
        p.rotate = r;
        assert_eq!(
            parse(&p.serialize()),
            p,
            "rotation {r:?} did not round-trip"
        );
    }
}

#[test]
fn named_and_focal_gravities_round_trip() {
    let gravities = [
        Gravity::Auto,
        Gravity::Center,
        Gravity::Top,
        Gravity::Bottom,
        Gravity::Left,
        Gravity::Right,
        Gravity::TopLeft,
        Gravity::TopRight,
        Gravity::BottomLeft,
        Gravity::BottomRight,
        Gravity::Face,
        Gravity::FocalPoint(0.25, 0.75),
    ];
    for g in gravities {
        let mut p = TransformParams::default();
        p.gravity = g.clone();
        assert_eq!(parse(&p.serialize()), p, "gravity {g:?} did not round-trip");
    }
}

// ---------------------------------------------------------------------------
// Order-independence → identical canonical string
// ---------------------------------------------------------------------------

#[test]
fn order_independent_inputs_produce_same_canonical_string() {
    let a = parse("w=800,h=600");
    let b = parse("h=600,w=800");
    assert_eq!(a, b, "field-order should not affect the parsed struct");
    assert_eq!(
        a.serialize(),
        b.serialize(),
        "both orderings must canonicalise to the same string"
    );
    assert_eq!(a.serialize(), "width=800,height=600");
}

#[test]
fn shuffled_rich_inputs_canonicalise_identically() {
    let forward = parse("w=400,h=300,q=70,format=webp,fit=cover");
    let shuffled = parse("fit=cover,format=webp,q=70,h=300,w=400");
    assert_eq!(forward, shuffled);
    assert_eq!(forward.serialize(), shuffled.serialize());
}

// ---------------------------------------------------------------------------
// Idempotency across repeated rounds
// ---------------------------------------------------------------------------

#[test]
fn serialize_is_idempotent_after_first_round() {
    let start = parse(
        "h=600,w=800,q=90,format=avif,fit=cover,gravity=face,blur=3,gamma=2.2,dpr=2,\
         metadata=keep,anim=false,background=112233,compression=fast,rotate=180",
    );

    let mut current = start.serialize();
    for round in 1..=5 {
        let params = parse(&current);
        let next = params.serialize();
        if round > 1 {
            assert_eq!(
                current, next,
                "serialization must stabilise from round 2 onward (round {round})"
            );
        }
        current = next;
    }
}

#[test]
fn repeated_parse_serialize_preserves_struct_equality() {
    let original = parse("w=800,h=600,q=85,format=webp,fit=cover,blur=5");
    let mut params = original.clone();
    for _ in 0..5 {
        let serialized = params.serialize();
        params = parse(&serialized);
        assert_eq!(
            params, original,
            "struct identity must be preserved each round"
        );
    }
}

// ---------------------------------------------------------------------------
// output_options round-trip (parser ↔ serializer inverse)
// ---------------------------------------------------------------------------

#[test]
fn non_default_output_options_round_trip() {
    let mut p = TransformParams::default();
    p.format = OutputFormat::Jpeg;
    p.output_options = Some(OutputOptions {
        progressive_jpeg: true,
        quality: 60,
    });
    let serialized = p.serialize();
    assert!(serialized.contains("progressive=true"), "{serialized}");
    assert!(serialized.contains("output_quality=60"), "{serialized}");
    assert_eq!(parse(&serialized), p);
}

#[test]
fn onerror_round_trips() {
    // `serialize` (unlike `cache_key`) preserves the onerror fallback.
    let mut p = TransformParams::default();
    p.onerror = Some("https://example.com/fallback.jpg".to_string());
    let serialized = p.serialize();
    assert!(serialized.contains("onerror="), "{serialized}");
    assert_eq!(parse(&serialized), p);
}

#[test]
fn background_color_round_trips() {
    let mut p = TransformParams::default();
    p.background = Color::new(0x12, 0x34, 0x56, 0x80);
    let serialized = p.serialize();
    let reparsed = parse(&serialized);
    assert_eq!(reparsed.background, p.background);
    assert_eq!(reparsed, p);
}

// ---------------------------------------------------------------------------
// Multi-side geometry (comma-free `x` form survives the top-level comma split)
// ---------------------------------------------------------------------------

#[test]
fn uniform_geometry_uses_compact_short_form() {
    let p = parse("trim=8,pad=0.1,border=4:ff0000");
    let serialized = p.serialize();
    // Uniform sides collapse to the single-value short form — no `x`, no commas.
    assert!(serialized.contains("trim=8"), "{serialized}");
    assert!(serialized.contains("pad=0.1"), "{serialized}");
    assert!(serialized.contains("border=4:ff0000"), "{serialized}");
    assert!(
        !serialized.contains('x'),
        "uniform geometry must not use the x-form: {serialized}"
    );
    assert_eq!(parse(&serialized), p);
}

#[test]
fn non_uniform_trim_round_trips_via_x_form() {
    let p = parse("trim=10x5x10x5");
    let trim = p.trim.expect("trim should be set");
    assert_eq!(
        (trim.top, trim.right, trim.bottom, trim.left),
        (10, 5, 10, 5)
    );
    let serialized = p.serialize();
    assert!(serialized.contains("trim=10x5x10x5"), "{serialized}");
    assert_eq!(parse(&serialized), p);
}

#[test]
fn non_uniform_border_round_trips_via_x_form() {
    let mut p = TransformParams::default();
    p.border = Some(oximedia_image_transform::transform::Border {
        color: Color::new(0xaa, 0xbb, 0xcc, 0xff),
        top: 1,
        right: 2,
        bottom: 3,
        left: 4,
    });
    let serialized = p.serialize();
    assert!(serialized.contains("border=1x2x3x4:aabbcc"), "{serialized}");
    assert_eq!(parse(&serialized), p);
}

#[test]
fn non_uniform_pad_round_trips_via_x_form() {
    let mut p = TransformParams::default();
    p.pad = Some(oximedia_image_transform::transform::Padding {
        top: 0.01,
        right: 0.02,
        bottom: 0.03,
        left: 0.04,
    });
    let serialized = p.serialize();
    assert!(
        serialized.contains("pad=0.01x0.02x0.03x0.04"),
        "{serialized}"
    );
    assert_eq!(parse(&serialized), p);
}

/// The legacy comma form must still parse via `parse_query_params`, where the
/// comma is not a top-level delimiter — guarding against regression.
#[test]
fn legacy_comma_geometry_still_parses_via_query_params() {
    use oximedia_image_transform::parser::parse_query_params;
    let p = parse_query_params("trim=10,5,10,5&border=1,2,3,4:00ff00")
        .unwrap_or_else(|e| panic!("query parse failed: {e:?}"));
    let trim = p.trim.expect("trim set");
    assert_eq!(
        (trim.top, trim.right, trim.bottom, trim.left),
        (10, 5, 10, 5)
    );
    let border = p.border.expect("border set");
    assert_eq!(
        (border.top, border.right, border.bottom, border.left),
        (1, 2, 3, 4)
    );
}

// ---------------------------------------------------------------------------
// Edge cases — defined behaviour, never a panic
// ---------------------------------------------------------------------------

#[test]
fn zero_width_parses_then_fails_validation_but_does_not_panic() {
    // Parser accepts `w=0` (it is a syntactically valid u32); the value is
    // rejected only at validation time, never by a panic.
    let p = parse("w=0");
    assert_eq!(p.width, Some(0));
    assert!(p.validate().is_err(), "zero width must fail validation");
    // It still round-trips structurally.
    assert_eq!(parse(&p.serialize()), p);
}

#[test]
fn negative_quality_is_a_parse_error_not_a_panic() {
    // `quality` is a u8; a negative literal cannot parse → defined Err.
    let result = parse_transform_string("q=-5");
    assert!(
        result.is_err(),
        "negative quality must be an Err, not a panic"
    );
}

#[test]
fn over_range_quality_is_a_parse_error_not_a_panic() {
    // 300 overflows u8 → defined Err.
    let result = parse_transform_string("q=300");
    assert!(result.is_err(), "out-of-u8-range quality must be an Err");
}

#[test]
fn in_range_but_invalid_quality_fails_validation_only() {
    // 0 fits in u8 (so it parses) but is out of the 1..=100 range → validation Err.
    let p = parse("q=0");
    assert_eq!(p.quality, 0);
    assert!(p.validate().is_err());
}

#[test]
fn unknown_format_string_is_a_parse_error_not_a_panic() {
    let result = parse_transform_string("format=bmp");
    assert!(
        result.is_err(),
        "unknown format must be an Err, not a panic"
    );
}

#[test]
fn non_numeric_dimension_is_a_parse_error_not_a_panic() {
    assert!(parse_transform_string("w=abc").is_err());
    assert!(parse_transform_string("h=ten").is_err());
}

#[test]
fn empty_and_bare_tokens_are_ignored_without_panic() {
    // Empty segments, bare keys, and trailing commas must all be tolerated.
    let p = parse(",,w=100,,bare_key,h=50,");
    assert_eq!(p.width, Some(100));
    assert_eq!(p.height, Some(50));
    assert_eq!(p.serialize(), "width=100,height=50");
}

#[test]
fn out_of_range_focal_point_is_a_parse_error_not_a_panic() {
    assert!(parse_transform_string("gravity=1.5x0.5").is_err());
}
