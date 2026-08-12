//! AVFoundation capture backend for macOS.
//!
//! This module is the *safe* half of the backend: the [`CaptureBackend`] and
//! [`CaptureRunner`] implementations, the plain-data description of a chosen
//! device mode, and the pure conversions between what Core Media reports
//! (four-character codes, `CMTime` value/timescale pairs) and what this crate
//! models ([`CaptureEncoding`], [`std::time::Duration`], exact rational frame
//! rates). Every one of those conversions is unit-tested without a camera.
//!
//! The three sibling modules carry the Objective-C bindings and are the only
//! files in the crate that opt out of the workspace's `unsafe_code = "deny"`:
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`avf`] | Authorization, device discovery, session lifecycle |
//! | [`delegate`] | The `AVCaptureVideoDataOutputSampleBufferDelegate` class |
//! | [`pixel`] | Copying samples out of a locked `CVPixelBuffer` |
//!
//! # Threading invariant
//!
//! **No Objective-C object is created by [`CaptureBackend::open`].**
//!
//! `objc2::rc::Retained<T>` is `!Send`: an Objective-C object handed out on one
//! thread may not be released on another, because `-release` can run arbitrary
//! `-dealloc` code and many AppKit/AVFoundation classes are not thread-safe.
//! [`crate::session::CaptureSession`] builds the runner on the *caller's*
//! thread and then moves it to a freshly spawned capture thread, so anything
//! `open()` allocated would cross a thread boundary.
//!
//! Therefore `open()` only records plain data — a device id, the negotiated
//! [`CaptureFormat`], the request — and **all** of `AVCaptureSession`,
//! `AVCaptureDevice`, `AVCaptureDeviceInput`, `AVCaptureVideoDataOutput`, the
//! sample-buffer delegate and the dispatch queue are created inside
//! [`CaptureRunner::run`], live for the duration of that call, and are dropped
//! on the same thread before it returns. Nothing Objective-C escapes.
//!
//! The one thread this backend does *not* own is the delegate's serial dispatch
//! queue, which AVFoundation drives. See [`delegate`] for the lifetime argument
//! that makes publishing from that thread sound.

use std::time::Duration;

use oximedia_core::PixelFormat;

use crate::backend::{CaptureBackend, CaptureRunner, FrameSink, StopSignal};
use crate::config::CaptureConfig;
use crate::device::{BackendKind, CaptureDevice, CaptureEncoding, CaptureFormat};
use crate::error::CaptureError;
// Shared with the Media Foundation backend, which reduces its own frame rates
// the same way; see `platform::common`.
use super::common::gcd;

mod avf;
mod delegate;
mod pixel;

/// Name given to the serial dispatch queue that runs delegate callbacks.
pub(crate) const CALLBACK_QUEUE_LABEL: &str = "io.cooljapan.oximedia.capture";

/// How long the capture thread parks between stop-flag checks.
///
/// The thread has nothing to do while AVFoundation pushes frames from its own
/// queue, so this is purely shutdown latency: [`crate::CaptureSession::stop`]
/// joins this thread, and it cannot notice the flag faster than this.
pub(crate) const PARK_INTERVAL: Duration = Duration::from_millis(10);

/// Longest wait for the user to answer the camera permission prompt.
///
/// `+requestAccessForMediaType:completionHandler:` never times out on its own:
/// if the dialog is dismissed by a crash, or the process has no way to show it,
/// the completion handler simply never runs. A capture thread that parks
/// forever would make [`crate::CaptureSession::stop`] hang forever with it, so
/// the wait is bounded and a timeout is reported as
/// [`CaptureError::PermissionDenied`] — the honest answer, since access was in
/// fact not granted.
pub(crate) const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(60);

/// Device name used in errors raised before any specific device is known.
pub(crate) const VIDEO_CAPTURE_SUBJECT: &str = "<video capture>";

// ── Four-character codes ─────────────────────────────────────────────────────

/// Pack a four-character code the way Core Media stores one.
///
/// `CMFormatDescriptionGetMediaSubType` returns a `FourCharCode`, which is the
/// four ASCII bytes of the code in big-endian order packed into a `u32`.
const fn fourcc(code: &[u8; 4]) -> u32 {
    ((code[0] as u32) << 24) | ((code[1] as u32) << 16) | ((code[2] as u32) << 8) | (code[3] as u32)
}

