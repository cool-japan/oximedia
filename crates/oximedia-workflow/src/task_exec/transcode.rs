//! Real transcoding for [`TaskType::Transcode`](crate::task::TaskType::Transcode).
//!
//! Drives `oximedia-transcode`'s [`TranscodePipeline`] exactly the way the
//! CLI does: build a [`TranscodePipelineBuilder`], `build()`, `execute()`,
//! then verify the artefact. A task only reports success once the output file
//! exists on disk and is non-empty.
//!
//! # Preset names
//!
//! [`TaskType::Transcode::preset`](crate::task::TaskType::Transcode) is a free
//! -form string, so this module resolves it through a fixed table onto codecs
//! that `oximedia-transcode` can really encode. Anything not in the table is
//! rejected with the accepted list — a workflow never silently runs a
//! different encode than it asked for.
//!
//! | Preset | Video | Audio | Notes |
//! |---|---|---|---|
//! | `copy`, `remux`, `stream-copy`, `passthrough`, `""` | copy | copy | container remux only |
//! | `proxy`, `proxy-lowres` | `mjpeg` @ 640 wide | `pcm` | review proxy |
//! | `standard` | `mjpeg` | `flac` | |
//! | `high-quality`, `hq` | `ffv1` | `flac` | mathematically lossless |
//! | `ultra-hq`, `archive`, `lossless` | `ffv1` | `flac` | mathematically lossless |
//! | `audio-flac`, `audio-pcm`, `audio-alac` | copy | `flac`/`pcm`/`alac` | audio-only re-encode |
//! | `mjpeg`, `apv`, `mpeg2`, `ffv1`, `prores`, `rawvideo` | that codec | copy | bare video codec name |
//! | `flac`, `pcm`, `alac` | copy | that codec | bare audio codec name |
//!
//! Paired presets (video **and** audio) are defaults, not promises that both
//! streams exist: the half with no matching stream in the input is dropped by
//! [`TranscodeSettings::adapt_to_input`] and reported in the task result's
//! `preset_settings_dropped` field. A codec named explicitly in `params` is
//! never dropped.
//!
//! # Parameter overrides
//!
//! `params` refines (and overrides) the preset. Every key is validated; an
//! unrecognised key is an error rather than a silently ignored typo. See
//! [`TranscodeSettings::apply_params`] for the accepted keys.
//!
//! A decimal `fps` is taken literally (`23.976` → `23976/1000`). Use the
//! `"num/den"` form for exact broadcast rates (`"24000/1001"`,
//! `"30000/1001"`).

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;
use tracing::{debug, info};

use crate::error::{Result, WorkflowError};
use crate::task_exec::media::{detect_media_kind, MediaKind};
use crate::task_exec::{verify_non_empty_file, TaskOutcome};

use oximedia_transcode::{
    LoudnessStandard, MultiPassMode, NormalizationConfig, QualityConfig, QualityPreset,
    RateControlMode, ScaleSpec, StreamMap, TranscodePipeline,
};

/// Accepted `params` keys, used in error messages.
const ACCEPTED_PARAM_KEYS: &str = "video_codec, audio_codec, crf, video_bitrate, scale, fps, \
     start_secs, duration_secs, audio_gain_db, normalize, two_pass, map, hw_accel, track_progress";

/// Accepted preset names, used in error messages.
const ACCEPTED_PRESETS: &str = "copy/remux/stream-copy/passthrough, proxy, proxy-lowres, \
     standard, high-quality/hq, ultra-hq/archive/lossless, audio-flac, audio-pcm, audio-alac, \
     mjpeg, apv, mpeg2, ffv1, prores, rawvideo, flac, pcm, alac";

