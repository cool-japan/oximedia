//! Real media analysis for
//! [`TaskType::Analysis`](crate::task::TaskType::Analysis).
//!
//! The input is decoded with [`crate::task_exec::media`] and each requested
//! [`AnalysisType`] is answered by the crate that actually implements it:
//!
//! | [`AnalysisType`] | Input | Implementation |
//! |---|---|---|
//! | [`AudioLevels`](AnalysisType::AudioLevels) | WAV | `oximedia_audio_analysis::{energy, loudness}` |
//! | [`Silence`](AnalysisType::Silence) | WAV | `oximedia_audio_analysis::silence_detect` |
//! | [`BlackFrames`](AnalysisType::BlackFrames) | Y4M | `oximedia_analysis::black::BlackFrameDetector` |
//! | [`SceneDetection`](AnalysisType::SceneDetection) | Y4M | `oximedia_analysis::scene::SceneDetector` |
//! | [`Motion`](AnalysisType::Motion) | Y4M | `oximedia_analysis::motion::MotionAnalyzer` |
//! | [`Color`](AnalysisType::Color) | Y4M | `oximedia_analysis::color::ColorAnalyzer` |
//! | [`VideoQuality`](AnalysisType::VideoQuality) | Y4M | `oximedia_quality::QualityAssessor` (no-reference) |
//!
//! Requesting an audio analysis on a video-only input (or the reverse) is an
//! error naming the mismatch — the task never reports a measurement it did
//! not take. When the task carries an `output` path the full result document
//! is written there as JSON and the file is verified to be non-empty.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Map, Value};
use tracing::{debug, info};

use oximedia_analysis::black::BlackFrameDetector;
use oximedia_analysis::color::ColorAnalyzer;
use oximedia_analysis::motion::MotionAnalyzer;
use oximedia_analysis::scene::SceneDetector;
use oximedia_audio_analysis::energy::rms_energy;
use oximedia_audio_analysis::loudness::LoudnessMeter;
use oximedia_audio_analysis::silence_detect::{
    linear_to_dbfs, SilenceDetectConfig, SilenceDetector,
};
use oximedia_core::PixelFormat;
use oximedia_quality::{Frame, MetricType, QualityAssessor};

use crate::error::{Result, WorkflowError};
use crate::task::AnalysisType;
use crate::task_exec::media::{
    decode_wav, decode_y4m, detect_media_kind, DecodeLimits, DecodedAudio, DecodedVideo, MediaKind,
};
use crate::task_exec::{verify_non_empty_file, TaskOutcome};

/// Luma value below which a pixel counts as black.
const BLACK_THRESHOLD: u8 = 16;

/// Minimum number of consecutive black frames reported as a segment.
const BLACK_MIN_DURATION: usize = 1;

/// Histogram-difference threshold for scene-cut detection (the value
/// `oximedia-analysis`'s own tests use).
const SCENE_THRESHOLD: f64 = 0.3;

/// Number of dominant colours extracted by the colour analyser.
const DOMINANT_COLORS: usize = 5;

/// Maximum number of frames sampled for no-reference quality metrics.
const QUALITY_SAMPLE_FRAMES: usize = 5;

/// No-reference metrics attempted for [`AnalysisType::VideoQuality`].
const NO_REFERENCE_METRICS: [MetricType; 5] = [
    MetricType::Blur,
    MetricType::Noise,
    MetricType::Blockiness,
    MetricType::Brisque,
    MetricType::Niqe,
];

/// Whether an analysis consumes audio samples or video frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Domain {
    /// Needs decoded audio.
    Audio,
    /// Needs decoded video frames.
    Video,
}

/// The domain an analysis type belongs to.
const fn domain_of(analysis: &AnalysisType) -> Domain {
    match analysis {
        AnalysisType::AudioLevels | AnalysisType::Silence => Domain::Audio,
        AnalysisType::VideoQuality
        | AnalysisType::SceneDetection
        | AnalysisType::BlackFrames
        | AnalysisType::Color
        | AnalysisType::Motion => Domain::Video,
    }
}

/// Stable JSON key for an analysis type.
const fn key_of(analysis: &AnalysisType) -> &'static str {
    match analysis {
        AnalysisType::AudioLevels => "audio_levels",
        AnalysisType::VideoQuality => "video_quality",
        AnalysisType::SceneDetection => "scene_detection",
        AnalysisType::BlackFrames => "black_frames",
        AnalysisType::Silence => "silence",
        AnalysisType::Color => "color",
        AnalysisType::Motion => "motion",
    }
}

