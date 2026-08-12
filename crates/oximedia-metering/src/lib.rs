//! Professional broadcast audio metering for `OxiMedia`.
//!
//! This crate provides comprehensive, standards-compliant audio loudness measurement
//! and metering for broadcast, streaming, and professional audio applications.
//!
//! # Supported Standards
//!
//! - **ITU-R BS.1770-4** - Algorithms to measure audio programme loudness and true-peak level
//! - **ITU-R BS.1771** - Requirements for loudness and true-peak indicating meters
//! - **EBU R128** - Loudness normalisation and permitted maximum level (European standard)
//! - **ATSC A/85** - Techniques for Establishing and Maintaining Audio Loudness (US standard)
//! - **Dolby Metadata** - Dialogue Intelligence metadata generation (metadata only, respects IP)
//!
//! # Features
//!
//! ## Loudness Measurement
//!
//! - **Momentary Loudness** - 400ms sliding window with 75% overlap
//! - **Short-term Loudness** - 3-second sliding window with 75% overlap
//! - **Integrated Loudness** - Gated program loudness (LKFS/LUFS)
//! - **Loudness Range (LRA)** - Dynamic range measurement using percentile-based method
//!
//! ## True Peak Detection
//!
//! - **4x Oversampling** - Detects inter-sample peaks using sinc interpolation
//! - **Per-channel Tracking** - Individual true peak levels for each channel
//! - **dBTP Conversion** - True peak in dB relative to full scale
//!
//! ## Gating Algorithm
//!
//! - **Absolute Gate** - -70 LKFS threshold
//! - **Relative Gate** - -10 LU below ungated loudness
//! - **Two-stage Process** - ITU-R BS.1771 compliant gating
//!
//! ## Multi-channel Support
//!
//! Supports up to 7.1.4 Dolby Atmos layouts with proper channel weighting:
//! - Mono (1.0)
//! - Stereo (2.0)
//! - 5.1 Surround
//! - 7.1 Surround
//! - 7.1.4 Dolby Atmos (bed channels)
//!
//! ## Compliance Checking
//!
//! - EBU R128 compliance (target: -23 LUFS ±1 LU, peak: -1 dBTP)
//! - ATSC A/85 compliance (target: -24 LKFS ±2 dB, peak: -2 dBTP)
//! - Streaming platform targets (Spotify, `YouTube`, Apple Music, etc.)
//!
//! # Example Usage
//!
//! ## Basic Loudness Metering
//!
//! ```rust,no_run
//! use oximedia_metering::{LoudnessMeter, MeterConfig, Standard};
//!
//! // Create meter for EBU R128
//! let config = MeterConfig::new(Standard::EbuR128, 48000.0, 2);
//! let mut meter = LoudnessMeter::new(config).expect("Failed to create meter");
//!
//! // Process audio samples (interleaved f32)
//! # let audio_samples: &[f32] = &[];
//! meter.process_f32(audio_samples);
//!
//! // Get loudness metrics
//! let metrics = meter.metrics();
//! println!("Integrated: {:.1} LUFS", metrics.integrated_lufs);
//! println!("LRA: {:.1} LU", metrics.loudness_range);
//! println!("True Peak: {:.1} dBTP", metrics.true_peak_dbtp);
//!
//! // Check compliance
//! let compliance = meter.check_compliance();
//! if compliance.is_compliant() {
//!     println!("Audio is compliant with {}", compliance.standard_name());
//! }
//!
//! // Generate detailed report
//! let report = meter.generate_report();
//! println!("{}", report);
//! ```
//!
//! ## Peak Metering
//!
//! ```rust,no_run
//! use oximedia_metering::{PeakMeter, PeakMeterType};
//!
//! // Create a VU meter for stereo audio
//! let mut vu_meter = PeakMeter::new(
//!     PeakMeterType::Vu,
//!     48000.0,
//!     2,
//!     2.0  // 2 second peak hold
//! ).expect("Failed to create VU meter");
//!
//! # let audio_samples: &[f64] = &[];
//! vu_meter.process_interleaved(audio_samples);
//!
//! let peaks = vu_meter.peak_dbfs();
//! println!("L: {:.1} dBFS, R: {:.1} dBFS", peaks[0], peaks[1]);
//!
//! // Create an RMS meter with 300ms integration
//! let mut rms_meter = PeakMeter::new(
//!     PeakMeterType::Rms(0.3),
//!     48000.0,
//!     2,
//!     0.0
//! ).expect("Failed to create RMS meter");
//! ```
//!
//! ## K-System Metering
//!
//! ```rust,no_run
//! use oximedia_metering::{KSystemMeter, KSystemType};
//!
//! // Create K-14 meter (mastering standard)
//! let mut k_meter = KSystemMeter::new(
//!     KSystemType::K14,
//!     48000.0,
//!     2
//! ).expect("Failed to create K-meter");
//!
//! # let audio_samples: &[f64] = &[];
//! k_meter.process_interleaved(audio_samples);
//!
//! // Get levels relative to K-14 reference
//! let rms_levels = k_meter.rms_relative_db();
//! println!("RMS relative to K-14: L={:.1} dB, R={:.1} dB",
//!          rms_levels[0], rms_levels[1]);
//!
//! if k_meter.is_overload() {
//!     println!("Warning: Headroom exceeded!");
//! }
//! ```
//!
//! ## Phase Analysis
//!
//! ```rust,no_run
//! use oximedia_metering::{PhaseCorrelationMeter, StereoWidthAnalyzer};
//!
//! // Create phase correlation meter
//! let mut phase_meter = PhaseCorrelationMeter::new(48000.0, 0.4)
//!     .expect("Failed to create phase meter");
//!
//! # let audio_samples: &[f64] = &[];
//! phase_meter.process_interleaved(audio_samples);
//!
//! let correlation = phase_meter.correlation();
//! println!("Phase correlation: {:.2}", correlation);
//!
//! if phase_meter.has_phase_issues() {
//!     println!("Warning: Phase cancellation detected!");
//! }
//!
//! // Stereo width analysis
//! let mut width_analyzer = StereoWidthAnalyzer::new(48000.0)
//!     .expect("Failed to create width analyzer");
//!
//! width_analyzer.process_interleaved(audio_samples);
//! println!("Stereo width: {:.0}%", width_analyzer.width_percentage());
//! ```
//!
//! ## Spectrum Analysis
//!
//! ```rust,no_run
//! use oximedia_metering::{SpectrumAnalyzer, WindowFunction, WeightingCurve};
//!
//! // Create FFT-based spectrum analyzer
//! let mut spectrum = SpectrumAnalyzer::new(
//!     48000.0,
//!     2048,
//!     WindowFunction::Hann,
//!     WeightingCurve::A,
//!     1.0  // 1 second peak hold
//! ).expect("Failed to create spectrum analyzer");
//!
//! # let audio_samples: &[f64] = &[];
//! spectrum.process(audio_samples);
//!
//! let spectrum_db = spectrum.spectrum_db();
//! for (i, &magnitude) in spectrum_db.iter().take(10).enumerate() {
//!     let freq = spectrum.bin_frequency(i);
//!     println!("{:.0} Hz: {:.1} dB", freq, magnitude);
//! }
//! ```
//!
//! ## Video Metering
//!
//! ```rust,no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use oximedia_metering::{LuminanceMeter, GamutMeter, ColorGamut, QualityAnalyzer, Frame2D};
//!
//! // Luminance metering for HDR content
//! let mut lum_meter = LuminanceMeter::new(1920, 1080, 1000.0, 256)
//!     .expect("Failed to create luminance meter");
//!
//! # let luminance_frame = Frame2D::zeros(1080, 1920);
//! lum_meter.process(&luminance_frame)?;
//!
//! println!("Peak: {:.1} nits", lum_meter.peak_nits());
//! println!("Average: {:.1} nits", lum_meter.average_nits());
//! println!("Dynamic range: {:.1} stops", lum_meter.dynamic_range_stops());
//!
//! if lum_meter.is_hdr10() {
//!     println!("HDR10 content detected");
//! }
//!
//! // Color gamut analysis
//! let mut gamut_meter = GamutMeter::new(1920, 1080, ColorGamut::Rec2020)
//!     .expect("Failed to create gamut meter");
//!
//! # let r_channel = Frame2D::zeros(1080, 1920);
//! # let g_channel = Frame2D::zeros(1080, 1920);
//! # let b_channel = Frame2D::zeros(1080, 1920);
//! gamut_meter.process(&r_channel, &g_channel, &b_channel)?;
//!
//! println!("Rec.2020 coverage: {:.1}%", gamut_meter.gamut_coverage_percentage());
//! println!("Max saturation: {:.2}", gamut_meter.max_saturation());
//!
//! // Video quality metrics (PSNR, SSIM)
//! let quality = QualityAnalyzer::new(1920, 1080, 1.0)
//!     .expect("Failed to create quality analyzer");
//!
//! # let reference_frame = Frame2D::zeros(1080, 1920);
//! # let distorted_frame = Frame2D::zeros(1080, 1920);
//! let metrics = quality.analyze(&reference_frame, &distorted_frame)?;
//!
//! println!("PSNR: {:.2} dB", metrics.psnr);
//! println!("SSIM: {:.4}", metrics.ssim);
//! println!("Quality: {}", metrics.rating());
//! # Ok(())
//! # }
//! ```
//!
//! ## Meter Rendering
//!
//! ```rust,no_run
//! use oximedia_metering::{BarMeterConfig, BarMeterData, ColorGradient, Orientation};
//!
//! // Configure a vertical bar meter
//! let config = BarMeterConfig {
//!     orientation: Orientation::Vertical,
//!     width: 30,
//!     height: 200,
//!     min_value: -60.0,
//!     max_value: 0.0,
//!     gradient: ColorGradient::traffic_light(),
//!     show_peak_hold: true,
//!     show_scale: true,
//!     ..Default::default()
//! };
//!
//! // Create meter data from dBFS values
//! let meter_data = BarMeterData::from_dbfs(
//!     -12.0,  // Current level
//!     -6.0,   // Peak hold
//!     -60.0,  // Min range
//!     0.0     // Max range
//! );
//!
//! if meter_data.is_clipping {
//!     println!("Clipping detected!");
//! }
//!
//! // Get color for current level
//! let color = config.gradient.color_at(meter_data.level);
//! println!("Meter color: RGB({}, {}, {})", color.r, color.g, color.b);
//! ```
//!
//! # Technical Implementation
//!
//! ## K-weighting Filter
//!
//! The K-weighting filter chain implements ITU-R BS.1770-4 specification:
//! - **Stage 1**: High-pass filter at 78.5 Hz (head diffraction modeling)
//! - **Stage 2**: High-shelf filter for revised low-frequency B-weighting (RLB)
//!
//! Both filters are implemented as second-order IIR biquad filters with
//! precise coefficients calculated for the given sample rate.
//!
//! ## Block Processing
//!
//! Audio is processed in overlapping blocks:
//! - **Block size**: 100ms (400ms blocks for momentary, 3000ms for short-term)
//! - **Overlap**: 75% (blocks advance by 25% of their duration)
//! - **Gating**: Applied on 400ms blocks with absolute (-70 LKFS) and relative (-10 LU) gates
//!
//! ## True Peak Detection
//!
//! Uses 4x oversampling with windowed sinc interpolation:
//! - Lanczos-windowed sinc function (a=3)
//! - Linear-phase FIR resampling
//! - Per-sample peak tracking
//!
//! # Performance
//!
//! - **Real-time capable**: Processes audio faster than real-time on modern CPUs
//! - **Memory efficient**: Circular buffers for sliding windows
//! - **Zero-copy where possible**: Processes interleaved or planar audio in-place when possible
//!
//! # Standards References
//!
//! - ITU-R BS.1770-4 (10/2015): "Algorithms to measure audio programme loudness and true-peak audio level"
//! - ITU-R BS.1771 (2006): "Requirements for loudness and true-peak indicating meters"
//! - EBU R 128 (2020): "Loudness normalisation and permitted maximum level of audio signals"
//! - ATSC A/85:2013: "Techniques for Establishing and Maintaining Audio Loudness for Digital Television"
//!
//! # Pipeline Architecture
//!
//! The core loudness pipeline lives in [`ebu_r128_impl::EbuR128Meter`], which this crate
//! root re-exports as [`EbuR128Meter`]. ([`LoudnessMeter`] is a *separate*, block-based
//! pipeline over [`lkfs`] / [`gating`] / [`filters`]; it shares the K-weighting design but
//! not the gating implementation, and it is the meter behind the approximate
//! [`ebu::EbuR128Meter`].) Every sample flows through four stages —
//! **filter → LKFS → gating → LRA**:
//!
//! ```text
//!  interleaved samples                K-weighting filter (per channel)
//!  in [-1.0, 1.0]        ───────►     Stage 1: high-shelf (head effects, ITU-R BS.1770-4)
//!                                     Stage 2: high-pass   (RLB, f_c ≈ 38 Hz)
//!                                                   │
//!                                                   ▼ squared, channel-weighted (Table 2)
//!                                     100 ms hop accumulator (`complete_hop`)
//!                                                   │
//!                     ┌─────────────────────────────┼─────────────────────────────┐
//!                     ▼                             ▼                             ▼
//!            400 ms momentary window       3 s short-term window        400 ms gating block
//!            (4 hops, updated /100ms)      (30 hops, updated /100ms)    (4 hops, updated /100ms)
//!                     │                             │                             │
//!                     ▼                             ▼                             ▼
//!             Momentary LUFS                 Short-term LUFS               `gating_blocks[]`
//!         (`momentary_lufs`)              (`short_term_lufs`)          (LUFS, power) history
//!                                                                                  │
//!                                          ┌───────────────────────────────────────┴──────────────────────────┐
//!                                          ▼                                                                  ▼
//!                          `integrated_lufs()` — 2-stage gate                        `loudness_range_lu()` — cascaded gate
//!                          (ITU-R BS.1770-4 §3 / BS.1771)                              (EBU Tech 3342 §3.1)
//!                            1. absolute gate  ≥ −70 LUFS                               1. absolute gate ≥ −70 LUFS
//!                            2. relative gate  ≥ abs-mean − 10 LU                       2. relative gate ≥ abs-mean − 20 LU
//!                                          │                                            3. LRA = p95 − p10 of survivors
//!                                          ▼                                                                  ▼
//!                                 Integrated Loudness (LUFS)                               Loudness Range (LU)
//! ```
//!
//! In parallel — not gated, and independent of the K-weighting/LKFS chain above — raw
//! samples also feed one 4×-oversampled windowed-sinc
//! [`ebu_r128_impl::TruePeakDetector`] **per channel** for inter-sample peak detection;
//! `true_peak_dbtp()` reports the maximum across channels and
//! `channel_true_peaks_dbtp()` the individual readings. [`ebu_r128_impl::LoudnessReport::from_meter`]
//! combines Integrated Loudness, Loudness Range, and True Peak into a single compliance
//! report checked against EBU R128, ATSC A/85, and ARIB TR-B32.
//!
//! # Complementary Metering Modules
//!
//! [`clip_counter`], [`rms_envelope`], and [`silence_detect`] are **not** wired into the
//! `EbuR128Meter`/`LoudnessMeter` pipeline above — they are independent meters that a
//! caller runs on the *same* audio stream, in parallel with loudness metering, for
//! additional broadcast QC that BS.1770/EBU R128 loudness measurement does not cover:
//!
//! - [`clip_counter`] — counts and timestamps digital clipping events (samples at or
//!   above a configurable threshold, default 0 dBFS) per channel. This complements True
//!   Peak detection by catching *sustained* full-scale overs rather than the brief
//!   inter-sample peaks the oversampled true-peak detector is designed for.
//! - [`rms_envelope`] — an attack/release RMS envelope follower for level metering and
//!   dynamics visualisation. Unlike the fixed 400 ms/3 s LKFS windows above, its time
//!   constants are freely configurable, making it suited to VU-style ballistics rather
//!   than standards-accurate loudness.
//! - [`silence_detect`] — flags sustained silence/near-silence ("dead air") using a
//!   configurable dBFS threshold and minimum duration, as a discrete *event*. This is
//!   distinct from the −70 LUFS absolute gate used by `integrated_lufs()`/
//!   `loudness_range_lu()`, which silently *excludes* low-level blocks from the
//!   loudness statistics rather than reporting them.
//!
//! None of the three modules share state with `EbuR128Meter`; a typical integration
//! feeds one buffer of samples to a `LoudnessMeter` *and* to whichever of these
//! modules the application also needs.
//!
//! # Compliance Testing Guide (EBU R128 Verification)
//!
//! To verify a build against EBU R128, run the crate's conformance test suite
//! (`cargo test -p oximedia-metering --lib`) and check the following, in order:
//!
//! 1. **Reference-signal test** — `test_ebu_r128_reference_signal` (`src/lib.rs`)
//!    feeds a 997 Hz sine wave calibrated to −23 LUFS through [`LoudnessMeter`] and
//!    asserts the reported Integrated Loudness is within ±0.5 LUFS of the EBU R128
//!    target. This is the canonical "does the meter read the right number" check.
//! 2. **Two-stage gating conformance** — `src/gating.rs` (e.g. `test_gating_constants`,
//!    `test_gated_percentage`) together with the integrated-loudness tests in
//!    `src/ebu_r128_impl.rs` exercise the ITU-R BS.1770-4 §3 absolute gate (−70 LUFS)
//!    and relative gate (−10 LU below the absolute-gated mean).
//! 3. **Loudness Range (LRA) conformance** — `src/ebu_r128_impl.rs`
//!    `test_lra_ebu_tech3342_case1`..`case4` reproduce all four synthetic "minimum
//!    requirements" test signals published in EBU Tech 3342 Table 1 (stereo, in-phase
//!    sine segments at specified per-channel dBFS levels) and assert the measured LRA
//!    is within the published ±1 LU tolerance of the reference value (10, 5, 20, and
//!    15 LU respectively).
//! 4. **True Peak conformance** — `test_tp_detector_full_scale_sine` and
//!    `test_true_peak_detected_above_signal_peak` (`src/ebu_r128_impl.rs`) verify the
//!    4× oversampled true-peak detector reports a finite, sane dBTP for full-scale
//!    input.
//! 5. **Target-signal / compliance check** — `test_ebu_compliance_for_target_signal`
//!    (`src/ebu_r128_impl.rs`) calibrates a sine wave to ≈ −23 LUFS, feeds it through
//!    the meter, and confirms `LoudnessReport::complies_ebu_r128` is `true` — i.e. that
//!    a signal built to land on the target is actually reported as compliant.
//!
//! When adding a new conformance vector, follow the pattern used by the LRA tests
//! (helper `stereo_sine_segments` in `ebu_r128_impl::tests`): build the signal exactly
//! as specified by the publishing standard, run it through `EbuR128Meter`, and assert
//! against the *published* expected value and tolerance — never loosen a tolerance
//! just to make a test pass.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::similar_names)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::unused_self)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::fn_params_excessive_bools)]
#![allow(clippy::let_and_return)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::format_push_string)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::missing_panics_doc)]
#![allow(dead_code)]
#![allow(
    clippy::float_cmp,
    clippy::too_many_lines,
    clippy::return_self_not_must_use
)]

