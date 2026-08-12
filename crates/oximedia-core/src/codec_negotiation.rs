//! Codec negotiation utilities for `OxiMedia`.
//!
//! This module provides types and functions for negotiating codec parameters
//! between local and remote endpoints, preferring hardware-accelerated codecs
//! when available.
//!
//! # Example
//!
//! ```
//! use oximedia_core::codec_negotiation::{CodecCapability, CodecNegotiator, negotiate};
//!
//! let local = vec![
//!     CodecCapability {
//!         name: "av1".to_string(),
//!         profiles: vec!["main".to_string()],
//!         max_level: 40,
//!         hardware_accelerated: false,
//!     },
//! ];
//! let remote = vec![
//!     CodecCapability {
//!         name: "av1".to_string(),
//!         profiles: vec!["main".to_string()],
//!         max_level: 30,
//!         hardware_accelerated: false,
//!     },
//! ];
//! let result = negotiate(&local, &remote);
//! assert!(result.is_some());
//! assert_eq!(result.expect("negotiation succeeded").selected_codec, "av1");
//! ```

#![allow(dead_code)]

/// Describes the codec capabilities of one endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecCapability {
    /// Codec name (e.g. `"av1"`, `"vp9"`).
    pub name: String,
    /// List of supported codec profiles.
    pub profiles: Vec<String>,
    /// Maximum codec level supported (e.g. 40 for level 4.0).
    pub max_level: u32,
    /// Whether the codec benefits from hardware acceleration on this device.
    pub hardware_accelerated: bool,
}

impl CodecCapability {
    /// Creates a new `CodecCapability`.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        profiles: Vec<String>,
        max_level: u32,
        hardware_accelerated: bool,
    ) -> Self {
        Self {
            name: name.into(),
            profiles,
            max_level,
            hardware_accelerated,
        }
    }

    /// Returns `true` if `profile` is listed in this capability.
    #[must_use]
    pub fn supports_profile(&self, profile: &str) -> bool {
        self.profiles.iter().any(|p| p == profile)
    }

    /// Returns `true` if this codec uses hardware acceleration.
    #[must_use]
    pub fn is_hw_accelerated(&self) -> bool {
        self.hardware_accelerated
    }
}

/// Handles codec negotiation between a local and a remote set of capabilities.
#[derive(Debug, Default)]
pub struct CodecNegotiator {
    /// Codecs supported locally.
    pub local_caps: Vec<CodecCapability>,
    /// Codecs supported by the remote endpoint.
    pub remote_caps: Vec<CodecCapability>,
}

impl CodecNegotiator {
    /// Creates an empty `CodecNegotiator`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a local codec capability.
    pub fn add_local(&mut self, cap: CodecCapability) {
        self.local_caps.push(cap);
    }

    /// Adds a remote codec capability.
    pub fn add_remote(&mut self, cap: CodecCapability) {
        self.remote_caps.push(cap);
    }

    /// Returns the names of codecs supported by both endpoints.
    #[must_use]
    pub fn common_codecs(&self) -> Vec<&str> {
        self.local_caps
            .iter()
            .filter(|l| self.remote_caps.iter().any(|r| r.name == l.name))
            .map(|l| l.name.as_str())
            .collect()
    }

    /// Returns the preferred common codec, favouring hardware-accelerated ones.
    ///
    /// Returns `None` when there are no codecs in common.
    #[must_use]
    pub fn preferred_codec(&self) -> Option<&str> {
        // Collect common names first.
        let common = self.common_codecs();
        if common.is_empty() {
            return None;
        }
        // Prefer hardware-accelerated; fall back to first common.
        for name in &common {
            if let Some(cap) = self.local_caps.iter().find(|c| &c.name.as_str() == name) {
                if cap.hardware_accelerated {
                    return Some(name);
                }
            }
        }
        common.into_iter().next()
    }
}

/// The result of a successful codec negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiationResult {
    /// The codec selected by both endpoints.
    pub selected_codec: String,
    /// The agreed-upon profile.
    pub profile: String,
    /// The agreed-upon level (minimum of local and remote max levels).
    pub level: u32,
    /// Whether the selected codec uses hardware acceleration on the local side.
    pub hardware_accelerated: bool,
}

impl NegotiationResult {
    /// Returns `true` if the selected codec uses hardware acceleration.
    #[must_use]
    pub fn is_hardware(&self) -> bool {
        self.hardware_accelerated
    }
}

