//! HLS packaging implementation.

use crate::config::{BitrateEntry, PackagerConfig, SegmentFormat};
use crate::encryption::EncryptionHandler;
use crate::error::{PackagerError, PackagerResult};
use crate::hls::variant::{VariantConfig, VariantSet};
use crate::ladder::{LadderGenerator, SourceInfo};
use crate::output::OutputManager;
use crate::segment::{SegmentGenerator, SegmentWriter};
use camino::Utf8Path;
use std::time::Duration;
use tracing::{info, warn};

/// HLS packager.
pub struct HlsPackager {
    config: PackagerConfig,
    output_manager: OutputManager,
    variant_set: VariantSet,
    encryption_handler: Option<EncryptionHandler>,
}

impl HlsPackager {
    /// Create a new HLS packager.
    pub fn new(config: PackagerConfig) -> PackagerResult<Self> {
        config.validate()?;

        let output_manager = OutputManager::new(config.output.clone())?;
        let variant_set = VariantSet::new(config.segment.duration);

        let encryption_handler = if config.encryption.is_enabled() {
            Some(EncryptionHandler::new(config.encryption.method))
        } else {
            None
        };

        Ok(Self {
            config,
            output_manager,
            variant_set,
            encryption_handler,
        })
    }

    /// Package input to HLS output.
    pub async fn package(&mut self, input: &str) -> PackagerResult<()> {
        info!("Starting HLS packaging for: {}", input);

        // Initialize output structure
        self.output_manager.initialize().await?;

        // Generate or validate bitrate ladder
        let ladder = if self.config.ladder.auto_generate {
            self.generate_ladder_from_source(input).await?
        } else {
            self.config.ladder.clone()
        };

        info!(
            "Using bitrate ladder with {} variants",
            ladder.entries.len()
        );

        // Create variants
        for (idx, entry) in ladder.entries.iter().enumerate() {
            let variant_name = format!("variant_{idx}");
            let variant_config = VariantConfig::new(entry.clone(), variant_name.clone());

            // Create variant directory
            let _variant_dir = self
                .output_manager
                .structure_mut()
                .add_variant(&variant_name, entry.bitrate);

            self.variant_set.add_variant(variant_config);

            info!(
                "Created variant: {} ({}x{} @ {} bps)",
                variant_name, entry.width, entry.height, entry.bitrate
            );
        }

        // Process input and generate segments
        self.process_input(input).await?;

        // Generate playlists
        self.generate_playlists().await?;

        info!("HLS packaging completed");

        Ok(())
    }

    /// Generate a bitrate ladder from real source-media parameters.
    ///
    /// Resolution order:
    /// 1. [`crate::config::PackagerConfig::source_media`], if the caller
    ///    explicitly supplied it via `with_source_info(..)` — an explicit
    ///    override always wins over auto-detection.
    /// 2. Otherwise, a real probe of `input` (see
    ///    [`crate::source_probe`]): when `input` names a readable media file,
    ///    this reads its header and runs `oximedia-container`'s real
    ///    `MultiFormatProber` — the same prober behind `oximedia-cli`'s probe
    ///    tooling — to recover the actual codec and dimensions, never a
    ///    fabricated resolution.
    /// 3. If neither yields usable source info (no config override, and
    ///    `input` is unreadable, unrecognized, or has no usable video
    ///    stream), this returns an error rather than guessing — the caller
    ///    must supply source info via `with_source_info(..)` or an explicit
    ///    ladder via `with_ladder(..)` with `auto_generate = false`.
    ///
    /// Rungs are derived from the actual source resolution, framerate and
    /// codec: [`LadderGenerator`] caps rungs at or below the source
    /// resolution and labels each rung with the source codec.
    async fn generate_ladder_from_source(
        &self,
        input: &str,
    ) -> PackagerResult<crate::config::BitrateLadder> {
        let source = match self.config.source_media.clone() {
            Some(source) => source,
            None => match crate::source_probe::probe_source_info(input).await {
                Some(probed) => probed,
                None => {
                    return Err(PackagerError::invalid_config(format!(
                        "automatic bitrate ladder requested (ladder.auto_generate = true) but no \
                         source media info is available for '{input}' (it could not be probed as \
                         a readable, video-bearing media file either); call \
                         with_source_info(SourceInfo::new(..)) with the real \
                         resolution/framerate/codec, or supply an explicit ladder via \
                         with_ladder(..) and set auto_generate = false"
                    )))
                }
            },
        };

        let codec = source.codec.clone();
        let generator = LadderGenerator::new(source).with_codec(&codec);
        generator.generate()
    }

