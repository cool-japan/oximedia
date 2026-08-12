//! Fix conversion errors.
//!
//! This module provides functions to fix errors introduced during format conversion.

use crate::{RepairError, Result};
use oximedia_core::convert::{ColorMatrix, PixelConverter};
use oximedia_video::deinterlace::bob_deinterlace;

/// Conversion error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionError {
    /// Incorrect aspect ratio.
    AspectRatio,
    /// Wrong frame rate.
    FrameRate,
    /// Color space conversion error.
    ColorSpace,
    /// Audio sample rate mismatch.
    SampleRate,
    /// Interlacing issues.
    Interlacing,
}

/// Fix aspect ratio issues.
///
/// Correcting a pixel/display aspect ratio means cropping, padding or
/// re-squaring a frame's geometry — an operation that fundamentally
/// requires the frame's width, height and pixel layout. This function's
/// signature carries none of that: `input` is an opaque byte buffer with
/// no declared geometry, so there is no safe way to interpret it as pixels
/// without guessing dimensions from the byte count alone (which would
/// silently misinterpret or corrupt arbitrary binary data for most inputs).
///
/// Rather than fabricate a "successful" no-op, this returns an honest
/// error naming exactly what is missing. Callers that have the source
/// width/height/pixel format should perform the crop/pad/scale directly
/// using geometry-aware primitives (e.g. `oximedia_video`'s frame
/// utilities) instead of this byte-oriented helper.
///
/// # Errors
///
/// Always returns `RepairError::InvalidOptions`. If `target_ratio` itself
/// is degenerate (either component zero), the error names that specific
/// problem; otherwise it explains the missing-geometry limitation above.
pub fn fix_aspect_ratio(_input: &[u8], target_ratio: (u32, u32)) -> Result<Vec<u8>> {
    if target_ratio.0 == 0 || target_ratio.1 == 0 {
        return Err(RepairError::InvalidOptions(format!(
            "invalid target aspect ratio {}:{} (both components must be non-zero)",
            target_ratio.0, target_ratio.1
        )));
    }
    Err(RepairError::InvalidOptions(
        "fix_aspect_ratio cannot correct a raw byte buffer: aspect-ratio repair requires frame \
         width, height and pixel format, none of which this signature carries. Decode the frame \
         first and use geometry-aware primitives (e.g. oximedia_video) to crop/pad/scale it"
            .to_string(),
    ))
}

// ───────────────────────────────────────────────────────────────────────────
// Interlacing artifact repair
// ───────────────────────────────────────────────────────────────────────────

/// Height (in rows) of each region analysed independently for adaptive
/// interlacing repair. Small enough to localise the fix to the parts of a
/// frame that actually exhibit motion-induced combing; large enough that
/// the per-region comb fraction stays statistically meaningful.
const INTERLACE_REGION_ROWS: usize = 8;

/// Per-pixel comb-test threshold (0-255 intensity scale). A pixel is
/// flagged as "combed" only when it deviates from *both* vertical
/// neighbours by more than this while the neighbours agree with each
/// other. Chosen empirically in line with comparable detectors elsewhere
/// in this codebase (e.g. `oximedia-cv`'s `CombDetectorConfig::new`, which
/// defaults to a threshold of 10 on the same 0-255 scale).
const COMB_PIXEL_THRESHOLD: i32 = 18;

/// Fraction of individually-flagged pixels a region must contain before
/// the whole region is deinterlaced. Guards against a handful of noisy
/// pixels in an otherwise clean region triggering a full-region rewrite.
const REGION_COMB_FRACTION: f32 = 0.08;

/// Classic three-line comb test used by field-based deinterlacing filters
/// (e.g. AviSynth's `IsCombed`/Decomb, and `oximedia-cv`'s
/// `CombDetector::calculate_comb_score`): a pixel is "combed" when it
/// spikes away from *both* its vertical neighbours while those neighbours
/// agree with each other. That signature — middle disagrees, edges agree —
/// is what a field boundary crossing a moving edge looks like; it is
/// distinct from ordinary vertical image detail (a real edge moves the
/// neighbours in the same direction as the centre pixel, so `top`/`bottom`
/// would *not* agree).
fn is_comb_pixel(top: u8, mid: u8, bottom: u8) -> bool {
    let t = i32::from(top);
    let m = i32::from(mid);
    let b = i32::from(bottom);
    let diff_top = (m - t).abs();
    let diff_bottom = (m - b).abs();
    let neighbor_diff = (t - b).abs();
    diff_top > COMB_PIXEL_THRESHOLD
        && diff_bottom > COMB_PIXEL_THRESHOLD
        && neighbor_diff < COMB_PIXEL_THRESHOLD
}