/// Attempts to negotiate a codec between `local` and `remote` capability sets.
///
/// Hardware-accelerated codecs are preferred. The first common codec whose
/// profile list has at least one entry in common is selected.
///
/// Returns `None` when no mutually supported codec/profile pair exists.
#[must_use]
pub fn negotiate(
    local: &[CodecCapability],
    remote: &[CodecCapability],
) -> Option<NegotiationResult> {
    // Build a prioritised list: hw-accelerated local caps first.
    let mut ordered: Vec<&CodecCapability> = local.iter().collect();
    ordered.sort_by_key(|c| u8::from(!c.hardware_accelerated));

    for local_cap in ordered {
        if let Some(remote_cap) = remote.iter().find(|r| r.name == local_cap.name) {
            // Find a common profile.
            let common_profile = local_cap
                .profiles
                .iter()
                .find(|p| remote_cap.profiles.contains(p));
            if let Some(profile) = common_profile {
                let level = local_cap.max_level.min(remote_cap.max_level);
                return Some(NegotiationResult {
                    selected_codec: local_cap.name.clone(),
                    profile: profile.clone(),
                    level,
                    hardware_accelerated: local_cap.hardware_accelerated,
                });
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Automatic format negotiation
// ─────────────────────────────────────────────────────────────────────────────

use crate::types::{PixelFormat, SampleFormat};

/// Pixel format capabilities of a codec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelFormatCaps {
    /// Supported pixel formats, in order of preference.
    pub formats: Vec<PixelFormat>,
}

impl PixelFormatCaps {
    /// Creates new pixel-format capabilities.
    #[must_use]
    pub fn new(formats: Vec<PixelFormat>) -> Self {
        Self { formats }
    }

    /// Returns the first format supported by both sides, or `None`.
    #[must_use]
    pub fn negotiate(&self, other: &Self) -> Option<PixelFormat> {
        self.formats
            .iter()
            .find(|f| other.formats.contains(f))
            .copied()
    }

    /// Returns all formats supported by both sides.
    #[must_use]
    pub fn common_formats(&self, other: &Self) -> Vec<PixelFormat> {
        self.formats
            .iter()
            .filter(|f| other.formats.contains(f))
            .copied()
            .collect()
    }
}

/// Audio sample format capabilities of a codec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleFormatCaps {
    /// Supported sample formats, in order of preference.
    pub formats: Vec<SampleFormat>,
}

impl SampleFormatCaps {
    /// Creates new sample-format capabilities.
    #[must_use]
    pub fn new(formats: Vec<SampleFormat>) -> Self {
        Self { formats }
    }

    /// Returns the first format supported by both sides, or `None`.
    #[must_use]
    pub fn negotiate(&self, other: &Self) -> Option<SampleFormat> {
        self.formats
            .iter()
            .find(|f| other.formats.contains(f))
            .copied()
    }

    /// Returns all formats supported by both sides.
    #[must_use]
    pub fn common_formats(&self, other: &Self) -> Vec<SampleFormat> {
        self.formats
            .iter()
            .filter(|f| other.formats.contains(f))
            .copied()
            .collect()
    }
}

/// Full format capabilities for automatic negotiation between encoder/decoder.
#[derive(Debug, Clone)]
pub struct FormatCapabilities {
    /// Codec name (e.g. "av1", "opus").
    pub codec_name: String,
    /// Supported pixel formats (empty for audio-only codecs).
    pub pixel_formats: PixelFormatCaps,
    /// Supported sample formats (empty for video-only codecs).
    pub sample_formats: SampleFormatCaps,
    /// Supported sample rates (empty for video-only codecs).
    pub sample_rates: Vec<u32>,
    /// Supported channel counts (empty for video-only codecs).
    pub channel_counts: Vec<u32>,
}

impl FormatCapabilities {
    /// Creates a new `FormatCapabilities` for a video codec.
    #[must_use]
    pub fn video(codec_name: impl Into<String>, pixel_formats: Vec<PixelFormat>) -> Self {
        Self {
            codec_name: codec_name.into(),
            pixel_formats: PixelFormatCaps::new(pixel_formats),
            sample_formats: SampleFormatCaps::new(vec![]),
            sample_rates: vec![],
            channel_counts: vec![],
        }
    }

    /// Creates a new `FormatCapabilities` for an audio codec.
    #[must_use]
    pub fn audio(
        codec_name: impl Into<String>,
        sample_formats: Vec<SampleFormat>,
        sample_rates: Vec<u32>,
        channel_counts: Vec<u32>,
    ) -> Self {
        Self {
            codec_name: codec_name.into(),
            pixel_formats: PixelFormatCaps::new(vec![]),
            sample_formats: SampleFormatCaps::new(sample_formats),
            sample_rates,
            channel_counts,
        }
    }
}

/// Result of automatic format negotiation between encoder and decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatNegotiationResult {
    /// Negotiated pixel format (if video).
    pub pixel_format: Option<PixelFormat>,
    /// Negotiated sample format (if audio).
    pub sample_format: Option<SampleFormat>,
    /// Negotiated sample rate (if audio).
    pub sample_rate: Option<u32>,
    /// Negotiated channel count (if audio).
    pub channel_count: Option<u32>,
}

/// Negotiates the best format parameters between a decoder's output capabilities
/// and an encoder's input capabilities.
///
/// For each parameter, the first mutually-supported value (in the decoder's
/// preference order) is chosen. Returns `None` if the two capability sets have
/// no format in common for the relevant media type.
#[must_use]
pub fn negotiate_formats(
    decoder: &FormatCapabilities,
    encoder: &FormatCapabilities,
) -> Option<FormatNegotiationResult> {
    let is_video =
        !decoder.pixel_formats.formats.is_empty() && !encoder.pixel_formats.formats.is_empty();
    let is_audio =
        !decoder.sample_formats.formats.is_empty() && !encoder.sample_formats.formats.is_empty();

    if !is_video && !is_audio {
        return None;
    }

    let pixel_format = if is_video {
        let pf = decoder.pixel_formats.negotiate(&encoder.pixel_formats);
        pf?;
        pf
    } else {
        None
    };

    let sample_format = if is_audio {
        let sf = decoder.sample_formats.negotiate(&encoder.sample_formats);
        sf?;
        sf
    } else {
        None
    };

    let sample_rate = if is_audio {
        decoder
            .sample_rates
            .iter()
            .find(|r| encoder.sample_rates.contains(r))
            .copied()
    } else {
        None
    };

    let channel_count = if is_audio {
        decoder
            .channel_counts
            .iter()
            .find(|c| encoder.channel_counts.contains(c))
            .copied()
    } else {
        None
    };

    Some(FormatNegotiationResult {
        pixel_format,
        sample_format,
        sample_rate,
        channel_count,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Resolution / bitrate constraints for full auto-negotiation
// ─────────────────────────────────────────────────────────────────────────────

/// Resolution constraint for video negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionRange {
    /// Minimum supported width.
    pub min_width: u32,
    /// Maximum supported width.
    pub max_width: u32,
    /// Minimum supported height.
    pub min_height: u32,
    /// Maximum supported height.
    pub max_height: u32,
}

impl ResolutionRange {
    /// Creates a new resolution range.
    #[must_use]
    pub fn new(min_width: u32, max_width: u32, min_height: u32, max_height: u32) -> Self {
        Self {
            min_width,
            max_width,
            min_height,
            max_height,
        }
    }

    /// Returns whether the given dimensions fit within this range.
    #[must_use]
    pub fn contains(&self, width: u32, height: u32) -> bool {
        width >= self.min_width
            && width <= self.max_width
            && height >= self.min_height
            && height <= self.max_height
    }

    /// Returns the intersection of two resolution ranges, or `None` if they don't overlap.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let min_w = self.min_width.max(other.min_width);
        let max_w = self.max_width.min(other.max_width);
        let min_h = self.min_height.max(other.min_height);
        let max_h = self.max_height.min(other.max_height);
        if min_w <= max_w && min_h <= max_h {
            Some(Self::new(min_w, max_w, min_h, max_h))
        } else {
            None
        }
    }
}

impl Default for ResolutionRange {
    fn default() -> Self {
        Self {
            min_width: 1,
            max_width: 8192,
            min_height: 1,
            max_height: 4320,
        }
    }
}

/// Bitrate constraint for negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitrateRange {
    /// Minimum bitrate in bits per second.
    pub min_bps: u64,
    /// Maximum bitrate in bits per second.
    pub max_bps: u64,
}

impl BitrateRange {
    /// Creates a new bitrate range.
    #[must_use]
    pub fn new(min_bps: u64, max_bps: u64) -> Self {
        Self { min_bps, max_bps }
    }

    /// Returns whether the given bitrate fits within this range.
    #[must_use]
    pub fn contains(&self, bps: u64) -> bool {
        bps >= self.min_bps && bps <= self.max_bps
    }

    /// Returns the intersection of two bitrate ranges, or `None` if they don't overlap.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let min = self.min_bps.max(other.min_bps);
        let max = self.max_bps.min(other.max_bps);
        if min <= max {
            Some(Self::new(min, max))
        } else {
            None
        }
    }
}

