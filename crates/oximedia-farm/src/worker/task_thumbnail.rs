//! Real thumbnail task execution.
//!
//! Split out of `executor.rs` (which still owns [`TaskExecutor`] itself and
//! dispatches to [`TaskExecutor::execute_thumbnail`] here) to keep both
//! files under the workspace's 2000-line-per-file policy; `task_transcode.rs`
//! is the analogous transcode-task split. Rust allows a type's inherent
//! methods to be defined across multiple `impl` blocks in different modules
//! within the same crate, so this file simply adds another `impl
//! TaskExecutor` block next to the one in `executor.rs`.

use super::executor::{TaskExecutor, TaskSpecification};
use super::media;
use crate::{FarmError, Result, TaskId};
use oximedia_codec::{MjpegConfig, MjpegEncoder, Plane, VideoEncoder as _, VideoFrame};
use oximedia_core::PixelFormat;
use std::path::Path;

/// Default thumbnail long-edge cap (pixels) when `spec.parameters` gives
/// neither `width` nor `height`. Aspect-preserving, never upscaled — see
/// [`TaskExecutor::fit_thumbnail_dimensions`].
const DEFAULT_THUMBNAIL_MAX_SIZE: u32 = 320;

/// Default JPEG quality (1-100) for generated thumbnails.
const DEFAULT_THUMBNAIL_QUALITY: u8 = 85;