pub mod atsc;
pub mod ballistics;
pub mod bs2051_weights;
pub mod bs2132;
pub mod clip_counter;
pub mod correlation;
pub mod dr_meter;
pub mod dynamics;
pub mod ebu;
pub mod ebu_r128_impl;
pub mod filters;
pub mod gating;
pub mod k_weighting;
pub mod leq;
pub mod lkfs;
pub mod loudness_gate;
pub mod loudness_history;
pub mod m_s_meter;
pub mod meter_type_config;
pub mod ms_ssim;
pub mod octave_bands;
pub mod peak;
pub mod phase;
pub mod phase_scope;
pub mod ppm;
pub mod range;
pub mod render;
pub mod report;
pub mod rms_envelope;
pub mod silence_detect;
pub mod spectral_balance;
pub mod spectral_energy;
pub mod spectrum;
pub mod spectrum_bands;
pub mod true_peak_meter;
pub mod truepeak;
pub mod video_color;
pub mod video_luminance;
pub mod video_quality;
pub mod vmaf_estimate;
pub mod vmaf_features;
pub mod vu_meter;

/// Backward-compatibility aliases for merged modules.
pub use correlation as correlation_meter;
/// Backward-compatibility alias: items from `dynamic_range_meter` now in [`dynamics`].
pub use dynamics as dynamic_range_meter;
/// Backward-compatibility alias: items from `peak_meter` now in [`peak`].
pub use peak as peak_meter;
/// Backward-compatibility alias: items from `phase_analysis` now in [`phase`].
pub use phase as phase_analysis;
/// Backward-compatibility alias: items from `true_peak` now in [`truepeak`].
pub use truepeak as true_peak;

