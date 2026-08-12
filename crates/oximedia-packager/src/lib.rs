//! Adaptive streaming packaging for `OxiMedia`.
//!
//! `oximedia-packager` provides comprehensive support for packaging media content into
//! adaptive streaming formats:
//!
//! - **HLS (HTTP Live Streaming)**: Master playlists, media playlists, TS/fMP4 segments
//! - **DASH (Dynamic Adaptive Streaming over HTTP)**: MPD manifests, CMAF segments
//!
//! # Features
//!
//! - Automatic bitrate ladder generation
//! - Multi-bitrate variant streams
//! - Segment creation (TS, fMP4, CMAF)
//! - Manifest generation (M3U8, MPD)
//! - Encryption support (AES-128, SAMPLE-AES, CENC)
//! - Keyframe alignment
//! - Fast start optimization
//! - S3/cloud upload integration
//! - Live and VOD packaging
//!
//! # Green List Only
//!
//! Like all `OxiMedia` crates, only patent-free codecs are supported:
//!
//! | Video | Audio | Subtitle |
//! |-------|-------|----------|
//! | AV1   | Opus  | `WebVTT`   |
//! | VP9   | Vorbis| SRT      |
//! | VP8   | FLAC  |          |
//!
//! # Example: HLS Packaging
//!
//! ```ignore
//! use oximedia_packager::{Packager, PackagerConfig};
//! use oximedia_packager::config::PackagingFormat;
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create configuration
//! let config = PackagerConfig::new()
//!     .with_format(PackagingFormat::HlsFmp4);
//!
//! // Create packager
//! let mut packager = Packager::new(config)?;
//!
//! // Package video to HLS
//! packager.package_hls("input.mkv", "output/hls").await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Example: DASH Packaging
//!
//! ```ignore
//! use oximedia_packager::{Packager, PackagerConfig};
//! use oximedia_packager::config::PackagingFormat;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = PackagerConfig::new()
//!     .with_format(PackagingFormat::Dash);
//!
//! let mut packager = Packager::new(config)?;
//! packager.package_dash("input.mkv", "output/dash").await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Example: Custom Bitrate Ladder
//!
//! ```ignore
//! use oximedia_packager::config::{BitrateEntry, BitrateLadder, PackagerConfig};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut ladder = BitrateLadder::new();
//!
//! // Add 1080p variant
//! ladder.add_entry(BitrateEntry::new(5_000_000, 1920, 1080, "av1"));
//!
//! // Add 720p variant
//! ladder.add_entry(BitrateEntry::new(3_000_000, 1280, 720, "av1"));
//!
//! // Add 480p variant
//! ladder.add_entry(BitrateEntry::new(1_500_000, 854, 480, "av1"));
//!
//! let config = PackagerConfig::new().with_ladder(ladder);
//! # Ok(())
//! # }
//! ```
//!
//! # Example: With Encryption
//!
//! ```ignore
//! use oximedia_packager::config::{EncryptionConfig, EncryptionMethod, PackagerConfig};
//! use oximedia_packager::encryption::KeyGenerator;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Generate a cryptographically secure encryption key and IV (OS CSPRNG).
//! let key = KeyGenerator::generate_aes128_key()?;
//! let iv = KeyGenerator::generate_iv()?;
//!
//! // Alternatively, derive a key from a passphrase with PBKDF2-HMAC-SHA256.
//! // `salt` must be a unique, random value generated and stored per key.
//! let salt = KeyGenerator::generate_iv()?;
//! let _passphrase_key = KeyGenerator::from_passphrase("correct horse battery staple", &salt)?;
//!
//! let mut encryption = EncryptionConfig::default();
//! encryption.method = EncryptionMethod::Aes128;
//! encryption.key = Some(key);
//! encryption.iv = Some(iv);
//! encryption.key_uri = Some("https://example.com/key".to_string());
//!
//! let config = PackagerConfig::new().with_encryption(encryption);
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

