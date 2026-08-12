//! Professional audio metering suite.
//!
//! This module provides comprehensive audio metering tools for professional audio applications,
//! including broadcast, mixing, and mastering workflows.
//!
//! # Features
//!
//! ## Level Meters
//!
//! - **VU Meter** (IEC 60268-10) - Classic average level meter with 300ms ballistics
//! - **PPM** (Peak Programme Meter) - Multiple standards (BBC, EBU, Nordic, DIN)
//! - **Digital Peak Meter** - Sample-accurate peak detection in dBFS
//! - **RMS Level Meter** - Root mean square level measurement
//! - **LUFS Meter** - Integrated with existing loudness module (EBU R128)
//! - **Batch Meter** ([`batch::BatchMeterProcessor`]) - peak / RMS / true-peak
//!   for many channels in a single allocation-free pass (mixing-console style)
//!
//! ## Frequency Meters
//!
//! - **Spectrum Analyzer** - Integrated with existing spectrum module
//! - **Correlation Meter** - Stereo phase correlation measurement
//! - **Goniometer** - Stereo field visualization (L/R and M/S modes)
//!
//! ## Ballistics
//!
//! Proper time-domain response characteristics:
//! - Integration time constants (300ms VU, 10ms BBC PPM, etc.)
//! - Attack/release envelopes
//! - Peak hold with configurable duration
//! - Decay rates and return-to-zero behavior
//! - Overload detection with hysteresis
//!
//! ## Standards Compliance
//!
//! - **BS.6840** - BBC PPM specification
//! - **IEC 60268-10** - VU and PPM international standard
//! - **EBU R128** - Loudness normalization (via loudness module)
//! - **SMPTE RP 0155** - Reference levels for audio
//!
//! ## Visualization
//!
//! All meters provide visualization data:
//! - Normalized values (0.0 to 1.0) for bar graphs
//! - Peak markers and hold indicators
//! - Scale markings and labels
//! - Color zones (green/yellow/red)
//! - History buffers for time-domain plots
//!
//! # Examples
//!
//! ## VU Meter
//!
//! ```rust,no_run
//! use oximedia_audio::meters::{VuMeter};
//! use oximedia_audio::AudioFrame;
//!
//! let mut vu = VuMeter::new(48000.0, 2);
//!
//! # let frame = AudioFrame::new(
//! #     oximedia_core::SampleFormat::F32,
//! #     48000,
//! #     oximedia_audio::ChannelLayout::Stereo
//! # );
//! vu.process(&frame);
//!
//! println!("Left: {:.1} dBVU", vu.vu_reading(0));
//! println!("Right: {:.1} dBVU", vu.vu_reading(1));
//! ```
//!
//! ## PPM Meter
//!
//! ```rust,no_run
//! use oximedia_audio::meters::{PpmMeter, PpmStandard};
//!
//! let mut ppm = PpmMeter::new(PpmStandard::Bbc, 48000.0, 2);
//!
//! # let frame = oximedia_audio::AudioFrame::new(
//! #     oximedia_core::SampleFormat::F32,
//! #     48000,
//! #     oximedia_audio::ChannelLayout::Stereo
//! # );
//! ppm.process(&frame);
//!
//! println!("PPM reading: {:.1}", ppm.ppm_units(0));
//! ```
//!
//! ## Unified Meter Bridge
//!
//! ```rust,no_run
//! use oximedia_audio::meters::{MeterBridge, MeterBridgeConfig};
//!
//! let config = MeterBridgeConfig::default();
//! let mut bridge = MeterBridge::new(config, 48000.0, 2);
//!
//! # let frame = oximedia_audio::AudioFrame::new(
//! #     oximedia_core::SampleFormat::F32,
//! #     48000,
//! #     oximedia_audio::ChannelLayout::Stereo
//! # );
//! let readings = bridge.process(&frame);
//!
//! println!("Peak: {:.1} dBFS", readings.peak_dbfs[0]);
//! println!("RMS: {:.1} dBFS", readings.rms_dbfs[0]);
//! println!("VU: {:.1} dBVU", readings.vu_dbvu[0]);
//! println!("Correlation: {:.2}", readings.correlation);
//! ```
//!
//! ## Correlation Meter
//!
//! ```rust,no_run
//! use oximedia_audio::meters::CorrelationMeter;
//!
//! let mut correlation = CorrelationMeter::new(48000.0, 0.4);
//!
//! # let frame = oximedia_audio::AudioFrame::new(
//! #     oximedia_core::SampleFormat::F32,
//! #     48000,
//! #     oximedia_audio::ChannelLayout::Stereo
//! # );
//! correlation.process(&frame);
//!
//! let coeff = correlation.correlation();
//! if coeff > 0.95 {
//!     println!("Warning: Signal is essentially mono");
//! } else if coeff < -0.5 {
//!     println!("Warning: Phase issues detected");
//! }
//! ```
//!
//! # Files in this directory that are deliberately not modules
//!
//! `meters/itu.rs` and `meters/dolby.rs` exist on disk but are **not** declared
//! here, so they are not compiled. This is intentional, not an oversight:
//!
//! - **`itu.rs`** would add a *third* ITU-R BS.1770-4 loudness meter alongside
//!   [`crate::loudness`] and `oximedia_metering::ebu_r128_impl`, and its
//!   `Bs1770Meter` computes ITU channel weights but never applies them (5.1
//!   LFE would be metered at weight 1.0 instead of 0.0), while its
//!   `true_peak_dbtp` is a plain sample peak with no oversampling. It also has
//!   two `E0502` borrow errors and no tests.
//! - **`dolby.rs`** is a sketch: its A-weighting coefficients are hard-coded
//!   for one unstated sample rate and its `sample_rate` argument is ignored,
//!   its M-weighting is two hand-tuned biquads labelled "simplified", and its
//!   spectral-flatness helper takes the product of every sample in a frame,
//!   which underflows to zero for realistic frame sizes.
//!
//! Both need real work plus conformance tests before they can be exposed. The
//! blocker-by-blocker assessment lives in the "Orphan meter modules" section of
//! this crate's task list.

