//! Normalization target standards and presets.
//!
//! This module defines loudness targets for various broadcast and streaming standards.

use oximedia_metering::Standard;

/// Normalization target with loudness and peak specifications.
#[derive(Clone, Debug)]
pub struct NormalizationTarget {
    /// Target loudness in LUFS.
    pub target_lufs: f64,

    /// Maximum true peak in dBTP.
    pub max_peak_dbtp: f64,

    /// Loudness tolerance in LU.
    pub tolerance_lu: f64,

    /// Target name/description.
    pub name: String,

    /// Whether to apply limiting.
    pub apply_limiting: bool,

    /// Whether to apply DRC.
    pub apply_drc: bool,

    /// Preserve loudness range when possible.
    pub preserve_lra: bool,
}

impl NormalizationTarget {
    /// Create a new normalization target.
    pub fn new(target_lufs: f64, max_peak_dbtp: f64, name: String) -> Self {
        Self {
            target_lufs,
            max_peak_dbtp,
            tolerance_lu: 1.0,
            name,
            apply_limiting: true,
            apply_drc: false,
            preserve_lra: true,
        }
    }

    /// Create from a metering standard.
    pub fn from_standard(standard: Standard) -> Self {
        Self {
            target_lufs: standard.target_lufs(),
            max_peak_dbtp: standard.max_true_peak_dbtp(),
            tolerance_lu: standard.tolerance_lu(),
            name: standard.name().to_string(),
            apply_limiting: true,
            apply_drc: false,
            preserve_lra: true,
        }
    }

    /// Check if a measured loudness is within tolerance.
    pub fn is_compliant(&self, measured_lufs: f64) -> bool {
        measured_lufs >= self.target_lufs - self.tolerance_lu
            && measured_lufs <= self.target_lufs + self.tolerance_lu
    }

    /// Calculate required gain to reach target.
    pub fn required_gain_db(&self, measured_lufs: f64) -> f64 {
        self.target_lufs - measured_lufs
    }
}

/// Preset normalization targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TargetPreset {
    /// EBU R128: -23 LUFS, -1 dBTP (European broadcast).
    EbuR128,

    /// ATSC A/85: -24 LKFS, -2 dBTP (US broadcast).
    AtscA85,

    /// Spotify: -14 LUFS, -1 dBTP.
    Spotify,

    /// YouTube: -14 LUFS, -1 dBTP.
    YouTube,

    /// Apple Music: -16 LUFS, -1 dBTP.
    AppleMusic,

    /// Tidal: -14 LUFS, -1 dBTP.
    Tidal,

    /// Deezer: -15 LUFS, -1 dBTP.
    Deezer,

    /// Amazon Music: -14 LUFS, -2 dBTP.
    AmazonMusic,

    /// Netflix (drama): -27 LUFS, -2 dBTP.
    NetflixDrama,

    /// Netflix (loud content): -24 LUFS, -2 dBTP.
    NetflixLoud,

    /// Amazon Prime Video: -24 LUFS, -2 dBTP.
    AmazonPrime,

    /// BBC iPlayer: -23 LUFS, -1 dBTP.
    BbcIPlayer,

    /// iTunes/Apple Podcasts: -16 LUFS, -1 dBTP.
    Podcast,

    /// Mastering for CD: -9 LUFS, -0.1 dBTP.
    CdMastering,

    /// Mastering for streaming: -14 LUFS, -1 dBTP.
    StreamingMastering,

    /// ReplayGain reference: 89 dB SPL ≈ -18 LUFS, -1 dBTP.
    ReplayGain,

    /// TikTok short-form video delivery: -14 LUFS ±1 LU, -1 dBTP.
    TikTok,
}

impl TargetPreset {
    /// Get the target loudness in LUFS.
    pub fn target_lufs(&self) -> f64 {
        match self {
            Self::EbuR128 => -23.0,
            Self::AtscA85 => -24.0,
            Self::Spotify | Self::YouTube | Self::Tidal | Self::AmazonMusic | Self::TikTok => -14.0,
            Self::AppleMusic | Self::Podcast => -16.0,
            Self::Deezer => -15.0,
            Self::NetflixDrama => -27.0,
            Self::NetflixLoud => -24.0,
            Self::AmazonPrime => -24.0,
            Self::BbcIPlayer => -23.0,
            Self::CdMastering => -9.0,
            Self::StreamingMastering => -14.0,
            Self::ReplayGain => -18.0,
        }
    }

