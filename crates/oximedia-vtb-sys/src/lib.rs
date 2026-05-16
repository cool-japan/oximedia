//! Raw FFI bindings to Apple VideoToolbox and AudioToolbox.
//!
//! Mirrors FFmpeg's `libavcodec/videotoolboxenc.c` /
//! `libavcodec/audiotoolboxenc.c` approach: a `-sys` crate that re-exports
//! the C ABI verbatim so higher layers can build safe wrappers on top.
//!
//! - On macOS / iOS: bindings are generated at build time by
//!   `build.rs` via bindgen, against the system SDK headers located by
//!   `xcrun --sdk macosx --show-sdk-path`. The five required frameworks
//!   are linked automatically.
//! - On other platforms: this crate compiles to an empty module so
//!   `cargo build` on the workspace still succeeds; downstream callers
//!   should gate use behind `cfg(any(target_os = "macos", target_os = "ios"))`.
//!
//! Higher-level safe wrappers (compression session, decompression session,
//! audio converter) belong in a separate crate that depends on this one.

#![allow(clippy::all)]
#![allow(clippy::pedantic)]

#[cfg(any(target_os = "macos", target_os = "ios"))]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

/// True when this crate was built with real bindings against the Apple SDK.
pub const HAS_BINDINGS: bool = cfg!(any(target_os = "macos", target_os = "ios"));

#[cfg(test)]
mod tests {
    use super::HAS_BINDINGS;

    #[test]
    fn bindings_present_on_apple_platforms() {
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            assert!(HAS_BINDINGS, "expected bindings to be generated on Apple");
        } else {
            assert!(!HAS_BINDINGS);
        }
    }

    /// Smoke test on Apple platforms: round-trip a CFString through the
    /// VideoToolbox frameworks. If linking, sysroot resolution, and bindgen
    /// allowlists are all wired correctly, this exercises CoreFoundation
    /// without touching any encoder state.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn cfstring_round_trip() {
        use super::*;
        // CFStringBuiltInEncodings::kCFStringEncodingUTF8 — bindgen emits the
        // enum value with this constant, but the exact path varies by
        // bindgen version; the underlying value is stable per Apple's headers.
        const K_UTF8: u32 = 0x0800_0100;
        unsafe {
            let raw = b"hello-vtb\0";
            let s = CFStringCreateWithCString(
                std::ptr::null(),
                raw.as_ptr() as *const _,
                K_UTF8,
            );
            assert!(!s.is_null());
            let len = CFStringGetLength(s);
            assert_eq!(len as usize, raw.len() - 1);
            CFRelease(s as *const _);
        }
    }
}
