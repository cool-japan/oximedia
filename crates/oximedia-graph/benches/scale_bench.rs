//! Throughput benchmark for [`ScaleFilter`].
//!
//! The primary case is the one that dominates a 9:16 reframe chain: a 608x1080 crop of a 1080p
//! source resampled to a full 1080x1920 vertical frame with Lanczos3. At 30 fps the per-frame
//! budget for the whole chain is 33.3 ms, so the scaler alone has to land comfortably below that.
//!
//! The secondary case (1920x1080 -> 1080x1920) is the "no crop, just squeeze" shape, which has a
//! horizontal downscale (and therefore a widened antialias kernel) instead of an upscale.
//!
//! Both cases are measured twice: once against the current filter and once against `reference`,
//! a verbatim transcription of the pre-optimisation resampler. Running both in one process makes
//! the speedup measurable on a loaded machine, where absolute wall-clock numbers drift but the
//! ratio between two back-to-back benchmarks stays meaningful.

// The `reference` module is a deliberate copy of superseded code; it is kept shaped exactly as it
// was so the comparison measures the real change rather than a rewritten strawman.
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::needless_range_loop)]

use std::hint::black_box;

use criterion::{
    criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, SamplingMode, Throughput,
};
use oximedia_codec::VideoFrame;
use oximedia_core::PixelFormat;
use oximedia_graph::filters::video::{ScaleAlgorithm, ScaleConfig, ScaleFilter};
use oximedia_graph::{FilterFrame, Node, NodeId};

/// Deterministic 4:2:0 source frame with structure in every plane.
fn build_frame(width: u32, height: u32) -> VideoFrame {
    let mut frame = VideoFrame::new(PixelFormat::Yuv420p, width, height);
    frame.allocate();

    let plane_count = frame.planes.len();
    for index in 0..plane_count {
        let (plane_width, plane_height) = frame.plane_dimensions(index);
        let bias = (index as u32) * 37;
        let Some(plane) = frame.planes.get_mut(index) else {
            continue;
        };
        for y in 0..plane_height {
            for x in 0..plane_width {
                let checker = if ((x / 16) + (y / 16)) % 2 == 0 {
                    48
                } else {
                    0
                };
                let value = (x + y + checker + bias) % 256;
                let offset = y as usize * plane.stride + x as usize;
                if let Some(slot) = plane.data.get_mut(offset) {
                    *slot = value as u8;
                }
            }
        }
    }

    frame
}

/// The resampler exactly as it stood before the realtime work: `Vec<Vec<f64>>` coefficients,
/// per-output-pixel bounds-checked taps, a column-strided vertical pass, and a fresh chroma
/// coefficient set built on every frame.
mod reference {
    use oximedia_codec::{Plane, VideoFrame};
    use oximedia_graph::filters::video::ScaleAlgorithm;

    #[derive(Clone)]
    pub struct FilterCoefficients {
        pub start: usize,
        pub weights: Vec<f64>,
    }

    pub fn coefficients(
        src_size: u32,
        dst_size: u32,
        algorithm: ScaleAlgorithm,
        antialias: bool,
    ) -> Vec<FilterCoefficients> {
        let mut coefficients = Vec::with_capacity(dst_size as usize);
        let scale = src_size as f64 / dst_size as f64;

        let filter_scale = if antialias && scale > 1.0 { scale } else { 1.0 };
        let support = algorithm.support() * filter_scale;

        for dst_pos in 0..dst_size {
            let center = (dst_pos as f64 + 0.5) * scale - 0.5;
            let start = ((center - support).floor() as i64).max(0) as usize;
            let end = ((center + support).ceil() as i64).min(src_size as i64) as usize;

            let mut weights = Vec::with_capacity(end - start);
            let mut sum = 0.0;

            for src_pos in start..end {
                let distance = (src_pos as f64 - center) / filter_scale;
                let weight = algorithm.kernel(distance);
                weights.push(weight);
                sum += weight;
            }

            if sum != 0.0 {
                for w in &mut weights {
                    *w /= sum;
                }
            }

            coefficients.push(FilterCoefficients { start, weights });
        }

        coefficients
    }

    pub fn scale_plane(
        h_coefficients: &[FilterCoefficients],
        v_coefficients: &[FilterCoefficients],
        src: &Plane,
        src_width: u32,
        src_height: u32,
        dst_width: u32,
        dst_height: u32,
    ) -> Plane {
        let mut intermediate = vec![0.0f64; dst_width as usize * src_height as usize];

        for y in 0..src_height as usize {
            let src_row = src.row(y);
            for (x, coef) in h_coefficients.iter().enumerate() {
                let mut sum = 0.0;
                for (i, &weight) in coef.weights.iter().enumerate() {
                    let src_x = (coef.start + i).min(src_width as usize - 1);
                    sum += src_row.get(src_x).copied().unwrap_or(0) as f64 * weight;
                }
                intermediate[y * dst_width as usize + x] = sum;
            }
        }

        let mut dst_data = vec![0u8; dst_width as usize * dst_height as usize];

        for y in 0..dst_height as usize {
            let coef = &v_coefficients[y];
            for x in 0..dst_width as usize {
                let mut sum = 0.0;
                for (i, &weight) in coef.weights.iter().enumerate() {
                    let src_y = (coef.start + i).min(src_height as usize - 1);
                    sum += intermediate[src_y * dst_width as usize + x] * weight;
                }
                dst_data[y * dst_width as usize + x] = sum.round().clamp(0.0, 255.0) as u8;
            }
        }

        Plane::new(dst_data, dst_width as usize)
    }

