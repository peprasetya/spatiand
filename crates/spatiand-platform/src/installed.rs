//! Applications installed into Spatiand: listed with its own, behind the "Spatiand" bubble.
//!
//! Spatiand's own applications are the programs it carries (`crates/spatiand/src/own.rs`). An
//! installed one is a folder in `~/.local/share/spatiand/apps/<id>/` with a manifest,
//! `spatiand-app.toml`, at its top:
//!
//! ```toml
//! id = "spatiquake"              # the folder's name; lowercase letters, digits, '.', '-', '_'
//! name = "SpatiQuake"            # what the launcher shows
//! exec = "spatiquake.sh"         # the program, relative to the folder
//! args = []                      # given to it, before anything else
//! icon = "spatiquake.png"        # relative to the folder; a PNG or an SVG
//! categories = ["Game"]          # freedesktop categories, as a desktop entry has them
//! kind = "vr"                    # "window" (the default) or "vr": it becomes the room
//! version = "0.1.0"              # shown when it is offered for installing
//! ```
//!
//! It arrives as a **`.spatiapp`**: that folder as a gzipped tar, with the manifest at the top
//! or inside one top folder. Installing unpacks it beside the others under a name of its own,
//! checks the manifest and the program, and only then puts it in place, replacing an earlier
//! version of the same id in one rename. A folder with a manifest installs the same way.
//!
//! Nothing here is distribution-shaped: it is the same on the Deck and on any other Linux.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

/// The manifest's file name.
pub const MANIFEST: &str = "spatiand-app.toml";
/// The package's file extension.
pub const EXTENSION: &str = "spatiapp";

/// What an installed application is in the room.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A window among the others.
    #[default]
    Window,
    /// It takes the view: an OpenXR application, which finds Spatiand's runtime through the
    /// session's `XR_RUNTIME_JSON`.
    Vr,
}

/// `spatiand-app.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub exec: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub version: Option<String>,
}

/// An application as it is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub manifest: Manifest,
    /// Its folder.
    pub dir: PathBuf,
}

impl Installed {
    /// The program to run.
    pub fn program(&self) -> PathBuf {
        self.dir.join(&self.manifest.exec)
    }

    /// The icon, if it has one that is there.
    pub fn icon(&self) -> Option<PathBuf> {
        let icon = self.dir.join(self.manifest.icon.as_ref()?);
        icon.is_file().then_some(icon)
    }

    /// The command line, as [`crate::launch`] takes it: every part quoted, since a home folder
    /// may have a space in its name.
    pub fn command(&self) -> String {
        std::iter::once(self.program().to_string_lossy().into_owned())
            .chain(self.manifest.args.iter().cloned())
            .map(|part| quote(&part))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn quote(part: &str) -> String {
    format!("\"{}\"", part.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Where installed applications are: `$XDG_DATA_HOME/spatiand/apps`, or
/// `~/.local/share/spatiand/apps`.
pub fn apps_dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("spatiand").join("apps"))
}

/// The installed applications, by name. One whose manifest does not read, or whose program is
/// missing, is left out and said in the log: a half-copied folder is not an application.
pub fn list() -> Vec<Installed> {
    apps_dir().map(|dir| list_in(&dir)).unwrap_or_default()
}

pub fn list_in(dir: &Path) -> Vec<Installed> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Installed> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.path().is_dir())
        .filter_map(|e| match read(&e.path()) {
            Ok(app) if app.manifest.id == e.file_name().to_string_lossy() => Some(app),
            Ok(app) => {
                log::warn!("{}: its manifest says id {:?}; not listed", e.path().display(), app.manifest.id);
                None
            }
            Err(err) => {
                log::warn!("{}: {err}; not listed", e.path().display());
                None
            }
        })
        .collect();
    found.sort_by(|a, b| a.manifest.name.to_lowercase().cmp(&b.manifest.name.to_lowercase()));
    found
}

/// Read and check an application's folder.
pub fn read(dir: &Path) -> Result<Installed, String> {
    let text = std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("no {MANIFEST}: {e}"))?;
    let manifest: Manifest = toml::from_str(&text).map_err(|e| format!("{MANIFEST} does not read: {e}"))?;
    check_id(&manifest.id)?;
    if manifest.name.trim().is_empty() {
        return Err("it has no name".into());
    }
    inside(&manifest.exec).map_err(|e| format!("exec: {e}"))?;
    if let Some(icon) = &manifest.icon {
        inside(icon).map_err(|e| format!("icon: {e}"))?;
    }
    let app = Installed { manifest, dir: dir.to_path_buf() };
    if !app.program().is_file() {
        return Err(format!("its program {} is not there", app.manifest.exec));
    }
    Ok(app)
}

