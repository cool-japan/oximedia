//! Raw FFI bindings to Intel oneVPL (Video Processing Library, formerly
//! the Media SDK + QSV stack).
//!
//! Mirrors FFmpeg's `libavcodec/qsvenc.c` / `qsvdec.c` approach. Linking is
//! deferred to the `vpl` import library (set up by `build.rs` when the SDK
//! is available); on platforms where the SDK is absent this crate compiles
//! to an empty module.

#![allow(clippy::all)]
#![allow(clippy::pedantic)]

#[cfg(any(target_os = "linux", target_os = "windows"))]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

/// True if `build.rs` found oneVPL headers and emitted real bindings.
pub const HAS_BINDINGS: bool = cfg!(any(target_os = "linux", target_os = "windows"))
    && (option_env!("VPL_ROOT").is_some()
        || option_env!("OXIMEDIA_VPL_DETECTED").is_some());

#[cfg(test)]
mod tests {
    use super::HAS_BINDINGS;

    #[test]
    fn off_target_has_no_bindings() {
        if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
            assert!(!HAS_BINDINGS);
        }
    }
}
