//! Build script for oximedia-vpl-sys.
//!
//! Intel oneVPL is the replacement for the old Media SDK / libmfx and is
//! the supported QSV (Quick Sync Video) entry point.
//!
//! Discovery is intentionally env-var only — no `pkg-config`, no
//! system probing — to honour the workspace README's "one cargo add,
//! no system library installations" promise.
//!
//! If `VPL_ROOT` is set we generate real bindings against
//! `${VPL_ROOT}/include/vpl/mfx.h` and link `-lvpl`.  Otherwise we
//! emit an empty bindings file so the workspace stays buildable on
//! every host.  Callers that want hardware-accelerated QSV install
//! oneVPL themselves and set `VPL_ROOT`.

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

    let Ok(root) = env::var("VPL_ROOT") else {
        std::fs::write(
            &bindings_path,
            "// oximedia-vpl-sys: empty bindings (VPL_ROOT not set)\n",
        )
        .expect("write empty bindings");
        return;
    };
    let include_dirs = [PathBuf::from(&root).join("include")];

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

    // Link the dispatcher; the runtime drivers it loads belong to
    // the user.  Add VPL_ROOT/lib to the search path so the linker
    // finds the dispatcher without a system-wide install.
    println!("cargo:rustc-link-search=native={}/lib", root);
    println!("cargo:rustc-link-lib=vpl");
}
