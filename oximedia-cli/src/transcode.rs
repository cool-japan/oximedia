//! Transcoding operations for converting media files.
//!
//! Provides transcode command implementation with:
//! - Codec selection and validation
//! - Filter chain construction
//! - Format detection
//! - Multi-pass encoding
//! - Stream selection (`--map`, FFmpeg-style selectors)
//! - Seek and duration trimming (`-ss` / `-t`)

use crate::progress::{ProgressFormat, TranscodeProgress};
use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use oximedia_transcode::StreamMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Options for transcode operation.
#[derive(Debug, Clone)]
pub struct TranscodeOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub preset_name: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub video_bitrate: Option<String>,
    pub audio_bitrate: Option<String>,
    pub scale: Option<String>,
    /// Video filtergraph string (`-vf`); `scale=W:H` is wired into the real
    /// frame-level pipeline, other filters fail with a clear error.
    pub video_filter: Option<String>,
    /// Audio filtergraph string (`-af`); `volume=N`/`volume=NdB` is wired
    /// into the real frame-level pipeline, other filters fail clearly.
    pub audio_filter: Option<String>,
    /// Start time (`-ss`) in any FFmpeg duration format
    /// (`HH:MM:SS.mmm`, plain seconds, `Nh`/`Nm`/`Ns`).
    pub start_time: Option<String>,
    /// Output duration limit (`-t`), measured from the seek point, in any
    /// FFmpeg duration format.
    pub duration: Option<String>,
    /// Output frame rate (`-r`), e.g. "30", "23.976", "30000/1001"; wired
    /// into the frame-level pipeline's frame-rate converter.
    pub framerate: Option<String>,
    pub preset: String,
    pub two_pass: bool,
    pub crf: Option<u32>,
    /// Thread count (`--threads`). Has no effect in the packet-level
    /// stream-copy pipeline; a non-zero value triggers a stderr warning.
    pub threads: usize,
    pub overwrite: bool,
    /// FFmpeg-style `--map` stream selectors (e.g. `"0:v"`, `"0:a:1"`,
    /// `"-0:s"`). Empty keeps every stream.
    pub map: Vec<String>,
    /// Apply EBU R128 loudness normalization to the audio track during
    /// transcode (`--normalize-audio`). When `true`, `transcode_single_pass`
    /// and `transcode_two_pass` attach an `oximedia_transcode::NormalizationConfig`
    /// targeting the EBU R128 standard to the `TranscodePipelineBuilder`,
    /// matching the working pattern in `normalize_cmd::cmd_process`.
    pub normalize_audio: bool,
    /// Progress output format for this transcode operation.
    #[allow(dead_code)]
    pub progress_format: ProgressFormat,
}

/// Video codec targets accepted by `-c:v`.
///
/// MJPEG/APV/MPEG-2/FFV1/ProRes/rawvideo all have real encode pipelines --
/// verified against `oximedia_transcode::codec_dispatch::make_video_encoder`,
/// whose module doc is the authoritative list (each behind its own
/// `oximedia-codec` feature, all enabled by default in this workspace). AV1/
/// VP9/VP8 parse here so the pipeline can return a precise unsupported-codec
/// error instead of a generic "unknown codec" -- their encoders do not yet
/// produce a valid bitstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Mjpeg,
    Apv,
    Mpeg2,
    Ffv1,
    ProRes,
    RawVideo,
    Av1,
    Vp9,
    Vp8,
}

impl VideoCodec {
    /// Parse codec from string.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "mjpeg" | "motion-jpeg" | "motion_jpeg" => Ok(Self::Mjpeg),
            "apv" => Ok(Self::Apv),
            "mpeg2" | "mpeg2video" => Ok(Self::Mpeg2),
            "ffv1" => Ok(Self::Ffv1),
            "prores" => Ok(Self::ProRes),
            "rawvideo" | "raw" => Ok(Self::RawVideo),
            "av1" | "libaom-av1" => Ok(Self::Av1),
            "vp9" | "libvpx-vp9" => Ok(Self::Vp9),
            "vp8" | "libvpx" => Ok(Self::Vp8),
            _ => Err(anyhow!(
                "Unsupported video codec: {} (supported for re-encode: mjpeg, apv, mpeg2, ffv1, \
                 prores, rawvideo)",
                s
            )),
        }
    }

    /// Get codec name (as understood by the transcode pipeline).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Mjpeg => "mjpeg",
            Self::Apv => "apv",
            Self::Mpeg2 => "mpeg2",
            Self::Ffv1 => "ffv1",
            Self::ProRes => "prores",
            Self::RawVideo => "rawvideo",
            Self::Av1 => "av1",
            Self::Vp9 => "vp9",
            Self::Vp8 => "vp8",
        }
    }

    /// Get default CRF range for this codec.
    #[allow(dead_code)]
    pub fn default_crf(&self) -> u32 {
        match self {
            Self::Mjpeg => 85,                // JPEG quality 1-100 (higher = better)
            Self::Apv => 22,                  // qp 0-63 (lower = better)
            Self::Mpeg2 => 6,                 // qscale 1-31 (lower = better)
            Self::Ffv1 | Self::RawVideo => 0, // lossless — no quality knob
            Self::ProRes => 0,                // qscale auto
            Self::Av1 => 30,                  // 0-255 range
            Self::Vp9 => 31,                  // 0-63 range
            Self::Vp8 => 10,                  // 0-63 range
        }
    }

    /// Validate CRF value for this codec.
    pub fn validate_crf(&self, crf: u32) -> Result<()> {
        let max = match self {
            Self::Mjpeg => 100,
            Self::Apv | Self::Vp9 | Self::Vp8 => 63,
            Self::Mpeg2 => 31,
            Self::Ffv1 | Self::RawVideo => {
                if crf > 0 {
                    eprintln!(
                        "{} --crf has no effect for lossless codec {}; ignored",
                        "Warning:".yellow().bold(),
                        self.name()
                    );
                }
                return Ok(());
            }
            Self::ProRes => 31,
            Self::Av1 => 255,
        };

        if crf > max {
            Err(anyhow!(
                "CRF {} is out of range for {} (max: {})",
                crf,
                self.name(),
                max
            ))
        } else {
            Ok(())
        }
    }
}

