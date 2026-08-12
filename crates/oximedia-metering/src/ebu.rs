//! EBU R128 loudness normalization standard.
//!
//! Implements the European Broadcasting Union's R128 recommendation for
//! loudness normalization and permitted maximum level of audio signals.
//!
//! # Standards
//!
//! - EBU R 128 (2020): Loudness normalisation and permitted maximum level of audio signals
//! - EBU Tech 3341: Loudness Metering: 'EBU Mode' metering to supplement EBU R 128
//! - EBU Tech 3342: Loudness Range: A measure to supplement EBU R 128 loudness normalisation
//!
//! # Target Levels
//!
//! - Programme Loudness: -23.0 LUFS ±1.0 LU
//! - Maximum True Peak: -1.0 dBTP
//! - Loudness Range: No strict requirement, but typically 5-20 LU

use crate::{LoudnessMeter, LoudnessMetrics, MeterConfig, Standard};

/// EBU R128 target loudness in LUFS.
pub const EBU_TARGET_LUFS: f64 = -23.0;

/// EBU R128 tolerance in LU.
pub const EBU_TOLERANCE_LU: f64 = 1.0;

/// EBU R128 maximum true peak in dBTP.
pub const EBU_MAX_TRUEPEAK_DBTP: f64 = -1.0;

/// Recommended minimum LRA for broadcast content.
pub const EBU_LRA_MIN: f64 = 1.0;

/// Recommended maximum LRA for broadcast content.
pub const EBU_LRA_MAX: f64 = 30.0;

/// EBU R128 program type classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramType {
    /// General broadcast programs.
    General,
    /// Drama and narrative content.
    Drama,
    /// Documentary programs.
    Documentary,
    /// Sports broadcasting.
    Sports,
    /// Music programs.
    Music,
    /// News and current affairs.
    News,
    /// Commercial advertisements.
    Commercial,
}

impl ProgramType {
    /// Get the program type name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::General => "General Broadcast",
            Self::Drama => "Drama",
            Self::Documentary => "Documentary",
            Self::Sports => "Sports",
            Self::Music => "Music",
            Self::News => "News",
            Self::Commercial => "Commercial",
        }
    }

    /// Get typical LRA range for this program type.
    pub fn typical_lra_range(&self) -> (f64, f64) {
        match self {
            Self::General => (5.0, 15.0),
            Self::Drama => (8.0, 18.0),
            Self::Documentary => (6.0, 16.0),
            Self::Sports => (4.0, 12.0),
            Self::Music => (3.0, 10.0),
            Self::News => (3.0, 8.0),
            Self::Commercial => (2.0, 6.0),
        }
    }
}

/// EBU R128 compliance status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComplianceStatus {
    /// Fully compliant with EBU R128.
    Compliant,
    /// Loudness too high (exceeds +1 LU tolerance).
    TooLoud,
    /// Loudness too quiet (exceeds -1 LU tolerance).
    TooQuiet,
    /// True peak exceeds -1.0 dBTP.
    PeakExceeded,
    /// Multiple non-compliances.
    Multiple,
    /// Insufficient data to determine compliance.
    Unknown,
}

impl ComplianceStatus {
    /// Check if compliant.
    pub fn is_compliant(&self) -> bool {
        matches!(self, Self::Compliant)
    }

    /// Get status description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Compliant => "Compliant with EBU R128",
            Self::TooLoud => "Programme loudness exceeds +1 LU tolerance",
            Self::TooQuiet => "Programme loudness exceeds -1 LU tolerance",
            Self::PeakExceeded => "True peak exceeds -1.0 dBTP",
            Self::Multiple => "Multiple compliance issues",
            Self::Unknown => "Insufficient data for compliance check",
        }
    }
}

/// EBU R128 compliance result.
#[derive(Clone, Debug)]
pub struct EbuR128Compliance {
    /// Compliance status.
    pub status: ComplianceStatus,
    /// Measured integrated loudness in LUFS.
    pub integrated_lufs: f64,
    /// Measured true peak in dBTP.
    pub true_peak_dbtp: f64,
    /// Measured loudness range in LU.
    pub loudness_range: f64,
    /// Deviation from target in LU.
    pub deviation_lu: f64,
    /// Is loudness within tolerance?
    pub loudness_ok: bool,
    /// Is true peak within limit?
    pub peak_ok: bool,
    /// Is LRA reasonable?
    pub lra_ok: bool,
}

