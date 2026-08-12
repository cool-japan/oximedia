//! Stream information types.

use bytes::Bytes;
use oximedia_core::{CodecId, MediaType, Rational};

use crate::demux::matroska::matroska_v4::BlockAdditionMapping;
use crate::track_header::TransformMatrix;

/// Information about a stream in a container.
///
/// Each stream in a container has associated metadata including codec,
/// timebase, and format-specific parameters.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    /// Stream index (0-based).
    pub index: usize,

    /// Codec used for this stream.
    pub codec: CodecId,

    /// Type of media in this stream.
    pub media_type: MediaType,

    /// Timebase for interpreting timestamps.
    ///
    /// Timestamps are expressed as multiples of this rational number.
    /// For example, a timebase of 1/1000 means timestamps are in milliseconds.
    pub timebase: Rational,

    /// Duration of the stream in timebase units, if known.
    pub duration: Option<i64>,

    /// Codec-specific parameters.
    pub codec_params: CodecParams,

    /// Stream metadata (title, language, etc.).
    pub metadata: Metadata,

    /// Display rotation in degrees, snapped to a quarter turn
    /// (`0`, `90`, `180` or `270`).
    ///
    /// Derived from the container's display transform matrix (MP4 `tkhd`).
    /// `None` when the container carries no orientation metadata for this
    /// stream. Phone and camera captures routinely record portrait footage as
    /// landscape pixels plus a 90/270 degree rotation here, so any reframing or
    /// scaling stage must consult this before trusting `width`/`height`.
    pub rotation: Option<u16>,

    /// The full display transform matrix backing [`Self::rotation`].
    ///
    /// Row-major `[a, b, u, c, d, v, tx, ty, w]` in floating point (the raw
    /// 16.16 / 2.30 fixed-point values already decoded). `None` when the
    /// container carries no matrix.
    pub display_matrix: Option<TransformMatrix>,
}

impl StreamInfo {
    /// Creates a new `StreamInfo` with the given parameters.
    #[must_use]
    pub fn new(index: usize, codec: CodecId, timebase: Rational) -> Self {
        Self {
            index,
            codec,
            media_type: codec.media_type(),
            timebase,
            duration: None,
            codec_params: CodecParams::default(),
            metadata: Metadata::default(),
            rotation: None,
            display_matrix: None,
        }
    }

    /// Sets the display rotation, in degrees.
    #[must_use]
    pub const fn with_rotation(mut self, degrees: u16) -> Self {
        self.rotation = Some(degrees);
        self
    }

    /// Returns `true` when the stream is rotated by a quarter turn, i.e. its
    /// coded `width`/`height` are swapped relative to how it must be displayed.
    #[must_use]
    pub fn is_quarter_turned(&self) -> bool {
        matches!(self.rotation, Some(90 | 270))
    }

    /// Returns the display dimensions with [`Self::rotation`] applied.
    ///
    /// Returns `None` unless both coded dimensions are known.
    #[must_use]
    pub fn display_dimensions(&self) -> Option<(u32, u32)> {
        let width = self.codec_params.width?;
        let height = self.codec_params.height?;
        if self.is_quarter_turned() {
            Some((height, width))
        } else {
            Some((width, height))
        }
    }

    /// Returns the duration in seconds, if known.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn duration_seconds(&self) -> Option<f64> {
        self.duration.map(|d| {
            if self.timebase.den == 0 {
                0.0
            } else {
                (d as f64 * self.timebase.num as f64) / self.timebase.den as f64
            }
        })
    }

    /// Returns true if this is a video stream.
    #[must_use]
    pub const fn is_video(&self) -> bool {
        matches!(self.media_type, MediaType::Video)
    }

    /// Returns true if this is an audio stream.
    #[must_use]
    pub const fn is_audio(&self) -> bool {
        matches!(self.media_type, MediaType::Audio)
    }

    /// Returns true if this is a subtitle stream.
    #[must_use]
    pub const fn is_subtitle(&self) -> bool {
        matches!(self.media_type, MediaType::Subtitle)
    }
}

/// Codec-specific parameters.
///
/// Contains format-specific information needed for decoding,
/// such as video dimensions or audio sample rate.
#[derive(Clone, Debug, Default)]
pub struct CodecParams {
    /// Video width in pixels.
    pub width: Option<u32>,

    /// Video height in pixels.
    pub height: Option<u32>,

    /// Audio sample rate in Hz.
    pub sample_rate: Option<u32>,

    /// Number of audio channels.
    pub channels: Option<u8>,

    /// Codec-specific extra data (e.g., SPS/PPS for video, codec headers).
    pub extradata: Option<Bytes>,

