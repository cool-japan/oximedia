//! Build script for oximedia-amf-sys.
//!
//! AMD's AMF SDK is a header-only C/C++ tree distributed from GitHub. The
//! install path is supplied via `AMF_ROOT`; we look for the public include
//! directory inside it (`amf/public/include`). The runtime library
//! (`amfrt64.dll` / `libamfrt64.so.1`) is dlopen'd at use-time, so we
//! don't emit a `rustc-link-lib` line.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AMF_ROOT");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let bindings_path = out_dir.join("bindings.rs");

    let supported = matches!(target_os.as_str(), "linux" | "windows");
    let root = env::var("AMF_ROOT").ok();

    let stub_reason = match (supported, &root) {
        (false, _) => Some(format!("target_os = {target_os:?} (AMF is Linux/Windows only)")),
        (true, None) => Some("AMF_ROOT not set".to_string()),
        _ => None,
    };

    if let Some(reason) = stub_reason {
        std::fs::write(
            &bindings_path,
            format!("// oximedia-amf-sys: empty bindings ({reason})\n"),
        )
        .expect("write empty bindings");
        return;
    }

    let root = PathBuf::from(root.expect("checked above"));
    let include_dir = root.join("amf").join("public").join("include");
    if !include_dir.join("core").join("Factory.h").exists() {
        panic!(
            "AMF_ROOT={} does not contain amf/public/include/core/Factory.h",
            root.display()
        );
    }

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", include_dir.display()))
        // AMF headers are C++; tell bindgen to parse with the C++ frontend.
        .clang_arg("-x")
        .clang_arg("c++")
        .clang_arg("-std=c++17")
        .allowlist_function("AMF.*")
        .allowlist_function("amf_.*")
        .allowlist_type("AMF.*")
        .allowlist_type("amf_.*")
        .allowlist_var("AMF.*")
        .allowlist_var("amf_.*")
        .opaque_type("std::.*")
        .layout_tests(false)
        .generate_comments(false)
        .derive_default(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed on AMF headers");

    bindings
        .write_to_file(&bindings_path)
        .expect("write bindings.rs");
}