impl EbuR128Compliance {
    /// Create a compliance result from measurements.
    pub fn from_metrics(metrics: &LoudnessMetrics) -> Self {
        let integrated = metrics.integrated_lufs;
        let peak = metrics.true_peak_dbtp;
        let lra = metrics.loudness_range;

        let loudness_ok = if integrated.is_finite() {
            (EBU_TARGET_LUFS - EBU_TOLERANCE_LU..=EBU_TARGET_LUFS + EBU_TOLERANCE_LU)
                .contains(&integrated)
        } else {
            false
        };

        let peak_ok = peak <= EBU_MAX_TRUEPEAK_DBTP;
        let lra_ok = (EBU_LRA_MIN..=EBU_LRA_MAX).contains(&lra);

        let deviation = if integrated.is_finite() {
            integrated - EBU_TARGET_LUFS
        } else {
            0.0
        };

        let status = if !integrated.is_finite() {
            ComplianceStatus::Unknown
        } else if loudness_ok && peak_ok {
            ComplianceStatus::Compliant
        } else if !loudness_ok && !peak_ok {
            ComplianceStatus::Multiple
        } else if !peak_ok {
            ComplianceStatus::PeakExceeded
        } else if deviation > EBU_TOLERANCE_LU {
            ComplianceStatus::TooLoud
        } else {
            ComplianceStatus::TooQuiet
        };

        Self {
            status,
            integrated_lufs: integrated,
            true_peak_dbtp: peak,
            loudness_range: lra,
            deviation_lu: deviation,
            loudness_ok,
            peak_ok,
            lra_ok,
        }
    }

    /// Get recommended gain adjustment to achieve compliance.
    ///
    /// Returns gain in dB (positive = increase, negative = decrease).
    pub fn recommended_gain(&self) -> f64 {
        if self.integrated_lufs.is_finite() {
            EBU_TARGET_LUFS - self.integrated_lufs
        } else {
            0.0
        }
    }

    /// Check if gain adjustment would cause clipping.
    ///
    /// # Arguments
    ///
    /// * `gain_db` - Proposed gain adjustment in dB
    ///
    /// # Returns
    ///
    /// `true` if adjustment would cause true peak to exceed -1 dBTP
    pub fn would_clip(&self, gain_db: f64) -> bool {
        let adjusted_peak = self.true_peak_dbtp + gain_db;
        adjusted_peak > EBU_MAX_TRUEPEAK_DBTP
    }

    /// Get safe gain adjustment that won't cause clipping.
    pub fn safe_gain(&self) -> f64 {
        let desired_gain = self.recommended_gain();
        let max_safe_gain = EBU_MAX_TRUEPEAK_DBTP - self.true_peak_dbtp;

        desired_gain.min(max_safe_gain)
    }
}

/// EBU R128 meter with program type awareness.
///
/// # Accuracy
///
/// This meter is an **approximation** layered on the block-based
/// [`crate::gating`] / [`crate::lkfs`] pipeline via [`LoudnessMeter`]. It adds
/// program-type awareness (typical LRA ranges) on top, but it is *not*
/// validated against the EBU Tech 3342 reference signals and its gating does
/// not implement the ITU-R BS.1771 two-stage algorithm exactly.
///
/// For measurements that must be standards-accurate — integrated loudness,
/// LRA, or true peak that will be reported or used to drive normalisation —
/// use [`crate::ebu_r128_impl::EbuR128Meter`], which is what the crate root
/// re-exports as `oximedia_metering::EbuR128Meter`.
pub struct EbuR128Meter {
    meter: LoudnessMeter,
    program_type: ProgramType,
}

impl EbuR128Meter {
    /// Create a new EBU R128 meter.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of audio channels
    /// * `program_type` - Type of program being measured
    pub fn new(
        sample_rate: f64,
        channels: usize,
        program_type: ProgramType,
    ) -> crate::MeteringResult<Self> {
        let config = MeterConfig::new(Standard::EbuR128, sample_rate, channels);
        let meter = LoudnessMeter::new(config)?;

        Ok(Self {
            meter,
            program_type,
        })
    }

    /// Process f32 audio samples.
    pub fn process_f32(&mut self, samples: &[f32]) {
        self.meter.process_f32(samples);
    }

    /// Process f64 audio samples.
    pub fn process_f64(&mut self, samples: &[f64]) {
        self.meter.process_f64(samples);
    }

    /// Get current loudness metrics.
    pub fn metrics(&mut self) -> LoudnessMetrics {
        self.meter.metrics()
    }

    /// Check EBU R128 compliance.
    pub fn check_compliance(&mut self) -> EbuR128Compliance {
        let metrics = self.metrics();
        EbuR128Compliance::from_metrics(&metrics)
    }

