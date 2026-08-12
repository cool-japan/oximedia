//! Smoke tests for newly-wired orphan modules in `oximedia-audio`.
//!
//! Each test exercises the public API of one or more modules that were
//! previously not declared in `lib.rs`.  These are minimal sanity checks:
//! construction, a short processing run, and basic postcondition assertions.

// ─── aac ─────────────────────────────────────────────────────────────────────

#[test]
fn test_aac_object_type_id() {
    use oximedia_audio::aac::AacObjectType;
    assert_eq!(AacObjectType::AacLc.object_type_id(), 2);
    assert_eq!(AacObjectType::HeAacV1.object_type_id(), 5);
    assert_eq!(AacObjectType::HeAacV2.object_type_id(), 29);
}

/// AAC decoding is not implemented: every decode entry point must fail closed
/// rather than hand back fabricated PCM.
#[test]
fn test_aac_decoder_decode_paths_error() {
    use oximedia_audio::aac::AacDecoder;
    use oximedia_audio::AudioError;

    let mut dec = AacDecoder::new();
    assert_eq!(AacDecoder::codec_name(), "aac");

    let err = dec
        .send_packet(&[], 0)
        .expect_err("AAC decoding must not succeed");
    assert!(
        matches!(err, AudioError::UnsupportedFormat(_)),
        "expected UnsupportedFormat, got {err:?}"
    );

    assert!(
        dec.receive_frame().is_err(),
        "no frame may ever be produced by the AAC stub"
    );
    assert_eq!(dec.decode_errors(), 1);
}

// ─── alac ────────────────────────────────────────────────────────────────────

#[test]
fn test_alac_decoder_new_no_config() {
    use oximedia_audio::alac::AlacDecoder;
    use oximedia_audio::traits::AudioDecoder;
    // Without a magic cookie there is no config, but construction must succeed.
    let dec = AlacDecoder::new();
    // No config yet → channel_layout returns None.
    assert!(dec.channel_layout().is_none());
}

#[test]
fn test_alac_magic_cookie_too_short() {
    use oximedia_audio::alac::AlacMagicCookie;
    // 23 bytes is one byte short of the 24-byte minimum.
    let result = AlacMagicCookie::parse(&[0u8; 23]);
    assert!(result.is_err(), "parse must fail for too-short cookie");
}

// ─── auto_gain ───────────────────────────────────────────────────────────────

#[test]
fn test_auto_gain_controller_default_config() {
    use oximedia_audio::auto_gain::{AutoGainConfig, AutoGainController};
    let ctrl = AutoGainController::new(AutoGainConfig::default(), 48_000)
        .expect("default config must be valid");
    // Initial gain should be finite.
    let g = ctrl.current_gain_db();
    assert!(g.is_finite(), "initial gain_db must be finite, got {g}");
}

#[test]
fn test_auto_gain_controller_process_is_finite() {
    use oximedia_audio::auto_gain::{AutoGainConfig, AutoGainController};
    let mut ctrl =
        AutoGainController::new(AutoGainConfig::default(), 48_000).expect("valid config");
    let input: Vec<f32> = (0..480).map(|i| (i as f32 * 0.1_f32).sin() * 0.5).collect();
    let output = ctrl.process(&input);
    assert_eq!(output.len(), input.len());
    for s in &output {
        assert!(s.is_finite(), "auto_gain output must be finite");
    }
}

// ─── auto_level ──────────────────────────────────────────────────────────────

#[test]
fn test_auto_level_processes_block() {
    use oximedia_audio::auto_level::{AutoLevel, AutoLevelConfig};
    let mut al = AutoLevel::new(48_000, AutoLevelConfig::default());
    let input: Vec<f32> = (0..512)
        .map(|i| (i as f32 * 0.05_f32).sin() * 0.3)
        .collect();
    let output: Vec<f32> = input.iter().map(|&s| al.process_sample(s)).collect();
    assert_eq!(output.len(), input.len());
    for s in &output {
        assert!(s.is_finite());
    }
}

// ─── channel_mixer ───────────────────────────────────────────────────────────

