//! Extended tests for the analysis, generation, validation and database
//! helpers that live in sibling modules.

use super::*;

fn tmp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("oximedia-bench-sequences-ext-{name}"))
}

// ------------------------------------------------------------------
// SequenceGenerator::generate_* -- distinguishing statistical properties
// ------------------------------------------------------------------
//
// `SequenceGenerator::new` and the private `rgb_to_yuv420_bt601` helper are
// covered by `super::generator`'s own `mod tests`, which can see the module's
// private items.

#[test]
fn test_generate_solid_color_white_and_black() {
    let gen = SequenceGenerator::new(4, 4, Rational::new(30, 1));

    let white = gen
        .generate_solid_color(2, (255, 255, 255))
        .expect("generate_solid_color should succeed");
    assert_eq!(white.len(), 2);
    for frame in &white {
        assert!(frame.planes[0].data.iter().all(|&v| v == 235));
        assert!(frame.planes[1].data.iter().all(|&v| v == 128));
        assert!(frame.planes[2].data.iter().all(|&v| v == 128));
    }
    // Zero temporal variance: every frame is byte-identical.
    assert_eq!(white[0].planes[0].data, white[1].planes[0].data);

    let black = gen
        .generate_solid_color(1, (0, 0, 0))
        .expect("generate_solid_color should succeed");
    assert!(black[0].planes[0].data.iter().all(|&v| v == 16));
}

#[test]
fn test_generate_gradient_spans_black_to_white_and_is_static() {
    let gen = SequenceGenerator::new(8, 8, Rational::new(30, 1));
    let frames = gen
        .generate_gradient(3)
        .expect("generate_gradient should succeed");
    assert_eq!(frames.len(), 3);

    let y = &frames[0].planes[0].data;
    // Top-left corner (col=0, row=0): diag=0 -> 16 (black).
    assert_eq!(y[0], 16);
    // Bottom-right corner (col=7, row=7): diag=max -> 235 (white).
    assert_eq!(y[8 * 8 - 1], 235);

    // Zero temporal variance: every frame is identical -- distinguishes
    // a static gradient background from motion/noise.
    assert_eq!(frames[0].planes[0].data, frames[1].planes[0].data);
    assert_eq!(frames[1].planes[0].data, frames[2].planes[0].data);
}

#[test]
fn test_generate_noise_zero_level_is_flat_mid_gray() {
    let gen = SequenceGenerator::new(8, 8, Rational::new(30, 1));
    let frames = gen
        .generate_noise(2, 0.0)
        .expect("generate_noise should succeed");
    for frame in &frames {
        assert!(frame.planes[0].data.iter().all(|&v| v == 128));
    }
}

#[test]
fn test_generate_noise_variance_grows_with_level() {
    let gen = SequenceGenerator::new(32, 32, Rational::new(30, 1));
    let low = gen
        .generate_noise(1, 0.1)
        .expect("generate_noise should succeed");
    let high = gen
        .generate_noise(1, 1.0)
        .expect("generate_noise should succeed");

    fn stddev(data: &[u8]) -> f64 {
        let mean = data.iter().map(|&v| f64::from(v)).sum::<f64>() / data.len() as f64;
        let variance = data
            .iter()
            .map(|&v| (f64::from(v) - mean).powi(2))
            .sum::<f64>()
            / data.len() as f64;
        variance.sqrt()
    }

    let low_stddev = stddev(&low[0].planes[0].data);
    let high_stddev = stddev(&high[0].planes[0].data);
    assert!(
        high_stddev > low_stddev,
        "high noise_level ({high_stddev}) should have more variance than low ({low_stddev})"
    );
}

#[test]
fn test_generate_noise_frames_differ_temporally() {
    let gen = SequenceGenerator::new(8, 8, Rational::new(30, 1));
    let frames = gen
        .generate_noise(2, 1.0)
        .expect("generate_noise should succeed");
    assert_ne!(frames[0].planes[0].data, frames[1].planes[0].data);
}

#[test]
fn test_generate_motion_moves_by_velocity() {
    let gen = SequenceGenerator::new(64, 64, Rational::new(30, 1));
    let velocity = (4.0, 0.0);
    let frames = gen
        .generate_motion(5, velocity)
        .expect("generate_motion should succeed");
    assert_eq!(frames.len(), 5);

    // The block's top row is always row 0 here (velocity.1 == 0.0), so
    // the first bright (235) sample in row-major order sits at
    // (row=0, col=pos_x) -- i.e. its flat index IS pos_x.
    fn leftmost_bright_col(frame: &VideoFrame) -> usize {
        frame.planes[0]
            .data
            .iter()
            .position(|&v| v == 235)
            .expect("motion frame should contain a bright block")
    }

    let col0 = leftmost_bright_col(&frames[0]);
    let col4 = leftmost_bright_col(&frames[4]);
    assert_eq!(
        col4 as i64 - col0 as i64,
        (velocity.0 * 4.0) as i64,
        "block should have moved exactly velocity.0 * frame_idx pixels"
    );
}

