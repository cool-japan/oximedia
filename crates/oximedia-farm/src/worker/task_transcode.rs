//! Real transcode task execution.
//!
//! Split out of `executor.rs` (which still owns [`TaskExecutor`] itself and
//! dispatches to [`TaskExecutor::execute_transcode`] here) to keep both
//! files under the workspace's 2000-line-per-file policy; `task_thumbnail.rs`
//! is the analogous thumbnail-task split. Rust allows a type's inherent
//! methods to be defined across multiple `impl` blocks in different modules
//! within the same crate, so this file simply adds another `impl
//! TaskExecutor` block next to the one in `executor.rs`.

use super::executor::{TaskExecutor, TaskSpecification};
use super::media;
use crate::{FarmError, Result, TaskId};
use oximedia_transcode::{
    QualityConfig, QualityPreset, RateControlMode, ScaleSpec, TranscodePipeline,
};
use std::collections::HashMap;
use std::path::Path;

impl TaskExecutor {
    /// Execute transcoding task.
    ///
    /// Runs a real transcode via [`oximedia_transcode::TranscodePipeline`],
    /// following the same builder → build → execute → verify pattern as
    /// `oximedia-workflow`'s transcode task executor
    /// (`task_exec/transcode.rs`). Supported inputs are WAV and Y4M (see
    /// [`media::detect_media_kind`]); `spec.parameters["video_codec"]`/
    /// `["audio_codec"]` select the target codec explicitly, or else
    /// [`Self::default_transcode_codec`] derives one from
    /// `spec.output_path`'s extension, matching `oximedia-transcode`'s own
    /// per-target extension gates. Anything the pipeline cannot really
    /// encode yet (FFV1, AV1, VP9, …) surfaces the pipeline's own honest
    /// error verbatim — this method never fabricates success or invents an
    /// artifact.
    ///
    /// # Errors
    ///
    /// Returns [`FarmError::NotFound`] when the input is missing, and
    /// [`FarmError::Task`] for an empty input/output path, an unsupported
    /// input container, an unknown or invalid parameter, a pipeline
    /// configuration/execution failure, or a missing/empty output file
    /// after a reported success.
    pub(super) async fn execute_transcode(
        &self,
        task_id: TaskId,
        spec: &TaskSpecification,
    ) -> Result<Vec<u8>> {
        tracing::info!("Transcoding {} to {}", spec.input_path, spec.output_path);
        self.update_progress(task_id, 0.05);

        if spec.output_path.is_empty() {
            return Err(FarmError::Task(
                "transcode task requires a non-empty output_path".to_string(),
            ));
        }

        let input = Path::new(&spec.input_path);
        let input_meta = tokio::fs::metadata(input).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                FarmError::NotFound(format!("transcode input not found: {}", spec.input_path))
            } else {
                FarmError::Task(format!(
                    "cannot stat transcode input '{}': {e}",
                    spec.input_path
                ))
            }
        })?;
        if input_meta.len() == 0 {
            return Err(FarmError::Task(format!(
                "transcode input '{}' is empty",
                spec.input_path
            )));
        }

        let kind = media::detect_media_kind(&spec.input_path).await?;
        self.update_progress(task_id, 0.15);

        const ACCEPTED_TRANSCODE_PARAMS: &str =
            "video_codec, audio_codec, crf, scale, start_secs, duration_secs, hw_accel";
        for key in spec.parameters.keys() {
            if !matches!(
                key.as_str(),
                "video_codec"
                    | "audio_codec"
                    | "crf"
                    | "scale"
                    | "start_secs"
                    | "duration_secs"
                    | "hw_accel"
            ) {
                return Err(FarmError::Task(format!(
                    "unknown transcode parameter '{key}' (accepted: {ACCEPTED_TRANSCODE_PARAMS})"
                )));
            }
        }

        let mut video_codec = spec.parameters.get("video_codec").cloned();
        let mut audio_codec = spec.parameters.get("audio_codec").cloned();
        if video_codec.is_none() && audio_codec.is_none() {
            Self::default_transcode_codec(
                kind,
                Path::new(&spec.output_path),
                &mut video_codec,
                &mut audio_codec,
            )?;
        }

        let crf = Self::parse_opt_param::<u8>(&spec.parameters, "crf")?;
        let scale = Self::parse_scale_param(&spec.parameters)?;
        let start_secs = Self::parse_opt_param::<f64>(&spec.parameters, "start_secs")?;
        let duration_secs = Self::parse_opt_param::<f64>(&spec.parameters, "duration_secs")?;
        let hw_accel = Self::parse_bool_param(&spec.parameters, "hw_accel", true)?;

        let output = Path::new(&spec.output_path);
        if let Some(parent) = output.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    FarmError::Task(format!(
                        "cannot create transcode output directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
        }
        // Remove a stale artifact so "output exists and is non-empty" below
        // cannot be satisfied by a previous run's file.
        match tokio::fs::remove_file(output).await {
            Ok(()) => tracing::debug!("Removed stale transcode output {}", output.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(FarmError::Task(format!(
                    "cannot clear existing transcode output '{}': {e}",
                    output.display()
                )));
            }
        }

        self.update_progress(task_id, 0.25);

        let mut builder = TranscodePipeline::builder()
            .input(input.to_path_buf())
            .output(output.to_path_buf())
            .hw_accel(hw_accel)
            .track_progress(false);
        if let Some(codec) = &video_codec {
            builder = builder.video_codec(codec.clone());
        }
        if let Some(codec) = &audio_codec {
            builder = builder.audio_codec(codec.clone());
        }
        if let Some((width, height)) = scale {
            builder = builder.video_scale(ScaleSpec { width, height });
        }
        if let Some(secs) = start_secs {
            builder = builder.start_time_secs(secs);
        }
        if let Some(secs) = duration_secs {
            builder = builder.duration_secs(secs);
        }
        if let Some(crf) = crf {
            builder = builder.quality(QualityConfig {
                preset: QualityPreset::Medium,
                rate_control: RateControlMode::Crf(crf),
                two_pass: false,
                lookahead: None,
                tune: None,
            });
        }

        let mut pipeline = builder.build().map_err(|e| {
            FarmError::Task(format!("transcode pipeline configuration rejected: {e}"))
        })?;

        self.update_progress(task_id, 0.35);

        let stats = pipeline.execute().await.map_err(|e| {
            FarmError::Task(format!(
                "transcode {} -> {} failed: {e}",
                spec.input_path, spec.output_path
            ))
        })?;

        // The pipeline reported success — prove it on disk.
        let output_meta = tokio::fs::metadata(output).await.map_err(|e| {
            FarmError::Task(format!(
                "transcode reported success but output '{}' is unreadable: {e}",
                spec.output_path
            ))
        })?;
        if output_meta.len() == 0 {
            return Err(FarmError::Task(format!(
                "transcode reported success but output '{}' is empty",
                spec.output_path
            )));
        }

        self.update_progress(task_id, 1.0);

        let payload = serde_json::json!({
            "kind": "transcode",
            "input": spec.input_path,
            "input_bytes": input_meta.len(),
            "output": spec.output_path,
            "output_bytes": output_meta.len(),
            "video_codec": video_codec.unwrap_or_else(|| "copy".to_string()),
            "audio_codec": audio_codec.unwrap_or_else(|| "copy".to_string()),
            "encoding_time_secs": stats.encoding_time,
            "speed_factor": stats.speed_factor,
            "content_duration_secs": stats.duration,
            "artifacts": [spec.output_path],
        });

        serde_json::to_vec(&payload)
            .map_err(|e| FarmError::Task(format!("failed to serialize transcode result: {e}")))
    }
    /// Defaults `video_codec`/`audio_codec` from the output path's
    /// extension when the caller specified neither, mirroring
    /// `oximedia-transcode`'s own per-target extension gates (see its
    /// `frame_level` module docs): WAV → FLAC (`.flac`/`.mka`/`.mkv`), PCM
    /// (`.wav`), or ALAC (`.caf`); Y4M → MJPEG (`.mkv`/`.webm`), MPEG-2
    /// (`.m2v`/`.mpg`/`.mpeg`/`.mpv`), or rawvideo (`.y4m`). Only the axis
    /// the input actually carries is ever set — the real pipeline itself
    /// rejects a video codec on WAV and an audio codec on Y4M input.
    ///
    /// # Errors
    ///
    /// Returns [`FarmError::Task`] when the output extension has no known
    /// default for this input kind (e.g. FFV1/APV/ProRes video, which has
    /// no single "natural" extension here) — set `video_codec`/
    /// `audio_codec` explicitly instead.
    fn default_transcode_codec(
        kind: media::MediaKind,
        output: &Path,
        video_codec: &mut Option<String>,
        audio_codec: &mut Option<String>,
    ) -> Result<()> {
        let ext = output
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();

        match kind {
            media::MediaKind::Wav => {
                let codec = match ext.as_str() {
                    "flac" | "mka" | "mkv" => "flac",
                    "wav" => "pcm",
                    "caf" => "alac",
                    other => {
                        return Err(FarmError::Task(format!(
                            "cannot default an audio codec for WAV input with output extension \
                             '.{other}'; supported output extensions are .flac/.mka/.mkv \
                             (FLAC), .wav (PCM), .caf (ALAC), or set `audio_codec` explicitly"
                        )));
                    }
                };
                *audio_codec = Some(codec.to_string());
            }
            media::MediaKind::Y4m => {
                let codec = match ext.as_str() {
                    "mkv" | "webm" => "mjpeg",
                    "m2v" | "mpg" | "mpeg" | "mpv" => "mpeg2",
                    "y4m" => "rawvideo",
                    other => {
                        return Err(FarmError::Task(format!(
                            "cannot default a video codec for Y4M input with output extension \
                             '.{other}'; supported output extensions are .mkv/.webm (MJPEG), \
                             .m2v/.mpg/.mpeg/.mpv (MPEG-2), .y4m (rawvideo), or set \
                             `video_codec` explicitly (e.g. apv, ffv1, prores)"
                        )));
                    }
                };
                *video_codec = Some(codec.to_string());
            }
        }
        Ok(())
    }
    /// Parses a `scale` transcode parameter (`"WxH"`, either side `-1`/
    /// `auto`/empty meaning "derive from the other side") into
    /// `(width, height)`.
    ///
    /// # Errors
    ///
    /// Returns [`FarmError::Task`] for a malformed value, a zero dimension,
    /// or both sides left to derive.
    fn parse_scale_param(
        params: &HashMap<String, String>,
    ) -> Result<Option<(Option<u32>, Option<u32>)>> {
        let Some(raw) = params.get("scale") else {
            return Ok(None);
        };
        let sep = if raw.contains('x') { 'x' } else { ':' };
        let Some((w_raw, h_raw)) = raw.split_once(sep) else {
            return Err(FarmError::Task(format!(
                "invalid `scale` value '{raw}': expected `WxH` (e.g. `640x360`, `640x-1`)"
            )));
        };
        let axis = |s: &str| -> Result<Option<u32>> {
            let t = s.trim();
            if t.is_empty() || t == "-1" || t.eq_ignore_ascii_case("auto") {
                return Ok(None);
            }
            let value: u32 = t
                .parse()
                .map_err(|e| FarmError::Task(format!("invalid `scale` axis '{s}': {e}")))?;
            if value == 0 {
                return Err(FarmError::Task(
                    "invalid `scale` axis: dimensions must be non-zero".to_string(),
                ));
            }
            Ok(Some(value))
        };
        let (width, height) = (axis(w_raw)?, axis(h_raw)?);
        if width.is_none() && height.is_none() {
            return Err(FarmError::Task(format!(
                "invalid `scale` value '{raw}': at least one dimension must be fixed"
            )));
        }
        Ok(Some((width, height)))
    }

    /// Parses an optional boolean-valued task parameter, falling back to
    /// `default` when absent.
    ///
    /// Accepts case-insensitive `true`/`false`, `1`/`0`, `yes`/`no`, and
    /// `on`/`off` — `bool::FromStr` (what [`Self::parse_opt_param`] would
    /// otherwise delegate to) only accepts the exact lowercase strings
    /// `"true"`/`"false"`, which would reject plausible caller values like
    /// `"1"` or `"True"` with a hard parameter error instead of proceeding;
    /// every other string parameter in this worker (`qc_profile`, `scale`'s
    /// `auto` marker) is already case-insensitive, so this keeps `hw_accel`
    /// consistent with that.
    ///
    /// # Errors
    ///
    /// Returns [`FarmError::Task`] naming the offending value when present
    /// but unrecognised.
    fn parse_bool_param(
        params: &HashMap<String, String>,
        key: &str,
        default: bool,
    ) -> Result<bool> {
        let Some(raw) = params.get(key) else {
            return Ok(default);
        };
        match raw.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            other => Err(FarmError::Task(format!(
                "invalid `{key}` value '{other}' (accepted: true/false, 1/0, yes/no, on/off)"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oximedia_farm_task_transcode_{}_{name}",
            std::process::id()
        ))
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

    fn sine_i16(freq: f64, sample_rate: u32, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(sample_rate);
                ((t * freq * std::f64::consts::TAU).sin() * 20_000.0) as i16
            })
            .collect()
    }

    /// Builds a transcode task payload with a real output path and
    /// string-valued parameters.
    fn task_payload_full(
        task_type: &str,
        input_path: &std::path::Path,
        output_path: &std::path::Path,
        parameters: &[(&str, &str)],
    ) -> Vec<u8> {
        let params: HashMap<&str, &str> = parameters.iter().copied().collect();
        serde_json::to_vec(&serde_json::json!({
            "task_type": task_type,
            "input_path": input_path.to_string_lossy(),
            "output_path": output_path.to_string_lossy(),
            "parameters": params,
        }))
        .expect("serialize task payload")
    }

    /// Builds a one-frame YUV4MPEG2 fixture with constant per-plane values,
    /// so tests can assert on exact known color content after a JPEG
    /// encode/decode round-trip. `chroma` is one of `"420jpeg"`, `"422"`,
    /// `"444"`, or `"mono"` (no chroma planes at all).
    fn y4m_single_frame(width: usize, height: usize, chroma: &str, y: u8, u: u8, v: u8) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(
            format!("YUV4MPEG2 W{width} H{height} F25:1 Ip A1:1 C{chroma}\n").as_bytes(),
        );
        buf.extend_from_slice(b"FRAME\n");
        buf.extend(std::iter::repeat_n(y, width * height));
        let (cw, ch) = match chroma {
            "420jpeg" | "420mpeg2" | "420paldv" => (width.div_ceil(2), height.div_ceil(2)),
            "422" => (width.div_ceil(2), height),
            "444" => (width, height),
            _ => (0, 0),
        };
        if cw > 0 && ch > 0 {
            buf.extend(std::iter::repeat_n(u, cw * ch));
            buf.extend(std::iter::repeat_n(v, cw * ch));
        }
        buf
    }

    // -----------------------------------------------------------------
    // Real transcode work (no sleep-simulation)
    // -----------------------------------------------------------------

    // Real transcode work (no sleep-simulation)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_execute_transcode_wav_to_flac_round_trip() {
        // 16-bit mono WAV: `oximedia-transcode`'s frame-level WAV loader
        // passes 16-bit integer PCM through byte-exact
        // (`wav_payload_to_i16`), so a sample-exact round-trip through the
        // real FLAC encoder is achievable here. 24/32-bit sources are NOT
        // (their low bits are truncated on the way into the pipeline), so
        // this fixture must stay 16-bit for the exactness assertion below
        // to mean anything.
        let path = temp_path("transcode_src.wav");
        let samples = sine_i16(440.0, 16_000, 800);
        std::fs::write(&path, wav_bytes(&samples, 16_000, 1)).expect("write wav");

        let out = temp_path("transcode_out.flac");
        let _ = std::fs::remove_file(&out);

        let executor = TaskExecutor::new();
        let result = executor
            .execute(
                TaskId::new(),
                task_payload_full("transcode", &path, &out, &[]),
            )
            .await
            .expect("transcode task must succeed on a real WAV file");
        assert!(result.success);

        let value: serde_json::Value =
            serde_json::from_slice(&result.output).expect("transcode output must be valid JSON");
        assert_eq!(value["kind"], "transcode");
        assert_eq!(value["audio_codec"], "flac");
        assert_eq!(value["video_codec"], "copy");
        assert!(value["output_bytes"].as_u64().unwrap_or(0) > 0);
        assert_eq!(
            value["artifacts"],
            serde_json::json!([out.to_string_lossy()])
        );

        let flac_bytes = std::fs::read(&out).expect("read flac output");
        let (params, decoded) = oximedia_transcode::flac_decode::decode_flac_to_i16(&flac_bytes)
            .expect("decode the produced FLAC file with the spec-compliant decoder");
        assert_eq!(params.sample_rate, 16_000);
        assert_eq!(params.channels, 1);
        assert_eq!(
            decoded, samples,
            "FLAC round-trip through the real pipeline must be sample-exact"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out);
    }

    #[tokio::test]
    async fn test_execute_transcode_y4m_rawvideo_round_trip_is_pixel_exact() {
        let path = temp_path("transcode_src.y4m");
        std::fs::write(&path, y4m_single_frame(16, 16, "420jpeg", 77, 180, 90)).expect("write y4m");

        let out = temp_path("transcode_out.y4m");
        let _ = std::fs::remove_file(&out);

        let executor = TaskExecutor::new();
        let result = executor
            .execute(
                TaskId::new(),
                task_payload_full("transcode", &path, &out, &[]),
            )
            .await
            .expect("transcode task must succeed on a real Y4M file");

        let value: serde_json::Value =
            serde_json::from_slice(&result.output).expect("transcode output must be valid JSON");
        assert_eq!(value["video_codec"], "rawvideo");
        assert_eq!(value["audio_codec"], "copy");

        let original =
            media::extract_y4m_frame(&path.to_string_lossy(), media::FrameSelector::Index(0))
                .await
                .expect("read source y4m frame");
        let reread =
            media::extract_y4m_frame(&out.to_string_lossy(), media::FrameSelector::Index(0))
                .await
                .expect("read transcoded y4m frame");
        assert_eq!(reread.width, original.width);
        assert_eq!(reread.height, original.height);
        assert_eq!(
            reread.data, original.data,
            "rawvideo transcode must be a pixel-exact copy"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out);
    }

    #[tokio::test]
    async fn test_execute_transcode_ffv1_request_surfaces_pipeline_honest_err() {
        // FFV1's *encoder* is real, but no container in this workspace can
        // yet mux it (see `oximedia_transcode::frame_level`'s `VideoTarget::Ffv1`
        // arm) -- this proves the farm task surfaces that honest limitation
        // verbatim instead of hand-maintaining its own codec allowlist.
        let path = temp_path("transcode_ffv1_src.y4m");
        std::fs::write(&path, y4m_single_frame(16, 16, "420jpeg", 100, 128, 128))
            .expect("write y4m");
        let out = temp_path("transcode_ffv1_out.mkv");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(
                TaskId::new(),
                task_payload_full("transcode", &path, &out, &[("video_codec", "ffv1")]),
            )
            .await
            .expect_err("FFV1 muxing is not wired up yet and must fail honestly");
        assert!(err.to_string().contains("FFV1"), "{err}");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_transcode_unsupported_input_is_honest_err() {
        let path = temp_path("transcode_unsupported.bin");
        std::fs::write(&path, b"this is not any known media container").expect("write bin");
        let out = temp_path("transcode_unsupported_out.flac");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(
                TaskId::new(),
                task_payload_full("transcode", &path, &out, &[]),
            )
            .await
            .expect_err("transcode of an unrecognised container must fail honestly");
        assert!(err.to_string().contains("WAV and Y4M"), "{err}");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_transcode_missing_input_is_not_found() {
        let path = temp_path("transcode_missing.wav");
        let _ = std::fs::remove_file(&path);
        let out = temp_path("transcode_missing_out.flac");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(
                TaskId::new(),
                task_payload_full("transcode", &path, &out, &[]),
            )
            .await
            .expect_err("missing transcode input must fail");
        assert!(matches!(err, FarmError::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn test_execute_transcode_missing_output_path_is_honest_err() {
        let path = temp_path("transcode_no_out_src.wav");
        std::fs::write(&path, wav_bytes(&sine_i16(440.0, 8_000, 400), 8_000, 1))
            .expect("write wav");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(
                TaskId::new(),
                task_payload_full("transcode", &path, std::path::Path::new(""), &[]),
            )
            .await
            .expect_err("empty output_path must fail");
        assert!(err.to_string().contains("output_path"), "{err}");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_transcode_unknown_parameter_is_honest_err() {
        let path = temp_path("transcode_badparam_src.wav");
        std::fs::write(&path, wav_bytes(&sine_i16(440.0, 8_000, 400), 8_000, 1))
            .expect("write wav");
        let out = temp_path("transcode_badparam_out.flac");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(
                TaskId::new(),
                task_payload_full("transcode", &path, &out, &[("bitrate", "5000")]),
            )
            .await
            .expect_err("unknown parameter must be rejected");
        assert!(err.to_string().contains("bitrate"), "{err}");
        assert!(
            err.to_string().contains("video_bitrate") || err.to_string().contains("accepted"),
            "{err}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_default_transcode_codec_maps_known_extensions() {
        let mut video = None;
        let mut audio = None;
        TaskExecutor::default_transcode_codec(
            media::MediaKind::Wav,
            std::path::Path::new("out.flac"),
            &mut video,
            &mut audio,
        )
        .expect("flac default");
        assert_eq!(video, None);
        assert_eq!(audio.as_deref(), Some("flac"));

        let mut video = None;
        let mut audio = None;
        TaskExecutor::default_transcode_codec(
            media::MediaKind::Wav,
            std::path::Path::new("out.wav"),
            &mut video,
            &mut audio,
        )
        .expect("pcm default");
        assert_eq!(audio.as_deref(), Some("pcm"));

        let mut video = None;
        let mut audio = None;
        TaskExecutor::default_transcode_codec(
            media::MediaKind::Y4m,
            std::path::Path::new("out.mkv"),
            &mut video,
            &mut audio,
        )
        .expect("mjpeg default");
        assert_eq!(video.as_deref(), Some("mjpeg"));
        assert_eq!(audio, None);

        let mut video = None;
        let mut audio = None;
        TaskExecutor::default_transcode_codec(
            media::MediaKind::Y4m,
            std::path::Path::new("out.y4m"),
            &mut video,
            &mut audio,
        )
        .expect("rawvideo default");
        assert_eq!(video.as_deref(), Some("rawvideo"));
    }

    #[test]
    fn test_default_transcode_codec_rejects_unknown_extension() {
        let mut video = None;
        let mut audio = None;
        let err = TaskExecutor::default_transcode_codec(
            media::MediaKind::Wav,
            std::path::Path::new("out.ogg"),
            &mut video,
            &mut audio,
        )
        .expect_err("no default audio codec for .ogg");
        assert!(err.to_string().contains("audio_codec"), "{err}");

        let mut video = None;
        let mut audio = None;
        let err = TaskExecutor::default_transcode_codec(
            media::MediaKind::Y4m,
            std::path::Path::new("out.avi"),
            &mut video,
            &mut audio,
        )
        .expect_err("no default video codec for .avi");
        assert!(err.to_string().contains("video_codec"), "{err}");
    }

    #[test]
    fn test_parse_scale_param_forms() {
        let mut params = HashMap::new();
        params.insert("scale".to_string(), "640x360".to_string());
        assert_eq!(
            TaskExecutor::parse_scale_param(&params).expect("valid"),
            Some((Some(640), Some(360)))
        );

        let mut params = HashMap::new();
        params.insert("scale".to_string(), "1280x-1".to_string());
        assert_eq!(
            TaskExecutor::parse_scale_param(&params).expect("valid"),
            Some((Some(1280), None))
        );

        assert_eq!(
            TaskExecutor::parse_scale_param(&HashMap::new()).expect("absent"),
            None
        );

        let mut bad = HashMap::new();
        bad.insert("scale".to_string(), "wide".to_string());
        assert!(TaskExecutor::parse_scale_param(&bad).is_err());

        let mut bad = HashMap::new();
        bad.insert("scale".to_string(), "-1x-1".to_string());
        assert!(TaskExecutor::parse_scale_param(&bad).is_err());
    }

    #[test]
    fn test_parse_bool_param_accepts_common_spellings_and_defaults_when_absent() {
        assert_eq!(
            TaskExecutor::parse_bool_param(&HashMap::new(), "hw_accel", true).expect("absent"),
            true
        );
        assert_eq!(
            TaskExecutor::parse_bool_param(&HashMap::new(), "hw_accel", false).expect("absent"),
            false
        );

        for (raw, expected) in [
            ("true", true),
            ("True", true),
            ("TRUE", true),
            ("1", true),
            ("yes", true),
            ("on", true),
            ("false", false),
            ("0", false),
            ("no", false),
            ("Off", false),
        ] {
            let mut params = HashMap::new();
            params.insert("hw_accel".to_string(), raw.to_string());
            assert_eq!(
                TaskExecutor::parse_bool_param(&params, "hw_accel", true).expect(raw),
                expected,
                "input {raw:?}"
            );
        }

        let mut bad = HashMap::new();
        bad.insert("hw_accel".to_string(), "maybe".to_string());
        let err = TaskExecutor::parse_bool_param(&bad, "hw_accel", true)
            .expect_err("unrecognised value must be rejected");
        assert!(err.to_string().contains("maybe"), "{err}");
    }
}