// ---------------------------------------------------------------------------
// Audio analyses
// ---------------------------------------------------------------------------

/// Peak, RMS and integrated-loudness measurements.
fn analyse_audio_levels(audio: &DecodedAudio) -> Result<Value> {
    let mono = audio.to_mono();
    if mono.is_empty() {
        return Err(WorkflowError::generic(
            "AudioLevels: decoded audio contains no samples".to_string(),
        ));
    }

    let peak = mono.iter().fold(0.0f32, |acc, &s| acc.max(s.abs()));
    let rms = rms_energy(&mono);

    let mut meter = LoudnessMeter::new(audio.sample_rate as f32);
    for &sample in &mono {
        meter.push_sample(sample);
    }
    let integrated = meter.integrated_loudness();
    let momentary = meter.momentary_loudness();

    // Per-channel peaks, so a dead channel is visible rather than averaged away.
    let channels = audio.channels.max(1) as usize;
    let mut channel_peaks = vec![0.0f32; channels];
    for frame in audio.samples.chunks(channels) {
        for (peak, &sample) in channel_peaks.iter_mut().zip(frame.iter()) {
            *peak = peak.max(sample.abs());
        }
    }

    Ok(json!({
        "peak": peak,
        "peak_dbfs": linear_to_dbfs(f64::from(peak)),
        "rms": rms,
        "rms_dbfs": linear_to_dbfs(f64::from(rms)),
        "channel_peaks_dbfs": channel_peaks
            .iter()
            .map(|&p| linear_to_dbfs(f64::from(p)))
            .collect::<Vec<f64>>(),
        "integrated_lufs": integrated,
        "momentary_lufs": momentary,
        "loudness_windows": meter.window_count(),
    }))
}

/// Silence-region detection over the decoded audio.
fn analyse_silence(audio: &DecodedAudio) -> Result<Value> {
    let mono = audio.to_mono();
    if mono.is_empty() {
        return Err(WorkflowError::generic(
            "Silence: decoded audio contains no samples".to_string(),
        ));
    }

    let detector = SilenceDetector::new(SilenceDetectConfig::default());
    let result = detector.detect(&mono, f64::from(audio.sample_rate));

    let regions: Vec<Value> = result
        .regions
        .iter()
        .filter(|region| region.is_silent)
        .map(|region| {
            json!({
                "start_s": region.start_s,
                "end_s": region.end_s,
                "duration_s": region.duration_s(),
                "avg_rms_dbfs": linear_to_dbfs(region.avg_rms),
                "peak_dbfs": linear_to_dbfs(region.peak_level),
            })
        })
        .collect();

    Ok(json!({
        "threshold_dbfs": SilenceDetectConfig::default().threshold_dbfs,
        "silence_count": result.silence_count,
        "total_silence_s": result.total_silence_s,
        "total_active_s": result.total_active_s,
        "silence_ratio": result.silence_ratio,
        "silent_regions": regions,
    }))
}

// ---------------------------------------------------------------------------
// Video analyses
// ---------------------------------------------------------------------------

/// Black-frame segment detection.
fn analyse_black_frames(video: &DecodedVideo) -> Result<Value> {
    let mut detector = BlackFrameDetector::new(BLACK_THRESHOLD, BLACK_MIN_DURATION);
    for (index, frame) in video.frames.iter().enumerate() {
        detector
            .process_frame(&frame.y, video.width, video.height, index)
            .map_err(|e| {
                WorkflowError::generic(format!("BlackFrames: frame {index} rejected: {e}"))
            })?;
    }
    let segments = detector.finalize();
    let value = serde_json::to_value(&segments)?;

    Ok(json!({
        "threshold": BLACK_THRESHOLD,
        "min_duration_frames": BLACK_MIN_DURATION,
        "segment_count": segments.len(),
        "segments": value,
    }))
}