/// Fully resolved transcode settings, ready to drive the pipeline builder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscodeSettings {
    /// Video codec name passed to the pipeline (`None` = stream copy).
    pub video_codec: Option<String>,
    /// Audio codec name passed to the pipeline (`None` = stream copy).
    pub audio_codec: Option<String>,
    /// Output scale (`-vf scale`).
    pub scale: Option<(Option<u32>, Option<u32>)>,
    /// Constant-rate-factor value.
    pub crf: Option<u8>,
    /// Target video bitrate in bits per second.
    pub video_bitrate: Option<u64>,
    /// Output frame rate as `(num, den)`.
    pub fps: Option<(u32, u32)>,
    /// Start offset in seconds (`-ss`).
    pub start_secs: Option<f64>,
    /// Output duration limit in seconds (`-t`).
    pub duration_secs: Option<f64>,
    /// Audio gain in dB (`-af volume`).
    pub audio_gain_db: Option<f64>,
    /// Loudness normalisation standard.
    pub normalize: Option<LoudnessStandard>,
    /// Whether to run a two-pass encode.
    pub two_pass: bool,
    /// FFmpeg-style stream selection maps (`--map`).
    pub map: Vec<String>,
    /// Whether hardware acceleration may be used.
    pub hw_accel: bool,
    /// Whether the pipeline reports progress.
    pub track_progress: bool,
    /// `true` when `video_codec` came from `params` rather than the preset.
    ///
    /// An explicitly requested codec is **binding**: it is never dropped by
    /// [`Self::adapt_to_input`], so an impossible request surfaces the
    /// transcode layer's real error instead of being silently ignored.
    pub video_codec_explicit: bool,
    /// `true` when `audio_codec` came from `params` rather than the preset.
    pub audio_codec_explicit: bool,
}

impl TranscodeSettings {
    /// Resolves a workflow preset name onto real codecs.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowError::InvalidParameter`] for an unknown preset,
    /// naming every accepted value.
    pub fn from_preset(preset: &str) -> Result<Self> {
        let mut settings = Self {
            hw_accel: true,
            ..Self::default()
        };

        let key = preset.trim().to_ascii_lowercase();
        match key.as_str() {
            "" | "copy" | "remux" | "stream-copy" | "stream_copy" | "passthrough" => {}
            "proxy" | "proxy-lowres" | "proxy_lowres" => {
                settings.video_codec = Some("mjpeg".to_string());
                settings.audio_codec = Some("pcm".to_string());
                settings.scale = Some((Some(640), None));
            }
            "standard" => {
                settings.video_codec = Some("mjpeg".to_string());
                settings.audio_codec = Some("flac".to_string());
            }
            "high-quality" | "high_quality" | "hq" | "ultra-hq" | "ultra_hq" | "archive"
            | "lossless" => {
                settings.video_codec = Some("ffv1".to_string());
                settings.audio_codec = Some("flac".to_string());
            }
            "audio-flac" | "audio_flac" => settings.audio_codec = Some("flac".to_string()),
            "audio-pcm" | "audio_pcm" | "audio-wav" | "audio_wav" => {
                settings.audio_codec = Some("pcm".to_string());
            }
            "audio-alac" | "audio_alac" => settings.audio_codec = Some("alac".to_string()),
            "mjpeg" | "apv" | "mpeg2" | "ffv1" | "prores" | "rawvideo" => {
                settings.video_codec = Some(key);
            }
            "flac" | "pcm" | "alac" => settings.audio_codec = Some(key),
            other => {
                return Err(WorkflowError::InvalidParameter {
                    param: "preset".to_string(),
                    value: format!(
                        "{other} (accepted presets: {ACCEPTED_PRESETS}; \
                         or set `video_codec`/`audio_codec` in `params`)"
                    ),
                });
            }
        }

        Ok(settings)
    }