/// Audio codec targets accepted by `-c:a`.
///
/// FLAC/PCM/ALAC/Opus have real encode pipelines; Vorbis/AAC/MP3 parse here
/// so the pipeline can return a precise unsupported-codec error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Opus,
    Vorbis,
    Flac,
    Pcm,
    Alac,
    Aac,
    Mp3,
}

impl AudioCodec {
    /// Parse codec from string.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "opus" | "libopus" => Ok(Self::Opus),
            "vorbis" | "libvorbis" => Ok(Self::Vorbis),
            "flac" => Ok(Self::Flac),
            "pcm" | "pcm_s16le" | "pcm_s24le" | "pcm_f32le" | "wav" => Ok(Self::Pcm),
            "alac" => Ok(Self::Alac),
            "aac" | "libfdk_aac" => Ok(Self::Aac),
            "mp3" | "libmp3lame" | "lame" => Ok(Self::Mp3),
            _ => Err(anyhow!(
                "Unsupported audio codec: {} (supported for re-encode: flac, pcm, alac, opus)",
                s
            )),
        }
    }

    /// Get codec name (as understood by the transcode pipeline).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Vorbis => "vorbis",
            Self::Flac => "flac",
            Self::Pcm => "pcm",
            Self::Alac => "alac",
            Self::Aac => "aac",
            Self::Mp3 => "mp3",
        }
    }
}

/// Encoder preset (affects speed/quality tradeoff).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderPreset {
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    Medium,
    Slow,
    Slower,
    Veryslow,
}

impl EncoderPreset {
    /// Parse preset from string.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "ultrafast" => Ok(Self::Ultrafast),
            "superfast" => Ok(Self::Superfast),
            "veryfast" => Ok(Self::Veryfast),
            "faster" => Ok(Self::Faster),
            "fast" => Ok(Self::Fast),
            "medium" => Ok(Self::Medium),
            "slow" => Ok(Self::Slow),
            "slower" => Ok(Self::Slower),
            "veryslow" => Ok(Self::Veryslow),
            _ => Err(anyhow!("Unknown preset: {}", s)),
        }
    }

    /// Get speed factor (higher = faster but lower quality).
    #[allow(dead_code)]
    pub fn speed_factor(&self) -> u32 {
        match self {
            Self::Ultrafast => 9,
            Self::Superfast => 8,
            Self::Veryfast => 7,
            Self::Faster => 6,
            Self::Fast => 5,
            Self::Medium => 4,
            Self::Slow => 3,
            Self::Slower => 2,
            Self::Veryslow => 1,
        }
    }
}

/// `-ss` / `-t` / `--map` values parsed into pipeline-ready form.
#[derive(Debug, Clone, Default)]
struct TrimAndMap {
    /// `-ss` start time in seconds.
    start_secs: Option<f64>,
    /// `-t` duration in seconds (measured from the seek point).
    duration_secs: Option<f64>,
    /// Parsed `--map` stream selectors.
    stream_map: Vec<StreamMap>,
}

/// Parse the `-ss` / `-t` / `--map` option strings.
///
/// Timecodes go through `oximedia_compat_ffmpeg::parse_duration`, which
/// accepts every FFmpeg duration form (`HH:MM:SS.mmm`, `MM:SS`, plain
/// seconds, `Nh`/`Nm`/`Ns`). Map selectors go through
/// `oximedia_transcode::StreamMap::parse`, whose errors list the accepted
/// selector grammar.
fn parse_trim_and_map(options: &TranscodeOptions) -> Result<TrimAndMap> {
    let start_secs = options
        .start_time
        .as_deref()
        .map(|s| {
            oximedia_compat_ffmpeg::parse_duration(s)
                .map(|d| d.as_secs_f64())
                .map_err(|e| anyhow!("invalid start time (-ss) '{s}': {e}"))
        })
        .transpose()?;

    let duration_secs = options
        .duration
        .as_deref()
        .map(|s| {
            oximedia_compat_ffmpeg::parse_duration(s)
                .map(|d| d.as_secs_f64())
                .map_err(|e| anyhow!("invalid duration (-t) '{s}': {e}"))
        })
        .transpose()?;

    if let Some(duration) = duration_secs {
        if duration <= 0.0 {
            return Err(anyhow!(
                "duration (-t) must be greater than zero, got {duration}"
            ));
        }
    }

    let stream_map = options
        .map
        .iter()
        .map(|s| StreamMap::parse(s).map_err(|e| anyhow!("{e}")))
        .collect::<Result<Vec<StreamMap>>>()?;

    Ok(TrimAndMap {
        start_secs,
        duration_secs,
        stream_map,
    })
}

/// Apply parsed `-ss` / `-t` / `--map` values to a pipeline builder.
fn apply_trim_and_map(
    mut builder: oximedia_transcode::TranscodePipelineBuilder,
    trim_and_map: &TrimAndMap,
) -> oximedia_transcode::TranscodePipelineBuilder {
    if !trim_and_map.stream_map.is_empty() {
        builder = builder.stream_map(trim_and_map.stream_map.clone());
    }
    if let Some(start) = trim_and_map.start_secs {
        builder = builder.start_time_secs(start);
    }
    if let Some(duration) = trim_and_map.duration_secs {
        builder = builder.duration_secs(duration);
    }
    builder
}

