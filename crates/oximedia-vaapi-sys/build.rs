//! Build script for oximedia-vaapi-sys.
//!
//! libva is a Linux-only API typically discovered via pkg-config (`libva`,
//! `libva-drm`, `libva-x11`). When pkg-config isn't installed or the
//! package is missing — or we're not on Linux — we emit empty bindings so
//! the workspace still builds.

use std::env;
use std::path::PathBuf;

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

    // Probe each library; we don't require X11 to be present (DRM-only
    // headless builds are common in render farms).
    let libva = pkg_config::Config::new().probe("libva");
    let libva_drm = pkg_config::Config::new().probe("libva-drm");
    let libva_x11 = pkg_config::Config::new().probe("libva-x11");

    let mut include_paths: Vec<PathBuf> = Vec::new();
    let mut linked_any = false;

    if let Ok(l) = &libva {
        include_paths.extend(l.include_paths.iter().cloned());
        linked_any = true;
    }
    if let Ok(l) = &libva_drm {
        include_paths.extend(l.include_paths.iter().cloned());
        linked_any = true;
    }
    if let Ok(l) = &libva_x11 {
        include_paths.extend(l.include_paths.iter().cloned());
        linked_any = true;
    }

    if !linked_any {
        std::fs::write(
            &bindings_path,
            "// oximedia-vaapi-sys: empty bindings (pkg-config 'libva' not found)\n",
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