/// `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange` — NV12, video range.
pub(crate) const FOURCC_420V: u32 = fourcc(b"420v");
/// `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` — NV12, full range.
pub(crate) const FOURCC_420F: u32 = fourcc(b"420f");
/// `kCVPixelFormatType_422YpCbCr8` — packed 4:2:2, U first (UYVY).
pub(crate) const FOURCC_2VUY: u32 = fourcc(b"2vuy");
/// `kCVPixelFormatType_422YpCbCr8_yuvs` — packed 4:2:2, Y first (YUY2).
pub(crate) const FOURCC_YUVS: u32 = fourcc(b"yuvs");
/// `kCMVideoCodecType_JPEG_OpenDML` — Motion JPEG as UVC cameras report it.
pub(crate) const FOURCC_DMB1: u32 = fourcc(b"dmb1");
/// `kCMVideoCodecType_JPEG` — baseline JPEG.
pub(crate) const FOURCC_JPEG: u32 = fourcc(b"jpeg");

/// Map a Core Media media sub-type onto this crate's encoding model.
///
/// Returns `None` for every code that is not in the table above. Unknown codes
/// are *skipped* by [`avf::enumerate_devices`], never guessed at: a wrong guess
/// would be reported to the caller as a real capture mode and would then
/// produce mis-decoded samples, which is far more expensive to diagnose than a
/// device that appears to advertise one mode fewer.
pub(crate) const fn encoding_for_fourcc(code: u32) -> Option<CaptureEncoding> {
    match code {
        // Both bi-planar 4:2:0 flavours are NV12 in memory. They differ only in
        // the luma range convention (16-235 vs 0-255), which `PixelFormat` does
        // not model; the range is a colorimetry property, not a layout.
        FOURCC_420V | FOURCC_420F => Some(CaptureEncoding::Raw(PixelFormat::Nv12)),
        FOURCC_2VUY => Some(CaptureEncoding::Raw(PixelFormat::Uyvy422)),
        FOURCC_YUVS => Some(CaptureEncoding::Raw(PixelFormat::Yuyv422)),
        FOURCC_DMB1 | FOURCC_JPEG => Some(CaptureEncoding::Mjpeg),
        _ => None,
    }
}

/// Render a four-character code the way Apple's headers spell it.
///
/// Non-printable bytes become `.`, so a corrupt or unexpected code still
/// produces a log line that can be read.
pub(crate) fn fourcc_name(code: u32) -> String {
    code.to_be_bytes()
        .iter()
        .map(|&byte| {
            if byte.is_ascii_graphic() || byte == b' ' {
                char::from(byte)
            } else {
                '.'
            }
        })
        .collect()
}

// ── CMTime ───────────────────────────────────────────────────────────────────

/// `kCMTimeFlags_Valid`.
pub(crate) const CM_TIME_FLAG_VALID: u32 = 1 << 0;

/// Convert a `CMTime` into a [`Duration`], or `None` when it cannot represent
/// one.
///
/// `CMTime` is a rational: `value / timescale` seconds. Four cases have no
/// `Duration` answer and all four are reported as `None` rather than as a
/// fabricated zero:
///
/// * the `Valid` flag is clear — the timestamp is `kCMTimeInvalid`, or one of
///   the infinities / `kCMTimeIndefinite`;
/// * `timescale <= 0` — division by zero, or a nonsensical negative rate;
/// * `value < 0` — a time before the timeline origin, which [`Duration`]
///   cannot hold;
/// * the whole-seconds part overflows [`u64`].
///
/// A `None` here is what makes the caller fall back to
/// [`crate::TimestampSource::HostArrival`] instead of inventing a device clock.
pub(crate) fn cmtime_to_duration(value: i64, timescale: i32, flags: u32) -> Option<Duration> {
    if flags & CM_TIME_FLAG_VALID == 0 {
        return None;
    }
    if timescale <= 0 || value < 0 {
        return None;
    }
    // 128-bit intermediate: `value` can be a nanosecond-scale host timestamp
    // (~10^18 by the time a Mac has been up for a month) and multiplying the
    // remainder by 10^9 would overflow `i64`.
    let value = u128::from(value.unsigned_abs());
    let timescale = u128::from(timescale.unsigned_abs());
    let seconds = u64::try_from(value / timescale).ok()?;
    let nanos = u32::try_from((value % timescale) * 1_000_000_000 / timescale).ok()?;
    Some(Duration::new(seconds, nanos))
}