fn check_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
    if ok {
        Ok(())
    } else {
        Err(format!("id {id:?} is not lowercase letters, digits, '.', '-' and '_'"))
    }
}

/// A path that stays in the application's folder.
fn inside(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if path.is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(format!("{path:?} is not a path inside the application's folder"));
    }
    Ok(())
}

/// Whether a file is offered as an application package, by its name.
pub fn is_package(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// What a package or folder holds, read without installing it: for asking first.
pub fn inspect(source: &Path) -> Result<Manifest, String> {
    let dir = apps_dir().ok_or("no home folder")?;
    let staging = stage(&dir, source)?;
    let result = find_root(&staging.0).and_then(|root| read(&root)).map(|app| app.manifest);
    drop(staging);
    result
}

/// Install a `.spatiapp` (or a folder with a manifest) into [`apps_dir`].
pub fn install(source: &Path) -> Result<Installed, String> {
    install_into(&apps_dir().ok_or("no home folder")?, source)
}

pub fn install_into(dir: &Path, source: &Path) -> Result<Installed, String> {
    let staging = stage(dir, source)?;
    let root = find_root(&staging.0)?;
    let app = read(&root)?;
    make_executable(&app.program())?;
    let target = dir.join(&app.manifest.id);
    // The old version moves aside and the new one into its place, each one rename, so the
    // launcher never sees a folder half of one and half of the other.
    let aside = dir.join(format!(".old-{}-{}", app.manifest.id, unique()));
    let had_old = target.exists();
    if had_old {
        std::fs::rename(&target, &aside).map_err(|e| format!("moving the installed version aside: {e}"))?;
    }
    if let Err(e) = std::fs::rename(&root, &target) {
        if had_old {
            let _ = std::fs::rename(&aside, &target);
        }
        return Err(format!("putting it in place: {e}"));
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&aside);
    }
    drop(staging);
    log::info!("installed {} ({}) in {}", app.manifest.name, app.manifest.id, target.display());
    read(&target)
}

/// Remove an installed application.
pub fn uninstall(id: &str) -> Result<(), String> {
    check_id(id)?;
    let dir = apps_dir().ok_or("no home folder")?.join(id);
    std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// A folder being unpacked into, removed when dropped.
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

/// The package unpacked (or the folder copied) beside the installed applications -- on the same
/// file system, so putting it in place is a rename.
fn stage(dir: &Path, source: &Path) -> Result<Staging, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let staging = Staging(dir.join(format!(".install-{}", unique())));
    std::fs::create_dir(&staging.0).map_err(|e| format!("{}: {e}", staging.0.display()))?;
    if source.is_dir() {
        copy_dir(source, &staging.0)?;
    } else {
        let file = std::fs::File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
        archive.set_preserve_permissions(true);
        // Entries are unpacked inside the folder; `..` and absolute paths are refused.
        archive.unpack(&staging.0).map_err(|e| format!("{} does not unpack: {e}", source.display()))?;
    }
    Ok(staging)
}

