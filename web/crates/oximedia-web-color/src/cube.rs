// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! String-based `.cube` (Adobe/Resolve/ffmpeg) 3D LUT parser and exporter.
//!
//! Data lines are in **R-fastest** lattice order, matching
//! `crates/oximedia-lut/src/formats/cube.rs` and every mainstream `.cube`
//! consumer (the B-fastest order written by `oximedia-colormgmt`'s
//! `GradingLut3D::export` is a known cross-compat bug and is not copied).
//!
//! The parser is hostile-input safe — browser users upload these files:
//! * `LUT_3D_SIZE` must be present, unique, in `2..=129`, and precede data;
//! * the data-line count must match `size³` exactly (excess aborts early,
//!   bounding memory);
//! * every value must be a finite `f32` (NaN/±Inf/overflow rejected);
//! * `DOMAIN_MIN`/`DOMAIN_MAX` are honoured and validated (`max > min`);
//! * `TITLE` round-trips; `LUT_1D_SIZE` is rejected with a clear error;
//! * CRLF and arbitrary garbage never panic.

use crate::error::ColorError;
use crate::lut::{Lut3d, MAX_LUT_SIZE, MIN_LUT_SIZE};

/// Returns the remainder after `keyword` if `line` starts with the keyword
/// followed by whitespace (or end of line).
fn keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

/// Parses exactly `N` finite `f32` tokens from `rest`.
fn parse_floats<const N: usize>(
    rest: &str,
    line: usize,
    what: &str,
) -> Result<[f32; N], ColorError> {
    let mut out = [0.0f32; N];
    let mut it = rest.split_whitespace();
    for (i, slot) in out.iter_mut().enumerate() {
        let tok = it.next().ok_or_else(|| ColorError::CubeParse {
            line,
            message: format!("{what}: expected {N} values, found {i}"),
        })?;
        let v: f32 = tok.parse().map_err(|_| ColorError::CubeParse {
            line,
            message: format!("{what}: {tok:?} is not a number"),
        })?;
        if !v.is_finite() {
            return Err(ColorError::CubeParse {
                line,
                message: format!("{what}: {tok:?} is not finite"),
            });
        }
        *slot = v;
    }
    if it.next().is_some() {
        return Err(ColorError::CubeParse {
            line,
            message: format!("{what}: more than {N} values on the line"),
        });
    }
    Ok(out)
}

