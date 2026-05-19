//! Build script for oximedia-vaapi-sys.
//!
//! libva is a Linux-only API discovered via pkg-config (`libva`,
//! `libva-drm`, `libva-x11`).  Discovery is **opt-in only**: the
//! workspace's root README promises "no `pkg-config`" out of the
//! box, so without the `pkg-config` cargo feature the build script
//! emits empty bindings and the workspace stays buildable on any
//! host without system VAAPI headers.

use std::env;
use std::path::PathBuf;

#[cfg(feature = "pkg-config")]
fn probe_libva() -> Vec<PathBuf> {
    // Probe each library; we don't require X11 to be present
    // (DRM-only headless builds are common in render farms).
    let mut include_paths: Vec<PathBuf> = Vec::new();
    let mut linked_any = false;

    for pkg in ["libva", "libva-drm", "libva-x11"] {
        if let Ok(l) = pkg_config::Config::new().probe(pkg) {
            include_paths.extend(l.include_paths.iter().cloned());
            linked_any = true;
        }
    }

    if linked_any {
        include_paths
    } else {
        Vec::new()
    }
}

#[cfg(not(feature = "pkg-config"))]
fn probe_libva() -> Vec<PathBuf> {
    Vec::new()
}

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let bindings_path = out_dir.join("bindings.rs");

    if target_os != "linux" {
        std::fs::write(
            &bindings_path,
            format!("// oximedia-vaapi-sys: empty bindings (target_os = {target_os:?})\n"),
        )
        .expect("write empty bindings");
        return;
    }

    let include_paths = probe_libva();

    if include_paths.is_empty() {
        std::fs::write(
            &bindings_path,
            "// oximedia-vaapi-sys: empty bindings (pkg-config feature off or libva not found)\n",
        )
        .expect("write empty bindings");
        return;
    }

    let mut builder = bindgen::Builder::default().header("wrapper.h");
    for inc in &include_paths {
        builder = builder.clang_arg(format!("-I{}", inc.display()));
    }

    let bindings = builder
        .allowlist_function("va.*")
        .allowlist_function("vaapi.*")
        .allowlist_type("VA.*")
        .allowlist_var("VA.*")
        .layout_tests(false)
        .generate_comments(false)
        .derive_default(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed on libva headers");

    bindings
        .write_to_file(&bindings_path)
        .expect("write bindings.rs");

    // pkg-config probes already emitted rustc-link-lib lines; nothing else
    // to do here. We don't add X11 deliberately so the crate links cleanly
    // in DRM-only environments.
}
