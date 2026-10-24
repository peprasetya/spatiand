fn main() {
    println!("cargo:rerun-if-changed=native/surfaces.c");
    println!("cargo:rerun-if-changed=native/surfaces.h");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        cc::Build::new().file("native/surfaces.c").flag_if_supported("-Wno-deprecated-declarations").compile("spatiand_surfaces");
        // The child in this crate's test: a program of its own, as every real child is.
        println!("cargo:rerun-if-changed=native/post-helper.c");
        let out = std::env::var("OUT_DIR").unwrap();
        let helper = format!("{out}/surfaces-post-helper");
        let status = std::process::Command::new("cc")
            .args(["-Inative", "-o", &helper, "native/post-helper.c", "native/surfaces.c", "-framework", "IOSurface", "-framework", "CoreFoundation", "-lbsm", "-Wno-deprecated-declarations"])
            .status()
            .expect("cc");
        assert!(status.success(), "the test's helper did not build");
        println!("cargo:rustc-link-lib=framework=IOSurface");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=bsm");
    }
}