/// Scene-cut detection.
fn analyse_scenes(video: &DecodedVideo) -> Result<Value> {
    let mut detector = SceneDetector::new(SCENE_THRESHOLD);
    for (index, frame) in video.frames.iter().enumerate() {
        detector
            .process_frame(&frame.y, video.width, video.height, index)
            .map_err(|e| {
                WorkflowError::generic(format!("SceneDetection: frame {index} rejected: {e}"))
            })?;
    }
    let scenes = detector.finalize();

    Ok(json!({
        "threshold": SCENE_THRESHOLD,
        "scene_count": scenes.len(),
        "scenes": serde_json::to_value(&scenes)?,
    }))
}

/// Frame-to-frame motion statistics.
fn analyse_motion(video: &DecodedVideo) -> Result<Value> {
    if video.frames.len() < 2 {
        return Err(WorkflowError::generic(format!(
            "Motion: needs at least 2 frames, the input decoded to {}",
            video.frames.len()
        )));
    }

    let mut analyzer = MotionAnalyzer::new();
    for (index, frame) in video.frames.iter().enumerate() {
        analyzer
            .process_frame(&frame.y, video.width, video.height, index)
            .map_err(|e| WorkflowError::generic(format!("Motion: frame {index} rejected: {e}")))?;
    }
    let stats = analyzer.finalize();

    Ok(json!({
        "avg_motion": stats.avg_motion,
        "max_motion": stats.max_motion,
        "camera_motion": serde_json::to_value(&stats.camera_motion)?,
        "stability": stats.stability,
        "frames_compared": stats.frame_motion.len(),
    }))
}

/// Dominant-colour and saturation analysis (needs chroma planes).
fn analyse_color(video: &DecodedVideo) -> Result<Value> {
    if !video.has_chroma() {
        return Err(WorkflowError::generic(format!(
            "Color: the input is monochrome ({}), so there are no chroma planes to analyse",
            video.chroma
        )));
    }

    let mut analyzer = ColorAnalyzer::new(DOMINANT_COLORS);
    for (index, frame) in video.frames.iter().enumerate() {
        let (Some(u), Some(v)) = (frame.u.as_ref(), frame.v.as_ref()) else {
            return Err(WorkflowError::generic(format!(
                "Color: frame {index} has no chroma planes"
            )));
        };
        analyzer
            .process_frame(&frame.y, u, v, video.width, video.height, index)
            .map_err(|e| WorkflowError::generic(format!("Color: frame {index} rejected: {e}")))?;
    }
    let analysis = analyzer.finalize();

    Ok(json!({
        "chroma": video.chroma,
        "dominant_colors": serde_json::to_value(&analysis.dominant_colors)?,
        "avg_saturation": analysis.avg_saturation,
        "color_diversity": analysis.color_diversity,
        "grading_style": serde_json::to_value(&analysis.grading_style)?,
    }))
}

/// Builds a `Gray8` quality frame from a luma plane.
fn gray_frame(luma: &[u8], width: usize, height: usize) -> Result<Frame> {
    let mut frame = Frame::new(width, height, PixelFormat::Gray8).map_err(|e| {
        WorkflowError::generic(format!(
            "VideoQuality: cannot allocate {width}x{height} frame: {e}"
        ))
    })?;
    let plane = frame.luma_mut();
    if plane.len() != luma.len() {
        return Err(WorkflowError::generic(format!(
            "VideoQuality: luma plane size mismatch ({} vs {})",
            luma.len(),
            plane.len()
        )));
    }
    plane.copy_from_slice(luma);
    Ok(frame)
}

