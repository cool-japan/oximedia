//! Byte-exact golden regression tests for [`ScaleFilter`].
//!
//! The scaler is performance-sensitive (it dominates any reframe chain), so it is a magnet for
//! optimisation work: loop reordering, coefficient-table flattening, row parallelism. None of
//! those are allowed to change a single output byte.
//!
//! This test pins the BLAKE3 digest of the complete output of the filter -- every plane, every
//! byte -- for a fixed procedural 4:2:0 input frame across every [`ScaleAlgorithm`] and a set of
//! geometries chosen to exercise upscaling, antialiased downscaling, odd/non-even dimensions
//! (which make the chroma planes `div_ceil`-rounded and therefore not an exact half of luma), and
//! the `antialias = false` path.
//!
//! If a digest changes, the numerics changed. Either the change is a bug, or it is intentional
//! and the new digests must be re-pinned deliberately (with a documented reason).

use blake3::Hasher;
use oximedia_codec::{Plane, VideoFrame};
use oximedia_core::PixelFormat;
use oximedia_graph::filters::video::{ScaleAlgorithm, ScaleConfig, ScaleFilter};
use oximedia_graph::{FilterFrame, Node, NodeId};

/// One golden case: a source geometry, a target geometry and an antialias setting.
struct Geometry {
    label: &'static str,
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
    antialias: bool,
}

const GEOMETRIES: &[Geometry] = &[
    // Antialiased downscale: `filter_scale` widens the kernel support.
    Geometry {
        label: "down_160x120_to_96x72_aa",
        src_width: 160,
        src_height: 120,
        dst_width: 96,
        dst_height: 72,
        antialias: true,
    },
    // Downscale with antialiasing disabled: narrow kernel, different tap counts.
    Geometry {
        label: "down_160x120_to_96x72_noaa",
        src_width: 160,
        src_height: 120,
        dst_width: 96,
        dst_height: 72,
        antialias: false,
    },
    // Pure upscale: `filter_scale` stays 1.0 on both axes.
    Geometry {
        label: "up_96x72_to_224x168",
        src_width: 96,
        src_height: 72,
        dst_width: 224,
        dst_height: 168,
        antialias: true,
    },
    // Odd source dimensions: chroma is 31x19 (not 30x18), and both axes clamp at the edges.
    Geometry {
        label: "odd_61x37_to_128x96",
        src_width: 61,
        src_height: 37,
        dst_width: 128,
        dst_height: 96,
        antialias: true,
    },
    // Mixed axis directions: horizontal downscale, vertical upscale (the reframe shape).
    Geometry {
        label: "mixed_152x90_to_90x160",
        src_width: 152,
        src_height: 90,
        dst_width: 90,
        dst_height: 160,
        antialias: true,
    },
    // Large enough that both luma passes cross the rayon threshold while both chroma passes stay
    // serial -- so a single frame exercises the parallel and the serial resampler in one go.
    Geometry {
        label: "parallel_288x216_to_320x256",
        src_width: 288,
        src_height: 216,
        dst_width: 320,
        dst_height: 256,
        antialias: true,
    },
];

const ALGORITHMS: &[(&str, ScaleAlgorithm)] = &[
    ("nearest", ScaleAlgorithm::Nearest),
    ("bilinear", ScaleAlgorithm::Bilinear),
    ("bicubic", ScaleAlgorithm::Bicubic),
    ("catmullrom", ScaleAlgorithm::CatmullRom),
    ("lanczos2", ScaleAlgorithm::Lanczos2),
    ("lanczos3", ScaleAlgorithm::Lanczos3),
    ("lanczos4", ScaleAlgorithm::Lanczos4),
];