    /// Matroska BlockAdditionMapping metadata for this stream, if any.
    pub block_addition_mappings: Vec<BlockAdditionMapping>,
}

impl CodecParams {
    /// Creates video codec parameters.
    #[must_use]
    pub const fn video(width: u32, height: u32) -> Self {
        Self {
            width: Some(width),
            height: Some(height),
            sample_rate: None,
            channels: None,
            extradata: None,
            block_addition_mappings: Vec::new(),
        }
    }

    /// Creates audio codec parameters.
    #[must_use]
    pub const fn audio(sample_rate: u32, channels: u8) -> Self {
        Self {
            width: None,
            height: None,
            sample_rate: Some(sample_rate),
            channels: Some(channels),
            extradata: None,
            block_addition_mappings: Vec::new(),
        }
    }

    /// Sets the extradata.
    #[must_use]
    pub fn with_extradata(mut self, extradata: Bytes) -> Self {
        self.extradata = Some(extradata);
        self
    }

    /// Returns true if video dimensions are set.
    #[must_use]
    pub const fn has_video_params(&self) -> bool {
        self.width.is_some() && self.height.is_some()
    }

    /// Returns true if audio parameters are set.
    #[must_use]
    pub const fn has_audio_params(&self) -> bool {
        self.sample_rate.is_some() && self.channels.is_some()
    }
}

/// Stream and container metadata.
///
/// Contains textual metadata such as title, artist, and custom key-value pairs.
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    /// Stream or container title.
    pub title: Option<String>,

    /// Artist or author.
    pub artist: Option<String>,

    /// Album name (for audio).
    pub album: Option<String>,

    /// Additional metadata entries.
    pub entries: Vec<(String, String)>,
}

impl Metadata {
    /// Creates empty metadata.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            title: None,
            artist: None,
            album: None,
            entries: Vec::new(),
        }
    }

    /// Sets the title.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the artist.
    #[must_use]
    pub fn with_artist(mut self, artist: impl Into<String>) -> Self {
        self.artist = Some(artist.into());
        self
    }

    /// Sets the album.
    #[must_use]
    pub fn with_album(mut self, album: impl Into<String>) -> Self {
        self.album = Some(album.into());
        self
    }

    /// Adds a custom metadata entry.
    #[must_use]
    pub fn with_entry(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push((key.into(), value.into()));
        self
    }

    /// Gets a metadata value by key.
    ///
    /// First checks the common fields (title, artist, album),
    /// then searches the custom entries.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        let key_lower = key.to_lowercase();
        match key_lower.as_str() {
            "title" => self.title.as_deref(),
            "artist" | "author" => self.artist.as_deref(),
            "album" => self.album.as_deref(),
            _ => self
                .entries
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.as_str()),
        }
    }

    /// Returns true if the metadata is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.artist.is_none()
            && self.album.is_none()
            && self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_info() {
        let stream = StreamInfo::new(0, CodecId::Av1, Rational::new(1, 1000));
        assert_eq!(stream.index, 0);
        assert_eq!(stream.codec, CodecId::Av1);
        assert!(stream.is_video());
        assert!(!stream.is_audio());
    }

    #[test]
    fn test_stream_duration() {
        let mut stream = StreamInfo::new(0, CodecId::Opus, Rational::new(1, 48000));
        stream.duration = Some(480_000); // 10 seconds at 48kHz

        let duration = stream.duration_seconds().expect("operation should succeed");
        assert!((duration - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_codec_params_video() {
        let params = CodecParams::video(1920, 1080);
        assert!(params.has_video_params());
        assert!(!params.has_audio_params());
        assert_eq!(params.width, Some(1920));
        assert_eq!(params.height, Some(1080));
    }

    #[test]
    fn test_codec_params_audio() {
        let params = CodecParams::audio(48000, 2);
        assert!(!params.has_video_params());
        assert!(params.has_audio_params());
        assert_eq!(params.sample_rate, Some(48000));
        assert_eq!(params.channels, Some(2));
    }

    #[test]
    fn test_metadata() {
        let metadata = Metadata::new()
            .with_title("Test Title")
            .with_artist("Test Artist")
            .with_entry("language", "en");

        assert_eq!(metadata.get("title"), Some("Test Title"));
        assert_eq!(metadata.get("artist"), Some("Test Artist"));
        assert_eq!(metadata.get("language"), Some("en"));
        assert_eq!(metadata.get("nonexistent"), None);
    }

    #[test]
    fn test_metadata_is_empty() {
        assert!(Metadata::new().is_empty());
        assert!(!Metadata::new().with_title("Test").is_empty());
    }
}
