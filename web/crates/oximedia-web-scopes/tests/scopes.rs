// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Integration tests for the natively-testable [`ScopeRenderer`] API.
//!
//! Ported and adapted from the strongest native `oximedia-scopes` test cases
//! (waveform / vectorscope / histogram / false-colour), plus regression tests
//! for the three upstream bugs this port fixes (silent font, false-colour size,
//! per-frame allocation) and the data-plane guarantees (fixed output size,
//! never-panic error paths).

use oximedia_web_scopes::{
    FalseColorPreset, HistogramKind, ScopeError, ScopeRenderer, WaveformMode,
};

// ── frame builders ──────────────────────────────────────────────────────────

fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut f = vec![0u8; (w * h * 4) as usize];
    for px in f.chunks_exact_mut(4) {
        px[0] = rgb[0];
        px[1] = rgb[1];
        px[2] = rgb[2];
        px[3] = 255;
    }
    f
}

fn gray(w: u32, h: u32, v: u8) -> Vec<u8> {
    solid(w, h, [v, v, v])
}

/// Reads the RGBA pixel at `(x, y)` from a `w`-wide RGBA8 buffer.
fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

fn out_buf(w: u32, h: u32) -> Vec<u8> {
    vec![0u8; (w * h * 4) as usize]
}

// ── graticule / font ────────────────────────────────────────────────────────