impl TaskExecutor {
    /// Execute thumbnail generation task.
    ///
    /// Extracts one frame from a Y4M input (see
    /// [`media::extract_y4m_frame`]), downscales it with an area-averaging
    /// box filter (see [`media::box_downscale_plane`]; never upscales),
    /// re-encodes it as a baseline JPEG via [`oximedia_codec::MjpegEncoder`],
    /// and writes it to `spec.output_path`. Non-Y4M inputs, and Y4M streams
    /// using chroma layouts the MJPEG encoder cannot accept (mono,
    /// 4:4:4:alpha), return an honest error: this worker has no
    /// CLI-independent compressed-video decoder for inter frames yet, so
    /// nothing else can be thumbnailed for real.
    ///
    /// # Errors
    ///
    /// Returns [`FarmError::NotFound`] when the input is missing, and
    /// [`FarmError::Task`] for an empty output path, an unrecognised or
    /// invalid parameter, a non-Y4M or unsupported-chroma input, a frame
    /// index/timestamp beyond the stream, or a missing/empty output file
    /// after a reported success.
    pub(super) async fn execute_thumbnail(
        &self,
        task_id: TaskId,
        spec: &TaskSpecification,
    ) -> Result<Vec<u8>> {
        tracing::info!("Generating thumbnail for {}", spec.input_path);
        self.update_progress(task_id, 0.05);

        if spec.output_path.is_empty() {
            return Err(FarmError::Task(
                "thumbnail task requires a non-empty output_path".to_string(),
            ));
        }

        const ACCEPTED_THUMBNAIL_PARAMS: &str =
            "frame_index, timestamp_secs, width, height, quality";
        for key in spec.parameters.keys() {
            if !matches!(
                key.as_str(),
                "frame_index" | "timestamp_secs" | "width" | "height" | "quality"
            ) {
                return Err(FarmError::Task(format!(
                    "unknown thumbnail parameter '{key}' (accepted: {ACCEPTED_THUMBNAIL_PARAMS})"
                )));
            }
        }
        if spec.parameters.contains_key("frame_index")
            && spec.parameters.contains_key("timestamp_secs")
        {
            return Err(FarmError::Task(
                "thumbnail task accepts either `frame_index` or `timestamp_secs`, not both"
                    .to_string(),
            ));
        }

        let selector = match spec.parameters.get("frame_index") {
            Some(raw) => media::FrameSelector::Index(raw.trim().parse::<u64>().map_err(|e| {
                FarmError::Task(format!("invalid `frame_index` value '{raw}': {e}"))
            })?),
            None => match spec.parameters.get("timestamp_secs") {
                Some(raw) => {
                    media::FrameSelector::TimestampSecs(raw.trim().parse::<f64>().map_err(|e| {
                        FarmError::Task(format!("invalid `timestamp_secs` value '{raw}': {e}"))
                    })?)
                }
                None => media::FrameSelector::Index(0),
            },
        };

        let max_width = Self::parse_opt_param::<u32>(&spec.parameters, "width")?
            .unwrap_or(DEFAULT_THUMBNAIL_MAX_SIZE);
        let max_height =
            Self::parse_opt_param::<u32>(&spec.parameters, "height")?.unwrap_or(max_width);
        let quality = Self::parse_opt_param::<u8>(&spec.parameters, "quality")?
            .unwrap_or(DEFAULT_THUMBNAIL_QUALITY);
        if max_width == 0 || max_height == 0 {
            return Err(FarmError::Task(
                "thumbnail `width`/`height` must be non-zero".to_string(),
            ));
        }
        if quality == 0 {
            return Err(FarmError::Task(
                "thumbnail `quality` must be 1-100".to_string(),
            ));
        }

        self.update_progress(task_id, 0.2);
        let frame = media::extract_y4m_frame(&spec.input_path, selector).await?;
        self.update_progress(task_id, 0.5);

        let encoded = tokio::task::spawn_blocking(move || {
            Self::encode_thumbnail_jpeg(&frame, max_width, max_height, quality)
        })
        .await
        .map_err(|e| FarmError::Task(format!("thumbnail encode task join error: {e}")))??;
        self.update_progress(task_id, 0.8);

        let output = Path::new(&spec.output_path);
        if let Some(parent) = output.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    FarmError::Task(format!(
                        "cannot create thumbnail output directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
        }
        match tokio::fs::remove_file(output).await {
            Ok(()) => tracing::debug!("Removed stale thumbnail output {}", output.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(FarmError::Task(format!(
                    "cannot clear existing thumbnail output '{}': {e}",
                    output.display()
                )));
            }
        }
        tokio::fs::write(output, &encoded.jpeg).await.map_err(|e| {
            FarmError::Task(format!(
                "cannot write thumbnail output '{}': {e}",
                output.display()
            ))
        })?;

        // The encoder reported success — prove it on disk.
        let output_meta = tokio::fs::metadata(output).await.map_err(|e| {
            FarmError::Task(format!(
                "thumbnail reported success but output '{}' is unreadable: {e}",
                spec.output_path
            ))
        })?;
        if output_meta.len() == 0 {
            return Err(FarmError::Task(format!(
                "thumbnail reported success but output '{}' is empty",
                spec.output_path
            )));
        }

        self.update_progress(task_id, 1.0);

        let payload = serde_json::json!({
            "kind": "thumbnail",
            "input": spec.input_path,
            "output": spec.output_path,
            "frame_index": encoded.frame_index,
            "timestamp_secs": encoded.timestamp_secs,
            "source_width": encoded.source_width,
            "source_height": encoded.source_height,
            "width": encoded.width,
            "height": encoded.height,
            "chroma": encoded.chroma,
            "quality": quality,
            "output_bytes": output_meta.len(),
            "artifacts": [spec.output_path],
        });

        serde_json::to_vec(&payload)
            .map_err(|e| FarmError::Task(format!("failed to serialize thumbnail result: {e}")))
    }
    /// Fits `(src_w, src_h)` within `(max_w, max_h)` preserving aspect
    /// ratio, rounds both axes to even (4:2:0/4:2:2 chroma-plane
    /// requirement, matching `oximedia-transcode`'s own video-scale
    /// resolution), and never upscales.
    ///
    /// # Errors
    ///
    /// Returns [`FarmError::Task`] if the source has a zero dimension.
    fn fit_thumbnail_dimensions(
        src_w: u32,
        src_h: u32,
        max_w: u32,
        max_h: u32,
    ) -> Result<(u32, u32)> {
        if src_w == 0 || src_h == 0 {
            return Err(FarmError::Task(format!(
                "source frame has a zero dimension ({src_w}x{src_h})"
            )));
        }
        let scale = (f64::from(max_w) / f64::from(src_w))
            .min(f64::from(max_h) / f64::from(src_h))
            .min(1.0);
        let even = |v: f64| ((v.round() as u32).max(2) / 2) * 2;
        Ok((
            even(f64::from(src_w) * scale),
            even(f64::from(src_h) * scale),
        ))
    }
    /// Blocking half of [`Self::execute_thumbnail`]: pixel-format mapping,
    /// aspect-preserving box-filter downscale, and JPEG encode. Runs on a
    /// Tokio blocking thread (dispatched by the caller) since JPEG encoding
    /// is CPU-bound.
    ///
    /// # Errors
    ///
    /// Returns [`FarmError::Task`] when the frame's chroma subsampling has
    /// no MJPEG-compatible pixel format (mono, 4:4:4:alpha), the computed
    /// output dimensions are invalid, or the encoder itself fails.
    fn encode_thumbnail_jpeg(
        frame: &media::Y4mFrame,
        max_width: u32,
        max_height: u32,
        quality: u8,
    ) -> Result<ThumbnailEncoded> {
        use oximedia_container::demux::y4m::Y4mChroma;

        let pixel_format = match frame.chroma {
            Y4mChroma::C420jpeg | Y4mChroma::C420mpeg2 | Y4mChroma::C420paldv => {
                PixelFormat::Yuv420p
            }
            Y4mChroma::C422 => PixelFormat::Yuv422p,
            Y4mChroma::C444 => PixelFormat::Yuv444p,
            unsupported @ (Y4mChroma::Mono | Y4mChroma::C444alpha) => {
                return Err(FarmError::Task(format!(
                    "thumbnail extraction does not support Y4M chroma mode {unsupported:?}: \
                     the MJPEG encoder only accepts 4:2:0, 4:2:2 and 4:4:4 input"
                )));
            }
        };

        let (out_w, out_h) =
            Self::fit_thumbnail_dimensions(frame.width, frame.height, max_width, max_height)?;
        let (chroma_out_w, chroma_out_h) = match pixel_format {
            PixelFormat::Yuv420p => (out_w.div_ceil(2), out_h.div_ceil(2)),
            PixelFormat::Yuv422p => (out_w.div_ceil(2), out_h),
            PixelFormat::Yuv444p => (out_w, out_h),
            _ => unreachable!("pixel_format is one of the three YUV variants matched above"),
        };

        let (src_chroma_w, src_chroma_h) = frame.chroma_dims().ok_or_else(|| {
            FarmError::Task(
                "internal: chroma dims missing for a chroma-carrying Y4M frame".to_string(),
            )
        })?;
        let (src_y, src_u, src_v) = frame.planes();
        let (Some(src_u), Some(src_v)) = (src_u, src_v) else {
            return Err(FarmError::Task(
                "Y4M frame claims chroma subsampling but is missing U/V plane data".to_string(),
            ));
        };

        let y_out = media::box_downscale_plane(
            src_y,
            frame.width as usize,
            frame.height as usize,
            out_w as usize,
            out_h as usize,
        );
        let u_out = media::box_downscale_plane(
            src_u,
            src_chroma_w,
            src_chroma_h,
            chroma_out_w as usize,
            chroma_out_h as usize,
        );
        let v_out = media::box_downscale_plane(
            src_v,
            src_chroma_w,
            src_chroma_h,
            chroma_out_w as usize,
            chroma_out_h as usize,
        );

        let mut video_frame = VideoFrame::new(pixel_format, out_w, out_h);
        video_frame.planes = vec![
            Plane::with_dimensions(y_out, out_w as usize, out_w, out_h),
            Plane::with_dimensions(u_out, chroma_out_w as usize, chroma_out_w, chroma_out_h),
            Plane::with_dimensions(v_out, chroma_out_w as usize, chroma_out_w, chroma_out_h),
        ];

        let config = MjpegConfig::new(out_w, out_h)
            .map_err(|e| {
                FarmError::Task(format!("invalid thumbnail dimensions {out_w}x{out_h}: {e}"))
            })?
            .with_quality(quality)
            .with_pixel_format(pixel_format);
        let mut encoder = MjpegEncoder::new(config)
            .map_err(|e| FarmError::Task(format!("failed to create MJPEG encoder: {e}")))?;
        encoder
            .send_frame(&video_frame)
            .map_err(|e| FarmError::Task(format!("thumbnail JPEG encode failed: {e}")))?;
        let packet = encoder
            .receive_packet()
            .map_err(|e| FarmError::Task(format!("thumbnail JPEG encode failed: {e}")))?
            .ok_or_else(|| {
                FarmError::Task("thumbnail JPEG encoder produced no output packet".to_string())
            })?;

        let timestamp_secs = if frame.fps_num == 0 {
            0.0
        } else {
            frame.frame_index as f64 * f64::from(frame.fps_den.max(1)) / f64::from(frame.fps_num)
        };

        Ok(ThumbnailEncoded {
            frame_index: frame.frame_index,
            timestamp_secs,
            source_width: frame.width,
            source_height: frame.height,
            width: out_w,
            height: out_h,
            chroma: format!("{:?}", frame.chroma),
            jpeg: packet.data,
        })
    }
}

/// Output of [`TaskExecutor::encode_thumbnail_jpeg`]: the encoded JPEG plus
/// the metadata reported in the task result.
struct ThumbnailEncoded {
    frame_index: u64,
    timestamp_secs: f64,
    source_width: u32,
    source_height: u32,
    width: u32,
    height: u32,
    chroma: String,
    jpeg: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oximedia_farm_task_thumbnail_{}_{name}",
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