#[test]
fn test_channel_mixer_stereo_to_mono() {
    use oximedia_audio::channel_mixer::{ChannelMixer, MixMatrix};
    let matrix = MixMatrix::stereo_to_mono();
    let mixer = ChannelMixer::new(matrix);
    // 4 interleaved stereo samples → 2 mono samples.
    let stereo = vec![0.8_f32, 0.4_f32, 0.6_f32, 0.2_f32];
    let mono = mixer.process_interleaved(&stereo);
    assert_eq!(mono.len(), 2);
    let expected0 = (0.8_f32 + 0.4_f32) * 0.5;
    let expected1 = (0.6_f32 + 0.2_f32) * 0.5;
    assert!(
        (mono[0] - expected0).abs() < 1e-5,
        "mono[0]={} expected≈{expected0}",
        mono[0]
    );
    assert!(
        (mono[1] - expected1).abs() < 1e-5,
        "mono[1]={} expected≈{expected1}",
        mono[1]
    );
}

#[test]
fn test_channel_mixer_mono_to_stereo() {
    use oximedia_audio::channel_mixer::{ChannelMixer, MixMatrix};
    let matrix = MixMatrix::mono_to_stereo();
    let mixer = ChannelMixer::new(matrix);
    let mono = vec![0.5_f32, 0.3_f32];
    let stereo = mixer.process_interleaved(&mono);
    assert_eq!(stereo.len(), 4);
    assert!((stereo[0] - 0.5).abs() < 1e-5); // L
    assert!((stereo[1] - 0.5).abs() < 1e-5); // R
    assert!((stereo[2] - 0.3).abs() < 1e-5);
    assert!((stereo[3] - 0.3).abs() < 1e-5);
}

// ─── convolution_reverb ──────────────────────────────────────────────────────

#[test]
fn test_convolution_reverb_output_length() {
    use oximedia_audio::convolution_reverb::{ConvolutionReverb, ReverbConfig};
    // A simple decaying impulse response.
    let ir_len = 128_usize;
    let ir: Vec<f32> = (0..ir_len)
        .map(|i| (-3.0 * i as f32 / ir_len as f32).exp() * if i == 0 { 1.0 } else { 0.5 })
        .collect();
    let config = ReverbConfig { wet: 0.3, dry: 0.7 };
    let mut reverb = ConvolutionReverb::new(&ir, 64, config).expect("valid IR");
    let input: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05_f32).sin()).collect();
    let output = reverb.process(&input);
    assert_eq!(output.len(), input.len());
    for s in &output {
        assert!(s.is_finite());
    }
}

// ─── crossover ───────────────────────────────────────────────────────────────

#[test]
fn test_two_way_crossover_output_lengths() {
    use oximedia_audio::crossover::TwoWayCrossover;
    let mut xover = TwoWayCrossover::new(200.0, 48_000.0);
    let input: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05_f32).sin()).collect();
    let bands = xover.process_block(&input);
    assert_eq!(bands.low.len(), 256);
    assert_eq!(bands.high.len(), 256);
    for s in bands.low.iter().chain(bands.high.iter()) {
        assert!(s.is_finite());
    }
}

#[test]
fn test_three_way_crossover_output_lengths() {
    use oximedia_audio::crossover::ThreeWayCrossover;
    let mut xover = ThreeWayCrossover::new(200.0, 2000.0, 48_000.0);
    let input: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05_f32).sin()).collect();
    let bands = xover.process_block(&input);
    assert_eq!(bands.low.len(), 256);
    assert_eq!(bands.mid.len(), 256);
    assert_eq!(bands.high.len(), 256);
}

// ─── dolby_atmos ─────────────────────────────────────────────────────────────

#[test]
fn test_dolby_atmos_object_position_clamped() {
    use oximedia_audio::dolby_atmos::{AtmosObject, ObjectPosition};
    // Out-of-range values get clamped to [0, 1].
    let pos = ObjectPosition::new(-0.5, 1.5, 0.5);
    assert_eq!(pos.x, 0.0);
    assert_eq!(pos.y, 1.0);
    assert!((pos.z - 0.5).abs() < 1e-6);

    let obj = AtmosObject::new(0, 0).with_position(pos);
    let p = obj.position;
    assert!((0.0..=1.0).contains(&p.x));
    assert!((0.0..=1.0).contains(&p.y));
    assert!((0.0..=1.0).contains(&p.z));
}

