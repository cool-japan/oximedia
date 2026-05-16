//! Build script for oximedia-vtb-sys.
//!
//! On macOS/iOS we drive bindgen against the system frameworks and tell
//! rustc to link them. On every other platform we emit an empty bindings
//! file so the crate compiles to a stub and workspace builds elsewhere
//! aren't broken by a missing SDK.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let bindings_path = out_dir.join("bindings.rs");

    if target_os != "macos" && target_os != "ios" {
        // Off-platform: emit an empty bindings module so `include!` succeeds.
        std::fs::write(
            &bindings_path,
            "// oximedia-vtb-sys: empty bindings, target_os != macos|ios\n",
        )
        .expect("write empty bindings");
        return;
    }

    // Locate the macOS SDK via xcrun. This is the canonical path the system
    // toolchain uses; we don't fall back because if xcrun isn't available
    // we wouldn't be able to link against the frameworks anyway.
    let sdk_path = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("xcrun --show-sdk-path failed; install Xcode Command Line Tools");
    if !sdk_path.status.success() {
        panic!(
            "xcrun failed: {}",
            String::from_utf8_lossy(&sdk_path.stderr)
        );
    }
    let sdk_path = String::from_utf8(sdk_path.stdout)
        .expect("xcrun output not UTF-8")
        .trim()
        .to_string();

    // The Apple frameworks compile cleanly with clang; bindgen invokes
    // libclang under the hood. Allowlists keep the generated bindings to
    // the surface we'll actually use — without them we'd bring in tens of
    // thousands of unrelated symbols from CoreFoundation/Cocoa.
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-isysroot{sdk_path}"))
        .clang_arg("-fblocks")
        // Keep #define constants as actual constants in the output.
        .default_macro_constant_type(bindgen::MacroTypeVariation::Signed)
        // Treat opaque Apple types as opaque pointers (no struct fields).
        .opaque_type("CMVideoFormatDescription")
        .opaque_type("CMBlockBuffer")
        .opaque_type("CMSampleBuffer")
        .opaque_type("CVImageBuffer")
        .opaque_type("CVPixelBuffer")
        .opaque_type("CVPixelBufferPool")
        .opaque_type("VTCompressionSession")
        .opaque_type("VTDecompressionSession")
        // Scope the API surface — everything we'd actually call.
        .allowlist_function("CF.*")
        .allowlist_function("CM.*")
        .allowlist_function("CV.*")
        .allowlist_function("VT.*")
        .allowlist_function("AudioConverter.*")
        .allowlist_function("AudioFile.*")
        .allowlist_function("AudioQueue.*")
        .allowlist_type("CF.*")
        .allowlist_type("CM.*")
        .allowlist_type("CV.*")
        .allowlist_type("VT.*")
        .allowlist_type("OSStatus")
        .allowlist_type("AudioStream.*")
        .allowlist_type("AudioConverterRef")
        .allowlist_type("AudioBuffer.*")
        .allowlist_var("kCM.*")
        .allowlist_var("kCV.*")
        .allowlist_var("kVT.*")
        .allowlist_var("kAudio.*")
        .layout_tests(false)
        .generate_comments(false)
        .derive_default(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed on VideoToolbox/AudioToolbox headers");

    bindings
        .write_to_file(&bindings_path)
        .expect("write bindings.rs");

    // Link the frameworks. Order doesn't matter on macOS — the dynamic
    // linker resolves the dependency graph at load time.
    for framework in [
        "CoreFoundation",
        "CoreMedia",
        "CoreVideo",
        "VideoToolbox",
        "AudioToolbox",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
