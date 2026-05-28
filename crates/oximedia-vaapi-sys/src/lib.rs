//! Raw FFI bindings to libva (Video Acceleration API).
//!
//! Mirrors FFmpeg's `libavcodec/vaapi_*.c`: this crate is C-ABI only.
//! Linux exclusive — the API itself is Linux-defined and not portable to
//! macOS or Windows. On other platforms the crate compiles to an empty
//! module so the workspace builds everywhere.

#![allow(clippy::all)]
#![allow(clippy::pedantic)]

#[cfg(target_os = "linux")]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

/// True when libva bindings were generated (i.e. Linux + pkg-config found `libva`).
pub const HAS_BINDINGS: bool = cfg!(target_os = "linux");

#[cfg(test)]
mod tests {
    use super::HAS_BINDINGS;

    #[test]
    fn not_present_off_linux() {
        if !cfg!(target_os = "linux") {
            assert!(!HAS_BINDINGS);
        }
    }
}
