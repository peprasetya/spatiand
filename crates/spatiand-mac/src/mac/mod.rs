//! What is the Mac's: where the picture goes and where a window's picture comes from.

pub mod egl;
pub mod panel;
pub mod remote_video;

/// Say where what the app carries is, for the libraries that look in the environment: the
/// keyboard layouts libxkbcommon reads. Before anything else, since a compositor with no keymap
/// has no keyboard.
pub fn prepare() {
    // Where the compositor's socket goes. macOS has no runtime directory; the user's own
    // temporary one is as private.
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        let dir = std::env::temp_dir().join("spatiand-runtime");
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
    }
    if std::env::var_os("XKB_CONFIG_ROOT").is_none() {
        let exe = std::env::current_exe().ok();
        let dir = exe.as_deref().and_then(|e| e.parent());
        let candidates = [
            dir.map(|d| d.join("../Resources/xkb")),
            Some(std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../mac/xkbcommon/out/xkb"))),
        ];
        if let Some(found) = candidates.iter().flatten().find(|p| p.join("rules/evdev").exists()) {
            std::env::set_var("XKB_CONFIG_ROOT", found);
        }
    }
}
