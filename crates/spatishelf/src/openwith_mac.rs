//! Which applications open a file, as Finder's Open With has them: Launch Services, through
//! `NSWorkspace` -- `URLsForApplicationsToOpenURL` for all of them, `URLForApplicationToOpenURL`
//! for the default -- and each one's icon by `iconForFile`. Started with `open -a`, as Finder
//! would start it. "Other..." is what is in the Applications folders.

use std::path::{Path, PathBuf};

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

use super::App;

/// What "always open with" is remembered by: the ending, as a Mac tells kinds apart to the
/// wearer.
pub fn type_of(path: &Path, folder: bool) -> String {
    if folder {
        return "folder".into();
    }
    match path.extension() {
        Some(e) => format!("ext:{}", e.to_string_lossy().to_lowercase()),
        None => "ext:".into(),
    }
}

fn url_of(path: &Path) -> objc2::rc::Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn app_of(bundle: &Path, default: bool) -> App {
    let name = bundle.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    App { id: bundle.to_string_lossy().into_owned(), name, system_default: default, takes_urls: false }
}

fn path_of(url: &NSURL) -> Option<PathBuf> {
    url.path().map(|p| PathBuf::from(p.to_string()))
}

pub fn apps_for(path: &Path, _kind: &str, _room: &mut spatiand_app::App) -> Vec<App> {
    let workspace = NSWorkspace::sharedWorkspace();
    let url = url_of(path);
    let default = workspace.URLForApplicationToOpenURL(&url).and_then(|u| path_of(&u));
    let mut out: Vec<App> = default.iter().map(|d| app_of(d, true)).collect();
    for app in workspace.URLsForApplicationsToOpenURL(&url).to_vec() {
        if let Some(p) = path_of(&app) {
            if Some(&p) != default.as_ref() {
                out.push(app_of(&p, false));
            }
        }
    }
    out
}

pub fn all_apps(_room: &mut spatiand_app::App) -> Vec<App> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let folders = [PathBuf::from("/Applications"), PathBuf::from("/Applications/Utilities"), home.join("Applications"), PathBuf::from("/System/Applications"), PathBuf::from("/System/Applications/Utilities")];
    folders
        .iter()
        .flat_map(|f| std::fs::read_dir(f).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| app_of(&p, false))
        .collect()
}

pub fn launch(app: &App, files: &[String], _room: &mut spatiand_app::App) -> Result<(), String> {
    let mut command = std::process::Command::new("open");
    command.arg("-a").arg(&app.id).args(files);
    crate::detached(command)
}

/// The application's icon as Finder draws it, from its TIFF (whose first picture is read).
pub fn icon(app: &App, size: u32) -> Option<(u32, u32, Vec<u8>)> {
    let workspace = NSWorkspace::sharedWorkspace();
    let icon = workspace.iconForFile(&NSString::from_str(&app.id));
    let tiff = icon.TIFFRepresentation()?;
    let picture = image::load_from_memory_with_format(&tiff.to_vec(), image::ImageFormat::Tiff).ok()?;
    super::pixels_of(picture, size)
}
