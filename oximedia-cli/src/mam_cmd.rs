//! Media Asset Management (MAM) CLI commands.
//!
//! Provides commands for ingesting, searching, cataloging, exporting, and tagging
//! media assets in a local catalog database (JSON-based).

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Command definitions
// ---------------------------------------------------------------------------

/// MAM command subcommands.
#[derive(Subcommand, Debug)]
pub enum MamCommand {
    /// Ingest media files into the asset catalog
    Ingest {
        /// Input file(s) to ingest
        #[arg(short, long, required = true)]
        input: Vec<PathBuf>,

        /// Catalog database path (JSON file)
        #[arg(long)]
        catalog: PathBuf,

        /// Comma-separated tags to apply
        #[arg(long)]
        tags: Option<String>,

        /// Collection name to assign
        #[arg(long)]
        collection: Option<String>,

        /// Recursively scan directories
        #[arg(long)]
        recursive: bool,

        /// Generate a real proxy file for each ingested asset via
        /// `oximedia_proxy::ProxyGenerator`, the same real generator behind
        /// `oximedia proxy generate`. Only decodable input classes have a
        /// real encode target today: Y4M (raw video -> MJPEG-in-Matroska)
        /// and WAV (raw PCM audio -> FLAC). Any other format produces a
        /// visible per-file warning and the asset is still cataloged, just
        /// without a proxy -- never a silently-skipped or fabricated one.
        #[arg(long)]
        generate_proxy: bool,

        /// Output directory for generated proxies (only used with
        /// `--generate-proxy`). Defaults to a `proxies/` directory next to
        /// the catalog file.
        #[arg(long)]
        proxy_dir: Option<PathBuf>,

        /// Probe each file and store technical metadata (container format,
        /// codec, dimensions, duration) on the asset record
        #[arg(long)]
        extract_metadata: bool,
    },

    /// Search the asset catalog
    Search {
        /// Catalog database path (JSON file)
        #[arg(short, long)]
        catalog: PathBuf,

        /// Search query string
        #[arg(short = 'Q', long)]
        query: String,

        /// Filter by comma-separated tags
        #[arg(long)]
        tags: Option<String>,

        /// Filter by format (e.g., mkv, webm, flac)
        #[arg(long)]
        format: Option<String>,

        /// Filter by date (from), ISO 8601
        #[arg(long)]
        date_from: Option<String>,

        /// Filter by date (to), ISO 8601
        #[arg(long)]
        date_to: Option<String>,

        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,

        /// Sort by: relevance, name, date, size
        #[arg(long, default_value = "relevance")]
        sort: String,
    },

    /// Show catalog summary and statistics
    Catalog {
        /// Catalog database path (JSON file)
        #[arg(short, long)]
        catalog: PathBuf,

        /// Show detailed statistics
        #[arg(long)]
        stats: bool,

        /// Detect and report duplicate assets
        #[arg(long)]
        duplicates: bool,
    },

    /// Export assets from catalog
    Export {
        /// Catalog database path (JSON file)
        #[arg(short, long)]
        catalog: PathBuf,

        /// Output directory for exported assets
        #[arg(short, long)]
        output: PathBuf,

        /// Filter assets by query
        #[arg(long)]
        query: Option<String>,

        /// Filter assets by collection
        #[arg(long)]
        collection: Option<String>,

        /// Export mode: copy, move, link
        #[arg(long, default_value = "copy")]
        mode: String,

        /// Manifest format: json, csv
        #[arg(long, default_value = "json")]
        manifest_format: String,
    },

    /// Add or modify tags on assets
    Tag {
        /// Catalog database path (JSON file)
        #[arg(short, long)]
        catalog: PathBuf,

        /// Target asset ID
        #[arg(long)]
        asset_id: Option<String>,

        /// Target assets by query
        #[arg(long)]
        query: Option<String>,

        /// Comma-separated tags to add
        #[arg(long)]
        add_tags: Option<String>,

        /// Comma-separated tags to remove
        #[arg(long)]
        remove_tags: Option<String>,

        /// Set or change collection assignment
        #[arg(long)]
        set_collection: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Catalog data model
// ---------------------------------------------------------------------------

/// A single media asset record.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AssetRecord {
    id: String,
    path: String,
    filename: String,
    format: String,
    size_bytes: u64,
    duration_secs: Option<f64>,
    width: Option<u32>,
    height: Option<u32>,
    codec: Option<String>,
    tags: Vec<String>,
    collection: Option<String>,
    ingested_at: String,
    checksum: String,
    metadata: HashMap<String, String>,
    /// Path to a real generated proxy file (`--generate-proxy`), if one was
    /// produced for this asset. `serde(default)` keeps catalogs written
    /// before this field existed loadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proxy_path: Option<String>,
}

/// A named collection of assets.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CollectionRecord {
    name: String,
    description: String,
    created_at: String,
}

/// The full catalog database persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CatalogDb {
    version: u32,
    assets: Vec<AssetRecord>,
    collections: Vec<CollectionRecord>,
}

// ---------------------------------------------------------------------------
// Catalog persistence helpers
// ---------------------------------------------------------------------------

fn load_catalog(path: &PathBuf) -> Result<CatalogDb> {
    if !path.exists() {
        return Ok(CatalogDb {
            version: 1,
            ..CatalogDb::default()
        });
    }
    let data = std::fs::read_to_string(path).context("Failed to read catalog file")?;
    let db: CatalogDb = serde_json::from_str(&data).context("Failed to parse catalog JSON")?;
    Ok(db)
}

fn save_catalog(path: &PathBuf, db: &CatalogDb) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).context("Failed to create catalog directory")?;
        }
    }
    let data = serde_json::to_string_pretty(db).context("Failed to serialize catalog")?;
    std::fs::write(path, data).context("Failed to write catalog file")?;
    Ok(())
}

fn generate_asset_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("asset-{:016x}", now.as_nanos())
}

