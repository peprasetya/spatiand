//! Extensions on Steam's Chromium (CEF), end to end: a Chrome Manifest V3 extension and a Firefox
//! Manifest V2 one, their content scripts in the page's world in closures of their own
//! (crates/spatimesh/tests/common).
//!
//! These need CEF (tools/fetch-cef.sh, `SPATIMESH_CEF_DIR`), the program built against its
//! headers (`CEF_ROOT`), and a display (Xvfb in CI). Without them they say so and pass, unless
//! `SPATIMESH_CEF_REQUIRED` is set, as in CI.

#[path = "../../spatimesh/tests/common/extensions_engine.rs"]
mod common;

#[test]
fn chromes_and_firefoxs_extensions_run() {
    common::run(env!("CARGO_BIN_EXE_spatimesh-cef"), std::env::var_os("SPATIMESH_CEF_REQUIRED").is_some());
}
