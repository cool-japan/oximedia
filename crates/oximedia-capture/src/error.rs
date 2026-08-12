//! Capture error type.
//!
//! Every failure mode of this crate is a distinct variant. In particular there
//! is no "capture unavailable, here is an empty list" path: a platform with no
//! backend compiled in reports [`CaptureError::UnsupportedPlatform`], and a
//! device that cannot satisfy a request reports which request and how many
//! formats it did offer. Silence and empty vectors are indistinguishable from
//! "no cameras attached", and that ambiguity is exactly what makes capture bugs
//! expensive to diagnose.

use crate::device::{CaptureFormat, DeviceSelector};

/// Anything that can go wrong while enumerating, opening or running a capture
/// device.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CaptureError {
    /// No capture backend is compiled in for the current target.
    ///
    /// `target` is [`std::env::consts::OS`] for the running build.
    #[error("no capture backend on this platform ({target})")]
    UnsupportedPlatform {
        /// Operating system the build targets.
        target: &'static str,
    },

    /// The selector matched no enumerated device.
    #[error("capture device not found: {0}")]
    DeviceNotFound(DeviceSelector),

    /// The device advertised formats, but none satisfied the request.
    #[error("device {device:?} advertised {available} format(s), none acceptable")]
    NoMatchingFormat {
        /// Device the formats came from.
        device: String,
        /// How many formats the device advertised *before* filtering.
        available: usize,
    },

    /// The device advertised a format but refused to be configured for it.
    ///
    /// Cameras routinely advertise modes they cannot actually deliver — a
    /// frame rate only reachable at a lower resolution, a format the firmware
    /// exposes but never fills. This is that case, kept separate from
    /// [`Self::NoMatchingFormat`] because the fix is different: retry with a
    /// different mode rather than relax the request.
    #[error("device {device:?} rejected format {format}: {reason}")]
    FormatRejected {
        /// Device that rejected the format.
        device: String,
        /// The rejected format. Boxed to keep the enum small.
        format: Box<CaptureFormat>,
        /// What the backend reported.
        reason: String,
    },

    /// An operating-system I/O error on the device.
    #[error("I/O error on capture device {device:?}")]
    Io {
        /// Device the operation targeted.
        device: String,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// A backend-specific failure that has no portable representation.
    #[error("capture backend error: {0}")]
    Platform(String),

    /// The process is not permitted to open the device.
    ///
    /// On macOS this is the TCC camera permission; on Linux it is usually
    /// group membership on the `/dev/video*` node; on Windows it is the camera
    /// privacy setting.
    #[error("permission denied for capture device {device:?}")]
    PermissionDenied {
        /// Device that was refused.
        device: String,
    },

    /// The capture thread unwound. Any frames it had not yet delivered are
    /// lost, and the session is finished.
    #[error("capture thread panicked")]
    ThreadPanic,

    /// An error surfaced from `oximedia-core`.
    #[error(transparent)]
    Core(#[from] oximedia_core::OxiError),
}

impl CaptureError {
    /// Build a [`Self::Platform`] error from anything string-like.
    pub fn platform(message: impl Into<String>) -> Self {
        Self::Platform(message.into())
    }

    /// Build a [`Self::Io`] error for a named device.
    pub fn io(device: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            device: device.into(),
            source,
        }
    }

    /// Build a [`Self::FormatRejected`] error for a named device.
    pub fn format_rejected(
        device: impl Into<String>,
        format: CaptureFormat,
        reason: impl Into<String>,
    ) -> Self {
        Self::FormatRejected {
            device: device.into(),
            format: Box::new(format),
            reason: reason.into(),
        }
    }

    /// Attach a device id to an error raised by a component that did not know
    /// which device it was working on.
    ///
    /// [`crate::negotiate()`] is a pure function over a format list and cannot
    /// name the device, so it leaves the field empty; the session fills it in
    /// here. Errors that already carry a device id, and errors with no device
    /// field, pass through untouched.
    #[must_use]
    pub fn with_device(self, device: &str) -> Self {
        match self {
            Self::NoMatchingFormat {
                device: existing,
                available,
            } if existing.is_empty() => Self::NoMatchingFormat {
                device: device.to_owned(),
                available,
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::CaptureEncoding;

    #[test]
    fn unsupported_platform_names_the_target() {
        let err = CaptureError::UnsupportedPlatform {
            target: std::env::consts::OS,
        };
        let text = err.to_string();
        assert!(text.contains("no capture backend"), "{text}");
        assert!(text.contains(std::env::consts::OS), "{text}");
    }

    #[test]
    fn device_not_found_shows_the_selector() {
        let err = CaptureError::DeviceNotFound(DeviceSelector::Index(7));
        assert!(err.to_string().contains('7'), "{err}");
    }

    #[test]
    fn with_device_fills_an_empty_slot() {
        let err = CaptureError::NoMatchingFormat {
            device: String::new(),
            available: 3,
        }
        .with_device("/dev/video0");
        match err {
            CaptureError::NoMatchingFormat { device, available } => {
                assert_eq!(device, "/dev/video0");
                assert_eq!(available, 3);
            }
            other => panic!("expected NoMatchingFormat, got {other:?}"),
        }
    }

    #[test]
    fn with_device_does_not_overwrite_an_existing_id() {
        let err = CaptureError::NoMatchingFormat {
            device: "/dev/video1".into(),
            available: 1,
        }
        .with_device("/dev/video0");
        match err {
            CaptureError::NoMatchingFormat { device, .. } => assert_eq!(device, "/dev/video1"),
            other => panic!("expected NoMatchingFormat, got {other:?}"),
        }
    }

    #[test]
    fn with_device_passes_other_variants_through() {
        let err = CaptureError::platform("boom").with_device("/dev/video0");
        assert!(matches!(err, CaptureError::Platform(_)));
    }

    #[test]
    fn io_errors_keep_their_source() {
        let err = CaptureError::io(
            "/dev/video0",
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope"),
        );
        let source = std::error::Error::source(&err).map(ToString::to_string);
        assert_eq!(source.as_deref(), Some("nope"));
    }

    #[test]
    fn format_rejected_renders_the_format() {
        let format = CaptureFormat::new(CaptureEncoding::Mjpeg, 1920, 1080, 60, 1);
        let err = CaptureError::format_rejected("/dev/video0", format, "unsupported at 60 fps");
        let text = err.to_string();
        assert!(text.contains("1920x1080"), "{text}");
        assert!(text.contains("unsupported at 60 fps"), "{text}");
    }

    #[test]
    fn core_errors_convert() {
        let err: CaptureError = oximedia_core::OxiError::Eof.into();
        assert!(matches!(err, CaptureError::Core(_)));
    }
}