#![forbid(unsafe_code)]

pub mod ballistics;
/// Multi-lane batch metering: peak / RMS / true-peak for many channels in one
/// pass. See [`batch::BatchMeterProcessor`].
pub mod batch;
pub mod correlation;
pub mod peak;
pub mod ppm;
pub mod vu;

use crate::frame::AudioFrame;

// Re-export main types
pub use ballistics::{
    BallisticsConfig, BallisticsProcessor, OverloadDetector, PeakDetector, RmsWindow,
};
pub use batch::{BatchMeterConfig, BatchMeterProcessor, BatchMeterReading};
pub use correlation::{
    CorrelationMeter, CorrelationVisualization, GonioPoint, Goniometer, GoniometerMode,
    GoniometerVisualization,
};
pub use peak::{
    ColorZone as PeakColorZone, DigitalPeakMeter, PeakVisualization, RmsLevelMeter,
    RmsVisualization,
};
pub use ppm::{ColorZone as PpmColorZone, PpmMeter, PpmStandard, PpmVisualization};
pub use vu::{ColorZone as VuColorZone, VuMeter, VuVisualization};

/// Unified meter bridge configuration.
#[derive(Clone, Debug)]
pub struct MeterBridgeConfig {
    /// Enable VU meter.
    pub enable_vu: bool,
    /// Enable PPM meter.
    pub enable_ppm: bool,
    /// PPM standard to use.
    pub ppm_standard: PpmStandard,
    /// Enable digital peak meter.
    pub enable_peak: bool,
    /// Peak hold time in seconds.
    pub peak_hold_time: f64,
    /// Enable RMS meter.
    pub enable_rms: bool,
    /// RMS integration time in seconds.
    pub rms_integration_time: f64,
    /// Enable correlation meter.
    pub enable_correlation: bool,
    /// Correlation integration time in seconds.
    pub correlation_integration_time: f64,
    /// Enable goniometer.
    pub enable_goniometer: bool,
    /// Goniometer max points.
    pub goniometer_max_points: usize,
    /// Enable LUFS meter (requires loudness module).
    pub enable_lufs: bool,
}