// Wave 12 modules
pub mod crest_factor;
pub mod k_weighted;
pub mod meter_bridge;

// Wave 15 modules
pub mod loudness_trend;
pub mod noise_floor;
pub mod stereo_balance;

// Wave 16 modules
pub mod k_weight_simd;
pub mod temporal_noise;

use oximedia_core::types::SampleFormat;
use thiserror::Error;

pub use atsc::{AtscA85Compliance, AtscA85Meter};
pub use ballistics::{BallisticProcessor, BallisticType, MultiChannelBallistics};
pub use correlation::{
    CorrelationMeter, FrequencyBand, Goniometer as CorrelationGoniometer,
    GoniometerPoint as CorrelationGoniometerPoint, MultibandMeter, PhaseRelationship,
};
pub use dynamics::{DynamicRangeMeter, PlrMeter};
pub use ebu::EbuR128Compliance;
/// The crate-root `EbuR128Meter` is the standards-accurate implementation from
/// [`ebu_r128_impl`] (exact ITU-R BS.1770-4 Table 1 K-weighting, ITU-R BS.1771
/// two-stage gating, EBU Tech 3342 LRA, 4× oversampled per-channel true peak).
///
/// The older program-type-aware wrapper around [`LoudnessMeter`] remains
/// available as [`ebu::EbuR128Meter`]; it is an *approximate* meter built on the
/// block-based [`gating`]/[`lkfs`] pipeline and is not validated against the
/// EBU Tech 3342 reference signals.
pub use ebu_r128_impl::EbuR128Meter;
pub use filters::{KWeightFilter, KWeightFilterBank};
pub use gating::{GatingProcessor, GatingResult};
pub use lkfs::{LkfsCalculator, LufsValue};
pub use peak::{
    dbfs_to_linear, linear_to_dbfs, KSystemMeter, KSystemType, PeakMeter, PeakMeterType,
};
pub use phase::{Goniometer, GoniometerPoint, PhaseCorrelationMeter, StereoWidthAnalyzer};
pub use range::{LoudnessRange, LraCalculator};
pub use render::{
    colors, generate_db_scale, BarMeterConfig, BarMeterData, CircularMeterConfig, Color,
    ColorGradient, Orientation, ScaleMark, ScaleType,
};
pub use report::{ComplianceReport, LoudnessReport, MeteringReport};
pub use spectrum::{
    CachedSpectrumAnalyzer, OctaveBand, OctaveBandAnalyzer, SpectrumAnalyzer, WeightingCurve,
    WindowFunction,
};
pub use truepeak::{TruePeak, TruePeakDetector};
pub use video_color::{
    ColorGamut, ColorTemperatureMeter, GamutMeter, HsvColor, RgbColor, SaturationMeter,
};
pub use video_luminance::{BlackWhiteLevelMeter, LuminanceMeter};
pub use video_quality::{
    BlockinessDetector, Frame2D, PsnrCalculator, QualityAnalyzer, QualityMetrics, SsimCalculator,
};

