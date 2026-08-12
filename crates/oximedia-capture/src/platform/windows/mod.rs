//! Media Foundation capture backend for Windows.
//!
//! This module is the *safe* half of the backend: the [`CaptureBackend`] and
//! [`CaptureRunner`] implementations, the mapping between Media Foundation's
//! video subtype GUIDs and this crate's encoding model, and the stream and flag
//! constants the reader works in. It contains no `unsafe` block, because every
//! framework call it needs is already wrapped by a safe function in one of the
//! two files below.
//!
//! | Module | Responsibility | `unsafe` |
//! |---|---|---|
//! | [`com`] | COM/MF initialization, device activation objects, task memory, strings | yes |
//! | [`reader`] | `IMFSourceReader`: native type discovery and the capture loop | yes |
//! | [`super::mf_logic`] | attribute unpacking, stride and plane arithmetic, `HRESULT` classification | no |
//!
//! [`super::mf_logic`] is deliberately *not* under this module: it is compiled
//! and unit-tested on every host, not only on Windows, which is what puts the
//! stride, plane-split and error-classification decisions under test from a
//! machine that has no camera and no Windows.
//!
//! # Threading invariant
//!
//! **No COM object is created by [`CaptureBackend::open`].**
//!
//! COM is per-thread: an apartment is initialized on one thread, and an
//! interface pointer obtained inside it may not simply be used from another.
//! The `windows` crate's interface types are correspondingly `!Send`.
//! [`crate::session::CaptureSession`] builds the runner on the *caller's*
//! thread and then moves it to a freshly spawned capture thread, so anything
//! `open()` allocated would cross that boundary.
//!
//! Therefore `open()` records plain data only — a symbolic link, the negotiated
//! [`CaptureFormat`], the request — and the `MfGuard`, the activation object,
//! the media source and the source reader are all created inside
//! [`CaptureRunner::run`], live for the duration of that call, and are dropped
//! on the same thread before it returns.
//!
//! [`CaptureBackend::enumerate`] is the same story on the caller's thread: it
//! initializes COM, lists devices, and shuts everything down again before it
//! returns, so no COM object outlives the call.

use windows::core::GUID;
use windows::Win32::Media::MediaFoundation::{
    MFVideoFormat_MJPG, MFVideoFormat_NV12, MFVideoFormat_UYVY, MFVideoFormat_YUY2,
    MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
    MF_SOURCE_READERF_ERROR, MF_SOURCE_READERF_STREAMTICK, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM,
};

use oximedia_core::PixelFormat;

use crate::backend::{CaptureBackend, CaptureRunner, FrameSink, StopSignal};
use crate::config::CaptureConfig;
use crate::device::{BackendKind, CaptureDevice, CaptureEncoding, CaptureFormat};
use crate::error::CaptureError;

mod com;
mod reader;

// ── Stream and flag constants ────────────────────────────────────────────────

/// `MF_SOURCE_READER_FIRST_VIDEO_STREAM`, as `ReadSample` wants it.
///
/// The reader's stream selectors are *negative* `i32` sentinels — this one is
/// `-4` — while every stream-index parameter is a `u32`. The cast is the
/// documented calling convention, not a bug: `0xFFFF_FFFC` is exactly the bit
/// pattern the reader compares against.
pub(crate) const FIRST_VIDEO_STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

/// `MF_SOURCE_READER_ALL_STREAMS`, as a stream index. See [`FIRST_VIDEO_STREAM`].
pub(crate) const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;

/// `MF_SOURCE_READERF_ERROR`.
pub(crate) const READERF_ERROR: u32 = MF_SOURCE_READERF_ERROR.0 as u32;
/// `MF_SOURCE_READERF_ENDOFSTREAM`.
pub(crate) const READERF_ENDOFSTREAM: u32 = MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
/// `MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED`.
pub(crate) const READERF_CURRENTMEDIATYPECHANGED: u32 =
    MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32;
/// `MF_SOURCE_READERF_STREAMTICK`.
pub(crate) const READERF_STREAMTICK: u32 = MF_SOURCE_READERF_STREAMTICK.0 as u32;

/// How long the capture loop parks after a run of reads that produced no frame.
///
/// `ReadSample` normally blocks until the next frame, so this is never reached
/// on a healthy stream; it exists so that a source which returns immediately and
/// endlessly without producing anything cannot spin a core. It is also the
/// worst case this backend adds to
/// [`CaptureSession::stop`](crate::CaptureSession::stop)'s join, since the stop
/// flag is checked at the top of every iteration. See
/// [`super::mf_logic::should_park`].
pub(crate) const PARK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Upper bound on the media types read from one stream during enumeration.
///
/// The loop's real exit is `MF_E_NO_MORE_TYPES`. This bound only makes a driver
/// that never reports it a logged anomaly rather than a hang: no camera comes
/// close to four thousand modes.
pub(crate) const MAX_NATIVE_TYPES: u32 = 4096;