impl MeterBridgeConfig {
    /// Create a minimal configuration (peak meter only).
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            enable_vu: false,
            enable_ppm: false,
            ppm_standard: PpmStandard::Ebu,
            enable_peak: true,
            peak_hold_time: 2.0,
            enable_rms: false,
            rms_integration_time: 0.3,
            enable_correlation: false,
            correlation_integration_time: 0.4,
            enable_goniometer: false,
            goniometer_max_points: 1000,
            enable_lufs: false,
        }
    }

    /// Create a standard configuration (peak, RMS, VU).
    #[must_use]
    pub fn standard() -> Self {
        Self {
            enable_vu: true,
            enable_ppm: false,
            ppm_standard: PpmStandard::Ebu,
            enable_peak: true,
            peak_hold_time: 2.0,
            enable_rms: true,
            rms_integration_time: 0.3,
            enable_correlation: false,
            correlation_integration_time: 0.4,
            enable_goniometer: false,
            goniometer_max_points: 1000,
            enable_lufs: false,
        }
    }

    /// Create a professional configuration (all meters enabled).
    #[must_use]
    pub fn professional() -> Self {
        Self {
            enable_vu: true,
            enable_ppm: true,
            ppm_standard: PpmStandard::Ebu,
            enable_peak: true,
            peak_hold_time: 2.0,
            enable_rms: true,
            rms_integration_time: 0.3,
            enable_correlation: true,
            correlation_integration_time: 0.4,
            enable_goniometer: true,
            goniometer_max_points: 2000,
            enable_lufs: true,
        }
    }

    /// Create a broadcast configuration (PPM, peak, LUFS, correlation).
    #[must_use]
    pub fn broadcast() -> Self {
        Self {
            enable_vu: false,
            enable_ppm: true,
            ppm_standard: PpmStandard::Ebu,
            enable_peak: true,
            peak_hold_time: 1.0,
            enable_rms: false,
            rms_integration_time: 0.3,
            enable_correlation: true,
            correlation_integration_time: 0.4,
            enable_goniometer: true,
            goniometer_max_points: 1500,
            enable_lufs: true,
        }
    }
}

impl Default for MeterBridgeConfig {
    fn default() -> Self {
        Self::standard()
    }
}

/// Unified meter bridge.
///
/// Provides a single interface for processing audio through multiple meter types.
pub struct MeterBridge {
    /// Configuration.
    config: MeterBridgeConfig,
    /// VU meter (optional).
    vu_meter: Option<VuMeter>,
    /// PPM meter (optional).
    ppm_meter: Option<PpmMeter>,
    /// Digital peak meter (optional).
    peak_meter: Option<DigitalPeakMeter>,
    /// RMS level meter (optional).
    rms_meter: Option<RmsLevelMeter>,
    /// Correlation meter (optional).
    correlation_meter: Option<CorrelationMeter>,
    /// Goniometer (optional).
    goniometer: Option<Goniometer>,
    /// LUFS meter (optional).
    lufs_meter: Option<crate::loudness::LoudnessMeter>,
    /// Sample rate in Hz.
    #[allow(dead_code)]
    sample_rate: f64,
    /// Number of channels.
    #[allow(dead_code)]
    channels: usize,
}

impl MeterBridge {
    /// Create a new meter bridge.
    ///
    /// # Arguments
    ///
    /// * `config` - Meter bridge configuration
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of audio channels
    #[must_use]
    pub fn new(config: MeterBridgeConfig, sample_rate: f64, channels: usize) -> Self {
        let vu_meter = if config.enable_vu {
            Some(VuMeter::new(sample_rate, channels))
        } else {
            None
        };

        let ppm_meter = if config.enable_ppm {
            Some(PpmMeter::new(config.ppm_standard, sample_rate, channels))
        } else {
            None
        };

        let peak_meter = if config.enable_peak {
            Some(DigitalPeakMeter::new(
                sample_rate,
                channels,
                config.peak_hold_time,
            ))
        } else {
            None
        };

        let rms_meter = if config.enable_rms {
            Some(RmsLevelMeter::new(
                sample_rate,
                channels,
                config.rms_integration_time,
            ))
        } else {
            None
        };

        let correlation_meter = if config.enable_correlation && channels >= 2 {
            Some(CorrelationMeter::new(
                sample_rate,
                config.correlation_integration_time,
            ))
        } else {
            None
        };

        let goniometer = if config.enable_goniometer && channels >= 2 {
            Some(Goniometer::new(
                sample_rate,
                config.goniometer_max_points,
                GoniometerMode::LR,
            ))
        } else {
            None
        };

        let lufs_meter = if config.enable_lufs {
            Some(crate::loudness::LoudnessMeter::new(
                crate::loudness::LoudnessStandard::EbuR128,
                sample_rate,
                channels,
            ))
        } else {
            None
        };

        Self {
            config,
            vu_meter,
            ppm_meter,
            peak_meter,
            rms_meter,
            correlation_meter,
            goniometer,
            lufs_meter,
            sample_rate,
            channels,
        }
    }

