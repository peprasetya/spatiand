//! Extensions on WebKitGTK, end to end: a Chrome Manifest V3 extension and a Firefox Manifest V2
//! one, their content scripts in their own worlds (crates/spatimesh/tests/common).
//!
//! These need a display for GTK (Xvfb in CI). Without one, or where the program was built
//! without WebKitGTK, they say so and pass.

#[path = "../../spatimesh/tests/common/extensions_engine.rs"]
mod common;

#[test]
fn chromes_and_firefoxs_extensions_run() {
    common::run(env!("CARGO_BIN_EXE_spatimesh-webkitgtk"), false);
}
