//! Build script for oximedia-vpl-sys.
//!
//! Intel oneVPL is the replacement for the old Media SDK / libmfx and is
//! the supported QSV (Quick Sync Video) entry point. It ships as a
//! pkg-config-discoverable library named `vpl`.
//!
//! Discovery order:
//! 1. `VPL_ROOT` env var → header at `${VPL_ROOT}/include/vpl/mfx.h`.
//! 2. `pkg-config --cflags --libs vpl`.
//!
//! If neither is available (or we're on a non-Linux/Windows target),
//! we emit empty bindings so the workspace still builds.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=VPL_ROOT");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    let bindings_path = out_dir.join("bindings.rs");

    if !matches!(target_os.as_str(), "linux" | "windows") {
        std::fs::write(
            &bindings_path,
            format!("// oximedia-vpl-sys: empty bindings (target_os = {target_os:?})\n"),
        )
        .expect("write empty bindings");
        return;
    }

    // Try VPL_ROOT first; fall through to pkg-config; otherwise stub.
    let include_dirs: Vec<PathBuf> = if let Ok(root) = env::var("VPL_ROOT") {
        vec![PathBuf::from(root).join("include")]
    } else {
        match pkg_config::Config::new().probe("vpl") {
            Ok(lib) => lib.include_paths,
            Err(_) => {
                std::fs::write(
                    &bindings_path,
                    "// oximedia-vpl-sys: empty bindings (no VPL_ROOT, no pkg-config 'vpl')\n",
                )
                .expect("write empty bindings");
                return;
            }
        }
    };

    let mut builder = bindgen::Builder::default().header("wrapper.h");
    for inc in &include_dirs {
        builder = builder.clang_arg(format!("-I{}", inc.display()));
    }

    let bindings = builder
        .allowlist_function("MFX.*")
        .allowlist_type("mfx.*")
        .allowlist_type("MFX.*")
        .allowlist_var("MFX_.*")
        .layout_tests(false)
        .generate_comments(false)
        .derive_default(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed on oneVPL headers");

    bindings
        .write_to_file(&bindings_path)
        .expect("write bindings.rs");

    // Link the dispatcher; the runtime drivers it loads belong to the user.
    println!("cargo:rustc-link-lib=vpl");
}