/// Main transcode function.
pub async fn transcode(mut options: TranscodeOptions) -> Result<()> {
    info!("Starting transcode operation");
    debug!("Options: {:?}", options);

    // Handle preset if specified
    if let Some(ref preset_name) = options.preset_name {
        use crate::presets::PresetManager;

        let custom_dir = PresetManager::default_custom_dir()?;
        let manager = PresetManager::with_custom_dir(&custom_dir)?;
        let preset = manager.get_preset(preset_name)?;

        info!("Using preset: {} - {}", preset.name, preset.description);

        // Apply preset settings to options
        options.video_codec = Some(preset.video.codec.clone());
        options.audio_codec = Some(preset.audio.codec.clone());
        options.video_bitrate = preset.video.bitrate.clone();
        options.audio_bitrate = preset.audio.bitrate.clone();
        options.crf = preset.video.crf;
        options.two_pass = preset.video.two_pass;

        if let Some(ref preset_name) = preset.video.preset {
            options.preset = preset_name.clone();
        }

        // Apply scale if resolution is specified
        if let (Some(width), Some(height)) = (preset.video.width, preset.video.height) {
            options.scale = Some(format!("{}:{}", width, height));
        }
    }

    // Validate input file
    validate_input(&options.input).await?;

    // Check output file
    check_output(&options.output, options.overwrite).await?;

    // Parse and validate codec options
    let video_codec = parse_video_codec(&options)?;
    let audio_codec = parse_audio_codec(&options)?;
    let preset = EncoderPreset::from_str(&options.preset)?;

    // `EncoderPreset` genuinely has nowhere real to go: traced through
    // `TranscodePipelineBuilder::quality` -> `TranscodeConfig::quality` ->
    // `frame_level::execute_video_job`, which derives its `u8` quality
    // parameter *only* from `QualityConfig::rate_control`'s `Crf(v)` case
    // (falling back to a codec default otherwise) and never reads
    // `QualityConfig::preset` at all -- setting it would be plumbing a value
    // the encoder never consults, not a real effect. That is a property of
    // *this pipeline's encoder set*, not a missing wire: MJPEG/APV are
    // intra-frame with only a quality/QP knob, MPEG-2 only a qscale knob,
    // and FFV1/rawvideo are lossless -- none has the search-effort-vs-time
    // tradeoff a "preset" describes (that concept applies to inter-frame
    // GOP encoders like AV1/VP9, whose encode does not produce real output
    // yet). Validate the value above so typos fail loudly, then warn
    // instead of silently dropping an explicit non-default request.
    if options.preset != "medium" {
        eprintln!(
            "{} --preset '{}' has no effect in this pipeline; ignored (none of MJPEG/APV/\
             MPEG-2/FFV1/ProRes/rawvideo expose a speed/quality preset knob)",
            "Warning:".yellow().bold(),
            options.preset
        );
    }

    // Validate CRF if specified
    if let Some(crf) = options.crf {
        if let Some(codec) = video_codec {
            codec.validate_crf(crf)?;
        }
    }

    // Parse bitrate if specified
    let video_bitrate = if let Some(ref br) = options.video_bitrate {
        Some(parse_bitrate(br)?)
    } else {
        None
    };

    // `-b:a` cannot take real effect on any currently-wired audio target,
    // but *why* differs by codec, so the response does too. Validate the
    // value's syntax first either way, so typos fail loudly regardless of
    // codec.
    if let Some(ref br) = options.audio_bitrate {
        parse_bitrate(br)?;
        match audio_codec {
            // Opus is untrustworthy in this build (see
            // `oximedia_transcode::frame_level::parse_audio_target`: the
            // encoder's output has not passed reference-decoder
            // verification, and the decoder fails to reconstruct real CELT
            // packets, decoding them to silence). A silent "ignored"
            // warning here would let the user believe `-c:a opus -b:a ...`
            // is a coherent, honored request; it is not usable at all --
            // refuse instead.
            Some(AudioCodec::Opus) => {
                return Err(anyhow!(
                    "--audio-bitrate was given with --audio-codec opus, but Opus is not \
                     supported for transcode in this build (the encoder is unverified against \
                     any reference decoder, and the decoder fails to reconstruct real CELT \
                     packets). Use flac, pcm, or alac, which this pipeline actually wires."
                ));
            }
            // FLAC/PCM/ALAC are the wired, real, lossless audio encoders:
            // "bitrate" is not a meaningful knob for lossless output at all,
            // not merely an unwired one.
            Some(lossless @ (AudioCodec::Flac | AudioCodec::Pcm | AudioCodec::Alac)) => {
                eprintln!(
                    "{} --audio-bitrate is not implemented yet and is ignored ({} is \
                     lossless; bitrate does not apply)",
                    "Warning:".yellow().bold(),
                    lossless.name()
                );
            }
            // Vorbis/AAC/MP3 parse as codec names but have no real encoder
            // in this pipeline at all (see `AudioCodec::from_str`'s doc);
            // --audio-bitrate is inert for the same underlying reason the
            // codec choice itself will fail later, not a distinct gap.
            Some(other) => {
                eprintln!(
                    "{} --audio-bitrate is not implemented yet and is ignored ({} has no \
                     real encoder in this build)",
                    "Warning:".yellow().bold(),
                    other.name()
                );
            }
            // No --audio-codec given means stream-copy: there is no
            // re-encode step for a bitrate to apply to.
            None => {
                eprintln!(
                    "{} --audio-bitrate is not implemented yet and is ignored (no \
                     --audio-codec was given, so the audio stream is copied, not re-encoded)",
                    "Warning:".yellow().bold()
                );
            }
        }
    }

    // Parse scale if specified
    let scale_dimensions = if let Some(ref scale) = options.scale {
        Some(parse_scale(scale)?)
    } else {
        None
    };

    // Parse -ss / -t / --map up front so invalid values fail before any
    // output is created.
    let trim_and_map = parse_trim_and_map(&options)?;

    // Parse -vf / -af / -r / --scale into frame-level filters, failing on
    // anything the pipeline cannot really apply.
    let frame_filters = parse_frame_filters(&options, scale_dimensions)?;

    // The packet/frame pipelines are single-threaded — there is nothing for
    // a thread-count knob to parallelize, so be honest instead of silently
    // dropping an explicit request (0 is the "auto" default).
    if options.threads != 0 {
        eprintln!(
            "{} --threads has no effect in this pipeline; ignored",
            "Warning:".yellow().bold()
        );
    }

    // Print transcode plan (status output; suppressed by --quiet)
    if !crate::progress::is_quiet() {
        print_transcode_plan(
            &options,
            video_codec,
            audio_codec,
            video_bitrate,
            scale_dimensions,
        );
    }

    // Perform the transcode
    if options.two_pass {
        info!("Using two-pass encoding");
        transcode_two_pass(
            &options,
            video_codec,
            audio_codec,
            preset,
            video_bitrate,
            &frame_filters,
            &trim_and_map,
        )
        .await?;
    } else {
        info!("Using single-pass encoding");
        transcode_single_pass(
            &options,
            video_codec,
            audio_codec,
            preset,
            video_bitrate,
            &frame_filters,
            &trim_and_map,
        )
        .await?;
    }

    // Print summary
    print_transcode_summary(&options.output).await?;

    Ok(())
}