// ── Subtype mapping ──────────────────────────────────────────────────────────

/// Map a Media Foundation video subtype onto this crate's encoding model.
///
/// Returns `None` for every subtype not in the table, and those are *skipped*
/// by [`reader::describe_formats`] with a `debug!` line rather than guessed at:
/// a wrong guess is reported to the caller as a real capture mode and then
/// produces mis-decoded samples, which is far more expensive to diagnose than a
/// device that appears to advertise one mode fewer.
///
/// # `MFVideoFormat_RGB24` is deliberately absent
///
/// Media Foundation's `RGB24` is not [`PixelFormat::Rgb24`]. Windows inherits
/// its uncompressed RGB layout from DIBs: the bytes of a pixel are **B, G, R**,
/// and the image is usually stored bottom-up. `PixelFormat` has no `Bgr24`, so
/// the only ways to accept the subtype would be to hand back samples whose red
/// and blue channels are swapped — silently wrong colour in every frame — or to
/// permute the channels during the copy, which would make this backend a
/// converter and break the promise that it delivers what the camera produced.
/// Skipping it keeps both promises, and matches how the AVFoundation backend
/// treats `BGRA` for the same reason.
pub(crate) fn encoding_for_subtype(subtype: GUID) -> Option<CaptureEncoding> {
    if subtype == MFVideoFormat_NV12 {
        Some(CaptureEncoding::Raw(PixelFormat::Nv12))
    } else if subtype == MFVideoFormat_YUY2 {
        Some(CaptureEncoding::Raw(PixelFormat::Yuyv422))
    } else if subtype == MFVideoFormat_UYVY {
        Some(CaptureEncoding::Raw(PixelFormat::Uyvy422))
    } else if subtype == MFVideoFormat_MJPG {
        Some(CaptureEncoding::Mjpeg)
    } else {
        None
    }
}

/// The subtype that asks a device for `encoding`.
///
/// The exact inverse of [`encoding_for_subtype`]: an encoding this backend
/// cannot name has no subtype, and asking for it is
/// [`CaptureError::FormatRejected`] rather than a substitution.
pub(crate) fn subtype_for_encoding(encoding: CaptureEncoding) -> Option<GUID> {
    match encoding {
        CaptureEncoding::Raw(PixelFormat::Nv12) => Some(MFVideoFormat_NV12),
        CaptureEncoding::Raw(PixelFormat::Yuyv422) => Some(MFVideoFormat_YUY2),
        CaptureEncoding::Raw(PixelFormat::Uyvy422) => Some(MFVideoFormat_UYVY),
        CaptureEncoding::Mjpeg => Some(MFVideoFormat_MJPG),
        CaptureEncoding::Raw(_) => None,
    }
}

/// Render a video subtype the way the headers spell it.
///
/// Media Foundation's video subtypes are FOURCC-derived GUIDs: the first field
/// holds the four character code in little-endian order and the rest is the
/// fixed `0000-0010-8000-00AA00389B71` suffix. Codes whose bytes are printable
/// come out as `NV12` or `MJPG`; anything else — including the D3D format
/// numbers the RGB subtypes use — comes out as hexadecimal, so an unexpected
/// subtype still produces a log line that can be read.
pub(crate) fn subtype_name(subtype: GUID) -> String {
    let code = subtype.data1.to_le_bytes();
    if code
        .iter()
        .all(|&byte| byte.is_ascii_graphic() || byte == b' ')
    {
        code.iter().map(|&byte| char::from(byte)).collect()
    } else {
        format!("{:#010x}", subtype.data1)
    }
}

// ── Backend ──────────────────────────────────────────────────────────────────

/// The Media Foundation capture backend.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MfBackend;