    /// Builds a thumbnail task payload with a real output path and
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
    // Real thumbnail work (no sleep-simulation)
    // -----------------------------------------------------------------

    // Real thumbnail work (no sleep-simulation)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_execute_thumbnail_y4m_produces_decodable_jpeg_with_expected_dims_and_color() {
        // 32x32 source downscaled to a 16x16 thumbnail (exercises the
        // box-filter downscale path, not just a same-size copy). Y=150
        // (mid grey), Cb(U)=200 (high -> should push toward blue), Cr(V)=60
        // (low -> should pull away from red): asserting that directional
        // bias survives encode+decode below is what would actually catch a
        // chroma-plane-sizing bug (an undersized U/V plane reads back as
        // neutral grey and would still produce a "correctly sized,
        // decodable" JPEG).
        let path = temp_path("thumb_src.y4m");
        std::fs::write(&path, y4m_single_frame(32, 32, "420jpeg", 150, 200, 60))
            .expect("write y4m");
        let out = temp_path("thumb_out.jpg");
        let _ = std::fs::remove_file(&out);

        let executor = TaskExecutor::new();
        let result = executor
            .execute(
                TaskId::new(),
                task_payload_full(
                    "thumbnail",
                    &path,
                    &out,
                    &[("width", "16"), ("height", "16")],
                ),
            )
            .await
            .expect("thumbnail task must succeed on a real Y4M file");
        assert!(result.success);

