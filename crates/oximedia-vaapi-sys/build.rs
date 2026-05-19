//! Build script for oximedia-vaapi-sys.
//!
//! libva is a Linux-only API.  Discovery is intentionally env-var
//! only — no `pkg-config`, no system probing — to honour the
//! workspace README's "one cargo add, no system library
//! installations" promise.
//!
//! If `LIBVA_ROOT` is set we generate real bindings against
//! `${LIBVA_ROOT}/include/va/va.h` and link `-lva`.  Otherwise we
//! emit an empty bindings file so the workspace stays buildable on
//! every host.  Callers that want VAAPI acceleration install libva
//! themselves and set `LIBVA_ROOT` (and optionally use DRM-only or
//! X11 wrappers via `LIBVA_LINK_EXTRA`).

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LIBVA_ROOT");
    println!("cargo:rerun-if-env-changed=LIBVA_LINK_EXTRA");

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

    let Ok(root) = env::var("LIBVA_ROOT") else {
        std::fs::write(
            &bindings_path,
            "// oximedia-vaapi-sys: empty bindings (LIBVA_ROOT not set)\n",
        )
        .expect("write empty bindings");
        return;
    };
    let include_paths = [PathBuf::from(&root).join("include")];

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

    println!("cargo:rustc-link-search=native={}/lib", root);
    println!("cargo:rustc-link-lib=va");
    // Optional extras: callers needing DRM-only or X11 entry points
    // pass them via LIBVA_LINK_EXTRA (e.g. "va-drm va-x11").  Each
    // whitespace-separated token becomes one `-l<lib>` directive.
    if let Ok(extra) = env::var("LIBVA_LINK_EXTRA") {
        for lib in extra.split_whitespace() {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