// ─── drc ─────────────────────────────────────────────────────────────────────

#[test]
fn test_drc_limiter_clips_above_threshold() {
    use oximedia_audio::drc::DrcLimiter;
    let threshold = 0.9_f32;
    let mut limiter = DrcLimiter::new(threshold, 512);
    let mut samples: Vec<f32> = vec![1.5, -1.8, 0.3, 0.9, 1.1, -1.2];
    limiter.process(&mut samples);
    for s in &samples {
        assert!(
            s.abs() <= threshold + 1e-3,
            "sample {s} exceeds threshold {threshold} after DRC"
        );
    }
}

#[test]
fn test_drc_limiter_low_amplitude_passes() {
    use oximedia_audio::drc::DrcLimiter;
    let mut limiter = DrcLimiter::new(0.9, 128);
    let input: Vec<f32> = (0..256)
        .map(|i| (i as f32 * 0.01_f32).sin() * 0.5)
        .collect();
    let mut samples = input.clone();
    limiter.process(&mut samples);
    for s in &samples {
        assert!(s.is_finite());
    }
}

// ─── level_histogram ─────────────────────────────────────────────────────────

#[test]
fn test_level_histogram_basic_stats() {
    use oximedia_audio::level_histogram::{LevelHistogram, LevelHistogramConfig};
    let config = LevelHistogramConfig::default();
    let mut hist = LevelHistogram::new(config);
    // Sine wave at half amplitude.
    let samples: Vec<f32> = (0..4096)
        .map(|i| (i as f32 * 0.01_f32).sin() * 0.5)
        .collect();
    hist.process(&samples);
    let stats = hist.statistics();
    assert!(
        stats.peak_db <= 0.0,
        "peak_db={} must be ≤ 0",
        stats.peak_db
    );
    assert!(
        stats.peak_db > -10.0,
        "peak_db={} unreasonably low",
        stats.peak_db
    );
    assert_eq!(stats.clipped_samples, 0, "no clipping expected");
}

// ─── loudness_history ────────────────────────────────────────────────────────

#[test]
fn test_loudness_history_tracker_push_and_stats() {
    use oximedia_audio::loudness_history::{LoudnessHistoryConfig, LoudnessHistoryTracker};
    let mut tracker = LoudnessHistoryTracker::new(LoudnessHistoryConfig::default());
    for lufs in [-23.0_f64, -22.5, -23.5, -24.0, -21.0, -25.0, -22.0] {
        tracker.push_block(lufs);
    }
    let lra = tracker.loudness_range();
    assert!(lra >= 0.0, "LRA must be non-negative, got {lra}");
    let stats = tracker.stats();
    assert_eq!(stats.block_count, 7);
    assert!(stats.min_lufs <= stats.max_lufs);
}

// ─── noise_reduction ─────────────────────────────────────────────────────────

#[test]
fn test_noise_reduction_learn_profile_and_process() {
    use oximedia_audio::noise_reduction::{NoiseReducer, NoiseReductionConfig};
    let mut reducer = NoiseReducer::new(NoiseReductionConfig::default());
    assert!(!reducer.has_profile());
    let noise: Vec<f32> = (0..4096)
        .map(|i| (i as f32 * 0.003_f32).sin() * 0.05)
        .collect();
    reducer.learn_noise_profile(&noise);
    assert!(reducer.has_profile());

    let audio: Vec<f32> = (0..2048)
        .map(|i| (i as f32 * 0.05_f32).sin() * 0.4)
        .collect();
    let result = reducer.process(&audio).expect("process must succeed");
    assert_eq!(result.len(), audio.len());
    for s in &result {
        assert!(s.is_finite());
    }
}

// ─── r128 ────────────────────────────────────────────────────────────────────