        let value: serde_json::Value =
            serde_json::from_slice(&result.output).expect("thumbnail output must be valid JSON");
        assert_eq!(value["kind"], "thumbnail");
        assert_eq!(value["source_width"], 32);
        assert_eq!(value["source_height"], 32);
        assert_eq!(value["width"], 16);
        assert_eq!(value["height"], 16);
        assert_eq!(value["frame_index"], 0);
        assert_eq!(
            value["artifacts"],
            serde_json::json!([out.to_string_lossy()])
        );

        let jpeg_bytes = std::fs::read(&out).expect("read thumbnail file");
        assert!(!jpeg_bytes.is_empty());
        assert!(value["output_bytes"].as_u64().unwrap_or(0) > 0);

        let probed =
            oximedia_codec::MjpegDecoder::probe_frame(&jpeg_bytes).expect("probe jpeg dims");
        assert_eq!(probed.width, 16);
        assert_eq!(probed.height, 16);

        use oximedia_codec::VideoDecoder as _;
        let mut decoder = oximedia_codec::MjpegDecoder::new(16, 16);
        decoder.send_packet(&jpeg_bytes, 0).expect("decode jpeg");
        let decoded = decoder
            .receive_frame()
            .expect("receive frame")
            .expect("a decoded frame must be present");
        let rgb = &decoded.planes[0].data;
        assert_eq!(rgb.len(), 16 * 16 * 3);

