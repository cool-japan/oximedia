//! Raw FFI bindings to the NVIDIA Video Codec SDK (NVENC + NVDEC).
//!
//! Mirrors FFmpeg's `libavcodec/nvenc.c` and `libavcodec/cuviddec.c`: this
//! crate exposes the C ABI only. Higher-level lifetime-safe wrappers
//! belong in a separate crate.
//!
//! - Linux + Windows: bindings generated at build time by `build.rs` when
//!   `NV_CODEC_SDK` is set to the SDK install path. Loading the runtime
//!   library is left to the caller (the SDK is dlopen'd in practice so
//!   apps can degrade gracefully on machines without an NVIDIA driver).
//! - Other platforms: this crate compiles to an empty module — `cargo
//!   build` still succeeds on the workspace, and code that conditionally
//!   uses NVENC should gate on `cfg(any(target_os = "linux", target_os = "windows"))`
//!   and check `oximedia_nvenc_sys::HAS_BINDINGS`.

#![allow(clippy::all)]
#![allow(clippy::pedantic)]

#[cfg(any(target_os = "linux", target_os = "windows"))]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

/// True when bindings were generated. Mirrors what `build.rs` decided.
pub const HAS_BINDINGS: bool = {
    // If the bindings file is non-empty bindgen output we treat ourselves
    // as having bindings. The most reliable signal at compile time is the
    // platform cfg AND the env var presence; we approximate at runtime by
    // exposing the cfg.
    cfg!(any(target_os = "linux", target_os = "windows"))
        && option_env!("NV_CODEC_SDK").is_some()
};

#[cfg(test)]
mod tests {
    use super::HAS_BINDINGS;

    #[test]
    fn off_apple_compiles_to_stub_or_real() {
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            assert!(!HAS_BINDINGS, "NVENC must stub on Apple platforms");
        }
        // On Linux/Windows the value depends on whether NV_CODEC_SDK was set
        // at build time; we don't assert either way to avoid coupling CI to
        // local installs.
    }
}