/// Validate input file exists and is readable.
async fn validate_input(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("Input file does not exist: {}", path.display()));
    }

    if !path.is_file() {
        return Err(anyhow!("Input path is not a file: {}", path.display()));
    }

    let metadata = tokio::fs::metadata(path)
        .await
        .context("Failed to read input file metadata")?;

    if metadata.len() == 0 {
        return Err(anyhow!("Input file is empty"));
    }

    Ok(())
}

/// Check if output file exists and handle overwrite logic.
async fn check_output(path: &Path, overwrite: bool) -> Result<()> {
    if path.exists() {
        if overwrite {
            info!(
                "Output file exists, will be overwritten: {}",
                path.display()
            );
        } else {
            return Err(anyhow!(
                "Output file already exists: {}. Use -y to overwrite.",
                path.display()
            ));
        }
    }

    // Ensure output directory exists
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("Failed to create output directory")?;
        }
    }

    Ok(())
}

/// Parse video codec from options.
///
/// Without an explicit `-c:v`, Matroska-family outputs default to stream
/// copy (`None`) — re-encoding is opt-in — while raw-video containers
/// (`.y4m`, `.m2v`) imply their only possible codec.
fn parse_video_codec(options: &TranscodeOptions) -> Result<Option<VideoCodec>> {
    if let Some(ref codec) = options.video_codec {
        Ok(Some(VideoCodec::from_str(codec)?))
    } else {
        // Auto-detect from output extension
        if let Some(ext) = options.output.extension() {
            match ext.to_str() {
                Some("y4m") => Ok(Some(VideoCodec::RawVideo)),
                Some("m2v") | Some("mpv") => Ok(Some(VideoCodec::Mpeg2)),
                _ => Ok(None),
            }
        } else {
            Ok(None)
        }
    }
}

/// Parse audio codec from options.
///
/// Without an explicit `-c:a`, audio-only container extensions imply their
/// codec (FFmpeg semantics: `out.flac` means "encode to FLAC"), while
/// Matroska-family outputs default to stream copy (`None`).
fn parse_audio_codec(options: &TranscodeOptions) -> Result<Option<AudioCodec>> {
    if let Some(ref codec) = options.audio_codec {
        Ok(Some(AudioCodec::from_str(codec)?))
    } else {
        // Auto-detect from output extension
        if let Some(ext) = options.output.extension() {
            match ext.to_str() {
                Some("flac") => Ok(Some(AudioCodec::Flac)),
                Some("wav") => Ok(Some(AudioCodec::Pcm)),
                Some("caf") => Ok(Some(AudioCodec::Alac)),
                Some("ogg") | Some("oga") | Some("opus") => {
                    // Only imply an Opus encode when the input is a
                    // decodable audio source; Ogg→Ogg stays a stream copy.
                    let in_ext = options
                        .input
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(str::to_lowercase);
                    match in_ext.as_deref() {
                        Some("wav" | "flac") => Ok(Some(AudioCodec::Opus)),
                        _ => Ok(None),
                    }
                }
                Some("mp4") | Some("m4a") => Ok(Some(AudioCodec::Aac)),
                Some("mp3") => Ok(Some(AudioCodec::Mp3)),
                _ => Ok(None),
            }
        } else {
            Ok(None)
        }
    }
}

/// Parse bitrate string (e.g., "2M", "500k") to bits per second.
fn parse_bitrate(s: &str) -> Result<u64> {
    let s = s.trim().to_lowercase();

    if let Some(stripped) = s.strip_suffix('m') {
        let value: f64 = stripped.parse().context("Invalid bitrate format")?;
        Ok((value * 1_000_000.0) as u64)
    } else if let Some(stripped) = s.strip_suffix('k') {
        let value: f64 = stripped.parse().context("Invalid bitrate format")?;
        Ok((value * 1_000.0) as u64)
    } else {
        s.parse::<u64>().context("Invalid bitrate format")
    }
}

/// Parse scale string (e.g., "1280:720", "1920:-1") to dimensions.
fn parse_scale(s: &str) -> Result<(Option<u32>, Option<u32>)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Invalid scale format. Expected 'width:height'"));
    }

    let width = if parts[0] == "-1" {
        None
    } else {
        Some(parts[0].parse().context("Invalid width")?)
    };

    let height = if parts[1] == "-1" {
        None
    } else {
        Some(parts[1].parse().context("Invalid height")?)
    };

    Ok((width, height))
}

/// Frame filters resolved from `-vf` / `-af` / `-r` / `--scale`.
#[derive(Debug, Clone, Default)]
struct FrameFilters {
    /// Output resolution from `--scale` or `-vf scale=…`.
    scale: Option<oximedia_transcode::ScaleSpec>,
    /// Audio gain in dB from `-af volume=…`.
    gain_db: Option<f64>,
    /// Output frame rate from `-r`.
    fps: Option<(u32, u32)>,
}

/// Parse a `-vf` filtergraph. Only `scale=W:H` (or `WxH`) is implemented;
/// anything else fails loudly rather than being silently dropped.
fn parse_video_filter(vf: &str) -> Result<(Option<u32>, Option<u32>)> {
    let vf = vf.trim();
    if vf.contains(',') {
        return Err(anyhow!(
            "-vf filter chains are not supported yet; use a single 'scale=W:H' filter"
        ));
    }
    let Some(args) = vf.strip_prefix("scale=") else {
        let name = vf.split('=').next().unwrap_or(vf);
        return Err(anyhow!(
            "video filter '{name}' is not supported yet (supported: scale=W:H)"
        ));
    };
    let normalized = args.replace(['x', 'X'], ":");
    parse_scale(&normalized)
}