/// Parses `.cube` text into a [`Lut3d`].
///
/// # Errors
/// Returns a descriptive [`ColorError`] for any malformed, truncated,
/// non-finite, oversized or 1D input. Never panics.
pub fn parse_cube(text: &str) -> Result<Lut3d, ColorError> {
    let mut title: Option<String> = None;
    let mut size: Option<usize> = None;
    let mut domain_min = [0.0f32; 3];
    let mut domain_max = [1.0f32; 3];
    let mut data: Vec<f32> = Vec::new();
    let mut expected = 0usize;

    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(rest) = keyword(line, "TITLE") {
            let t = rest.trim();
            let t = t.strip_prefix('"').unwrap_or(t);
            let t = t.strip_suffix('"').unwrap_or(t);
            title = Some(t.to_string());
            continue;
        }
        if keyword(line, "LUT_1D_SIZE").is_some() {
            return Err(ColorError::CubeIs1d);
        }
        if let Some(rest) = keyword(line, "LUT_3D_SIZE") {
            if size.is_some() {
                return Err(ColorError::CubeParse {
                    line: line_no,
                    message: "duplicate LUT_3D_SIZE".to_string(),
                });
            }
            let mut it = rest.split_whitespace();
            let tok = it.next().ok_or_else(|| ColorError::CubeParse {
                line: line_no,
                message: "LUT_3D_SIZE: missing value".to_string(),
            })?;
            if it.next().is_some() {
                return Err(ColorError::CubeParse {
                    line: line_no,
                    message: "LUT_3D_SIZE: trailing tokens".to_string(),
                });
            }
            let s: usize = tok.parse().map_err(|_| ColorError::CubeParse {
                line: line_no,
                message: format!("LUT_3D_SIZE: {tok:?} is not a positive integer"),
            })?;
            if !(MIN_LUT_SIZE..=MAX_LUT_SIZE).contains(&s) {
                return Err(ColorError::LutSize { size: s });
            }
            expected = s * s * s * 3;
            data.reserve_exact(expected);
            size = Some(s);
            continue;
        }
        if let Some(rest) = keyword(line, "DOMAIN_MIN") {
            domain_min = parse_floats::<3>(rest, line_no, "DOMAIN_MIN")?;
            continue;
        }
        if let Some(rest) = keyword(line, "DOMAIN_MAX") {
            domain_max = parse_floats::<3>(rest, line_no, "DOMAIN_MAX")?;
            continue;
        }

        // Anything else must be an RGB data line.
        if size.is_none() {
            return Err(ColorError::CubeParse {
                line: line_no,
                message: "data (or unknown keyword) before LUT_3D_SIZE".to_string(),
            });
        }
        if data.len() + 3 > expected {
            return Err(ColorError::CubeParse {
                line: line_no,
                message: "more data lines than LUT_3D_SIZE^3".to_string(),
            });
        }
        let rgb = parse_floats::<3>(line, line_no, "RGB data")?;
        data.extend_from_slice(&rgb);
    }

    let s = size.ok_or_else(|| ColorError::CubeParse {
        line: 0,
        message: "missing LUT_3D_SIZE".to_string(),
    })?;
    if data.len() != expected {
        return Err(ColorError::CubeParse {
            line: 0,
            message: format!(
                "expected {} data lines (size {s}^3), found {}",
                expected / 3,
                data.len() / 3
            ),
        });
    }
    Lut3d::with_domain(s, data, domain_min, domain_max, title)
}