/// Recover an exact rational frame rate from a `CMTime` frame *duration*.
///
/// A frame duration of `value / timescale` seconds is a frame rate of
/// `timescale / value` frames per second, so the numerator and denominator
/// simply swap. Taking the rate this way rather than rounding
/// `AVFrameRateRange.maxFrameRate` (an `f64`) is what keeps `30000/1001`
/// exactly `30000/1001` instead of `29.97`, which
/// [`CaptureFormat::cmp_fps`](crate::CaptureFormat::cmp_fps) compares as a
/// different rate.
///
/// The result is reduced to lowest terms. AVFoundation reports plain 30 fps as
/// a duration of `1000000/30000000`, and while
/// [`CaptureFormat::cmp_fps`](crate::CaptureFormat::cmp_fps) would still
/// compare that equal to `30/1`, [`CaptureFormat`] itself derives `PartialEq`
/// field by field — so an unreduced pair would make two descriptions of the
/// same mode unequal, defeating the duplicate filter in enumeration and making
/// `30000000/1000000` the number users see. Reduction is exact: dividing both
/// terms by their GCD cannot change the value.
///
/// Returns `None` when either component is non-positive or does not fit in the
/// `u32` fields of [`CaptureFormat`].
pub(crate) fn fps_from_frame_duration(value: i64, timescale: i32) -> Option<(u32, u32)> {
    if value <= 0 || timescale <= 0 {
        return None;
    }
    let fps_num = u32::try_from(timescale).ok()?;
    let fps_den = u32::try_from(value).ok()?;
    let divisor = gcd(fps_num, fps_den).max(1);
    Some((fps_num / divisor, fps_den / divisor))
}

/// Split a frame rate back into the `CMTime` frame duration that expresses it.
///
/// The inverse of [`fps_from_frame_duration`], used to drive
/// `-[AVCaptureDevice setActiveVideoMinFrameDuration:]`, whose argument is the
/// reciprocal of the maximum frame rate. `None` for a rate with a zero
/// component, which [`CaptureFormat`] documents as "unknown / variable" and
/// which therefore must not be turned into a duration.
pub(crate) const fn frame_duration_for_fps(fps_num: u32, fps_den: u32) -> Option<(i64, i32)> {
    if fps_num == 0 || fps_den == 0 {
        return None;
    }
    Some((fps_den as i64, fps_num as i32))
}

// ── Backend ──────────────────────────────────────────────────────────────────

/// The AVFoundation capture backend.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AvfBackend;

impl AvfBackend {
    /// A backend handle. Holds no state and touches no framework until used.
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl CaptureBackend for AvfBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::AvFoundation
    }

    fn enumerate(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        avf::enumerate_devices()
    }

    fn open(
        &self,
        device: &CaptureDevice,
        format: CaptureFormat,
        config: &CaptureConfig,
    ) -> Result<Box<dyn CaptureRunner>, CaptureError> {
        // Deliberately allocates nothing Objective-C; see the module docs.
        if config.buffer_count != crate::config::DEFAULT_BUFFER_COUNT {
            tracing::debug!(
                requested = config.buffer_count,
                "AVFoundation manages its own capture buffer ring; buffer_count is ignored"
            );
        }
        Ok(Box::new(AvfRunner {
            device_id: device.id.clone(),
            device_name: device.name.clone(),
            format,
        }))
    }
}

// ── Runner ───────────────────────────────────────────────────────────────────

/// The capture loop for one opened AVFoundation device.
///
/// Plain data only — see the module docs on why this must stay `Send` without
/// holding a single `Retained<T>`.
#[derive(Debug)]
pub(crate) struct AvfRunner {
    /// `AVCaptureDevice.uniqueID` of the device to open.
    device_id: String,
    /// `AVCaptureDevice.localizedName`, carried for error messages.
    device_name: String,
    /// The mode [`crate::negotiate()`] settled on.
    format: CaptureFormat,
}

