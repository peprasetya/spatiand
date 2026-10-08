//! Where the linker finds libxkbcommon, which macOS has not got: `mac/xkbcommon/build.sh`'s.

fn main() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mac/xkbcommon/out");
    if !out.join("libxkbcommon.a").exists() {
        println!("cargo:warning=mac/xkbcommon/out/libxkbcommon.a is missing: run mac/xkbcommon/build.sh");
    }
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");
}