/// The manifest at the top, or in the one folder at the top.
fn find_root(staging: &Path) -> Result<PathBuf, String> {
    if staging.join(MANIFEST).is_file() {
        return Ok(staging.to_path_buf());
    }
    let entries: Vec<_> = std::fs::read_dir(staging).map_err(|e| e.to_string())?.flatten().collect();
    if let [only] = entries.as_slice() {
        if only.path().join(MANIFEST).is_file() {
            return Ok(only.path());
        }
    }
    Err(format!("no {MANIFEST} at its top"))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            std::fs::create_dir(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            copy_dir(&src, &dst)?;
        } else if kind.is_symlink() {
            let target = std::fs::read_link(&src).map_err(|e| e.to_string())?;
            std::os::unix::fs::symlink(target, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

fn make_executable(program: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(program).map_err(|e| e.to_string())?.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    std::fs::set_permissions(program, permissions).map_err(|e| format!("{}: {e}", program.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spatiand-installed-{name}-{}", unique()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const MANIFEST_TEXT: &str = "id = \"quake\"\nname = \"Quake\"\nexec = \"bin/run.sh\"\nargs = [\"-xr\"]\nicon = \"icon.png\"\nkind = \"vr\"\nversion = \"2\"\n";

    /// A package: the files given, under `top` if it is not empty.
    fn package(dir: &Path, top: &str, files: &[(&str, &str)]) -> PathBuf {
        let path = dir.join("app.spatiapp");
        let file = std::fs::File::create(&path).unwrap();
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::fast()));
        for (name, text) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            let name = if top.is_empty() { name.to_string() } else { format!("{top}/{name}") };
            builder.append_data(&mut header, name, text.as_bytes()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn a_package_installs_listed_and_runnable() {
        let dir = temp("ok");
        let pkg = package(&dir, "", &[(MANIFEST, MANIFEST_TEXT), ("bin/run.sh", "#!/bin/sh\n"), ("icon.png", "png")]);
        let apps = dir.join("apps");
        let app = install_into(&apps, &pkg).unwrap();
        assert_eq!(app.dir, apps.join("quake"));
        assert_eq!(app.manifest.kind, Kind::Vr);
        assert!(app.command().starts_with('"') && app.command().ends_with(" \"-xr\""));
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(std::fs::metadata(app.program()).unwrap().permissions().mode() & 0o111, 0);
        assert_eq!(list_in(&apps), vec![app]);
        // Nothing left over from unpacking.
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn one_top_folder_is_looked_into() {
        let dir = temp("top");
        let pkg = package(&dir, "Quake-2", &[(MANIFEST, MANIFEST_TEXT), ("bin/run.sh", "#!/bin/sh\n")]);
        let app = install_into(&dir.join("apps"), &pkg).unwrap();
        assert_eq!(app.manifest.id, "quake");
        assert!(app.icon().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installing_again_replaces_the_old_version() {
        let dir = temp("again");
        let apps = dir.join("apps");
        let old = package(&dir, "", &[(MANIFEST, MANIFEST_TEXT), ("bin/run.sh", "old"), ("stale", "x")]);
        install_into(&apps, &old).unwrap();
        let new = package(&dir, "", &[(MANIFEST, MANIFEST_TEXT), ("bin/run.sh", "new")]);
        let app = install_into(&apps, &new).unwrap();
        assert_eq!(std::fs::read_to_string(app.program()).unwrap(), "new");
        assert!(!app.dir.join("stale").exists());
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn what_is_wrong_is_refused_and_leaves_nothing() {
        let dir = temp("bad");
        let apps = dir.join("apps");
        let cases: &[&[(&str, &str)]] = &[
            &[("bin/run.sh", "x")],
            &[(MANIFEST, "id = \"Bad Id\"\nname = \"x\"\nexec = \"run\"\n"), ("run", "x")],
            &[(MANIFEST, "id = \"ok\"\nname = \"x\"\nexec = \"../run\"\n")],
            &[(MANIFEST, "id = \"ok\"\nname = \"x\"\nexec = \"/bin/sh\"\n")],
            &[(MANIFEST, "id = \"ok\"\nname = \"x\"\nexec = \"missing\"\n")],
            &[(MANIFEST, "id = \"ok\"\nname = \"x\"\nexec = \"run\"\nsurprise = 1\n"), ("run", "x")],
        ];
        for files in cases {
            let pkg = package(&dir, "", files);
            assert!(install_into(&apps, &pkg).is_err(), "{files:?}");
        }
        assert_eq!(std::fs::read_dir(&apps).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_folder_installs_like_a_package() {
        let dir = temp("folder");
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join(MANIFEST), MANIFEST_TEXT).unwrap();
        std::fs::write(src.join("bin/run.sh"), "#!/bin/sh\n").unwrap();
        let app = install_into(&dir.join("apps"), &src).unwrap();
        assert!(app.program().is_file());
        assert!(src.join(MANIFEST).is_file(), "the source is copied, not moved");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_folder_whose_name_is_not_its_id_is_not_listed() {
        let dir = temp("misnamed");
        let other = dir.join("other");
        std::fs::create_dir_all(other.join("bin")).unwrap();
        std::fs::write(other.join(MANIFEST), MANIFEST_TEXT).unwrap();
        std::fs::write(other.join("bin/run.sh"), "x").unwrap();
        assert!(list_in(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn packages_are_known_by_name() {
        assert!(is_package(Path::new("/x/SpatiQuake-1.spatiapp")));
        assert!(is_package(Path::new("a.SPATIAPP")));
        assert!(!is_package(Path::new("a.tar.gz")));
    }
}
