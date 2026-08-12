//! Core types for video-over-IP protocol.

use crate::error::{VideoIpError, VideoIpResult};
use serde::{Deserialize, Serialize};

/// Video codec types supported by the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VideoCodec {
    /// VP9 compressed video (patent-free).
    Vp9,
    /// AV1 compressed video (patent-free).
    Av1,
    /// VP8 compressed video (patent-free).
    Vp8,
    /// Uncompressed v210 (10-bit 4:2:2 YUV).
    V210,
    /// Uncompressed UYVY (8-bit 4:2:2 YUV).
    Uyvy,
    /// Uncompressed 8-bit 4:2:0 YUV.
    Yuv420p,
    /// Uncompressed 10-bit 4:2:0 YUV.
    Yuv420p10,
}

impl VideoCodec {
    /// Returns true if the codec is compressed.
    #[must_use]
    pub const fn is_compressed(self) -> bool {
        matches!(self, Self::Vp9 | Self::Av1 | Self::Vp8)
    }

    /// Returns the typical bytes per pixel for uncompressed formats.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> Option<f32> {
        match self {
            Self::Vp9 | Self::Av1 | Self::Vp8 => None,
            Self::V210 => Some(8.0 / 3.0), // 10-bit 4:2:2 packed
            Self::Uyvy => Some(2.0),       // 8-bit 4:2:2 packed
            Self::Yuv420p => Some(1.5),    // 8-bit 4:2:0 planar
            Self::Yuv420p10 => Some(3.0),  // 10-bit 4:2:0 planar
        }
    }
}

/// Audio codec types supported by the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioCodec {
    /// Opus compressed audio (patent-free).
    Opus,
    /// Uncompressed PCM (signed 16-bit).
    Pcm16,
    /// Uncompressed PCM (signed 24-bit).
    Pcm24,
    /// Uncompressed PCM (32-bit float).
    PcmF32,
}

impl AudioCodec {
    /// Returns true if the codec is compressed.
    #[must_use]
    pub const fn is_compressed(self) -> bool {
        matches!(self, Self::Opus)
    }

    /// Returns the bytes per sample for uncompressed formats.
    #[must_use]
    pub const fn bytes_per_sample(self) -> Option<usize> {
        match self {
            Self::Opus => None,
            Self::Pcm16 => Some(2),
            Self::Pcm24 => Some(3),
            Self::PcmF32 => Some(4),
        }
    }
}

/// Video resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Resolution {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Resolution {
    /// Creates a new resolution.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Standard definition (640x480).
    pub const SD: Self = Self::new(640, 480);

    /// High definition 720p (1280x720).
    pub const HD_720: Self = Self::new(1280, 720);

    /// High definition 1080p (1920x1080).
    pub const HD_1080: Self = Self::new(1920, 1080);

    /// Ultra high definition 4K (3840x2160).
    pub const UHD_4K: Self = Self::new(3840, 2160);

    /// Ultra high definition 8K (7680x4320).
    pub const UHD_8K: Self = Self::new(7680, 4320);

    /// Returns the total number of pixels.
    #[must_use]
    pub const fn pixel_count(self) -> u32 {
        self.width * self.height
    }
}

/// Frame rate representation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrameRate {
    /// Numerator.
    pub num: u32,
    /// Denominator.
    pub den: u32,
}

impl FrameRate {
    /// Creates a new frame rate.
    #[must_use]
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    /// Creates a frame rate from a floating-point value.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame rate is invalid (zero or negative).
    pub fn from_float(fps: f64) -> VideoIpResult<Self> {
        if fps <= 0.0 || !fps.is_finite() {
            return Err(VideoIpError::InvalidVideoConfig(format!(
                "invalid frame rate: {fps}"
            )));
        }

        // Handle common frame rates exactly
        let (num, den) = match (fps * 1000.0).round() as u32 {
            23976 => (24000, 1001), // 23.976
            24000 => (24, 1),
            25000 => (25, 1),
            29970 => (30000, 1001), // 29.97
            30000 => (30, 1),
            50000 => (50, 1),
            59940 => (60000, 1001), // 59.94
            60000 => (60, 1),
            _ => {
                // Convert to rational approximation
                let num = (fps * 1000.0).round() as u32;
                (num, 1000)
            }
        };

        Ok(Self::new(num, den))
    }

