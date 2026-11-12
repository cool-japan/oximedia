//! Reference output handlers (`oximedia version`, `oximedia info`).

use colored::Colorize;

/// Display OxiMedia version, build info, and feature set.
pub(crate) fn show_version(json: bool) {
    if json {
        let val = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "rust_version": rustc_version_str(),
            "license": env!("CARGO_PKG_LICENSE"),
            "copyright": "COOLJAPAN OU (Team KitaSan)",
            "homepage": env!("CARGO_PKG_HOMEPAGE"),
            "repository": env!("CARGO_PKG_REPOSITORY"),
            "features": ["audio","video","graph","subtitle","lut","filter","scene","qc","workflow",
                         "batch","monitor","restore","captions","streaming","image","graphics",
                         "multicam","vfx","ndi","videoip","distributed","farm","renderfarm",
                         "plugin","forensics","package","watermark","drm","dedup","archive"],
            "nmos": ["IS-04 v1.3","IS-05 v1.1","IS-07 v1.0","IS-08 v1.0","IS-09 v1.0","IS-11 v1.0"],
        });
        println!("{}", serde_json::to_string_pretty(&val).unwrap_or_default());
        return;
    }
    println!(
        "{}",
        format!("OxiMedia {}", env!("CARGO_PKG_VERSION"))
            .green()
            .bold()
    );
    println!("Built with:  Rust {}", rustc_version_str());
    println!(
        "Features:    {}",
        "audio, video, graph, subtitle, lut, filter, scene, qc, workflow, \
         batch, monitor, restore, captions, streaming, image, graphics, \
         multicam, vfx, ndi, videoip, distributed, farm, renderfarm, \
         plugin, forensics, package, watermark, drm, dedup, archive"
            .cyan()
    );
    println!(
        "NMOS:        {}",
        "IS-04 v1.3, IS-05 v1.1, IS-07 v1.0, IS-08 v1.0, IS-09 v1.0, IS-11 v1.0".cyan()
    );
    println!("License:     {}", env!("CARGO_PKG_LICENSE").yellow());
    println!("Copyright:   {}", "COOLJAPAN OU (Team KitaSan)".yellow());
    println!("Homepage:    {}", env!("CARGO_PKG_HOMEPAGE").dimmed());
    println!("Repository:  {}", env!("CARGO_PKG_REPOSITORY").dimmed());
}

/// Returns a compact Rust compiler version string (e.g. "1.77.0 (stable)").
///
/// Falls back to "unknown" if the version cannot be determined at compile time.
fn rustc_version_str() -> &'static str {
    option_env!("RUSTC_VERSION").unwrap_or("stable")
}

/// Display information about supported formats and codecs.
pub(crate) fn show_info() {
    println!(
        "{}",
        "OxiMedia - Patent-Free Multimedia Framework".green().bold()
    );
    println!();

    println!("{}", "Supported Containers:".cyan().bold());
    println!("  {} Matroska (.mkv)", "✓".green());
    println!("  {} WebM (.webm)", "✓".green());
    println!("  {} Ogg (.ogg, .opus, .oga)", "✓".green());
    println!("  {} FLAC (.flac)", "✓".green());
    println!("  {} WAV (.wav)", "✓".green());
    println!();

    println!("{}", "Supported Video Codecs (Green List):".cyan().bold());
    println!("  {} AV1 (Primary codec, best compression)", "✓".green());
    println!("  {} VP9 (Excellent quality/size ratio)", "✓".green());
    println!("  {} VP8 (Legacy support)", "✓".green());
    println!("  {} Theora (Legacy support)", "✓".green());
    println!();

    println!("{}", "Supported Audio Codecs (Green List):".cyan().bold());
    println!("  {} Opus (Primary codec, versatile)", "✓".green());
    println!("  {} Vorbis (High quality)", "✓".green());
    println!("  {} FLAC (Lossless)", "✓".green());
    println!("  {} PCM (Uncompressed)", "✓".green());
    println!();

    println!("{}", "Rejected Codecs (Patent-Encumbered):".red().bold());
    println!("  {} H.264/AVC", "✗".red());
    println!("  {} H.265/HEVC", "✗".red());
    println!("  {} AAC", "✗".red());
    println!("  {} AC-3/E-AC-3", "✗".red());
    println!("  {} DTS", "✗".red());
    println!();

    println!("{}", "FFmpeg-Compatible Options:".yellow().bold());
    println!("  -i <file>          Input file");
    println!("  -c:v <codec>       Video codec (av1, vp9, vp8)");
    println!("  -c:a <codec>       Audio codec (opus, vorbis, flac, pcm, aac, mp3)");
    println!("  -b:v <bitrate>     Video bitrate (e.g., 2M, 500k)");
    println!("  -vf <filter>       Video filter chain");
    println!("  -ss <time>         Seek to start time");
    println!("  -t <duration>      Duration limit");
    println!("  -r <fps>           Frame rate");
    println!();

    println!("{}", "Examples:".yellow().bold());
    println!("  oximedia transcode -i input.mp4 -o output.webm -c:v vp9 -b:v 2M");
    println!("  oximedia transcode -i input.mp4 -o output.webm --preset-name youtube-1080p");
    println!("  oximedia extract video.mkv frames_%04d.png --every 30");
    println!("  oximedia batch input/ output/ config.toml -j 4");
    println!("  oximedia concat video1.mkv video2.mkv -o output.mkv --method remux");
    println!("  oximedia thumbnail -i video.mkv -o thumb.png --mode auto");
    println!("  oximedia sprite -i video.mkv -o sprite.png --interval 10 --cols 5 --rows 5");
    println!("  oximedia sprite -i video.mkv -o sprite.png --count 100 --vtt --manifest");
    println!("  oximedia metadata -i video.mkv --show");
    println!("  oximedia benchmark -i test.mkv --codecs av1 vp9 --presets fast medium");
    println!("  oximedia validate video1.mkv video2.mkv --checks all --strict");
    println!("  oximedia preset list --category web");
    println!("  oximedia preset show youtube-1080p");
    println!();

    println!("{}", "Available Commands:".cyan().bold());
    println!(
        "  {} Probe media files and show format information",
        "probe".green()
    );
    println!("  {} Show supported formats and codecs", "info".green());
    println!("  {} Transcode media files", "transcode".green());
    println!("  {} Extract frames to images", "extract".green());
    println!("  {} Batch process multiple files", "batch".green());
    println!("  {} Concatenate multiple videos", "concat".green());
    println!("  {} Generate video thumbnails", "thumbnail".green());
    println!("  {} Generate video sprite sheets", "sprite".green());
    println!("  {} Edit media metadata/tags", "metadata".green());
    println!("  {} Run encoding benchmarks", "benchmark".green());
    println!("  {} Validate file integrity", "validate".green());
    println!("  {} Manage transcoding presets", "preset".green());
}