    /// Applies task `params` on top of the preset-derived settings.
    ///
    /// Accepted keys:
    ///
    /// | Key | JSON type | Meaning |
    /// |---|---|---|
    /// | `video_codec` / `audio_codec` | string | codec name (`"copy"` for stream copy) |
    /// | `crf` | integer 0-255 | constant rate factor |
    /// | `video_bitrate` | integer | target bitrate in bits/s |
    /// | `scale` | `"WxH"` (either side `-1`/`auto`) or `{"width":W,"height":H}` | output size |
    /// | `fps` | number, `"N"` or `"num/den"` | output frame rate |
    /// | `start_secs` / `duration_secs` | number | trim window in seconds |
    /// | `audio_gain_db` | number | audio gain in dB |
    /// | `normalize` | string | `ebu_r128`, `atsc_a85`, `apple_music`, `spotify`, `youtube`, `amazon`, `tidal`, `deezer` |
    /// | `two_pass` | bool | two-pass encoding |
    /// | `map` | array of strings | FFmpeg-style stream selectors |
    /// | `hw_accel` / `track_progress` | bool | pipeline switches |
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown key or a value of the wrong shape.
    pub fn apply_params(&mut self, params: &HashMap<String, Value>) -> Result<()> {
        for (key, value) in params {
            match key.as_str() {
                "video_codec" => {
                    self.video_codec = Some(as_codec_name(key, value)?);
                    self.video_codec_explicit = true;
                }
                "audio_codec" => {
                    self.audio_codec = Some(as_codec_name(key, value)?);
                    self.audio_codec_explicit = true;
                }
                "crf" => {
                    let raw = as_u64(key, value)?;
                    let crf = u8::try_from(raw).map_err(|_| WorkflowError::InvalidParameter {
                        param: key.clone(),
                        value: format!("{raw} (crf must be 0-255)"),
                    })?;
                    self.crf = Some(crf);
                }
                "video_bitrate" => self.video_bitrate = Some(as_u64(key, value)?),
                "scale" => self.scale = Some(parse_scale(key, value)?),
                "fps" => self.fps = Some(parse_fps(key, value)?),
                "start_secs" => self.start_secs = Some(as_f64(key, value)?),
                "duration_secs" => self.duration_secs = Some(as_f64(key, value)?),
                "audio_gain_db" => self.audio_gain_db = Some(as_f64(key, value)?),
                "normalize" => self.normalize = Some(parse_loudness_standard(key, value)?),
                "two_pass" => self.two_pass = as_bool(key, value)?,
                "hw_accel" => self.hw_accel = as_bool(key, value)?,
                "track_progress" => self.track_progress = as_bool(key, value)?,
                "map" => {
                    let Some(items) = value.as_array() else {
                        return Err(WorkflowError::InvalidParameter {
                            param: key.clone(),
                            value: format!("{value} (expected an array of stream selectors)"),
                        });
                    };
                    let mut maps = Vec::with_capacity(items.len());
                    for item in items {
                        let Some(selector) = item.as_str() else {
                            return Err(WorkflowError::InvalidParameter {
                                param: key.clone(),
                                value: format!("{item} (stream selectors must be strings)"),
                            });
                        };
                        maps.push(selector.to_string());
                    }
                    self.map = maps;
                }
                unknown => {
                    return Err(WorkflowError::InvalidParameter {
                        param: unknown.to_string(),
                        value: format!(
                            "unknown transcode parameter (accepted: {ACCEPTED_PARAM_KEYS})"
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Resolves preset and params into one settings value.
    ///
    /// # Errors
    ///
    /// Propagates preset and parameter validation failures.
    pub fn resolve(preset: &str, params: &HashMap<String, Value>) -> Result<Self> {
        let mut settings = Self::from_preset(preset)?;
        settings.apply_params(params)?;
        Ok(settings)
    }

    /// Drops preset-implied codecs the input has no stream for.
    ///
    /// A paired preset such as `standard` (`mjpeg` + `flac`) is a *default*,
    /// not a promise that both streams exist. Handing an audio codec to a
    /// video-only Y4M makes `oximedia-transcode` reject the job
    /// (`audio codec requested but Y4M input carries no audio stream`), so the
    /// preset-derived half that has no matching stream is dropped here — along
    /// with the filters that only make sense for it.
    ///
    /// Codecs named explicitly in `params` are **never** dropped: an explicit
    /// request that cannot be honoured must fail loudly.
    ///
    /// Returns the names of the settings that were dropped, for the task's
    /// result payload.
    pub fn adapt_to_input(&mut self, kind: &MediaKind) -> Vec<&'static str> {
        let mut dropped = Vec::new();
        match kind {
            MediaKind::Wav => {
                // Audio-only container: no video stream to encode or filter.
                if self.video_codec.is_some() && !self.video_codec_explicit {
                    self.video_codec = None;
                    dropped.push("video_codec");
                }
                if !self.video_codec_explicit {
                    if self.scale.take().is_some() {
                        dropped.push("scale");
                    }
                    if self.fps.take().is_some() {
                        dropped.push("fps");
                    }
                }
            }
            MediaKind::Y4m => {
                // Video-only container: no audio stream to encode or filter.
                if self.audio_codec.is_some() && !self.audio_codec_explicit {
                    self.audio_codec = None;
                    dropped.push("audio_codec");
                }
                if !self.audio_codec_explicit {
                    if self.audio_gain_db.take().is_some() {
                        dropped.push("audio_gain_db");
                    }
                    if self.normalize.take().is_some() {
                        dropped.push("normalize");
                    }
                }
            }
            // Any other container may well carry both domains (and is handled
            // by `oximedia-transcode`'s own demuxers even when this crate
            // cannot decode it); leave the settings untouched.
            MediaKind::Unsupported(_) => {}
        }
        dropped
    }

    /// Returns `true` when the settings request no re-encoding at all.
    #[must_use]
    pub fn is_pure_remux(&self) -> bool {
        let copies = |c: &Option<String>| {
            c.as_deref().is_none_or(|name| {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "copy" | "stream-copy" | "stream_copy"
                )
            })
        };
        copies(&self.video_codec)
            && copies(&self.audio_codec)
            && self.scale.is_none()
            && self.fps.is_none()
            && self.audio_gain_db.is_none()
    }
}

// ---------------------------------------------------------------------------
// Parameter parsing helpers
// ---------------------------------------------------------------------------

fn type_error(param: &str, value: &Value, expected: &str) -> WorkflowError {
    WorkflowError::InvalidParameter {
        param: param.to_string(),
        value: format!("{value} (expected {expected})"),
    }
}

fn as_codec_name(param: &str, value: &Value) -> Result<String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| type_error(param, value, "a codec name string"))
}

fn as_u64(param: &str, value: &Value) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| type_error(param, value, "a non-negative integer"))
}

fn as_f64(param: &str, value: &Value) -> Result<f64> {
    let parsed = value
        .as_f64()
        .ok_or_else(|| type_error(param, value, "a number"))?;
    if !parsed.is_finite() {
        return Err(type_error(param, value, "a finite number"));
    }
    Ok(parsed)
}

fn as_bool(param: &str, value: &Value) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| type_error(param, value, "a boolean"))
}

