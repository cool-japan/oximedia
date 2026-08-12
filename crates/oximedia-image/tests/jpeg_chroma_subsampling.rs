//! Baseline JPEG decode conformance, checked against libjpeg-turbo.
//!
//! Every fixture under `tests/fixtures/jpeg` is a JPEG plus the reference
//! decode Pillow/libjpeg-turbo produced from that exact file, stored
//! losslessly as PNG. `generate_fixtures.py` in that directory regenerates
//! both halves.
//!
//! The bug these tests exist for: the decoder used to read the per-component
//! sampling factors out of `SOF0` and throw them away, decoding every scan as
//! though it were 4:4:4. Chroma-subsampled files — which is to say almost
//! every photograph — came back as `Ok` with a 0→255 gradient in place of the
//! picture. For a redaction tool that is the worst possible failure: the
//! caller has no way to tell that the image it is about to publish is not the
//! image it was handed.
//!
//! So the bar here is agreement with a reference decoder, per pixel, not
//! "looks plausible". Chroma upsampling filters legitimately differ between
//! implementations, hence a tolerance rather than equality — but a tolerance
//! far tighter than any structural error could hide in.

use oximedia_image::jpeg::JpegDecoder;
use oximedia_image::png::PngDecoder;

/// Mean absolute per-sample difference from the reference decode.
///
/// Observed worst case across these fixtures is 0.20; the old decoder scored
/// 76–83 on the subsampled ones and 9–27 on 4:4:4.
const MAX_MEAN_ABS_DIFF: f64 = 2.0;

/// Largest single-sample difference from the reference decode.
///
/// Observed worst case is 3, from the float IDCT and from the ±1 rounding
/// libjpeg alternates between the two halves of a fancy-upsampled pair.
const MAX_ABS_DIFF: u32 = 32;

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jpeg")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

/// Difference statistics between two equally sized sample buffers.
fn diff_stats(actual: &[u8], expected: &[u8]) -> (f64, u32) {
    let mut sum = 0f64;
    let mut worst = 0u32;
    for (a, b) in actual.iter().zip(expected.iter()) {
        let d = (i32::from(*a) - i32::from(*b)).unsigned_abs();
        sum += f64::from(d);
        worst = worst.max(d);
    }
    (sum / actual.len() as f64, worst)
}

/// Decode `<name>.jpg` and assert it matches `<name>.ref.png` per pixel.
fn assert_matches_reference(name: &str) {
    let decoded = JpegDecoder::new()
        .decode(&fixture(&format!("{name}.jpg")))
        .unwrap_or_else(|e| panic!("{name}: decode failed: {e}"));
    let reference = PngDecoder::new()
        .decode(&fixture(&format!("{name}.ref.png")))
        .unwrap_or_else(|e| panic!("{name}: reference PNG decode failed: {e}"));

    assert_eq!(decoded.width, reference.width, "{name}: width");
    assert_eq!(decoded.height, reference.height, "{name}: height");
    assert_eq!(
        decoded.pixels.len(),
        reference.pixels.len(),
        "{name}: sample count ({} components decoded)",
        decoded.components
    );

    let (mean, worst) = diff_stats(&decoded.pixels, &reference.pixels);
    assert!(
        mean < MAX_MEAN_ABS_DIFF,
        "{name}: mean abs diff vs libjpeg is {mean:.3}, limit {MAX_MEAN_ABS_DIFF}"
    );
    assert!(
        worst < MAX_ABS_DIFF,
        "{name}: max abs diff vs libjpeg is {worst}, limit {MAX_ABS_DIFF}"
    );
}

#[test]
fn decode_444_matches_libjpeg() {
    for name in [
        "rgb_33x17_444_q70",
        "rgb_33x17_444_q90",
        "rgb_64x64_444_q70",
        "rgb_64x64_444_q90",
        "rgb_101x53_444_q70",
        "rgb_101x53_444_q90",
    ] {
        assert_matches_reference(name);
    }
}

#[test]
fn decode_422_matches_libjpeg() {
    // 2x1 sampling: chroma upsampled horizontally only.
    for name in [
        "rgb_33x17_422_q70",
        "rgb_33x17_422_q90",
        "rgb_64x64_422_q70",
        "rgb_64x64_422_q90",
        "rgb_101x53_422_q70",
        "rgb_101x53_422_q90",
    ] {
        assert_matches_reference(name);
    }
}

#[test]
fn decode_420_matches_libjpeg() {
    // 2x2 sampling: what essentially every phone camera writes.
    for name in [
        "rgb_33x17_420_q70",
        "rgb_33x17_420_q90",
        "rgb_64x64_420_q70",
        "rgb_64x64_420_q90",
        "rgb_101x53_420_q70",
        "rgb_101x53_420_q90",
    ] {
        assert_matches_reference(name);
    }
}

#[test]
fn decode_440_matches_libjpeg() {
    // 1x2 sampling: chroma upsampled vertically only. The mirror image of
    // 4:2:2, and the case a decoder that hard-codes "chroma is half width"
    // gets wrong.
    assert_matches_reference("rgb_64x64_440_q90");
}

#[test]
fn decode_grayscale_matches_libjpeg() {
    // Single-component frames use a non-interleaved scan: one 8x8 block per
    // MCU over the component's own block grid, with no MCU padding.
    assert_matches_reference("gray_64x64_q90");
}

#[test]
fn decode_with_restart_intervals_matches_libjpeg() {
    // DRI + RSTn every MCU row: the decoder must byte-align, consume the
    // marker and reset the DC predictors at each interval boundary.
    assert_matches_reference("rgb_101x53_420_q80_rst");
}