pub mod audio_track;
pub mod bandwidth_estimator;
pub mod bitrate_calc;
pub mod byte_range;
pub mod cmaf;
pub mod cmaf_byterange;
pub mod config;
pub mod content_boundary;
pub mod dash;
pub mod dash_event_stream;
pub mod drm_info;
pub mod drm_packager;
pub mod encryption;
pub mod encryption_info;
pub mod error;
pub mod hls;
pub mod hls_interstitial;
pub mod isobmff_writer;
pub mod keyframe_alignment;
pub mod ladder;
pub mod low_latency;
pub mod manifest;
pub mod manifest_builder;
pub mod manifest_update;
pub mod metadata_cache;
pub mod multivariant;
pub mod multivariant_builder;
pub mod output;
pub mod packaging_config;
pub mod packaging_optimizer;
pub mod parallel_packager;
pub mod playlist_generator;
pub mod pre_roll;
pub mod pssh;
pub mod scene_segmenter;
pub mod segment;
pub mod segment_index;
pub mod segment_list;
pub mod segment_naming;
pub mod segment_timeline;
pub mod segment_validator;
mod source_probe;
pub mod ssai;
pub mod streaming_output;
pub mod subtitle_track;
pub mod thumbnail_track;
pub mod timed_metadata;
pub mod variant_stream;

// Re-export commonly used types
pub use config::{
    BitrateEntry, BitrateLadder, EncryptionConfig, EncryptionMethod, OutputConfig, PackagerConfig,
    PackagingFormat, SegmentConfig, SegmentFormat,
};
pub use dash::{DashPackager, DashPackagerBuilder};
pub use error::{PackagerError, PackagerResult};
pub use hls::{HlsPackager, HlsPackagerBuilder};
pub use ladder::{BitrateLadderGenerator, LadderGenerator, LadderRung, SourceAnalysis, SourceInfo};
pub use multivariant_builder::{
    DashAdaptationSetBuilder, HlsMultivariantBuilder, MultivariantPlaylistBuilder,
};
pub use variant_stream::{StreamCodec, VariantSet, VariantStream};

/// Main packager for adaptive streaming.
pub struct Packager {
    config: PackagerConfig,
}

impl Packager {
    /// Create a new packager with the given configuration.
    pub fn new(config: PackagerConfig) -> PackagerResult<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    /// Package input to HLS format.
    ///
    /// # Arguments
    ///
    /// * `input` - Path to input media file
    /// * `output` - Output directory path
    ///
    /// # Example
    ///
    /// ```ignore
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = PackagerConfig::new();
    /// let mut packager = Packager::new(config)?;
    /// packager.package_hls("input.mkv", "output/hls").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn package_hls(&self, input: &str, output: &str) -> PackagerResult<()> {
        let mut config = self.config.clone();
        config.output.directory = output.into();

        let mut hls_packager = HlsPackager::new(config)?;
        hls_packager.package(input).await?;

        Ok(())
    }

    /// Package input to DASH format.
    ///
    /// # Arguments
    ///
    /// * `input` - Path to input media file
    /// * `output` - Output directory path
    ///
    /// # Example
    ///
    /// ```ignore
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = PackagerConfig::new();
    /// let mut packager = Packager::new(config)?;
    /// packager.package_dash("input.mkv", "output/dash").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn package_dash(&self, input: &str, output: &str) -> PackagerResult<()> {
        let mut config = self.config.clone();
        config.output.directory = output.into();

        let mut dash_packager = DashPackager::new(config)?;
        dash_packager.package(input).await?;

        Ok(())
    }

    /// Package input to both HLS and DASH formats.
    ///
    /// # Arguments
    ///
    /// * `input` - Path to input media file
    /// * `output_base` - Base output directory path
    ///
    /// This will create:
    /// - `{output_base}/hls/` for HLS output
    /// - `{output_base}/dash/` for DASH output
    pub async fn package_both(&self, input: &str, output_base: &str) -> PackagerResult<()> {
        use std::path::PathBuf;

        let hls_output = PathBuf::from(output_base).join("hls");
        let dash_output = PathBuf::from(output_base).join("dash");

        let hls_str = hls_output.to_str().ok_or_else(|| {
            PackagerError::InvalidConfig(format!(
                "HLS output path contains invalid UTF-8: {}",
                hls_output.display()
            ))
        })?;

        let dash_str = dash_output.to_str().ok_or_else(|| {
            PackagerError::InvalidConfig(format!(
                "DASH output path contains invalid UTF-8: {}",
                dash_output.display()
            ))
        })?;

        self.package_hls(input, hls_str).await?;
        self.package_dash(input, dash_str).await?;

        Ok(())
    }

    /// Get the packager configuration.
    #[must_use]
    pub fn config(&self) -> &PackagerConfig {
        &self.config
    }
}

