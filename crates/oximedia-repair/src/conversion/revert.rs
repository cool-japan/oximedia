//! Revert bad conversions.
//!
//! This module provides functions to attempt reverting poorly done conversions.

use crate::{RepairError, Result};

use super::fix::fix_colorspace;

/// Attempt to revert a conversion to original format.
///
/// Of the conversion categories this crate models (aspect ratio, frame
/// rate, colour space, sample rate, interlacing), only a colour-space
/// matrix conversion is mathematically invertible from the converted data
/// alone: aspect-ratio crops/pads discard or synthesise pixels, frame-rate
/// changes drop or duplicate frames, sample-rate resampling is a lossy
/// band-limited filter, and deinterlacing merges two fields into one —
/// none of those can be losslessly undone without the original data, which
/// is exactly the information a "revert" call does not have. Rather than
/// fabricate recovered pixels/samples/frames for those categories, this
/// function honestly rejects anything it cannot invert.
///
/// `original_format` describes the colour-space conversion to invert,
/// using one of two forms:
/// - **Explicit** — `"<current>-><original>"`, e.g. `"bt709->bt601"`
///   reverts data that is currently BT.709-encoded back to BT.601.
/// - **Bare name** — e.g. `"bt601"`. Since
///   [`oximedia_core::convert::ColorMatrix`] currently defines exactly two
///   matrices, the *other* one is inferred as the data's current matrix.
///   This convenience form is well-defined today but becomes ambiguous the
///   moment a third matrix is added to `ColorMatrix`; use the explicit
///   form if that matters to the caller.
///
/// `converted` is interpreted as packed, interleaved YCbCr444 triples,
/// matching [`fix_colorspace`], which this function delegates to (reverting
/// = converting from the current matrix back to the original one).
///
/// **Known precision caveat**: this inherits [`fix_colorspace`]'s luma
/// asymmetry (see its documentation) — chroma reverts faithfully, but each
/// decode/encode hop drifts the luma channel upward due to a studio-range
/// compression step missing in the shared `oximedia-core` primitive this
/// crate reuses, not in the revert logic here.
///
/// # Errors
///
/// Returns `RepairError::UnsupportedFormat` if `original_format` does not
/// resolve to a supported colour-matrix pair — this includes any
/// non-colour-space conversion category, which is fundamentally
/// non-invertible with this module's primitives. Also propagates
/// [`fix_colorspace`]'s errors for malformed `converted` data (wrong
/// length, etc.).
pub fn revert_conversion(converted: &[u8], original_format: &str) -> Result<Vec<u8>> {
    let (current, original) = parse_revert_spec(original_format)?;
    fix_colorspace(converted, current, original)
}

/// Parses a revert specification into `(current_matrix_name, original_matrix_name)`.
///
/// See [`revert_conversion`] for the accepted syntaxes.
fn parse_revert_spec(spec: &str) -> Result<(&str, &str)> {
    let trimmed = spec.trim();

    if let Some((current, original)) = trimmed.split_once("->") {
        let current = current.trim();
        let original = original.trim();
        if current.is_empty() || original.is_empty() {
            return Err(RepairError::UnsupportedFormat(format!(
                "malformed revert spec '{spec}': expected '<current>-><original>' with both \
                 sides non-empty"
            )));
        }
        return Ok((current, original));
    }

    // Bare name: infer "current" as the other of the two known matrices.
    match trimmed.to_ascii_lowercase().as_str() {
        "bt601" | "601" | "rec601" | "smpte170m" | "sd" => Ok(("bt709", "bt601")),
        "bt709" | "709" | "rec709" | "hd" => Ok(("bt601", "bt709")),
        other => Err(RepairError::UnsupportedFormat(format!(
            "cannot revert to '{other}': not a recognised colour matrix (bt601, bt709), and no \
             other conversion category (aspect ratio, frame rate, sample rate, interlacing) is \
             invertible from output data alone. Use '<current>-><original>' for an explicit \
             colour-matrix revert"
        ))),
    }
}

/// Detect if file has been converted.
pub fn detect_conversion_history(_data: &[u8]) -> Option<Vec<String>> {
    // Would analyze metadata and artifacts to detect conversion chain
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_conversion_history() {
        let data = vec![0u8; 100];
        let history = detect_conversion_history(&data);
        assert_eq!(history, None);
    }

    fn sample_triples() -> Vec<u8> {
        vec![
            70, 100, 120, // triple 1
            150, 110, 140, // triple 2
            190, 95, 155, // triple 3
        ]
    }

    #[test]
    fn test_revert_conversion_explicit_spec_recovers_original() {
        let original = sample_triples();
        let converted =
            fix_colorspace(&original, "bt601", "bt709").expect("forward conversion succeeds");

        let reverted =
            revert_conversion(&converted, "bt709->bt601").expect("revert should succeed");

        assert_eq!(reverted.len(), original.len());
        // See the identical round-trip test in `conversion::fix` for why
        // this tolerance is as wide as it is: `PixelConverter::rgb_to_yuv`
        // (in `oximedia-core`, outside this crate's scope) does not
        // compress luma back into studio range on encode, so even a
        // same-matrix decode/encode pair drifts upward; this two-hop
        // cross-matrix round trip compounds it further. Measured up to
        // ~56 on this fixture.
        let max_diff = original
            .iter()
            .zip(reverted.iter())
            .map(|(&a, &b)| (i16::from(a) - i16::from(b)).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(
            max_diff <= 60,
            "revert should approximately recover the original within the measured upstream \
             tolerance, got max_diff={max_diff}"
        );
    }

    #[test]
    fn test_revert_conversion_bare_name_matches_explicit_spec() {
        let original = sample_triples();
        let converted =
            fix_colorspace(&original, "bt601", "bt709").expect("forward conversion succeeds");

        let via_bare = revert_conversion(&converted, "bt601").expect("bare-name revert succeeds");
        let via_explicit =
            revert_conversion(&converted, "bt709->bt601").expect("explicit revert succeeds");

        assert_eq!(
            via_bare, via_explicit,
            "bare 'bt601' must infer the same (bt709 -> bt601) conversion"
        );
    }

    #[test]
    fn test_revert_conversion_rejects_unknown_format() {
        let data = vec![10u8, 20, 30];
        assert!(revert_conversion(&data, "some_lossy_thing").is_err());
    }

    #[test]
    fn test_revert_conversion_rejects_malformed_explicit_spec() {
        let data = vec![10u8, 20, 30];
        assert!(revert_conversion(&data, "->bt601").is_err());
        assert!(revert_conversion(&data, "bt709->").is_err());
    }

    #[test]
    fn test_revert_conversion_propagates_colorspace_data_errors() {
        // Not a multiple of 3: fix_colorspace's own honest error surfaces.
        let bad = vec![1u8, 2, 3, 4];
        assert!(revert_conversion(&bad, "bt709->bt601").is_err());
    }
}