/// Pinned digests. Regenerate ONLY when a numeric change is intentional -- a failing run prints
/// the full replacement table.
const GOLDEN: &[(&str, &str)] = &[
    // Re-pinned deliberately: `Nearest` is now a true point sampler, so `antialias` no longer
    // widens its footprint into a box average on a downscale. This digest is consequently
    // identical to `down_160x120_to_96x72_noaa/nearest` below -- the same source pixels, chosen by
    // the same rule, with and without antialiasing.
    (
        "down_160x120_to_96x72_aa/nearest",
        "b3df4d56ff919fb5c1342464f8b74015368ec0d65931b049fa0c48395e311da9",
    ),
    (
        "down_160x120_to_96x72_aa/bilinear",
        "2a2679beac30732bbb177fe923ff72e6878afb80686c3d59f0adcfae3843f201",
    ),
    (
        "down_160x120_to_96x72_aa/bicubic",
        "439e263ee59013fd8265e4c3367d412d40da588291eda8e0a575a99315379494",
    ),
    (
        "down_160x120_to_96x72_aa/catmullrom",
        "4fa92a79faf7154f587bc1d5cab36eed05a69a6e2d4d273b856cde0f8b51503f",
    ),
    (
        "down_160x120_to_96x72_aa/lanczos2",
        "49ac1cabca78afa49e381bdd0e0b35c432535bfc169983e528a2aa6e14447005",
    ),
    (
        "down_160x120_to_96x72_aa/lanczos3",
        "d49d0531b80423ecdad868b62a8821fedd5f664a5222a190eb87a82b52386f69",
    ),
    (
        "down_160x120_to_96x72_aa/lanczos4",
        "9736e37c05911e489faed10863f56adfd22d6bb7c1377a395668ef930ea8a4d4",
    ),
    (
        "down_160x120_to_96x72_noaa/nearest",
        "b3df4d56ff919fb5c1342464f8b74015368ec0d65931b049fa0c48395e311da9",
    ),
    (
        "down_160x120_to_96x72_noaa/bilinear",
        "f5aa469b192e0edfc7e1661871d3d6b046b12555138ffb3280ea1f0b4766ea6c",
    ),
    (
        "down_160x120_to_96x72_noaa/bicubic",
        "fea190387dfe666017f3630a69411fccf0f2b027122d49f0da6ea05cc09039c4",
    ),
    (
        "down_160x120_to_96x72_noaa/catmullrom",
        "14cb97145609b4e9f509989ada8dff2da6d0b41925ba26a7e675716a47863f7e",
    ),
    (
        "down_160x120_to_96x72_noaa/lanczos2",
        "386888b16a6ba6c1670c370311880f67042ee7627a70b7930e8fdb9c5f380092",
    ),
    (
        "down_160x120_to_96x72_noaa/lanczos3",
        "21d522a3dde0b5502ce3f16adb889ad62585b5faf9b85d27169af7e1bec15006",
    ),
    (
        "down_160x120_to_96x72_noaa/lanczos4",
        "941584817b59ccab33f2685726fe1e8f4e206ff763c621556aedd44c4d85cc8d",
    ),
    (
        "up_96x72_to_224x168/nearest",
        "587afe479e6985c4128a76f91af14d49ece353a7461f6aa9b5727a809226d736",
    ),
    (
        "up_96x72_to_224x168/bilinear",
        "2b1718f299a326e8e188b9aa79e979b8f6841f358657a787688442df35d250f1",
    ),
    (
        "up_96x72_to_224x168/bicubic",
        "b6de7d613746ec727806c8f15c97016655f4c4bc7edac50b472eecd0721b47b6",
    ),
    (
        "up_96x72_to_224x168/catmullrom",
        "b0655bbd1d01bc190350a1799994138912f69443f82a4a6b88143e8c24b3cba2",
    ),
    (
        "up_96x72_to_224x168/lanczos2",
        "7354b94815ac6cd8dedb6e408b51fa9b1bbf8bba937ba3afcd75d0183f3ae9c4",
    ),
    (
        "up_96x72_to_224x168/lanczos3",
        "4fd28501a784a87952612c3154f76b60e9667385127d8217f87f942746d1de5e",
    ),
    (
        "up_96x72_to_224x168/lanczos4",
        "a0df569864b1e25b97df5e9c1c5db70c2da8540e084da43b0f06c1d2c0dffe29",
    ),
    (
        "odd_61x37_to_128x96/nearest",
        "d02402c6443fa95776094e6c93eff4ae9a1cd52d9d3d62edf10ea0d6a01a5cb3",
    ),
    (
        "odd_61x37_to_128x96/bilinear",
        "bcc0afbea8dbb43e7cd98a60fcdeff1e3be03c22a10ac0de33ec9347c4419b48",
    ),
    (
        "odd_61x37_to_128x96/bicubic",
        "02a27d0ec1cb1cf3d4c931516d581dcbfe93ca3cd639118dc0ce961e745f4044",
    ),
    (
        "odd_61x37_to_128x96/catmullrom",
        "0f555a7b1b4de0a279ddfbfe4c4cd844ca3483fb047515be0c3471512b732c58",
    ),
    (
        "odd_61x37_to_128x96/lanczos2",
        "a82de617db1712e84005f8e2bc687caa78e8f9528b474cb727f4226e59f82065",
    ),
    (
        "odd_61x37_to_128x96/lanczos3",
        "364f8659cde1b1c8d9af4c1f8a6e7bdf1b759cdec969a5fc26bd42c16700c312",
    ),
    (
        "odd_61x37_to_128x96/lanczos4",
        "5753fbdf20d06f9302382fa3d357a4eb19c07e76e74d312e862b3ab46bbe84f4",
    ),
    // Re-pinned deliberately: the horizontal axis of this case is an antialiased downscale
    // (152 -> 90), which used to box-average `Nearest` over the widened footprint instead of
    // point-sampling it. The vertical axis (90 -> 160) is an upscale and is unchanged.
    (
        "mixed_152x90_to_90x160/nearest",
        "7c9fdffa283ff20f7508a0e58ab5137fdec134af5375ad4aaa4466652c677738",
    ),
    (
        "mixed_152x90_to_90x160/bilinear",
        "0b021dbc35f6fbbc530e7701e6256a99fe788cbc8581a46983925e311694ce80",
    ),
    (
        "mixed_152x90_to_90x160/bicubic",
        "ad30adf04f474b301ba96723f9347b9bd86e8bc46e04e7f6f6d773f78e9d4f52",
    ),
    (
        "mixed_152x90_to_90x160/catmullrom",
        "3e058cce8bc66a46b708c9ba883d9560e2d5badd5325f7bab9e7d6ce643e08a4",
    ),
    (
        "mixed_152x90_to_90x160/lanczos2",
        "57f9a4d486dbce6bc4faabc867238b74f3e0c48713c93b98e0d6aa5b87c494f9",
    ),
    (
        "mixed_152x90_to_90x160/lanczos3",
        "83e66f56eac3078bd448867a98a08f98a01f75b765f4858145b07ec7c87700c1",
    ),
    (
        "mixed_152x90_to_90x160/lanczos4",
        "3d20e79290db034df3cc0adb9d30045a5a8dfe1f5c0f6901c18e9531a772745f",
    ),
    (
        "parallel_288x216_to_320x256/nearest",
        "ec102d3ad1cb554833309fe5b1c0e468928f763111d145eaa95631237dcaf47e",
    ),
    (
        "parallel_288x216_to_320x256/bilinear",
        "bb41a7ecdf5884a52357a431bc037d946fea3d312c598d8429e9e96e40b55790",
    ),
    (
        "parallel_288x216_to_320x256/bicubic",
        "a1c2981897a88e7a491f873139606568799e48bdf9fd59da9464f10c35bb04af",
    ),
    (
        "parallel_288x216_to_320x256/catmullrom",
        "588343ee88d4fada21bc3d8e43ecbb01f8124641caf4db3a07544f3703d69344",
    ),
    (
        "parallel_288x216_to_320x256/lanczos2",
        "109be12eb6395546fe58c3bc76318cf938bbbb4d2faa6451a6188b58d800adcd",
    ),
    (
        "parallel_288x216_to_320x256/lanczos3",
        "a9a8b22d16f82face04b0d3fe3309bbfd4a8bd69da5e025d2fc0b01a07aa0560",
    ),
    (
        "parallel_288x216_to_320x256/lanczos4",
        "fb4f24a3cacbea6949582051aee33030474fa6498ea57020413502b76b2c3c8d",
    ),
];