/// Metering error types.
#[derive(Error, Debug)]
pub enum MeteringError {
    /// Invalid configuration.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// Insufficient data for measurement.
    #[error("Insufficient data: {0}")]
    InsufficientData(String),

    /// Sample format not supported.
    #[error("Unsupported sample format: {0:?}")]
    UnsupportedFormat(SampleFormat),

    /// Channel configuration error.
    #[error("Channel error: {0}")]
    ChannelError(String),

    /// Calculation error.
    #[error("Calculation error: {0}")]
    CalculationError(String),
}

/// Metering result type.
pub type MeteringResult<T> = std::result::Result<T, MeteringError>;

/// Broadcast loudness standard.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Standard {
    /// EBU R128 (European Broadcasting Union).
    ///
    /// Target: -23 LUFS ±1 LU
    /// Max True Peak: -1.0 dBTP
    #[default]
    EbuR128,

    /// ATSC A/85 (Advanced Television Systems Committee - US).
    ///
    /// Target: -24 LKFS ±2 dB
    /// Max True Peak: -2.0 dBTP
    AtscA85,

    /// Spotify streaming platform.
    ///
    /// Target: -14 LUFS
    /// Max True Peak: -1.0 dBTP
    Spotify,

    /// `YouTube` streaming platform.
    ///
    /// Target: -14 LUFS
    /// Max True Peak: -1.0 dBTP
    YouTube,

    /// Apple Music streaming platform.
    ///
    /// Target: -16 LUFS
    /// Max True Peak: -1.0 dBTP
    AppleMusic,

    /// Netflix streaming platform.
    ///
    /// Target: -27 LUFS
    /// Max True Peak: -2.0 dBTP
    Netflix,

    /// Amazon Prime Video.
    ///
    /// Target: -24 LUFS
    /// Max True Peak: -2.0 dBTP
    AmazonPrime,

    /// Tidal HiFi streaming platform.
    ///
    /// Target: -14 LUFS
    /// Max True Peak: -1.0 dBTP
    TidalHiFi,

    /// Amazon Music HD streaming platform.
    ///
    /// Target: -14 LUFS
    /// Max True Peak: -1.0 dBTP
    AmazonMusicHd,

    /// `TikTok` short-form video delivery.
    ///
    /// Target: -14 LUFS ±1 LU
    /// Max True Peak: -1.0 dBTP
    TikTok,

    /// Custom target loudness.
    ///
    /// Specify your own target in LUFS and max true peak in dBTP.
    Custom {
        /// Target loudness in LUFS.
        target_lufs: f64,
        /// Maximum true peak in dBTP.
        max_peak_dbtp: f64,
        /// Tolerance in LU.
        tolerance_lu: f64,
    },
}

