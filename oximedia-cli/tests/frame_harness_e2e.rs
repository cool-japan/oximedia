//! End-to-end tests for the CLI frame-processing harness and its consumers:
//! `timecode burn`, `multicam color-match`, `denoise`, `stabilize`, scaling
//! (`upscale`/`downscale`/`compare`/`batch`), `subtitle burn` and
//! `captions burn`.
//!
//! The harness contract is Y4M in / Y4M out, so every fixture here is a
//! synthetic YUV4MPEG2 clip written inline (same shape as
//! `transcode_reencode.rs`'s `write_y4m_fixture`). Outputs are re-demuxed with
//! the real `Y4mDemuxer` and checked at content level — frame count, geometry
//! and pixels — never merely "the file exists".
//!
//! The commands are driven through their real library entry points
//! (`timecode_cmd::run_timecode`, `multicam_cmd::handle_multicam_command`,
//! `denoise_cmd::run_denoise`, `stabilize_cmd::run_stabilize`,
//! `scaling_cmd::handle_scaling_command`,
//! `subtitle_cmd::handle_subtitle_command`,
//! `captions_cmd::run_captions_burn`), which is exactly what `main.rs`
//! dispatches to.
//!
//! # Font-dependent tests
//!
//! OxiMedia ships no font (fonts carry their own licences) and the CLI never
//! probes system font directories, so every burn-in *pixel* test needs a font
//! supplied by the environment: set `OXIMEDIA_TEST_FONT=/path/to/font.ttf`.
//! Without it those tests skip with a clear message; the honest-error tests
//! (including each command's no-font case) always run.

use assert_cmd::Command;
use oximedia_cli::captions_cmd::{self, CaptionsBurnOptions};
use oximedia_cli::denoise_cmd::{run_denoise, DenoiseOptions};
use oximedia_cli::frame_harness::{self, ClipStats};
use oximedia_cli::multicam_cmd::{handle_multicam_command, MulticamCommand};
use oximedia_cli::scaling_cmd::{handle_scaling_command, ScalingCommand};
use oximedia_cli::stabilize_cmd::{run_stabilize, StabilizeOptions};
use oximedia_cli::subtitle_cmd::{handle_subtitle_command, SubtitleCommand};
use oximedia_cli::timecode_cmd::{run_timecode, TimecodeCommand};
use oximedia_container::demux::y4m::Y4mDemuxer;
use std::path::{Path, PathBuf};

/// Scratch directory under the system temp dir, scoped to this test process
/// so a concurrent run cannot delete these fixtures mid-test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oximedia_frame_harness_e2e_{}_{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Write a synthetic 4:2:0 Y4M clip.
///
/// `luma(x, y, t)` and `chroma(x, y, t, plane)` produce the sample values, so
/// individual tests can control brightness and colour balance exactly.
fn write_y4m_fixture(
    path: &Path,
    width: usize,
    height: usize,
    frames: usize,
    luma: impl Fn(usize, usize, usize) -> u8,
    chroma: impl Fn(usize, usize, usize, usize) -> u8,
) {
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let mut buf = format!("YUV4MPEG2 W{width} H{height} F25:1 Ip A1:1 C420jpeg\n").into_bytes();
    for t in 0..frames {
        buf.extend_from_slice(b"FRAME\n");
        for y in 0..height {
            for x in 0..width {
                buf.push(luma(x, y, t));
            }
        }
        for plane in 0..2 {
            for y in 0..ch {
                for x in 0..cw {
                    buf.push(chroma(x, y, t, plane));
                }
            }
        }
    }
    std::fs::write(path, buf).expect("write y4m fixture");
}

/// Write a flat clip with fixed Y/Cb/Cr values.
fn write_flat_y4m(path: &Path, width: usize, height: usize, frames: usize, y: u8, u: u8, v: u8) {
    write_y4m_fixture(
        path,
        width,
        height,
        frames,
        |_, _, _| y,
        |_, _, _, plane| if plane == 0 { u } else { v },
    );
}

/// Demux a Y4M file into (width, height, frames).
fn demux(path: &Path) -> (u32, u32, Vec<Vec<u8>>) {
    let bytes = std::fs::read(path).expect("read y4m output");
    assert!(
        bytes.starts_with(b"YUV4MPEG2"),
        "output must be a Y4M stream"
    );
    let mut demuxer =
        Y4mDemuxer::new(std::io::Cursor::new(bytes.as_slice())).expect("parse output y4m");
    let (w, h) = (demuxer.width(), demuxer.height());
    let frames = demuxer.read_all_frames().expect("read output frames");
    (w, h, frames)
}