/// Parse an `-af` filtergraph. Only `volume=<gain>` is implemented, in
/// FFmpeg's two forms: `volume=0.5` (linear ratio) and `volume=-6dB`.
fn parse_audio_filter(af: &str) -> Result<f64> {
    let af = af.trim();
    if af.contains(',') {
        return Err(anyhow!(
            "-af filter chains are not supported yet; use a single 'volume=…' filter"
        ));
    }
    let Some(value) = af.strip_prefix("volume=") else {
        let name = af.split('=').next().unwrap_or(af);
        return Err(anyhow!(
            "audio filter '{name}' is not supported yet (supported: volume=N or volume=NdB)"
        ));
    };
    let value = value.trim();
    if let Some(db) = value
        .strip_suffix("dB")
        .or_else(|| value.strip_suffix("db"))
        .or_else(|| value.strip_suffix("DB"))
    {
        let db: f64 = db
            .trim()
            .parse()
            .with_context(|| format!("invalid -af volume gain '{value}'"))?;
        return Ok(db);
    }
    let linear: f64 = value
        .parse()
        .with_context(|| format!("invalid -af volume value '{value}'"))?;
    if linear <= 0.0 {
        return Err(anyhow!(
            "-af volume ratio must be positive, got {linear} (use e.g. volume=0.5)"
        ));
    }
    Ok(20.0 * linear.log10())
}

/// Parse `-r` frame-rate strings: "30", "23.976", "30000/1001".
fn parse_framerate(r: &str) -> Result<(u32, u32)> {
    let r = r.trim();
    if let Some((num, den)) = r.split_once('/') {
        let num: u32 = num.trim().parse().context("invalid -r numerator")?;
        let den: u32 = den.trim().parse().context("invalid -r denominator")?;
        if num == 0 || den == 0 {
            return Err(anyhow!("-r frame rate must be positive, got {r}"));
        }
        return Ok((num, den));
    }
    let fps: f64 = r
        .parse()
        .with_context(|| format!("invalid -r value '{r}'"))?;
    if !(fps.is_finite() && fps > 0.0 && fps <= 1000.0) {
        return Err(anyhow!("-r frame rate must be in (0, 1000], got {r}"));
    }
    // Represent decimals exactly enough via a /1000 base (23.976 → 23976/1000).
    let num = (fps * 1000.0).round() as u32;
    let (num, den) = if num % 1000 == 0 {
        (num / 1000, 1)
    } else {
        (num, 1000)
    };
    Ok((num, den))
}

/// Resolve `--scale`, `-vf`, `-af`, and `-r` into pipeline-ready filters.
fn parse_frame_filters(
    options: &TranscodeOptions,
    scale_dimensions: Option<(Option<u32>, Option<u32>)>,
) -> Result<FrameFilters> {
    let vf_scale = options
        .video_filter
        .as_deref()
        .map(parse_video_filter)
        .transpose()?;

    if scale_dimensions.is_some() && vf_scale.is_some() {
        return Err(anyhow!(
            "both --scale and -vf scale were given; use one of them"
        ));
    }

    let scale = vf_scale
        .or(scale_dimensions)
        .map(|(width, height)| {
            if width.is_none() && height.is_none() {
                Err(anyhow!("scale needs at least one concrete dimension"))
            } else {
                Ok(oximedia_transcode::ScaleSpec { width, height })
            }
        })
        .transpose()?;

    let gain_db = options
        .audio_filter
        .as_deref()
        .map(parse_audio_filter)
        .transpose()?;

    let fps = options
        .framerate
        .as_deref()
        .map(parse_framerate)
        .transpose()?;

    Ok(FrameFilters {
        scale,
        gain_db,
        fps,
    })
}

/// Apply resolved frame filters to a pipeline builder.
fn apply_frame_filters(
    mut builder: oximedia_transcode::TranscodePipelineBuilder,
    filters: &FrameFilters,
) -> oximedia_transcode::TranscodePipelineBuilder {
    if let Some(scale) = &filters.scale {
        builder = builder.video_scale(scale.clone());
    }
    if let Some(db) = filters.gain_db {
        builder = builder.audio_gain_db(db);
    }
    if let Some((num, den)) = filters.fps {
        builder = builder.output_fps(num, den);
    }
    builder
}