impl Standard {
    /// Get the target loudness in LUFS for this standard.
    pub fn target_lufs(&self) -> f64 {
        match self {
            Self::EbuR128 => -23.0,
            Self::AtscA85 | Self::AmazonPrime => -24.0,
            Self::Spotify
            | Self::YouTube
            | Self::TidalHiFi
            | Self::AmazonMusicHd
            | Self::TikTok => -14.0,
            Self::AppleMusic => -16.0,
            Self::Netflix => -27.0,
            Self::Custom { target_lufs, .. } => *target_lufs,
        }
    }

    /// Get the maximum true peak in dBTP for this standard.
    pub fn max_true_peak_dbtp(&self) -> f64 {
        match self {
            Self::EbuR128
            | Self::Spotify
            | Self::YouTube
            | Self::AppleMusic
            | Self::TidalHiFi
            | Self::AmazonMusicHd
            | Self::TikTok => -1.0,
            Self::AtscA85 | Self::Netflix | Self::AmazonPrime => -2.0,
            Self::Custom { max_peak_dbtp, .. } => *max_peak_dbtp,
        }
    }

    /// Get the tolerance in LU for this standard.
    pub fn tolerance_lu(&self) -> f64 {
        match self {
            Self::EbuR128
            | Self::Spotify
            | Self::YouTube
            | Self::AppleMusic
            | Self::TidalHiFi
            | Self::AmazonMusicHd
            | Self::TikTok => 1.0,
            Self::AtscA85 | Self::Netflix | Self::AmazonPrime => 2.0,
            Self::Custom { tolerance_lu, .. } => *tolerance_lu,
        }
    }

    /// Get the standard name as a string.
    pub fn name(&self) -> &str {
        match self {
            Self::EbuR128 => "EBU R128",
            Self::AtscA85 => "ATSC A/85",
            Self::Spotify => "Spotify",
            Self::YouTube => "YouTube",
            Self::AppleMusic => "Apple Music",
            Self::Netflix => "Netflix",
            Self::AmazonPrime => "Amazon Prime Video",
            Self::TidalHiFi => "Tidal HiFi",
            Self::AmazonMusicHd => "Amazon Music HD",
            Self::TikTok => "TikTok",
            Self::Custom { .. } => "Custom",
        }
    }
}

/// Meter configuration.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct MeterConfig {
    /// Broadcast standard to measure against.
    pub standard: Standard,
    /// Sample rate in Hz.
    pub sample_rate: f64,
    /// Number of audio channels.
    pub channels: usize,
    /// Enable true peak detection (4x oversampling).
    pub enable_true_peak: bool,
    /// Enable loudness range (LRA) calculation.
    pub enable_lra: bool,
    /// Enable momentary loudness tracking.
    pub enable_momentary: bool,
    /// Enable short-term loudness tracking.
    pub enable_short_term: bool,
    /// Enable integrated loudness (gated program loudness).
    pub enable_integrated: bool,
}

impl MeterConfig {
    /// Create a new meter configuration.
    ///
    /// # Arguments
    ///
    /// * `standard` - Broadcast standard
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of channels
    pub fn new(standard: Standard, sample_rate: f64, channels: usize) -> Self {
        Self {
            standard,
            sample_rate,
            channels,
            enable_true_peak: true,
            enable_lra: true,
            enable_momentary: true,
            enable_short_term: true,
            enable_integrated: true,
        }
    }

    /// Create a minimal configuration (integrated loudness and true peak only).
    pub fn minimal(standard: Standard, sample_rate: f64, channels: usize) -> Self {
        Self {
            standard,
            sample_rate,
            channels,
            enable_true_peak: true,
            enable_lra: false,
            enable_momentary: false,
            enable_short_term: false,
            enable_integrated: true,
        }
    }

    /// Validate the configuration.
    ///
    /// # Errors
    ///
    /// Returns `MeteringError::InvalidConfig` if any configuration parameters are out of valid range.
    pub fn validate(&self) -> MeteringResult<()> {
        if self.sample_rate < 8000.0 || self.sample_rate > 192_000.0 {
            return Err(MeteringError::InvalidConfig(format!(
                "Sample rate {} Hz is out of valid range (8000-192000 Hz)",
                self.sample_rate
            )));
        }

        if self.channels == 0 || self.channels > 16 {
            return Err(MeteringError::InvalidConfig(format!(
                "Channel count {} is out of valid range (1-16)",
                self.channels
            )));
        }

        if !self.enable_integrated && !self.enable_momentary && !self.enable_short_term {
            return Err(MeteringError::InvalidConfig(
                "At least one loudness measurement must be enabled".to_string(),
            ));
        }

        Ok(())
    }
}

/// Channel configuration for multi-channel audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChannelLayout {
    /// Mono (1.0).
    Mono,
    /// Stereo (2.0).
    Stereo,
    /// 5.1 Surround (L, R, C, LFE, Ls, Rs).
    Surround51,
    /// 7.1 Surround (L, R, C, LFE, Ls, Rs, Lrs, Rrs).
    Surround71,
    /// 7.1.4 Dolby Atmos bed (L, R, C, LFE, Ls, Rs, Lrs, Rrs, Ltf, Rtf, Ltb, Rtb).
    Atmos714,
    /// NHK 22.2 immersive audio layout per ITU-R BS.2051-3.
    ///
    /// 24 speakers arranged in 3 layers:
    /// - Top layer (9 ch): TpFL, TpFR, TpFC, TpC, TpBL, TpBR, TpSiL, TpSiR, TpBC
    /// - Middle layer (10 ch): FL, FR, FC, LFE1, BL, BR, FLc, FRc, BC, LFE2
    /// - Bottom layer (4 ch): BtFL, BtFR, BtFC, BtBC
    /// - Plus 1 overhead centre (CH): TpFC (already in top)
    ///
    /// Channel order follows ITU-R BS.2051-3 Table 1 (22.2 layout).
    Nhk222,
    /// Custom channel configuration.
    Custom(usize),
}