/// Parses one side of a `WxH` scale specification (`-1`/`auto`/`` = derive).
fn parse_scale_axis(param: &str, raw: &str, value: &Value) -> Result<Option<u32>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "-1" || trimmed.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    let parsed: u32 = trimmed
        .parse()
        .map_err(|_| type_error(param, value, "`WxH` with positive integers, `-1` or `auto`"))?;
    if parsed == 0 {
        return Err(type_error(param, value, "non-zero dimensions"));
    }
    Ok(Some(parsed))
}

/// Parses a `scale` parameter into `(width, height)`.
fn parse_scale(param: &str, value: &Value) -> Result<(Option<u32>, Option<u32>)> {
    if let Some(spec) = value.as_str() {
        let separator = if spec.contains('x') { 'x' } else { ':' };
        let Some((w, h)) = spec.split_once(separator) else {
            return Err(type_error(param, value, "`WxH` (e.g. `640x360`, `640x-1`)"));
        };
        let width = parse_scale_axis(param, w, value)?;
        let height = parse_scale_axis(param, h, value)?;
        if width.is_none() && height.is_none() {
            return Err(type_error(param, value, "at least one fixed dimension"));
        }
        return Ok((width, height));
    }

    if let Some(object) = value.as_object() {
        let width = match object.get("width") {
            Some(v) => Some(
                u32::try_from(as_u64(param, v)?)
                    .map_err(|_| type_error(param, value, "a width that fits in 32 bits"))?,
            ),
            None => None,
        };
        let height = match object.get("height") {
            Some(v) => Some(
                u32::try_from(as_u64(param, v)?)
                    .map_err(|_| type_error(param, value, "a height that fits in 32 bits"))?,
            ),
            None => None,
        };
        if width.is_none() && height.is_none() {
            return Err(type_error(param, value, "`width` and/or `height`"));
        }
        return Ok((width, height));
    }

    Err(type_error(
        param,
        value,
        "`WxH` string or {\"width\":W,\"height\":H}",
    ))
}

