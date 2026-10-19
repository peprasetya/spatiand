//! Steam's Chromium through a small C shim (native/cef.c), compiled against the headers of the
//! CEF version Steam ships, from the CEF distribution `CEF_ROOT` names (tools/fetch-cef.sh), so
//! that no CEF structure is written out by hand. libcef.so is not linked: the program loads
//! Steam's when it starts. Linux only; without CEF_ROOT the program is built without it
//! (`cfg(cef)` unset) and says so when started.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/cef.c");
    println!("cargo:rerun-if-env-changed=CEF_ROOT");
    println!("cargo:rustc-check-cfg=cfg(cef)");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let Some(root) = std::env::var_os("CEF_ROOT").map(PathBuf::from) else { return };
    if !root.join("include/capi/cef_app_capi.h").is_file() {
        println!("cargo:warning=CEF_ROOT ({}) has no CEF headers; spatimesh-cef is built without Chromium", root.display());
        return;
    }
    cc::Build::new()
        .file("native/cef.c")
        .include(&root)
        .flag("-std=gnu11")
        // A handler whose arguments are not the ones this CEF passes is a fault, not a warning.
        .flag("-Werror=incompatible-pointer-types")
        // CEF's callbacks take what they are given, wanted or not.
        .flag_if_supported("-Wno-unused-parameter")
        .compile("spatimesh_cef");
    println!("cargo:rustc-link-lib=dl");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-cfg=cef");
}