impl ChannelLayout {
    /// Get the number of channels.
    pub fn channel_count(&self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::Surround51 => 6,
            Self::Surround71 => 8,
            Self::Atmos714 => 12,
            Self::Nhk222 => 24,
            Self::Custom(n) => *n,
        }
    }

    /// Get ITU-R BS.1770-4 channel weights for this layout.
    ///
    /// Returns a vector of weights to apply to each channel during loudness calculation.
    /// For the NHK 22.2 layout the weights follow ITU-R BS.2051-3 Section 6 (reproduced
    /// below).  The standard specifies that LFE channels contribute zero power and that
    /// surround/rear/overhead channels use a +1.5 dB gain (linear ≈ 1.189) relative to
    /// front centre and left/right channels.
    pub fn channel_weights(&self) -> Vec<f64> {
        match self {
            Self::Mono => vec![1.0],
            Self::Stereo => vec![1.0, 1.0],
            Self::Surround51 => {
                // L, R, C, LFE, Ls, Rs
                vec![1.0, 1.0, 1.0, 0.0, 1.41, 1.41]
            }
            Self::Surround71 => {
                // L, R, C, LFE, Ls, Rs, Lrs, Rrs
                vec![1.0, 1.0, 1.0, 0.0, 1.41, 1.41, 1.41, 1.41]
            }
            Self::Atmos714 => {
                // L, R, C, LFE, Ls, Rs, Lrs, Rrs, Ltf, Rtf, Ltb, Rtb
                vec![
                    1.0, 1.0, 1.0, 0.0, 1.41, 1.41, 1.41, 1.41, 1.41, 1.41, 1.41, 1.41,
                ]
            }
            Self::Nhk222 => {
                // ITU-R BS.2051-3 NHK 22.2 channel weights.
                //
                // 24 channels in order (ITU-R BS.2051-3 Table 1):
                //
                // Top layer (9 channels):
                //   0:TpFL  1:TpFR  2:TpFC  3:TpC  4:TpBL  5:TpBR  6:TpSiL  7:TpSiR  8:TpBC
                // Middle layer (10 channels):
                //   9:FL  10:FR  11:FC  12:LFE1  13:BL  14:BR  15:FLc  16:FRc  17:BC  18:LFE2
                // Bottom layer (4 channels):
                //  19:BtFL  20:BtFR  21:BtFC  22:BtBC
                // Plus one overhead:
                //  23:CH (overhead centre, equivalent to +1.5 dB weight)
                //
                // Weight rules from BS.2051-3 §6:
                //   - Front centre (FC): 1.0 (0 dB reference)
                //   - Left/Right front (FL, FR, FLc, FRc): 1.0
                //   - Surround/Rear (BL, BR, BC): 1.189 (~+1.5 dB)
                //   - Top layer: 1.189 (~+1.5 dB)
                //   - Bottom layer: 1.189 (~+1.5 dB)
                //   - Overhead centre (CH): 1.189 (~+1.5 dB)
                //   - LFE1, LFE2: 0.0 (excluded per BS.1770-4 §3.4)

                // √2 ≈ 1.41213, +1.5 dB ≈ 1.18850
                const W_SURROUND: f64 = 1.188_502_227_4; // 10^(1.5/20)
                const W_FRONT: f64 = 1.0;
                const W_LFE: f64 = 0.0;

                vec![
                    // Top layer (9 ch)
                    W_SURROUND, // TpFL
                    W_SURROUND, // TpFR
                    W_SURROUND, // TpFC
                    W_SURROUND, // TpC
                    W_SURROUND, // TpBL
                    W_SURROUND, // TpBR
                    W_SURROUND, // TpSiL
                    W_SURROUND, // TpSiR
                    W_SURROUND, // TpBC
                    // Middle layer (10 ch)
                    W_FRONT,    // FL
                    W_FRONT,    // FR
                    W_FRONT,    // FC
                    W_LFE,      // LFE1
                    W_SURROUND, // BL
                    W_SURROUND, // BR
                    W_FRONT,    // FLc
                    W_FRONT,    // FRc
                    W_SURROUND, // BC
                    W_LFE,      // LFE2
                    // Bottom layer (4 ch)
                    W_SURROUND, // BtFL
                    W_SURROUND, // BtFR
                    W_SURROUND, // BtFC
                    W_SURROUND, // BtBC
                    // Overhead centre
                    W_SURROUND, // CH
                ]
            }
            Self::Custom(n) => vec![1.0; *n],
        }
    }

    /// Create from channel count.
    pub fn from_channel_count(count: usize) -> Self {
        match count {
            1 => Self::Mono,
            2 => Self::Stereo,
            6 => Self::Surround51,
            8 => Self::Surround71,
            12 => Self::Atmos714,
            24 => Self::Nhk222,
            n => Self::Custom(n),
        }
    }
}

/// Loudness measurement metrics.
#[derive(Clone, Debug, Default)]
pub struct LoudnessMetrics {
    /// Momentary loudness in LUFS (400ms window).
    pub momentary_lufs: f64,
    /// Short-term loudness in LUFS (3s window).
    pub short_term_lufs: f64,
    /// Integrated loudness in LUFS (gated program loudness).
    pub integrated_lufs: f64,
    /// Loudness range in LU.
    pub loudness_range: f64,
    /// True peak in dBTP (maximum across all channels).
    pub true_peak_dbtp: f64,
    /// True peak in linear scale.
    pub true_peak_linear: f64,
    /// Maximum momentary loudness seen.
    pub max_momentary: f64,
    /// Maximum short-term loudness seen.
    pub max_short_term: f64,
    /// Per-channel true peaks in dBTP.
    pub channel_peaks_dbtp: Vec<f64>,
}

/// Main loudness meter.
///
/// This is the primary interface for loudness measurement. It combines all
/// measurement algorithms (LKFS, gating, true peak, LRA) into a single meter.
pub struct LoudnessMeter {
    config: MeterConfig,
    lkfs_calculator: LkfsCalculator,
    gating_processor: GatingProcessor,
    true_peak_detector: Option<TruePeakDetector>,
    lra_calculator: Option<LraCalculator>,
    filter_bank: KWeightFilterBank,
    channel_layout: ChannelLayout,
    samples_processed: usize,
}

