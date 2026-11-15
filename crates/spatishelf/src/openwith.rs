//! "Open with": the applications of this machine that open a kind of file, Spatiand's own among
//! them, the one a double click uses first; any other application ("Other..."); and "always
//! open this kind with it", remembered for this machine alone (applications differ from one
//! machine to the next, so it is not in the settings Spatiand shares between them).
//!
//! The menu, the memory and the choice are this file's, the same everywhere. What differs is
//! only how a system says which applications open what, and how one is started:
//!
//! * Linux (the Deck): the freedesktop way (`openwith_linux.rs`): the file's MIME type from
//!   shared-mime-info, the desktop entries that list it (Flatpak's exported ones too), the
//!   default from `mimeapps.list`, started by the entry's `Exec` line as written.
//! * The Mac: Launch Services (`openwith_mac.rs`), started with `open -a`.
//! * Android (the Beam Pro): only the APK's own Java may ask the PackageManager, so the room is
//!   asked, over its protocol (`openwith_android.rs`), and the app opens in the room.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg(target_os = "android")]
#[path = "openwith_android.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "openwith_mac.rs"]
mod platform;
#[cfg(not(any(target_os = "macos", target_os = "android")))]
#[path = "openwith_linux.rs"]
pub mod platform;

/// An application of this machine's own that opens files.
#[derive(Debug, Clone, PartialEq)]
pub struct App {
    /// How the system knows it: a desktop entry's id, an application's path, an activity.
    pub id: String,
    pub name: String,
    /// The system's own choice for this kind of file.
    pub system_default: bool,
    /// Whether it takes addresses (`http:`) as well as files: a remote's file can be handed to
    /// it by its WebDAV address instead of being fetched first.
    pub takes_urls: bool,
}

/// What a file is opened with.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Choice {
    /// One of Spatiand's own: SpatiLoupe, SpatiReel, SpatiQuill.
    Own(String),
    /// One of the system's, by its id.
    System(String),
}

/// The kind of file a path is, as this system tells kinds apart: a MIME type on Linux and
/// Android, the ending on a Mac. What "always open with" is remembered by.
pub fn type_of(path: &Path, folder: bool) -> String {
    platform::type_of(path, folder)
}

/// The system's applications for a kind of file, the system's own choice first.
pub fn apps_for(path: &Path, kind: &str, room: &mut spatiand_app::App) -> Vec<App> {
    platform::apps_for(path, kind, room)
}

/// Every application there is, for "Other...", by name.
pub fn all_apps(room: &mut spatiand_app::App) -> Vec<App> {
    let mut apps = platform::all_apps(room);
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.id == b.id);
    apps
}

/// Start `app` with these files (paths, or addresses where it takes them).
pub fn launch(app: &App, files: &[String], room: &mut spatiand_app::App) -> Result<(), String> {
    platform::launch(app, files, room)
}

/// An application's picture, about `size` pixels a side, as RGBA (alpha not multiplied in).
pub fn icon(app: &App, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    platform::icon(app, size)
}

/// A picture file -- PNG, SVG, or anything `image` reads -- as RGBA, `size` pixels a side at
/// most.
pub fn picture_file(path: &Path, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let ending = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ending == "svg" || ending == "svgz" {
        let data = std::fs::read(path).ok()?;
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let (w, h) = (tree.size().width(), tree.size().height());
        let scale = size as f32 / w.max(h).max(1.0);
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)?;
        let offset = ((size as f32 - w * scale) * 0.5, (size as f32 - h * scale) * 0.5);
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(offset.0, offset.1), &mut pixmap.as_mut());
        // tiny-skia's pixels have alpha multiplied in; a texture here wants it not.
        let mut rgba = pixmap.take();
        for p in rgba.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        return Some((size, size, rgba));
    }
    let image = image::open(path).ok()?;
    pixels_of(image, size)
}

pub fn pixels_of(image: image::DynamicImage, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let small = if image.width() > size || image.height() > size { image.thumbnail(size, size) } else { image };
    let small = small.to_rgba8();
    let (w, h) = small.dimensions();
    Some((w, h, small.into_raw()))
}

// --- what is remembered ------------------------------------------------------------------

/// "Always open with", per kind of file, for this machine alone.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Always {
    pub chosen: BTreeMap<String, Choice>,
}

impl Always {
    fn file() -> PathBuf {
        spatiand_app::config::local_file("spatishelf-open-with.json", None)
    }

    pub fn load() -> Always {
        Always::read(&Always::file())
    }

    pub fn read(path: &Path) -> Always {
        let chosen = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Always { chosen }
    }

    pub fn save(&self) {
        self.write(&Always::file());
    }