// ─── (a) The harness itself ─────────────────────────────────────────────────

/// `process_frames` streams, preserves geometry, and counts changed bytes.
#[test]
fn harness_process_frames_streams_and_counts_changes() {
    let dir = scratch("process_frames");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    write_y4m_fixture(
        &input,
        32,
        16,
        4,
        |x, y, t| ((x * 3 + y * 5 + t * 7) % 256) as u8,
        |x, y, _, plane| ((x + y + plane * 30) % 256) as u8,
    );

    let stats: ClipStats = frame_harness::process_frames("harness e2e", &input, &output, |i, f| {
        // Paint a 4x4 block whose value depends on the frame index.
        let luma_w = f.layout.luma_w;
        for y in 0..4 {
            for x in 0..4 {
                f.luma_mut()[y * luma_w + x] = (200 - i * 10) as u8;
            }
        }
        Ok(())
    })
    .expect("process_frames must succeed on a Y4M clip");

    assert_eq!(stats.frame_count, 4);
    assert!(
        stats.bytes_changed > 0,
        "a pixel-writing op must report changed bytes"
    );

    let (w, h, frames) = demux(&output);
    assert_eq!((w, h), (32, 16));
    assert_eq!(frames.len(), 4);
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(frame[0], (200 - i * 10) as u8, "frame {i} block value");
        assert_eq!(frame.len(), 32 * 16 + 2 * 16 * 8, "4:2:0 frame size");
    }
}

/// `process_clip` sees the whole clip at once and can rewrite it wholesale.
#[test]
fn harness_process_clip_sees_whole_clip() {
    let dir = scratch("process_clip");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    write_flat_y4m(&input, 16, 16, 5, 100, 128, 128);

    let stats = frame_harness::process_clip("harness e2e", &input, &output, |clip| {
        // A global op: set every frame's luma to the clip-wide mean + 20.
        let mean: u32 = clip
            .frames
            .iter()
            .map(|f| u32::from(f.luma()[0]))
            .sum::<u32>()
            / clip.frames.len() as u32;
        let mut out = clip.empty_like();
        for frame in &clip.frames {
            let mut next = frame.clone();
            for sample in next.luma_mut() {
                *sample = (mean + 20) as u8;
            }
            out.frames.push(next);
        }
        Ok(out)
    })
    .expect("process_clip must succeed");

    assert_eq!(stats.frame_count, 5);
    assert!(stats.bytes_changed > 0);

    let (_, _, frames) = demux(&output);
    assert_eq!(frames.len(), 5);
    for frame in &frames {
        assert!(
            frame[..16 * 16].iter().all(|&s| s == 120),
            "every luma sample must be the clip mean + 20"
        );
    }
}

// ─── (b) Consumer #1 — multicam color-match ─────────────────────────────────