    /// Check if LRA is typical for the program type.
    pub fn is_lra_typical(&mut self) -> bool {
        let metrics = self.metrics();
        let (min, max) = self.program_type.typical_lra_range();
        metrics.loudness_range >= min && metrics.loudness_range <= max
    }

    /// Get program type.
    pub fn program_type(&self) -> ProgramType {
        self.program_type
    }

    /// Set program type.
    pub fn set_program_type(&mut self, program_type: ProgramType) {
        self.program_type = program_type;
    }

    /// Reset the meter.
    pub fn reset(&mut self) {
        self.meter.reset();
    }

    /// Get the underlying meter.
    pub fn meter(&self) -> &LoudnessMeter {
        &self.meter
    }

    /// Get mutable reference to underlying meter.
    pub fn meter_mut(&mut self) -> &mut LoudnessMeter {
        &mut self.meter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ebu_constants() {
        assert_eq!(EBU_TARGET_LUFS, -23.0);
        assert_eq!(EBU_TOLERANCE_LU, 1.0);
        assert_eq!(EBU_MAX_TRUEPEAK_DBTP, -1.0);
    }

    #[test]
    fn test_program_type_names() {
        assert_eq!(ProgramType::General.name(), "General Broadcast");
        assert_eq!(ProgramType::Drama.name(), "Drama");
    }

    #[test]
    fn test_program_type_lra_ranges() {
        let (min, max) = ProgramType::News.typical_lra_range();
        assert!(min < max);
        assert!(min >= 0.0);
    }

    #[test]
    fn test_compliance_status_descriptions() {
        assert!(!ComplianceStatus::TooLoud.is_compliant());
        assert!(ComplianceStatus::Compliant.is_compliant());
    }

    #[test]
    fn test_compliance_from_metrics() {
        let metrics = LoudnessMetrics {
            integrated_lufs: -23.0,
            true_peak_dbtp: -2.0,
            loudness_range: 10.0,
            ..Default::default()
        };

        let compliance = EbuR128Compliance::from_metrics(&metrics);
        assert!(compliance.status.is_compliant());
        assert!(compliance.loudness_ok);
        assert!(compliance.peak_ok);
    }

    #[test]
    fn test_compliance_too_loud() {
        let metrics = LoudnessMetrics {
            integrated_lufs: -20.0,
            true_peak_dbtp: -2.0,
            loudness_range: 10.0,
            ..Default::default()
        };

        let compliance = EbuR128Compliance::from_metrics(&metrics);
        assert_eq!(compliance.status, ComplianceStatus::TooLoud);
    }

    #[test]
    fn test_compliance_peak_exceeded() {
        let metrics = LoudnessMetrics {
            integrated_lufs: -23.0,
            true_peak_dbtp: 0.5,
            loudness_range: 10.0,
            ..Default::default()
        };

        let compliance = EbuR128Compliance::from_metrics(&metrics);
        assert_eq!(compliance.status, ComplianceStatus::PeakExceeded);
    }

    #[test]
    fn test_recommended_gain() {
        let metrics = LoudnessMetrics {
            integrated_lufs: -20.0,
            true_peak_dbtp: -2.0,
            loudness_range: 10.0,
            ..Default::default()
        };

        let compliance = EbuR128Compliance::from_metrics(&metrics);
        assert_eq!(compliance.recommended_gain(), -3.0);
    }

    #[test]
    fn test_safe_gain() {
        let metrics = LoudnessMetrics {
            integrated_lufs: -30.0,
            true_peak_dbtp: -2.0,
            loudness_range: 10.0,
            ..Default::default()
        };

        let compliance = EbuR128Compliance::from_metrics(&metrics);
        let safe = compliance.safe_gain();

        // Safe gain should not cause peak to exceed -1 dBTP
        assert!(compliance.true_peak_dbtp + safe <= EBU_MAX_TRUEPEAK_DBTP);
    }

    #[test]
    fn test_would_clip() {
        let metrics = LoudnessMetrics {
            integrated_lufs: -25.0,
            true_peak_dbtp: -1.5,
            loudness_range: 10.0,
            ..Default::default()
        };

        let compliance = EbuR128Compliance::from_metrics(&metrics);
        assert!(compliance.would_clip(1.0)); // -1.5 + 1.0 = -0.5 > -1.0
        assert!(!compliance.would_clip(0.4)); // -1.5 + 0.4 = -1.1 < -1.0
    }
}
