//! Raw FFI bindings to AMD AMF (Advanced Media Framework).
//!
//! Mirrors FFmpeg's `libavcodec/amfenc.c` approach. AMF is technically a
//! C++ SDK, but its public ABI is the C `amf_factory` dispatch surface,
//! which bindgen produces cleanly when invoked with `clang -x c++`. At
//! runtime callers `dlopen` `amfrt64.so.1` / `amfrt64.dll`.

#![allow(clippy::all)]
#![allow(clippy::pedantic)]

#[cfg(any(target_os = "linux", target_os = "windows"))]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

/// True if `AMF_ROOT` was set at build time and bindings were emitted.
pub const HAS_BINDINGS: bool = cfg!(any(target_os = "linux", target_os = "windows"))
    && option_env!("AMF_ROOT").is_some();

#[cfg(test)]
mod tests {
    use super::HAS_BINDINGS;

    #[test]
    fn not_present_on_apple() {
        if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
            assert!(!HAS_BINDINGS);
        }
    }
}