    /// Get the maximum true peak in dBTP.
    pub fn max_peak_dbtp(&self) -> f64 {
        match self {
            Self::EbuR128
            | Self::Spotify
            | Self::YouTube
            | Self::Tidal
            | Self::Deezer
            | Self::AppleMusic
            | Self::BbcIPlayer
            | Self::Podcast
            | Self::ReplayGain
            | Self::TikTok => -1.0,
            Self::AtscA85
            | Self::NetflixDrama
            | Self::NetflixLoud
            | Self::AmazonPrime
            | Self::AmazonMusic => -2.0,
            Self::CdMastering => -0.1,
            Self::StreamingMastering => -1.0,
        }
    }

    /// Get the tolerance in LU.
    pub fn tolerance_lu(&self) -> f64 {
        match self {
            Self::EbuR128
            | Self::BbcIPlayer
            | Self::Spotify
            | Self::YouTube
            | Self::Tidal
            | Self::Deezer
            | Self::AppleMusic
            | Self::Podcast
            | Self::ReplayGain
            | Self::TikTok => 1.0,
            Self::AtscA85 | Self::NetflixDrama | Self::NetflixLoud | Self::AmazonPrime => 2.0,
            Self::AmazonMusic => 1.0,
            Self::CdMastering | Self::StreamingMastering => 0.5,
        }
    }

    /// Get the preset name.
    pub fn name(&self) -> &str {
        match self {
            Self::EbuR128 => "EBU R128",
            Self::AtscA85 => "ATSC A/85",
            Self::Spotify => "Spotify",
            Self::YouTube => "YouTube",
            Self::AppleMusic => "Apple Music",
            Self::Tidal => "Tidal",
            Self::Deezer => "Deezer",
            Self::AmazonMusic => "Amazon Music",
            Self::NetflixDrama => "Netflix (Drama)",
            Self::NetflixLoud => "Netflix (Loud Content)",
            Self::AmazonPrime => "Amazon Prime Video",
            Self::BbcIPlayer => "BBC iPlayer",
            Self::Podcast => "Podcast",
            Self::CdMastering => "CD Mastering",
            Self::StreamingMastering => "Streaming Mastering",
            Self::ReplayGain => "ReplayGain",
            Self::TikTok => "TikTok",
        }
    }

    /// Get whether to apply limiting by default.
    pub fn default_apply_limiting(&self) -> bool {
        !matches!(self, Self::CdMastering | Self::StreamingMastering)
    }

    /// Get whether to apply DRC by default.
    pub fn default_apply_drc(&self) -> bool {
        matches!(self, Self::EbuR128 | Self::AtscA85 | Self::BbcIPlayer)
    }

    /// Convert to a `NormalizationTarget`.
    pub fn to_target(&self) -> NormalizationTarget {
        NormalizationTarget {
            target_lufs: self.target_lufs(),
            max_peak_dbtp: self.max_peak_dbtp(),
            tolerance_lu: self.tolerance_lu(),
            name: self.name().to_string(),
            apply_limiting: self.default_apply_limiting(),
            apply_drc: self.default_apply_drc(),
            preserve_lra: true,
        }
    }

    /// Convert to a metering `Standard`.
    pub fn to_standard(&self) -> Standard {
        match self {
            Self::EbuR128 | Self::BbcIPlayer => Standard::EbuR128,
            Self::AtscA85 => Standard::AtscA85,
            Self::Spotify => Standard::Spotify,
            Self::YouTube => Standard::YouTube,
            Self::AppleMusic | Self::Podcast => Standard::AppleMusic,
            Self::NetflixDrama | Self::NetflixLoud => Standard::Netflix,
            Self::AmazonPrime => Standard::AmazonPrime,
            Self::TikTok => Standard::TikTok,
            _ => Standard::Custom {
                target_lufs: self.target_lufs(),
                max_peak_dbtp: self.max_peak_dbtp(),
                tolerance_lu: self.tolerance_lu(),
            },
        }
    }