/// Build a deterministic 4:2:0 frame with structure in *every* plane.
///
/// The pattern combines a two-axis gradient (smooth, exercises interpolation) with a checkerboard
/// (hard edges, exercises ringing/clamping) and a per-plane bias so the U and V planes never end
/// up byte-identical to each other or to luma.
fn build_frame(width: u32, height: u32) -> VideoFrame {
    let mut frame = VideoFrame::new(PixelFormat::Yuv420p, width, height);
    frame.allocate();

    let plane_count = frame.planes.len();
    for index in 0..plane_count {
        let (plane_width, plane_height) = frame.plane_dimensions(index);
        let span_x = u32::from(plane_width > 1) * (plane_width.saturating_sub(1)) + 1;
        let span_y = u32::from(plane_height > 1) * (plane_height.saturating_sub(1)) + 1;
        let bias = (index as u32) * 37;
        let checker_size = 5 + index as u32 * 2;

        let Some(plane) = frame.planes.get_mut(index) else {
            continue;
        };
        for y in 0..plane_height {
            for x in 0..plane_width {
                let gradient = (x * 200 / span_x + y * 200 / span_y) / 2;
                let checker = if ((x / checker_size) + (y / checker_size)) % 2 == 0 {
                    48
                } else {
                    0
                };
                // A small non-separable ripple keeps the pattern from being exactly reproducible
                // by a separable filter, so a broken pass cannot accidentally match.
                let ripple = ((x * 7 + y * 13) % 17) * 3;
                let value = (gradient + checker + ripple + bias) % 256;
                let offset = y as usize * plane.stride + x as usize;
                if let Some(slot) = plane.data.get_mut(offset) {
                    *slot = value as u8;
                }
            }
        }
    }

    frame
}