    /// Process input and generate segments.
    async fn process_input(&mut self, input: &str) -> PackagerResult<()> {
        info!("Processing input: {}", input);

        // Collect (variant name, codec) pairs first to avoid borrow conflicts.
        // The codec drives `encrypt_for_codec`'s NAL-aware vs whole-buffer
        // routing below.
        let variant_names: Vec<(String, String)> = self
            .variant_set
            .variants()
            .iter()
            .map(|v| (v.config().name.clone(), v.config().bitrate.codec.clone()))
            .collect();

        for (variant_name, variant_codec) in variant_names {
            let variant_dir = self
                .output_manager
                .structure()
                .get_variant(&variant_name)
                .ok_or_else(|| PackagerError::invalid_config("Variant directory not found"))?;

            let mut segment_config = self.config.segment.clone();
            segment_config.format = match self.config.format {
                crate::config::PackagingFormat::HlsTs => SegmentFormat::MpegTs,
                _ => SegmentFormat::Fmp4,
            };

            let mut segment_generator = SegmentGenerator::new(segment_config);
            let segment_writer = SegmentWriter::new(variant_dir.clone().into());

            // Create 10 sample segments
            for i in 0..10 {
                let timestamp = Duration::from_secs(i * 6);
                let frame_data = vec![0u8; 100_000];

                if let Some(segment_info) =
                    segment_generator.add_frame(&frame_data, true, timestamp)?
                {
                    let segment_data = if let Some(handler) = &self.encryption_handler {
                        handler.encrypt_for_codec(&frame_data, &variant_codec)?
                    } else {
                        frame_data
                    };

                    segment_writer
                        .write_segment(&segment_info, &segment_data)
                        .await?;

                    // Find and update variant
                    if let Some(variant) = self
                        .variant_set
                        .variants_mut()
                        .iter_mut()
                        .find(|v| v.config().name == variant_name)
                    {
                        variant.add_segment(segment_info);
                    }
                }
            }

            // Finalize last segment
            let final_timestamp = Duration::from_secs(60);
            let final_data = vec![0u8; 100_000];

            if let Some(segment_info) =
                segment_generator.add_frame(&final_data, true, final_timestamp)?
            {
                let segment_data = if let Some(handler) = &self.encryption_handler {
                    handler.encrypt_for_codec(&final_data, &variant_codec)?
                } else {
                    final_data
                };

                segment_writer
                    .write_segment(&segment_info, &segment_data)
                    .await?;

                // Find and update variant
                if let Some(variant) = self
                    .variant_set
                    .variants_mut()
                    .iter_mut()
                    .find(|v| v.config().name == variant_name)
                {
                    variant.add_segment(segment_info);
                }
            }

            // Log completion
            if let Some(variant) = self
                .variant_set
                .variants()
                .iter()
                .find(|v| v.config().name == variant_name)
            {
                info!(
                    "Created {} segments for variant {}",
                    variant.segment_count(),
                    variant_name
                );
            }
        }

        Ok(())
    }

    /// Generate all playlists.
    async fn generate_playlists(&self) -> PackagerResult<()> {
        info!("Generating HLS playlists");

        // Generate media playlists for each variant
        for variant in self.variant_set.variants() {
            let playlist = variant.generate_vod_playlist()?;
            let playlist_path = Utf8Path::new(&variant.config().playlist_filename);

            self.output_manager
                .write_file(playlist_path, playlist.as_bytes())
                .await?;

            info!(
                "Generated media playlist: {}",
                variant.config().playlist_filename
            );
        }

        // Generate master playlist
        let master_playlist = self
            .variant_set
            .generate_master_playlist(self.output_manager.base_url())?;

        let master_path = Utf8Path::new("master.m3u8");
        self.output_manager
            .write_file(master_path, master_playlist.as_bytes())
            .await?;

        info!("Generated master playlist: master.m3u8");

        Ok(())
    }

    /// Package for live streaming.
    pub async fn package_live(&mut self, input: &str) -> PackagerResult<()> {
        info!("Starting HLS live packaging for: {}", input);

        self.output_manager.initialize().await?;

        // Similar to package() but without EXT-X-ENDLIST
        // and with segment cleanup

        warn!("Live packaging not fully implemented yet");

        Ok(())
    }

    /// Update playlists for live streaming.
    pub async fn update_live_playlists(&mut self) -> PackagerResult<()> {
        for variant in self.variant_set.variants() {
            let playlist = variant.generate_playlist()?;
            let playlist_path = Utf8Path::new(&variant.config().playlist_filename);

            self.output_manager
                .write_file(playlist_path, playlist.as_bytes())
                .await?;
        }

        let master_playlist = self
            .variant_set
            .generate_master_playlist(self.output_manager.base_url())?;

        let master_path = Utf8Path::new("master.m3u8");
        self.output_manager
            .write_file(master_path, master_playlist.as_bytes())
            .await?;

        Ok(())
    }

    /// Get output directory.
    #[must_use]
    pub fn output_directory(&self) -> &Utf8Path {
        &self.output_manager.structure().root
    }

    /// Add custom variant.
    pub fn add_variant(&mut self, entry: BitrateEntry, name: String) {
        let config = VariantConfig::new(entry.clone(), name.clone());

        let _variant_dir = self
            .output_manager
            .structure_mut()
            .add_variant(&name, entry.bitrate);

        self.variant_set.add_variant(config);

        info!("Added custom variant: {}", name);
    }