    /// Converts to floating-point frames per second.
    #[must_use]
    pub fn to_float(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// Common frame rates.
    pub const FPS_23_976: Self = Self::new(24000, 1001);
    /// 24 fps.
    pub const FPS_24: Self = Self::new(24, 1);
    /// 25 fps (PAL).
    pub const FPS_25: Self = Self::new(25, 1);
    /// 29.97 fps (NTSC).
    pub const FPS_29_97: Self = Self::new(30000, 1001);
    /// 30 fps.
    pub const FPS_30: Self = Self::new(30, 1);
    /// 50 fps.
    pub const FPS_50: Self = Self::new(50, 1);
    /// 59.94 fps.
    pub const FPS_59_94: Self = Self::new(60000, 1001);
    /// 60 fps.
    pub const FPS_60: Self = Self::new(60, 1);
}

/// Video format configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFormat {
    /// Video codec.
    pub codec: VideoCodec,
    /// Resolution.
    pub resolution: Resolution,
    /// Frame rate.
    pub frame_rate: FrameRate,
    /// Whether to include alpha channel.
    pub has_alpha: bool,
    /// Color space (BT.601, BT.709, BT.2020).
    pub color_space: ColorSpace,
}

impl VideoFormat {
    /// Creates a new video format.
    #[must_use]
    pub const fn new(codec: VideoCodec, resolution: Resolution, frame_rate: FrameRate) -> Self {
        Self {
            codec,
            resolution,
            frame_rate,
            has_alpha: false,
            color_space: ColorSpace::Bt709,
        }
    }

    /// Sets whether to include alpha channel.
    #[must_use]
    pub const fn with_alpha(mut self, has_alpha: bool) -> Self {
        self.has_alpha = has_alpha;
        self
    }

    /// Sets the color space.
    #[must_use]
    pub const fn with_color_space(mut self, color_space: ColorSpace) -> Self {
        self.color_space = color_space;
        self
    }

    /// Estimates the bitrate in bits per second for uncompressed video.
    #[must_use]
    pub fn uncompressed_bitrate(&self) -> Option<u64> {
        let bpp = self.codec.bytes_per_pixel()?;
        let pixels = self.resolution.pixel_count();
        let fps = self.frame_rate.to_float();
        let bytes_per_frame = f64::from(pixels) * f64::from(bpp);
        Some((bytes_per_frame * fps * 8.0) as u64)
    }
}

/// Color space definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColorSpace {
    /// BT.601 (SD).
    Bt601,
    /// BT.709 (HD).
    Bt709,
    /// BT.2020 (UHD/HDR).
    Bt2020,
}

/// Audio format configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFormat {
    /// Audio codec.
    pub codec: AudioCodec,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u8,
    /// Bits per sample (for uncompressed).
    pub bits_per_sample: Option<u8>,
}

impl AudioFormat {
    /// Creates a new audio format.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid.
    pub fn new(codec: AudioCodec, sample_rate: u32, channels: u8) -> VideoIpResult<Self> {
        if channels == 0 || channels > 16 {
            return Err(VideoIpError::InvalidAudioConfig(format!(
                "invalid channel count: {channels}"
            )));
        }

        if sample_rate == 0 {
            return Err(VideoIpError::InvalidAudioConfig(
                "sample rate must be non-zero".to_string(),
            ));
        }

        let bits_per_sample = match codec {
            AudioCodec::Pcm16 => Some(16),
            AudioCodec::Pcm24 => Some(24),
            AudioCodec::PcmF32 => Some(32),
            AudioCodec::Opus => None,
        };

        Ok(Self {
            codec,
            sample_rate,
            channels,
            bits_per_sample,
        })
    }