    /// Process an audio frame and update all enabled meters.
    ///
    /// # Arguments
    ///
    /// * `frame` - Audio frame to process
    ///
    /// # Returns
    ///
    /// Combined meter readings
    pub fn process(&mut self, frame: &AudioFrame) -> MeterReadings {
        // Process each enabled meter
        if let Some(ref mut meter) = self.vu_meter {
            meter.process(frame);
        }

        if let Some(ref mut meter) = self.ppm_meter {
            meter.process(frame);
        }

        if let Some(ref mut meter) = self.peak_meter {
            meter.process(frame);
        }

        if let Some(ref mut meter) = self.rms_meter {
            meter.process(frame);
        }

        if let Some(ref mut meter) = self.correlation_meter {
            meter.process(frame);
        }

        if let Some(ref mut meter) = self.goniometer {
            meter.process(frame);
        }

        if let Some(ref mut meter) = self.lufs_meter {
            meter.measure(frame);
        }

        // Collect readings
        self.get_readings()
    }

    /// Get current meter readings without processing new audio.
    #[must_use]
    pub fn get_readings(&self) -> MeterReadings {
        let mut peak_dbfs = vec![f64::NEG_INFINITY; self.channels];
        let mut rms_dbfs = vec![f64::NEG_INFINITY; self.channels];
        let mut vu_dbvu = vec![f64::NEG_INFINITY; self.channels];
        let mut ppm_db = vec![f64::NEG_INFINITY; self.channels];

        // Collect peak readings
        if let Some(ref meter) = self.peak_meter {
            for ch in 0..self.channels {
                peak_dbfs[ch] = meter.peak_dbfs(ch);
            }
        }

        // Collect RMS readings
        if let Some(ref meter) = self.rms_meter {
            for ch in 0..self.channels {
                rms_dbfs[ch] = meter.rms_dbfs(ch);
            }
        }

        // Collect VU readings
        if let Some(ref meter) = self.vu_meter {
            for ch in 0..self.channels {
                vu_dbvu[ch] = meter.vu_reading(ch);
            }
        }

        // Collect PPM readings
        if let Some(ref meter) = self.ppm_meter {
            for ch in 0..self.channels {
                ppm_db[ch] = meter.ppm_reading(ch);
            }
        }

        // Collect correlation
        let correlation = self
            .correlation_meter
            .as_ref()
            .map_or(0.0, CorrelationMeter::correlation);

        // Collect LUFS
        let lufs = self
            .lufs_meter
            .as_ref()
            .map(|m| m.get_metrics().integrated_lufs);

        MeterReadings {
            peak_dbfs,
            rms_dbfs,
            vu_dbvu,
            ppm_db,
            correlation,
            lufs,
            has_overload: self.has_overload(),
            has_phase_issues: self
                .correlation_meter
                .as_ref()
                .map_or(false, CorrelationMeter::has_phase_issues),
        }
    }

    /// Check if any channel has overload.
    #[must_use]
    pub fn has_overload(&self) -> bool {
        if let Some(ref meter) = self.peak_meter {
            (0..self.channels).any(|ch| meter.is_overload(ch))
        } else if let Some(ref meter) = self.vu_meter {
            (0..self.channels).any(|ch| meter.is_overload(ch))
        } else if let Some(ref meter) = self.ppm_meter {
            (0..self.channels).any(|ch| meter.is_overload(ch))
        } else {
            false
        }
    }

