//! Where the linker finds libxkbcommon on a Mac, which has not got one: `mac/xkbcommon/build.sh`'s,
//! the same one the compositor links. On the Deck it is the system's, and on Android
//! `android/build.sh` says where.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mac/xkbcommon/out");
    if !out.join("libxkbcommon.a").exists() {
        println!("cargo:warning=mac/xkbcommon/out/libxkbcommon.a is missing: run mac/xkbcommon/build.sh");
    }
    println!("cargo:rustc-link-search=native={}", out.display());
}
