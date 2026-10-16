//! Extensions on the Mac's WebKit, end to end: a Chrome Manifest V3 extension and a Firefox
//! Manifest V2 one, their content scripts in WKContentWorlds of their own
//! (crates/spatimesh/tests/common).
//!
//! These need a Mac whose session has a window server (CI's macOS runners have one). Where the
//! engine cannot start they say so and pass, unless `SPATIMESH_WEBKIT_REQUIRED` is set, as in CI.
#![cfg(target_os = "macos")]

#[path = "../../spatimesh/tests/common/extensions_engine.rs"]
mod common;

#[test]
fn chromes_and_firefoxs_extensions_run() {
    common::run(env!("CARGO_BIN_EXE_spatimesh-webkit"), spatimesh::extensions::set::Addresses::Browsers, std::env::var_os("SPATIMESH_WEBKIT_REQUIRED").is_some());
}