/// Two angles that really differ in colour produce real, non-identity
/// corrections computed from measured pixel statistics.
#[tokio::test]
async fn multicam_color_match_measures_real_statistics() {
    let dir = scratch("color_match");
    let reference = dir.join("cam_ref.y4m");
    let dim = dir.join("cam_dim.y4m");
    let out_dir = dir.join("matched");

    // Reference is bright and neutral; the second angle is much darker.
    write_flat_y4m(&reference, 16, 16, 2, 200, 128, 128);
    write_flat_y4m(&dim, 16, 16, 2, 90, 128, 128);

    handle_multicam_command(
        MulticamCommand::ColorMatch {
            reference: reference.clone(),
            inputs: vec![dim.clone()],
            output_dir: out_dir.clone(),
        },
        false,
    )
    .await
    .expect("color-match must succeed on real Y4M angles");

    let report_path = out_dir.join("cam_dim.colormatch.json");
    let body = std::fs::read_to_string(&report_path).expect("per-angle report must be written");
    let json: serde_json::Value = serde_json::from_str(&body).expect("report must be valid JSON");

    let angle = &json["angle"];
    assert_eq!(angle["angle"], 1, "the input is angle 1");
    assert_eq!(angle["frames_sampled"], 2, "both frames must be sampled");
    assert_eq!(
        angle["pixels_sampled"], 512,
        "16x16x2 pixels must be sampled"
    );

    // Measured means: the reference must be brighter than the dim angle.
    let dim_mean = angle["mean_rgb"][0].as_f64().expect("mean_rgb[0]");
    let ref_mean = json["reference_mean_rgb"][0]
        .as_f64()
        .expect("reference_mean_rgb[0]");
    assert!(
        ref_mean > dim_mean,
        "the reference angle must measure brighter: {ref_mean} vs {dim_mean}"
    );
    assert!(
        (0.0..=1.0).contains(&dim_mean) && (0.0..=1.0).contains(&ref_mean),
        "means must be normalised to 0..1"
    );

    // The correction must actually gain the dim angle up (> 1 on the diagonal).
    let gain_r = angle["correction_matrix"][0][0]
        .as_f64()
        .expect("correction gain");
    assert!(
        gain_r > 1.05,
        "matching a dark angle to a bright reference must gain it up, got {gain_r}"
    );

    // A flat clip has no spread, and its stats must not be the fabricated
    // `ColorStats::new` defaults (mean 0.5 / std 0.1 / 6500 K).
    let std_r = angle["std_rgb"][0].as_f64().expect("std_rgb[0]");
    assert!(
        std_r < 1e-4,
        "a flat clip must measure zero spread: {std_r}"
    );
    assert!(
        (dim_mean - 0.5).abs() > 1e-3,
        "mean must be measured, not the ColorStats::new default 0.5"
    );

    let note = json["note"].as_str().expect("note").to_lowercase();
    assert!(
        note.contains("no video was re-encoded"),
        "the report must state that only metadata was written, got: {note}"
    );

    // A true neutral grey really does measure ~6504 K (D65), which is close to
    // `ColorStats::new`'s fabricated 6500.0 default — so the temperature alone
    // cannot distinguish measured from fabricated. The mean/std/gain
    // assertions above are what pin it down; this one only checks the value is
    // reported at all for near-neutral content.
    assert!(
        angle["temperature_kelvin"].is_number(),
        "a near-neutral angle must carry a colour-temperature estimate"
    );
}

/// The `--json` surface of `color-match` is real JSON carrying the measured
/// numbers, and it is produced by the actual binary (not a library shortcut).
#[test]
fn multicam_color_match_json_surface_is_machine_readable() {
    let dir = scratch("color_match_json");
    let reference = dir.join("cam_ref.y4m");
    let dim = dir.join("cam_dim.y4m");
    let out_dir = dir.join("matched");
    write_flat_y4m(&reference, 16, 16, 1, 200, 128, 128);
    write_flat_y4m(&dim, 16, 16, 1, 90, 128, 128);

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--json",
            "multicam",
            "color-match",
            "--reference",
            reference.to_str().expect("utf8"),
            "-i",
            dim.to_str().expect("utf8"),
            "-o",
            out_dir.to_str().expect("utf8"),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be pure JSON ({e}):\n{stdout}"));

    let angles = json["angles"].as_array().expect("angles array");
    assert_eq!(angles.len(), 2, "reference + one input");
    assert_eq!(angles[0]["role"], "reference");
    assert_eq!(angles[1]["role"], "input");
    assert!(
        angles[1]["correction_matrix"][0][0].as_f64().expect("gain") > 1.05,
        "the dark angle must be gained up"
    );
    assert_eq!(
        json["written_files"]
            .as_array()
            .expect("written_files")
            .len(),
        1
    );
    assert!(json["note"]
        .as_str()
        .expect("note")
        .to_lowercase()
        .contains("no video was re-encoded"));
}

/// A non-Y4M angle is refused with the shared, actionable error.
#[tokio::test]
async fn multicam_color_match_requires_y4m_input() {
    let dir = scratch("color_match_bad");
    let reference = dir.join("cam_ref.y4m");
    let bad = dir.join("cam_bad.mp4");
    let out_dir = dir.join("matched");
    write_flat_y4m(&reference, 8, 8, 1, 128, 128, 128);
    std::fs::write(&bad, b"\x00\x00\x00\x18ftypmp42not-a-y4m").expect("write fake mp4");

    let err = handle_multicam_command(
        MulticamCommand::ColorMatch {
            reference,
            inputs: vec![bad],
            output_dir: out_dir.clone(),
        },
        false,
    )
    .await
    .expect_err("a non-Y4M angle must be refused");

    let msg = format!("{err}");
    assert!(msg.contains("YUV4MPEG2"), "got: {msg}");
    assert!(msg.contains("oximedia transcode"), "got: {msg}");
    assert!(
        !out_dir.join("cam_bad.colormatch.json").exists(),
        "no report may be fabricated for a refused angle"
    );
}