#[test]
fn test_generate_checkerboard_custom_block_size() {
    let gen = SequenceGenerator::new(16, 16, Rational::new(30, 1));
    let frames = gen
        .generate_checkerboard(1, 2)
        .expect("generate_checkerboard should succeed");
    let y = &frames[0].planes[0].data;

    // block_size=2: pixel(0,0) white, pixel(2,0) black -- a boundary
    // the free function's fixed CELL=8 pattern would not produce at
    // these coordinates (both would still be white there).
    assert_eq!(y[0], 235, "pixel(0,0) should be white");
    assert_eq!(y[2], 16, "pixel(2,0) should be black");
    assert_eq!(y[4], 235, "pixel(4,0) should be white again");
}

#[test]
fn test_generate_all_generators_reject_zero_dimensions() {
    let gen = SequenceGenerator::new(0, 0, Rational::new(30, 1));
    assert!(gen.generate_solid_color(1, (0, 0, 0)).is_err());
    assert!(gen.generate_gradient(1).is_err());
    assert!(gen.generate_noise(1, 0.5).is_err());
    assert!(gen.generate_motion(1, (1.0, 1.0)).is_err());
    assert!(gen.generate_checkerboard(1, 4).is_err());
}

// ------------------------------------------------------------------
// SequenceAnalyzer::detect_scene_changes
// ------------------------------------------------------------------

#[test]
fn test_detect_scene_changes_flags_real_cut_and_ignores_similar_frames() {
    let gen = SequenceGenerator::new(16, 16, Rational::new(30, 1));
    let mut frames = gen
        .generate_solid_color(2, (128, 128, 128))
        .expect("generate_solid_color should succeed");
    let mut cut_frame = gen
        .generate_solid_color(1, (255, 255, 255))
        .expect("generate_solid_color should succeed");
    frames.append(&mut cut_frame);

    let cuts = SequenceAnalyzer::detect_scene_changes(&frames)
        .expect("detect_scene_changes should succeed");
    // frames[0] vs frames[1]: identical solid gray -> no cut.
    // frames[1] vs frames[2]: gray -> white -> a real cut at index 2.
    assert_eq!(cuts, vec![2]);
}

#[test]
fn test_detect_scene_changes_empty_and_single_frame_have_no_cuts() {
    assert_eq!(
        SequenceAnalyzer::detect_scene_changes(&[]).expect("should succeed"),
        Vec::<usize>::new()
    );

    let gen = SequenceGenerator::new(4, 4, Rational::new(30, 1));
    let one_frame = gen
        .generate_solid_color(1, (0, 0, 0))
        .expect("generate_solid_color should succeed");
    assert_eq!(
        SequenceAnalyzer::detect_scene_changes(&one_frame).expect("should succeed"),
        Vec::<usize>::new()
    );
}

#[test]
fn test_sequence_database() {
    let mut db = SequenceDatabase::new();

    let seq = TestSequence::new(
        "test_1080p",
        tmp_path("test.y4m"),
        1920,
        1080,
        Rational::new(30, 1),
    );

    db.add(seq);

    assert_eq!(db.all().len(), 1);
    assert!(db.get_by_name("test_1080p").is_some());
    assert_eq!(db.get_by_resolution(1920, 1080).len(), 1);
}

#[test]
fn test_database_statistics() {
    let mut db = SequenceDatabase::new();

    db.add(
        TestSequence::new(
            "seq1",
            tmp_path("seq1.y4m"),
            1920,
            1080,
            Rational::new(30, 1),
        )
        .with_frame_count(300),
    );

    db.add(
        TestSequence::new(
            "seq2",
            tmp_path("seq2.y4m"),
            1280,
            720,
            Rational::new(30, 1),
        )
        .with_frame_count(150),
    );

    let stats = db.statistics();
    assert_eq!(stats.total_sequences, 2);
    assert_eq!(stats.total_frames, 450);
}

#[test]
fn test_motion_distribution() {
    let dist = MotionDistribution {
        low_motion_percentage: 60.0,
        medium_motion_percentage: 30.0,
        high_motion_percentage: 10.0,
    };

    assert_eq!(dist.low_motion_percentage, 60.0);
}

#[test]
fn test_validation_result() {
    let result = ValidationResult {
        is_valid: true,
        errors: vec![],
        warnings: vec!["Warning: Low frame count".to_string()],
    };

    assert!(result.is_valid);
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn test_hdr_metadata() {
    let metadata = HdrMetadata {
        transfer_function: "PQ".to_string(),
        color_primaries: "BT.2020".to_string(),
        master_display: None,
        content_light_level: Some(ContentLightLevel {
            max_cll: 1000,
            max_fall: 400,
        }),
    };

    assert_eq!(metadata.transfer_function, "PQ");
    assert!(metadata.content_light_level.is_some());
}
