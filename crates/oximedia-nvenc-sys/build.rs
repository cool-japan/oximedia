//! Build script for oximedia-nvenc-sys.
//!
//! Generates Rust bindings to the NVIDIA Video Codec SDK when:
//! 1. The target platform is Linux or Windows (Apple silicon and macOS have
//!    no NVIDIA driver path; Linux/Windows is where NVENC/NVDEC actually run).
//! 2. The `NV_CODEC_SDK` environment variable points at the SDK install
//!    (must contain `Interface/nvEncodeAPI.h`).
//!
//! Without those preconditions an empty bindings file is generated so the
//! workspace builds everywhere; downstream callers should gate use of any
//! symbol from this crate on `cfg(not(target_os = "macos"))` and confirm
//! `oximedia_nvenc_sys::HAS_BINDINGS` at runtime.
//!
//! Linking: NVENC loads dynamically through `libnvidia-encode.so.1` /
//! `nvEncodeAPI64.dll`. We tell rustc to link the import library when
//! available; otherwise the user is expected to `dlopen`/`LoadLibrary`
//! themselves.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=NV_CODEC_SDK");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let bindings_path = out_dir.join("bindings.rs");

    let supported = matches!(target_os.as_str(), "linux" | "windows");
    let sdk_path = env::var("NV_CODEC_SDK").ok();

    let stub_reason = match (supported, &sdk_path) {
        (false, _) => Some(format!("target_os = {target_os:?} (NVENC is Linux/Windows only)")),
        (true, None) => Some("NV_CODEC_SDK not set".to_string()),
        _ => None,
    };

    if let Some(reason) = stub_reason {
        std::fs::write(
            &bindings_path,
            format!("// oximedia-nvenc-sys: empty bindings ({reason})\n"),
        )
        .expect("write empty bindings");
        return;
    }

    let sdk_path = sdk_path.expect("checked above");
    let interface_dir = PathBuf::from(&sdk_path).join("Interface");
    if !interface_dir.join("nvEncodeAPI.h").exists() {
        panic!(
            "NV_CODEC_SDK={sdk_path} does not contain Interface/nvEncodeAPI.h"
        );
    }

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", interface_dir.display()))
        .allowlist_function("NvEnc.*")
        .allowlist_function("cuvid.*")
        .allowlist_type("NV_ENC.*")
        .allowlist_type("NV_ENCODE.*")
        .allowlist_type("CUVIDEO.*")
        .allowlist_type("CUvideo.*")
        .allowlist_type("CUvideopacketflags")
        .allowlist_type("CUVIDPARSERPARAMS")
        .allowlist_type("CUVIDPARSERDISPINFO")
        .allowlist_type("CUVIDPICPARAMS")
        .allowlist_var("NV_ENC.*")
        .allowlist_var("NVENC.*")
        .layout_tests(false)
        .generate_comments(false)
        .derive_default(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed on NVIDIA Video Codec SDK headers");

    bindings
        .write_to_file(&bindings_path)
        .expect("write bindings.rs");

    // The NVENC API is dispatched through a single `NvEncodeAPICreateInstance`
    // function — most users dlopen() the library at runtime so they can
    // diagnose missing-driver scenarios gracefully. We deliberately do NOT
    // emit a `rustc-link-lib` line so this crate doesn't fail to link on
    // build hosts that don't have the runtime library installed.
}