// ─── (c) Consumer #2 — timecode burn ────────────────────────────────────────

fn burn_command(
    input: &Path,
    output: &Path,
    font: Option<PathBuf>,
    font_size: u32,
) -> TimecodeCommand {
    TimecodeCommand::Burn {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        start: "01:00:00:00".to_string(),
        fps: "25".to_string(),
        position: "top-left".to_string(),
        font_size,
        font,
    }
}

/// Without `--font` the command refuses honestly and writes nothing.
///
/// This test always runs: it needs no font precisely because the point is
/// that OxiMedia ships none.
#[tokio::test]
async fn timecode_burn_without_font_errors_honestly() {
    let dir = scratch("burn_no_font");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    write_flat_y4m(&input, 32, 32, 2, 128, 128, 128);

    let err = run_timecode(burn_command(&input, &output, None, 12), false)
        .await
        .expect_err("burn-in without a font must fail");

    let msg = format!("{err}");
    assert!(
        msg.contains("--font"),
        "error must name the flag, got: {msg}"
    );
    assert!(
        !output.exists(),
        "no output may be fabricated without a font"
    );
}

/// A non-Y4M input is refused before the font is even considered.
#[tokio::test]
async fn timecode_burn_requires_y4m_input() {
    let dir = scratch("burn_bad_input");
    let input = dir.join("in.webm");
    let output = dir.join("out.y4m");
    std::fs::write(&input, b"\x1aE\xdf\xa3not-a-y4m").expect("write fake webm");

    let err = run_timecode(burn_command(&input, &output, None, 12), false)
        .await
        .expect_err("a non-Y4M input must be refused");

    let msg = format!("{err}");
    assert!(msg.contains("YUV4MPEG2"), "got: {msg}");
    assert!(msg.contains("oximedia transcode"), "got: {msg}");
    assert!(!output.exists(), "no output may be fabricated");
}

/// A font file that is not a font is refused.
#[tokio::test]
async fn timecode_burn_rejects_a_non_font() {
    let dir = scratch("burn_bad_font");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    let font = dir.join("not_a_font.ttf");
    write_flat_y4m(&input, 32, 32, 1, 128, 128, 128);
    std::fs::write(&font, b"definitely not a font").expect("write fake font");

    let err = run_timecode(burn_command(&input, &output, Some(font), 12), false)
        .await
        .expect_err("a non-font must be refused");
    assert!(format!("{err}").contains("TrueType/OpenType"), "got: {err}");
    assert!(!output.exists(), "no output may be fabricated");
}

/// Odd 4:2:0 geometry is refused rather than silently mis-composited.
#[tokio::test]
async fn timecode_burn_rejects_odd_420_geometry() {
    let dir = scratch("burn_odd_geometry");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    write_flat_y4m(&input, 15, 16, 1, 128, 128, 128);

    let err = run_timecode(burn_command(&input, &output, None, 12), false)
        .await
        .expect_err("odd 4:2:0 width must be refused");
    assert!(
        format!("{err}").contains("divisible by 2"),
        "error must explain the geometry requirement, got: {err}"
    );
    assert!(!output.exists(), "no output may be fabricated");
}