/// Parses an `fps` parameter into a `(num, den)` rational.
fn parse_fps(param: &str, value: &Value) -> Result<(u32, u32)> {
    if let Some(spec) = value.as_str() {
        if let Some((num, den)) = spec.split_once('/') {
            let num: u32 = num
                .trim()
                .parse()
                .map_err(|_| type_error(param, value, "`num/den` with integer numerator"))?;
            let den: u32 = den
                .trim()
                .parse()
                .map_err(|_| type_error(param, value, "`num/den` with integer denominator"))?;
            if num == 0 || den == 0 {
                return Err(type_error(param, value, "a non-zero frame rate"));
            }
            return Ok((num, den));
        }
        let whole: u32 = spec
            .trim()
            .parse()
            .map_err(|_| type_error(param, value, "a frame rate such as `25` or `30000/1001`"))?;
        if whole == 0 {
            return Err(type_error(param, value, "a non-zero frame rate"));
        }
        return Ok((whole, 1));
    }

    let numeric = as_f64(param, value)?;
    if numeric <= 0.0 {
        return Err(type_error(param, value, "a positive frame rate"));
    }
    // Represent a decimal rate exactly enough for the pipeline: x1000/1000.
    let scaled = (numeric * 1000.0).round();
    if scaled > f64::from(u32::MAX) {
        return Err(type_error(param, value, "a frame rate below 4 294 967"));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok((scaled as u32, 1000))
}

/// Parses a loudness-standard name.
fn parse_loudness_standard(param: &str, value: &Value) -> Result<LoudnessStandard> {
    let Some(name) = value.as_str() else {
        return Err(type_error(param, value, "a loudness-standard name"));
    };
    match name
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
        .as_str()
    {
        "ebu_r128" | "ebur128" | "r128" => Ok(LoudnessStandard::EbuR128),
        "atsc_a85" | "atsca85" | "a85" => Ok(LoudnessStandard::AtscA85),
        "apple_music" | "apple" | "itunes" => Ok(LoudnessStandard::AppleMusic),
        "spotify" => Ok(LoudnessStandard::Spotify),
        "youtube" => Ok(LoudnessStandard::YouTube),
        "amazon" => Ok(LoudnessStandard::Amazon),
        "tidal" => Ok(LoudnessStandard::Tidal),
        "deezer" => Ok(LoudnessStandard::Deezer),
        other => Err(WorkflowError::InvalidParameter {
            param: param.to_string(),
            value: format!(
                "{other} (accepted: ebu_r128, atsc_a85, apple_music, spotify, youtube, \
                 amazon, tidal, deezer)"
            ),
        }),
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Runs a real transcode described by a
/// [`TaskType::Transcode`](crate::task::TaskType::Transcode) task.
///
/// The input must exist and be non-empty; the output's parent directory is
/// created if needed and any stale file at the output path is removed first,
/// so the post-run non-empty check proves *this* run produced the artefact.
///
/// # Errors
///
/// Returns an error when the input is missing/empty, the preset or params are
/// invalid, the requested codec has no real encoder in this build (the
/// `oximedia-transcode` message naming the supported codecs is propagated
/// verbatim), the pipeline fails, or the output is missing/empty afterwards.
pub async fn execute_transcode(
    input: &Path,
    output: &Path,
    preset: &str,
    params: &HashMap<String, Value>,
) -> Result<TaskOutcome> {
    if !input.exists() {
        return Err(WorkflowError::FileNotFound(input.to_path_buf()));
    }
    let input_size = verify_non_empty_file(input, "Transcode input")?;

    let mut settings = TranscodeSettings::resolve(preset, params)?;
    let dropped = settings.adapt_to_input(&detect_media_kind(input)?);
    if !dropped.is_empty() {
        debug!(
            "Transcode {}: preset `{preset}` settings {dropped:?} dropped — the input has no \
             matching stream",
            input.display()
        );
    }

    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                WorkflowError::generic(format!(
                    "Cannot create transcode output directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
    }

    // Remove a stale artefact so "output exists and is non-empty" cannot be
    // satisfied by a previous run's file.
    match tokio::fs::remove_file(output).await {
        Ok(()) => debug!("Removed stale transcode output {}", output.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(WorkflowError::generic(format!(
                "Cannot clear existing transcode output {}: {e}",
                output.display()
            )));
        }
    }

    let mut builder = TranscodePipeline::builder()
        .input(input.to_path_buf())
        .output(output.to_path_buf())
        .hw_accel(settings.hw_accel)
        .track_progress(settings.track_progress);

    if let Some(codec) = &settings.video_codec {
        builder = builder.video_codec(codec.clone());
    }
    if let Some(codec) = &settings.audio_codec {
        builder = builder.audio_codec(codec.clone());
    }
    if let Some((width, height)) = settings.scale {
        builder = builder.video_scale(ScaleSpec { width, height });
    }
    if let Some((num, den)) = settings.fps {
        builder = builder.output_fps(num, den);
    }
    if let Some(start) = settings.start_secs {
        builder = builder.start_time_secs(start);
    }
    if let Some(duration) = settings.duration_secs {
        builder = builder.duration_secs(duration);
    }
    if let Some(gain) = settings.audio_gain_db {
        builder = builder.audio_gain_db(gain);
    }
    if let Some(standard) = settings.normalize {
        builder = builder.normalization(NormalizationConfig::new(standard));
    }
    if settings.two_pass {
        builder = builder.multipass(MultiPassMode::TwoPass);
    }
    if !settings.map.is_empty() {
        let mut maps = Vec::with_capacity(settings.map.len());
        for selector in &settings.map {
            let parsed =
                StreamMap::parse(selector).map_err(|e| WorkflowError::InvalidParameter {
                    param: "map".to_string(),
                    value: format!("{selector} ({e})"),
                })?;
            maps.push(parsed);
        }
        builder = builder.stream_map(maps);
    }
    if settings.crf.is_some() || settings.video_bitrate.is_some() {
        let rate_control = match (settings.crf, settings.video_bitrate) {
            (Some(crf), _) => RateControlMode::Crf(crf),
            (None, Some(bitrate)) => RateControlMode::Cbr(bitrate),
            (None, None) => unreachable!("guarded by the enclosing condition"),
        };
        builder = builder.quality(QualityConfig {
            preset: QualityPreset::Medium,
            rate_control,
            two_pass: settings.two_pass,
            lookahead: None,
            tune: None,
        });
    }

    let mut pipeline = builder.build().map_err(|e| {
        WorkflowError::generic(format!("Transcode pipeline configuration rejected: {e}"))
    })?;

    info!(
        "Transcode {} -> {} (preset `{}`, video {:?}, audio {:?})",
        input.display(),
        output.display(),
        preset,
        settings.video_codec.as_deref().unwrap_or("copy"),
        settings.audio_codec.as_deref().unwrap_or("copy"),
    );

    let transcode_output = pipeline.execute().await.map_err(|e| {
        WorkflowError::generic(format!(
            "Transcode {} -> {} failed: {e}",
            input.display(),
            output.display()
        ))
    })?;

    // The pipeline reported success — prove it on disk.
    let output_size = verify_non_empty_file(output, "Transcode")?;

    info!(
        "Transcode complete: {} ({} bytes in {:.2}s)",
        output.display(),
        output_size,
        transcode_output.encoding_time
    );

    Ok(TaskOutcome::with_data(serde_json::json!({
        "kind": "transcode",
        "input": input.display().to_string(),
        "input_bytes": input_size,
        "output": output.display().to_string(),
        "output_bytes": output_size,
        "preset": preset,
        "video_codec": settings.video_codec.clone().unwrap_or_else(|| "copy".to_string()),
        "audio_codec": settings.audio_codec.clone().unwrap_or_else(|| "copy".to_string()),
        "remux_only": settings.is_pure_remux(),
        "preset_settings_dropped": dropped,
        "encoding_time_secs": transcode_output.encoding_time,
        "speed_factor": transcode_output.speed_factor,
        "content_duration_secs": transcode_output.duration,
    }))
    .and_output(output.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn preset_copy_is_pure_remux() {
        let settings = TranscodeSettings::from_preset("copy").expect("known preset");
        assert!(settings.is_pure_remux());
        assert_eq!(settings.video_codec, None);
        assert_eq!(settings.audio_codec, None);
    }

    #[test]
    fn preset_proxy_maps_to_real_codecs() {
        let settings = TranscodeSettings::from_preset("proxy-lowres").expect("known preset");
        assert_eq!(settings.video_codec.as_deref(), Some("mjpeg"));
        assert_eq!(settings.audio_codec.as_deref(), Some("pcm"));
        assert_eq!(settings.scale, Some((Some(640), None)));
        assert!(!settings.is_pure_remux());
    }

    #[test]
    fn preset_high_quality_is_lossless_pair() {
        for name in ["high-quality", "hq", "ultra-hq", "archive", "lossless"] {
            let settings = TranscodeSettings::from_preset(name).expect("known preset");
            assert_eq!(settings.video_codec.as_deref(), Some("ffv1"), "{name}");
            assert_eq!(settings.audio_codec.as_deref(), Some("flac"), "{name}");
        }
    }

    #[test]
    fn unknown_preset_is_rejected_with_accepted_list() {
        let err = TranscodeSettings::from_preset("broadcast-magic").expect_err("unknown preset");
        let message = err.to_string();
        assert!(message.contains("broadcast-magic"), "{message}");
        assert!(message.contains("proxy-lowres"), "{message}");
    }

    #[test]
    fn params_override_preset_codecs() {
        let mut settings = TranscodeSettings::from_preset("standard").expect("known preset");
        settings
            .apply_params(&params(&[("audio_codec", Value::from("alac"))]))
            .expect("valid params");
        assert_eq!(settings.audio_codec.as_deref(), Some("alac"));
        assert_eq!(settings.video_codec.as_deref(), Some("mjpeg"));
    }

    #[test]
    fn unknown_param_key_is_rejected() {
        let mut settings = TranscodeSettings::default();
        let err = settings
            .apply_params(&params(&[("bitrate", Value::from(1_000_u64))]))
            .expect_err("typo must be rejected");
        let message = err.to_string();
        assert!(message.contains("bitrate"), "{message}");
        assert!(message.contains("video_bitrate"), "{message}");
    }

    #[test]
    fn scale_string_and_object_forms() {
        let mut settings = TranscodeSettings::default();
        settings
            .apply_params(&params(&[("scale", Value::from("640x360"))]))
            .expect("string form");
        assert_eq!(settings.scale, Some((Some(640), Some(360))));

        let mut settings = TranscodeSettings::default();
        settings
            .apply_params(&params(&[("scale", Value::from("1280x-1"))]))
            .expect("derived height");
        assert_eq!(settings.scale, Some((Some(1280), None)));

        let mut settings = TranscodeSettings::default();
        settings
            .apply_params(&params(&[(
                "scale",
                serde_json::json!({"width": 320, "height": 240}),
            )]))
            .expect("object form");
        assert_eq!(settings.scale, Some((Some(320), Some(240))));
    }

    #[test]
    fn scale_rejects_nonsense() {
        let mut settings = TranscodeSettings::default();
        assert!(settings
            .apply_params(&params(&[("scale", Value::from("wide"))]))
            .is_err());
        assert!(settings
            .apply_params(&params(&[("scale", Value::from("-1x-1"))]))
            .is_err());
        assert!(settings
            .apply_params(&params(&[("scale", Value::from(640))]))
            .is_err());
    }

    #[test]
    fn fps_forms() {
        let mut settings = TranscodeSettings::default();
        settings
            .apply_params(&params(&[("fps", Value::from("30000/1001"))]))
            .expect("rational");
        assert_eq!(settings.fps, Some((30_000, 1001)));

        let mut settings = TranscodeSettings::default();
        settings
            .apply_params(&params(&[("fps", Value::from("25"))]))
            .expect("whole");
        assert_eq!(settings.fps, Some((25, 1)));

        let mut settings = TranscodeSettings::default();
        settings
            .apply_params(&params(&[("fps", Value::from(23.976))]))
            .expect("decimal");
        assert_eq!(settings.fps, Some((23_976, 1000)));
    }

    #[test]
    fn crf_out_of_range_is_rejected() {
        let mut settings = TranscodeSettings::default();
        let err = settings
            .apply_params(&params(&[("crf", Value::from(999_u64))]))
            .expect_err("out of range");
        assert!(err.to_string().contains("crf must be 0-255"));
    }

    #[test]
    fn normalize_names_resolve() {
        let mut settings = TranscodeSettings::default();
        settings
            .apply_params(&params(&[("normalize", Value::from("EBU-R128"))]))
            .expect("standard name");
        assert!(matches!(
            settings.normalize,
            Some(LoudnessStandard::EbuR128)
        ));

        let mut settings = TranscodeSettings::default();
        assert!(settings
            .apply_params(&params(&[("normalize", Value::from("loud"))]))
            .is_err());
    }

    #[tokio::test]
    async fn missing_input_reports_file_not_found() {
        let missing = std::env::temp_dir().join(format!(
            "oximedia_wf_missing_{}_input.wav",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        let output = std::env::temp_dir().join(format!(
            "oximedia_wf_missing_{}_output.flac",
            std::process::id()
        ));

        let err = execute_transcode(&missing, &output, "audio-flac", &HashMap::new())
            .await
            .expect_err("missing input must fail");
        assert!(matches!(err, WorkflowError::FileNotFound(_)), "{err}");
    }
}
