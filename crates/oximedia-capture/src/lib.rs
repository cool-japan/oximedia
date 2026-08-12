//! Pure-Rust live video capture for OxiMedia.
//!
//! This crate is the replacement for `libavdevice`'s capture side: it opens a
//! camera, negotiates a format with it, and delivers frames on a bounded queue.
//! It compiles and links no C or C++, and the workspace lint set denies
//! `unsafe_code` for this crate outright — the platform bindings that do need
//! `unsafe` live in their own modules and arrive with the packages below.
//!
//! # Platform support
//!
//! The backend is selected by `cfg(target_os)` in `platform::native`:
//!
//! | Target | API | Package | Status |
//! |---|---|---|---|
//! | macOS | AVFoundation, via `objc2` | A5 | shipping, hardware-verified |
//! | Linux | Video4Linux2, via `rustix` ioctls | A6 | shipping, device-unverified |
//! | Windows | Media Foundation, via the `windows` crate | A7 | shipping, device-unverified |
//! | anything else | — | — | [`CaptureError::UnsupportedPlatform`] |
//!
//! "Shipping" means the backend is implemented, compiles for its target, and
//! passes its own unit tests — on its own that claims nothing about a real
//! device. macOS *was* verified against real hardware, during package A5:
//! its `enumerate()` path is exercised by a non-`#[ignore]`d unit test that
//! calls real AVFoundation on every run and accepts either a populated
//! device list or a permission refusal as correct, a materially stronger bar
//! than Linux/Windows below. Linux's V4L2 struct layouts are instead
//! pinned by a compile-time golden-layout table, checked by `cargo check`
//! itself on both a 64-bit (`aarch64-unknown-linux-gnu`) and a 32-bit
//! (`i686-unknown-linux-gnu`) target against kernel 6.19 UAPI headers, and
//! Windows' Media Foundation logic is exercised host-side against
//! synthetically constructed inputs. Neither Linux nor Windows has yet opened
//! a real `/dev/video*` node or a real Media Foundation source in this
//! workspace's own verification — see the crate README's platform matrix for
//! the full breakdown and the opt-in `OXIMEDIA_CAPTURE_DEVICE` live-device
//! test instructions.
//!
//! A target with no backend does **not** return an empty device list. An empty
//! list is what a working backend says when no camera is plugged in, and
//! conflating "not implemented" with "nothing attached" turns a missing feature
//! into a hardware bug hunt. [`backend_kind`] likewise reports
//! [`BackendKind::Unsupported`] rather than the API the platform *would* use.
//!
//! Everything that does **not** depend on a platform API is complete and
//! usable everywhere: the format model, the negotiation algorithm, the delivery
//! queue with its drop policies, the monotonic clock, the statistics, and a
//! deterministic `mock` backend that drives all of it without a camera.
//!
//! ## macOS permissions
//!
//! Camera access is gated by TCC. [`enumerate`] never *prompts* — an
//! undetermined status lists devices, names and modes without raising the
//! system dialog — but it does respect an answer already on file: a
//! previously denied or restricted status makes it return
//! [`CaptureError::PermissionDenied`] instead of a device list. [`open`] goes
//! further: if the process has never been asked, it triggers the system
//! dialog and waits, bounded, for the answer. A refusal is
//! [`CaptureError::PermissionDenied`], never a session that quietly delivers
//! black frames.
//!
//! ## Linux permissions
//!
//! Access is plain file permissions on the `/dev/video*` node, which on most
//! distributions means membership of the `video` group. A node this process may
//! not open is skipped during [`enumerate`], because a machine can have one
//! camera the user may use and one they may not; but if *every* node was refused
//! — so the list would be empty — that is reported as
//! [`CaptureError::PermissionDenied`] rather than as "no cameras attached", so
//! the answer points at `usermod -aG video` instead of at a USB cable.
//!
//! ## Windows permissions
//!
//! Camera access is gated by the system privacy settings. Listing devices is
//! not affected, but activating one answers `E_ACCESSDENIED` when access is
//! off, which this crate reports as [`CaptureError::PermissionDenied`] rather
//! than as a generic platform fault. During [`enumerate`] the same refusal
//! leaves the device listed with an empty format list — it exists, and this
//! process was not allowed to ask it anything.
//!
//! # Shape of the API
//!
//! ```no_run
//! use oximedia_capture::{CaptureConfig, CaptureEncoding, DeviceSelector};
//! use oximedia_core::PixelFormat;
//!
//! let config = CaptureConfig::default()
//!     .with_device(DeviceSelector::Index(0))
//!     .with_size(1280, 720)
//!     .with_fps(30.0)
//!     .with_preferred([CaptureEncoding::Raw(PixelFormat::Nv12)]);
//!
//! match oximedia_capture::open(config) {
//!     Ok(mut session) => {
//!         let negotiated = session.negotiated_format();
//!         if let Some(mut stream) = session.take_stream() {
//!             while let Ok(Some(frame)) = stream.recv() {
//!                 let _ = (negotiated, frame.sequence);
//!             }
//!         }
//!     }
//!     // No camera, no permission, or no backend for this target.
//!     Err(error) => eprintln!("capture unavailable: {error}"),
//! }
//! ```
//!
//! The format actually used is [`CaptureSession::negotiated_format`], not the
//! request: cameras rarely offer exactly what was asked for, and
//! [`negotiate()`] is a deterministic, fully tested function of the device's
//! advertised modes and the request. Its ranking is documented in the
//! [`negotiate`](mod@negotiate) module.
//!
//! # Testing without hardware
//!
//! Enable the `mock` feature for a synthetic device with a scripted frame
//! count, cadence, failure point, clock and sequence counter. See the `mock` module.
//!
//! # Async
//!
//! The core stays synchronous by design (see the `asyncio` module docs for
//! why). Enable the `tokio` feature for `CaptureStream::into_async`, a thin
//! bridge — a dedicated thread forwarding into a `tokio::sync::mpsc` channel
//! — onto a Tokio-native `AsyncCaptureStream`. Nothing about `open()`, the
//! platform backends or the sync `CaptureStream` changes when this feature
//! is enabled.
//!
//! # Testing against real hardware
//!
//! Live-device tests are opt-in through the `OXIMEDIA_CAPTURE_DEVICE`
//! environment variable, which names the device id to open, *and* are marked
//! `#[ignore]`. They are skipped when the variable is unset, because a CI
//! runner has no camera and a test that silently passes on absent hardware is
//! worse than no test:
//!
//! ```bash
//! # macOS: the id is the AVCaptureDevice uniqueID from `enumerate()`
//! OXIMEDIA_CAPTURE_DEVICE=0x8020000005ac8514 \
//!     cargo test -p oximedia-capture -- --ignored
//! ```