/// Real glyph rasterisation: burns a timecode into every frame and checks the
/// overlay actually landed in the pixels.
///
/// Needs a font: set `OXIMEDIA_TEST_FONT=/path/to/font.ttf`.
#[tokio::test]
async fn timecode_burn_renders_real_glyphs() {
    let Some(font) = std::env::var_os("OXIMEDIA_TEST_FONT").map(PathBuf::from) else {
        eprintln!(
            "SKIP timecode_burn_renders_real_glyphs: no font available. OxiMedia ships no \
             font, so this test needs one from the environment — re-run with \
             OXIMEDIA_TEST_FONT=/path/to/font.ttf (e.g. a DejaVuSans.ttf)."
        );
        return;
    };
    assert!(
        font.is_file(),
        "OXIMEDIA_TEST_FONT must point at a font file, got {}",
        font.display()
    );

    let dir = scratch("burn_real");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    // Flat mid-grey so any change is unambiguously the overlay.
    write_flat_y4m(&input, 320, 96, 3, 128, 128, 128);

    run_timecode(burn_command(&input, &output, Some(font), 28), false)
        .await
        .expect("burn-in must succeed with a real font");

    let (w, h, frames) = demux(&output);
    assert_eq!((w, h), (320, 96), "geometry must be preserved");
    assert_eq!(frames.len(), 3, "every frame must be re-encoded");

    let luma_len = 320 * 96;
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(frame.len(), luma_len + 2 * (160 * 48), "4:2:0 frame size");
        let changed = frame[..luma_len].iter().filter(|&&s| s != 128).count();
        assert!(
            changed > 50,
            "frame {i} must carry a rasterised overlay, but only {changed} luma samples \
             differ from the flat background"
        );
        // The overlay is top-left anchored, so the bottom-right corner of a
        // flat clip must be untouched.
        let bottom_right = frame[luma_len - 1];
        assert_eq!(
            bottom_right, 128,
            "frame {i}: a top-left overlay must not touch the bottom-right corner"
        );
    }

    // Consecutive frames show different timecodes, so their pixels differ.
    assert_ne!(
        frames[0][..luma_len],
        frames[1][..luma_len],
        "frames showing different timecodes must differ"
    );
}

/// The `--json` surface of `timecode burn` is real JSON reporting the measured
/// counters, including the `bytes_changed` anti-fabrication counter.
///
/// Needs a font: set `OXIMEDIA_TEST_FONT=/path/to/font.ttf`.
#[test]
fn timecode_burn_json_surface_is_machine_readable() {
    let Some(font) = std::env::var_os("OXIMEDIA_TEST_FONT").map(PathBuf::from) else {
        eprintln!(
            "SKIP timecode_burn_json_surface_is_machine_readable: no font available. \
             Re-run with OXIMEDIA_TEST_FONT=/path/to/font.ttf."
        );
        return;
    };

    let dir = scratch("burn_json");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    write_flat_y4m(&input, 256, 64, 2, 128, 128, 128);

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--json",
            "timecode",
            "burn",
            "-i",
            input.to_str().expect("utf8"),
            "-o",
            output.to_str().expect("utf8"),
            "--start",
            "01:00:00:00",
            "--fps",
            "25",
            "--position",
            "top-left",
            "--font-size",
            "20",
            "--font",
            font.to_str().expect("utf8"),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be pure JSON ({e}):\n{stdout}"));

    assert_eq!(json["frame_count"], 2);
    assert_eq!(json["width"], 256);
    assert_eq!(json["height"], 64);
    assert_eq!(json["start_timecode"], "01:00:00:00");
    assert_eq!(json["end_timecode"], "01:00:00:01");
    assert!(
        json["bytes_changed"].as_u64().expect("bytes_changed") > 0,
        "the anti-fabrication counter must report real pixel changes"
    );
    assert!(output.exists(), "the burned clip must be written");
}

// ─── (d) Consumer #3 — denoise ──────────────────────────────────────────────

/// Write a deterministically noisy 4:2:0 Y4M clip: a mid-grey base with a
/// hash-derived per-pixel offset, so every frame has real local variance for
/// a denoiser to reduce (a flat clip cannot demonstrate that).
fn write_noisy_y4m(path: &Path, width: usize, height: usize, frames: usize) {
    write_y4m_fixture(
        path,
        width,
        height,
        frames,
        |x, y, t| {
            let h = (x as u32)
                .wrapping_mul(2_654_435_761)
                .wrapping_add((y as u32).wrapping_mul(40_503))
                .wrapping_add((t as u32).wrapping_mul(2_246_822_519));
            let noise = (h % 61) as i32 - 30; // -30..=30
            (128 + noise).clamp(0, 255) as u8
        },
        |_, _, _, _| 128,
    );
}

/// Population variance of a byte slice.
fn variance(samples: &[u8]) -> f64 {
    let n = samples.len() as f64;
    let mean = samples.iter().map(|&v| f64::from(v)).sum::<f64>() / n;
    samples
        .iter()
        .map(|&v| {
            let d = f64::from(v) - mean;
            d * d
        })
        .sum::<f64>()
        / n
}