    /// Estimates the bitrate in bits per second for uncompressed audio.
    #[must_use]
    pub fn uncompressed_bitrate(&self) -> Option<u64> {
        let bps = self.bits_per_sample?;
        Some(u64::from(self.sample_rate) * u64::from(self.channels) * u64::from(bps))
    }
}

/// Video configuration for source.
#[derive(Debug, Clone)]
pub struct VideoConfig {
    /// Video format.
    pub format: VideoFormat,
    /// Target bitrate for compressed video (bits per second).
    pub target_bitrate: Option<u64>,
    /// Maximum bitrate for compressed video (bits per second).
    pub max_bitrate: Option<u64>,
}

impl VideoConfig {
    /// Creates a new video configuration.
    ///
    /// The codec defaults to [`VideoCodec::Vp9`], which **cannot be sent**:
    /// no working VP9 encoder exists behind this crate, so
    /// [`VideoIpSource::new`](crate::VideoIpSource::new) rejects it (see
    /// [`crate::codec`]). Call [`with_codec`](Self::with_codec) with an
    /// uncompressed codec — [`VideoCodec::Uyvy`], [`VideoCodec::V210`],
    /// [`VideoCodec::Yuv420p`] or [`VideoCodec::Yuv420p10`] — to build a
    /// transmittable configuration. The VP9 default is retained because it is
    /// still meaningful on the receiving side, where VP9 decode is real.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid.
    pub fn new(width: u32, height: u32, fps: f64) -> VideoIpResult<Self> {
        let resolution = Resolution::new(width, height);
        let frame_rate = FrameRate::from_float(fps)?;
        let format = VideoFormat::new(VideoCodec::Vp9, resolution, frame_rate);

        Ok(Self {
            format,
            target_bitrate: None,
            max_bitrate: None,
        })
    }

    /// Sets the video codec.
    #[must_use]
    pub const fn with_codec(mut self, codec: VideoCodec) -> Self {
        self.format.codec = codec;
        self
    }

    /// Sets the target bitrate for compressed video.
    #[must_use]
    pub const fn with_bitrate(mut self, bitrate: u64) -> Self {
        self.target_bitrate = Some(bitrate);
        self
    }
}

/// Audio configuration for source.
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Audio format.
    pub format: AudioFormat,
    /// Target bitrate for compressed audio (bits per second).
    pub target_bitrate: Option<u64>,
}

impl AudioConfig {
    /// Creates a new audio configuration.
    ///
    /// The codec defaults to [`AudioCodec::Opus`], which this crate can
    /// neither encode nor decode — the backing implementation produces and
    /// consumes output that is not the actual signal, so both directions fail
    /// honestly (see [`crate::codec`]). Call
    /// [`with_codec`](Self::with_codec) with [`AudioCodec::Pcm16`],
    /// [`AudioCodec::Pcm24`] or [`AudioCodec::PcmF32`] for a usable
    /// configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid.
    pub fn new(sample_rate: u32, channels: u8) -> VideoIpResult<Self> {
        let format = AudioFormat::new(AudioCodec::Opus, sample_rate, channels)?;

        Ok(Self {
            format,
            target_bitrate: None,
        })
    }

    /// Sets the audio codec.
    ///
    /// # Errors
    ///
    /// Returns an error if the codec configuration is invalid.
    pub fn with_codec(mut self, codec: AudioCodec) -> VideoIpResult<Self> {
        self.format = AudioFormat::new(codec, self.format.sample_rate, self.format.channels)?;
        Ok(self)
    }

    /// Sets the target bitrate for compressed audio.
    #[must_use]
    pub const fn with_bitrate(mut self, bitrate: u64) -> Self {
        self.target_bitrate = Some(bitrate);
        self
    }
}