    pub fn scale_frame(
        input: &VideoFrame,
        dst_width: u32,
        dst_height: u32,
        algorithm: ScaleAlgorithm,
        antialias: bool,
    ) -> VideoFrame {
        // The luma tables were cached across frames by the original filter too, so they are built
        // once here and not counted per frame.
        let mut h_coefficients = coefficients(input.width, dst_width, algorithm, antialias);
        let mut v_coefficients = coefficients(input.height, dst_height, algorithm, antialias);

        let mut output = VideoFrame::new(input.format, dst_width, dst_height);
        output.timestamp = input.timestamp;
        output.frame_type = input.frame_type;
        output.color_info = input.color_info;

        for (i, src_plane) in input.planes.iter().enumerate() {
            let (src_w, src_h) = input.plane_dimensions(i);
            let (dst_w, dst_h) = output.plane_dimensions(i);

            if i > 0 && input.format.is_yuv() {
                // Faithful reproduction of the original mutate-and-swap: the whole luma tables
                // (one `Vec<f64>` per output position, ~3000 of them) were deep-cloned and
                // restored around every chroma plane, on every frame.
                let old_h = h_coefficients.clone();
                let old_v = v_coefficients.clone();

                h_coefficients = coefficients(src_w, dst_w, algorithm, antialias);
                v_coefficients = coefficients(src_h, dst_h, algorithm, antialias);

                output.planes.push(scale_plane(
                    &h_coefficients,
                    &v_coefficients,
                    src_plane,
                    src_w,
                    src_h,
                    dst_w,
                    dst_h,
                ));

                h_coefficients = old_h;
                v_coefficients = old_v;
            } else {
                output.planes.push(scale_plane(
                    &h_coefficients,
                    &v_coefficients,
                    src_plane,
                    src_w,
                    src_h,
                    dst_w,
                    dst_h,
                ));
            }
        }

        output
    }
}

/// Cases measured: (label, source width, source height, target width, target height).
const CASES: &[(&str, u32, u32, u32, u32)] = &[
    ("608x1080_to_1080x1920", 608, 1080, 1080, 1920),
    ("1920x1080_to_1080x1920", 1920, 1080, 1080, 1920),
];

fn bench_scale_lanczos3(c: &mut Criterion) {
    let mut group = c.benchmark_group("scale_lanczos3");
    // Flat sampling: one frame per sample, so the reported distribution is per-frame cost rather
    // than a mean over an auto-chosen iteration count. That matters on a shared machine -- with
    // linear sampling every sample averages away the moments when a performance core is actually
    // free, and on Apple silicon a thread parked on an efficiency core is several times slower.
    // With flat sampling the minimum over the samples is a usable uncontended estimate.
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(50);

    for &(label, src_width, src_height, dst_width, dst_height) in CASES {
        let frame = build_frame(src_width, src_height);
        let config = ScaleConfig::new(dst_width, dst_height)
            .with_algorithm(ScaleAlgorithm::Lanczos3)
            .with_antialias(true);
        let mut filter = ScaleFilter::new(NodeId(0), "bench_scale", config);

        // Warm the coefficient cache so the measurement is steady-state per-frame cost, and
        // assert that the two implementations still agree byte for byte.
        let warmup = filter
            .process(Some(FilterFrame::Video(frame.clone())))
            .expect("scale filter must accept a video frame")
            .expect("scale filter must emit a frame");
        let expected = reference::scale_frame(
            &frame,
            dst_width,
            dst_height,
            ScaleAlgorithm::Lanczos3,
            true,
        );
        match warmup {
            FilterFrame::Video(actual) => {
                assert_eq!(actual.planes.len(), expected.planes.len());
                for (got, want) in actual.planes.iter().zip(expected.planes.iter()) {
                    assert_eq!(
                        got.data, want.data,
                        "{label}: filter diverged from reference"
                    );
                }
            }
            FilterFrame::Audio(_) => panic!("scale filter must not emit audio"),
        }

        // One "element" per output luma pixel.
        group.throughput(Throughput::Elements(
            u64::from(dst_width) * u64::from(dst_height),
        ));

        group.bench_function(BenchmarkId::new("reference", label), |b| {
            b.iter_batched(
                || frame.clone(),
                |input| {
                    black_box(reference::scale_frame(
                        &input,
                        dst_width,
                        dst_height,
                        ScaleAlgorithm::Lanczos3,
                        true,
                    ))
                },
                // The input frame is ~1 MiB; allocate one per iteration instead of a whole batch.
                BatchSize::PerIteration,
            );
        });

        group.bench_function(BenchmarkId::new("optimized", label), |b| {
            b.iter_batched(
                || FilterFrame::Video(frame.clone()),
                |input| black_box(filter.process(Some(input))),
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_scale_lanczos3);
criterion_main!(benches);