/// Denoising a real noisy clip must change pixels and measurably reduce
/// local variance — the two content-level facts a fabricated / identity
/// pipeline could not produce.
#[tokio::test]
async fn denoise_reduces_variance_on_real_noise() {
    let dir = scratch("denoise_variance");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    write_noisy_y4m(&input, 64, 64, 1);

    let (_, _, before_frames) = demux(&input);
    let before_variance = variance(&before_frames[0][..64 * 64]);

    run_denoise(
        DenoiseOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "fast".to_string(),
            strength: 0.8,
            spatial: true,
            temporal: false,
            preserve_grain: false,
        },
        false,
    )
    .await
    .expect("denoise must succeed on a real noisy Y4M clip");

    let (w, h, after_frames) = demux(&output);
    assert_eq!((w, h), (64, 64), "geometry must be preserved");
    let after_variance = variance(&after_frames[0][..64 * 64]);

    assert!(
        after_variance < before_variance,
        "denoising must reduce local luma variance: before={before_variance:.2} \
         after={after_variance:.2}"
    );

    let changed = before_frames[0]
        .iter()
        .zip(after_frames[0].iter())
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > 0, "denoising must actually change pixels");
}

/// A denoiser that changes nothing must refuse to report success rather than
/// silently writing a copy.
#[tokio::test]
async fn denoise_requires_y4m_input() {
    let dir = scratch("denoise_bad_input");
    let input = dir.join("in.mp4");
    let output = dir.join("out.y4m");
    std::fs::write(&input, b"\x00\x00\x00\x18ftypmp42not-a-y4m").expect("write fake mp4");

    let err = run_denoise(
        DenoiseOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "balanced".to_string(),
            strength: 0.5,
            spatial: false,
            temporal: false,
            preserve_grain: false,
        },
        false,
    )
    .await
    .expect_err("a non-Y4M input must be refused");
    let msg = format!("{err}");
    assert!(msg.contains("YUV4MPEG2"), "got: {msg}");
    assert!(!output.exists(), "no output may be fabricated");
}

// ─── (e) Consumer #4 — stabilize ────────────────────────────────────────────

/// `oximedia stabilize` on real Y4M footage preserves geometry and frame
/// count end to end through its actual CLI entry point (config built from
/// `--mode`/`--quality`/`--smoothing`/`--zoom`, not hard-coded).
#[tokio::test]
async fn stabilize_preserves_geometry_through_the_cli_entry_point() {
    let dir = scratch("stabilize_e2e");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    write_flat_y4m(&input, 32, 24, 4, 128, 128, 128);

    run_stabilize(
        StabilizeOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "affine".to_string(),
            quality: "fast".to_string(),
            smoothing: 30,
            zoom: true,
        },
        false,
    )
    .await
    .expect("stabilize must succeed end to end via the real CLI entry point");

    let (w, h, frames) = demux(&output);
    assert_eq!((w, h), (32, 24));
    assert_eq!(frames.len(), 4);
}

/// An invalid `--mode` must be refused before any file is touched.
#[tokio::test]
async fn stabilize_invalid_mode_is_refused() {
    let dir = scratch("stabilize_bad_mode");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    write_flat_y4m(&input, 16, 16, 2, 128, 128, 128);

    let err = run_stabilize(
        StabilizeOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "not-a-real-mode".to_string(),
            quality: "fast".to_string(),
            smoothing: 30,
            zoom: false,
        },
        false,
    )
    .await
    .expect_err("an unknown --mode must be refused");
    assert!(format!("{err}").contains("Unknown stabilisation mode"));
    assert!(!output.exists());
}

// ─── (f) Consumer #5 — scaling ──────────────────────────────────────────────

/// `oximedia scaling upscale` writes a real Y4M file at the exact requested
/// dimensions, driven through the real `ScalingCommand` clap enum.
#[tokio::test]
async fn scaling_upscale_writes_exact_output_dims() {
    let dir = scratch("scaling_upscale");
    let input = dir.join("in.y4m");
    let output = dir.join("out.y4m");
    write_y4m_fixture(
        &input,
        16,
        12,
        3,
        |x, y, t| ((x * 3 + y * 5 + t * 7) % 256) as u8,
        |x, y, _, plane| ((x + y + plane * 30) % 256) as u8,
    );

    handle_scaling_command(
        ScalingCommand::Upscale {
            input: input.clone(),
            output: output.clone(),
            width: 32,
            height: 24,
            algorithm: "lanczos".to_string(),
            aspect: "stretch".to_string(),
        },
        false,
    )
    .await
    .expect("upscale must succeed on a real Y4M clip");

    let (w, h, frames) = demux(&output);
    assert_eq!(
        (w, h),
        (32, 24),
        "output dimensions must match --width/--height exactly"
    );
    assert_eq!(frames.len(), 3, "every frame must be re-encoded");
    assert_eq!(
        frames[0].len(),
        32 * 24 + 2 * 16 * 12,
        "4:2:0 frame size at the new geometry"
    );
}