/// Stream type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamType {
    /// Main program feed.
    Program,
    /// Preview feed.
    Preview,
    /// Alpha channel (for keying).
    Alpha,
    /// Custom stream.
    Custom(u8),
}

impl StreamType {
    /// Converts to a numeric identifier.
    #[must_use]
    pub const fn to_id(self) -> u8 {
        match self {
            Self::Program => 0,
            Self::Preview => 1,
            Self::Alpha => 2,
            Self::Custom(id) => id,
        }
    }

    /// Converts from a numeric identifier.
    #[must_use]
    pub const fn from_id(id: u8) -> Self {
        match id {
            0 => Self::Program,
            1 => Self::Preview,
            2 => Self::Alpha,
            _ => Self::Custom(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_codec_properties() {
        assert!(VideoCodec::Vp9.is_compressed());
        assert!(!VideoCodec::V210.is_compressed());
        assert_eq!(VideoCodec::Uyvy.bytes_per_pixel(), Some(2.0));
    }

    #[test]
    fn test_audio_codec_properties() {
        assert!(AudioCodec::Opus.is_compressed());
        assert!(!AudioCodec::Pcm16.is_compressed());
        assert_eq!(AudioCodec::Pcm16.bytes_per_sample(), Some(2));
    }

    #[test]
    fn test_resolution() {
        let hd = Resolution::HD_1080;
        assert_eq!(hd.width, 1920);
        assert_eq!(hd.height, 1080);
        assert_eq!(hd.pixel_count(), 1920 * 1080);
    }

    #[test]
    fn test_frame_rate() {
        let fps = FrameRate::from_float(29.97).expect("should succeed in test");
        assert_eq!(fps.num, 30000);
        assert_eq!(fps.den, 1001);
        assert!((fps.to_float() - 29.97).abs() < 0.01);
    }

    #[test]
    fn test_frame_rate_common() {
        assert_eq!(FrameRate::FPS_60.to_float(), 60.0);
        assert_eq!(FrameRate::FPS_25.to_float(), 25.0);
    }

    #[test]
    fn test_video_config() {
        let config = VideoConfig::new(1920, 1080, 60.0).expect("should succeed in test");
        assert_eq!(config.format.resolution.width, 1920);
        assert_eq!(config.format.resolution.height, 1080);
    }

    #[test]
    fn test_audio_config() {
        let config = AudioConfig::new(48000, 2).expect("should succeed in test");
        assert_eq!(config.format.sample_rate, 48000);
        assert_eq!(config.format.channels, 2);
    }

    #[test]
    fn test_invalid_audio_config() {
        assert!(AudioConfig::new(48000, 0).is_err());
        assert!(AudioConfig::new(48000, 17).is_err());
    }

    #[test]
    fn test_stream_type_conversion() {
        assert_eq!(StreamType::Program.to_id(), 0);
        assert_eq!(StreamType::from_id(0), StreamType::Program);
        assert_eq!(StreamType::Preview.to_id(), 1);
        assert_eq!(StreamType::from_id(1), StreamType::Preview);
    }

    #[test]
    fn test_uncompressed_bitrate() {
        let format = VideoFormat::new(VideoCodec::Uyvy, Resolution::HD_1080, FrameRate::FPS_60);
        let bitrate = format
            .uncompressed_bitrate()
            .expect("should succeed in test");
        // 1920 * 1080 * 2 bytes * 60 fps * 8 bits
        assert_eq!(bitrate, 1920 * 1080 * 2 * 60 * 8);
    }

    #[test]
    fn test_audio_uncompressed_bitrate() {
        let format = AudioFormat::new(AudioCodec::Pcm16, 48000, 2).expect("should succeed in test");
        let bitrate = format
            .uncompressed_bitrate()
            .expect("should succeed in test");
        // 48000 * 2 channels * 16 bits
        assert_eq!(bitrate, 48000 * 2 * 16);
    }
}
