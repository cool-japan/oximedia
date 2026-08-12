//! Platform backend dispatch.
//!
//! [`native`] answers a single question: which capture API does this build
//! speak?
//!
//! | Target | Backend | Package |
//! |---|---|---|
//! | `target_os = "macos"` | AVFoundation, via `objc2` | A5 |
//! | `target_os = "linux"` | V4L2, via `rustix` ioctls on `/dev/video*` | A6 |
//! | `target_os = "windows"` | Media Foundation, via the `windows` crate | A7 |
//! | anything else | [`UnsupportedBackend`] | — |
//!
//! Every target without a backend gets [`UnsupportedBackend`], which reports
//! [`crate::CaptureError::UnsupportedPlatform`] rather than an empty device
//! list. That is the whole reason this indirection exists: the honest "capture
//! is not implemented here" answer stays available, and each remaining package
//! replaces one `cfg` arm without touching any other module.
//!
//! iOS is *not* on the AVFoundation arm. The framework is there, but the
//! device-discovery types, the session-preset behaviour and the permission flow
//! differ enough that claiming support without being able to test it would be
//! the same fabrication [`UnsupportedBackend`] exists to prevent.

use crate::backend::{CaptureBackend, UnsupportedBackend};

/// Arithmetic more than one backend needs. Compiled for the targets whose
/// backends use it, and under `cfg(test)` everywhere so its tests always run.
#[cfg(any(test, target_os = "macos", target_os = "windows", target_os = "linux"))]
mod common;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

/// The pure half of the Media Foundation backend.
///
/// Split out of [`windows`] and compiled under `cfg(test)` on *every* target,
/// because `windows` itself is `cfg(target_os = "windows")` and its tests would
/// otherwise never run on a developer machine. See the module documentation.
#[cfg(any(test, target_os = "windows"))]
mod mf_logic;

#[cfg(target_os = "windows")]
mod windows;

/// The capture backend for the current target.
///
/// Each arm is additive; the fall-through stays as written. It is written as a
/// `return` inside a `cfg` block rather than as an `if`/`else` so that the
/// fall-through is compiled and type-checked on *every* target — which is also
/// what keeps [`UnsupportedBackend`] from being reported as dead code on a
/// platform that never reaches it.
pub(crate) fn native() -> Box<dyn CaptureBackend> {
    #[cfg(target_os = "macos")]
    {
        return Box::new(macos::AvfBackend::new());
    }

    #[cfg(target_os = "windows")]
    {
        return Box::new(windows::MfBackend::new());
    }

    #[cfg(target_os = "linux")]
    {
        return Box::new(linux::V4l2Backend::new());
    }

    #[allow(unreachable_code)]
    Box::new(UnsupportedBackend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::BackendKind;
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    use crate::error::CaptureError;

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_dispatches_to_the_avfoundation_backend() {
        let backend = native();
        assert_eq!(backend.kind(), BackendKind::AvFoundation);
        assert!(backend.kind().is_available());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_dispatches_to_the_media_foundation_backend() {
        let backend = native();
        assert_eq!(backend.kind(), BackendKind::MediaFoundation);
        assert!(backend.kind().is_available());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_dispatches_to_the_v4l2_backend() {
        let backend = native();
        assert_eq!(backend.kind(), BackendKind::V4l2);
        assert!(backend.kind().is_available());
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    fn targets_without_a_backend_report_unsupported() {
        let backend = native();
        assert_eq!(backend.kind(), BackendKind::Unsupported);
        assert!(!backend.kind().is_available());
    }

    /// A backend that is present but finds nothing must say `Ok(vec![])`; a
    /// backend that is absent must say [`CaptureError::UnsupportedPlatform`].
    /// Neither may be substituted for the other.
    #[test]
    fn enumeration_never_conflates_absent_with_empty() {
        #[cfg(target_os = "macos")]
        {
            use crate::error::CaptureError;
            match native().enumerate() {
                // A machine whose user has refused camera access is the one
                // other honest outcome; it is still not "no backend".
                Ok(_) | Err(CaptureError::PermissionDenied { .. }) => {}
                Err(other) => panic!("unexpected AVFoundation enumeration failure: {other:?}"),
            }
        }
        #[cfg(target_os = "windows")]
        {
            use crate::error::CaptureError;
            match native().enumerate() {
                // The Windows camera privacy setting is the one other honest
                // outcome; it is still not "no backend".
                Ok(_) | Err(CaptureError::PermissionDenied { .. }) => {}
                Err(other) => {
                    panic!("unexpected Media Foundation enumeration failure: {other:?}")
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            use crate::error::CaptureError;
            match native().enumerate() {
                // A machine whose `/dev/video*` nodes are all owned by a group
                // this user is not in, and a container with no `/dev` to scan,
                // are the two other honest outcomes; neither is "no backend".
                Ok(_)
                | Err(CaptureError::PermissionDenied { .. })
                | Err(CaptureError::Io { .. }) => {}
                Err(other) => panic!("unexpected V4L2 enumeration failure: {other:?}"),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        match native().enumerate() {
            Err(CaptureError::UnsupportedPlatform { target }) => {
                assert_eq!(target, std::env::consts::OS);
            }
            Ok(devices) => panic!(
                "an unimplemented backend must not claim {} device(s) were found",
                devices.len()
            ),
            Err(other) => panic!("expected UnsupportedPlatform, got {other:?}"),
        }
    }
}
