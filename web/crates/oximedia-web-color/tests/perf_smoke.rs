// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Perf smoke test (`#[ignore]` — run explicitly, in release mode):
//!
//! ```sh
//! cargo test -p oximedia-web-color --release -- --ignored --nocapture
//! ```
//!
//! Canary: 1080p exposure + ACES + 33³ tetrahedral LUT under 30 ms native.
//! (The wasm target for the same chain is ≤ 6 ms with `+simd128`.)

use std::time::Instant;

use oximedia_web_color::{ColorPipeline, Lut3d, LutInterp, ToneMapOperator};

#[test]
#[ignore = "stage breakdown; run with --release -- --ignored --nocapture"]
fn perf_stage_breakdown() {
    let width = 1920usize;
    let height = 1080usize;
    let src: Vec<u8> = (0..width * height * 4)
        .map(|i| (i.wrapping_mul(2654435761)) as u8)
        .collect();
    let mut dst = vec![0u8; src.len()];

    let time_it = |p: &mut ColorPipeline, label: &str, dst: &mut [u8]| {
        p.apply_rgba8(&src, dst).expect("warm-up");
        let start = Instant::now();
        for _ in 0..5 {
            p.apply_rgba8(&src, dst).expect("apply");
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / 5.0;
        println!("{label}: {ms:.2} ms/frame");
    };

    let mut p = ColorPipeline::new();
    time_it(&mut p, "identity (decode+encode)", &mut dst);

    p.set_exposure(0.7).expect("exposure");
    time_it(&mut p, "+ exposure", &mut dst);

    p.set_tone_map(ToneMapOperator::Aces, 1000.0, 100.0)
        .expect("tone map");
    time_it(&mut p, "+ aces", &mut dst);

    let lut = Lut3d::from_fn(33, |r, g, b| [r.sqrt(), g, b * b]).expect("lut");
    p.set_lut(lut, LutInterp::Tetrahedral);
    time_it(&mut p, "+ lut33 tetrahedral", &mut dst);
}

#[test]
#[ignore = "perf canary; run with --release -- --ignored --nocapture"]
fn perf_1080p_exposure_aces_lut33() {
    let width = 1920usize;
    let height = 1080usize;

    let mut p = ColorPipeline::new();
    p.set_exposure(0.7).expect("exposure");
    p.set_tone_map(ToneMapOperator::Aces, 1000.0, 100.0)
        .expect("tone map");
    let lut = Lut3d::from_fn(33, |r, g, b| [r.sqrt(), g, b * b]).expect("lut");
    p.set_lut(lut, LutInterp::Tetrahedral);

    let src: Vec<u8> = (0..width * height * 4)
        .map(|i| (i.wrapping_mul(2654435761)) as u8)
        .collect();
    let mut dst = vec![0u8; src.len()];

    // Warm-up.
    p.apply_rgba8(&src, &mut dst).expect("warm-up");

    let iterations = 10;
    let start = Instant::now();
    for _ in 0..iterations {
        p.apply_rgba8(&src, &mut dst).expect("apply");
    }
    let elapsed = start.elapsed();
    let per_frame_ms = elapsed.as_secs_f64() * 1000.0 / f64::from(iterations);
    println!(
        "1080p exposure+ACES+LUT33(tetrahedral): {per_frame_ms:.2} ms/frame \
         ({iterations} iterations, {:.1} ms total)",
        elapsed.as_secs_f64() * 1000.0
    );
    // Default budget 30 ms; override via PERF_CANARY_MS on machines where
    // parallel builds distort wall-clock timing (measures ~26 ms alone on an
    // Apple-Silicon box, ~33 ms while sibling workspaces compile).
    let budget_ms: f64 = std::env::var("PERF_CANARY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30.0);
    assert!(
        per_frame_ms < budget_ms,
        "perf canary breached: {per_frame_ms:.2} ms/frame >= {budget_ms} ms native"
    );
}