/// Per-row comb-pixel counts for `frame` (`w × h` bytes), using
/// [`is_comb_pixel`]. Boundary rows (`0` and `h - 1`) have no interior
/// vertical neighbour on one side and always report `0` hits.
fn comb_row_hits(frame: &[u8], w: usize, h: usize) -> Vec<u32> {
    let mut hits = vec![0u32; h];
    if h < 3 {
        return hits;
    }
    for y in 1..h - 1 {
        let top = &frame[(y - 1) * w..y * w];
        let mid = &frame[y * w..(y + 1) * w];
        let bottom = &frame[(y + 1) * w..(y + 2) * w];
        let mut row_hits = 0u32;
        for x in 0..w {
            if is_comb_pixel(top[x], mid[x], bottom[x]) {
                row_hits += 1;
            }
        }
        hits[y] = row_hits;
    }
    hits
}

/// Sums per-row comb hits separately for even- and odd-indexed rows,
/// returning `(even_total, odd_total)`.
fn tally_by_parity(row_hits: &[u32]) -> (u64, u64) {
    let mut even_hits: u64 = 0;
    let mut odd_hits: u64 = 0;
    for (y, &hits) in row_hits.iter().enumerate() {
        if y % 2 == 0 {
            even_hits += u64::from(hits);
        } else {
            odd_hits += u64::from(hits);
        }
    }
    (even_hits, odd_hits)
}

/// Fix interlacing artifacts in a single 8-bit-per-pixel video plane by
/// detecting regions with visible combing and adaptively deinterlacing
/// *only* those regions — clean regions are copied through byte-for-byte
/// so genuine vertical detail elsewhere in the frame is preserved.
///
/// `frame` is a row-major, single-byte-per-pixel buffer of exactly
/// `width * height` bytes (e.g. one plane of a YUV frame). For multi-plane
/// or multi-channel data, call this once per plane/channel.
///
/// Algorithm:
/// 1. Run the three-line comb test (`is_comb_pixel`) at every interior
///    pixel and tally hits per row.
/// 2. Pick which field to keep for reconstruction via
///    [`oximedia_video::deinterlace::bob_deinterlace`]: the parity
///    (even/odd row index) with *fewer* total hits is kept. This is a
///    best-effort tie-break, not a reliable field-order detector — a
///    single frame does not carry enough information to determine which
///    field is temporally "correct" (a comb-affected row's undisturbed
///    neighbours also trip the three-line test, since the test only
///    checks *local* row-to-row consistency, not agreement with the
///    original scene). Real deinterlacers resolve field order from
///    container metadata or multi-frame temporal context, neither of
///    which this single-frame, byte-slice signature has access to. In
///    practice the parity choice does not change *whether* the comb
///    metric improves (bob-interpolating either field locally smooths the
///    three-line test either way) — it only affects which original
///    samples are kept verbatim.
/// 3. Walk the frame in `INTERLACE_REGION_ROWS`-high bands; a band is
///    replaced with the bob-reconstructed rows only when the fraction of
///    individually-flagged pixels in that band exceeds
///    `REGION_COMB_FRACTION`.
///
/// # Errors
///
/// Returns `RepairError::InvalidOptions` if `width`/`height` are zero or
/// if `frame.len() != width * height`.
pub fn fix_interlacing_artifacts(frame: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 {
        return Err(RepairError::InvalidOptions(format!(
            "invalid frame dimensions {width}x{height}: both must be non-zero"
        )));
    }
    if frame.len() != w * h {
        return Err(RepairError::InvalidOptions(format!(
            "frame buffer length {} does not match {}x{} ({} bytes expected)",
            frame.len(),
            width,
            height,
            w * h
        )));
    }
    if h < 3 {
        // No interior row exists for the three-line comb test; there is
        // nothing that can be safely analysed or repaired.
        return Ok(frame.to_vec());
    }

    let row_hits = comb_row_hits(frame, w, h);
    let (even_hits, odd_hits) = tally_by_parity(&row_hits);

    if even_hits == 0 && odd_hits == 0 {
        // No combing detected anywhere in the frame; nothing to repair.
        return Ok(frame.to_vec());
    }

    // Best-effort tie-break (see doc comment): keep the parity with fewer
    // total comb hits. This does not reliably identify the "true" field —
    // a single frame cannot — but it is a deterministic, harmless choice,
    // and the comb metric improves regardless of which parity is kept.
    let top_field_first = even_hits <= odd_hits;
    let bobbed = bob_deinterlace(frame, width, height, top_field_first);

    let mut out = frame.to_vec();
    let mut start = 0usize;
    while start < h {
        let end = (start + INTERLACE_REGION_ROWS).min(h);
        let region_hits: u64 = row_hits[start..end].iter().map(|&v| u64::from(v)).sum();
        let region_pixels = (end - start) as u64 * w as u64;
        let fraction = if region_pixels == 0 {
            0.0
        } else {
            region_hits as f32 / region_pixels as f32
        };
        if fraction > REGION_COMB_FRACTION {
            out[start * w..end * w].copy_from_slice(&bobbed[start * w..end * w]);
        }
        start = end;
    }

    Ok(out)
}