impl Default for BitrateRange {
    fn default() -> Self {
        Self {
            min_bps: 0,
            max_bps: u64::MAX,
        }
    }
}

/// Full endpoint capabilities for automatic encoder/decoder negotiation.
///
/// Combines codec capabilities, format capabilities, and hardware constraints
/// into a single description that `AutoNegotiator` can reason about.
#[derive(Debug, Clone)]
pub struct EndpointCapabilities {
    /// Codec-level capabilities (profiles, levels, hw).
    pub codec: CodecCapability,
    /// Format capabilities (pixel formats, sample formats, sample rates, channels).
    pub formats: FormatCapabilities,
    /// Resolution constraint (video only; ignored for audio).
    pub resolution: ResolutionRange,
    /// Bitrate constraint.
    pub bitrate: BitrateRange,
}

impl EndpointCapabilities {
    /// Creates video endpoint capabilities.
    #[must_use]
    pub fn video(
        codec: CodecCapability,
        pixel_formats: Vec<PixelFormat>,
        resolution: ResolutionRange,
        bitrate: BitrateRange,
    ) -> Self {
        Self {
            formats: FormatCapabilities::video(&codec.name, pixel_formats),
            codec,
            resolution,
            bitrate,
        }
    }

    /// Creates audio endpoint capabilities.
    #[must_use]
    pub fn audio(
        codec: CodecCapability,
        sample_formats: Vec<SampleFormat>,
        sample_rates: Vec<u32>,
        channel_counts: Vec<u32>,
        bitrate: BitrateRange,
    ) -> Self {
        Self {
            formats: FormatCapabilities::audio(
                &codec.name,
                sample_formats,
                sample_rates,
                channel_counts,
            ),
            codec,
            resolution: ResolutionRange::default(),
            bitrate,
        }
    }
}

/// Result of full automatic negotiation.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoNegotiationResult {
    /// Selected codec name.
    pub codec: String,
    /// Agreed profile.
    pub profile: String,
    /// Agreed level (min of both sides).
    pub level: u32,
    /// Whether hardware acceleration is available.
    pub hardware_accelerated: bool,
    /// Negotiated format parameters.
    pub format: FormatNegotiationResult,
    /// Negotiated resolution range (video only).
    pub resolution: Option<ResolutionRange>,
    /// Negotiated bitrate range.
    pub bitrate: Option<BitrateRange>,
    /// Quality score (0.0 - 1.0, higher is better).
    pub score: f64,
}

/// Automatic negotiator that finds the best encoder/decoder pairing.
///
/// Combines codec negotiation, format negotiation, resolution/bitrate constraint
/// intersection, and quality scoring into a single pass.
///
/// # Example
///
/// ```
/// use oximedia_core::codec_negotiation::*;
/// use oximedia_core::types::{PixelFormat, SampleFormat};
///
/// let decoder = EndpointCapabilities::video(
///     CodecCapability::new("av1", vec!["main".into()], 50, true),
///     vec![PixelFormat::Yuv420p, PixelFormat::Yuv420p10le],
///     ResolutionRange::new(1, 3840, 1, 2160),
///     BitrateRange::new(500_000, 20_000_000),
/// );
/// let encoder = EndpointCapabilities::video(
///     CodecCapability::new("av1", vec!["main".into()], 40, false),
///     vec![PixelFormat::Yuv420p10le, PixelFormat::Yuv420p],
///     ResolutionRange::new(1, 1920, 1, 1080),
///     BitrateRange::new(1_000_000, 15_000_000),
/// );
/// let result = auto_negotiate(&decoder, &encoder).expect("should negotiate");
/// assert_eq!(result.codec, "av1");
/// assert_eq!(result.format.pixel_format, Some(PixelFormat::Yuv420p));
/// assert!(result.resolution.is_some());
/// ```
#[must_use]
pub fn auto_negotiate(
    decoder: &EndpointCapabilities,
    encoder: &EndpointCapabilities,
) -> Option<AutoNegotiationResult> {
    // 1. Codec-level negotiation
    let codec_result = negotiate(
        std::slice::from_ref(&decoder.codec),
        std::slice::from_ref(&encoder.codec),
    )?;

    // 2. Format negotiation
    let format_result = negotiate_formats(&decoder.formats, &encoder.formats)?;

    // 3. Resolution intersection
    let resolution = decoder.resolution.intersect(&encoder.resolution);

    // 4. Bitrate intersection
    let bitrate = decoder.bitrate.intersect(&encoder.bitrate);

    // 5. Compute quality score
    let score = compute_score(&codec_result, &format_result, &resolution, &bitrate);

    Some(AutoNegotiationResult {
        codec: codec_result.selected_codec,
        profile: codec_result.profile,
        level: codec_result.level,
        hardware_accelerated: codec_result.hardware_accelerated,
        format: format_result,
        resolution,
        bitrate,
        score,
    })
}