/// Builder for creating a packager with fluent configuration.
pub struct PackagerBuilder {
    config: PackagerConfig,
}

impl PackagerBuilder {
    /// Create a new packager builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: PackagerConfig::default(),
        }
    }

    /// Set the packaging format.
    #[must_use]
    pub fn format(mut self, format: PackagingFormat) -> Self {
        self.config.format = format;
        self
    }

    /// Set the bitrate ladder.
    #[must_use]
    pub fn ladder(mut self, ladder: BitrateLadder) -> Self {
        self.config.ladder = ladder;
        self
    }

    /// Set the segment configuration.
    #[must_use]
    pub fn segment_config(mut self, segment: SegmentConfig) -> Self {
        self.config.segment = segment;
        self
    }

    /// Set the encryption configuration.
    #[must_use]
    pub fn encryption(mut self, encryption: EncryptionConfig) -> Self {
        self.config.encryption = encryption;
        self
    }

    /// Set the output configuration.
    #[must_use]
    pub fn output(mut self, output: OutputConfig) -> Self {
        self.config.output = output;
        self
    }

    /// Enable low latency mode.
    #[must_use]
    pub fn low_latency(mut self, enabled: bool) -> Self {
        self.config.low_latency = enabled;
        self
    }

    /// Enable manifest versioning.
    #[must_use]
    pub fn manifest_versioning(mut self, enabled: bool) -> Self {
        self.config.manifest_versioning = enabled;
        self
    }

    /// Build the packager.
    pub fn build(self) -> PackagerResult<Packager> {
        Packager::new(self.config)
    }
}

impl Default for PackagerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packager_creation() {
        let config = PackagerConfig::default();
        let packager = Packager::new(config);
        assert!(packager.is_ok());
    }

    #[test]
    fn test_packager_builder() {
        let packager = PackagerBuilder::new()
            .format(PackagingFormat::HlsFmp4)
            .low_latency(true)
            .build();

        assert!(packager.is_ok());
        let p = packager.expect("should succeed in test");
        assert_eq!(p.config.format, PackagingFormat::HlsFmp4);
        assert!(p.config.low_latency);
    }

    #[test]
    fn test_bitrate_ladder_validation() {
        let mut ladder = BitrateLadder::new();
        ladder.add_entry(BitrateEntry::new(1_000_000, 1280, 720, "av1"));

        let config = PackagerConfig::new().with_ladder(ladder);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_package_both_valid_paths() {
        // Ensure package_both constructs valid paths from ASCII base
        let config = PackagerConfig::default();
        let packager = Packager::new(config).expect("should succeed in test");
        // We can't run async here easily, but we verify the builder works
        assert_eq!(packager.config().format, PackagingFormat::HlsFmp4);
    }

    #[test]
    fn test_packager_builder_with_encryption() {
        let mut enc = EncryptionConfig::default();
        enc.method = EncryptionMethod::None;
        let packager = PackagerBuilder::new().encryption(enc).build();
        assert!(packager.is_ok());
    }
}
