// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Parser fuzz / robustness tests.
//!
//! Feed malformed, boundary, and adversarial inputs to the public parse
//! functions and verify that no panics occur and results are safe (either
//! `Ok` with sane values, or `Err` — never UB or a panic).

use oximedia_image_transform::parser::{parse_cdn_url, parse_query_params, parse_transform_string};

// ── Empty / blank inputs ────────────────────────────────────────────────────

#[test]
fn test_fuzz_transform_string_empty() {
    // Empty string must succeed (returns default params).
    let result = parse_transform_string("");
    assert!(result.is_ok(), "empty string must return Ok");
    assert!(result.expect("ok").is_identity());
}

#[test]
fn test_fuzz_query_params_empty() {
    let result = parse_query_params("");
    assert!(result.is_ok());
}

#[test]
fn test_fuzz_cdn_url_empty() {
    // Empty CDN URL has no prefix — must be Err, not a panic.
    let result = parse_cdn_url("");
    assert!(result.is_err());
}

// ── Missing values ──────────────────────────────────────────────────────────

#[test]
fn test_fuzz_width_empty_value() {
    // "w=" with no value → invalid parameter, not a panic.
    let result = parse_transform_string("w=");
    assert!(result.is_err(), "empty width value must be Err");
}

#[test]
fn test_fuzz_height_empty_value() {
    let result = parse_transform_string("h=");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_quality_empty_value() {
    let result = parse_transform_string("q=");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_both_dims_empty() {
    // "w=,h=" — both empty; first error encountered must be returned.
    let result = parse_transform_string("w=,h=");
    assert!(result.is_err());
}

// ── Integer overflow / very large numbers ───────────────────────────────────

#[test]
fn test_fuzz_width_overflow_u32() {
    // 99999999999 exceeds u32::MAX — parse_u32 must fail.
    let result = parse_transform_string("w=99999999999");
    assert!(result.is_err(), "overflowing width must be Err");
}

#[test]
fn test_fuzz_height_overflow_u32() {
    let result = parse_transform_string("h=99999999999");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_large_valid_dimension() {
    // A large but valid u32 that fits the type (validation is separate).
    let result = parse_transform_string("w=4294967295");
    // May be Ok or Err depending on MAX_DIMENSION validation, but no panic.
    let _ = result;
}

#[test]
fn test_fuzz_quality_overflow_u8() {
    // 256 exceeds u8::MAX.
    let result = parse_transform_string("q=256");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_negative_dimension() {
    // u32 parser cannot represent negatives.
    let result = parse_transform_string("w=-1");
    assert!(result.is_err());
}

// ── Shell injection / path traversal ────────────────────────────────────────

#[test]
fn test_fuzz_injection_shell_commands() {
    // Semicolons and shell meta-characters must not cause a panic.
    let result = parse_transform_string("w=1&h=1;rm -rf /");
    // Either Ok (unknown params silently dropped) or Err — no panic.
    let _ = result;
}

#[test]
fn test_fuzz_injection_semicolon_in_value() {
    let result = parse_transform_string("format=webp;DROP TABLE images");
    // "webp;DROP TABLE images" is not a known format → Err.
    assert!(result.is_err());
}

#[test]
fn test_fuzz_cdn_url_path_traversal() {
    let result = parse_cdn_url("/cdn-cgi/image/w=100/../../etc/passwd");
    match result {
        Ok(req) => {
            // If it succeeds, the path must not contain "..".
            assert!(
                !req.source_path.contains(".."),
                "sanitised path must not contain '..'"
            );
        }
        Err(_) => {
            // Rejection is also acceptable.
        }
    }
}

#[test]
fn test_fuzz_cdn_url_null_byte() {
    // Null bytes in paths must not panic.
    let result = parse_cdn_url("/cdn-cgi/image/w=100/photo\0.jpg");
    let _ = result;
}

// ── Unicode and non-ASCII ────────────────────────────────────────────────────

#[test]
fn test_fuzz_unicode_in_width() {
    let result = parse_transform_string("w=💀");
    assert!(result.is_err(), "unicode width must be Err");
}

#[test]
fn test_fuzz_unicode_in_format() {
    let result = parse_transform_string("f=🖼️");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_unicode_in_gravity() {
    let result = parse_transform_string("g=中央");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_query_unicode_value() {
    let result = parse_query_params("w=1&h=💀");
    assert!(result.is_err());
}

// ── Extreme lengths ─────────────────────────────────────────────────────────

#[test]
fn test_fuzz_very_long_key() {
    let long_key = "x".repeat(100_000);
    let input = format!("{long_key}=100");
    // Unknown key — must succeed silently (forward-compat unknown key ignore).
    let result = parse_transform_string(&input);
    assert!(result.is_ok(), "very long unknown key must not panic");
}

#[test]
fn test_fuzz_very_long_value() {
    let long_val = "a".repeat(100_000);
    let input = format!("format={long_val}");
    // Not a valid format string → Err, not a panic.
    assert!(parse_transform_string(&input).is_err());
}

#[test]
fn test_fuzz_many_pairs() {
    // 10 000 unknown pairs — must not exhaust stack or heap.
    let pairs: Vec<String> = (0..10_000).map(|i| format!("unknown_{i}={i}")).collect();
    let input = pairs.join(",");
    let result = parse_transform_string(&input);
    assert!(result.is_ok(), "many unknown pairs must succeed");
}

// ── Misformed pair syntax ────────────────────────────────────────────────────

#[test]
fn test_fuzz_bare_key_no_equals() {
    // A bare token without '=' is silently ignored.
    let result = parse_transform_string("width");
    assert!(result.is_ok());
}

#[test]
fn test_fuzz_multiple_equals() {
    // "w=8=0" — split_once gives key="w", value="8=0"; parse_u32("8=0") fails.
    let result = parse_transform_string("w=8=0");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_only_commas() {
    let result = parse_transform_string(",,,,,");
    assert!(result.is_ok(), "only commas must parse as empty/identity");
}

#[test]
fn test_fuzz_only_ampersands() {
    let result = parse_query_params("&&&&&");
    assert!(
        result.is_ok(),
        "only ampersands must parse as empty/identity"
    );
}

// ── CDN URL edge cases ───────────────────────────────────────────────────────

#[test]
fn test_fuzz_cdn_url_no_image_path() {
    // Missing image path after transform spec.
    let result = parse_cdn_url("/cdn-cgi/image/w=100");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_cdn_url_wrong_prefix() {
    let result = parse_cdn_url("/images/w=100/photo.jpg");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_cdn_url_double_slash_transform() {
    // Double slash means empty transform string → default params.
    let result = parse_cdn_url("/cdn-cgi/image//photo.jpg");
    assert!(result.is_ok(), "empty transform section must succeed");
}

// ── Boundary values ──────────────────────────────────────────────────────────

#[test]
fn test_fuzz_max_valid_dimension_boundary() {
    // MAX_DIMENSION = 12000; 12000 must be accepted by the parser
    // (validation checks for > MAX_DIMENSION, not >= MAX_DIMENSION).
    let result = parse_transform_string("w=12000");
    assert!(result.is_ok(), "12000 is the maximum valid dimension");
    let params = result.expect("must be ok");
    assert_eq!(params.width, Some(12_000));
}

#[test]
fn test_fuzz_quality_boundary_1() {
    let result = parse_transform_string("q=1");
    assert!(result.is_ok());
    assert_eq!(result.expect("ok").quality, 1);
}

#[test]
fn test_fuzz_quality_boundary_100() {
    let result = parse_transform_string("q=100");
    assert!(result.is_ok());
    assert_eq!(result.expect("ok").quality, 100);
}

#[test]
fn test_fuzz_dpr_string() {
    // DPR must be a float.
    let result = parse_transform_string("dpr=notanumber");
    assert!(result.is_err());
}

#[test]
fn test_fuzz_blur_negative() {
    // Negative float parses fine but validate() would reject it;
    // parse alone should not panic.
    let result = parse_transform_string("blur=-1.5");
    // The parser accepts the float; validation is a separate step.
    let _ = result;
}
