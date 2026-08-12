//! Metadata editing and management for media files.
//!
//! Provides functionality to read, edit, and manage metadata/tags
//! in media files including title, artist, album, cover art, etc.

use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Options for metadata operations.
#[derive(Debug, Clone)]
pub struct MetadataOptions {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub operation: MetadataOperation,
    pub json_output: bool,
}

/// Type of metadata operation to perform.
#[derive(Debug, Clone)]
pub enum MetadataOperation {
    /// Show metadata
    Show,
    /// Set metadata fields
    Set { fields: HashMap<String, String> },
    /// Remove specific metadata fields
    Remove { fields: Vec<String> },
    /// Clear all metadata
    Clear,
    /// Copy metadata from another file
    Copy { source: PathBuf },
}

/// Metadata container for a media file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaMetadata {
    /// Basic information
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,

    /// Extended information
    pub comment: Option<String>,
    pub composer: Option<String>,
    pub publisher: Option<String>,
    pub copyright: Option<String>,
    pub encoder: Option<String>,
    pub creation_time: Option<String>,

    /// Custom tags
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub custom: HashMap<String, String>,
}

impl Default for MediaMetadata {
    fn default() -> Self {
        Self {
            title: None,
            artist: None,
            album: None,
            album_artist: None,
            genre: None,
            year: None,
            track_number: None,
            disc_number: None,
            comment: None,
            composer: None,
            publisher: None,
            copyright: None,
            encoder: None,
            creation_time: None,
            custom: HashMap::new(),
        }
    }
}

impl MediaMetadata {
    /// Set a metadata field by name.
    pub fn set_field(&mut self, key: &str, value: String) -> Result<()> {
        match key.to_lowercase().as_str() {
            "title" => self.title = Some(value),
            "artist" => self.artist = Some(value),
            "album" => self.album = Some(value),
            "album_artist" | "albumartist" => self.album_artist = Some(value),
            "genre" => self.genre = Some(value),
            "year" => {
                let year: u32 = value.parse().context("Invalid year value")?;
                self.year = Some(year);
            }
            "track" | "track_number" => {
                let track: u32 = value.parse().context("Invalid track number")?;
                self.track_number = Some(track);
            }
            "disc" | "disc_number" => {
                let disc: u32 = value.parse().context("Invalid disc number")?;
                self.disc_number = Some(disc);
            }
            "comment" => self.comment = Some(value),
            "composer" => self.composer = Some(value),
            "publisher" => self.publisher = Some(value),
            "copyright" => self.copyright = Some(value),
            "encoder" => self.encoder = Some(value),
            "creation_time" => self.creation_time = Some(value),
            _ => {
                // Custom field
                self.custom.insert(key.to_string(), value);
            }
        }
        Ok(())
    }

    /// Remove a metadata field by name.
    pub fn remove_field(&mut self, key: &str) {
        match key.to_lowercase().as_str() {
            "title" => self.title = None,
            "artist" => self.artist = None,
            "album" => self.album = None,
            "album_artist" | "albumartist" => self.album_artist = None,
            "genre" => self.genre = None,
            "year" => self.year = None,
            "track" | "track_number" => self.track_number = None,
            "disc" | "disc_number" => self.disc_number = None,
            "comment" => self.comment = None,
            "composer" => self.composer = None,
            "publisher" => self.publisher = None,
            "copyright" => self.copyright = None,
            "encoder" => self.encoder = None,
            "creation_time" => self.creation_time = None,
            _ => {
                self.custom.remove(key);
            }
        }
    }

    /// Clear all metadata.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Check if metadata is empty.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.artist.is_none()
            && self.album.is_none()
            && self.album_artist.is_none()
            && self.genre.is_none()
            && self.year.is_none()
            && self.track_number.is_none()
            && self.disc_number.is_none()
            && self.comment.is_none()
            && self.composer.is_none()
            && self.publisher.is_none()
            && self.copyright.is_none()
            && self.encoder.is_none()
            && self.creation_time.is_none()
            && self.custom.is_empty()
    }
}

/// Main metadata management function.
pub async fn manage_metadata(options: MetadataOptions) -> Result<()> {
    info!("Starting metadata operation");
    debug!("Metadata options: {:?}", options);

    // Validate input
    validate_input(&options.input).await?;

    // Perform operation
    match &options.operation {
        MetadataOperation::Show => show_metadata(&options).await,
        MetadataOperation::Set { fields } => set_metadata(&options, fields).await,
        MetadataOperation::Remove { fields } => remove_metadata(&options, fields).await,
        MetadataOperation::Clear => clear_metadata(&options).await,
        MetadataOperation::Copy { source } => copy_metadata(&options, source).await,
    }
}