fn compute_checksum(path: &std::path::Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).context("Failed to open file for checksum")?;
    let mut hasher_state: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    let mut buf = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buf)
            .context("Failed to read file for checksum")?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            hasher_state ^= u64::from(byte);
            hasher_state = hasher_state.wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("{:016x}", hasher_state))
}

fn detect_format(path: &std::path::Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("unknown")
        .to_lowercase()
}

fn now_iso8601() -> String {
    // Real RFC 3339 / ISO 8601 UTC timestamp. Catalogs written by older
    // builds stored plain epoch-seconds strings here; parse_asset_timestamp
    // accepts both, so old and new records stay comparable.
    chrono::Utc::now().to_rfc3339()
}

/// Parse a stored `ingested_at` value into Unix epoch seconds.
///
/// Accepts the current RFC 3339 form and the legacy plain epoch-seconds
/// string written by pre-0.2.0 catalogs. Returns `None` for unparseable
/// values (such records are excluded when a date filter is active — they
/// cannot be compared honestly).
fn parse_asset_timestamp(s: &str) -> Option<i64> {
    if let Ok(epoch) = s.parse::<i64>() {
        return Some(epoch);
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

/// Parse a `--date-from` / `--date-to` bound into Unix epoch seconds.
///
/// Accepts a full RFC 3339 timestamp (`2026-07-15T12:00:00Z`), a plain date
/// (`2026-07-15` — interpreted as the start of that UTC day for `--date-from`
/// and the end of it for `--date-to`), or raw epoch seconds.
fn parse_date_bound(s: &str, is_end: bool) -> Result<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.timestamp());
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let time = if is_end {
            chrono::NaiveTime::from_hms_opt(23, 59, 59)
        } else {
            chrono::NaiveTime::from_hms_opt(0, 0, 0)
        }
        .ok_or_else(|| anyhow::anyhow!("Internal error building time bound"))?;
        return Ok(date.and_time(time).and_utc().timestamp());
    }
    if let Ok(epoch) = s.parse::<i64>() {
        return Ok(epoch);
    }
    Err(anyhow::anyhow!(
        "Invalid date '{s}'. Expected RFC 3339 (2026-07-15T12:00:00Z), a date (2026-07-15), \
         or Unix epoch seconds"
    ))
}