#[cfg(feature = "tokio")]
pub mod asyncio;
mod backend;
pub mod config;
pub mod device;
pub mod error;
pub mod frame;
pub mod negotiate;
mod platform;
mod pool;
pub mod session;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

#[cfg(feature = "tokio")]
pub use asyncio::AsyncCaptureStream;
pub use config::{
    CaptureConfig, DropPolicy, DEFAULT_BUFFER_COUNT, DEFAULT_ENCODING_PREFERENCE,
    DEFAULT_QUEUE_DEPTH,
};
pub use device::{BackendKind, CaptureDevice, CaptureEncoding, CaptureFormat, DeviceSelector};
pub use error::CaptureError;
pub use frame::{CaptureFrame, FramePayload, TimestampSource};
pub use negotiate::negotiate;
pub use session::{CaptureSession, CaptureStats, CaptureStream};

/// List the capture devices the platform currently reports.
///
/// # Errors
///
/// Returns [`CaptureError::UnsupportedPlatform`] on a target with no backend
/// compiled in (anything other than macOS, Linux or Windows). A backend that
/// is present but finds no camera returns `Ok(vec![])` instead; the two cases
/// are deliberately distinguishable.
///
/// # Examples
///
/// ```
/// match oximedia_capture::enumerate() {
///     Ok(devices) => println!("{} capture device(s)", devices.len()),
///     Err(error) => eprintln!("capture unavailable: {error}"),
/// }
/// ```
pub fn enumerate() -> Result<Vec<CaptureDevice>, CaptureError> {
    platform::native().enumerate()
}

/// Open a capture session.
///
/// Enumerates, applies [`CaptureConfig::device`], negotiates a format against
/// what the device advertises, configures it, and starts a capture thread.
///
/// # Errors
///
/// See [`CaptureError`]. On a target with no backend compiled in (anything
/// other than macOS, Linux or Windows) this is always
/// [`CaptureError::UnsupportedPlatform`].
pub fn open(config: CaptureConfig) -> Result<CaptureSession, CaptureError> {
    CaptureSession::open(config)
}