/// Validate input file exists and is readable.
async fn validate_input(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("Input file does not exist: {}", path.display()));
    }

    if !path.is_file() {
        return Err(anyhow!("Input path is not a file: {}", path.display()));
    }

    Ok(())
}

/// Show metadata from a file.
async fn show_metadata(options: &MetadataOptions) -> Result<()> {
    info!("Reading metadata from {}", options.input.display());

    let metadata = read_metadata(&options.input).await?;

    if options.json_output {
        println!("{}", serde_json::to_string_pretty(&metadata)?);
    } else {
        print_metadata_pretty(&metadata, &options.input);
    }

    Ok(())
}

/// Set metadata fields in a file.
async fn set_metadata(options: &MetadataOptions, fields: &HashMap<String, String>) -> Result<()> {
    info!("Setting metadata fields");

    let mut metadata = read_metadata(&options.input).await?;

    // Apply changes
    for (key, value) in fields {
        debug!("Setting {}: {}", key, value);
        metadata.set_field(key, value.clone())?;
    }

    // Write metadata
    let output = options.output.as_ref().unwrap_or(&options.input);
    write_metadata(output, &metadata).await?;

    if !options.json_output && !crate::progress::is_quiet() {
        println!("{}", "Metadata updated successfully".green().bold());
        println!("{:20} {}", "Output file:", output.display());
    }

    Ok(())
}

/// Remove metadata fields from a file.
async fn remove_metadata(options: &MetadataOptions, fields: &[String]) -> Result<()> {
    info!("Removing metadata fields");

    let mut metadata = read_metadata(&options.input).await?;

    // Remove fields
    for field in fields {
        debug!("Removing field: {}", field);
        metadata.remove_field(field);
    }

    // Write metadata
    let output = options.output.as_ref().unwrap_or(&options.input);
    write_metadata(output, &metadata).await?;

    if !options.json_output && !crate::progress::is_quiet() {
        println!("{}", "Metadata fields removed successfully".green().bold());
        println!("{:20} {}", "Output file:", output.display());
    }

    Ok(())
}

/// Clear all metadata from a file.
async fn clear_metadata(options: &MetadataOptions) -> Result<()> {
    info!("Clearing all metadata");

    let mut metadata = MediaMetadata::default();
    metadata.clear();

    let output = options.output.as_ref().unwrap_or(&options.input);
    write_metadata(output, &metadata).await?;

    if !options.json_output && !crate::progress::is_quiet() {
        println!("{}", "All metadata cleared".green().bold());
        println!("{:20} {}", "Output file:", output.display());
    }

    Ok(())
}

/// Copy metadata from one file to another.
async fn copy_metadata(options: &MetadataOptions, source: &Path) -> Result<()> {
    info!(
        "Copying metadata from {} to {}",
        source.display(),
        options.input.display()
    );

    validate_input(source).await?;

    let metadata = read_metadata(source).await?;
    let output = options.output.as_ref().unwrap_or(&options.input);
    write_metadata(output, &metadata).await?;

    if !options.json_output && !crate::progress::is_quiet() {
        println!("{}", "Metadata copied successfully".green().bold());
        println!("{:20} {}", "Source:", source.display());
        println!("{:20} {}", "Destination:", output.display());
    }

    Ok(())
}

/// Sidecar file extension used to persist metadata alongside media files.
const METADATA_SIDECAR_EXT: &str = "oxmeta";

/// Derive the path of the JSON sidecar file for a given media path.
fn sidecar_path(media_path: &Path) -> PathBuf {
    let mut p = media_path.to_path_buf();
    p.set_extension(METADATA_SIDECAR_EXT);
    p
}

/// Format a Unix timestamp (seconds since epoch) as an RFC 3339 string.
///
/// Returns `"ts:<secs>"` if `chrono` cannot represent the timestamp (overflow).
fn format_timestamp(secs: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| format!("ts:{secs}"))
}

/// Recognised media format types for metadata dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormatType {
    /// MP3 — carries ID3v2 tags at the start of the file.
    Mp3,
    /// Ogg/FLAC/Opus — carries Vorbis Comments.
    OggFamily,
    /// MP4/M4A/MOV — carries iTunes atoms.
    Mp4,
    /// MKV/WebM — carries Matroska tags.
    Matroska,
    /// Unknown format — fall back to sidecar.
    Unknown,
}

/// Detect the format type from a file extension.
fn detect_format_type(path: &Path) -> FormatType {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("mp3") => FormatType::Mp3,
        Some("flac") | Some("ogg") | Some("opus") | Some("oga") => FormatType::OggFamily,
        Some("mp4") | Some("m4a") | Some("m4v") | Some("mov") => FormatType::Mp4,
        Some("mkv") | Some("webm") | Some("mka") => FormatType::Matroska,
        _ => FormatType::Unknown,
    }
}