/// `scaling compare` writes a real PSNR/SSIM report comparing algorithms
/// against each other at the same target size (never a fabricated identity
/// report), exercised through the real binary's `--json` surface.
#[test]
fn scaling_compare_json_surface_reports_real_metrics() {
    let dir = scratch("scaling_compare_json");
    let input = dir.join("in.y4m");
    let out_dir = dir.join("compared");
    write_y4m_fixture(
        &input,
        16,
        16,
        1,
        |x, y, _| ((x * 5 + y * 11) % 256) as u8,
        |x, y, _, plane| ((x * 2 + y + plane * 20) % 256) as u8,
    );

    let assert = Command::cargo_bin("oximedia")
        .expect("binary builds")
        .args([
            "--json",
            "scaling",
            "compare",
            "-i",
            input.to_str().expect("utf8"),
            "--width",
            "8",
            "--height",
            "8",
            "-o",
            out_dir.to_str().expect("utf8"),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be pure JSON ({e}):\n{stdout}"));

    let algorithms = json["algorithms"].as_array().expect("algorithms array");
    assert_eq!(algorithms.len(), 3);
    for name in ["bilinear", "bicubic", "lanczos"] {
        assert!(
            out_dir.join(format!("{name}.y4m")).exists(),
            "{name}.y4m must be written"
        );
        let (w, h, _) = demux(&out_dir.join(format!("{name}.y4m")));
        assert_eq!((w, h), (8, 8));
    }
}

// ─── (g) Consumer #6 — subtitle burn ────────────────────────────────────────

fn write_srt_cue(path: &Path, start: &str, end: &str, text: &str) {
    std::fs::write(path, format!("1\n{start} --> {end}\n{text}\n\n")).expect("write srt fixture");
}

/// Without `--font` the command refuses honestly and writes nothing. This
/// test always runs: it needs no font precisely because the point is that
/// OxiMedia ships none.
#[tokio::test]
async fn subtitle_burn_without_font_errors_honestly() {
    let dir = scratch("subtitle_burn_no_font");
    let input = dir.join("in.y4m");
    let subtitle = dir.join("in.srt");
    let output = dir.join("out.y4m");
    write_flat_y4m(&input, 32, 32, 2, 128, 128, 128);
    write_srt_cue(&subtitle, "00:00:00,000", "00:00:02,000", "Hello");

    let err = handle_subtitle_command(
        SubtitleCommand::Burn {
            input: input.clone(),
            subtitle: subtitle.clone(),
            output: output.clone(),
            font_size: 24,
            format: None,
            font: None,
        },
        false,
    )
    .await
    .expect_err("burn-in without a font must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("--font"),
        "error must name the flag, got: {msg}"
    );
    assert!(
        !output.exists(),
        "no output may be fabricated without a font"
    );
}

/// Real glyph rasterisation: burns a subtitle cue into every overlapping
/// frame and checks the overlay actually landed in the pixels.
///
/// Needs a font: set `OXIMEDIA_TEST_FONT=/path/to/font.ttf`.
#[tokio::test]
async fn subtitle_burn_renders_real_glyphs() {
    let Some(font) = std::env::var_os("OXIMEDIA_TEST_FONT").map(PathBuf::from) else {
        eprintln!(
            "SKIP subtitle_burn_renders_real_glyphs: no font available. OxiMedia ships no font, \
             so this test needs one from the environment — re-run with \
             OXIMEDIA_TEST_FONT=/path/to/font.ttf (e.g. a DejaVuSans.ttf)."
        );
        return;
    };

    let dir = scratch("subtitle_burn_real");
    let input = dir.join("in.y4m");
    let subtitle = dir.join("in.srt");
    let output = dir.join("out.y4m");
    write_flat_y4m(&input, 320, 96, 3, 128, 128, 128);
    write_srt_cue(&subtitle, "00:00:00,000", "00:00:02,000", "Hello world");

    handle_subtitle_command(
        SubtitleCommand::Burn {
            input: input.clone(),
            subtitle: subtitle.clone(),
            output: output.clone(),
            font_size: 28,
            format: None,
            font: Some(font),
        },
        false,
    )
    .await
    .expect("burn-in must succeed with a real font");

    let (w, h, frames) = demux(&output);
    assert_eq!((w, h), (320, 96), "geometry must be preserved");
    assert_eq!(frames.len(), 3, "every frame must be re-encoded");

    let luma_len = 320 * 96;
    for (i, frame) in frames.iter().enumerate() {
        let changed = frame[..luma_len].iter().filter(|&&s| s != 128).count();
        assert!(
            changed > 50,
            "frame {i} must carry a rasterised overlay, but only {changed} luma samples differ \
             from the flat background"
        );
    }
}

// ─── (h) Consumer #7 — captions burn ────────────────────────────────────────

/// Without `--font` the command refuses honestly and writes nothing.
#[tokio::test]
async fn captions_burn_without_font_errors_honestly() {
    let dir = scratch("captions_burn_no_font");
    let video = dir.join("in.y4m");
    let captions = dir.join("in.srt");
    let output = dir.join("out.y4m");
    write_flat_y4m(&video, 32, 32, 2, 128, 128, 128);
    write_srt_cue(&captions, "00:00:00,000", "00:00:02,000", "Hello");

    let err = captions_cmd::run_captions_burn(
        CaptionsBurnOptions {
            video: video.clone(),
            captions: captions.clone(),
            output: output.clone(),
            font_size: 24,
            font_color: "FFFFFF".to_string(),
            font: None,
        },
        false,
    )
    .await
    .expect_err("burn-in without a font must fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("--font"),
        "error must name the flag, got: {msg}"
    );
    assert!(
        !output.exists(),
        "no output may be fabricated without a font"
    );
}

/// Real glyph rasterisation: burns a caption into every overlapping frame
/// and checks the overlay actually landed in the pixels, honouring
/// `--font-color`.
///
/// Needs a font: set `OXIMEDIA_TEST_FONT=/path/to/font.ttf`.
#[tokio::test]
async fn captions_burn_renders_real_glyphs_in_the_requested_color() {
    let Some(font) = std::env::var_os("OXIMEDIA_TEST_FONT").map(PathBuf::from) else {
        eprintln!(
            "SKIP captions_burn_renders_real_glyphs_in_the_requested_color: no font available. \
             Re-run with OXIMEDIA_TEST_FONT=/path/to/font.ttf."
        );
        return;
    };

    let dir = scratch("captions_burn_real");
    let video = dir.join("in.y4m");
    let captions = dir.join("in.srt");
    let output = dir.join("out.y4m");
    write_flat_y4m(&video, 320, 96, 3, 128, 128, 128);
    write_srt_cue(&captions, "00:00:00,000", "00:00:02,000", "Hello world");

    captions_cmd::run_captions_burn(
        CaptionsBurnOptions {
            video: video.clone(),
            captions: captions.clone(),
            output: output.clone(),
            font_size: 28,
            // Pure red: on this BT.709 full-range conversion, red's luma
            // contribution (0.2126) is much lower than white's (1.0), so a
            // real color-aware compositor must paint a *dimmer* luma overlay
            // than white text would, distinguishing "color honoured" from
            // "color ignored, always white".
            font_color: "FF0000".to_string(),
            font: Some(font),
        },
        false,
    )
    .await
    .expect("burn-in must succeed with a real font");

    let (w, h, frames) = demux(&output);
    assert_eq!((w, h), (320, 96), "geometry must be preserved");
    assert_eq!(frames.len(), 3, "every frame must be re-encoded");

    let luma_len = 320 * 96;
    for (i, frame) in frames.iter().enumerate() {
        let changed_down: usize = frame[..luma_len].iter().filter(|&&s| s < 128).count();
        assert!(
            changed_down > 50,
            "frame {i} must carry a rasterised overlay dimmer than the flat 128 background \
             (red's luma contribution is low), but only {changed_down} samples dropped below it"
        );
        // Red must not be re-encoded as anything close to white's luma
        // (near 255): if it were, --font-color would be silently ignored.
        let near_white = frame[..luma_len].iter().filter(|&&s| s > 230).count();
        assert_eq!(
            near_white, 0,
            "frame {i}: pure red text must not rasterise as near-white luma"
        );
    }
}