/// Computes a quality score in [0.0, 1.0] for a negotiation result.
///
/// Factors:
/// - Hardware acceleration: +0.2
/// - Higher codec level: up to +0.3
/// - Higher bit-depth pixel format: up to +0.2
/// - Resolution available: +0.15
/// - Bitrate available: +0.15
fn compute_score(
    codec: &NegotiationResult,
    format: &FormatNegotiationResult,
    resolution: &Option<ResolutionRange>,
    bitrate: &Option<BitrateRange>,
) -> f64 {
    let mut score = 0.0;

    // Hardware acceleration bonus
    if codec.hardware_accelerated {
        score += 0.2;
    }

    // Codec level score (normalised to 0..0.3 assuming level range 10..63)
    let level_norm = f64::from(codec.level.min(63).saturating_sub(10)) / 53.0;
    score += level_norm * 0.3;

    // Pixel format bit depth score
    if let Some(pf) = format.pixel_format {
        let depth_score = f64::from(pf.bits_per_component().min(16)) / 16.0;
        score += depth_score * 0.2;
    }

    // Resolution available
    if resolution.is_some() {
        score += 0.15;
    }

    // Bitrate available
    if bitrate.is_some() {
        score += 0.15;
    }

    // Clamp to [0.0, 1.0]
    score.min(1.0)
}

// ─────────────────────────────────────────────────────────────────────────────
// FormatCost trait + FormatNegotiator
// ─────────────────────────────────────────────────────────────────────────────

/// Describes the cost of converting between two instances of the same format
/// type (e.g., two [`PixelFormat`] values or two [`SampleFormat`] values).
///
/// Implementors return `Some(cost)` where the ordinal indicates relative
/// expense:
/// - `0` — identical formats (no conversion)
/// - `1` — same family, trivial re-interpretation (e.g., NV12 ↔ NV21)
/// - `2` — same colour space, different subsampling or bit-depth
/// - `3` — different colour space (e.g., YUV ↔ RGB)
/// - `4` — any other cross-family conversion
///
/// Return `None` to indicate that no conversion path exists at all.
pub trait FormatCost: Clone + PartialEq {
    /// Returns the conversion cost from `self` to `target`, or `None` if no
    /// conversion path is available.
    fn conversion_cost(&self, target: &Self) -> Option<u32>;
}

// ── PixelFormat ──────────────────────────────────────────────────────────────

/// Helper — returns `true` if `f` belongs to the YUV 4:2:0 family.
fn pf_is_yuv420(f: &PixelFormat) -> bool {
    matches!(
        f,
        PixelFormat::Yuv420p | PixelFormat::Nv12 | PixelFormat::Nv21
    )
}

/// Helper — returns `true` if `f` belongs to the YUV 4:2:2 family.
fn pf_is_yuv422(f: &PixelFormat) -> bool {
    // The packed capture formats belong to the 4:2:2 family too, so a
    // Yuyv422/Uyvy422 <-> Yuv422p negotiation scores as a same-family
    // conversion rather than "other family". (The 10/12/16-bit planar
    // variants predate this helper and are deliberately left unlisted so
    // their negotiation cost does not silently change here.)
    matches!(
        f,
        PixelFormat::Yuv422p | PixelFormat::Yuyv422 | PixelFormat::Uyvy422
    )
}

/// Helper — returns `true` if `f` belongs to the YUV 4:4:4 family.
fn pf_is_yuv444(f: &PixelFormat) -> bool {
    matches!(f, PixelFormat::Yuv444p)
}

/// Helper — returns `true` if `f` is a packed / planar RGB/RGBA format.
fn pf_is_rgb(f: &PixelFormat) -> bool {
    matches!(f, PixelFormat::Rgb24 | PixelFormat::Rgba32)
}

/// Helper — returns `true` if `f` is a greyscale format.
fn pf_is_gray(f: &PixelFormat) -> bool {
    matches!(f, PixelFormat::Gray8 | PixelFormat::Gray16)
}

/// Helper — returns `true` if `f` is a high-bit-depth YUV semi-planar format.
fn pf_is_hbd_yuv(f: &PixelFormat) -> bool {
    matches!(
        f,
        PixelFormat::Yuv420p10le | PixelFormat::Yuv420p12le | PixelFormat::P010 | PixelFormat::P016
    )
}

impl FormatCost for PixelFormat {
    fn conversion_cost(&self, target: &Self) -> Option<u32> {
        if self == target {
            return Some(0);
        }

        // Same family — cost 1
        if (pf_is_yuv420(self) && pf_is_yuv420(target))
            || (pf_is_yuv422(self) && pf_is_yuv422(target))
            || (pf_is_yuv444(self) && pf_is_yuv444(target))
            || (pf_is_rgb(self) && pf_is_rgb(target))
            || (pf_is_gray(self) && pf_is_gray(target))
            || (pf_is_hbd_yuv(self) && pf_is_hbd_yuv(target))
        {
            return Some(1);
        }

        let self_yuv =
            pf_is_yuv420(self) || pf_is_yuv422(self) || pf_is_yuv444(self) || pf_is_hbd_yuv(self);
        let target_yuv = pf_is_yuv420(target)
            || pf_is_yuv422(target)
            || pf_is_yuv444(target)
            || pf_is_hbd_yuv(target);

        // Same colour space (YUV), different subsampling — cost 2
        if self_yuv && target_yuv {
            return Some(2);
        }

        // RGB ↔ YUV — cost 3
        if (pf_is_rgb(self) && target_yuv) || (self_yuv && pf_is_rgb(target)) {
            return Some(3);
        }

        // Any other cross-family conversion — cost 4
        Some(4)
    }
}

// ── SampleFormat ─────────────────────────────────────────────────────────────

impl FormatCost for SampleFormat {
    fn conversion_cost(&self, target: &Self) -> Option<u32> {
        if self == target {
            return Some(0);
        }

        // Interleaved ↔ planar of same width & encoding — cost 1
        // (self.to_packed() == target.to_packed() covers e.g. S16 ↔ S16p)
        if self.to_packed() == target.to_packed() {
            return Some(1);
        }

        // Same numeric family (both int or both float), different bit-depth — cost 2
        let self_float = self.is_float();
        let target_float = target.is_float();
        if self_float == target_float {
            return Some(2);
        }

        // Integer ↔ float — cost 3
        Some(3)
    }
}

// ── FormatConversionResult ────────────────────────────────────────────────────