impl MfBackend {
    /// A backend handle. Holds no state and touches no framework until used.
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl CaptureBackend for MfBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::MediaFoundation
    }

    /// List every video capture device Media Foundation reports.
    ///
    /// Listing a device's *modes* requires instantiating its media source, so
    /// this briefly activates each camera in turn — see
    /// [`reader::describe_formats`]. Everything COM this creates is released
    /// before the call returns.
    fn enumerate(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        // Declared first, so it is dropped last: every activation object and
        // every media source below must be released before `MFShutdown`.
        let guard = com::MfGuard::new()?;
        let activates = com::enumerate_activates(&guard)?;

        let mut devices = Vec::with_capacity(activates.len());
        for activate in &activates {
            let Some(id) = com::symbolic_link(activate) else {
                // Without a symbolic link there is no stable id, so the device
                // could be listed but never reopened. Reporting it would hand
                // the caller a `DeviceSelector::Id` that never matches again.
                tracing::debug!("skipping a capture source with no symbolic link");
                continue;
            };
            let name = com::friendly_name(activate).unwrap_or_else(|| id.clone());
            let formats = reader::describe_formats(activate, &id);
            devices.push(CaptureDevice {
                id,
                name,
                formats,
                backend: BackendKind::MediaFoundation,
            });
        }
        Ok(devices)
    }

    fn open(
        &self,
        device: &CaptureDevice,
        format: CaptureFormat,
        config: &CaptureConfig,
    ) -> Result<Box<dyn CaptureRunner>, CaptureError> {
        // Deliberately allocates nothing COM; see the module docs.
        if config.buffer_count != crate::config::DEFAULT_BUFFER_COUNT {
            tracing::debug!(
                requested = config.buffer_count,
                "Media Foundation manages its own capture buffer pool; buffer_count is ignored"
            );
        }
        Ok(Box::new(MfRunner {
            device_id: device.id.clone(),
            device_name: device.name.clone(),
            format,
        }))
    }
}

// ── Runner ───────────────────────────────────────────────────────────────────

/// The capture loop for one opened Media Foundation device.
///
/// Plain data only — see the module docs on why this must stay `Send` without
/// holding a single interface pointer.
#[derive(Debug)]
pub(crate) struct MfRunner {
    /// The device's symbolic link, which is the id this backend hands out.
    device_id: String,
    /// The device's friendly name, carried for logs and errors.
    device_name: String,
    /// The mode [`crate::negotiate()`] settled on, updated if the device
    /// changes the stream's media type mid-session.
    format: CaptureFormat,
}

impl MfRunner {
    /// The device this runner will open.
    pub(crate) fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Human-readable device name, for logs and errors.
    pub(crate) fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The mode currently in effect.
    pub(crate) const fn format(&self) -> CaptureFormat {
        self.format
    }

    /// Record a mid-session media type change.
    ///
    /// Delivered frames carry [`CaptureFrame::format`](crate::CaptureFrame),
    /// so this is what keeps a frame's description of itself true after a
    /// device re-negotiates its own stream.
    pub(crate) const fn set_format(&mut self, format: CaptureFormat) {
        self.format = format;
    }
}