fn parse_tags(tags: &Option<String>) -> Vec<String> {
    tags.as_ref()
        .map(|t| {
            t.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

/// Handle MAM command dispatch.
pub async fn handle_mam_command(command: MamCommand, json_output: bool) -> Result<()> {
    match command {
        MamCommand::Ingest {
            input,
            catalog,
            tags,
            collection,
            recursive,
            generate_proxy,
            proxy_dir,
            extract_metadata,
        } => {
            run_ingest(
                &input,
                &catalog,
                &tags,
                &collection,
                recursive,
                generate_proxy,
                proxy_dir.as_deref(),
                extract_metadata,
                json_output,
            )
            .await
        }
        MamCommand::Search {
            catalog,
            query,
            tags,
            format,
            date_from,
            date_to,
            limit,
            sort,
        } => {
            run_search(
                &catalog,
                &query,
                &tags,
                &format,
                date_from.as_deref(),
                date_to.as_deref(),
                limit,
                &sort,
                json_output,
            )
            .await
        }
        MamCommand::Catalog {
            catalog,
            stats,
            duplicates,
        } => run_catalog(&catalog, stats, duplicates, json_output).await,
        MamCommand::Export {
            catalog,
            output,
            query,
            collection,
            mode,
            manifest_format,
        } => {
            run_export(
                &catalog,
                &output,
                &query,
                &collection,
                &mode,
                &manifest_format,
                json_output,
            )
            .await
        }
        MamCommand::Tag {
            catalog,
            asset_id,
            query,
            add_tags,
            remove_tags,
            set_collection,
        } => {
            run_tag(
                &catalog,
                &asset_id,
                &query,
                &add_tags,
                &remove_tags,
                &set_collection,
                json_output,
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

/// Probe a media file's container and fill real technical metadata into the
/// asset record (`--extract-metadata`): codec, dimensions, duration, plus a
/// `container_format` entry and any container-level key/value metadata.
///
/// Uses `oximedia_container::MultiFormatProber` on the file head — the same
/// real prober behind the TUI mini-probe. Unrecognized files simply gain no
/// metadata (the prober reports `"unknown"`), which is recorded honestly.
fn extract_asset_metadata(path: &std::path::Path, record: &mut AssetRecord) -> Result<()> {
    use std::io::Read;

    const PROBE_BYTES: usize = 64 * 1024;
    let mut buf = vec![0u8; PROBE_BYTES];
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open {} for probing", path.display()))?;
    let n = file
        .read(&mut buf)
        .with_context(|| format!("Failed to read {} for probing", path.display()))?;
    buf.truncate(n);

    let info = oximedia_container::MultiFormatProber::probe(&buf);

    record
        .metadata
        .insert("container_format".to_string(), info.format.clone());
    if let Some(duration_ms) = info.duration_ms {
        record.duration_secs = Some(duration_ms as f64 / 1000.0);
    }
    for (key, value) in &info.metadata {
        record.metadata.insert(key.clone(), value.clone());
    }

    if let Some(video) = info.streams.iter().find(|s| s.stream_type == "video") {
        record.codec = Some(video.codec.clone());
        record.width = video.width;
        record.height = video.height;
    } else if let Some(audio) = info.streams.iter().find(|s| s.stream_type == "audio") {
        record.codec = Some(audio.codec.clone());
        if let Some(sr) = audio.sample_rate {
            record
                .metadata
                .insert("sample_rate_hz".to_string(), sr.to_string());
        }
        if let Some(ch) = audio.channels {
            record
                .metadata
                .insert("channels".to_string(), ch.to_string());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Proxy generation (real, for decodable input classes only)
// ---------------------------------------------------------------------------
//
// `oximedia_proxy::ProxyGenerator` ultimately runs every input through
// `oximedia_transcode`'s frame-level engine, whose real audio/video codec
// dispatch (`frame_level::parse_audio_target`/`parse_video_target`) accepts
// only a small set of codecs and containers -- see that module for the
// authoritative list. Two consequences shape what follows:
//
// - `ProxyGenerationSettings`'s own `default()`/named presets all choose
//   `aac`/`opus` as the audio codec, and `parse_audio_target` rejects both
//   unconditionally (Opus is untrustworthy; AAC is patent-encumbered and not
//   implemented) *before* the pipeline even inspects the input file. Reusing
//   those presets here would make every ingest proxy request fail --
//   settings below are built by hand with codecs the real dispatch accepts.
// - `ProxyEncoder::encode` only forwards `codec`/`audio_codec`/
//   `use_hw_accel` to the transcode pipeline; `scale_factor`/`bitrate`/
//   `quality_preset` are not applied. This ingest surface therefore does
//   not expose `--proxy-resolution`/`--proxy-quality` flags, which would
//   otherwise silently do nothing.
//
// Only two input classes have a real, honest proxy target given the above:
// Y4M (raw video, re-encoded to MJPEG in Matroska -- MJPEG has a real
// encoder) and WAV (raw PCM audio, re-encoded to FLAC -- lossless, real,
// and genuinely smaller than raw PCM). Every other format is refused with a
// per-format message naming the detected magic bytes.

/// The two input classes this ingest surface can genuinely proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyInputClass {
    /// YUV4MPEG2 raw video (magic `"YUV4MPEG2"`).
    Y4m,
    /// RIFF/WAVE raw PCM audio (magic `"RIFF"`/`"RF64"`).
    Wav,
}

/// Sniff the leading bytes of `path` to decide which (if any) real proxy
/// path applies. Mirrors the magic-byte checks `decode_helper`/
/// `frame_harness` already use elsewhere in this CLI, rather than trusting
/// the file extension.
fn sniff_proxy_input_class(path: &std::path::Path) -> Result<Option<ProxyInputClass>> {
    use std::io::Read;

    let mut buf = [0u8; 12];
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let n = file
        .read(&mut buf)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let head = &buf[..n];

    if head.starts_with(b"YUV4MPEG2") {
        Ok(Some(ProxyInputClass::Y4m))
    } else if head.starts_with(b"RIFF") || head.starts_with(b"RF64") {
        Ok(Some(ProxyInputClass::Wav))
    } else {
        Ok(None)
    }
}

/// Real `ProxyGenerationSettings` for each supported input class, built by
/// hand rather than via a preset (see the module note above for why).
fn proxy_settings_for(class: ProxyInputClass) -> oximedia_proxy::ProxyGenerationSettings {
    match class {
        ProxyInputClass::Y4m => oximedia_proxy::ProxyGenerationSettings {
            // Not applied by `ProxyEncoder::encode` (see module note); 1.0
            // is the honest value since no scaling actually happens.
            scale_factor: 1.0,
            codec: "mjpeg".to_string(),
            // Required to be > 0 by `ProxyGenerationSettings::validate`;
            // not itself applied.
            bitrate: 2_000_000,
            // Y4M carries no audio track; "copy" keeps the pipeline from
            // trying (and failing) to re-encode a stream that isn't there.
            audio_codec: "copy".to_string(),
            audio_bitrate: 0,
            preserve_frame_rate: true,
            preserve_timecode: true,
            preserve_metadata: true,
            container: "mkv".to_string(),
            use_hw_accel: false,
            threads: 0,
            quality_preset: "medium".to_string(),
        },
        ProxyInputClass::Wav => oximedia_proxy::ProxyGenerationSettings {
            scale_factor: 1.0,
            // WAV carries no video track; "copy" is the video no-op.
            codec: "copy".to_string(),
            bitrate: 1,
            audio_codec: "flac".to_string(),
            audio_bitrate: 0,
            preserve_frame_rate: true,
            preserve_timecode: true,
            preserve_metadata: true,
            container: "flac".to_string(),
            use_hw_accel: false,
            threads: 0,
            quality_preset: "lossless".to_string(),
        },
    }
}

/// Real proxy generation for one asset. Returns the proxy file path on
/// success.
///
/// Delegates to `oximedia_proxy::ProxyGenerator` -- the same real generator
/// `oximedia proxy generate` uses -- with settings hand-built for the
/// sniffed input class (see the module note above). Errors propagate to the
/// caller; no placeholder file is ever written.
async fn generate_asset_proxy(
    input_path: &std::path::Path,
    proxy_dir: &std::path::Path,
    class: ProxyInputClass,
) -> Result<std::path::PathBuf> {
    use oximedia_proxy::ProxyGenerator;

    std::fs::create_dir_all(proxy_dir)
        .with_context(|| format!("Failed to create proxy directory: {}", proxy_dir.display()))?;

    let stem = input_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "asset".to_string());
    let ext = match class {
        ProxyInputClass::Y4m => "mkv",
        ProxyInputClass::Wav => "flac",
    };
    let proxy_path = proxy_dir.join(format!("{stem}_proxy.{ext}"));

    let settings = proxy_settings_for(class);
    let generator = ProxyGenerator::new();
    generator
        .generate_with_settings(input_path, &proxy_path, settings)
        .await
        .with_context(|| {
            format!(
                "Proxy generation failed for {} -> {}",
                input_path.display(),
                proxy_path.display()
            )
        })?;

    Ok(proxy_path)
}

#[allow(clippy::too_many_arguments)]
async fn run_ingest(
    inputs: &[PathBuf],
    catalog: &PathBuf,
    tags: &Option<String>,
    collection: &Option<String>,
    recursive: bool,
    generate_proxy: bool,
    proxy_dir: Option<&std::path::Path>,
    extract_metadata: bool,
    json_output: bool,
) -> Result<()> {
    let mut db = load_catalog(catalog)?;
    let tag_list = parse_tags(tags);
    let mut ingested: Vec<AssetRecord> = Vec::new();

    // Default proxy output directory: `proxies/` next to the catalog file.
    let resolved_proxy_dir: std::path::PathBuf = proxy_dir.map_or_else(
        || {
            catalog
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("proxies")
        },
        std::path::Path::to_path_buf,
    );

    // Collect all files
    let mut files: Vec<PathBuf> = Vec::new();
    for p in inputs {
        if p.is_dir() {
            collect_files(p, recursive, &mut files)?;
        } else if p.is_file() {
            files.push(p.clone());
        } else {
            return Err(anyhow::anyhow!("Path not found: {}", p.display()));
        }
    }

    if files.is_empty() {
        return Err(anyhow::anyhow!("No files found to ingest"));
    }

    for file_path in &files {
        let meta = std::fs::metadata(file_path)
            .with_context(|| format!("Failed to read metadata for {}", file_path.display()))?;
        let checksum = compute_checksum(file_path)?;

        // Skip if already in catalog (by checksum)
        if db.assets.iter().any(|a| a.checksum == checksum) {
            if !json_output {
                println!(
                    "  {} {} (already in catalog)",
                    "Skip:".yellow(),
                    file_path.display()
                );
            }
            continue;
        }

        let mut record = AssetRecord {
            id: generate_asset_id(),
            path: file_path.to_string_lossy().to_string(),
            filename: file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            format: detect_format(file_path),
            size_bytes: meta.len(),
            duration_secs: None,
            width: None,
            height: None,
            codec: None,
            tags: tag_list.clone(),
            collection: collection.clone(),
            ingested_at: now_iso8601(),
            checksum,
            metadata: HashMap::new(),
            proxy_path: None,
        };

        // --extract-metadata: probe the container for real and store what it
        // reports (codec, dimensions, duration, container format).
        if extract_metadata {
            extract_asset_metadata(file_path, &mut record)?;
        }

        // --generate-proxy: real proxy generation for decodable input
        // classes (Y4M/WAV); a visible per-file warning for anything else,
        // never a silent skip or a fabricated proxy path.
        if generate_proxy {
            match sniff_proxy_input_class(file_path)? {
                Some(class) => {
                    match generate_asset_proxy(file_path, &resolved_proxy_dir, class).await {
                        Ok(proxy_path) => {
                            if !json_output && !crate::progress::is_quiet() {
                                println!(
                                    "  {} {} -> {}",
                                    "Proxy:".cyan(),
                                    file_path.display(),
                                    proxy_path.display()
                                );
                            }
                            record.proxy_path = Some(proxy_path.to_string_lossy().to_string());
                        }
                        Err(e) => {
                            eprintln!(
                                "warning: proxy generation failed for {}: {e:#}",
                                file_path.display()
                            );
                        }
                    }
                }
                None => {
                    eprintln!(
                        "warning: --generate-proxy has no real target for {} (only Y4M and \
                         WAV input are supported today); cataloging without a proxy",
                        file_path.display()
                    );
                }
            }
        }

        ingested.push(record.clone());
        db.assets.push(record);
    }

    // Ensure collection record exists
    if let Some(ref coll_name) = collection {
        if !db.collections.iter().any(|c| &c.name == coll_name) {
            db.collections.push(CollectionRecord {
                name: coll_name.clone(),
                description: String::new(),
                created_at: now_iso8601(),
            });
        }
    }

    save_catalog(catalog, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "ingest",
            "catalog": catalog.display().to_string(),
            "ingested_count": ingested.len(),
            "total_assets": db.assets.len(),
            "assets": ingested.iter().map(|a| serde_json::json!({
                "id": a.id,
                "filename": a.filename,
                "size_bytes": a.size_bytes,
                "format": a.format,
                "proxy_path": a.proxy_path,
            })).collect::<Vec<_>>(),
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "MAM Ingest".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Catalog:", catalog.display());
        println!("{:20} {}", "Files ingested:", ingested.len());
        println!("{:20} {}", "Total assets:", db.assets.len());
        println!();
        for a in &ingested {
            println!(
                "  {} {} ({} bytes, {})",
                "+".green(),
                a.filename,
                a.size_bytes,
                a.format
            );
        }
    }

    Ok(())
}

fn collect_files(dir: &PathBuf, recursive: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("Failed to read dir {}", dir.display()))?;
    for entry in entries {
        let entry = entry.context("Failed to read directory entry")?;
        let path = entry.path();
        if path.is_file() {
            out.push(path);
        } else if path.is_dir() && recursive {
            collect_files(&path, recursive, out)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_search(
    catalog: &PathBuf,
    query: &str,
    tags: &Option<String>,
    format: &Option<String>,
    date_from: Option<&str>,
    date_to: Option<&str>,
    limit: Option<u32>,
    sort: &str,
    json_output: bool,
) -> Result<()> {
    let db = load_catalog(catalog)?;
    let tag_filter = parse_tags(tags);
    let query_lower = query.to_lowercase();
    let max_results = limit.unwrap_or(50) as usize;

    // Parse date bounds up front so invalid values fail before any output.
    let from_epoch = date_from.map(|s| parse_date_bound(s, false)).transpose()?;
    let to_epoch = date_to.map(|s| parse_date_bound(s, true)).transpose()?;
    if let (Some(from), Some(to)) = (from_epoch, to_epoch) {
        if from > to {
            return Err(anyhow::anyhow!(
                "--date-from is after --date-to; no asset can match"
            ));
        }
    }

    let mut results: Vec<&AssetRecord> = db
        .assets
        .iter()
        .filter(|a| {
            // Text match on filename, path, tags, collection, format
            let text_match = a.filename.to_lowercase().contains(&query_lower)
                || a.path.to_lowercase().contains(&query_lower)
                || a.tags
                    .iter()
                    .any(|t| t.to_lowercase().contains(&query_lower))
                || a.collection
                    .as_ref()
                    .map_or(false, |c| c.to_lowercase().contains(&query_lower))
                || a.format.to_lowercase().contains(&query_lower);

            // Tag filter
            let tag_ok =
                tag_filter.is_empty() || tag_filter.iter().all(|tf| a.tags.iter().any(|t| t == tf));

            // Format filter
            let fmt_ok = format
                .as_ref()
                .map_or(true, |f| a.format.eq_ignore_ascii_case(f));

            // Ingest-date filter: compares real parsed timestamps. Records
            // whose timestamp cannot be parsed are excluded while a date
            // filter is active — they cannot be compared honestly.
            let date_ok = if from_epoch.is_none() && to_epoch.is_none() {
                true
            } else {
                match parse_asset_timestamp(&a.ingested_at) {
                    Some(epoch) => {
                        from_epoch.map_or(true, |from| epoch >= from)
                            && to_epoch.map_or(true, |to| epoch <= to)
                    }
                    None => false,
                }
            };

            text_match && tag_ok && fmt_ok && date_ok
        })
        .collect();

    // Sort
    match sort {
        "name" => results.sort_by(|a, b| a.filename.cmp(&b.filename)),
        "size" => results.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes)),
        // Numeric timestamp sort — string comparison would order legacy
        // epoch-second records against RFC 3339 records incorrectly.
        "date" => results.sort_by_key(|a| {
            std::cmp::Reverse(parse_asset_timestamp(&a.ingested_at).unwrap_or(i64::MIN))
        }),
        _ => {} // relevance = insertion order
    }

    let total = results.len();
    results.truncate(max_results);

    if json_output {
        let result = serde_json::json!({
            "command": "search",
            "query": query,
            "total": total,
            "returned": results.len(),
            "assets": results.iter().map(|a| serde_json::json!({
                "id": a.id,
                "filename": a.filename,
                "format": a.format,
                "size_bytes": a.size_bytes,
                "tags": a.tags,
                "collection": a.collection,
            })).collect::<Vec<_>>(),
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{s}");
    } else {
        println!("{}", "MAM Search".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Query:", query);
        println!("{:20} {} (showing {})", "Results:", total, results.len());
        println!();
        for (i, a) in results.iter().enumerate() {
            println!(
                "  {}. {} [{}] {} bytes{}",
                i + 1,
                a.filename.cyan(),
                a.format,
                a.size_bytes,
                a.collection
                    .as_ref()
                    .map(|c| format!(" ({})", c))
                    .unwrap_or_default()
            );
            if !a.tags.is_empty() {
                println!("     tags: {}", a.tags.join(", ").dimmed());
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

async fn run_catalog(
    catalog: &PathBuf,
    stats: bool,
    duplicates: bool,
    json_output: bool,
) -> Result<()> {
    let db = load_catalog(catalog)?;

    let total_size: u64 = db.assets.iter().map(|a| a.size_bytes).sum();
    let formats: HashMap<String, usize> = db.assets.iter().fold(HashMap::new(), |mut acc, a| {
        *acc.entry(a.format.clone()).or_insert(0) += 1;
        acc
    });

    // Detect duplicates by checksum
    let dup_groups: Vec<Vec<&AssetRecord>> = if duplicates {
        let mut checksum_map: HashMap<&str, Vec<&AssetRecord>> = HashMap::new();
        for a in &db.assets {
            checksum_map.entry(&a.checksum).or_default().push(a);
        }
        checksum_map.into_values().filter(|v| v.len() > 1).collect()
    } else {
        Vec::new()
    };

    if json_output {
        let mut result = serde_json::json!({
            "command": "catalog",
            "catalog": catalog.display().to_string(),
            "total_assets": db.assets.len(),
            "total_collections": db.collections.len(),
            "total_size_bytes": total_size,
            "formats": formats,
        });
        if duplicates {
            result["duplicate_groups"] = serde_json::json!(dup_groups.len());
        }
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{s}");
    } else {
        println!("{}", "MAM Catalog".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Catalog:", catalog.display());
        println!("{:20} {}", "Total assets:", db.assets.len());
        println!("{:20} {}", "Collections:", db.collections.len());
        println!(
            "{:20} {:.2} MB",
            "Total size:",
            total_size as f64 / (1024.0 * 1024.0)
        );

        if stats {
            println!();
            println!("{}", "Format Distribution".cyan().bold());
            println!("{}", "-".repeat(60));
            let mut fmt_vec: Vec<_> = formats.into_iter().collect();
            fmt_vec.sort_by(|a, b| b.1.cmp(&a.1));
            for (fmt, count) in &fmt_vec {
                println!("  {:12} {}", fmt, count);
            }

            if !db.collections.is_empty() {
                println!();
                println!("{}", "Collections".cyan().bold());
                println!("{}", "-".repeat(60));
                for c in &db.collections {
                    let count = db
                        .assets
                        .iter()
                        .filter(|a| a.collection.as_ref() == Some(&c.name))
                        .count();
                    println!("  {:20} {} assets", c.name, count);
                }
            }
        }

        if duplicates && !dup_groups.is_empty() {
            println!();
            println!("{}", "Duplicate Groups".yellow().bold());
            println!("{}", "-".repeat(60));
            for (i, group) in dup_groups.iter().enumerate() {
                println!("  Group {} ({} files):", i + 1, group.len());
                for a in group {
                    println!("    - {} ({})", a.filename, a.id);
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

async fn run_export(
    catalog: &PathBuf,
    output: &PathBuf,
    query: &Option<String>,
    collection: &Option<String>,
    mode: &str,
    manifest_format: &str,
    json_output: bool,
) -> Result<()> {
    let db = load_catalog(catalog)?;

    let selected: Vec<&AssetRecord> = db
        .assets
        .iter()
        .filter(|a| {
            let query_ok = query.as_ref().map_or(true, |q| {
                let ql = q.to_lowercase();
                a.filename.to_lowercase().contains(&ql)
                    || a.tags.iter().any(|t| t.to_lowercase().contains(&ql))
            });
            let coll_ok = collection
                .as_ref()
                .map_or(true, |c| a.collection.as_ref() == Some(c));
            query_ok && coll_ok
        })
        .collect();

    if selected.is_empty() {
        return Err(anyhow::anyhow!("No assets match the export criteria"));
    }

    // Ensure output directory exists
    if !output.exists() {
        std::fs::create_dir_all(output).context("Failed to create output directory")?;
    }

    let mut exported = Vec::new();
    for asset in &selected {
        let src = std::path::Path::new(&asset.path);
        if !src.exists() {
            if !json_output {
                println!("  {} {} (source missing)", "Skip:".yellow(), asset.filename);
            }
            continue;
        }
        let dest = output.join(&asset.filename);
        match mode {
            "copy" => {
                std::fs::copy(src, &dest)
                    .with_context(|| format!("Failed to copy {}", asset.filename))?;
            }
            "link" => {
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(src, &dest)
                        .with_context(|| format!("Failed to symlink {}", asset.filename))?;
                }
                #[cfg(not(unix))]
                {
                    std::fs::copy(src, &dest)
                        .with_context(|| format!("Failed to copy {}", asset.filename))?;
                }
            }
            _ => {
                std::fs::copy(src, &dest)
                    .with_context(|| format!("Failed to copy {}", asset.filename))?;
            }
        }
        exported.push(asset);
    }

    // Write manifest
    let manifest_path = output.join(format!("manifest.{manifest_format}"));
    let manifest_data: Vec<serde_json::Value> = exported
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "filename": a.filename,
                "format": a.format,
                "size_bytes": a.size_bytes,
                "checksum": a.checksum,
                "tags": a.tags,
                "collection": a.collection,
            })
        })
        .collect();
    let manifest_str =
        serde_json::to_string_pretty(&manifest_data).context("Failed to serialize manifest")?;
    std::fs::write(&manifest_path, &manifest_str).context("Failed to write manifest")?;

    if json_output {
        let result = serde_json::json!({
            "command": "export",
            "output": output.display().to_string(),
            "exported_count": exported.len(),
            "mode": mode,
            "manifest": manifest_path.display().to_string(),
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "MAM Export".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Exported:", exported.len());
        println!("{:20} {}", "Mode:", mode);
        println!("{:20} {}", "Manifest:", manifest_path.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tag
// ---------------------------------------------------------------------------

async fn run_tag(
    catalog: &PathBuf,
    asset_id: &Option<String>,
    query: &Option<String>,
    add_tags: &Option<String>,
    remove_tags: &Option<String>,
    set_collection: &Option<String>,
    json_output: bool,
) -> Result<()> {
    let mut db = load_catalog(catalog)?;
    let tags_to_add = parse_tags(add_tags);
    let tags_to_remove = parse_tags(remove_tags);

    if asset_id.is_none() && query.is_none() {
        return Err(anyhow::anyhow!(
            "Must specify either --asset-id or --query to select assets"
        ));
    }

    let mut modified_count = 0usize;
    for asset in &mut db.assets {
        let matches = if let Some(ref id) = asset_id {
            asset.id == *id
        } else if let Some(ref q) = query {
            let ql = q.to_lowercase();
            asset.filename.to_lowercase().contains(&ql)
                || asset.tags.iter().any(|t| t.to_lowercase().contains(&ql))
        } else {
            false
        };

        if !matches {
            continue;
        }

        // Add tags
        for tag in &tags_to_add {
            if !asset.tags.contains(tag) {
                asset.tags.push(tag.clone());
            }
        }
        // Remove tags
        asset.tags.retain(|t| !tags_to_remove.contains(t));
        // Set collection
        if let Some(ref coll) = set_collection {
            asset.collection = Some(coll.clone());
        }

        modified_count += 1;
    }

    save_catalog(catalog, &db)?;

    if json_output {
        let result = serde_json::json!({
            "command": "tag",
            "modified_count": modified_count,
            "tags_added": tags_to_add,
            "tags_removed": tags_to_remove,
            "collection_set": set_collection,
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "MAM Tag".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Modified assets:", modified_count);
        if !tags_to_add.is_empty() {
            println!("{:20} {}", "Tags added:", tags_to_add.join(", "));
        }
        if !tags_to_remove.is_empty() {
            println!("{:20} {}", "Tags removed:", tags_to_remove.join(", "));
        }
        if let Some(ref coll) = set_collection {
            println!("{:20} {}", "Collection:", coll);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tags_some() {
        let tags = Some("foo, bar ,baz".to_string());
        let result = parse_tags(&tags);
        assert_eq!(result, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn test_parse_tags_none() {
        let result = parse_tags(&None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_detect_format() {
        assert_eq!(detect_format(std::path::Path::new("video.mkv")), "mkv");
        assert_eq!(detect_format(std::path::Path::new("audio.FLAC")), "flac");
        assert_eq!(detect_format(std::path::Path::new("noext")), "unknown");
    }

    #[test]
    fn test_catalog_roundtrip() {
        let db = CatalogDb {
            version: 1,
            assets: vec![AssetRecord {
                id: "test-001".to_string(),
                path: std::env::temp_dir()
                    .join("test.mkv")
                    .to_string_lossy()
                    .to_string(),
                filename: "test.mkv".to_string(),
                format: "mkv".to_string(),
                size_bytes: 1024,
                duration_secs: Some(60.0),
                width: Some(1920),
                height: Some(1080),
                codec: Some("av1".to_string()),
                tags: vec!["raw".to_string()],
                collection: Some("dailies".to_string()),
                ingested_at: "1234567890".to_string(),
                checksum: "abcdef0123456789".to_string(),
                metadata: HashMap::new(),
                proxy_path: None,
            }],
            collections: vec![CollectionRecord {
                name: "dailies".to_string(),
                description: "Daily rushes".to_string(),
                created_at: "1234567890".to_string(),
            }],
        };
        let json = serde_json::to_string(&db);
        assert!(json.is_ok());
        let parsed: Result<CatalogDb, _> =
            serde_json::from_str(&json.expect("serialization should succeed"));
        assert!(parsed.is_ok());
        let parsed = parsed.expect("deserialization should succeed");
        assert_eq!(parsed.assets.len(), 1);
        assert_eq!(parsed.collections.len(), 1);
    }

    #[test]
    fn test_generate_asset_id_uniqueness() {
        let id1 = generate_asset_id();
        let id2 = generate_asset_id();
        // IDs should start with "asset-"
        assert!(id1.starts_with("asset-"));
        assert!(id2.starts_with("asset-"));
    }

    #[test]
    fn test_parse_asset_timestamp_forms() {
        // Legacy epoch-seconds string form.
        assert_eq!(parse_asset_timestamp("1234567890"), Some(1_234_567_890));
        // Current RFC 3339 form.
        let epoch = parse_asset_timestamp("2026-07-15T00:00:00+00:00").expect("rfc3339");
        assert_eq!(epoch, 1_784_073_600);
        // Garbage is None, never a bogus comparison value.
        assert_eq!(parse_asset_timestamp("not-a-date"), None);
        // now_iso8601 output must round-trip through the parser.
        assert!(parse_asset_timestamp(&now_iso8601()).is_some());
    }

    #[test]
    fn test_parse_date_bound_forms() {
        // Plain date: from = start of day, to = end of day (inclusive).
        let from = parse_date_bound("2026-07-15", false).expect("date from");
        let to = parse_date_bound("2026-07-15", true).expect("date to");
        assert_eq!(from, 1_784_073_600);
        assert_eq!(to - from, 86_399, "end-of-day bound must span the day");
        // RFC 3339 and epoch forms.
        assert_eq!(
            parse_date_bound("2026-07-15T12:00:00Z", false).expect("rfc3339"),
            1_784_116_800
        );
        assert_eq!(parse_date_bound("1234", false).expect("epoch"), 1234);
        // Garbage errors with the accepted grammar.
        let msg = parse_date_bound("yesterday", false)
            .expect_err("must reject")
            .to_string();
        assert!(
            msg.contains("RFC 3339"),
            "must explain accepted forms: {msg}"
        );
    }

    /// End-to-end proof that `--date-from` / `--date-to` genuinely filter:
    /// two assets with known ingest dates, a window matching only one.
    #[tokio::test]
    async fn test_search_date_filter_is_real() {
        let dir = std::env::temp_dir();
        let catalog = dir.join("oximedia_mam_date_filter_test.json");
        std::fs::remove_file(&catalog).ok();

        let mk_asset = |id: &str, name: &str, ingested_at: &str| AssetRecord {
            id: id.to_string(),
            path: dir.join(name).to_string_lossy().to_string(),
            filename: name.to_string(),
            format: "wav".to_string(),
            size_bytes: 10,
            duration_secs: None,
            width: None,
            height: None,
            codec: None,
            tags: vec![],
            collection: None,
            ingested_at: ingested_at.to_string(),
            checksum: id.to_string(),
            metadata: HashMap::new(),
            proxy_path: None,
        };

        let db = CatalogDb {
            version: 1,
            assets: vec![
                mk_asset("a-old", "old.wav", "2026-01-05T10:00:00+00:00"),
                // Legacy epoch form: 2026-07-10T00:00:00Z = 1783641600.
                mk_asset("a-new", "new.wav", "1783641600"),
            ],
            collections: vec![],
        };
        save_catalog(&catalog, &db).expect("save catalog");

        // Window covering only July 2026 must match only the new asset —
        // and must do so across BOTH stored timestamp formats.
        run_search(
            &catalog,
            "wav",
            &None,
            &None,
            Some("2026-07-01"),
            Some("2026-07-31"),
            None,
            "relevance",
            true,
        )
        .await
        .expect("search must succeed");

        // Assert by re-running the same filter logic the search uses.
        let from = parse_date_bound("2026-07-01", false).expect("from");
        let to = parse_date_bound("2026-07-31", true).expect("to");
        let loaded = load_catalog(&catalog).expect("load");
        let matched: Vec<&AssetRecord> = loaded
            .assets
            .iter()
            .filter(|a| {
                parse_asset_timestamp(&a.ingested_at)
                    .map(|e| e >= from && e <= to)
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(matched.len(), 1, "exactly one asset in the July window");
        assert_eq!(matched[0].id, "a-new");

        std::fs::remove_file(&catalog).ok();
    }

    /// `--extract-metadata` must store real probed values for a real WAV file.
    #[test]
    fn test_extract_asset_metadata_wav() {
        let dir = std::env::temp_dir();
        let wav_path = dir.join("oximedia_mam_probe_test.wav");

        // Minimal valid WAV: RIFF header + fmt + tiny data chunk.
        let sample_rate: u32 = 44_100;
        let data: Vec<u8> = vec![0u8; 32];
        let mut wav: Vec<u8> = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        std::fs::write(&wav_path, &wav).expect("write wav");

        let mut record = AssetRecord {
            id: "probe-test".to_string(),
            path: wav_path.to_string_lossy().to_string(),
            filename: "oximedia_mam_probe_test.wav".to_string(),
            format: "wav".to_string(),
            size_bytes: wav.len() as u64,
            duration_secs: None,
            width: None,
            height: None,
            codec: None,
            tags: vec![],
            collection: None,
            ingested_at: now_iso8601(),
            checksum: "x".to_string(),
            metadata: HashMap::new(),
            proxy_path: None,
        };

        extract_asset_metadata(&wav_path, &mut record).expect("probe must succeed");
        assert_eq!(
            record.metadata.get("container_format").map(String::as_str),
            Some("wav"),
            "real prober must identify the WAV container, got {:?}",
            record.metadata
        );

        std::fs::remove_file(&wav_path).ok();
    }

    // ── --generate-proxy: real proxy generation for Y4M/WAV ────────────────

    /// Build a minimal single-frame 4:2:0 Y4M clip in memory.
    fn make_test_y4m(width: u32, height: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(format!("YUV4MPEG2 W{width} H{height} C420jpeg\n").as_bytes());
        buf.extend_from_slice(b"FRAME\n");
        buf.extend(std::iter::repeat_n(126u8, (width * height) as usize));
        let chroma_w = width.div_ceil(2);
        let chroma_h = height.div_ceil(2);
        buf.extend(std::iter::repeat_n(
            128u8,
            (chroma_w * chroma_h) as usize * 2,
        ));
        buf
    }

    /// Build a minimal valid WAV file (real sine samples, not silence).
    fn make_test_wav(sample_rate: u32, num_samples: u32) -> Vec<u8> {
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * u32::from(bits_per_sample / 8);
        let data_size = num_samples * u32::from(bits_per_sample / 8);
        let mut buf = Vec::with_capacity(44 + data_size as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_size).to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&bits_per_sample.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            let pcm = (sample * 20000.0) as i16;
            buf.extend_from_slice(&pcm.to_le_bytes());
        }
        buf
    }

    #[test]
    fn test_sniff_proxy_input_class_y4m() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_mam_sniff_test.y4m");
        std::fs::write(&path, make_test_y4m(4, 4)).expect("write y4m");

        assert_eq!(
            sniff_proxy_input_class(&path).expect("sniff must succeed"),
            Some(ProxyInputClass::Y4m)
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_sniff_proxy_input_class_wav() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_mam_sniff_test.wav");
        std::fs::write(&path, make_test_wav(8000, 80)).expect("write wav");

        assert_eq!(
            sniff_proxy_input_class(&path).expect("sniff must succeed"),
            Some(ProxyInputClass::Wav)
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_sniff_proxy_input_class_unrecognized_is_none() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_mam_sniff_test.bin");
        std::fs::write(&path, b"not a known media magic").expect("write file");

        assert_eq!(
            sniff_proxy_input_class(&path).expect("sniff must succeed"),
            None
        );
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_run_ingest_generates_real_y4m_proxy() {
        let dir = std::env::temp_dir().join("oximedia_mam_ingest_proxy_y4m");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let input = dir.join("clip.y4m");
        std::fs::write(&input, make_test_y4m(16, 16)).expect("write y4m fixture");
        let catalog = dir.join("catalog.json");
        let proxy_dir = dir.join("proxies");

        run_ingest(
            std::slice::from_ref(&input),
            &catalog,
            &None,
            &None,
            false,
            true,
            Some(&proxy_dir),
            false,
            true,
        )
        .await
        .expect("ingest with a real Y4M proxy target must succeed");

        let db = load_catalog(&catalog).expect("load catalog");
        assert_eq!(db.assets.len(), 1);
        let proxy_path = db.assets[0]
            .proxy_path
            .as_ref()
            .expect("Y4M input must produce a real proxy path");
        let proxy_path = std::path::PathBuf::from(proxy_path);
        assert!(
            proxy_path.exists(),
            "the proxy file itself must really exist on disk: {}",
            proxy_path.display()
        );
        assert!(
            std::fs::metadata(&proxy_path).expect("stat proxy").len() > 0,
            "the proxy file must not be empty"
        );
        assert_eq!(proxy_path.extension().and_then(|e| e.to_str()), Some("mkv"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_run_ingest_generates_real_wav_proxy() {
        let dir = std::env::temp_dir().join("oximedia_mam_ingest_proxy_wav");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let input = dir.join("clip.wav");
        std::fs::write(&input, make_test_wav(44_100, 4410)).expect("write wav fixture");
        let catalog = dir.join("catalog.json");
        let proxy_dir = dir.join("proxies");

        run_ingest(
            std::slice::from_ref(&input),
            &catalog,
            &None,
            &None,
            false,
            true,
            Some(&proxy_dir),
            false,
            true,
        )
        .await
        .expect("ingest with a real WAV proxy target must succeed");

        let db = load_catalog(&catalog).expect("load catalog");
        assert_eq!(db.assets.len(), 1);
        let proxy_path = db.assets[0]
            .proxy_path
            .as_ref()
            .expect("WAV input must produce a real proxy path");
        let proxy_path = std::path::PathBuf::from(proxy_path);
        assert!(
            proxy_path.exists(),
            "the proxy file itself must really exist on disk: {}",
            proxy_path.display()
        );
        assert!(
            std::fs::metadata(&proxy_path).expect("stat proxy").len() > 0,
            "the proxy file must not be empty"
        );
        assert_eq!(
            proxy_path.extension().and_then(|e| e.to_str()),
            Some("flac")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_run_ingest_unsupported_format_warns_and_still_catalogs() {
        let dir = std::env::temp_dir().join("oximedia_mam_ingest_proxy_unsupported");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let input = dir.join("clip.bin");
        std::fs::write(&input, b"not a real media file at all").expect("write fixture");
        let catalog = dir.join("catalog.json");
        let proxy_dir = dir.join("proxies");

        run_ingest(
            std::slice::from_ref(&input),
            &catalog,
            &None,
            &None,
            false,
            true,
            Some(&proxy_dir),
            false,
            true,
        )
        .await
        .expect("an unsupported proxy format must not fail the whole ingest");

        let db = load_catalog(&catalog).expect("load catalog");
        assert_eq!(
            db.assets.len(),
            1,
            "the asset must still be cataloged even without a proxy"
        );
        assert!(
            db.assets[0].proxy_path.is_none(),
            "no proxy path may be fabricated for an unsupported format"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_run_ingest_default_proxy_dir_is_next_to_catalog() {
        let dir = std::env::temp_dir().join("oximedia_mam_ingest_proxy_default_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let input = dir.join("clip.y4m");
        std::fs::write(&input, make_test_y4m(8, 8)).expect("write y4m fixture");
        let catalog = dir.join("catalog.json");

        run_ingest(
            std::slice::from_ref(&input),
            &catalog,
            &None,
            &None,
            false,
            true,
            None, // no --proxy-dir: must default to <catalog_parent>/proxies
            false,
            true,
        )
        .await
        .expect("ingest must succeed");

        let expected_dir = dir.join("proxies");
        assert!(
            expected_dir.is_dir(),
            "default proxy dir must be created next to the catalog: {}",
            expected_dir.display()
        );

        let db = load_catalog(&catalog).expect("load catalog");
        let proxy_path = db.assets[0]
            .proxy_path
            .as_ref()
            .expect("must have generated a proxy");
        assert!(
            std::path::PathBuf::from(proxy_path).starts_with(&expected_dir),
            "proxy must land under the default proxies/ dir, got: {proxy_path}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
