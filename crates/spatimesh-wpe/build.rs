//! WPE WebKit through a small C half (native/wpe.c), compiled against the headers of the WPE
//! third_party/wpe/build.sh made.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/wpe.c");
    println!("cargo:rerun-if-env-changed=SPATIAND_WPE");
    println!("cargo:rustc-check-cfg=cfg(wpe)");
    let platform = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => "mac",
        Ok("android") => "android",
        _ => "linux",
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let wpe = std::env::var("SPATIAND_WPE").unwrap_or_else(|_| format!("{home}/Developer/spatiand-wpe/out/{platform}"));
    let pkgconfig = format!("{wpe}/lib/pkgconfig");
    let path = std::env::var("PKG_CONFIG_PATH").map(|p| format!("{pkgconfig}:{p}")).unwrap_or(pkgconfig);
    std::env::set_var("PKG_CONFIG_PATH", path);
    match pkg_config::Config::new().probe("wpe-webkit-2.0") {
        Ok(library) => {
            let mut build = cc::Build::new();
            build.file("native/wpe.c");
            for path in &library.include_paths {
                build.include(path);
            }
            // epoxy/egl.h and xkbcommon, which WPE's headers include, are beside WPE's own.
            build.include(format!("{wpe}/include"));
            // GObject's signal handlers take what the signal gives, wanted or not.
            build.flag_if_supported("-Wno-unused-parameter");
            build.compile("spatimesh_wpe");
            // libepoxy, whose EGL this calls for SpatiMesh's WPE display.
            println!("cargo:rustc-link-search=native={wpe}/lib");
            println!("cargo:rustc-link-lib=dylib=epoxy");
            // The platform's interface font, by fontconfig's name for it (and on a Mac,
            // CoreText's choice of it, and the Mac's Dark Mode).
            println!("cargo:rustc-link-lib=dylib=fontconfig");
            if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
                println!("cargo:rustc-link-lib=framework=CoreText");
                println!("cargo:rustc-link-lib=framework=CoreFoundation");
            }
            // Where WPE is, for this program to find it: where it was built, and in the app --
            // on Linux, the wpe/ folder beside this program (as DT_RPATH, which also serves the
            // libraries it loads; on a Mac the app's own path is added by mac/make-app.sh).
            println!("cargo:rustc-link-arg=-Wl,-rpath,{wpe}/lib");
            // Android's pictures are AHardwareBuffers, read back here (native/wpe.c).
            if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
                println!("cargo:rustc-link-lib=dylib=nativewindow");
            }
            if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
                println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/wpe/lib");
                println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
            }
            println!("cargo:rustc-cfg=wpe");
            println!("cargo:rustc-env=SPATIMESH_WPE_DIR={wpe}");
        }
        Err(e) => println!("cargo:warning=no WPE WebKit at {wpe} (third_party/wpe/build.sh {platform}): {e}; spatimesh-wpe is built without it"),
    }
}
