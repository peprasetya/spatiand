//! WebKit through a small Objective-C half (native/webkit.m), on the Mac only.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/webkit.m");
    println!("cargo:rerun-if-changed=native/sound.m");
    println!("cargo:rerun-if-changed=Info.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("native/webkit.m")
        .file("native/sound.m")
        .flag("-fobjc-arc")
        .flag("-fmodules")
        // AppKit's own deprecations, and the SPI it is asked for by name, are not this file's.
        .flag("-Wno-deprecated-declarations")
        .flag("-Wno-unused-parameter")
        // macOS 14.2's process taps, used where they exist (sound.m asks first).
        .flag("-Wno-unguarded-availability-new")
        .compile("spatimesh_webkit");
    for framework in ["WebKit", "AppKit", "Foundation", "CoreGraphics", "QuartzCore", "CoreAudio"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    // A browser loads plain http:// pages too, which App Transport Security allows only where
    // the program's Info.plist says so; a program outside an app bundle carries its Info.plist
    // in its binary (each argument passed to the linker on its own, whatever the path holds).
    let plist = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("Info.plist");
    let plist = plist.to_string_lossy().into_owned();
    for arg in ["-sectcreate", "__TEXT", "__info_plist", plist.as_str()] {
        println!("cargo:rustc-link-arg-bins=-Xlinker");
        println!("cargo:rustc-link-arg-bins={arg}");
    }
}