/// Try to read in-file metadata using the `oximedia-metadata` crate.
///
/// Returns `None` if the file format is not supported by this path (the caller
/// should then fall back to the sidecar).
fn try_read_infile_metadata(path: &Path, kind: FormatType) -> Option<MediaMetadata> {
    use oximedia_metadata::{Metadata, MetadataFormat};

    let format = match kind {
        FormatType::Mp3 => MetadataFormat::Id3v2,
        FormatType::OggFamily => MetadataFormat::VorbisComments,
        FormatType::Mp4 => MetadataFormat::iTunes,
        FormatType::Matroska => MetadataFormat::Matroska,
        FormatType::Unknown => return None,
    };

    // Read the file synchronously (metadata blobs are small — typically < 1 MB).
    let data = std::fs::read(path).ok()?;

    // For ID3v2 the tag must start with the magic bytes "ID3".
    if format == MetadataFormat::Id3v2 && !data.starts_with(b"ID3") {
        return None;
    }

    let parsed = Metadata::parse(&data, format).ok()?;
    let common = parsed.common();

    // Map CommonFields → MediaMetadata.
    let mut meta = MediaMetadata::default();
    meta.title = common.title;
    meta.artist = common.artist;
    meta.album = common.album;
    meta.album_artist = common.album_artist;
    meta.genre = common.genre;
    meta.year = common.year;
    meta.track_number = common.track_number;
    meta.disc_number = common.disc_number;
    meta.comment = common.comment;
    meta.composer = common.composer;
    meta.publisher = common.publisher;
    meta.copyright = common.copyright;
    meta.encoder = common.encoder;
    // Use the date field if available; otherwise leave creation_time empty.
    meta.creation_time = common.date;

    Some(meta)
}

/// Read metadata from a file.
///
/// Priority:
/// 1. JSON sidecar file (`.oxmeta`) — written by a previous `set` operation.
/// 2. In-file metadata tags (ID3v2, Vorbis Comments, iTunes atoms, Matroska tags).
/// 3. Filesystem fallback (title from filename, modification time in RFC 3339).
async fn read_metadata(path: &Path) -> Result<MediaMetadata> {
    debug!("Reading metadata from {}", path.display());

    // 1. Check for a sidecar JSON file written by a previous `set` operation.
    let sidecar = sidecar_path(path);
    if sidecar.exists() {
        debug!("Found metadata sidecar: {}", sidecar.display());
        let json = tokio::fs::read_to_string(&sidecar)
            .await
            .with_context(|| format!("Failed to read metadata sidecar {}", sidecar.display()))?;
        let metadata: MediaMetadata =
            serde_json::from_str(&json).context("Failed to parse metadata sidecar")?;
        return Ok(metadata);
    }

    // 2. Try in-file metadata.
    let kind = detect_format_type(path);
    if let Some(in_file) = try_read_infile_metadata(path, kind) {
        info!(
            "Read in-file {} metadata from {}",
            format!("{kind:?}"),
            path.display()
        );
        return Ok(in_file);
    }

    // 3. Filesystem fallback: synthesise basic metadata.
    let mut metadata = MediaMetadata::default();

    // Use the file stem as a default title.
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        metadata.title = Some(stem.to_string());
    }

    metadata.encoder = Some("OxiMedia".to_string());

    // Use the file's last-modified time formatted as RFC 3339.
    if let Ok(fs_meta) = tokio::fs::metadata(path).await {
        if let Ok(modified) = fs_meta.modified() {
            let secs = modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            metadata.creation_time = Some(format_timestamp(secs));
        }
    }

    info!(
        "Metadata synthesised from filesystem for {}",
        path.display()
    );
    Ok(metadata)
}

/// Write metadata to a file.
///
/// Serialises `metadata` to a JSON sidecar file (`.oxmeta`) placed next to the
/// target media file.  This is a non-destructive approach that avoids modifying
/// the original container while still persisting edits across CLI invocations.
async fn write_metadata(path: &Path, metadata: &MediaMetadata) -> Result<()> {
    debug!("Writing metadata to {}", path.display());

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("Failed to create output directory")?;
        }
    }

    // Serialise metadata to JSON sidecar
    let sidecar = sidecar_path(path);
    let json =
        serde_json::to_string_pretty(metadata).context("Failed to serialise metadata to JSON")?;
    tokio::fs::write(&sidecar, json)
        .await
        .with_context(|| format!("Failed to write metadata sidecar {}", sidecar.display()))?;

    debug!("Metadata written to sidecar: {}", sidecar.display());
    Ok(())
}

