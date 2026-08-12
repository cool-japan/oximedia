//! DASH packaging implementation.

use crate::config::{PackagerConfig, SegmentFormat};
use crate::dash::cmaf::{CmafHeader, CmafTrack, TrackType};
use crate::dash::mpd::{
    representation_from_bitrate_entry, AdaptationSet, DashProfile, MpdBuilder, MpdType, Period,
    SegmentTemplate,
};
use crate::encryption::EncryptionHandler;
use crate::error::{PackagerError, PackagerResult};
use crate::ladder::{LadderGenerator, SourceInfo};
use crate::output::OutputManager;
use crate::segment::{SegmentGenerator, SegmentWriter};
use camino::Utf8Path;
use std::time::Duration;
use tracing::info;

/// DASH packager.
pub struct DashPackager {
    config: PackagerConfig,
    output_manager: OutputManager,
    encryption_handler: Option<EncryptionHandler>,
    segment_infos: Vec<(String, Vec<crate::segment::SegmentInfo>)>, // (representation_id, segments)
}

impl DashPackager {
    /// Create a new DASH packager.
    pub fn new(config: PackagerConfig) -> PackagerResult<Self> {
        config.validate()?;

        let output_manager = OutputManager::new(config.output.clone())?;

        let encryption_handler = if config.encryption.is_enabled() {
            Some(EncryptionHandler::new(config.encryption.method))
        } else {
            None
        };

        Ok(Self {
            config,
            output_manager,
            encryption_handler,
            segment_infos: Vec::new(),
        })
    }