/// Exports a [`Lut3d`] to `.cube` text (R-fastest data order, `TITLE` and
/// `DOMAIN_MIN`/`DOMAIN_MAX` preserved).
///
/// Values are written with Rust's shortest-round-trip float formatting, so
/// `parse_cube(export_cube(lut))` reproduces the lattice bit-exactly.
#[must_use]
pub fn export_cube(lut: &Lut3d) -> String {
    let size = lut.size();
    let mut out = String::with_capacity(size * size * size * 24 + 128);

    let title = lut.title().unwrap_or("OxiMedia LUT");
    let sanitized: String = title
        .chars()
        .map(|c| if c == '"' || c == '\n' || c == '\r' { '_' } else { c })
        .collect();
    out.push_str("TITLE \"");
    out.push_str(&sanitized);
    out.push_str("\"\n");

    out.push_str(&format!("LUT_3D_SIZE {size}\n"));
    let dmin = lut.domain_min();
    let dmax = lut.domain_max();
    out.push_str(&format!("DOMAIN_MIN {} {} {}\n", dmin[0], dmin[1], dmin[2]));
    out.push_str(&format!("DOMAIN_MAX {} {} {}\n", dmax[0], dmax[1], dmax[2]));
    out.push('\n');

    // Internal storage is already R-fastest, so a sequential dump is exactly
    // the .cube data order.
    for rgb in lut.data().chunks_exact(3) {
        out.push_str(&format!("{} {} {}\n", rgb[0], rgb[1], rgb[2]));
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lut::LutInterp;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    fn tiny_cube(size: usize) -> String {
        let lut = Lut3d::from_fn(size, |r, g, b| [r, g, b]).expect("identity");
        export_cube(&lut)
    }

    // ── Round trips ──────────────────────────────────────────────────────────

    #[test]
    fn export_parse_round_trip_is_bit_exact() {
        let mut lut = Lut3d::from_fn(7, |r, g, b| {
            [r * 0.9 + 0.03, (g + b) * 0.5, (b * 0.7).sqrt()]
        })
        .expect("from_fn");
        lut.set_title(Some("Round Trip".to_string()));
        let text = export_cube(&lut);
        let parsed = parse_cube(&text).expect("parse");
        assert_eq!(parsed.size(), 7);
        assert_eq!(parsed.title(), Some("Round Trip"));
        for (a, b) in lut.data().iter().zip(parsed.data()) {
            assert!(approx(*a, *b, 1e-6), "{a} vs {b}");
        }
    }

    #[test]
    fn domain_round_trips() {
        let lut = Lut3d::with_domain(
            2,
            vec![0.25f32; 24],
            [-0.5, 0.0, 0.25],
            [1.5, 2.0, 1.0],
            Some("Domain".to_string()),
        )
        .expect("lut");
        let parsed = parse_cube(&export_cube(&lut)).expect("parse");
        assert_eq!(parsed.domain_min(), [-0.5, 0.0, 0.25]);
        assert_eq!(parsed.domain_max(), [1.5, 2.0, 1.0]);
    }

    #[test]
    fn r_fastest_order_matches_ffmpeg_convention() {
        // Hand-written 2×2×2 cube: first data line is lattice (0,0,0), the
        // SECOND is (r=1, g=0, b=0) — red advances fastest.
        let text = "LUT_3D_SIZE 2\n\
                    0 0 0\n\
                    1 0 0\n\
                    0 1 0\n\
                    1 1 0\n\
                    0 0 1\n\
                    1 0 1\n\
                    0 1 1\n\
                    1 1 1\n";
        let lut = parse_cube(text).expect("parse");
        // This is the identity mapping; sampling red must return red.
        let out = lut.sample(LutInterp::Tetrahedral, 1.0, 0.0, 0.0);
        assert!(approx(out[0], 1.0, 1e-6) && approx(out[1], 0.0, 1e-6));
        let out = lut.sample(LutInterp::Trilinear, 0.0, 0.0, 1.0);
        assert!(approx(out[2], 1.0, 1e-6) && approx(out[0], 0.0, 1e-6));
    }

    #[test]
    fn crlf_input_parses() {
        let text = tiny_cube(2).replace('\n', "\r\n");
        let lut = parse_cube(&text).expect("CRLF parse");
        assert_eq!(lut.size(), 2);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let text = format!("# a comment\n\n# another\n{}\n# trailing\n", tiny_cube(2));
        assert!(parse_cube(&text).is_ok());
    }

    #[test]
    fn title_with_spaces_round_trips() {
        let text = "TITLE \"My Fancy Grade v2\"\nLUT_3D_SIZE 2\n".to_string()
            + &"0.5 0.5 0.5\n".repeat(8);
        let lut = parse_cube(&text).expect("parse");
        assert_eq!(lut.title(), Some("My Fancy Grade v2"));
        let round = parse_cube(&export_cube(&lut)).expect("round");
        assert_eq!(round.title(), Some("My Fancy Grade v2"));
    }

    // ── Torture cases (must error, never panic) ─────────────────────────────

    #[test]
    fn torture_empty_input() {
        assert!(matches!(parse_cube(""), Err(ColorError::CubeParse { .. })));
        assert!(parse_cube("   \n\n  ").is_err());
    }

    #[test]
    fn torture_truncated_data() {
        let full = tiny_cube(2);
        // Drop the last data line.
        let truncated: Vec<&str> = full.lines().collect();
        let truncated = truncated[..truncated.len() - 1].join("\n");
        assert!(matches!(
            parse_cube(&truncated),
            Err(ColorError::CubeParse { .. })
        ));
    }

    #[test]
    fn torture_excess_data() {
        let text = format!("{}0.1 0.2 0.3\n", tiny_cube(2));
        let err = parse_cube(&text);
        assert!(matches!(err, Err(ColorError::CubeParse { .. })), "{err:?}");
    }

    #[test]
    fn torture_nan_and_inf_values() {
        for bad in ["NaN", "nan", "inf", "-inf", "Infinity", "1e9999"] {
            let text = format!(
                "LUT_3D_SIZE 2\n{} 0 0\n{}",
                bad,
                "0 0 0\n".repeat(7)
            );
            assert!(
                parse_cube(&text).is_err(),
                "value {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn torture_size_10000() {
        assert!(matches!(
            parse_cube("LUT_3D_SIZE 10000\n"),
            Err(ColorError::LutSize { size: 10000 })
        ));
    }

    #[test]
    fn torture_size_edge_cases() {
        assert!(matches!(parse_cube("LUT_3D_SIZE 1\n"), Err(ColorError::LutSize { .. })));
        assert!(matches!(parse_cube("LUT_3D_SIZE 0\n"), Err(ColorError::LutSize { .. })));
        assert!(parse_cube("LUT_3D_SIZE -3\n").is_err());
        assert!(parse_cube("LUT_3D_SIZE 999999999999999999999999\n").is_err());
        assert!(parse_cube("LUT_3D_SIZE two\n").is_err());
        assert!(parse_cube("LUT_3D_SIZE\n").is_err());
        assert!(parse_cube("LUT_3D_SIZE 2 2\n").is_err());
    }

    #[test]
    fn torture_binary_garbage() {
        let garbage = "\u{0}\u{1}\u{2}ÿþPK\u{3}\u{4}\nLUT?!\n\u{7f}\u{7f}\u{7f}";
        assert!(parse_cube(garbage).is_err());
        let garbage2 = "%PDF-1.4\n%\u{e2}\u{e3}\u{cf}\u{d3}\n1 0 obj\n";
        assert!(parse_cube(garbage2).is_err());
    }

    #[test]
    fn torture_1d_lut_rejected_with_clear_error() {
        let text = "LUT_1D_SIZE 256\n0 0 0\n1 1 1\n";
        assert!(matches!(parse_cube(text), Err(ColorError::CubeIs1d)));
        let msg = ColorError::CubeIs1d.to_string();
        assert!(msg.contains("1D"), "error must mention 1D: {msg}");
    }

    #[test]
    fn torture_duplicate_size() {
        let text = "LUT_3D_SIZE 2\nLUT_3D_SIZE 3\n";
        assert!(matches!(parse_cube(text), Err(ColorError::CubeParse { .. })));
    }

    #[test]
    fn torture_data_before_size() {
        let text = "0.5 0.5 0.5\nLUT_3D_SIZE 2\n";
        assert!(matches!(parse_cube(text), Err(ColorError::CubeParse { .. })));
    }

    #[test]
    fn torture_wrong_token_counts() {
        assert!(parse_cube("LUT_3D_SIZE 2\n0.5 0.5\n").is_err());
        assert!(parse_cube("LUT_3D_SIZE 2\n0.5 0.5 0.5 0.5\n").is_err());
    }

    #[test]
    fn torture_degenerate_domain() {
        let text = format!(
            "LUT_3D_SIZE 2\nDOMAIN_MIN 1 1 1\nDOMAIN_MAX 0 0 0\n{}",
            "0 0 0\n".repeat(8)
        );
        assert!(matches!(parse_cube(&text), Err(ColorError::LutDomain)));
    }

    #[test]
    fn torture_unknown_keyword_lines() {
        // Unknown alphabetic keywords cannot be data lines; strict rejection.
        let text = format!("LUT_3D_SIZE 2\nLUT_3D_INPUT_RANGE 0 1\n{}", "0 0 0\n".repeat(8));
        assert!(parse_cube(&text).is_err());
    }

    #[test]
    fn torture_error_line_numbers_are_reported() {
        let text = "LUT_3D_SIZE 2\n0 0 0\nbogus line here\n";
        match parse_cube(text) {
            Err(ColorError::CubeParse { line, .. }) => assert_eq!(line, 3),
            other => panic!("expected CubeParse with line number, got {other:?}"),
        }
    }
}