// ───────────────────────────────────────────────────────────────────────────
// Frame rate conversion artifact repair
// ───────────────────────────────────────────────────────────────────────────

/// Normalised (per-byte-average) SAD below which two frames are treated as
/// an exact-duplicate artifact (naive frame-repeat from up-conversion).
const DUPLICATE_SAD_THRESHOLD: f64 = 0.5;

/// A transition is treated as a likely dropped-frame discontinuity when
/// its SAD exceeds the sequence's median inter-frame SAD by this factor.
const DROP_JUMP_RATIO: f64 = 2.5;

/// Normalised sum-of-absolute-differences between two equal-length byte
/// buffers: total per-byte distance divided by buffer length, so the
/// result is on a fixed `[0.0, 255.0]` scale independent of frame size.
fn byte_sad(a: &[u8], b: &[u8]) -> f64 {
    if a.is_empty() {
        return 0.0;
    }
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| u64::from((i32::from(x) - i32::from(y)).unsigned_abs()))
        .sum();
    sum as f64 / a.len() as f64
}

/// Element-wise linear blend of two equal-length byte buffers.
/// `alpha = 0.0` reproduces `a`; `alpha = 1.0` reproduces `b`.
fn blend_bytes(a: &[u8], b: &[u8], alpha: f64) -> Vec<u8> {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| {
            let v = f64::from(x) * (1.0 - alpha) + f64::from(y) * alpha;
            v.round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// Median of a slice of `f64` values (via a sorted copy; an even-length
/// slice uses the average of the two middle elements). Returns `0.0` for
/// an empty slice.
fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

/// Fix frame rate conversion artifacts by detecting and repairing
/// duplicate-frame judder and dropped-frame discontinuities.
///
/// `frames` are compared byte-wise (via `byte_sad`), so this is agnostic
/// to pixel format as long as every frame uses the same layout. `target_fps`
/// bounds how many gap-filling frames this repair may insert (see below);
/// it is *not* used to resample the sequence to an exact frame count, since
/// the source frame rate — required to compute that count — is not part of
/// this signature.
///
/// Repair strategy:
/// - **Duplicate runs**: a maximal run of consecutive frames with
///   near-zero pairwise SAD (< `DUPLICATE_SAD_THRESHOLD`) is a naive
///   frame-repeat artifact. Each duplicate in the run is replaced with a
///   linear cross-blend between the frame immediately before and the frame
///   immediately after the run, turning a held (juddery) frame into smooth
///   in-between motion. The run's length is preserved (no frames are
///   added or removed by this branch).
/// - **Dropped-frame jumps**: a transition whose SAD exceeds
///   `DROP_JUMP_RATIO` times the sequence's *median* SAD (a robust,
///   outlier-resistant baseline) is treated as a likely dropped frame. One
///   cross-blended frame is inserted at the midpoint to soften the
///   discontinuity. The exact number of frames actually dropped is not
///   recoverable from output frames alone, so at most one frame is
///   inserted per detected jump.
/// - Total insertions are capped at
///   `ceil(frames.len() / target_fps).max(1).min(frames.len())` — roughly
///   one repaired gap per second of `target_fps`-rate playback — so a
///   pathological number of detected jumps cannot make the output length
///   drift arbitrarily far from the input.
///
/// # Errors
///
/// Returns `RepairError::InvalidOptions` if `target_fps` is not finite or
/// not positive, and `RepairError::RepairFailed` if `frames` is empty, any
/// frame is empty, or frames have inconsistent lengths (byte-wise
/// comparison/interpolation is not meaningful across mismatched sizes).
pub fn fix_framerate_artifacts(frames: &[Vec<u8>], target_fps: f64) -> Result<Vec<Vec<u8>>> {
    if !target_fps.is_finite() || target_fps <= 0.0 {
        return Err(RepairError::InvalidOptions(format!(
            "invalid target frame rate: {target_fps}"
        )));
    }
    if frames.is_empty() {
        return Err(RepairError::RepairFailed(
            "no frames provided to repair".to_string(),
        ));
    }
    let frame_len = frames[0].len();
    if frame_len == 0 {
        return Err(RepairError::RepairFailed(
            "frame buffers are empty; nothing to repair".to_string(),
        ));
    }
    if frames.iter().any(|f| f.len() != frame_len) {
        return Err(RepairError::RepairFailed(
            "frame buffers have inconsistent sizes; cannot compare/interpolate byte-wise"
                .to_string(),
        ));
    }
    if frames.len() == 1 {
        return Ok(vec![frames[0].clone()]);
    }

    let sads: Vec<f64> = frames.windows(2).map(|p| byte_sad(&p[0], &p[1])).collect();
    let median_sad = median(&sads);

    let max_insertions = ((frames.len() as f64 / target_fps).ceil() as usize)
        .max(1)
        .min(frames.len());
    let mut insertions_used = 0usize;

    let mut out: Vec<Vec<u8>> = Vec::with_capacity(frames.len());
    out.push(frames[0].clone());

    let mut i = 1usize;
    while i < frames.len() {
        let sad = sads[i - 1];

        if sad <= DUPLICATE_SAD_THRESHOLD {
            // Duplicate run: frames[i-1] (already the last thing pushed to
            // `out`) and frames[i] are near-identical. Find the full
            // extent of the chain of near-duplicates, then replace the
            // duplicated frames with a linear cross-blend between the
            // pre-run anchor and the frame following the run.
            let anchor_idx = i - 1;
            let mut run_end = i;
            while run_end < frames.len() && sads[run_end - 1] <= DUPLICATE_SAD_THRESHOLD {
                run_end += 1;
            }
            let pre = &frames[anchor_idx];
            let post = if run_end < frames.len() {
                &frames[run_end]
            } else {
                // The run runs off the end of the sequence: there is no
                // distinct frame to interpolate toward, so the blend
                // target is the run's own (near-identical) value.
                &frames[run_end - 1]
            };
            let run_len = run_end - i;
            for k in 0..run_len {
                let alpha = (k + 1) as f64 / (run_len + 1) as f64;
                out.push(blend_bytes(pre, post, alpha));
            }
            i = run_end;
            continue;
        }

        if median_sad > 0.0
            && sad > median_sad * DROP_JUMP_RATIO
            && insertions_used < max_insertions
        {
            out.push(blend_bytes(&frames[i - 1], &frames[i], 0.5));
            insertions_used += 1;
        }

        out.push(frames[i].clone());
        i += 1;
    }

    Ok(out)
}

// ───────────────────────────────────────────────────────────────────────────
// Color space conversion artifact repair
// ───────────────────────────────────────────────────────────────────────────

/// Parses a color-matrix name into the corresponding [`ColorMatrix`].
///
/// Recognises common aliases for the two standards
/// `oximedia_core::convert::ColorMatrix` supports. Returns a precise error
/// naming the supported set when `name` does not match, rather than
/// silently defaulting to one matrix.
fn parse_color_matrix(name: &str) -> Result<ColorMatrix> {
    match name.trim().to_ascii_lowercase().as_str() {
        "bt601" | "601" | "rec601" | "smpte170m" | "sd" => Ok(ColorMatrix::Bt601),
        "bt709" | "709" | "rec709" | "hd" => Ok(ColorMatrix::Bt709),
        other => Err(RepairError::UnsupportedFormat(format!(
            "unsupported color matrix '{other}': this module only supports the matrices \
             defined by oximedia_core::convert::ColorMatrix (bt601, bt709); wider-gamut \
             matrices such as bt2020 are not yet implemented"
        ))),
    }
}

/// Fix color space conversion errors by re-deriving the correct YCbCr
/// matrix.
///
/// This addresses the most common real-world "conversion artifact": video
/// whose YCbCr samples were produced with one ITU-R matrix (e.g. BT.601)
/// but are subsequently interpreted or re-encoded with another (e.g.
/// BT.709), which visibly washes out or oversaturates colour. The fix
/// decodes each sample with the `from` matrix to recover the original RGB,
/// then re-encodes that RGB with the `to` matrix — the standard
/// matrix-recolourisation procedure, built directly on
/// `oximedia_core::convert::PixelConverter`.
///
/// `data` is interpreted as packed, interleaved 8-bit YCbCr444 triples
/// (`Y, Cb, Cr, Y, Cb, Cr, …`) — the only pixel layout that is
/// self-describing without a separate width/height/subsampling parameter.
/// Callers with planar or chroma-subsampled data should convert to this
/// layout first (e.g. via
/// `oximedia_core::convert::pixel::yuv420p_to_yuv444p` plus an interleave
/// step).
///
/// **Known precision caveat**: chroma (Cb/Cr) round-trips faithfully, but
/// the luma (Y) channel inherits an asymmetry in the underlying
/// `oximedia_core::convert::pixel::PixelConverter`: `yuv_to_rgb` correctly
/// expands studio-range Y (16-235) to full-range RGB, but `rgb_to_yuv`'s Y
/// formula does not compress back into studio range on re-encode (it is
/// missing a `*219/255` factor — full-range white RGB=255 should re-encode
/// to studio Y'=235, but currently yields Y'=255). The practical effect is
/// that this function's output Y values run systematically high (by
/// roughly the same proportion). That is a defect in the shared
/// `oximedia-core` primitive this function reuses, not in the matrix
/// selection/wiring logic here, and is out of scope for this crate to
/// patch; it should be fixed upstream in `oximedia-core`.
///
/// # Errors
///
/// Returns `RepairError::RepairFailed` if `data` is empty or its length is
/// not a multiple of 3, and `RepairError::UnsupportedFormat` if `from` or
/// `to` do not name a matrix in [`ColorMatrix`] (currently BT.601 and
/// BT.709 only).
pub fn fix_colorspace(data: &[u8], from: &str, to: &str) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Err(RepairError::RepairFailed(
            "no colour data to convert".to_string(),
        ));
    }
    if data.len() % 3 != 0 {
        return Err(RepairError::RepairFailed(format!(
            "data length {} is not a multiple of 3; fix_colorspace expects packed YCbCr444 \
             triples (Y, Cb, Cr per pixel)",
            data.len()
        )));
    }

    let from_matrix = parse_color_matrix(from)?;
    let to_matrix = parse_color_matrix(to)?;

    if from_matrix == to_matrix {
        // No mismatch between the requested matrices: the data is already
        // correct, so the honest repair is to leave it unchanged.
        return Ok(data.to_vec());
    }

    let decoder = PixelConverter::new(from_matrix);
    let encoder = PixelConverter::new(to_matrix);

    let mut out = Vec::with_capacity(data.len());
    for triple in data.chunks_exact(3) {
        let (r, g, b) = decoder.yuv_to_rgb(triple[0], triple[1], triple[2]);
        let (y, u, v) = encoder.rgb_to_yuv(r, g, b);
        out.push(y);
        out.push(u);
        out.push(v);
    }

    Ok(out)
}