/// Result of a [`FormatNegotiator`] negotiation run.
///
/// Discriminates between a direct match (no conversion needed), a conversion
/// with an associated cost ordinal, and a fully incompatible pair.
///
/// # Examples
///
/// ```
/// use oximedia_core::codec_negotiation::{FormatNegotiator, FormatConversionResult};
/// use oximedia_core::types::PixelFormat;
///
/// let neg = FormatNegotiator::<PixelFormat> {
///     decoder_produces: &[PixelFormat::Yuv420p],
///     encoder_accepts: &[PixelFormat::Yuv420p],
/// };
/// assert_eq!(neg.negotiate(), FormatConversionResult::Direct(PixelFormat::Yuv420p));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum FormatConversionResult<F> {
    /// Encoder directly accepts what the decoder produces — no conversion needed.
    Direct(F),
    /// A conversion is required.  `cost` is an ordinal (0 = identity, higher =
    /// more expensive).
    Convert {
        /// The format the decoder produces.
        from: F,
        /// The format the encoder accepts.
        to: F,
        /// Conversion cost ordinal.
        cost: u32,
    },
    /// No compatible conversion path exists.
    Incompatible,
}

// ── FormatNegotiator ──────────────────────────────────────────────────────────

/// Automatically selects the best [`PixelFormat`] or [`SampleFormat`] that
/// bridges what a decoder produces and what an encoder accepts.
///
/// The negotiator first looks for a **direct match** (zero-cost); if none
/// exists it picks the conversion with the **lowest cost** as determined by
/// [`FormatCost::conversion_cost`].  If no conversion path exists it returns
/// [`FormatConversionResult::Incompatible`].
///
/// # Examples
///
/// ```
/// use oximedia_core::codec_negotiation::{FormatNegotiator, FormatConversionResult};
/// use oximedia_core::types::PixelFormat;
///
/// let neg = FormatNegotiator::<PixelFormat> {
///     decoder_produces: &[PixelFormat::Yuv422p],
///     encoder_accepts: &[PixelFormat::Yuv420p],
/// };
/// match neg.negotiate() {
///     FormatConversionResult::Convert { from, to, cost } => {
///         assert_eq!(from, PixelFormat::Yuv422p);
///         assert_eq!(to, PixelFormat::Yuv420p);
///         assert!(cost >= 1);
///     }
///     other => panic!("unexpected: {other:?}"),
/// }
/// ```
pub struct FormatNegotiator<'a, F> {
    /// Pixel / sample formats the decoder is able to output.
    pub decoder_produces: &'a [F],
    /// Pixel / sample formats the encoder is able to accept as input.
    pub encoder_accepts: &'a [F],
}