/// Hash every plane of a frame, including its geometry, so a shape change is also a digest change.
fn digest(frame: &VideoFrame) -> String {
    let mut hasher = Hasher::new();
    hasher.update(&frame.width.to_le_bytes());
    hasher.update(&frame.height.to_le_bytes());
    hasher.update(&(frame.planes.len() as u32).to_le_bytes());
    for plane in &frame.planes {
        hasher.update(&(plane.stride as u32).to_le_bytes());
        hasher.update(&(plane.data.len() as u32).to_le_bytes());
        hasher.update(&plane.data);
    }
    hasher.finalize().to_hex().to_string()
}

/// Run one (geometry, algorithm) case through the public filter API.
fn run_case(geometry: &Geometry, algorithm: ScaleAlgorithm) -> VideoFrame {
    let config = ScaleConfig::new(geometry.dst_width, geometry.dst_height)
        .with_algorithm(algorithm)
        .with_antialias(geometry.antialias);
    let mut filter = ScaleFilter::new(NodeId(0), "golden_scale", config);
    let input = build_frame(geometry.src_width, geometry.src_height);

    let output = filter
        .process(Some(FilterFrame::Video(input)))
        .expect("scale filter must accept a video frame")
        .expect("scale filter must emit a frame for a video input");

    match output {
        FilterFrame::Video(frame) => frame,
        FilterFrame::Audio(_) => panic!("scale filter must not emit audio"),
    }
}