    /// Get all available presets.
    pub fn all() -> Vec<Self> {
        vec![
            Self::EbuR128,
            Self::AtscA85,
            Self::Spotify,
            Self::YouTube,
            Self::AppleMusic,
            Self::Tidal,
            Self::Deezer,
            Self::AmazonMusic,
            Self::NetflixDrama,
            Self::NetflixLoud,
            Self::AmazonPrime,
            Self::BbcIPlayer,
            Self::Podcast,
            Self::CdMastering,
            Self::StreamingMastering,
            Self::ReplayGain,
            Self::TikTok,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_creation() {
        let target = NormalizationTarget::new(-23.0, -1.0, "Test".to_string());
        assert_eq!(target.target_lufs, -23.0);
        assert_eq!(target.max_peak_dbtp, -1.0);
    }

    #[test]
    fn test_target_from_standard() {
        let target = NormalizationTarget::from_standard(Standard::EbuR128);
        assert_eq!(target.target_lufs, -23.0);
        assert_eq!(target.max_peak_dbtp, -1.0);
    }

    #[test]
    fn test_compliance_check() {
        let target = NormalizationTarget::new(-23.0, -1.0, "Test".to_string());
        assert!(target.is_compliant(-23.0));
        assert!(target.is_compliant(-22.0));
        assert!(target.is_compliant(-24.0));
        assert!(!target.is_compliant(-20.0));
    }

    #[test]
    fn test_required_gain() {
        let target = NormalizationTarget::new(-23.0, -1.0, "Test".to_string());
        assert_eq!(target.required_gain_db(-20.0), -3.0);
        assert_eq!(target.required_gain_db(-26.0), 3.0);
    }

    #[test]
    fn test_preset_values() {
        assert_eq!(TargetPreset::EbuR128.target_lufs(), -23.0);
        assert_eq!(TargetPreset::EbuR128.max_peak_dbtp(), -1.0);
        assert_eq!(TargetPreset::Spotify.target_lufs(), -14.0);
        assert_eq!(TargetPreset::NetflixDrama.target_lufs(), -27.0);
    }

    #[test]
    fn test_preset_to_target() {
        let target = TargetPreset::EbuR128.to_target();
        assert_eq!(target.target_lufs, -23.0);
        assert_eq!(target.name, "EBU R128");
    }

    #[test]
    fn test_all_presets() {
        let presets = TargetPreset::all();
        assert!(!presets.is_empty());
        assert!(presets.contains(&TargetPreset::EbuR128));
        assert!(presets.contains(&TargetPreset::Spotify));
        assert!(presets.contains(&TargetPreset::TikTok));
    }

    /// TikTok delivery target: -14 LUFS integrated, -1 dBTP ceiling, ±1 LU.
    #[test]
    fn test_tiktok_preset() {
        let preset = TargetPreset::TikTok;
        assert_eq!(preset.target_lufs(), -14.0);
        assert_eq!(preset.max_peak_dbtp(), -1.0);
        assert_eq!(preset.tolerance_lu(), 1.0);
        assert_eq!(preset.name(), "TikTok");

        let target = preset.to_target();
        assert_eq!(target.target_lufs, -14.0);
        assert_eq!(target.max_peak_dbtp, -1.0);
        assert_eq!(target.tolerance_lu, 1.0);
        assert_eq!(target.name, "TikTok");
        assert!(target.is_compliant(-14.5));
        assert!(!target.is_compliant(-12.0));

        // The metering standard must carry the same numbers.
        let standard = preset.to_standard();
        assert_eq!(standard, Standard::TikTok);
        assert_eq!(standard.target_lufs(), -14.0);
        assert_eq!(standard.max_true_peak_dbtp(), -1.0);
        assert_eq!(standard.tolerance_lu(), 1.0);
    }
}