impl AvfRunner {
    /// The device this runner will open.
    pub(crate) fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Human-readable device name, for logs and errors.
    pub(crate) fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The negotiated mode.
    pub(crate) const fn format(&self) -> CaptureFormat {
        self.format
    }
}

impl CaptureRunner for AvfRunner {
    fn run(&mut self, sink: &FrameSink, stop: &StopSignal) -> Result<(), CaptureError> {
        avf::run_capture(self, sink, stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Four-character codes ────────────────────────────────────────────────

    #[test]
    fn fourcc_packs_big_endian_ascii() {
        assert_eq!(FOURCC_420V, 0x3432_3076);
        assert_eq!(FOURCC_420F, 0x3432_3066);
        assert_eq!(FOURCC_2VUY, 0x3276_7579);
        assert_eq!(FOURCC_YUVS, 0x7975_7673);
        assert_eq!(FOURCC_DMB1, 0x646D_6231);
        assert_eq!(FOURCC_JPEG, 0x6A70_6567);
    }

    #[test]
    fn known_subtypes_map_to_the_documented_encodings() {
        let table = [
            (FOURCC_420V, CaptureEncoding::Raw(PixelFormat::Nv12)),
            (FOURCC_420F, CaptureEncoding::Raw(PixelFormat::Nv12)),
            (FOURCC_2VUY, CaptureEncoding::Raw(PixelFormat::Uyvy422)),
            (FOURCC_YUVS, CaptureEncoding::Raw(PixelFormat::Yuyv422)),
            (FOURCC_DMB1, CaptureEncoding::Mjpeg),
            (FOURCC_JPEG, CaptureEncoding::Mjpeg),
        ];
        for (code, expected) in table {
            assert_eq!(
                encoding_for_fourcc(code),
                Some(expected),
                "{} mapped wrong",
                fourcc_name(code)
            );
        }
    }

    #[test]
    fn the_two_nv12_flavours_agree_because_range_is_not_a_layout() {
        assert_eq!(
            encoding_for_fourcc(FOURCC_420V),
            encoding_for_fourcc(FOURCC_420F)
        );
    }

    #[test]
    fn unknown_subtypes_are_skipped_never_guessed() {
        // Real codes this backend has no copy path for: 10-bit bi-planar,
        // 32-bit BGRA, planar 4:2:0 with a separate V plane, and HEVC.
        for code in [
            fourcc(b"x420"),
            fourcc(b"BGRA"),
            fourcc(b"y420"),
            fourcc(b"hvc1"),
            0,
            u32::MAX,
        ] {
            assert_eq!(
                encoding_for_fourcc(code),
                None,
                "{} must not be guessed at",
                fourcc_name(code)
            );
        }
    }

    #[test]
    fn every_mapped_code_round_trips_through_its_name() {
        for (code, name) in [
            (FOURCC_420V, "420v"),
            (FOURCC_420F, "420f"),
            (FOURCC_2VUY, "2vuy"),
            (FOURCC_YUVS, "yuvs"),
            (FOURCC_DMB1, "dmb1"),
            (FOURCC_JPEG, "jpeg"),
        ] {
            assert_eq!(fourcc_name(code), name);
            assert_eq!(fourcc(name.as_bytes().try_into().expect("4 bytes")), code);
            assert!(encoding_for_fourcc(code).is_some());
        }
    }

    #[test]
    fn fourcc_names_survive_non_printable_bytes() {
        assert_eq!(fourcc_name(0), "....");
        assert_eq!(fourcc_name(fourcc(b"jp  ")), "jp  ");
    }

    // ── CMTime → Duration ───────────────────────────────────────────────────

    #[test]
    fn valid_cmtime_converts_exactly() {
        assert_eq!(
            cmtime_to_duration(1_500_000_000, 1_000_000_000, CM_TIME_FLAG_VALID),
            Some(Duration::from_millis(1_500))
        );
        assert_eq!(
            cmtime_to_duration(1001, 30_000, CM_TIME_FLAG_VALID),
            Some(Duration::from_nanos(33_366_666))
        );
    }

    #[test]
    fn a_large_host_timestamp_does_not_overflow() {
        // ~158 years of nanoseconds, near the top of what a `CMTime` value can
        // hold: the 128-bit intermediate is load-bearing, because the remainder
        // is multiplied by 10^9 before the division.
        let converted =
            cmtime_to_duration(5_000_000_000_000_000_001, 1_000_000_000, CM_TIME_FLAG_VALID)
                .expect("representable");
        assert_eq!(converted.as_secs(), 5_000_000_000);
        assert_eq!(converted.subsec_nanos(), 1);
    }

    #[test]
    fn timescale_zero_has_no_duration() {
        assert_eq!(cmtime_to_duration(1_000, 0, CM_TIME_FLAG_VALID), None);
    }

    #[test]
    fn an_invalid_flag_word_has_no_duration() {
        assert_eq!(cmtime_to_duration(1_000, 1_000, 0), None);
        // kCMTimeIndefinite / the infinities all leave `Valid` clear.
        assert_eq!(cmtime_to_duration(0, 1, 1 << 4), None);
    }

    #[test]
    fn negative_components_have_no_duration() {
        assert_eq!(cmtime_to_duration(-1, 1_000, CM_TIME_FLAG_VALID), None);
        assert_eq!(cmtime_to_duration(1_000, -30, CM_TIME_FLAG_VALID), None);
    }

    #[test]
    fn zero_is_a_representable_instant_not_a_failure() {
        assert_eq!(
            cmtime_to_duration(0, 30_000, CM_TIME_FLAG_VALID),
            Some(Duration::ZERO)
        );
    }

    /// The clock, not the conversion, is what makes a backwards device
    /// timestamp harmless — [`cmtime_to_duration`] hands raw instants through
    /// unchanged and [`crate::frame::MonotonicClock`] clamps them.
    #[test]
    fn backwards_device_timestamps_are_the_clocks_problem_not_the_conversions() {
        let mut clock = crate::frame::MonotonicClock::new();
        let raw: Vec<Duration> = [(3000_i64, 1000_i32), (4000, 1000), (3500, 1000)]
            .into_iter()
            .map(|(value, timescale)| {
                cmtime_to_duration(value, timescale, CM_TIME_FLAG_VALID).expect("valid")
            })
            .collect();
        let mapped: Vec<Duration> = raw.iter().map(|&raw| clock.map(raw)).collect();
        assert_eq!(
            mapped,
            vec![
                Duration::ZERO,
                Duration::from_secs(1),
                Duration::from_secs(1)
            ]
        );
        assert_eq!(clock.regressions(), 1);
    }

    // ── Frame rate rationals ────────────────────────────────────────────────

    #[test]
    fn ntsc_frame_duration_yields_an_exact_rational() {
        assert_eq!(fps_from_frame_duration(1001, 30_000), Some((30_000, 1001)));
        let format = CaptureFormat::new(CaptureEncoding::Mjpeg, 1920, 1080, 30_000, 1001);
        assert!((format.fps() - 29.970_029_97).abs() < 1e-6);
    }

    #[test]
    fn integer_frame_durations_yield_integer_rationals() {
        assert_eq!(fps_from_frame_duration(1, 30), Some((30, 1)));
        assert_eq!(fps_from_frame_duration(1, 60), Some((60, 1)));
    }

    /// The shape a real Mac camera reports: AVFoundation uses a timescale of
    /// 1 000 000, so plain 30 fps arrives as 1000000/30000000.
    #[test]
    fn microsecond_timescale_durations_reduce_to_lowest_terms() {
        assert_eq!(
            fps_from_frame_duration(1_000_000, 30_000_000),
            Some((30, 1))
        );
        assert_eq!(
            fps_from_frame_duration(1_000_000, 15_000_000),
            Some((15, 1))
        );
        assert_eq!(
            fps_from_frame_duration(1_000_000, 60_000_000),
            Some((60, 1))
        );
        assert_eq!(fps_from_frame_duration(1_000_000, 1_000_000), Some((1, 1)));
    }

    #[test]
    fn reduction_never_changes_the_rate_it_describes() {
        for (value, timescale) in [
            (1_000_000_i64, 30_000_000_i32),
            (1001, 30_000),
            (2002, 60_000),
            (1, 30),
            (7, 13),
        ] {
            let (fps_num, fps_den) =
                fps_from_frame_duration(value, timescale).expect("a real frame rate");
            assert_eq!(
                u64::from(fps_num) * value.unsigned_abs(),
                u64::from(fps_den) * timescale.unsigned_abs() as u64,
                "{fps_num}/{fps_den} is not {timescale}/{value}"
            );
            assert_eq!(gcd(fps_num, fps_den), 1, "{fps_num}/{fps_den} not reduced");
        }
    }

    #[test]
    fn ntsc_is_already_in_lowest_terms_and_survives_reduction() {
        // 60000/2002 reduces to 30000/1001, the same rate written once.
        assert_eq!(fps_from_frame_duration(2002, 60_000), Some((30_000, 1001)));
        assert_eq!(fps_from_frame_duration(1001, 30_000), Some((30_000, 1001)));
    }

    #[test]
    fn gcd_is_the_usual_one() {
        assert_eq!(gcd(30_000_000, 1_000_000), 1_000_000);
        assert_eq!(gcd(30_000, 1001), 1);
        assert_eq!(gcd(0, 5), 5);
        assert_eq!(gcd(5, 0), 5);
        assert_eq!(gcd(0, 0), 0);
    }

    #[test]
    fn non_positive_frame_durations_have_no_rate() {
        assert_eq!(fps_from_frame_duration(0, 30), None);
        assert_eq!(fps_from_frame_duration(-1, 30), None);
        assert_eq!(fps_from_frame_duration(1, 0), None);
        assert_eq!(fps_from_frame_duration(1, -30), None);
    }

    #[test]
    fn frame_rates_that_do_not_fit_u32_are_rejected() {
        assert_eq!(fps_from_frame_duration(i64::from(u32::MAX) + 1, 30), None);
    }

    #[test]
    fn frame_duration_round_trips_through_the_frame_rate() {
        for (fps_num, fps_den) in [(30_000_u32, 1001_u32), (30, 1), (60, 1), (24, 1)] {
            let (value, timescale) =
                frame_duration_for_fps(fps_num, fps_den).expect("a real frame rate");
            assert_eq!(
                fps_from_frame_duration(value, timescale),
                Some((fps_num, fps_den))
            );
        }
    }

    #[test]
    fn an_unknown_frame_rate_has_no_frame_duration() {
        assert_eq!(frame_duration_for_fps(30, 0), None);
        assert_eq!(frame_duration_for_fps(0, 1), None);
    }

    // ── Backend surface ─────────────────────────────────────────────────────

    #[test]
    fn the_backend_names_itself_avfoundation() {
        let backend = AvfBackend::new();
        assert_eq!(backend.kind(), BackendKind::AvFoundation);
        assert!(backend.kind().is_available());
    }

    #[test]
    fn open_records_the_request_without_touching_the_framework() {
        // No camera, no permission and no framework call is involved: `open`
        // is pure bookkeeping, which is exactly the threading invariant this
        // backend depends on.
        let format = CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 1280, 720, 30, 1);
        let device = CaptureDevice {
            id: "0x8020000005ac8514".into(),
            name: "FaceTime HD Camera".into(),
            formats: vec![format],
            backend: BackendKind::AvFoundation,
        };
        let runner = AvfBackend::new()
            .open(&device, format, &CaptureConfig::default())
            .expect("open records the request");
        drop(runner);
    }

    #[test]
    fn runner_accessors_report_what_open_recorded() {
        let format = CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 640, 480, 30, 1);
        let runner = AvfRunner {
            device_id: "unique-id".into(),
            device_name: "A Camera".into(),
            format,
        };
        assert_eq!(runner.device_id(), "unique-id");
        assert_eq!(runner.device_name(), "A Camera");
        assert_eq!(runner.format(), format);
    }

    #[test]
    fn the_runner_is_send_because_it_holds_no_objective_c() {
        const fn assert_send<T: Send>() {}
        assert_send::<AvfRunner>();
        assert_send::<AvfBackend>();
    }
}