/// Compute every case, in a stable order.
fn compute_all() -> Vec<(String, String)> {
    let mut results = Vec::with_capacity(GEOMETRIES.len() * ALGORITHMS.len());
    for geometry in GEOMETRIES {
        for (algorithm_label, algorithm) in ALGORITHMS {
            let frame = run_case(geometry, *algorithm);
            assert_eq!(frame.width, geometry.dst_width);
            assert_eq!(frame.height, geometry.dst_height);
            assert_eq!(frame.planes.len(), 3, "4:2:0 output must keep three planes");
            results.push((
                format!("{}/{}", geometry.label, algorithm_label),
                digest(&frame),
            ));
        }
    }
    results
}

/// Render the actual results as a paste-ready `GOLDEN` table.
fn render_table(results: &[(String, String)]) -> String {
    let mut out = String::from("const GOLDEN: &[(&str, &str)] = &[\n");
    for (name, hash) in results {
        out.push_str(&format!(
            "    (\n        \"{name}\",\n        \"{hash}\",\n    ),\n"
        ));
    }
    out.push_str("];\n");
    out
}

#[test]
fn scale_output_is_byte_identical_to_goldens() {
    let results = compute_all();

    let mut mismatches = Vec::new();
    for (name, hash) in &results {
        match GOLDEN.iter().find(|(golden_name, _)| golden_name == name) {
            Some((_, golden_hash)) if golden_hash == hash => {}
            Some((_, golden_hash)) => {
                mismatches.push(format!("  {name}: expected {golden_hash}, got {hash}"));
            }
            None => mismatches.push(format!("  {name}: missing from GOLDEN, got {hash}")),
        }
    }
    for (golden_name, _) in GOLDEN {
        if !results.iter().any(|(name, _)| name == golden_name) {
            mismatches.push(format!("  {golden_name}: pinned but not produced"));
        }
    }

    assert!(
        mismatches.is_empty(),
        "ScaleFilter output changed -- the resampler is NOT byte-identical any more.\n\
         Mismatches:\n{}\n\nIf (and only if) the change is intentional, replace the table with:\n\n{}",
        mismatches.join("\n"),
        render_table(&results)
    );
}

#[test]
fn scale_output_is_deterministic_across_runs() {
    // Row parallelism must not introduce any run-to-run variation: the same input has to produce
    // the same bytes every time, regardless of how rayon happens to schedule the rows.
    let first = compute_all();
    let second = compute_all();
    assert_eq!(first, second, "scaler output is not run-to-run stable");
}

/// Independent point-sample oracle: `floor((dst_pos + 0.5) * src_len / dst_len)` per axis, applied
/// per plane so the 4:2:0 chroma geometry is sampled on its own grid.
///
/// Deliberately written without touching the filter's coefficient machinery, so it can contradict
/// it.
fn point_sample(input: &VideoFrame, dst_width: u32, dst_height: u32) -> VideoFrame {
    let mut output = VideoFrame::new(input.format, dst_width, dst_height);

    let plane_count = input.planes.len();
    for index in 0..plane_count {
        let (src_w, src_h) = input.plane_dimensions(index);
        let (dst_w, dst_h) = output.plane_dimensions(index);
        let Some(src_plane) = input.planes.get(index) else {
            continue;
        };

        let mut data = vec![0u8; dst_w as usize * dst_h as usize];
        for y in 0..dst_h {
            let src_y = (((f64::from(y) + 0.5) * f64::from(src_h) / f64::from(dst_h)).floor()
                as u32)
                .min(src_h.saturating_sub(1)) as usize;
            for x in 0..dst_w {
                let src_x = (((f64::from(x) + 0.5) * f64::from(src_w) / f64::from(dst_w)).floor()
                    as u32)
                    .min(src_w.saturating_sub(1)) as usize;
                let source = src_y * src_plane.stride + src_x;
                if let (Some(slot), Some(value)) = (
                    data.get_mut(y as usize * dst_w as usize + x as usize),
                    src_plane.data.get(source),
                ) {
                    *slot = *value;
                }
            }
        }
        output.planes.push(Plane::new(data, dst_w as usize));
    }

    output
}