/// Print the transcode plan before starting.
///
/// Only settings that genuinely reach the pipeline are echoed here — a plan
/// line implying an effect that will not happen would be fabricated output
/// (which is why `--preset` / `--audio-bitrate` are absent: both warn on
/// stderr instead until a real encoder knob exists).
fn print_transcode_plan(
    options: &TranscodeOptions,
    video_codec: Option<VideoCodec>,
    audio_codec: Option<AudioCodec>,
    video_bitrate: Option<u64>,
    scale: Option<(Option<u32>, Option<u32>)>,
) {
    println!("{}", "Transcode Plan".cyan().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Input:", options.input.display());
    println!("{:20} {}", "Output:", options.output.display());

    if let Some(codec) = video_codec {
        println!("{:20} {}", "Video Codec:", codec.name());
    }

    if let Some(codec) = audio_codec {
        println!("{:20} {}", "Audio Codec:", codec.name());
    }

    if let Some(bitrate) = video_bitrate {
        println!("{:20} {} bps", "Video Bitrate:", bitrate);
    }

    if let Some((w, h)) = scale {
        println!(
            "{:20} {}x{}",
            "Scale:",
            w.map_or("-1".to_string(), |v| v.to_string()),
            h.map_or("-1".to_string(), |v| v.to_string())
        );
    }

    if options.two_pass {
        println!("{:20} {}", "Mode:", "Two-pass".yellow());
    }

    if let Some(crf) = options.crf {
        println!("{:20} {}", "CRF:", crf);
    }

    if let Some(ref start) = options.start_time {
        println!("{:20} {}", "Seek (-ss):", start);
    }

    if let Some(ref duration) = options.duration {
        println!("{:20} {}", "Duration (-t):", duration);
    }

    if !options.map.is_empty() {
        println!("{:20} {}", "Stream Map:", options.map.join(", "));
    }

    println!("{}", "=".repeat(60));
    println!();
}

/// Perform single-pass transcode using the real `oximedia_transcode` pipeline.
async fn transcode_single_pass(
    options: &TranscodeOptions,
    video_codec: Option<VideoCodec>,
    audio_codec: Option<AudioCodec>,
    _preset: EncoderPreset,
    video_bitrate: Option<u64>,
    filters: &FrameFilters,
    trim_and_map: &TrimAndMap,
) -> Result<()> {
    use oximedia_transcode::TranscodePipeline;

    info!("Starting single-pass encode");

    let mut builder = TranscodePipeline::builder()
        .input(options.input.clone())
        .output(options.output.clone())
        .track_progress(true);

    // Apply video codec.
    if let Some(vc) = video_codec {
        builder = builder.video_codec(vc.name().to_lowercase());
    }

    // Apply audio codec.
    if let Some(ac) = audio_codec {
        builder = builder.audio_codec(ac.name().to_lowercase());
    }

    // Apply -ss / -t / --map.
    builder = apply_trim_and_map(builder, trim_and_map);

    // Apply -vf scale / --scale, -af volume, -r.
    builder = apply_frame_filters(builder, filters);

    // Apply EBU R128 loudness normalization when requested via
    // `--normalize-audio`. Mirrors the working pattern in
    // `normalize_cmd::cmd_process`: a bare `NormalizationConfig::new(..)`
    // targeting the EBU R128 standard (-23 LUFS / -1 dBTP).
    if options.normalize_audio {
        use oximedia_transcode::{LoudnessStandard, NormalizationConfig};
        builder = builder.normalization(NormalizationConfig::new(LoudnessStandard::EbuR128));
    }

    // Apply quality / CRF config if specified.
    if let Some(crf) = options.crf {
        use oximedia_transcode::{QualityConfig, QualityPreset, RateControlMode};
        let crf_u8 = u8::try_from(crf.min(255)).unwrap_or(30);
        let qconfig = QualityConfig {
            preset: QualityPreset::Medium,
            rate_control: RateControlMode::Crf(crf_u8),
            two_pass: false,
            lookahead: None,
            tune: None,
        };
        builder = builder.quality(qconfig);
    } else if let Some(bitrate) = video_bitrate {
        use oximedia_transcode::{QualityConfig, QualityPreset, RateControlMode};
        let qconfig = QualityConfig {
            preset: QualityPreset::Medium,
            rate_control: RateControlMode::Cbr(bitrate),
            two_pass: false,
            lookahead: None,
            tune: None,
        };
        builder = builder.quality(qconfig);
    }

    let mut pipeline = builder
        .build()
        .context("Failed to build transcode pipeline")?;

    // Show a simple progress indicator while the pipeline runs.
    let progress = TranscodeProgress::new_with_format(0, options.progress_format);

    let result = pipeline.execute().await;

    progress.finish();

    match result {
        Ok(output) => {
            info!(
                "Single-pass encode complete: {} bytes in {:.2}s (speed {:.2}×)",
                output.file_size, output.encoding_time, output.speed_factor
            );
        }
        Err(e) => {
            return Err(anyhow!("Transcode pipeline failed: {}", e));
        }
    }

    Ok(())
}

/// Perform two-pass transcode using the real `oximedia_transcode` pipeline.
async fn transcode_two_pass(
    options: &TranscodeOptions,
    video_codec: Option<VideoCodec>,
    audio_codec: Option<AudioCodec>,
    preset: EncoderPreset,
    video_bitrate: Option<u64>,
    filters: &FrameFilters,
    trim_and_map: &TrimAndMap,
) -> Result<()> {
    use oximedia_transcode::{MultiPassMode, TranscodePipeline};

    info!("Starting two-pass encode");

    let mut builder = TranscodePipeline::builder()
        .input(options.input.clone())
        .output(options.output.clone())
        .multipass(MultiPassMode::TwoPass)
        .track_progress(true);

    if let Some(vc) = video_codec {
        builder = builder.video_codec(vc.name().to_lowercase());
    }
    if let Some(ac) = audio_codec {
        builder = builder.audio_codec(ac.name().to_lowercase());
    }

    // Apply -ss / -t / --map (same wiring as the single-pass path).
    builder = apply_trim_and_map(builder, trim_and_map);

    // Apply -vf scale / --scale, -af volume, -r.
    builder = apply_frame_filters(builder, filters);

    // Apply EBU R128 loudness normalization when requested via
    // `--normalize-audio` (same pattern as the single-pass path above).
    if options.normalize_audio {
        use oximedia_transcode::{LoudnessStandard, NormalizationConfig};
        builder = builder.normalization(NormalizationConfig::new(LoudnessStandard::EbuR128));
    }

    if let Some(bitrate) = video_bitrate {
        use oximedia_transcode::{QualityConfig, QualityPreset, RateControlMode};
        let qconfig = QualityConfig {
            preset: QualityPreset::Medium,
            rate_control: RateControlMode::Vbr {
                target: bitrate,
                max: bitrate + bitrate / 4,
            },
            two_pass: true,
            lookahead: Some(16),
            tune: None,
        };
        builder = builder.quality(qconfig);
    }

    // Silence unused-variable warnings for `preset` by logging it.
    debug!("Encoder preset: {:?}", preset);

    if !crate::progress::is_quiet() {
        println!("\n{}", "Two-pass transcode starting...".yellow().bold());
    }

    let mut pipeline = builder
        .build()
        .context("Failed to build two-pass transcode pipeline")?;

    let progress = TranscodeProgress::new_with_format(0, options.progress_format);
    let result = pipeline.execute().await;
    progress.finish();

    match result {
        Ok(output) => {
            info!(
                "Two-pass encode complete: {} bytes in {:.2}s (speed {:.2}×)",
                output.file_size, output.encoding_time, output.speed_factor
            );
        }
        Err(e) => {
            return Err(anyhow!("Two-pass transcode pipeline failed: {}", e));
        }
    }

    Ok(())
}

/// Print transcode summary after completion.
///
/// The output-file metadata read stays even under `--quiet` so a missing
/// output still fails loudly; only the status text is suppressed.
async fn print_transcode_summary(output: &Path) -> Result<()> {
    let metadata = fs::metadata(output).context("Failed to read output file metadata")?;

    if crate::progress::is_quiet() {
        return Ok(());
    }

    println!();
    println!("{}", "Transcode Complete".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Output File:", output.display());
    println!(
        "{:20} {:.2} MB",
        "File Size:",
        metadata.len() as f64 / 1_048_576.0
    );
    println!("{}", "=".repeat(60));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bitrate() {
        assert_eq!(parse_bitrate("2M").expect("2M should parse"), 2_000_000);
        assert_eq!(parse_bitrate("500k").expect("500k should parse"), 500_000);
        assert_eq!(parse_bitrate("1000").expect("1000 should parse"), 1000);
    }

    #[test]
    fn test_parse_scale() {
        assert_eq!(
            parse_scale("1280:720").expect("1280:720 should parse"),
            (Some(1280), Some(720))
        );
        assert_eq!(
            parse_scale("1920:-1").expect("1920:-1 should parse"),
            (Some(1920), None)
        );
        assert_eq!(
            parse_scale("-1:1080").expect("-1:1080 should parse"),
            (None, Some(1080))
        );
    }

    #[test]
    fn test_parse_video_filter_scale_forms() {
        assert_eq!(
            parse_video_filter("scale=640:480").expect("colon form"),
            (Some(640), Some(480))
        );
        assert_eq!(
            parse_video_filter("scale=640x480").expect("x form"),
            (Some(640), Some(480))
        );
        assert_eq!(
            parse_video_filter("scale=320:-1").expect("free axis"),
            (Some(320), None)
        );
    }

    #[test]
    fn test_parse_video_filter_rejects_unknown_and_chains() {
        let msg = parse_video_filter("hflip")
            .expect_err("hflip must be rejected")
            .to_string();
        assert!(msg.contains("hflip"), "must name the filter: {msg}");
        assert!(
            parse_video_filter("scale=1:1,hflip").is_err(),
            "chains must be rejected"
        );
    }

    #[test]
    fn test_parse_audio_filter_volume_forms() {
        let db = parse_audio_filter("volume=-6dB").expect("dB form");
        assert!((db + 6.0).abs() < 1e-9);
        let db = parse_audio_filter("volume=0.5").expect("linear form");
        assert!(
            (db + 6.0206).abs() < 0.001,
            "0.5 linear ≈ -6.02 dB, got {db}"
        );
        let db = parse_audio_filter("volume=2.0").expect("boost");
        assert!((db - 6.0206).abs() < 0.001);
    }

    #[test]
    fn test_parse_audio_filter_rejects_unknown() {
        let msg = parse_audio_filter("loudnorm=I=-23")
            .expect_err("loudnorm must be rejected")
            .to_string();
        assert!(msg.contains("loudnorm"), "must name the filter: {msg}");
        assert!(
            parse_audio_filter("volume=0").is_err(),
            "zero ratio invalid"
        );
        assert!(parse_audio_filter("volume=-1").is_err(), "negative ratio");
    }

    #[test]
    fn test_parse_framerate_forms() {
        assert_eq!(parse_framerate("30").expect("integer"), (30, 1));
        assert_eq!(parse_framerate("23.976").expect("decimal"), (23_976, 1_000));
        assert_eq!(
            parse_framerate("30000/1001").expect("rational"),
            (30_000, 1_001)
        );
        assert!(parse_framerate("0").is_err());
        assert!(parse_framerate("abc").is_err());
        assert!(parse_framerate("10/0").is_err());
    }

    #[test]
    fn test_frame_filters_conflict_between_scale_and_vf() {
        let mut options = options_fixture();
        options.scale = Some("640:480".to_string());
        options.video_filter = Some("scale=320:240".to_string());
        let scale_dims = Some((Some(640), Some(480)));
        assert!(
            parse_frame_filters(&options, scale_dims).is_err(),
            "--scale together with -vf scale must be rejected"
        );
    }

    #[test]
    fn test_video_codec_parsing() {
        assert_eq!(
            VideoCodec::from_str("av1").expect("av1 should parse"),
            VideoCodec::Av1
        );
        assert_eq!(
            VideoCodec::from_str("vp9").expect("vp9 should parse"),
            VideoCodec::Vp9
        );
        assert_eq!(
            VideoCodec::from_str("vp8").expect("vp8 should parse"),
            VideoCodec::Vp8
        );
        assert_eq!(
            VideoCodec::from_str("mjpeg").expect("mjpeg should parse"),
            VideoCodec::Mjpeg
        );
        assert_eq!(
            VideoCodec::from_str("apv").expect("apv should parse"),
            VideoCodec::Apv
        );
        assert_eq!(
            VideoCodec::from_str("mpeg2").expect("mpeg2 should parse"),
            VideoCodec::Mpeg2
        );
        assert_eq!(
            VideoCodec::from_str("ffv1").expect("ffv1 should parse"),
            VideoCodec::Ffv1
        );
        assert_eq!(
            VideoCodec::from_str("prores").expect("prores should parse"),
            VideoCodec::ProRes
        );
        assert_eq!(
            VideoCodec::from_str("rawvideo").expect("rawvideo should parse"),
            VideoCodec::RawVideo
        );
        assert!(VideoCodec::from_str("h264").is_err());
    }

    #[test]
    fn test_audio_codec_parsing_alac() {
        assert_eq!(
            AudioCodec::from_str("alac").expect("alac should parse"),
            AudioCodec::Alac
        );
    }

    #[test]
    fn test_audio_codec_parsing() {
        assert_eq!(
            AudioCodec::from_str("opus").expect("opus should parse"),
            AudioCodec::Opus
        );
        assert_eq!(
            AudioCodec::from_str("vorbis").expect("vorbis should parse"),
            AudioCodec::Vorbis
        );
        assert_eq!(
            AudioCodec::from_str("flac").expect("flac should parse"),
            AudioCodec::Flac
        );
        assert_eq!(
            AudioCodec::from_str("pcm").expect("pcm should parse"),
            AudioCodec::Pcm
        );
        assert_eq!(
            AudioCodec::from_str("pcm_s16le").expect("pcm_s16le should parse"),
            AudioCodec::Pcm
        );
        assert_eq!(
            AudioCodec::from_str("wav").expect("wav should parse"),
            AudioCodec::Pcm
        );
        assert_eq!(
            AudioCodec::from_str("aac").expect("aac should parse"),
            AudioCodec::Aac
        );
        assert_eq!(
            AudioCodec::from_str("libfdk_aac").expect("libfdk_aac should parse"),
            AudioCodec::Aac
        );
        assert_eq!(
            AudioCodec::from_str("mp3").expect("mp3 should parse"),
            AudioCodec::Mp3
        );
        assert_eq!(
            AudioCodec::from_str("libmp3lame").expect("libmp3lame should parse"),
            AudioCodec::Mp3
        );
        assert_eq!(
            AudioCodec::from_str("lame").expect("lame should parse"),
            AudioCodec::Mp3
        );
        assert!(AudioCodec::from_str("unknown_codec").is_err());
    }

    #[test]
    fn test_crf_validation() {
        let av1 = VideoCodec::Av1;
        assert!(av1.validate_crf(30).is_ok());
        assert!(av1.validate_crf(255).is_ok());
        assert!(av1.validate_crf(256).is_err());

        let vp9 = VideoCodec::Vp9;
        assert!(vp9.validate_crf(31).is_ok());
        assert!(vp9.validate_crf(63).is_ok());
        assert!(vp9.validate_crf(64).is_err());
    }

    // ── parse_trim_and_map ────────────────────────────────────────────────

    /// `TranscodeOptions` with every field at its CLI default.
    fn options_fixture() -> TranscodeOptions {
        TranscodeOptions {
            input: PathBuf::from("in.mkv"),
            output: PathBuf::from("out.mkv"),
            preset_name: None,
            video_codec: None,
            audio_codec: None,
            video_bitrate: None,
            audio_bitrate: None,
            scale: None,
            video_filter: None,
            audio_filter: None,
            start_time: None,
            duration: None,
            framerate: None,
            preset: "medium".to_string(),
            two_pass: false,
            crf: None,
            threads: 0,
            overwrite: false,
            map: Vec::new(),
            normalize_audio: false,
            progress_format: ProgressFormat::Plain,
        }
    }

    #[test]
    fn test_parse_trim_and_map_defaults() {
        let parsed = parse_trim_and_map(&options_fixture()).expect("defaults should parse cleanly");
        assert_eq!(parsed.start_secs, None);
        assert_eq!(parsed.duration_secs, None);
        assert!(parsed.stream_map.is_empty());
    }

    #[test]
    fn test_parse_trim_and_map_timecode_formats() {
        let mut options = options_fixture();
        options.start_time = Some("00:01:30.500".to_string());
        options.duration = Some("2.5".to_string());
        let parsed = parse_trim_and_map(&options).expect("timecodes should parse");
        let start = parsed.start_secs.expect("start must be set");
        let duration = parsed.duration_secs.expect("duration must be set");
        assert!(
            (start - 90.5).abs() < 1e-9,
            "HH:MM:SS.mmm form, got {start}"
        );
        assert!(
            (duration - 2.5).abs() < 1e-9,
            "plain seconds, got {duration}"
        );

        // Unit-suffix form.
        options.start_time = Some("2m".to_string());
        let parsed = parse_trim_and_map(&options).expect("unit suffix should parse");
        let start = parsed.start_secs.expect("start must be set");
        assert!(
            (start - 120.0).abs() < 1e-9,
            "'2m' must be 120 s, got {start}"
        );
    }

    #[test]
    fn test_parse_trim_and_map_rejects_bad_timecodes() {
        let mut options = options_fixture();
        options.start_time = Some("not-a-time".to_string());
        let msg = parse_trim_and_map(&options)
            .expect_err("bad -ss must be rejected")
            .to_string();
        assert!(msg.contains("-ss"), "error must name the flag, got: {msg}");

        let mut options = options_fixture();
        options.duration = Some("xyz".to_string());
        let msg = parse_trim_and_map(&options)
            .expect_err("bad -t must be rejected")
            .to_string();
        assert!(msg.contains("-t"), "error must name the flag, got: {msg}");
    }

    #[test]
    fn test_parse_trim_and_map_rejects_zero_duration() {
        let mut options = options_fixture();
        options.duration = Some("0".to_string());
        assert!(
            parse_trim_and_map(&options).is_err(),
            "-t 0 must be rejected"
        );
    }

    #[test]
    fn test_parse_trim_and_map_selectors() {
        let mut options = options_fixture();
        options.map = vec!["0:a".to_string(), "-0:s".to_string()];
        let parsed = parse_trim_and_map(&options).expect("valid selectors should parse");
        assert_eq!(parsed.stream_map.len(), 2);
        assert!(!parsed.stream_map[0].negative);
        assert!(parsed.stream_map[1].negative);
    }

    #[test]
    fn test_parse_trim_and_map_rejects_bad_selector_with_syntax_help() {
        let mut options = options_fixture();
        options.map = vec!["0:x".to_string()];
        let msg = parse_trim_and_map(&options)
            .expect_err("invalid selector must be rejected")
            .to_string();
        assert!(
            msg.contains("valid --map selectors"),
            "error must list the accepted grammar, got: {msg}"
        );
    }
}