    pub fn write(&self, path: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.chosen) {
            let _ = std::fs::write(path, bytes);
        }
    }

    pub fn get(&self, kind: &str) -> Option<&Choice> {
        self.chosen.get(kind)
    }

    pub fn set(&mut self, kind: &str, choice: Choice) {
        self.chosen.insert(kind.to_string(), choice);
    }

    pub fn forget(&mut self, kind: &str) -> bool {
        self.chosen.remove(kind).is_some()
    }
}

/// What a double click (or Enter) opens a kind with: what was chosen to always open it, else
/// Spatiand's own application for it, else the system's own choice.
pub fn first_choice(always: Option<&Choice>, own: Option<&str>, system: &[App]) -> Option<Choice> {
    if let Some(choice) = always {
        return Some(choice.clone());
    }
    if let Some(own) = own {
        return Some(Choice::Own(own.to_string()));
    }
    system.iter().find(|a| a.system_default).or(system.first()).map(|a| Choice::System(a.id.clone()))
}

/// The menu's applications in order: the double click's first, then Spatiand's own, then the
/// system's (its own choice first), each once.
pub fn ordered(first: Option<&Choice>, own: &[&str], system: &[App]) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::new();
    let add = |c: Choice, out: &mut Vec<Choice>| {
        if !out.contains(&c) {
            out.push(c);
        }
    };
    if let Some(f) = first {
        add(f.clone(), &mut out);
    }
    for o in own {
        add(Choice::Own(o.to_string()), &mut out);
    }
    for a in system.iter().filter(|a| a.system_default) {
        add(Choice::System(a.id.clone()), &mut out);
    }
    for a in system {
        add(Choice::System(a.id.clone()), &mut out);
    }
    out
}

/// What "always" is said of in the menu: the ending (".jpg files") where there is one.
pub fn kind_words(name: &str, kind: &str, folder: bool) -> String {
    if folder {
        return "folders".into();
    }
    match name.rsplit_once('.') {
        Some((stem, ending)) if !stem.is_empty() && !ending.is_empty() && !ending.contains(' ') => format!(".{} files", ending.to_lowercase()),
        _ => format!("files of this kind ({kind})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, default: bool) -> App {
        App { id: id.into(), name: id.into(), system_default: default, takes_urls: false }
    }

    #[test]
    fn a_double_click_opens_with_what_was_chosen_then_our_own_then_the_systems() {
        let system = [app("gimp", false), app("eog", true)];
        let always = Choice::System("gimp".into());
        assert_eq!(first_choice(Some(&always), Some("spatiloupe"), &system), Some(always.clone()));
        assert_eq!(first_choice(None, Some("spatiloupe"), &system), Some(Choice::Own("spatiloupe".into())));
        assert_eq!(first_choice(None, None, &system), Some(Choice::System("eog".into())));
        assert_eq!(first_choice(None, None, &[app("only", false)]), Some(Choice::System("only".into())));
        assert_eq!(first_choice(None, None, &[]), None);
    }

    #[test]
    fn the_menu_has_each_once_the_first_first() {
        let system = [app("gimp", false), app("eog", true)];
        let first = Choice::System("gimp".into());
        let order = ordered(Some(&first), &["spatiloupe"], &system);
        assert_eq!(order, [Choice::System("gimp".into()), Choice::Own("spatiloupe".into()), Choice::System("eog".into())]);
        let order = ordered(Some(&Choice::Own("spatiloupe".into())), &["spatiloupe"], &system);
        assert_eq!(order, [Choice::Own("spatiloupe".into()), Choice::System("eog".into()), Choice::System("gimp".into())], "the system's own choice before its others");
    }

    #[test]
    fn always_is_remembered_and_forgotten() {
        let file = std::env::temp_dir().join(format!("spatishelf-always-{}.json", std::process::id()));
        let mut always = Always::default();
        always.set("image/jpeg", Choice::System("org.gimp.GIMP.desktop".into()));
        always.set("ext:pdf", Choice::Own("spatiloupe".into()));
        always.write(&file);
        let back = Always::read(&file);
        assert_eq!(back, always);
        let mut back = back;
        assert!(back.forget("image/jpeg") && !back.forget("image/jpeg"));
        assert_eq!(back.get("ext:pdf"), Some(&Choice::Own("spatiloupe".into())));
        assert_eq!(Always::read(Path::new("/nowhere/at/all.json")), Always::default());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn what_always_is_said_of() {
        assert_eq!(kind_words("Holiday.JPG", "image/jpeg", false), ".jpg files");
        assert_eq!(kind_words("README", "text/plain", false), "files of this kind (text/plain)");
        assert_eq!(kind_words(".bashrc", "text/plain", false), "files of this kind (text/plain)");
        assert_eq!(kind_words("Photos", "inode/directory", true), "folders");
    }
}
