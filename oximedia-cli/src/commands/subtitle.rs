//! Subtitle/caption-domain sub-enum definitions used by `Commands`.
//!
//! Currently houses [`CaptionsCommand`]. The `Subtitle` and `Timecode`
//! variants on the top-level `Commands` enum delegate to sub-enums that
//! live alongside their handler modules (`subtitle_cmd::SubtitleCommand`,
//! `timecode_cmd::TimecodeCommand`) so they are not re-defined here.

use clap::Subcommand;
use std::path::PathBuf;

/// Captions subcommands.
#[derive(Subcommand)]
pub(crate) enum CaptionsCommand {
    /// Generate captions from audio
    Generate {
        /// Input audio/video file
        #[arg(short, long)]
        input: PathBuf,

        /// Output caption file
        #[arg(short, long)]
        output: PathBuf,

        /// Output format: srt, vtt, ass, ttml, scc
        #[arg(long, default_value = "srt")]
        format: String,

        /// Language code (e.g. en, ja)
        #[arg(long, default_value = "en")]
        language: String,

        /// Path to the ONNX caption encoder model weights.
        /// Required when the `caption-gen` feature is enabled.
        #[arg(long)]
        model: Option<PathBuf>,

        /// Path to a JSON vocabulary file (maps token ID strings to words).
        /// Required together with `--model`.
        #[arg(long)]
        vocab: Option<PathBuf>,
    },

    /// Synchronize captions to audio
    Sync {
        /// Input caption file
        #[arg(short, long)]
        input: PathBuf,

        /// Reference audio/video file
        #[arg(long)]
        reference: PathBuf,

        /// Output synced caption file
        #[arg(short, long)]
        output: PathBuf,

        /// Maximum time shift in milliseconds
        #[arg(long, default_value = "5000")]
        max_shift_ms: i64,
    },

    /// Convert between caption formats
    Convert {
        /// Input caption file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file
        #[arg(short, long)]
        output: PathBuf,

        /// Source format (auto-detected if not specified)
        #[arg(long)]
        from_format: Option<String>,

        /// Target format: srt, vtt, ass, ttml, scc
        #[arg(long)]
        to_format: String,
    },

    /// Burn captions into an uncompressed Y4M clip (requires --font)
    Burn {
        /// Input video file (uncompressed YUV4MPEG2 / .y4m)
        #[arg(long)]
        video: PathBuf,

        /// Input caption file
        #[arg(long)]
        captions: PathBuf,

        /// Output video file (YUV4MPEG2 / .y4m)
        #[arg(short, long)]
        output: PathBuf,

        /// Font size in points
        #[arg(long, default_value = "24")]
        font_size: u32,

        /// Font color (hex)
        #[arg(long, default_value = "FFFFFF")]
        font_color: String,

        /// TrueType/OpenType font used to rasterise captions (required;
        /// OxiMedia ships no font and never picks a system one)
        #[arg(long, value_name = "PATH")]
        font: Option<PathBuf>,
    },

    /// Extract captions from media
    Extract {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Output caption file
        #[arg(short, long)]
        output: PathBuf,

        /// Output format
        #[arg(long, default_value = "srt")]
        format: String,

        /// Track index to extract
        #[arg(long, default_value = "0")]
        track: usize,
    },

    /// Validate caption file against standards
    Validate {
        /// Input caption file
        #[arg(short, long)]
        input: PathBuf,

        /// Standard: fcc, wcag, cea608, cea708, ebu
        #[arg(long, default_value = "fcc")]
        standard: String,

        /// Save report to file
        #[arg(long)]
        report: Option<PathBuf>,
    },
}