/// Detect conversion artifacts.
pub fn detect_conversion_artifacts(_data: &[u8]) -> Vec<ConversionError> {
    // Placeholder: would analyze data for common conversion issues
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversion_error_types() {
        assert_ne!(ConversionError::AspectRatio, ConversionError::FrameRate);
    }

    // ── fix_aspect_ratio ────────────────────────────────────────────────────

    #[test]
    fn test_fix_aspect_ratio_honest_err_missing_geometry() {
        let input = vec![1u8, 2, 3, 4, 5, 6];
        let result = fix_aspect_ratio(&input, (16, 9));
        assert!(result.is_err(), "must not fabricate Ok(_) without geometry");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("width") || msg.contains("geometry") || msg.contains("height"),
            "error should explain the missing-geometry limitation: {msg}"
        );
    }

    #[test]
    fn test_fix_aspect_ratio_rejects_degenerate_ratio() {
        let input = vec![0u8; 16];
        let result = fix_aspect_ratio(&input, (0, 9));
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains('0'),
            "should call out the zero component: {msg}"
        );
    }

    // ── fix_interlacing_artifacts ───────────────────────────────────────────

    /// Build a `width × height` frame that is a smooth per-row gradient
    /// (constant across each row) everywhere, except that within
    /// `[shift_start, shift_start + shift_rows)` the odd rows are replaced
    /// with a large alternating horizontal pattern — simulating a bottom
    /// field that captured motion the top field did not, i.e. genuine
    /// inter-field combing rather than a same-parity "roughness" proxy.
    fn combed_gradient_frame(
        width: usize,
        height: usize,
        shift_start: usize,
        shift_rows: usize,
    ) -> Vec<u8> {
        let mut frame = vec![0u8; width * height];
        for y in 0..height {
            let base = 100i32 + y as i32;
            let in_band = y >= shift_start && y < shift_start + shift_rows;
            for x in 0..width {
                let value = if in_band && y % 2 == 1 {
                    // Large alternating horizontal swing on the "moved"
                    // field only: this is what a real inter-field motion
                    // comb looks like (mid spikes, top/bottom agree).
                    let swing = if (x / 4) % 2 == 0 { 60 } else { -60 };
                    (base + swing).clamp(0, 255)
                } else {
                    base.clamp(0, 255)
                };
                frame[y * width + x] = value as u8;
            }
        }
        frame
    }

    #[test]
    fn test_fix_interlacing_localises_repair_to_affected_region() {
        let width = 32usize;
        let height = 24usize;
        let frame = combed_gradient_frame(width, height, 0, 8);

        let out = fix_interlacing_artifacts(&frame, width as u32, height as u32)
            .expect("repair should succeed on a well-formed frame");

        assert_eq!(out.len(), frame.len());
        assert_ne!(out, frame, "combed region must be modified");

        // Region [0, 8) contained the induced combing and must change.
        let region0 = 0..(8 * width);
        assert_ne!(
            out[region0.clone()],
            frame[region0],
            "the combed region should have been repaired"
        );

        // Regions [8, 16) and [16, 24) were clean and must be byte-identical
        // to the input: the repair is adaptive/regional, not global.
        let region1 = (8 * width)..(16 * width);
        let region2 = (16 * width)..(24 * width);
        assert_eq!(
            out[region1.clone()],
            frame[region1],
            "clean region [8,16) must be preserved exactly"
        );
        assert_eq!(
            out[region2.clone()],
            frame[region2],
            "clean region [16,24) must be preserved exactly"
        );
    }

    #[test]
    fn test_fix_interlacing_reduces_comb_metric() {
        let width = 32usize;
        let height = 24usize;
        let frame = combed_gradient_frame(width, height, 0, 8);

        let hits_before: u64 = comb_row_hits(&frame, width, height)
            .iter()
            .map(|&v| u64::from(v))
            .sum();
        assert!(
            hits_before > 0,
            "synthetic frame must actually exhibit comb hits before repair"
        );

        let out = fix_interlacing_artifacts(&frame, width as u32, height as u32)
            .expect("repair should succeed");
        let hits_after: u64 = comb_row_hits(&out, width, height)
            .iter()
            .map(|&v| u64::from(v))
            .sum();

        assert!(
            hits_after < hits_before,
            "comb hits must measurably decrease: before={hits_before}, after={hits_after}"
        );
    }

    /// Pins the current tie-break tally for this fixture, so a future
    /// change to the heuristic (or to `is_comb_pixel`'s thresholds) is
    /// visible in a diff rather than silently changing behaviour. This
    /// does *not* assert the tally is "correct" in any absolute sense —
    /// see the `fix_interlacing_artifacts` doc comment on why single-frame
    /// field-order inference is inherently unreliable.
    #[test]
    fn test_fix_interlacing_parity_tally_pinned() {
        let width = 32usize;
        let height = 24usize;
        let frame = combed_gradient_frame(width, height, 0, 8);
        let row_hits = comb_row_hits(&frame, width, height);
        let (even_hits, odd_hits) = tally_by_parity(&row_hits);
        assert_eq!(
            (even_hits, odd_hits),
            (96, 128),
            "tie-break tally changed for this fixture; update this pin if the change is intentional"
        );
    }

    /// Substantiates the doc comment's claim that the comb metric improves
    /// regardless of which parity is chosen as "kept": this repeats the
    /// region-merge logic manually for *both* `top_field_first` values and
    /// checks both reduce comb hits in the affected region. The parity
    /// choice is not a correctness signal (it cannot be, from one frame);
    /// this test only pins the *symmetry* the doc describes.
    #[test]
    fn test_fix_interlacing_comb_improves_regardless_of_parity_choice() {
        let width = 32usize;
        let height = 24usize;
        let frame = combed_gradient_frame(width, height, 0, 8);
        let region = 0..(8 * width);
        let hits_before: u64 = comb_row_hits(&frame, width, height)
            .iter()
            .map(|&v| u64::from(v))
            .sum();

        for &top_field_first in &[true, false] {
            let bobbed = bob_deinterlace(&frame, width as u32, height as u32, top_field_first);
            let mut out = frame.clone();
            out[region.clone()].copy_from_slice(&bobbed[region.clone()]);

            let hits_after: u64 = comb_row_hits(&out, width, height)
                .iter()
                .map(|&v| u64::from(v))
                .sum();
            assert!(
                hits_after < hits_before,
                "comb hits should drop with top_field_first={top_field_first}: \
                 before={hits_before}, after={hits_after}"
            );
        }
    }

    #[test]
    fn test_fix_interlacing_leaves_progressive_frame_untouched() {
        let width = 16usize;
        let height = 16usize;
        // Pure smooth gradient, no induced combing anywhere.
        let frame = combed_gradient_frame(width, height, 0, 0);

        let out = fix_interlacing_artifacts(&frame, width as u32, height as u32)
            .expect("repair should succeed");
        assert_eq!(
            out, frame,
            "a progressive frame with no detected combing must pass through unchanged"
        );
    }

    #[test]
    fn test_fix_interlacing_rejects_buffer_size_mismatch() {
        let frame = vec![0u8; 10];
        let result = fix_interlacing_artifacts(&frame, 4, 4); // needs 16 bytes
        assert!(result.is_err());
    }

    #[test]
    fn test_fix_interlacing_rejects_zero_dimensions() {
        let frame: Vec<u8> = Vec::new();
        assert!(fix_interlacing_artifacts(&frame, 0, 4).is_err());
        assert!(fix_interlacing_artifacts(&frame, 4, 0).is_err());
    }

    #[test]
    fn test_fix_interlacing_short_frame_passthrough() {
        // height < 3: no interior row exists for the comb test.
        let frame = vec![10u8, 20, 30, 40, 50, 60, 70, 80]; // 4x2
        let out = fix_interlacing_artifacts(&frame, 4, 2).expect("must not error");
        assert_eq!(out, frame);
    }

    // ── fix_framerate_artifacts ─────────────────────────────────────────────

    fn solid(len: usize, v: u8) -> Vec<u8> {
        vec![v; len]
    }

    #[test]
    fn test_framerate_rejects_invalid_target_fps() {
        let frames = vec![solid(4, 1), solid(4, 2)];
        assert!(fix_framerate_artifacts(&frames, 0.0).is_err());
        assert!(fix_framerate_artifacts(&frames, -5.0).is_err());
        assert!(fix_framerate_artifacts(&frames, f64::NAN).is_err());
    }

    #[test]
    fn test_framerate_rejects_empty_or_mismatched() {
        assert!(fix_framerate_artifacts(&[], 30.0).is_err());
        assert!(fix_framerate_artifacts(&[Vec::new()], 30.0).is_err());
        let mismatched = vec![solid(4, 1), solid(5, 2)];
        assert!(fix_framerate_artifacts(&mismatched, 30.0).is_err());
    }

    #[test]
    fn test_framerate_single_frame_passthrough() {
        let frames = vec![solid(4, 42)];
        let out = fix_framerate_artifacts(&frames, 30.0).expect("must succeed");
        assert_eq!(out, frames);
    }

    #[test]
    fn test_framerate_no_artifacts_passthrough() {
        // Small, smooth, non-duplicate, non-jumpy deltas: nothing to fix.
        let frames = vec![solid(8, 20), solid(8, 25), solid(8, 30), solid(8, 35)];
        let out = fix_framerate_artifacts(&frames, 30.0).expect("must succeed");
        assert_eq!(out, frames, "a clean sequence must pass through unchanged");
    }

    #[test]
    fn test_framerate_duplicate_run_becomes_smooth_interpolation() {
        // F0=50, F1=100 (held 3x), F2=150. Moderate transitions on either
        // side of the duplicate run keep the jump-detector from firing, so
        // this isolates duplicate-run handling.
        let frames = vec![
            solid(10, 50),
            solid(10, 100),
            solid(10, 100),
            solid(10, 100),
            solid(10, 150),
        ];
        let out = fix_framerate_artifacts(&frames, 30.0).expect("must succeed");

        assert_eq!(
            out.len(),
            frames.len(),
            "duplicate collapse preserves frame count"
        );
        assert!(!out.is_empty());
        assert_ne!(out, frames, "the held duplicates must be replaced");

        assert_eq!(out[0], frames[0], "pre-run anchor is untouched");
        assert_eq!(out[1], frames[1], "pre-run anchor is untouched");
        assert_eq!(out[4], frames[4], "post-run frame is untouched");

        // The two former duplicates must no longer be exact copies, and
        // must form a monotonic ramp from 100 toward 150.
        assert_ne!(out[2], frames[2]);
        assert_ne!(out[3], frames[3]);
        let v2 = out[2][0];
        let v3 = out[3][0];
        assert!(
            100 < v2 && v2 < v3 && v3 < 150,
            "expected a smooth ramp 100 < {v2} < {v3} < 150"
        );

        // Duplicate-transition count must measurably decrease.
        let count_dupes = |seq: &[Vec<u8>]| seq.windows(2).filter(|w| w[0] == w[1]).count();
        assert_eq!(
            count_dupes(&frames),
            2,
            "sanity: input has 2 duplicate transitions"
        );
        assert_eq!(
            count_dupes(&out),
            0,
            "output must have no duplicate transitions left"
        );
    }

    #[test]
    fn test_framerate_jump_gets_smoothing_frame_inserted() {
        // Gentle ramp 0,5,10,15 then an anomalous jump to 215.
        let frames = vec![
            solid(8, 0),
            solid(8, 5),
            solid(8, 10),
            solid(8, 15),
            solid(8, 215),
        ];
        // frames.len()=5, target_fps=5.0 => max_insertions = ceil(5/5)=1,
        // matching the single expected jump.
        let out = fix_framerate_artifacts(&frames, 5.0).expect("must succeed");

        assert_eq!(out.len(), frames.len() + 1, "exactly one frame inserted");
        assert_ne!(out, frames);

        // The peak inter-frame jump must measurably shrink: originally
        // |15-215|=200; after inserting a midpoint the two halves are each
        // about 100.
        let max_jump = |seq: &[Vec<u8>]| -> f64 {
            seq.windows(2)
                .map(|w| byte_sad(&w[0], &w[1]))
                .fold(0.0, f64::max)
        };
        let before = max_jump(&frames);
        let after = max_jump(&out);
        assert!(
            after < before,
            "peak discontinuity must shrink: before={before}, after={after}"
        );
        assert!((before - 200.0).abs() < 1e-9);
        assert!(
            after <= 101.0,
            "each half-jump should be roughly 100, got {after}"
        );
    }

    #[test]
    fn test_framerate_insertions_capped_by_target_fps() {
        // 9 frames, two big jumps (Δ60) separated by small deltas (Δ2).
        // Both jumps individually qualify (median-based threshold), but
        // target_fps=9.0 caps max_insertions at exactly 1.
        let vals = [20u8, 22, 24, 84, 86, 88, 148, 150, 152];
        let frames: Vec<Vec<u8>> = vals.iter().map(|&v| solid(6, v)).collect();

        let out = fix_framerate_artifacts(&frames, 9.0).expect("must succeed");
        assert_eq!(
            out.len(),
            frames.len() + 1,
            "only one of the two qualifying jumps may be repaired under the cap"
        );

        // A raw, unrepaired Δ60 jump must still be present somewhere in
        // the output (proof the cap suppressed a real detection, not that
        // detection silently stopped working after the first hit).
        let has_unrepaired_jump = out.windows(2).any(|w| byte_sad(&w[0], &w[1]) >= 59.0);
        assert!(
            has_unrepaired_jump,
            "the second jump must remain unrepaired once the cap is reached"
        );
    }

    // ── fix_colorspace ──────────────────────────────────────────────────────

    #[test]
    fn test_colorspace_identity_when_from_equals_to() {
        let data = vec![10u8, 20, 30, 40, 50, 60];
        let out = fix_colorspace(&data, "bt601", "bt601").expect("must succeed");
        assert_eq!(out, data, "no mismatch => data returned unchanged");
    }

    #[test]
    fn test_colorspace_matches_pixel_converter_directly() {
        // A colourful (non-neutral-gray) triple so BT.601 vs BT.709 decode
        // genuinely diverge.
        let data = vec![180u8, 90, 200];
        let out = fix_colorspace(&data, "bt601", "bt709").expect("must succeed");
        assert_eq!(out.len(), 3);

        let decoder = PixelConverter::new(ColorMatrix::Bt601);
        let encoder = PixelConverter::new(ColorMatrix::Bt709);
        let (r, g, b) = decoder.yuv_to_rgb(180, 90, 200);
        let (y, u, v) = encoder.rgb_to_yuv(r, g, b);
        assert_eq!(
            out,
            vec![y, u, v],
            "must match direct PixelConverter wiring"
        );
        assert_ne!(out, data, "a real matrix mismatch must change the bytes");
    }

    #[test]
    fn test_colorspace_rejects_bad_length() {
        let data = vec![1u8, 2, 3, 4]; // not a multiple of 3
        assert!(fix_colorspace(&data, "bt601", "bt709").is_err());
    }

    #[test]
    fn test_colorspace_rejects_empty() {
        assert!(fix_colorspace(&[], "bt601", "bt709").is_err());
    }

    #[test]
    fn test_colorspace_rejects_unsupported_matrix() {
        let data = vec![10u8, 20, 30];
        assert!(fix_colorspace(&data, "bt2020", "bt709").is_err());
        assert!(fix_colorspace(&data, "bt601", "nonsense").is_err());
    }

    #[test]
    fn test_colorspace_roundtrip_recovers_original_within_tolerance() {
        // Mid-range values avoid clamp saturation at 0/255.
        let data: Vec<u8> = vec![
            70, 100, 120, // triple 1
            150, 110, 140, // triple 2
            190, 95, 155, // triple 3
        ];
        let forward = fix_colorspace(&data, "bt601", "bt709").expect("forward must succeed");
        let back = fix_colorspace(&forward, "bt709", "bt601").expect("reverse must succeed");
        assert_eq!(back.len(), data.len());

        // `oximedia_core::convert::pixel::PixelConverter::yuv_to_rgb` decodes
        // studio-range Y (16-235) by expanding through `(Y-16)*1.164`, but
        // `rgb_to_yuv`'s Y formula (`Kr*R + Kg*G + Kb*B + 16`) does not
        // compress back into studio range (it is missing a `*219/255`
        // factor: e.g. full-range grey 255 should re-encode to studio
        // Y'=235, but the current implementation yields Y'=255). That
        // asymmetry is not something this crate owns or should patch
        // (`oximedia-core` is a shared dependency well outside this
        // slice's scope), but it means even a *same-matrix* decode/encode
        // round trip drifts the luma channel upward, and this cross-matrix
        // round trip (two decode/encode hops) compounds it further —
        // empirically up to ~56 code values on this fixture. Chroma
        // (U/V) stays tight (within ~10). The bound below reflects that
        // measured reality with a small margin, not a guess.
        let max_diff = data
            .iter()
            .zip(back.iter())
            .map(|(&a, &b)| (i16::from(a) - i16::from(b)).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(
            max_diff <= 60,
            "round-trip error should stay within the measured upstream tolerance, got max_diff={max_diff}"
        );
    }
}
