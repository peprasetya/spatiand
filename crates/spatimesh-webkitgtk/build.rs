//! WebKitGTK through a small C shim (native/webkitgtk.c), compiled against the system's own
//! headers so that no GTK or WebKit structure is written out by hand. Linux only; where
//! webkit2gtk-4.1 is not installed the program is built without it (`cfg(webkitgtk)` unset).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/webkitgtk.c");
    println!("cargo:rustc-check-cfg=cfg(webkitgtk)");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    match pkg_config::Config::new().atleast_version("2.40").probe("webkit2gtk-4.1") {
        Ok(library) => {
            let mut build = cc::Build::new();
            build.file("native/webkitgtk.c");
            for path in &library.include_paths {
                build.include(path);
            }
            // The GDK event fields this fills in are GTK 3's public structure, and stay.
            build.flag_if_supported("-Wno-deprecated-declarations");
        // GTK signal handlers take what the signal gives, wanted or not.
        build.flag_if_supported("-Wno-unused-parameter");
            build.compile("spatimesh_webkitgtk");
            println!("cargo:rustc-cfg=webkitgtk");
        }
        Err(e) => println!("cargo:warning=no WebKitGTK here (webkit2gtk-4.1): {e}; spatimesh-webkitgtk is built without it"),
    }
}