#[test]
fn test_r128_meter_integrated_lufs_after_signal() {
    use oximedia_audio::r128::R128Meter;
    let mut meter = R128Meter::new(48_000);
    let block: Vec<f32> = (0..480)
        .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48_000.0).sin() * 0.224)
        .collect();
    // Feed ~4.8 s worth of signal (100 × 480 samples at 48 kHz).
    for _ in 0..100 {
        meter.add_block(&block);
    }
    let lufs = meter.integrated_lufs();
    assert!(
        lufs < 0.0,
        "LUFS must be negative for a real signal, got {lufs}"
    );
}

// ─── spectral_analysis ───────────────────────────────────────────────────────

#[test]
fn test_spectral_analyzer_centroid_finite() {
    use oximedia_audio::spectral_analysis::{SpectralAnalysisConfig, SpectralAnalyzer};
    let config = SpectralAnalysisConfig::new(48_000, 2048, 1024);
    let mut analyzer = SpectralAnalyzer::new(config).expect("valid config");
    let samples: Vec<f32> = (0..2048)
        .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48_000.0).sin())
        .collect();
    let frame = analyzer.analyze(&samples);
    assert!(frame.centroid_hz.is_finite(), "centroid must be finite");
    assert!(frame.centroid_hz >= 0.0);
    assert!(frame.flatness >= 0.0 && frame.flatness <= 1.0 + 1e-9);
}

// ─── stft ────────────────────────────────────────────────────────────────────

#[test]
fn test_stft_forward_produces_frames() {
    use oximedia_audio::stft::{Stft, StftConfig, WindowType};
    let config = StftConfig::new(512, 128, WindowType::Hann);
    let mut stft = Stft::new(config);
    let signal: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.01_f32).sin()).collect();
    let frames = stft.forward(&signal);
    assert!(!frames.is_empty(), "must produce at least one frame");
    // Each frame has fft_size/2 + 1 = 257 bins.
    for frame in &frames {
        assert_eq!(frame.len(), 257);
    }
}

#[test]
fn test_stft_inverse_is_finite() {
    use oximedia_audio::stft::{Istft, Stft, StftConfig, WindowType};
    let config = StftConfig::new(256, 64, WindowType::Hann);
    let mut stft = Stft::new(config.clone());
    let mut istft = Istft::new(config);
    let signal: Vec<f32> = (0..1024)
        .map(|i| (i as f32 * 0.05_f32).sin() * 0.5)
        .collect();
    let frames = stft.forward(&signal);
    let reconstructed = istft.inverse(&frames);
    assert!(!reconstructed.is_empty());
    for s in &reconstructed {
        assert!(s.is_finite(), "ISTFT output must be finite");
    }
}

// ─── watermark ───────────────────────────────────────────────────────────────

#[test]
fn test_watermark_embedder_output_length() {
    use oximedia_audio::watermark::{WatermarkConfig, WatermarkEmbedder};
    let config = WatermarkConfig::default();
    let embedder = WatermarkEmbedder::new(config).expect("default config is valid");
    let mut audio: Vec<f32> = (0..8192)
        .map(|i| (i as f32 * 0.05_f32).sin() * 0.6)
        .collect();
    let original_len = audio.len();
    let blocks = embedder
        .embed(&mut audio, 0b1010_1010_u64)
        .expect("embed should succeed");
    assert_eq!(
        audio.len(),
        original_len,
        "embed must not change buffer length"
    );
    assert!(blocks > 0, "at least one block should be embedded");
    for s in &audio {
        assert!(s.is_finite());
    }
}

#[test]
fn test_watermark_detector_does_not_panic() {
    use oximedia_audio::watermark::{WatermarkConfig, WatermarkDetector, WatermarkEmbedder};
    let config = WatermarkConfig::default();
    let embedder = WatermarkEmbedder::new(config.clone()).expect("valid config");
    let detector = WatermarkDetector::new(config).expect("valid config");
    let mut audio: Vec<f32> = (0..8192)
        .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48_000.0).sin() * 0.5)
        .collect();
    embedder
        .embed(&mut audio, 0b1111_0000_u64)
        .expect("embed must succeed");
    // Detection returns DetectionResult (not a Result — always succeeds structurally).
    let result = detector.detect(&audio);
    // Confidence must be finite.
    assert!(result.confidence.is_finite());
    assert!(result.blocks_analyzed >= 1);
}