    /// Get variant set.
    #[must_use]
    pub fn variant_set(&self) -> &VariantSet {
        &self.variant_set
    }

    /// Get mutable variant set.
    pub fn variant_set_mut(&mut self) -> &mut VariantSet {
        &mut self.variant_set
    }
}

/// HLS packager builder for easier configuration.
pub struct HlsPackagerBuilder {
    config: PackagerConfig,
}

impl HlsPackagerBuilder {
    /// Create a new HLS packager builder.
    #[must_use]
    pub fn new() -> Self {
        let mut config = PackagerConfig::default();
        config.format = crate::config::PackagingFormat::HlsFmp4;

        Self { config }
    }

    /// Use TS segments.
    #[must_use]
    pub fn with_ts_segments(mut self) -> Self {
        self.config.format = crate::config::PackagingFormat::HlsTs;
        self.config.segment.format = SegmentFormat::MpegTs;
        self
    }

    /// Use fMP4 segments.
    #[must_use]
    pub fn with_fmp4_segments(mut self) -> Self {
        self.config.format = crate::config::PackagingFormat::HlsFmp4;
        self.config.segment.format = SegmentFormat::Fmp4;
        self
    }

    /// Set segment duration.
    #[must_use]
    pub fn with_segment_duration(mut self, duration: Duration) -> Self {
        self.config.segment.duration = duration;
        self
    }

    /// Set output directory.
    #[must_use]
    pub fn with_output_directory(mut self, dir: std::path::PathBuf) -> Self {
        self.config.output.directory = dir;
        self
    }

    /// Set base URL.
    #[must_use]
    pub fn with_base_url(mut self, url: String) -> Self {
        self.config.output.base_url = Some(url);
        self
    }

    /// Enable encryption.
    #[must_use]
    pub fn with_encryption(mut self, method: crate::config::EncryptionMethod) -> Self {
        self.config.encryption.method = method;
        self
    }

    /// Set bitrate ladder.
    #[must_use]
    pub fn with_ladder(mut self, ladder: crate::config::BitrateLadder) -> Self {
        self.config.ladder = ladder;
        self
    }

    /// Set the real source-media parameters used to derive an automatic bitrate
    /// ladder (resolution, framerate, codec).
    #[must_use]
    pub fn with_source_info(mut self, source: SourceInfo) -> Self {
        self.config.source_media = Some(source);
        self
    }

    /// Build the packager.
    pub fn build(self) -> PackagerResult<HlsPackager> {
        HlsPackager::new(self.config)
    }
}

impl Default for HlsPackagerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hls_packager_builder() {
        let packager = HlsPackagerBuilder::new()
            .with_segment_duration(Duration::from_secs(4))
            .with_output_directory(std::env::temp_dir().join("oximedia-packager-hls-out"))
            .build();

        assert!(packager.is_ok());
    }

    #[test]
    fn test_ts_segment_format() {
        let packager = HlsPackagerBuilder::new()
            .with_ts_segments()
            .build()
            .expect("should succeed in test");

        assert_eq!(packager.config.segment.format, SegmentFormat::MpegTs);
    }

    #[tokio::test]
    async fn test_ladder_derived_from_source_caps_at_source_resolution() {
        // A 720p source must not yield rungs above 720p, and every rung must be
        // labelled with the real source codec (vp9 here), not a hardcoded av1.
        let mut config = PackagerConfig::default();
        config.output.directory = std::env::temp_dir().join("oximedia-pkg-hls-ladder-test");
        config.source_media = Some(SourceInfo::new(1280, 720, 30.0, "vp9".to_string()));
        let packager = HlsPackager::new(config).expect("packager should build");

        let ladder = packager
            .generate_ladder_from_source("dummy_input.mkv")
            .await
            .expect("ladder should derive from source info");

        assert!(!ladder.entries.is_empty(), "ladder must have rungs");
        for entry in &ladder.entries {
            assert!(
                entry.height <= 720,
                "rung height {} exceeds source height 720",
                entry.height
            );
            assert!(
                entry.width <= 1280,
                "rung width {} exceeds source width 1280",
                entry.width
            );
            assert_eq!(entry.codec, "vp9", "rung codec must match source codec");
        }
    }

    #[tokio::test]
    async fn test_ladder_without_source_info_is_honest_error() {
        // auto_generate defaults to true; with no source_media the packager must
        // refuse rather than fabricate a 1080p/av1 ladder unrelated to the input.
        let mut config = PackagerConfig::default();
        config.output.directory = std::env::temp_dir().join("oximedia-pkg-hls-noladder-test");
        let packager = HlsPackager::new(config).expect("packager should build");

        let result = packager.generate_ladder_from_source("dummy.mkv").await;
        assert!(
            result.is_err(),
            "must not fabricate a ladder without real source info"
        );
    }
}