        let (mut sum_r, mut sum_b) = (0u64, 0u64);
        for px in rgb.chunks_exact(3) {
            sum_r += u64::from(px[0]);
            sum_b += u64::from(px[2]);
        }
        let n = (rgb.len() / 3) as u64;
        let (avg_r, avg_b) = (sum_r / n, sum_b / n);
        assert!(
            avg_b > avg_r + 100,
            "expected a strong blue bias to survive from high-Cb/low-Cr chroma \
             (a chroma-sizing bug would read back as neutral grey instead): \
             avg_r={avg_r} avg_b={avg_b}"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out);
    }

    #[tokio::test]
    async fn test_execute_thumbnail_non_y4m_input_is_honest_err() {
        let path = temp_path("thumb_notay4m.wav");
        std::fs::write(&path, wav_bytes(&sine_i16(440.0, 8_000, 400), 8_000, 1))
            .expect("write wav");
        let out = temp_path("thumb_notay4m_out.jpg");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(
                TaskId::new(),
                task_payload_full("thumbnail", &path, &out, &[]),
            )
            .await
            .expect_err("thumbnail of a non-Y4M input must fail honestly");
        assert!(err.to_string().contains("YUV4MPEG2"), "{err}");
        assert!(
            err.to_string()
                .contains("no CLI-independent compressed-video decode"),
            "{err}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_thumbnail_unsupported_chroma_is_honest_err() {
        let path = temp_path("thumb_mono.y4m");
        std::fs::write(&path, y4m_single_frame(16, 16, "mono", 128, 0, 0)).expect("write y4m");
        let out = temp_path("thumb_mono_out.jpg");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(
                TaskId::new(),
                task_payload_full("thumbnail", &path, &out, &[]),
            )
            .await
            .expect_err("mono Y4M has no chroma the MJPEG encoder can accept");
        assert!(err.to_string().contains("Mono"), "{err}");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn test_execute_thumbnail_missing_input_is_not_found() {
        let path = temp_path("thumb_missing.y4m");
        let _ = std::fs::remove_file(&path);
        let out = temp_path("thumb_missing_out.jpg");

        let executor = TaskExecutor::new();
        let err = executor
            .execute(
                TaskId::new(),
                task_payload_full("thumbnail", &path, &out, &[]),
            )
            .await
            .expect_err("missing thumbnail input must fail");
        assert!(matches!(err, FarmError::NotFound(_)), "{err}");
    }

    #[test]
    fn test_fit_thumbnail_dimensions_preserves_aspect_never_upscales_rounds_even() {
        // Downscale, preserving aspect ratio.
        assert_eq!(
            TaskExecutor::fit_thumbnail_dimensions(1000, 500, 100, 100).expect("fits"),
            (100, 50)
        );
        // Never upscale: a source smaller than the cap passes through
        // unchanged (already even).
        assert_eq!(
            TaskExecutor::fit_thumbnail_dimensions(10, 10, 100, 100).expect("fits"),
            (10, 10)
        );
        // Odd source dimensions round to even in the output.
        let (w, h) = TaskExecutor::fit_thumbnail_dimensions(15, 15, 100, 100).expect("fits");
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);

        assert!(TaskExecutor::fit_thumbnail_dimensions(0, 10, 100, 100).is_err());
    }
}