    /// Reset all meters.
    pub fn reset(&mut self) {
        if let Some(ref mut meter) = self.vu_meter {
            meter.reset();
        }
        if let Some(ref mut meter) = self.ppm_meter {
            meter.reset();
        }
        if let Some(ref mut meter) = self.peak_meter {
            meter.reset();
        }
        if let Some(ref mut meter) = self.rms_meter {
            meter.reset();
        }
        if let Some(ref mut meter) = self.correlation_meter {
            meter.reset();
        }
        if let Some(ref mut meter) = self.goniometer {
            meter.clear();
        }
        if let Some(ref mut meter) = self.lufs_meter {
            meter.reset();
        }
    }

    /// Reset peak holds only.
    pub fn reset_peaks(&mut self) {
        if let Some(ref mut meter) = self.vu_meter {
            meter.reset_peaks();
        }
        if let Some(ref mut meter) = self.ppm_meter {
            meter.reset_peaks();
        }
        if let Some(ref mut meter) = self.peak_meter {
            meter.reset_peak_hold();
        }
    }

    /// Get VU meter reference.
    #[must_use]
    pub fn vu_meter(&self) -> Option<&VuMeter> {
        self.vu_meter.as_ref()
    }

    /// Get PPM meter reference.
    #[must_use]
    pub fn ppm_meter(&self) -> Option<&PpmMeter> {
        self.ppm_meter.as_ref()
    }

    /// Get peak meter reference.
    #[must_use]
    pub fn peak_meter(&self) -> Option<&DigitalPeakMeter> {
        self.peak_meter.as_ref()
    }

    /// Get RMS meter reference.
    #[must_use]
    pub fn rms_meter(&self) -> Option<&RmsLevelMeter> {
        self.rms_meter.as_ref()
    }

    /// Get correlation meter reference.
    #[must_use]
    pub fn correlation_meter(&self) -> Option<&CorrelationMeter> {
        self.correlation_meter.as_ref()
    }

    /// Get goniometer reference.
    #[must_use]
    pub fn goniometer(&self) -> Option<&Goniometer> {
        self.goniometer.as_ref()
    }

    /// Get LUFS meter reference.
    #[must_use]
    pub fn lufs_meter(&self) -> Option<&crate::loudness::LoudnessMeter> {
        self.lufs_meter.as_ref()
    }

    /// Get configuration.
    #[must_use]
    pub fn config(&self) -> &MeterBridgeConfig {
        &self.config
    }
}

/// Combined meter readings.
#[derive(Clone, Debug)]
pub struct MeterReadings {
    /// Peak levels per channel (dBFS).
    pub peak_dbfs: Vec<f64>,
    /// RMS levels per channel (dBFS).
    pub rms_dbfs: Vec<f64>,
    /// VU readings per channel (dBVU).
    pub vu_dbvu: Vec<f64>,
    /// PPM readings per channel (dB or PPM units).
    pub ppm_db: Vec<f64>,
    /// Stereo correlation coefficient.
    pub correlation: f64,
    /// Integrated loudness (LUFS).
    pub lufs: Option<f64>,
    /// Overload indicator.
    pub has_overload: bool,
    /// Phase issues indicator.
    pub has_phase_issues: bool,
}

impl MeterReadings {
    /// Get stereo peak level (max of L/R).
    #[must_use]
    pub fn stereo_peak_dbfs(&self) -> f64 {
        self.peak_dbfs
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// Get stereo RMS level (average of L/R in linear domain).
    #[must_use]
    pub fn stereo_rms_dbfs(&self) -> f64 {
        if self.rms_dbfs.len() >= 2 {
            let left = self.rms_dbfs[0];
            let right = self.rms_dbfs[1];
            if left.is_finite() && right.is_finite() {
                let left_lin = ballistics::db_to_linear(left);
                let right_lin = ballistics::db_to_linear(right);
                ballistics::linear_to_db((left_lin + right_lin) / 2.0)
            } else if left.is_finite() {
                left
            } else {
                right
            }
        } else if let Some(&first) = self.rms_dbfs.first() {
            first
        } else {
            f64::NEG_INFINITY
        }
    }

    /// Check if signal is safe for broadcast.
    #[must_use]
    pub fn is_broadcast_safe(&self) -> bool {
        !self.has_overload
            && !self.has_phase_issues
            && self
                .lufs
                .map_or(true, |l| l.is_finite() && l > -40.0 && l < -10.0)
    }
}