#[test]
fn nearest_point_samples_on_exact_ratios() {
    // The canonical trigger for the old zero-tap defect, exercised through the public filter API:
    // an exact 2x downscale puts every output centre on a source-pixel boundary, where the
    // `Nearest` support of exactly 0.5 left the whole tap window at weight zero and the frame came
    // out black. The oracle is an independent point sampler, so this pins the *values*, not just
    // "not black".
    //
    // Both antialias settings are checked: antialiasing widens the footprint of a convolution
    // kernel, but a point sampler has nothing to widen, so the two must agree exactly.
    for (src_width, src_height, dst_width, dst_height) in [
        (160u32, 120u32, 80u32, 60u32),
        (96, 72, 48, 36),
        (64, 64, 16, 16),
    ] {
        let input = build_frame(src_width, src_height);
        let expected = point_sample(&input, dst_width, dst_height);

        let mut digests = Vec::new();
        for antialias in [true, false] {
            let config = ScaleConfig::new(dst_width, dst_height)
                .with_algorithm(ScaleAlgorithm::Nearest)
                .with_antialias(antialias);
            let mut filter = ScaleFilter::new(NodeId(0), "nearest_exact_ratio", config);
            let output = filter
                .process(Some(FilterFrame::Video(input.clone())))
                .expect("scale filter must accept a video frame")
                .expect("scale filter must emit a frame");
            let FilterFrame::Video(frame) = output else {
                panic!("scale filter must not emit audio");
            };

            assert_eq!(frame.planes.len(), expected.planes.len());
            for (plane_index, (got, want)) in
                frame.planes.iter().zip(expected.planes.iter()).enumerate()
            {
                assert!(
                    got.data.iter().any(|byte| *byte != 0),
                    "{src_width}x{src_height}->{dst_width}x{dst_height} aa={antialias} \
                     plane {plane_index}: output is entirely black"
                );
                assert_eq!(
                    got.data, want.data,
                    "{src_width}x{src_height}->{dst_width}x{dst_height} aa={antialias} \
                     plane {plane_index}: Nearest did not point-sample"
                );
            }
            digests.push(digest(&frame));
        }

        assert_eq!(
            digests.first(),
            digests.get(1),
            "{src_width}x{src_height}->{dst_width}x{dst_height}: Nearest must not depend on the \
             antialias flag"
        );
    }
}

#[test]
fn scale_reuses_coefficients_across_frames() {
    // The coefficient cache is keyed on (source, target, algorithm, antialias); feeding the same
    // geometry repeatedly must keep producing identical bytes, and feeding an interleaved second
    // geometry must not corrupt the first one's cached tables.
    let config = ScaleConfig::new(128, 96).with_algorithm(ScaleAlgorithm::Lanczos3);
    let mut filter = ScaleFilter::new(NodeId(0), "cache_scale", config);

    let small = build_frame(61, 37);
    let large = build_frame(160, 120);

    let mut small_digests = Vec::new();
    let mut large_digests = Vec::new();
    for _ in 0..3 {
        for (source, sink) in [(&small, &mut small_digests), (&large, &mut large_digests)] {
            let output = filter
                .process(Some(FilterFrame::Video(source.clone())))
                .expect("scale filter must accept a video frame")
                .expect("scale filter must emit a frame");
            match output {
                FilterFrame::Video(frame) => sink.push(digest(&frame)),
                FilterFrame::Audio(_) => panic!("scale filter must not emit audio"),
            }
        }
    }

    assert_eq!(small_digests[0], small_digests[1]);
    assert_eq!(small_digests[0], small_digests[2]);
    assert_eq!(large_digests[0], large_digests[1]);
    assert_eq!(large_digests[0], large_digests[2]);
    assert_ne!(small_digests[0], large_digests[0]);
}