impl LoudnessMeter {
    /// Create a new loudness meter.
    ///
    /// # Arguments
    ///
    /// * `config` - Meter configuration
    ///
    /// # Errors
    ///
    /// Returns error if configuration is invalid.
    pub fn new(config: MeterConfig) -> MeteringResult<Self> {
        config.validate()?;

        let channel_layout = ChannelLayout::from_channel_count(config.channels);
        let filter_bank = KWeightFilterBank::new(config.channels, config.sample_rate);
        let lkfs_calculator = LkfsCalculator::new(config.sample_rate, config.channels);
        let gating_processor = GatingProcessor::new(config.sample_rate, config.channels);

        let true_peak_detector = if config.enable_true_peak {
            Some(TruePeakDetector::new(config.sample_rate, config.channels))
        } else {
            None
        };

        let lra_calculator = if config.enable_lra {
            Some(LraCalculator::new())
        } else {
            None
        };

        Ok(Self {
            config,
            lkfs_calculator,
            gating_processor,
            true_peak_detector,
            lra_calculator,
            filter_bank,
            channel_layout,
            samples_processed: 0,
        })
    }

    /// Process f32 audio samples (interleaved).
    ///
    /// # Arguments
    ///
    /// * `samples` - Interleaved audio samples normalized to -1.0 to 1.0
    pub fn process_f32(&mut self, samples: &[f32]) {
        let f64_samples: Vec<f64> = samples.iter().map(|&s| f64::from(s)).collect();
        self.process_f64(&f64_samples);
    }

    /// Process f64 audio samples (interleaved).
    ///
    /// # Arguments
    ///
    /// * `samples` - Interleaved audio samples normalized to -1.0 to 1.0
    pub fn process_f64(&mut self, samples: &[f64]) {
        if samples.is_empty() {
            return;
        }

        // Apply K-weighting filter
        let mut filtered = vec![0.0; samples.len()];
        self.filter_bank
            .process_interleaved(samples, self.config.channels, &mut filtered);

        // Process LKFS calculation
        self.lkfs_calculator.process_interleaved(&filtered);

        // Process gating (for integrated loudness)
        self.gating_processor.process_interleaved(&filtered);

        // Process true peak (on original unfiltered samples)
        if let Some(ref mut detector) = self.true_peak_detector {
            detector.process_interleaved(samples);
        }

        self.samples_processed += samples.len() / self.config.channels;
    }

    /// Get current loudness metrics.
    pub fn metrics(&mut self) -> LoudnessMetrics {
        let momentary = if self.config.enable_momentary {
            self.lkfs_calculator.momentary_loudness()
        } else {
            f64::NEG_INFINITY
        };

        let short_term = if self.config.enable_short_term {
            self.lkfs_calculator.short_term_loudness()
        } else {
            f64::NEG_INFINITY
        };

        let integrated = if self.config.enable_integrated {
            self.gating_processor.integrated_loudness()
        } else {
            f64::NEG_INFINITY
        };

        let loudness_range = if let Some(ref mut lra_calc) = self.lra_calculator {
            let blocks = self.gating_processor.get_blocks_for_lra();
            lra_calc.calculate(&blocks)
        } else {
            0.0
        };

        let (true_peak_dbtp, true_peak_linear, channel_peaks_dbtp) =
            if let Some(ref detector) = self.true_peak_detector {
                let peaks = detector.channel_peaks_dbtp();
                let max_peak = detector.true_peak_dbtp();
                let max_linear = detector.true_peak_linear();
                (max_peak, max_linear, peaks)
            } else {
                (f64::NEG_INFINITY, 0.0, vec![])
            };

        LoudnessMetrics {
            momentary_lufs: momentary,
            short_term_lufs: short_term,
            integrated_lufs: integrated,
            loudness_range,
            true_peak_dbtp,
            true_peak_linear,
            max_momentary: self.lkfs_calculator.max_momentary(),
            max_short_term: self.lkfs_calculator.max_short_term(),
            channel_peaks_dbtp,
        }
    }

    /// Check compliance with the configured standard.
    pub fn check_compliance(&mut self) -> ComplianceResult {
        let metrics = self.metrics();
        let standard = &self.config.standard;

        let target = standard.target_lufs();
        let tolerance = standard.tolerance_lu();
        let max_peak = standard.max_true_peak_dbtp();

        let loudness_compliant = if metrics.integrated_lufs.is_finite() {
            metrics.integrated_lufs >= target - tolerance
                && metrics.integrated_lufs <= target + tolerance
        } else {
            false
        };

        let peak_compliant = metrics.true_peak_dbtp <= max_peak;

        let lra_acceptable = metrics.loudness_range >= 1.0 && metrics.loudness_range <= 30.0;

        ComplianceResult {
            standard: *standard,
            loudness_compliant,
            peak_compliant,
            lra_acceptable,
            integrated_lufs: metrics.integrated_lufs,
            true_peak_dbtp: metrics.true_peak_dbtp,
            loudness_range: metrics.loudness_range,
            target_lufs: target,
            max_peak_dbtp: max_peak,
            deviation_lu: if metrics.integrated_lufs.is_finite() {
                metrics.integrated_lufs - target
            } else {
                0.0
            },
        }
    }

    /// Generate a detailed loudness report.
    #[allow(clippy::cast_precision_loss)]
    pub fn generate_report(&mut self) -> LoudnessReport {
        let metrics = self.metrics();
        let compliance = self.check_compliance();
        let duration_seconds = self.samples_processed as f64 / self.config.sample_rate;

        LoudnessReport::new(metrics, compliance, duration_seconds)
    }

    /// Reset the meter to initial state.
    pub fn reset(&mut self) {
        self.lkfs_calculator.reset();
        self.gating_processor.reset();
        if let Some(ref mut detector) = self.true_peak_detector {
            detector.reset();
        }
        if let Some(ref mut lra_calc) = self.lra_calculator {
            lra_calc.reset();
        }
        self.filter_bank.reset();
        self.samples_processed = 0;
    }

    /// Get the meter configuration.
    pub fn config(&self) -> &MeterConfig {
        &self.config
    }

    /// Get the number of samples processed (per channel).
    pub fn samples_processed(&self) -> usize {
        self.samples_processed
    }

    /// Get the duration of processed audio in seconds.
    #[allow(clippy::cast_precision_loss)]
    pub fn duration_seconds(&self) -> f64 {
        self.samples_processed as f64 / self.config.sample_rate
    }
}