/// No-reference quality metrics over evenly sampled frames.
fn analyse_video_quality(video: &DecodedVideo) -> Result<Value> {
    let total = video.frames.len();
    let sample_count = QUALITY_SAMPLE_FRAMES.min(total);
    let indices: Vec<usize> = (0..sample_count)
        .map(|i| {
            if sample_count == 1 {
                0
            } else {
                i * (total - 1) / (sample_count - 1)
            }
        })
        .collect();

    let assessor = QualityAssessor::new();
    let mut metrics = Map::new();
    let mut any_measured = false;

    for metric in NO_REFERENCE_METRICS {
        let mut scores = Vec::new();
        let mut last_error: Option<String> = None;

        for &index in &indices {
            let frame = gray_frame(&video.frames[index].y, video.width, video.height)?;
            match assessor.assess_no_reference(&frame, metric) {
                Ok(score) => scores.push(score.score),
                Err(e) => last_error = Some(e.to_string()),
            }
        }

        let key = format!("{metric:?}").to_lowercase();
        if scores.is_empty() {
            metrics.insert(
                key,
                json!({
                    "available": false,
                    "reason": last_error
                        .unwrap_or_else(|| "metric produced no score".to_string()),
                }),
            );
        } else {
            any_measured = true;
            let mean = scores.iter().sum::<f64>() / scores.len() as f64;
            let min = scores.iter().copied().fold(f64::INFINITY, f64::min);
            let max = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            metrics.insert(
                key,
                json!({
                    "available": true,
                    "mean": mean,
                    "min": min,
                    "max": max,
                    "frames_scored": scores.len(),
                }),
            );
        }
    }

    if !any_measured {
        return Err(WorkflowError::generic(format!(
            "VideoQuality: no no-reference metric could be computed on {}x{} frames \
             (Blur/Noise need 8x8, Blockiness 16x16, BRISQUE 32x32, NIQE 96x96)",
            video.width, video.height
        )));
    }

    Ok(json!({
        "sampled_frame_indices": indices,
        "metrics": Value::Object(metrics),
    }))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Runs every requested analysis over `input`, optionally writing the result
/// document to `output`.
///
/// # Errors
///
/// Returns an error when the input is missing/empty, its container cannot be
/// decoded by this crate, no analyses were requested, an analysis is
/// incompatible with the input's domain (audio vs video), an individual
/// analysis fails, or the output document cannot be written.
pub async fn execute_analysis(
    input: &Path,
    analyses: &[AnalysisType],
    output: Option<&Path>,
) -> Result<TaskOutcome> {
    if !input.exists() {
        return Err(WorkflowError::FileNotFound(input.to_path_buf()));
    }
    let input_size = verify_non_empty_file(input, "Analysis input")?;

    if analyses.is_empty() {
        return Err(WorkflowError::generic(format!(
            "Analysis task for {} requested no analyses",
            input.display()
        )));
    }

    // Deduplicate while keeping a deterministic order.
    let mut seen = BTreeSet::new();
    let requested: Vec<&AnalysisType> =
        analyses.iter().filter(|a| seen.insert(key_of(a))).collect();

    let kind = detect_media_kind(input)?;
    let mut results = Map::new();
    let mut source = Map::new();

    match kind {
        MediaKind::Wav => {
            let mismatched: Vec<&'static str> = requested
                .iter()
                .filter(|a| domain_of(a) == Domain::Video)
                .map(|a| key_of(a))
                .collect();
            if !mismatched.is_empty() {
                return Err(WorkflowError::generic(format!(
                    "Analysis of the audio-only file {} cannot run video analyses {mismatched:?}",
                    input.display()
                )));
            }

            let audio = decode_wav(input).await?;
            source.insert("kind".to_string(), json!("wav"));
            source.insert("channels".to_string(), json!(audio.channels));
            source.insert("sample_rate".to_string(), json!(audio.sample_rate));
            source.insert("frames".to_string(), json!(audio.frame_count()));
            source.insert("duration_secs".to_string(), json!(audio.duration_secs()));

            for analysis in &requested {
                let value = match analysis {
                    AnalysisType::AudioLevels => analyse_audio_levels(&audio)?,
                    AnalysisType::Silence => analyse_silence(&audio)?,
                    other => {
                        return Err(WorkflowError::generic(format!(
                            "Analysis {other:?} is not implemented for audio input"
                        )));
                    }
                };
                results.insert(key_of(analysis).to_string(), value);
            }
        }
        MediaKind::Y4m => {
            let mismatched: Vec<&'static str> = requested
                .iter()
                .filter(|a| domain_of(a) == Domain::Audio)
                .map(|a| key_of(a))
                .collect();
            if !mismatched.is_empty() {
                return Err(WorkflowError::generic(format!(
                    "Analysis of the video-only file {} cannot run audio analyses {mismatched:?}",
                    input.display()
                )));
            }

            let video = decode_y4m(input, DecodeLimits::default()).await?;
            source.insert("kind".to_string(), json!("y4m"));
            source.insert("width".to_string(), json!(video.width));
            source.insert("height".to_string(), json!(video.height));
            source.insert("chroma".to_string(), json!(video.chroma));
            source.insert("fps".to_string(), json!(video.fps()));
            source.insert("frames_decoded".to_string(), json!(video.frames.len()));
            source.insert("truncated".to_string(), json!(video.truncated));

            for analysis in &requested {
                let value = match analysis {
                    AnalysisType::BlackFrames => analyse_black_frames(&video)?,
                    AnalysisType::SceneDetection => analyse_scenes(&video)?,
                    AnalysisType::Motion => analyse_motion(&video)?,
                    AnalysisType::Color => analyse_color(&video)?,
                    AnalysisType::VideoQuality => analyse_video_quality(&video)?,
                    other => {
                        return Err(WorkflowError::generic(format!(
                            "Analysis {other:?} is not implemented for video input"
                        )));
                    }
                };
                results.insert(key_of(analysis).to_string(), value);
            }
        }
        MediaKind::Unsupported(magic) => {
            return Err(WorkflowError::generic(format!(
                "Analysis cannot decode {}: leading bytes {magic} are neither RIFF/WAVE nor \
                 YUV4MPEG2, and this crate decodes no other container directly",
                input.display()
            )));
        }
    }

    let document = json!({
        "kind": "analysis",
        "input": input.display().to_string(),
        "input_bytes": input_size,
        "source": Value::Object(source),
        "analyses": Value::Object(results),
    });

    let mut outcome = TaskOutcome::with_data(document.clone());

    if let Some(output_path) = output {
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    WorkflowError::generic(format!(
                        "Cannot create analysis output directory {}: {e}",
                        parent.display()
                    ))
                })?;
            }
        }
        let serialised = serde_json::to_vec_pretty(&document)?;
        tokio::fs::write(output_path, &serialised)
            .await
            .map_err(|e| {
                WorkflowError::generic(format!(
                    "Cannot write analysis result to {}: {e}",
                    output_path.display()
                ))
            })?;
        verify_non_empty_file(output_path, "Analysis")?;
        debug!("Analysis result written to {}", output_path.display());
        outcome = outcome.and_output(output_path.to_path_buf());
    }

    info!(
        "Analysis of {} completed: {} analysis/analyses",
        input.display(),
        requested.len()
    );

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oximedia_wf_an_{}_{name}", std::process::id()))
    }

    fn wav_bytes(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let data_size = (samples.len() * 2) as u32;
        let byte_rate = sample_rate * u32::from(channels) * 2;
        let mut buf = Vec::with_capacity(44 + data_size as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_size).to_le_bytes());
        buf.extend_from_slice(b"WAVEfmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&(channels * 2).to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for s in samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf
    }

    /// Half a second of tone followed by half a second of digital silence.
    fn tone_then_silence(sample_rate: u32) -> Vec<i16> {
        let half = sample_rate as usize / 2;
        let mut samples = Vec::with_capacity(half * 2);
        for i in 0..half {
            let t = i as f64 / f64::from(sample_rate);
            samples.push(((t * 440.0 * std::f64::consts::TAU).sin() * 16_000.0) as i16);
        }
        samples.extend(std::iter::repeat_n(0i16, half));
        samples
    }

    fn y4m_bytes(width: usize, height: usize, frames: usize, black_frames: &[usize]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(
            format!("YUV4MPEG2 W{width} H{height} F25:1 Ip A1:1 C420jpeg\n").as_bytes(),
        );
        for t in 0..frames {
            buf.extend_from_slice(b"FRAME\n");
            let black = black_frames.contains(&t);
            for y in 0..height {
                for x in 0..width {
                    buf.push(if black {
                        0
                    } else {
                        ((x * 3 + y * 5 + t * 29) % 256) as u8
                    });
                }
            }
            for _ in 0..2 {
                for _ in 0..height.div_ceil(2) {
                    for _ in 0..width.div_ceil(2) {
                        buf.push(128);
                    }
                }
            }
        }
        buf
    }

    #[tokio::test]
    async fn audio_levels_and_silence_are_measured() {
        let path = temp_path("levels.wav");
        std::fs::write(&path, wav_bytes(&tone_then_silence(16_000), 16_000, 1)).expect("write wav");

        let outcome = execute_analysis(
            &path,
            &[AnalysisType::AudioLevels, AnalysisType::Silence],
            None,
        )
        .await
        .expect("analysis must succeed");

        let data = outcome.data.expect("analysis payload");
        let levels = &data["analyses"]["audio_levels"];
        let peak_dbfs = levels["peak_dbfs"].as_f64().expect("peak_dbfs");
        assert!(
            (-8.0..0.0).contains(&peak_dbfs),
            "peak {peak_dbfs} dBFS should be just under full scale"
        );

        let silence = &data["analyses"]["silence"];
        let ratio = silence["silence_ratio"].as_f64().expect("silence_ratio");
        assert!(
            (0.3..0.7).contains(&ratio),
            "half the file is silent, got ratio {ratio}"
        );
        assert!(silence["silence_count"].as_u64().unwrap_or(0) >= 1);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn video_analyses_find_the_injected_black_frames() {
        let path = temp_path("black.y4m");
        std::fs::write(&path, y4m_bytes(64, 64, 8, &[3, 4])).expect("write y4m");

        let outcome = execute_analysis(
            &path,
            &[
                AnalysisType::BlackFrames,
                AnalysisType::Motion,
                AnalysisType::SceneDetection,
                AnalysisType::Color,
                AnalysisType::VideoQuality,
            ],
            None,
        )
        .await
        .expect("analysis must succeed");

        let data = outcome.data.expect("analysis payload");
        let black = &data["analyses"]["black_frames"];
        assert!(
            black["segment_count"].as_u64().unwrap_or(0) >= 1,
            "injected black frames must be detected: {black}"
        );
        let segments = black["segments"].as_array().expect("segments array");
        assert!(segments
            .iter()
            .any(|s| s["start_frame"].as_u64() == Some(3)));

        assert!(
            data["analyses"]["motion"]["frames_compared"]
                .as_u64()
                .unwrap_or(0)
                >= 7
        );
        assert!(data["analyses"]["color"]["dominant_colors"].is_array());
        assert!(
            data["analyses"]["video_quality"]["metrics"]["blur"]["available"]
                .as_bool()
                .unwrap_or(false)
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn writes_result_document_to_output_slot() {
        let path = temp_path("doc.y4m");
        let out = temp_path("doc_result/analysis.json");
        std::fs::write(&path, y4m_bytes(32, 32, 4, &[])).expect("write y4m");
        let _ = std::fs::remove_file(&out);

        let outcome = execute_analysis(&path, &[AnalysisType::BlackFrames], Some(&out))
            .await
            .expect("analysis must succeed");

        assert_eq!(outcome.outputs, vec![out.clone()]);
        let written = std::fs::read_to_string(&out).expect("result document");
        let parsed: Value = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(parsed["source"]["kind"], "y4m");
        assert!(parsed["analyses"]["black_frames"].is_object());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_dir(out.parent().unwrap_or(Path::new("/")));
    }

    #[tokio::test]
    async fn audio_analysis_on_video_input_is_refused() {
        let path = temp_path("mismatch.y4m");
        std::fs::write(&path, y4m_bytes(16, 16, 2, &[])).expect("write y4m");

        let err = execute_analysis(&path, &[AnalysisType::AudioLevels], None)
            .await
            .expect_err("domain mismatch must fail");
        assert!(err.to_string().contains("cannot run audio analyses"));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn video_analysis_on_audio_input_is_refused() {
        let path = temp_path("mismatch.wav");
        std::fs::write(&path, wav_bytes(&[0, 1, -1, 2], 8_000, 1)).expect("write wav");

        let err = execute_analysis(&path, &[AnalysisType::Motion], None)
            .await
            .expect_err("domain mismatch must fail");
        assert!(err.to_string().contains("cannot run video analyses"));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn undecodable_container_is_refused() {
        let path = temp_path("unknown.mkv");
        std::fs::write(&path, b"\x1a\x45\xdf\xa3matroska-ish-bytes").expect("write mkv");

        let err = execute_analysis(&path, &[AnalysisType::BlackFrames], None)
            .await
            .expect_err("undecodable input must fail");
        assert!(err.to_string().contains("neither RIFF/WAVE nor"));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn empty_analysis_list_is_refused() {
        let path = temp_path("noanalyses.wav");
        std::fs::write(&path, wav_bytes(&[0, 1, -1, 2], 8_000, 1)).expect("write wav");

        let err = execute_analysis(&path, &[], None)
            .await
            .expect_err("empty request must fail");
        assert!(err.to_string().contains("requested no analyses"));

        let _ = std::fs::remove_file(path);
    }
}