/// Print metadata in a human-readable format.
fn print_metadata_pretty(metadata: &MediaMetadata, path: &Path) {
    println!("{}", "Media Metadata".cyan().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "File:", path.display());
    println!();

    if metadata.is_empty() {
        println!("{}", "No metadata found".yellow());
    } else {
        println!("{}", "Basic Information:".green().bold());
        print_field("Title", &metadata.title);
        print_field("Artist", &metadata.artist);
        print_field("Album", &metadata.album);
        print_field("Album Artist", &metadata.album_artist);
        print_field("Genre", &metadata.genre);

        if let Some(year) = metadata.year {
            println!("{:20} {}", "Year:", year);
        }
        if let Some(track) = metadata.track_number {
            println!("{:20} {}", "Track Number:", track);
        }
        if let Some(disc) = metadata.disc_number {
            println!("{:20} {}", "Disc Number:", disc);
        }

        let has_extended = metadata.comment.is_some()
            || metadata.composer.is_some()
            || metadata.publisher.is_some()
            || metadata.copyright.is_some()
            || metadata.encoder.is_some()
            || metadata.creation_time.is_some();

        if has_extended {
            println!();
            println!("{}", "Extended Information:".green().bold());
            print_field("Comment", &metadata.comment);
            print_field("Composer", &metadata.composer);
            print_field("Publisher", &metadata.publisher);
            print_field("Copyright", &metadata.copyright);
            print_field("Encoder", &metadata.encoder);
            print_field("Creation Time", &metadata.creation_time);
        }

        if !metadata.custom.is_empty() {
            println!();
            println!("{}", "Custom Tags:".green().bold());
            for (key, value) in &metadata.custom {
                println!("{:20} {}", format!("{}:", key), value);
            }
        }
    }

    println!("{}", "=".repeat(60));
}

/// Helper function to print optional metadata field.
fn print_field(label: &str, value: &Option<String>) {
    if let Some(v) = value {
        println!("{:20} {}", format!("{}:", label), v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_timestamp_epoch() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00+00:00");
    }

    #[test]
    fn format_timestamp_specific() {
        let result = format_timestamp(1_700_000_000);
        // 2023-11-14T22:13:20+00:00
        assert!(
            result.starts_with("2023-11-14"),
            "expected 2023-11-14..., got: {result}"
        );
    }

    #[test]
    fn detect_format_mp3() {
        assert_eq!(
            detect_format_type(std::path::Path::new("track.mp3")),
            FormatType::Mp3
        );
    }

    #[test]
    fn detect_format_ogg_family() {
        assert_eq!(
            detect_format_type(std::path::Path::new("track.flac")),
            FormatType::OggFamily
        );
        assert_eq!(
            detect_format_type(std::path::Path::new("track.opus")),
            FormatType::OggFamily
        );
    }

    #[test]
    fn detect_format_mp4() {
        assert_eq!(
            detect_format_type(std::path::Path::new("video.mp4")),
            FormatType::Mp4
        );
        assert_eq!(
            detect_format_type(std::path::Path::new("audio.m4a")),
            FormatType::Mp4
        );
    }

    #[test]
    fn detect_format_matroska() {
        assert_eq!(
            detect_format_type(std::path::Path::new("video.mkv")),
            FormatType::Matroska
        );
    }

    #[test]
    fn detect_format_unknown() {
        assert_eq!(
            detect_format_type(std::path::Path::new("file.xyz")),
            FormatType::Unknown
        );
    }

    #[test]
    fn test_set_field() {
        let mut metadata = MediaMetadata::default();

        metadata
            .set_field("title", "Test Title".to_string())
            .expect("set_field should succeed");
        assert_eq!(metadata.title, Some("Test Title".to_string()));

        metadata
            .set_field("year", "2024".to_string())
            .expect("set_field should succeed");
        assert_eq!(metadata.year, Some(2024));

        metadata
            .set_field("custom_tag", "Custom Value".to_string())
            .expect("set_field should succeed");
        assert_eq!(
            metadata.custom.get("custom_tag"),
            Some(&"Custom Value".to_string())
        );
    }

    #[test]
    fn test_remove_field() {
        let mut metadata = MediaMetadata::default();
        metadata.title = Some("Title".to_string());
        metadata.artist = Some("Artist".to_string());

        metadata.remove_field("title");
        assert_eq!(metadata.title, None);
        assert_eq!(metadata.artist, Some("Artist".to_string()));
    }

    #[test]
    fn test_clear() {
        let mut metadata = MediaMetadata::default();
        metadata.title = Some("Title".to_string());
        metadata.artist = Some("Artist".to_string());

        metadata.clear();
        assert!(metadata.is_empty());
    }

    #[test]
    fn test_is_empty() {
        let metadata = MediaMetadata::default();
        assert!(metadata.is_empty());

        let mut metadata_with_data = MediaMetadata::default();
        metadata_with_data.title = Some("Title".to_string());
        assert!(!metadata_with_data.is_empty());
    }
}