/// Compliance result.
#[derive(Clone, Debug)]
pub struct ComplianceResult {
    /// Standard being checked.
    pub standard: Standard,
    /// Is loudness compliant?
    pub loudness_compliant: bool,
    /// Is peak compliant?
    pub peak_compliant: bool,
    /// Is LRA acceptable?
    pub lra_acceptable: bool,
    /// Measured integrated loudness.
    pub integrated_lufs: f64,
    /// Measured true peak.
    pub true_peak_dbtp: f64,
    /// Measured loudness range.
    pub loudness_range: f64,
    /// Target loudness.
    pub target_lufs: f64,
    /// Maximum allowed peak.
    pub max_peak_dbtp: f64,
    /// Deviation from target in LU.
    pub deviation_lu: f64,
}

impl ComplianceResult {
    /// Check if fully compliant (loudness and peak).
    pub fn is_compliant(&self) -> bool {
        self.loudness_compliant && self.peak_compliant
    }

    /// Get the standard name.
    pub fn standard_name(&self) -> &str {
        self.standard.name()
    }

    /// Get recommended gain adjustment to meet target.
    ///
    /// Returns gain in dB (positive = increase, negative = decrease).
    pub fn recommended_gain_db(&self) -> f64 {
        if self.integrated_lufs.is_finite() {
            self.target_lufs - self.integrated_lufs
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EBU R128 reference signal test: 997 Hz sine at -23 LUFS.
    ///
    /// The complete ITU-R BS.1770-4 K-weighting chain (Stage 1 high-shelf at
    /// 1 681.97 Hz with G ≈ +4 dB, Stage 2 high-pass at 38.135 Hz) has a gain of
    /// **+0.691 dB** at 997 Hz — see `filters::tests::
    /// test_k_weight_chain_matches_itu_table1`, which pins the chain against
    /// Table 1 of the standard. The generated amplitude is compensated by the
    /// inverse of that gain so the meter should read exactly -23 LUFS.
    ///
    /// Calibration procedure:
    ///   1. Target LUFS = -23; after filter the mean-square must equal
    ///      `10^((-23 + 0.691) / 10)`.
    ///   2. The K-weight chain at 997 Hz has power gain ≈ 1.172 (+0.691 dB).
    ///   3. Required pre-filter RMS² = target_power / filter_gain.
    ///   4. For a sine wave: peak amplitude A = sqrt(2 × RMS²).
    ///
    /// We generate 10 seconds of stereo audio (enough for gating to converge)
    /// and assert the integrated loudness is within ±0.5 LUFS of -23.0.
    ///
    /// Before the K-weighting fix this constant read `3.41` dB, which silently
    /// compensated for a Stage 1 that was a high-pass scaled by the decibel
    /// value 3.9998 used as a linear gain.
    #[test]
    fn test_ebu_r128_reference_signal() {
        let sample_rate = 48000.0_f64;
        let channels = 2_usize;
        let duration_secs = 10.0_f64;
        let freq_hz = 997.0_f64;

        // K-weighting chain gain at 997 Hz, from ITU-R BS.1770-4 Table 1.
        let k_weight_power_gain_db = 0.691_f64;
        let target_power = 10.0_f64.powf((-23.0_f64 + 0.691) / 10.0);
        let filter_power_gain = 10.0_f64.powf(k_weight_power_gain_db / 10.0);
        // For a stereo signal with identical L/R: the gating normalises by
        // total_weight = 2.0, so block_ms = (ch0_ms + ch1_ms) / N / 2 = A²/2 * gain.
        // Solving A²/2 * gain = target_power:
        let amplitude = (2.0 * target_power / filter_power_gain).sqrt();

        let total_samples = (sample_rate * duration_secs) as usize;
        let mut interleaved = Vec::with_capacity(total_samples * channels);

        for i in 0..total_samples {
            let t = i as f64 / sample_rate;
            let sample = amplitude * (2.0 * std::f64::consts::PI * freq_hz * t).sin();
            // Stereo: identical L and R channels
            interleaved.push(sample);
            interleaved.push(sample);
        }

        let config = MeterConfig::new(Standard::EbuR128, sample_rate, channels);
        let mut meter = LoudnessMeter::new(config).expect("Failed to create LoudnessMeter");
        meter.process_f64(&interleaved);

        let metrics = meter.metrics();
        let integrated = metrics.integrated_lufs;

        assert!(
            integrated.is_finite(),
            "Integrated loudness should be finite, got {integrated}"
        );
        assert!(
            (integrated - (-23.0)).abs() <= 0.5,
            "Expected -23.0 LUFS ±0.5, got {integrated:.2} LUFS"
        );
    }

    /// Verify TidalHiFi standard has correct target loudness.
    #[test]
    fn test_tidal_hifi_standard() {
        let s = Standard::TidalHiFi;
        assert_eq!(s.target_lufs(), -14.0);
        assert_eq!(s.max_true_peak_dbtp(), -1.0);
        assert_eq!(s.name(), "Tidal HiFi");
    }

    /// Verify AmazonMusicHd standard has correct target loudness.
    #[test]
    fn test_amazon_music_hd_standard() {
        let s = Standard::AmazonMusicHd;
        assert_eq!(s.target_lufs(), -14.0);
        assert_eq!(s.max_true_peak_dbtp(), -1.0);
        assert_eq!(s.name(), "Amazon Music HD");
    }

    /// Verify the TikTok delivery target: -14 LUFS, -1 dBTP, ±1 LU.
    #[test]
    fn test_tiktok_standard() {
        let s = Standard::TikTok;
        assert_eq!(s.target_lufs(), -14.0);
        assert_eq!(s.max_true_peak_dbtp(), -1.0);
        assert_eq!(s.tolerance_lu(), 1.0);
        assert_eq!(s.name(), "TikTok");
    }

    /// The crate-root `EbuR128Meter` must be the standards-accurate
    /// `ebu_r128_impl` implementation, not the approximate `ebu` wrapper.
    #[test]
    fn test_crate_root_ebu_meter_is_accurate_impl() {
        // `ebu_r128_impl::EbuR128Meter::new` is infallible and takes (u32, u32);
        // the approximate `ebu::EbuR128Meter::new` returns a Result and needs a
        // `ProgramType`, so this only compiles for the accurate implementation.
        let meter: crate::EbuR128Meter = crate::EbuR128Meter::new(48_000, 2);
        assert_eq!(meter.sample_rate(), 48_000);
        assert_eq!(meter.channels(), 2);
    }
}
