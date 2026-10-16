//! WebKit through a small Objective-C half (native/webkit.m), on the Mac only.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/webkit.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("native/webkit.m")
        .flag("-fobjc-arc")
        .flag("-fmodules")
        // AppKit's own deprecations, and the SPI it is asked for by name, are not this file's.
        .flag("-Wno-deprecated-declarations")
        .flag("-Wno-unused-parameter")
        .compile("spatimesh_webkit");
    for framework in ["WebKit", "AppKit", "Foundation", "CoreGraphics", "QuartzCore"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