impl<F> FormatNegotiator<'_, F>
where
    F: FormatCost + std::fmt::Debug,
{
    /// Runs the negotiation and returns the best [`FormatConversionResult`].
    #[must_use]
    pub fn negotiate(&self) -> FormatConversionResult<F> {
        // Pass 1: direct match (identity — cost 0)
        for prod in self.decoder_produces {
            for acc in self.encoder_accepts {
                if prod == acc {
                    return FormatConversionResult::Direct(prod.clone());
                }
            }
        }

        // Pass 2: cheapest conversion
        let mut best: Option<(F, F, u32)> = None;
        for prod in self.decoder_produces {
            for acc in self.encoder_accepts {
                if let Some(cost) = prod.conversion_cost(acc) {
                    let is_better = best.as_ref().map_or(true, |(_, _, bc)| cost < *bc);
                    if is_better {
                        best = Some((prod.clone(), acc.clone(), cost));
                    }
                }
            }
        }

        if let Some((from, to, cost)) = best {
            FormatConversionResult::Convert { from, to, cost }
        } else {
            FormatConversionResult::Incompatible
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn av1_cap(hw: bool) -> CodecCapability {
        CodecCapability::new("av1", vec!["main".to_string(), "high".to_string()], 40, hw)
    }

    fn vp9_cap() -> CodecCapability {
        CodecCapability::new("vp9", vec!["profile0".to_string()], 50, false)
    }

    // 1. supports_profile – positive
    #[test]
    fn test_supports_profile_positive() {
        let cap = av1_cap(false);
        assert!(cap.supports_profile("main"));
        assert!(cap.supports_profile("high"));
    }

    // 2. supports_profile – negative
    #[test]
    fn test_supports_profile_negative() {
        let cap = av1_cap(false);
        assert!(!cap.supports_profile("baseline"));
    }

    // 3. is_hw_accelerated
    #[test]
    fn test_is_hw_accelerated() {
        assert!(av1_cap(true).is_hw_accelerated());
        assert!(!av1_cap(false).is_hw_accelerated());
    }

    // 4. CodecNegotiator::common_codecs – overlap
    #[test]
    fn test_common_codecs_overlap() {
        let mut neg = CodecNegotiator::new();
        neg.add_local(av1_cap(false));
        neg.add_local(vp9_cap());
        neg.add_remote(av1_cap(false));
        let common = neg.common_codecs();
        assert_eq!(common, vec!["av1"]);
    }

    // 5. CodecNegotiator::common_codecs – no overlap
    #[test]
    fn test_common_codecs_no_overlap() {
        let mut neg = CodecNegotiator::new();
        neg.add_local(vp9_cap());
        neg.add_remote(av1_cap(false));
        assert!(neg.common_codecs().is_empty());
    }

    // 6. preferred_codec – hw preferred
    #[test]
    fn test_preferred_codec_hw_first() {
        let mut neg = CodecNegotiator::new();
        neg.add_local(vp9_cap()); // software
        neg.add_local(av1_cap(true)); // hardware
        neg.add_remote(vp9_cap());
        neg.add_remote(av1_cap(true));
        // av1 is hw, should be preferred
        assert_eq!(neg.preferred_codec(), Some("av1"));
    }

    // 7. preferred_codec – no common
    #[test]
    fn test_preferred_codec_none() {
        let mut neg = CodecNegotiator::new();
        neg.add_local(vp9_cap());
        neg.add_remote(av1_cap(false));
        assert!(neg.preferred_codec().is_none());
    }

    // 8. preferred_codec – falls back to first when no hw
    #[test]
    fn test_preferred_codec_fallback() {
        let mut neg = CodecNegotiator::new();
        neg.add_local(av1_cap(false));
        neg.add_local(vp9_cap());
        neg.add_remote(av1_cap(false));
        neg.add_remote(vp9_cap());
        // No hw acceleration; first common codec returned
        let pref = neg.preferred_codec();
        assert!(pref.is_some());
    }

    // 9. negotiate – success
    #[test]
    fn test_negotiate_success() {
        let local = vec![av1_cap(false)];
        let remote = vec![av1_cap(false)];
        let result = negotiate(&local, &remote).expect("negotiation should succeed");
        assert_eq!(result.selected_codec, "av1");
        assert!(result.profile == "main" || result.profile == "high");
        assert_eq!(result.level, 40);
        assert!(!result.is_hardware());
    }

    // 10. negotiate – hw preferred
    #[test]
    fn test_negotiate_prefers_hw() {
        let local = vec![vp9_cap(), av1_cap(true)];
        let remote = vec![vp9_cap(), av1_cap(true)];
        let result = negotiate(&local, &remote).expect("negotiation should succeed");
        assert_eq!(result.selected_codec, "av1");
        assert!(result.is_hardware());
    }

    // 11. negotiate – level is min of both
    #[test]
    fn test_negotiate_level_min() {
        let local = vec![CodecCapability::new(
            "av1",
            vec!["main".to_string()],
            50,
            false,
        )];
        let remote = vec![CodecCapability::new(
            "av1",
            vec!["main".to_string()],
            30,
            false,
        )];
        let result = negotiate(&local, &remote).expect("negotiation should succeed");
        assert_eq!(result.level, 30);
    }

    // 12. negotiate – no common codec returns None
    #[test]
    fn test_negotiate_no_common() {
        let local = vec![av1_cap(false)];
        let remote = vec![vp9_cap()];
        assert!(negotiate(&local, &remote).is_none());
    }

    // 13. negotiate – profile mismatch returns None
    #[test]
    fn test_negotiate_profile_mismatch() {
        let local = vec![CodecCapability::new(
            "av1",
            vec!["high".to_string()],
            40,
            false,
        )];
        let remote = vec![CodecCapability::new(
            "av1",
            vec!["baseline".to_string()],
            40,
            false,
        )];
        assert!(negotiate(&local, &remote).is_none());
    }

    // 14. NegotiationResult::is_hardware
    #[test]
    fn test_negotiation_result_is_hardware() {
        let r = NegotiationResult {
            selected_codec: "av1".to_string(),
            profile: "main".to_string(),
            level: 40,
            hardware_accelerated: true,
        };
        assert!(r.is_hardware());
        let r2 = NegotiationResult {
            hardware_accelerated: false,
            ..r
        };
        assert!(!r2.is_hardware());
    }

    // ── Format negotiation tests ──────────────────────────────────────

    #[test]
    fn test_pixel_format_negotiate_common() {
        let dec = PixelFormatCaps::new(vec![PixelFormat::Yuv420p, PixelFormat::Nv12]);
        let enc = PixelFormatCaps::new(vec![PixelFormat::Nv12, PixelFormat::Yuv420p]);
        // Decoder prefers Yuv420p, encoder also supports it
        assert_eq!(dec.negotiate(&enc), Some(PixelFormat::Yuv420p));
    }

    #[test]
    fn test_pixel_format_negotiate_no_common() {
        let dec = PixelFormatCaps::new(vec![PixelFormat::Yuv420p]);
        let enc = PixelFormatCaps::new(vec![PixelFormat::Rgb24]);
        assert_eq!(dec.negotiate(&enc), None);
    }

    #[test]
    fn test_pixel_format_common_formats() {
        let dec = PixelFormatCaps::new(vec![
            PixelFormat::Yuv420p,
            PixelFormat::Nv12,
            PixelFormat::Rgb24,
        ]);
        let enc = PixelFormatCaps::new(vec![PixelFormat::Nv12, PixelFormat::Rgb24]);
        let common = dec.common_formats(&enc);
        assert_eq!(common, vec![PixelFormat::Nv12, PixelFormat::Rgb24]);
    }

    #[test]
    fn test_sample_format_negotiate_common() {
        let dec = SampleFormatCaps::new(vec![SampleFormat::F32, SampleFormat::S16]);
        let enc = SampleFormatCaps::new(vec![SampleFormat::S16, SampleFormat::S24]);
        assert_eq!(dec.negotiate(&enc), Some(SampleFormat::S16));
    }

    #[test]
    fn test_sample_format_negotiate_no_common() {
        let dec = SampleFormatCaps::new(vec![SampleFormat::F32]);
        let enc = SampleFormatCaps::new(vec![SampleFormat::S24]);
        assert_eq!(dec.negotiate(&enc), None);
    }

    #[test]
    fn test_sample_format_common_formats() {
        let dec = SampleFormatCaps::new(vec![
            SampleFormat::F32,
            SampleFormat::S16,
            SampleFormat::S24,
        ]);
        let enc = SampleFormatCaps::new(vec![SampleFormat::S24, SampleFormat::F32]);
        let common = dec.common_formats(&enc);
        assert_eq!(common, vec![SampleFormat::F32, SampleFormat::S24]);
    }

    #[test]
    fn test_negotiate_formats_video() {
        let decoder =
            FormatCapabilities::video("av1", vec![PixelFormat::Yuv420p, PixelFormat::Yuv420p10le]);
        let encoder =
            FormatCapabilities::video("av1", vec![PixelFormat::Yuv420p10le, PixelFormat::Yuv420p]);
        let result = negotiate_formats(&decoder, &encoder).expect("should negotiate");
        assert_eq!(result.pixel_format, Some(PixelFormat::Yuv420p));
        assert_eq!(result.sample_format, None);
    }

    #[test]
    fn test_negotiate_formats_audio() {
        let decoder = FormatCapabilities::audio(
            "opus",
            vec![SampleFormat::F32, SampleFormat::S16],
            vec![48000, 44100],
            vec![2, 1],
        );
        let encoder = FormatCapabilities::audio(
            "opus",
            vec![SampleFormat::S16, SampleFormat::F32],
            vec![48000],
            vec![2],
        );
        let result = negotiate_formats(&decoder, &encoder).expect("should negotiate");
        assert_eq!(result.sample_format, Some(SampleFormat::F32));
        assert_eq!(result.sample_rate, Some(48000));
        assert_eq!(result.channel_count, Some(2));
        assert_eq!(result.pixel_format, None);
    }

    #[test]
    fn test_negotiate_formats_no_common_pixel_format() {
        let decoder = FormatCapabilities::video("av1", vec![PixelFormat::Yuv420p]);
        let encoder = FormatCapabilities::video("av1", vec![PixelFormat::Rgb24]);
        assert!(negotiate_formats(&decoder, &encoder).is_none());
    }

    #[test]
    fn test_negotiate_formats_no_common_sample_format() {
        let decoder =
            FormatCapabilities::audio("opus", vec![SampleFormat::F32], vec![48000], vec![2]);
        let encoder =
            FormatCapabilities::audio("opus", vec![SampleFormat::S24], vec![48000], vec![2]);
        assert!(negotiate_formats(&decoder, &encoder).is_none());
    }

    #[test]
    fn test_negotiate_formats_empty_caps() {
        let decoder = FormatCapabilities::video("av1", vec![]);
        let encoder = FormatCapabilities::video("av1", vec![]);
        assert!(negotiate_formats(&decoder, &encoder).is_none());
    }

    #[test]
    fn test_negotiate_formats_video_with_semi_planar() {
        let decoder = FormatCapabilities::video("av1", vec![PixelFormat::Nv12, PixelFormat::P010]);
        let encoder =
            FormatCapabilities::video("av1", vec![PixelFormat::P010, PixelFormat::Yuv420p]);
        let result = negotiate_formats(&decoder, &encoder).expect("should negotiate");
        assert_eq!(result.pixel_format, Some(PixelFormat::P010));
    }

    #[test]
    fn test_format_capabilities_video_constructor() {
        let caps = FormatCapabilities::video("vp9", vec![PixelFormat::Yuv420p]);
        assert_eq!(caps.codec_name, "vp9");
        assert_eq!(caps.pixel_formats.formats.len(), 1);
        assert!(caps.sample_formats.formats.is_empty());
        assert!(caps.sample_rates.is_empty());
        assert!(caps.channel_counts.is_empty());
    }

    #[test]
    fn test_format_capabilities_audio_constructor() {
        let caps = FormatCapabilities::audio(
            "flac",
            vec![SampleFormat::S16, SampleFormat::S24],
            vec![44100, 48000, 96000],
            vec![1, 2],
        );
        assert_eq!(caps.codec_name, "flac");
        assert!(caps.pixel_formats.formats.is_empty());
        assert_eq!(caps.sample_formats.formats.len(), 2);
        assert_eq!(caps.sample_rates.len(), 3);
        assert_eq!(caps.channel_counts.len(), 2);
    }

    // ── ResolutionRange tests ───────────────────────────────────────

    #[test]
    fn test_resolution_range_contains() {
        let r = ResolutionRange::new(1, 1920, 1, 1080);
        assert!(r.contains(1920, 1080));
        assert!(r.contains(1, 1));
        assert!(r.contains(1280, 720));
        assert!(!r.contains(3840, 2160));
        assert!(!r.contains(0, 0));
    }

    #[test]
    fn test_resolution_range_intersect() {
        let a = ResolutionRange::new(1, 3840, 1, 2160);
        let b = ResolutionRange::new(640, 1920, 480, 1080);
        let intersect = a.intersect(&b).expect("should intersect");
        assert_eq!(intersect.min_width, 640);
        assert_eq!(intersect.max_width, 1920);
        assert_eq!(intersect.min_height, 480);
        assert_eq!(intersect.max_height, 1080);
    }

    #[test]
    fn test_resolution_range_no_intersect() {
        let a = ResolutionRange::new(1, 640, 1, 480);
        let b = ResolutionRange::new(1920, 3840, 1080, 2160);
        assert!(a.intersect(&b).is_none());
    }

    #[test]
    fn test_resolution_range_default() {
        let r = ResolutionRange::default();
        assert!(r.contains(1920, 1080));
        assert!(r.contains(7680, 4320));
    }

    // ── BitrateRange tests ──────────────────────────────────────────

    #[test]
    fn test_bitrate_range_contains() {
        let b = BitrateRange::new(1_000_000, 10_000_000);
        assert!(b.contains(5_000_000));
        assert!(b.contains(1_000_000));
        assert!(b.contains(10_000_000));
        assert!(!b.contains(500_000));
        assert!(!b.contains(20_000_000));
    }

    #[test]
    fn test_bitrate_range_intersect() {
        let a = BitrateRange::new(500_000, 20_000_000);
        let b = BitrateRange::new(1_000_000, 15_000_000);
        let intersect = a.intersect(&b).expect("should intersect");
        assert_eq!(intersect.min_bps, 1_000_000);
        assert_eq!(intersect.max_bps, 15_000_000);
    }

    #[test]
    fn test_bitrate_range_no_intersect() {
        let a = BitrateRange::new(1_000_000, 2_000_000);
        let b = BitrateRange::new(5_000_000, 10_000_000);
        assert!(a.intersect(&b).is_none());
    }

    #[test]
    fn test_bitrate_range_default() {
        let b = BitrateRange::default();
        assert!(b.contains(0));
        assert!(b.contains(1_000_000_000));
    }

    // ── auto_negotiate tests ────────────────────────────────────────

    #[test]
    fn test_auto_negotiate_video_success() {
        let decoder = EndpointCapabilities::video(
            CodecCapability::new("av1", vec!["main".into()], 50, true),
            vec![PixelFormat::Yuv420p, PixelFormat::Yuv420p10le],
            ResolutionRange::new(1, 3840, 1, 2160),
            BitrateRange::new(500_000, 20_000_000),
        );
        let encoder = EndpointCapabilities::video(
            CodecCapability::new("av1", vec!["main".into()], 40, false),
            vec![PixelFormat::Yuv420p10le, PixelFormat::Yuv420p],
            ResolutionRange::new(1, 1920, 1, 1080),
            BitrateRange::new(1_000_000, 15_000_000),
        );
        let result = auto_negotiate(&decoder, &encoder).expect("should negotiate");
        assert_eq!(result.codec, "av1");
        assert_eq!(result.profile, "main");
        assert_eq!(result.level, 40); // min of 50 and 40
        assert!(result.hardware_accelerated); // decoder has hw
        assert_eq!(result.format.pixel_format, Some(PixelFormat::Yuv420p));

        let res = result.resolution.expect("should have resolution");
        assert_eq!(res.max_width, 1920);
        assert_eq!(res.max_height, 1080);

        let br = result.bitrate.expect("should have bitrate");
        assert_eq!(br.min_bps, 1_000_000);
        assert_eq!(br.max_bps, 15_000_000);

        assert!(result.score > 0.0);
        assert!(result.score <= 1.0);
    }

    #[test]
    fn test_auto_negotiate_audio_success() {
        let decoder = EndpointCapabilities::audio(
            CodecCapability::new("opus", vec!["default".into()], 0, false),
            vec![SampleFormat::F32, SampleFormat::S16],
            vec![48000, 44100],
            vec![2, 1],
            BitrateRange::new(64_000, 510_000),
        );
        let encoder = EndpointCapabilities::audio(
            CodecCapability::new("opus", vec!["default".into()], 0, false),
            vec![SampleFormat::S16, SampleFormat::F32],
            vec![48000],
            vec![2],
            BitrateRange::new(96_000, 256_000),
        );
        let result = auto_negotiate(&decoder, &encoder).expect("should negotiate");
        assert_eq!(result.codec, "opus");
        assert_eq!(result.format.sample_format, Some(SampleFormat::F32));
        assert_eq!(result.format.sample_rate, Some(48000));
        assert_eq!(result.format.channel_count, Some(2));

        let br = result.bitrate.expect("should have bitrate");
        assert_eq!(br.min_bps, 96_000);
        assert_eq!(br.max_bps, 256_000);
    }

    #[test]
    fn test_auto_negotiate_codec_mismatch() {
        let decoder = EndpointCapabilities::video(
            CodecCapability::new("av1", vec!["main".into()], 40, false),
            vec![PixelFormat::Yuv420p],
            ResolutionRange::default(),
            BitrateRange::default(),
        );
        let encoder = EndpointCapabilities::video(
            CodecCapability::new("vp9", vec!["profile0".into()], 40, false),
            vec![PixelFormat::Yuv420p],
            ResolutionRange::default(),
            BitrateRange::default(),
        );
        assert!(auto_negotiate(&decoder, &encoder).is_none());
    }

    #[test]
    fn test_auto_negotiate_format_mismatch() {
        let decoder = EndpointCapabilities::video(
            CodecCapability::new("av1", vec!["main".into()], 40, false),
            vec![PixelFormat::Yuv420p],
            ResolutionRange::default(),
            BitrateRange::default(),
        );
        let encoder = EndpointCapabilities::video(
            CodecCapability::new("av1", vec!["main".into()], 40, false),
            vec![PixelFormat::Rgb24],
            ResolutionRange::default(),
            BitrateRange::default(),
        );
        assert!(auto_negotiate(&decoder, &encoder).is_none());
    }

    #[test]
    fn test_auto_negotiate_hw_boosts_score() {
        let make_endpoint = |hw: bool| {
            EndpointCapabilities::video(
                CodecCapability::new("av1", vec!["main".into()], 40, hw),
                vec![PixelFormat::Yuv420p],
                ResolutionRange::default(),
                BitrateRange::default(),
            )
        };
        let hw_result =
            auto_negotiate(&make_endpoint(true), &make_endpoint(true)).expect("should negotiate");
        let sw_result =
            auto_negotiate(&make_endpoint(false), &make_endpoint(false)).expect("should negotiate");
        assert!(hw_result.score > sw_result.score);
    }

    #[test]
    fn test_auto_negotiate_resolution_no_overlap_still_returns() {
        // Resolution ranges don't overlap, but codec+format do => result returned with None resolution
        let decoder = EndpointCapabilities::video(
            CodecCapability::new("av1", vec!["main".into()], 40, false),
            vec![PixelFormat::Yuv420p],
            ResolutionRange::new(1, 640, 1, 480),
            BitrateRange::default(),
        );
        let encoder = EndpointCapabilities::video(
            CodecCapability::new("av1", vec!["main".into()], 40, false),
            vec![PixelFormat::Yuv420p],
            ResolutionRange::new(1920, 3840, 1080, 2160),
            BitrateRange::default(),
        );
        let result = auto_negotiate(&decoder, &encoder).expect("should still negotiate");
        assert!(result.resolution.is_none());
    }

    #[test]
    fn test_score_range() {
        // Score should always be in [0, 1]
        let endpoint = EndpointCapabilities::video(
            CodecCapability::new("av1", vec!["main".into()], 63, true),
            vec![PixelFormat::Yuv420p10le],
            ResolutionRange::default(),
            BitrateRange::default(),
        );
        let result = auto_negotiate(&endpoint, &endpoint).expect("should negotiate");
        assert!(result.score >= 0.0);
        assert!(result.score <= 1.0);
    }

    #[test]
    fn test_endpoint_capabilities_video_constructor() {
        let ep = EndpointCapabilities::video(
            CodecCapability::new("vp9", vec!["profile0".into()], 50, false),
            vec![PixelFormat::Yuv420p, PixelFormat::Nv12],
            ResolutionRange::new(1, 1920, 1, 1080),
            BitrateRange::new(1_000_000, 10_000_000),
        );
        assert_eq!(ep.codec.name, "vp9");
        assert_eq!(ep.formats.pixel_formats.formats.len(), 2);
        assert_eq!(ep.resolution.max_width, 1920);
        assert_eq!(ep.bitrate.max_bps, 10_000_000);
    }

    #[test]
    fn test_endpoint_capabilities_audio_constructor() {
        let ep = EndpointCapabilities::audio(
            CodecCapability::new("flac", vec!["default".into()], 0, false),
            vec![SampleFormat::S16, SampleFormat::S24],
            vec![44100, 48000, 96000],
            vec![1, 2],
            BitrateRange::new(0, 5_000_000),
        );
        assert_eq!(ep.codec.name, "flac");
        assert_eq!(ep.formats.sample_formats.formats.len(), 2);
        assert_eq!(ep.formats.sample_rates.len(), 3);
    }
}