#[test]
fn decode_photo_matches_libjpeg_block_means() {
    // A full-size 4:2:0 photograph. Storing libjpeg's 800x600 decode verbatim
    // would cost 1.4 MB, so the reference is its 8x8 block means (100x75) —
    // still enough to catch any structural error, since a mis-assembled MCU
    // grid moves whole blocks.
    let name = "photo_800x600_420_q85";
    let decoded = JpegDecoder::new()
        .decode(&fixture(&format!("{name}.jpg")))
        .unwrap_or_else(|e| panic!("{name}: decode failed: {e}"));
    assert_eq!(decoded.width, 800);
    assert_eq!(decoded.height, 600);
    assert_eq!(decoded.components, 3);

    let reference = PngDecoder::new()
        .decode(&fixture(&format!("{name}.ref8.png")))
        .unwrap_or_else(|e| panic!("{name}: reference PNG decode failed: {e}"));
    assert_eq!(reference.width, 100);
    assert_eq!(reference.height, 75);

    let mut means = Vec::with_capacity(100 * 75 * 3);
    for block_y in 0..75usize {
        for block_x in 0..100usize {
            for channel in 0..3usize {
                let mut sum = 0u32;
                for y in 0..8usize {
                    for x in 0..8usize {
                        let row = block_y * 8 + y;
                        let col = block_x * 8 + x;
                        sum += u32::from(decoded.pixels[(row * 800 + col) * 3 + channel]);
                    }
                }
                means.push(((f64::from(sum) / 64.0).round()).clamp(0.0, 255.0) as u8);
            }
        }
    }

    assert_eq!(means.len(), reference.pixels.len());
    let (mean, worst) = diff_stats(&means, &reference.pixels);
    assert!(
        mean < MAX_MEAN_ABS_DIFF,
        "{name}: block-mean abs diff vs libjpeg is {mean:.3}"
    );
    assert!(
        worst < MAX_ABS_DIFF,
        "{name}: worst block-mean diff vs libjpeg is {worst}"
    );
}

#[test]
fn subsampled_decode_is_never_the_old_gradient_ramp() {
    // The decoder used to answer `Ok` with `((i / components) % 256) as u8`
    // whenever the real decode failed. Nothing may ever produce that buffer
    // again — an error is the only honest answer to a file we cannot read.
    for name in [
        "rgb_64x64_420_q70",
        "rgb_64x64_422_q70",
        "rgb_64x64_440_q90",
        "rgb_101x53_420_q90",
    ] {
        let decoded = JpegDecoder::new()
            .decode(&fixture(&format!("{name}.jpg")))
            .unwrap_or_else(|e| panic!("{name}: decode failed: {e}"));
        let components = decoded.components as usize;
        let ramp: Vec<u8> = (0..decoded.pixels.len())
            .map(|i| ((i / components) % 256) as u8)
            .collect();
        assert_ne!(
            decoded.pixels, ramp,
            "{name}: decoded to the placeholder ramp"
        );
    }
}

#[test]
fn rejects_progressive_jpeg() {
    // SOF2 shares DQT/DHT/SOS syntax with baseline but codes coefficients
    // across several scans; decoding its entropy data as baseline yields
    // noise, so it must be refused by name.
    let err = JpegDecoder::new()
        .decode(&fixture("rgb_64x64_420_q90_progressive.jpg"))
        .expect_err("progressive JPEG must be rejected, not decoded");
    let message = err.to_string();
    assert!(
        message.contains("progressive"),
        "error should name the coding process, got: {message}"
    );
}

#[test]
fn rejects_sampling_ratio_it_cannot_upsample() {
    // Force Hmax = 4 with 1x1 chroma, i.e. a 4:1:1-style ratio this decoder
    // does not implement. It must say so rather than guess.
    let mut data = fixture("rgb_64x64_444_q90.jpg");
    let sof = find_marker(&data, 0xC0).expect("fixture has an SOF0 segment");
    data[sof + 11] = 0x41; // component 0: H=4, V=1
    let err = JpegDecoder::new()
        .decode(&data)
        .expect_err("unsupported sampling ratio must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("upsampling ratio"),
        "error should explain the ratio, got: {message}"
    );
}

#[test]
fn rejects_truncated_entropy_data() {
    // A scan that ends early must not be completed out of padding bits.
    for name in ["rgb_64x64_420_q70", "rgb_101x53_422_q90", "gray_64x64_q90"] {
        let data = fixture(&format!("{name}.jpg"));
        let sos = find_marker(&data, 0xDA).expect("fixture has an SOS segment");
        let cut = sos + (data.len() - sos) / 2;
        assert!(
            JpegDecoder::new().decode(&data[..cut]).is_err(),
            "{name}: truncated scan decoded to Ok"
        );
    }
}

#[test]
fn rejects_frame_without_a_scan() {
    let data = fixture("rgb_64x64_420_q70.jpg");
    let sos = find_marker(&data, 0xDA).expect("fixture has an SOS segment");
    let mut headers_only = data[..sos].to_vec();
    headers_only.extend_from_slice(&[0xFF, 0xD9]); // EOI
    assert!(JpegDecoder::new().decode(&headers_only).is_err());
}

/// Offset of the first `0xFF <marker>` segment with the given marker byte.
fn find_marker(data: &[u8], marker: u8) -> Option<usize> {
    (0..data.len().saturating_sub(1)).find(|&i| data[i] == 0xFF && data[i + 1] == marker)
}