impl CaptureRunner for MfRunner {
    fn run(&mut self, sink: &FrameSink, stop: &StopSignal) -> Result<(), CaptureError> {
        reader::run_capture(self, sink, stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Subtype mapping ─────────────────────────────────────────────────────

    #[test]
    fn known_subtypes_map_to_the_documented_encodings() {
        let table = [
            (MFVideoFormat_NV12, CaptureEncoding::Raw(PixelFormat::Nv12)),
            (
                MFVideoFormat_YUY2,
                CaptureEncoding::Raw(PixelFormat::Yuyv422),
            ),
            (
                MFVideoFormat_UYVY,
                CaptureEncoding::Raw(PixelFormat::Uyvy422),
            ),
            (MFVideoFormat_MJPG, CaptureEncoding::Mjpeg),
        ];
        for (subtype, expected) in table {
            assert_eq!(
                encoding_for_subtype(subtype),
                Some(expected),
                "{} mapped wrong",
                subtype_name(subtype)
            );
        }
    }

    #[test]
    fn the_subtype_map_round_trips() {
        for subtype in [
            MFVideoFormat_NV12,
            MFVideoFormat_YUY2,
            MFVideoFormat_UYVY,
            MFVideoFormat_MJPG,
        ] {
            let encoding = encoding_for_subtype(subtype).expect("a mapped subtype");
            assert_eq!(subtype_for_encoding(encoding), Some(subtype));
        }
    }

    #[test]
    fn unsupported_subtypes_are_skipped_never_guessed() {
        use windows::Win32::Media::MediaFoundation::{
            MFVideoFormat_ARGB32, MFVideoFormat_H264, MFVideoFormat_P010, MFVideoFormat_RGB24,
            MFVideoFormat_YV12,
        };
        for subtype in [
            // The one that is skipped on purpose despite being uncompressed.
            MFVideoFormat_RGB24,
            MFVideoFormat_ARGB32,
            MFVideoFormat_P010,
            MFVideoFormat_YV12,
            MFVideoFormat_H264,
            GUID::zeroed(),
        ] {
            assert_eq!(
                encoding_for_subtype(subtype),
                None,
                "{} must not be guessed at",
                subtype_name(subtype)
            );
        }
    }

    #[test]
    fn encodings_with_no_subtype_have_no_substitute() {
        for encoding in [
            CaptureEncoding::Raw(PixelFormat::Rgb24),
            CaptureEncoding::Raw(PixelFormat::Yuv420p),
            CaptureEncoding::Raw(PixelFormat::Gray8),
            CaptureEncoding::Raw(PixelFormat::Nv21),
        ] {
            assert_eq!(subtype_for_encoding(encoding), None, "{encoding}");
        }
    }

    #[test]
    fn subtype_names_read_as_four_character_codes() {
        assert_eq!(subtype_name(MFVideoFormat_NV12), "NV12");
        assert_eq!(subtype_name(MFVideoFormat_YUY2), "YUY2");
        assert_eq!(subtype_name(MFVideoFormat_UYVY), "UYVY");
        assert_eq!(subtype_name(MFVideoFormat_MJPG), "MJPG");
    }

    #[test]
    fn a_non_printable_subtype_still_logs_readably() {
        use windows::Win32::Media::MediaFoundation::MFVideoFormat_RGB24;
        // D3DFMT_R8G8B8 is 20, which is not four printable bytes.
        assert_eq!(subtype_name(MFVideoFormat_RGB24), "0x00000014");
        assert_eq!(subtype_name(GUID::zeroed()), "0x00000000");
    }

    // ── Stream constants ────────────────────────────────────────────────────

    #[test]
    fn the_stream_selectors_are_the_documented_sentinels() {
        assert_eq!(FIRST_VIDEO_STREAM, 0xFFFF_FFFC, "-4 as a stream index");
        assert_eq!(ALL_STREAMS, 0xFFFF_FFFE, "-2 as a stream index");
    }

    #[test]
    fn the_reader_flags_are_distinct_bits() {
        let flags = [
            READERF_ERROR,
            READERF_ENDOFSTREAM,
            READERF_CURRENTMEDIATYPECHANGED,
            READERF_STREAMTICK,
        ];
        for (index, flag) in flags.iter().enumerate() {
            assert_eq!(flag.count_ones(), 1, "flag {index} is not a single bit");
            for other in &flags[index + 1..] {
                assert_eq!(flag & other, 0, "flags overlap");
            }
        }
    }

    // ── Backend surface ─────────────────────────────────────────────────────

    #[test]
    fn the_backend_names_itself_media_foundation() {
        let backend = MfBackend::new();
        assert_eq!(backend.kind(), BackendKind::MediaFoundation);
        assert!(backend.kind().is_available());
    }

    #[test]
    fn open_records_the_request_without_touching_the_framework() {
        // No camera, no COM apartment and no framework call is involved:
        // `open` is pure bookkeeping, which is exactly the threading invariant
        // this backend depends on.
        let format = CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 1280, 720, 30, 1);
        let device = CaptureDevice {
            id: r"\\?\usb#vid_046d&pid_0825&mi_00#6&1ec1c4c&0&0000".into(),
            name: "HD Pro Webcam C920".into(),
            formats: vec![format],
            backend: BackendKind::MediaFoundation,
        };
        let runner = MfBackend::new()
            .open(&device, format, &CaptureConfig::default())
            .expect("open records the request");
        drop(runner);
    }

    #[test]
    fn runner_accessors_report_what_open_recorded() {
        let format = CaptureFormat::new(CaptureEncoding::Raw(PixelFormat::Nv12), 640, 480, 30, 1);
        let mut runner = MfRunner {
            device_id: r"\\?\usb#vid_046d".into(),
            device_name: "A Camera".into(),
            format,
        };
        assert_eq!(runner.device_id(), r"\\?\usb#vid_046d");
        assert_eq!(runner.device_name(), "A Camera");
        assert_eq!(runner.format(), format);

        let changed = CaptureFormat::new(CaptureEncoding::Mjpeg, 1920, 1080, 30, 1);
        runner.set_format(changed);
        assert_eq!(runner.format(), changed, "a mid-session type change sticks");
    }

    #[test]
    fn the_runner_is_send_because_it_holds_no_com_pointer() {
        const fn assert_send<T: Send>() {}
        assert_send::<MfRunner>();
        assert_send::<MfBackend>();
    }
}