#[test]
fn graticule_present_only_when_enabled() {
    let (sw, sh) = (256u32, 256u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = gray(64, 64, 0); // black: trace sits on the bottom row only
    let mut on = out_buf(sw, sh);
    let mut off = out_buf(sw, sh);
    r.waveform(&frame, 64, 64, WaveformMode::Luma, true, &mut on)
        .expect("wf on");
    r.waveform(&frame, 64, 64, WaveformMode::Luma, false, &mut off)
        .expect("wf off");

    // The vertical graticule line at x = w/4 crosses row 100 (no trace there).
    assert!(px(&on, sw, 64, 100)[0] > 0, "graticule line missing when on");
    assert_eq!(px(&off, sw, 64, 100)[0], 0, "stray pixel when graticule off");
}

#[test]
fn parade_labels_actually_render() {
    // Regression for the silent-font upstream bug: with graticule+labels on, the
    // RGB parade caption row must contain lit label pixels near the top.
    let (sw, sh) = (300u32, 128u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = gray(96, 96, 0);
    let mut on = out_buf(sw, sh);
    let mut off = out_buf(sw, sh);
    r.waveform(&frame, 96, 96, WaveformMode::RgbParade, true, &mut on)
        .expect("parade on");
    r.waveform(&frame, 96, 96, WaveformMode::RgbParade, false, &mut off)
        .expect("parade off");

    // Count lit pixels in the top label band (rows 1..9) where the "R/G/B"
    // glyphs live. Labels-on must light strictly more than labels-off.
    let band_lit = |buf: &[u8]| {
        let mut n = 0u32;
        for y in 1..9 {
            for x in 0..sw {
                if px(buf, sw, x, y)[0] > 0 {
                    n += 1;
                }
            }
        }
        n
    };
    assert!(
        band_lit(&on) > band_lit(&off) + 5,
        "parade labels did not render (on={}, off={})",
        band_lit(&on),
        band_lit(&off)
    );
}

// ── waveform ────────────────────────────────────────────────────────────────

#[test]
fn constant_gray_concentrates_at_correct_ire_row() {
    let (sw, sh) = (256u32, 256u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = gray(256, 256, 128); // 50% gray → ~50 IRE → middle row
    let mut out = out_buf(sw, sh);
    r.waveform(&frame, 256, 256, WaveformMode::Luma, false, &mut out)
        .expect("waveform");

    // Row with the most trace energy should be the ~50% row (128 → row 127).
    let mut best_row = 0u32;
    let mut best_sum = 0u64;
    for y in 0..sh {
        let mut s = 0u64;
        for x in 0..sw {
            s += u64::from(px(&out, sw, x, y)[0]);
        }
        if s > best_sum {
            best_sum = s;
            best_row = y;
        }
    }
    assert!(
        (125..=130).contains(&best_row),
        "gray trace landed at row {best_row}, expected ~127"
    );
    // Far-away rows must be empty.
    assert_eq!(px(&out, sw, 100, 40)[0], 0);
    assert_eq!(px(&out, sw, 100, 220)[0], 0);
}

#[test]
fn rgb_parade_separates_panes() {
    let (sw, sh) = (300u32, 256u32); // section_w = 100
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = solid(128, 128, [255, 0, 0]); // pure red
    let mut out = out_buf(sw, sh);
    r.waveform(&frame, 128, 128, WaveformMode::RgbParade, false, &mut out)
        .expect("parade");

    // R (value 255) → top of section 0; G,B (value 0) → bottom of sections 1,2.
    assert!(px(&out, sw, 50, 0)[0] > 0, "R pane top should be lit red");
    assert_eq!(px(&out, sw, 50, 255)[0], 0, "R pane bottom should be empty");
    assert!(px(&out, sw, 150, 255)[1] > 0, "G pane bottom should be lit");
    assert!(px(&out, sw, 250, 255)[2] > 0, "B pane bottom should be lit");
    // Panes carry their channel colour only.
    assert_eq!(px(&out, sw, 50, 0)[1], 0);
    assert_eq!(px(&out, sw, 50, 0)[2], 0);
}

#[test]
fn ycbcr_waveform_neutral_gray_centers_chroma() {
    let (sw, sh) = (300u32, 256u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = gray(128, 128, 128); // neutral → Cb=Cr=128 → middle row
    let mut out = out_buf(sw, sh);
    r.waveform(&frame, 128, 128, WaveformMode::Ycbcr, false, &mut out)
        .expect("ycbcr");
    // Cb pane (section 1) and Cr pane (section 2) traces sit at the ~middle row.
    let mid = sh / 2;
    let lit_near_mid = |sx: u32| {
        (mid - 3..=mid + 3).any(|y| px(&out, sw, sx, y)[0] > 0)
    };
    assert!(lit_near_mid(150), "Cb pane not centred for neutral gray");
    assert!(lit_near_mid(250), "Cr pane not centred for neutral gray");
}

// ── vectorscope ─────────────────────────────────────────────────────────────

#[test]
fn vectorscope_pure_red_lands_at_expected_point() {
    let (sw, sh) = (256u32, 256u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = solid(128, 128, [255, 0, 0]);
    let mut out = out_buf(sw, sh);
    r.vectorscope(&frame, 128, 128, 1.0, false, false, &mut out)
        .expect("vectorscope");

    // BT.601 pure red is Cb=85, Cr=255 (documented core constant).
    let (cb, cr) = (85.0f32, 255.0f32);
    let mr = (sw.min(sh) / 2 - 10) as f32; // 118
    let scale = mr / 128.0;
    let sx = (128.0 + (cb - 128.0) * scale) as i32 as u32;
    let sy = (128.0 - (cr - 128.0) * scale) as i32 as u32;

    assert!(sx < 128 && sy < 128, "red must plot upper-left");
    assert!(
        px(&out, sw, sx, sy)[0] > 200,
        "red trace not bright at ({sx},{sy})"
    );
    // Centre stays empty (no neutral pixels in a pure-red frame).
    assert_eq!(px(&out, sw, 128, 128)[0], 0);
}

#[test]
fn vectorscope_skin_tone_line_drawn_at_angle() {
    let (sw, sh) = (256u32, 256u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = gray(64, 64, 0); // black → trace collapses to centre
    let mut with = out_buf(sw, sh);
    let mut without = out_buf(sw, sh);
    r.vectorscope(&frame, 64, 64, 1.0, true, true, &mut with)
        .expect("with skin");
    r.vectorscope(&frame, 64, 64, 1.0, false, true, &mut without)
        .expect("no skin");

    // Warm pixels (R noticeably above G and B) are the skin-tone line's colour.
    let warm = |buf: &[u8]| -> u32 {
        let mut n = 0;
        for y in 0..sh {
            for x in 0..sw {
                let p = px(buf, sw, x, y);
                if p[0] as i32 > p[1] as i32 + 5 && p[0] as i32 > p[2] as i32 + 5 {
                    n += 1;
                }
            }
        }
        n
    };
    assert!(
        warm(&with) > warm(&without) + 20,
        "skin-tone line missing (with={}, without={})",
        warm(&with),
        warm(&without)
    );

    // Spot-check a point on the 123-degree ray at radius 60.
    let ang = 123.0f32.to_radians();
    let lx = (128.0 + ang.cos() * 60.0) as i32;
    let ly = (128.0 - ang.sin() * 60.0) as i32;
    let mut hit = false;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let p = px(&with, sw, (lx + dx) as u32, (ly + dy) as u32);
            if p[0] as i32 > p[1] as i32 + 5 && p[0] as i32 > p[2] as i32 + 5 {
                hit = true;
            }
        }
    }
    assert!(hit, "no skin-tone colour near the 123-degree ray");
}

// ── histogram ───────────────────────────────────────────────────────────────

#[test]
fn luma_histogram_of_two_value_image_has_two_bins() {
    let (sw, sh) = (256u32, 128u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    // Top half luma 128, bottom half luma 200.
    let (w, h) = (64u32, 64u32);
    let mut frame = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let v = if y < h / 2 { 128 } else { 200 };
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            frame[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    let mut out = out_buf(sw, sh);
    r.histogram(&frame, w, h, HistogramKind::Luma, false, &mut out)
        .expect("histogram");

    let bins = r.last_luma_histogram();
    let nonzero: Vec<usize> = bins
        .iter()
        .enumerate()
        .filter(|(_, &c)| c > 0)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(nonzero, vec![128, 200], "expected exactly two luma bins");
}

#[test]
fn rgb_histogram_renders_channels() {
    let (sw, sh) = (256u32, 128u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = solid(64, 64, [200, 0, 0]); // only the red channel populated
    let mut out = out_buf(sw, sh);
    r.histogram(&frame, 64, 64, HistogramKind::Rgb, false, &mut out)
        .expect("rgb histogram");
    // Some red bar must be visible.
    let any_red = out.chunks_exact(4).any(|p| p[0] > 100);
    assert!(any_red, "RGB histogram drew no red bars");
}

// ── false colour ────────────────────────────────────────────────────────────

#[test]
fn false_color_maps_known_luma_to_preset_colors() {
    let (sw, sh) = (128u32, 96u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let mut out = out_buf(sw, sh);

    // Spectrum: 50% gray → yellow-green band [128,255,0].
    r.false_color(&gray(80, 60, 128), 80, 60, FalseColorPreset::Spectrum, &mut out)
        .expect("spectrum gray");
    assert_eq!(px(&out, sw, sw / 2, sh / 2), [128, 255, 0, 255]);

    // Spectrum: black → dark blue.
    r.false_color(&gray(80, 60, 0), 80, 60, FalseColorPreset::Spectrum, &mut out)
        .expect("spectrum black");
    assert_eq!(px(&out, sw, sw / 2, sh / 2), [0, 0, 128, 255]);

    // ARRI: white → clipping red; gray → grey; black → crushed purple.
    r.false_color(&gray(80, 60, 255), 80, 60, FalseColorPreset::Arri, &mut out)
        .expect("arri white");
    assert_eq!(px(&out, sw, sw / 2, sh / 2), [255, 0, 0, 255]);
    r.false_color(&gray(80, 60, 128), 80, 60, FalseColorPreset::Arri, &mut out)
        .expect("arri gray");
    assert_eq!(px(&out, sw, sw / 2, sh / 2), [128, 128, 128, 255]);
    r.false_color(&gray(80, 60, 0), 80, 60, FalseColorPreset::Arri, &mut out)
        .expect("arri black");
    assert_eq!(px(&out, sw, sw / 2, sh / 2), [128, 0, 128, 255]);
}

#[test]
fn false_color_renders_at_scope_size_not_input_size() {
    // Regression for the upstream size bug: scope 300x200, input 640x480, with a
    // white top-left quadrant. The scope must be scope-sized and spatially
    // sample the input (white→magenta top-left, black→dark-blue bottom-right).
    let (sw, sh) = (300u32, 200u32);
    let (fw, fh) = (640u32, 480u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");

    let mut frame = vec![0u8; (fw * fh * 4) as usize];
    for y in 0..fh {
        for x in 0..fw {
            let i = ((y * fw + x) * 4) as usize;
            let white = x < fw / 2 && y < fh / 2;
            let v = if white { 255 } else { 0 };
            frame[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    let mut out = out_buf(sw, sh);
    r.false_color(&frame, fw, fh, FalseColorPreset::Spectrum, &mut out)
        .expect("false color");

    assert_eq!(out.len(), (sw * sh * 4) as usize, "output is not scope-sized");
    // Top-left (samples the white quadrant) → bright magenta band.
    assert_eq!(px(&out, sw, 10, 10), [255, 0, 255, 255]);
    // Bottom-right (samples black) → dark blue.
    assert_eq!(px(&out, sw, sw - 10, sh - 10), [0, 0, 128, 255]);
}

#[test]
fn every_scope_yields_configured_output_size() {
    let (sw, sh) = (321u32, 199u32); // deliberately odd, not /3
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = gray(200, 150, 90);
    let expected = (sw * sh * 4) as usize;
    let mut out = out_buf(sw, sh);

    for mode in [
        WaveformMode::Luma,
        WaveformMode::RgbParade,
        WaveformMode::RgbOverlay,
        WaveformMode::Ycbcr,
    ] {
        r.waveform(&frame, 200, 150, mode, true, &mut out)
            .unwrap_or_else(|e| panic!("waveform {mode:?}: {e}"));
        assert_eq!(out.len(), expected);
    }
    r.vectorscope(&frame, 200, 150, 1.0, true, true, &mut out)
        .expect("vectorscope");
    r.histogram(&frame, 200, 150, HistogramKind::Luma, true, &mut out)
        .expect("hist luma");
    r.histogram(&frame, 200, 150, HistogramKind::Rgb, true, &mut out)
        .expect("hist rgb");
    r.false_color(&frame, 200, 150, FalseColorPreset::Arri, &mut out)
        .expect("false color");
    assert_eq!(out.len(), expected);
}

// ── allocation discipline ───────────────────────────────────────────────────

#[test]
fn scopes_are_deterministic_and_do_not_grow() {
    let (sw, sh) = (300u32, 256u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = solid(128, 128, [180, 90, 30]);

    // Warm up so the input-column map has grown to its final size.
    let mut warm = out_buf(sw, sh);
    r.waveform(&frame, 128, 128, WaveformMode::RgbParade, true, &mut warm)
        .expect("warm");
    let cap = r.accumulator_capacity();

    for (label, run) in [
        ("luma", WaveformMode::Luma),
        ("parade", WaveformMode::RgbParade),
        ("overlay", WaveformMode::RgbOverlay),
        ("ycbcr", WaveformMode::Ycbcr),
    ] {
        let mut a = out_buf(sw, sh);
        let mut b = out_buf(sw, sh);
        r.waveform(&frame, 128, 128, run, true, &mut a)
            .unwrap_or_else(|e| panic!("{label} a: {e}"));
        r.waveform(&frame, 128, 128, run, true, &mut b)
            .unwrap_or_else(|e| panic!("{label} b: {e}"));
        assert_eq!(a, b, "{label} not deterministic");
        assert_eq!(r.accumulator_capacity(), cap, "{label} grew buffers");
    }

    let mut v1 = out_buf(sw, sh);
    let mut v2 = out_buf(sw, sh);
    r.vectorscope(&frame, 128, 128, 1.2, true, true, &mut v1)
        .expect("v1");
    r.vectorscope(&frame, 128, 128, 1.2, true, true, &mut v2)
        .expect("v2");
    assert_eq!(v1, v2, "vectorscope not deterministic");
    assert_eq!(r.accumulator_capacity(), cap, "vectorscope grew buffers");

    let mut fc1 = out_buf(sw, sh);
    let mut fc2 = out_buf(sw, sh);
    r.false_color(&frame, 128, 128, FalseColorPreset::Spectrum, &mut fc1)
        .expect("fc1");
    r.false_color(&frame, 128, 128, FalseColorPreset::Spectrum, &mut fc2)
        .expect("fc2");
    assert_eq!(fc1, fc2, "false colour not deterministic");
    assert_eq!(r.accumulator_capacity(), cap, "false colour grew buffers");
}

// ── error paths (never panic) ───────────────────────────────────────────────

#[test]
fn short_frame_is_error_not_panic() {
    let (sw, sh) = (128u32, 128u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let short = vec![0u8; 10];
    let mut out = out_buf(sw, sh);
    let err = r
        .waveform(&short, 64, 64, WaveformMode::Luma, false, &mut out)
        .unwrap_err();
    assert!(matches!(err, ScopeError::Core(_)));
}

#[test]
fn wrong_output_length_is_error() {
    let (sw, sh) = (128u32, 128u32);
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let frame = gray(64, 64, 50);
    let mut too_small = vec![0u8; 16];
    let err = r
        .waveform(&frame, 64, 64, WaveformMode::Luma, false, &mut too_small)
        .unwrap_err();
    assert!(matches!(err, ScopeError::OutputLength { .. }));
}

#[test]
fn zero_frame_dimension_is_error() {
    let mut r = ScopeRenderer::new(64, 64).expect("renderer");
    let mut out = out_buf(64, 64);
    let err = r
        .vectorscope(&[], 0, 64, 1.0, false, false, &mut out)
        .unwrap_err();
    assert!(matches!(err, ScopeError::Core(_)));
}

#[test]
fn parade_needs_three_columns() {
    let mut r = ScopeRenderer::new(2, 64).expect("renderer");
    let frame = gray(8, 8, 100);
    let mut out = out_buf(2, 64);
    let err = r
        .waveform(&frame, 8, 8, WaveformMode::RgbParade, false, &mut out)
        .unwrap_err();
    assert!(matches!(err, ScopeError::ScopeTooSmall));
}

#[test]
fn zero_scope_dimension_is_error() {
    assert!(ScopeRenderer::new(0, 64).is_err());
    assert!(ScopeRenderer::new(64, 0).is_err());
}

// ── stats ───────────────────────────────────────────────────────────────────

#[test]
fn stats_of_gray_frame() {
    let r = ScopeRenderer::new(64, 64).expect("renderer");
    let frame = gray(100, 100, 128);
    let s = r.compute_stats(&frame, 100, 100).expect("stats");
    assert_eq!(s.min_luma, 128);
    assert_eq!(s.max_luma, 128);
    assert!((s.avg_luma - 128.0).abs() < 0.5);
    assert!(s.std_dev < 0.5, "uniform frame should have ~0 std dev");
    assert!(s.black_clip_percent < 0.01);
    assert!(s.white_clip_percent < 0.01);
}

#[test]
fn stats_clipping_split_frame() {
    let r = ScopeRenderer::new(64, 64).expect("renderer");
    // Half black, half white.
    let (w, h) = (100u32, 100u32);
    let mut frame = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let v = if y < h / 2 { 0 } else { 255 };
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            frame[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    let s = r.compute_stats(&frame, w, h).expect("stats");
    assert_eq!(s.min_luma, 0);
    assert_eq!(s.max_luma, 255);
    assert!(s.black_clip_percent > 40.0);
    assert!(s.white_clip_percent > 40.0);
}

// ── performance canary (native) ─────────────────────────────────────────────

#[test]
#[ignore = "perf canary; run with --ignored"]
fn perf_all_scopes_1080p() {
    use std::time::Instant;

    let (fw, fh) = (1920u32, 1080u32);
    let (sw, sh) = (512u32, 256u32);
    // Deterministic pseudo-random content so the trace is non-trivial.
    let mut frame = vec![0u8; (fw * fh * 4) as usize];
    let mut seed = 0x1234_5678u32;
    for px in frame.chunks_exact_mut(4) {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        px[0] = (seed >> 24) as u8;
        px[1] = (seed >> 16) as u8;
        px[2] = (seed >> 8) as u8;
        px[3] = 255;
    }
    let mut r = ScopeRenderer::new(sw, sh).expect("renderer");
    let mut out = out_buf(sw, sh);

    let time = |label: &str, f: &mut dyn FnMut()| {
        let t = Instant::now();
        f();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!("  {label:<12} {ms:7.3} ms");
        ms
    };

    println!("perf_all_scopes_1080p (native, {fw}x{fh} -> {sw}x{sh}):");
    let mut total = 0.0;
    total += time("waveform", &mut || {
        r.waveform(&frame, fw, fh, WaveformMode::RgbParade, true, &mut out)
            .expect("wf");
    });
    total += time("vectorscope", &mut || {
        r.vectorscope(&frame, fw, fh, 1.0, true, true, &mut out)
            .expect("vs");
    });
    total += time("histogram", &mut || {
        r.histogram(&frame, fw, fh, HistogramKind::Rgb, true, &mut out)
            .expect("hist");
    });
    total += time("falsecolor", &mut || {
        r.false_color(&frame, fw, fh, FalseColorPreset::Arri, &mut out)
            .expect("fc");
    });
    println!("  {:<12} {total:7.3} ms (total)", "TOTAL");
    assert!(total < 40.0, "native total {total:.3} ms exceeded 40 ms canary");
}