    /// Package input to DASH output.
    pub async fn package(&mut self, input: &str) -> PackagerResult<()> {
        info!("Starting DASH packaging for: {}", input);

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

        // Process each representation
        for (idx, entry) in ladder.entries.iter().enumerate() {
            let representation_id = format!("video_{idx}");

            info!(
                "Processing representation: {} ({}x{} @ {} bps)",
                representation_id, entry.width, entry.height, entry.bitrate
            );

            // Create representation directory
            let _repr_dir = self
                .output_manager
                .structure_mut()
                .add_variant(&representation_id, entry.bitrate);

            // Generate initialization segment
            self.generate_init_segment(&representation_id, entry)
                .await?;

            // Process and segment video
            let segments = self
                .process_representation(input, &representation_id, entry)
                .await?;

            self.segment_infos
                .push((representation_id.clone(), segments));
        }

        // Generate MPD manifest
        self.generate_mpd().await?;

        info!("DASH packaging completed");

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

    /// Generate initialization segment for a representation.
    async fn generate_init_segment(
        &self,
        representation_id: &str,
        entry: &crate::config::BitrateEntry,
    ) -> PackagerResult<()> {
        let track = CmafTrack::new(1, TrackType::Video, 90000, entry.codec.clone())
            .with_duration(Duration::from_secs(60));

        let header = CmafHeader::new(track);
        let init_data = header.generate_init_segment()?;

        // Write initialization segment
        let init_path = Utf8Path::new(representation_id).join("init.mp4");
        self.output_manager
            .write_file(&init_path, &init_data)
            .await?;

        info!(
            "Generated initialization segment for {}: {} bytes",
            representation_id,
            init_data.len()
        );

        Ok(())
    }

    /// Process a representation and generate segments.
    async fn process_representation(
        &self,
        _input: &str,
        representation_id: &str,
        entry: &crate::config::BitrateEntry,
    ) -> PackagerResult<Vec<crate::segment::SegmentInfo>> {
        let repr_dir = self
            .output_manager
            .structure()
            .get_variant(representation_id)
            .ok_or_else(|| PackagerError::invalid_config("Representation directory not found"))?;

        let mut segment_config = self.config.segment.clone();
        segment_config.format = SegmentFormat::Cmaf;

        let mut segment_generator = SegmentGenerator::new(segment_config);
        let segment_writer = SegmentWriter::new(repr_dir.clone().into());

        let mut segments = Vec::new();

        // Create 10 sample segments
        for i in 0..10 {
            let timestamp = Duration::from_secs(i * 6);

            // Create sample frame data
            let frame_data = vec![0u8; 100_000];

            // Add frame (keyframe every 6 seconds)
            if let Some(segment_info) = segment_generator.add_frame(&frame_data, true, timestamp)? {
                // Encrypt if needed, routed by the representation's real codec
                // (NAL-aware subsample mapping for NAL-structured codecs;
                // whole-buffer pattern encryption for this tree's actual
                // codecs — see `EncryptionHandler::encrypt_for_codec`).
                let segment_data = if let Some(handler) = &self.encryption_handler {
                    handler.encrypt_for_codec(&frame_data, &entry.codec)?
                } else {
                    frame_data
                };

                // Write segment
                segment_writer
                    .write_segment(&segment_info, &segment_data)
                    .await?;

                segments.push(segment_info);
            }
        }

        // Finalize last segment
        let final_timestamp = Duration::from_secs(60);
        let final_data = vec![0u8; 100_000];

        if let Some(segment_info) =
            segment_generator.add_frame(&final_data, true, final_timestamp)?
        {
            let segment_data = if let Some(handler) = &self.encryption_handler {
                handler.encrypt_for_codec(&final_data, &entry.codec)?
            } else {
                final_data
            };

            segment_writer
                .write_segment(&segment_info, &segment_data)
                .await?;
            segments.push(segment_info);
        }

        info!(
            "Created {} segments for representation {}",
            segments.len(),
            representation_id
        );

        Ok(segments)
    }

    /// Generate MPD manifest.
    async fn generate_mpd(&self) -> PackagerResult<()> {
        info!("Generating DASH MPD manifest");

        let mpd_type = MpdType::Static;
        let profile = DashProfile::OnDemand;

        let mut mpd_builder =
            MpdBuilder::new(mpd_type, profile).with_min_buffer_time(Duration::from_secs(2));

        if let Some(base_url) = self.output_manager.base_url() {
            mpd_builder = mpd_builder.with_base_url(base_url.to_string());
        }

        // Calculate total duration
        let total_duration = if let Some((_, segments)) = self.segment_infos.first() {
            segments.iter().map(|s| s.duration).sum()
        } else {
            Duration::ZERO
        };

        mpd_builder = mpd_builder.with_duration(total_duration);

        // Create period
        let mut period = Period::new("0".to_string()).with_duration(total_duration);

        // Create video adaptation set
        let mut video_set = AdaptationSet::new(0, "video".to_string(), "video/mp4".to_string());

        for (repr_id, _segments) in &self.segment_infos {
            // Find bitrate entry for this representation
            let idx: usize = repr_id
                .strip_prefix("video_")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            if let Some(entry) = self.config.ladder.entries.get(idx) {
                let mut repr = representation_from_bitrate_entry(entry, repr_id.clone())?;

                // Add segment template
                let segment_template = SegmentTemplate::new(
                    format!("{repr_id}/init.mp4"),
                    format!("{repr_id}/$Number$.m4s"),
                    self.config.segment.duration.as_secs() * 90000,
                    90000,
                );

                repr = repr.with_segment_template(segment_template);

                video_set.add_representation(repr);
            }
        }

        period.add_adaptation_set(video_set);
        mpd_builder.add_period(period);

        // Build MPD
        let mpd_xml = mpd_builder.build()?;

        // Write MPD
        let mpd_path = Utf8Path::new("manifest.mpd");
        self.output_manager
            .write_file(mpd_path, mpd_xml.as_bytes())
            .await?;

        info!("Generated MPD manifest: manifest.mpd");

        Ok(())
    }

    /// Package for live streaming.
    pub async fn package_live(&mut self, input: &str) -> PackagerResult<()> {
        info!("Starting DASH live packaging for: {}", input);

        self.output_manager.initialize().await?;

        tracing::warn!("Live packaging not fully implemented yet");

        Ok(())
    }

    /// Update MPD for live streaming.
    pub async fn update_live_mpd(&mut self) -> PackagerResult<()> {
        self.generate_mpd().await
    }

    /// Get output directory.
    #[must_use]
    pub fn output_directory(&self) -> &Utf8Path {
        &self.output_manager.structure().root
    }
}

/// DASH packager builder for easier configuration.
pub struct DashPackagerBuilder {
    config: PackagerConfig,
}

impl DashPackagerBuilder {
    /// Create a new DASH packager builder.
    #[must_use]
    pub fn new() -> Self {
        let mut config = PackagerConfig::default();
        config.format = crate::config::PackagingFormat::Dash;
        config.segment.format = SegmentFormat::Cmaf;

        Self { config }
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

    /// Enable low latency mode.
    #[must_use]
    pub fn with_low_latency(mut self, enabled: bool) -> Self {
        self.config.low_latency = enabled;
        self
    }

    /// Build the packager.
    pub fn build(self) -> PackagerResult<DashPackager> {
        DashPackager::new(self.config)
    }
}

impl Default for DashPackagerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dash_packager_builder() {
        let packager = DashPackagerBuilder::new()
            .with_segment_duration(Duration::from_secs(4))
            .with_output_directory(std::env::temp_dir().join("oximedia-packager-dash-out"))
            .build();

        assert!(packager.is_ok());
    }

    #[test]
    fn test_cmaf_segment_format() {
        let packager = DashPackagerBuilder::new()
            .build()
            .expect("should succeed in test");

        assert_eq!(packager.config.segment.format, SegmentFormat::Cmaf);
    }

    #[test]
    fn test_low_latency_mode() {
        let packager = DashPackagerBuilder::new()
            .with_low_latency(true)
            .build()
            .expect("should succeed in test");

        assert!(packager.config.low_latency);
    }

    #[tokio::test]
    async fn test_ladder_derived_from_source_caps_at_source_resolution() {
        // A 720p source must not yield rungs above 720p, and every rung must be
        // labelled with the real source codec (vp9 here), not a hardcoded av1.
        let mut config = PackagerConfig::default();
        config.output.directory = std::env::temp_dir().join("oximedia-pkg-dash-ladder-test");
        config.source_media = Some(SourceInfo::new(1280, 720, 30.0, "vp9".to_string()));
        let packager = DashPackager::new(config).expect("packager should build");

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
        config.output.directory = std::env::temp_dir().join("oximedia-pkg-dash-noladder-test");
        let packager = DashPackager::new(config).expect("packager should build");

        let result = packager.generate_ladder_from_source("dummy.mkv").await;
        assert!(
            result.is_err(),
            "must not fabricate a ladder without real source info"
        );
    }
}