/// Which capture API this build would use.
///
/// Returns [`BackendKind::Unsupported`] when no backend is compiled in for the
/// current target. It never names the backend a platform *would* use once
/// implemented — reporting `AvFoundation` from a build that contains no
/// AVFoundation code would be a fabricated answer to a capability question.
///
/// # Examples
///
/// ```
/// use oximedia_capture::{backend_kind, BackendKind};
///
/// if backend_kind() == BackendKind::Unsupported {
///     eprintln!("no capture backend compiled in for this target");
/// }
/// ```
pub fn backend_kind() -> BackendKind {
    platform::native().kind()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn re_exports_cover_the_documented_surface() {
        // A compile-time check that the crate root exposes the whole model.
        let config = CaptureConfig::default()
            .with_device(DeviceSelector::Default)
            .with_drop_policy(DropPolicy::DropOldest);
        let format = CaptureFormat::new(CaptureEncoding::Mjpeg, 640, 480, 30, 1);
        let device = CaptureDevice {
            id: "x".into(),
            name: "x".into(),
            formats: vec![format],
            backend: BackendKind::Mock,
        };
        assert_eq!(
            negotiate(&device.formats, &config).expect("one format, no constraints"),
            format
        );
        assert_eq!(DEFAULT_QUEUE_DEPTH, config.queue_depth);
        assert_eq!(DEFAULT_BUFFER_COUNT, config.buffer_count);
        assert_eq!(DEFAULT_ENCODING_PREFERENCE.len(), 6);
        assert_eq!(TimestampSource::HostArrival.name(), "host-arrival");
        assert!(FramePayload::Compressed(Vec::new()).is_compressed());
    }

    /// On a target with no backend compiled in, both entry points must report
    /// the same thing, and that thing must be
    /// [`CaptureError::UnsupportedPlatform`].
    ///
    /// This cannot be asserted on a platform that *has* a backend: `open()`
    /// starts a real capture session — on macOS that means a TCC permission
    /// dialog and a camera LED — and its outcome legitimately differs from
    /// `enumerate()`'s (a machine can list a camera that another process is
    /// already holding). Linux is the same argument with a different mechanism:
    /// a machine with no camera makes `enumerate()` say `Ok(vec![])`, which is
    /// correct, while `open()` says [`CaptureError::DeviceNotFound`], which is
    /// also correct — so the two legitimately disagree about `is_err()`. The
    /// live tests in `tests/live_capture.rs` cover the real device path, opt-in
    /// and off by default.
    // Windows is carved out for the same reason as macOS/Linux: it has a real
    // backend, so on a camera-less host `enumerate()` returns `Ok(vec![])`
    // while `open()` returns `DeviceNotFound` — a legitimate disagreement.
    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn enumerate_and_open_agree_about_platform_support() {
        let listed = enumerate();
        let opened = open(CaptureConfig::default());
        assert_eq!(
            listed.is_err(),
            opened.is_err(),
            "enumerate and open must not disagree about whether capture works"
        );
        if backend_kind() == BackendKind::Unsupported {
            assert!(matches!(
                listed,
                Err(CaptureError::UnsupportedPlatform { .. })
            ));
        }
    }

    /// Linux always has a backend compiled in, and enumeration is side-effect
    /// free: no node is opened for streaming and no permission is requested.
    /// The three honest outcomes are a device list (possibly empty), a refusal
    /// to open any node, and a `/dev` that cannot be scanned at all.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_native_backend_is_v4l2_on_linux() {
        assert_eq!(backend_kind(), BackendKind::V4l2);
        match enumerate() {
            Ok(_) | Err(CaptureError::PermissionDenied { .. }) | Err(CaptureError::Io { .. }) => {}
            other => panic!("unexpected V4L2 enumeration result: {other:?}"),
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_native_backend_is_avfoundation_on_macos() {
        assert_eq!(backend_kind(), BackendKind::AvFoundation);
        // Enumeration is prompt-free; it either lists devices or says the user
        // refused access. It never claims the platform is unsupported.
        match enumerate() {
            Ok(_) | Err(CaptureError::PermissionDenied { .. }) => {}
            other => panic!("unexpected AVFoundation enumeration result: {other:?}"),
        }
    }
}
