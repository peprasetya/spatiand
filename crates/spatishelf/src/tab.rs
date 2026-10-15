//! A tab: a process of its own that lists a place, makes pictures of what is in it, and does
//! what is asked to files -- on this machine by itself, on a remote through the storage
//! service. Run as `spatishelf --tab`; spoken to over its standard input and output.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde_json::json;
use spatiand_storage::Storage;

use crate::proto::{self, FromTab, Item, Kind, Op, Place, ToTab};

type Out = Arc<Mutex<io::Stdout>>;

fn tell(out: &Out, message: &FromTab) -> bool {
    let mut out = out.lock().unwrap();
    proto::send(&mut *out, &proto::encode_from_tab(message)).is_ok()
}

/// The storage service, connected to once and kept.
fn storage(kept: &Mutex<Option<Storage>>) -> Result<Storage, String> {
    let mut kept = kept.lock().unwrap();
    if let Some(s) = kept.as_ref() {
        return Ok(s.clone());
    }
    let s = Storage::connect()?;
    *kept = Some(s.clone());
    Ok(s)
}

fn seconds(time: std::io::Result<std::time::SystemTime>) -> Option<i64> {
    time.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64)
}

/// A Mac's folders that are not folders to anyone using one: an application, a library.
fn is_package(name: &str) -> bool {
    let Some((_, ending)) = name.rsplit_once('.') else { return false };
    ["app", "bundle", "framework", "plugin", "kext", "photoslibrary", "musiclibrary", "tvlibrary", "photolibrary"].contains(&ending.to_lowercase().as_str())
}

fn list_local(path: &str) -> Result<(Vec<Item>, Option<u64>), String> {
    let read = std::fs::read_dir(path).map_err(|e| match e.kind() {
        io::ErrorKind::PermissionDenied => "This folder may not be opened.".to_string(),
        io::ErrorKind::NotFound => "This folder is not there any more.".to_string(),
        _ => e.to_string(),
    })?;
    let mut items: Vec<Item> = read
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let meta = std::fs::metadata(entry.path()).or_else(|_| entry.metadata());
            let folder = meta.as_ref().is_ok_and(|m| m.is_dir()) && !is_package(&name);
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta.ok().and_then(|m| seconds(m.modified()));
            Item { hidden: name.starts_with('.'), kind: Kind::of(&name, folder), name, size, modified }
        })
        .collect();
    sort(&mut items);
    Ok((items, free_space(path)))
}

fn free_space(path: &str) -> Option<u64> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    Some(s.f_bavail as u64 * s.f_frsize as u64)
}

fn sort(items: &mut [Item]) {
    items.sort_by(|a, b| (a.kind != Kind::Folder).cmp(&(b.kind != Kind::Folder)).then_with(|| natural(&a.name, &b.name)));
}

/// Names as a person orders them: "IMG_9" before "IMG_10".
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| -> Vec<(u8, String, u128)> {
        let mut out = Vec::new();
        let mut chars = s.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                let mut n = String::new();
                while let Some(&d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                    n.push(d);
                    chars.next();
                }
                out.push((0, String::new(), n.parse().unwrap_or(u128::MAX)));
            } else {
                let mut w = String::new();
                while let Some(&d) = chars.peek().filter(|d| !d.is_ascii_digit()) {
                    w.extend(d.to_lowercase());
                    chars.next();
                }
                out.push((1, w, 0));
            }
        }
        out
    };
    key(a).cmp(&key(b))
}

fn list_remote(storage: &Storage, remote: &str, path: &str) -> Result<(Vec<Item>, Option<u64>), String> {
    let entries = storage.list(remote, path)?;
    let mut items: Vec<Item> = entries
        .into_iter()
        .map(|e| Item {
            hidden: e.name.starts_with('.'),
            kind: Kind::of(&e.name, e.is_dir),
            size: e.size.max(0) as u64,
            modified: parse_time(&e.modified),
            name: e.name,
        })
        .collect();
    sort(&mut items);
    Ok((items, None))
}

/// RFC 3339's date and time to seconds since 1970, near enough for showing.
fn parse_time(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|n| n.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.get(..8)?.split(':').map(|n| n.parse::<i64>().ok());
    let (h, min, s) = (t.next()??, t.next()??, t.next()??);
    // Days from the civil calendar (Howard Hinnant's).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + min * 60 + s)
}

/// A small picture of a file, for its tile.
fn picture(place: &Place, kind: Kind, size: u32, storage: &Mutex<Option<Storage>>) -> Option<(u32, u32, Vec<u8>)> {
    let shrink = |image: image::DynamicImage| {
        let small = image.thumbnail(size, size).to_rgba8();
        let (w, h) = small.dimensions();
        Some((w, h, small.into_raw()))
    };
    let still = |source: &str| spatiand_media::still::grab(source, size).map(|s| (s.width, s.height, s.rgba));
    match (place, kind) {
        (Place::Local(path), Kind::Picture) => {
            let ending = Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ["heic", "heif", "avif", "jp2", "jxl"].contains(&ending.as_str()) {
                return still(path);
            }
            use image::ImageDecoder;
            let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
            let mut decoder = reader.into_decoder().ok()?;
            let orientation = decoder.orientation().ok();
            let mut image = image::DynamicImage::from_decoder(decoder).ok()?;
            if let Some(orientation) = orientation {
                image.apply_orientation(orientation);
            }
            shrink(image)
        }
        (Place::Local(path), Kind::Film) => still(path),
        (Place::Remote(remote, path), Kind::Picture) => {
            let storage = storage.lock().unwrap().clone()?;
            let ending = Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ["heic", "heif", "avif", "jp2", "jxl"].contains(&ending.as_str()) {
                return still(&storage.url(remote, path));
            }
            let bytes = storage.read(remote, path, 40 << 20).ok()?;
            shrink(image::load_from_memory(&bytes).ok()?)
        }
        (Place::Remote(remote, path), Kind::Film) => {
            let storage = storage.lock().unwrap().clone()?;
            still(&storage.url(remote, path))
        }
        _ => None,
    }
}

// --- doing things to files ---

/// The rclone name of a place: a remote's `remote:path`, or this machine's path.
fn fs_and_name(place: &Place) -> (String, String) {
    match place {
        Place::Local(p) => {
            let path = Path::new(p);
            (path.parent().map_or("/".into(), |d| d.to_string_lossy().into_owned()), path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()))
        }
        Place::Remote(r, p) => match p.rsplit_once('/') {
            Some((dir, name)) => (format!("{r}:{dir}"), name.to_string()),
            None => (format!("{r}:"), p.clone()),
        },
    }
}

fn whole(place: &Place) -> String {
    match place {
        Place::Local(p) => p.clone(),
        Place::Remote(r, p) => format!("{r}:{p}"),
    }
}

fn is_dir(place: &Place, storage: &Storage) -> bool {
    match place {
        Place::Local(p) => Path::new(p).is_dir(),
        Place::Remote(r, p) => {
            let (fs, name) = fs_and_name(&Place::Remote(r.clone(), p.clone()));
            storage.rc("operations/stat", json!({"fs": fs, "remote": name})).ok().and_then(|v| v.get("item").and_then(|i| i.get("IsDir")).and_then(|b| b.as_bool())).unwrap_or(false)
        }
    }
}

/// A name in `folder` not yet taken: "a.jpg", then "a 2.jpg", "a 3.jpg".
fn free_name(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ending) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (2..).map(|n| folder.join(format!("{stem} {n}{ending}"))).find(|p| !p.exists()).unwrap()
}

fn copy_local(from: &Path, to: &Path, done: &mut u64, total: u64, report: &mut dyn FnMut(u64, u64, &str)) -> io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_local(&entry.path(), &to.join(entry.file_name()), done, total, report)?;
        }
        return Ok(());
    }
    report(*done, total, &from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    std::fs::copy(from, to)?;
    *done += std::fs::metadata(from).map(|m| m.len()).unwrap_or(0);
    Ok(())
}

fn size_of(path: &Path) -> u64 {
    if path.is_dir() {
        std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| size_of(&e.path())).sum()
    } else {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }
}

/// Through rclone, waiting for the job and saying how it goes.
fn rclone_job(storage: &Storage, method: &str, mut input: serde_json::Value, what: &str, report: &mut dyn FnMut(u64, u64, &str)) -> Result<(), String> {
    input["_async"] = json!(true);
    let started = storage.rc(method, input)?;
    let job = started.get("jobid").and_then(|j| j.as_i64()).ok_or("the storage service did not start the job")?;
    loop {
        std::thread::sleep(Duration::from_millis(400));
        let status = storage.rc("job/status", json!({"jobid": job}))?;
        let stats = storage.rc("core/stats", json!({"group": format!("job/{job}")})).unwrap_or_default();
        let done = stats.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0);
        let total = stats.get("totalBytes").and_then(|b| b.as_u64()).unwrap_or(0);
        report(done, total.max(done), what);
        if status.get("finished").and_then(|f| f.as_bool()) == Some(true) {
            let error = status.get("error").and_then(|e| e.as_str()).unwrap_or("");
            return if error.is_empty() { Ok(()) } else { Err(error.to_string()) };
        }
    }
}

/// Copy or move one thing into a folder, by whichever way the two places allow.
fn transfer(item: &Place, into: &Place, moving: bool, storage: &Mutex<Option<Storage>>, report: &mut dyn FnMut(u64, u64, &str)) -> Result<(), String> {
    let name = item.name();
    if let (Place::Local(from), Place::Local(to)) = (item, into) {
        let from = Path::new(from);
        let target = free_name(Path::new(to), &name);
        if target.starts_with(from) {
            return Err(format!("{name} cannot go inside itself"));
        }
        if moving && std::fs::rename(from, &target).is_ok() {
            return Ok(());
        }
        let total = size_of(from);
        let mut done = 0;
        copy_local(from, &target, &mut done, total, report).map_err(|e| e.to_string())?;
        if moving {
            if from.is_dir() { std::fs::remove_dir_all(from) } else { std::fs::remove_file(from) }.map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let storage = storage_or(storage)?;
    let dir = is_dir(item, &storage);
    let what = format!("{} {name}", if moving { "Moving" } else { "Copying" });
    if dir {
        let target = whole(&into.join(&name));
        let method = if moving { "sync/move" } else { "sync/copy" };
        rclone_job(&storage, method, json!({"srcFs": whole(item), "dstFs": target, "createEmptySrcDirs": true, "deleteEmptySrcDirs": moving}), &what, report)
    } else {
        let (src_fs, src_name) = fs_and_name(item);
        let method = if moving { "operations/movefile" } else { "operations/copyfile" };
        rclone_job(&storage, method, json!({"srcFs": src_fs, "srcRemote": src_name, "dstFs": whole(into), "dstRemote": name}), &what, report)
    }
}

fn storage_or(kept: &Mutex<Option<Storage>>) -> Result<Storage, String> {
    storage(kept)
}

/// Into the trash: a Mac's own, or the freedesktop one everything on Linux shares.
fn trash(path: &Path) -> Result<(), String> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("no home folder")?);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if cfg!(target_os = "macos") {
        let bin = home.join(".Trash");
        let target = free_name(&bin, &name);
        return std::fs::rename(path, &target).map_err(|e| e.to_string());
    }
    if cfg!(target_os = "android") {
        return Err("There is no trash on this machine.".into());
    }
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("Trash");
    let (files, info) = (base.join("files"), base.join("info"));
    std::fs::create_dir_all(&files).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&info).map_err(|e| e.to_string())?;
    let target = free_name(&files, &name);
    let leaf = target.file_name().unwrap().to_string_lossy().into_owned();
    let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, secs) = (now.div_euclid(86400), now.rem_euclid(86400));
    // Civil date from days (the inverse of the above).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    let stamp = format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60);
    let original = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    std::fs::write(info.join(format!("{leaf}.trashinfo")), format!("[Trash Info]\nPath={}\nDeletionDate={stamp}\n", original.display())).map_err(|e| e.to_string())?;
    std::fs::rename(path, &target).map_err(|e| e.to_string())
}

fn delete(place: &Place, storage: &Mutex<Option<Storage>>) -> Result<(), String> {
    match place {
        Place::Local(p) => {
            let path = Path::new(p);
            if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }.map_err(|e| e.to_string())
        }
        Place::Remote(..) => {
            let storage = storage_or(storage)?;
            if is_dir(place, &storage) {
                storage.rc("operations/purge", json!({"fs": whole(place), "remote": ""})).map(|_| ())
            } else {
                let (fs, name) = fs_and_name(place);
                storage.rc("operations/deletefile", json!({"fs": fs, "remote": name})).map(|_| ())
            }
        }
    }
}

fn run_op(op: Op, storage: &Mutex<Option<Storage>>, report: &mut dyn FnMut(u64, u64, &str)) -> Result<(), String> {
    match op {
        Op::Copy { from, to } => from.iter().try_for_each(|item| transfer(item, &to, false, storage, report)),
        Op::Move { from, to } => from.iter().try_for_each(|item| {
            if item.parent().as_ref() == Some(&to) {
                return Ok(());
            }
            transfer(item, &to, true, storage, report)
        }),
        Op::Trash(items) => items.iter().try_for_each(|item| match item {
            Place::Local(p) => trash(Path::new(p)),
            Place::Remote(..) => Err("A remote has no trash here: delete it instead.".into()),
        }),
        Op::Delete(items) => items.iter().try_for_each(|item| delete(item, storage)),
        Op::Rename { item, name } => {
            if name.contains('/') || name.trim().is_empty() {
                return Err("A name may not be empty or have a / in it.".into());
            }
            match &item {
                Place::Local(p) => {
                    let to = Path::new(p).with_file_name(&name);
                    if to.exists() {
                        return Err(format!("There is already something called {name} here."));
                    }
                    std::fs::rename(p, to).map_err(|e| e.to_string())
                }
                Place::Remote(..) => {
                    let storage = storage_or(storage)?;
                    let (fs, old) = fs_and_name(&item);
                    if is_dir(&item, &storage) {
                        let parent = item.parent().unwrap_or(item.clone());
                        rclone_job(&storage, "sync/move", json!({"srcFs": whole(&item), "dstFs": whole(&parent.join(&name)), "deleteEmptySrcDirs": true}), "Renaming", report)
                    } else {
                        storage.rc("operations/movefile", json!({"srcFs": fs, "srcRemote": old, "dstFs": fs, "dstRemote": name})).map(|_| ())
                    }
                }
            }
        }
        Op::Mkdir { inside, name } => match &inside {
            Place::Local(p) => std::fs::create_dir(Path::new(p).join(&name)).map_err(|e| e.to_string()),
            Place::Remote(..) => {
                let storage = storage_or(storage)?;
                let target = inside.join(&name);
                let (fs, leaf) = fs_and_name(&target);
                storage.rc("operations/mkdir", json!({"fs": fs, "remote": leaf})).map(|_| ())
            }
        },
    }
}

pub fn run() -> io::Result<()> {
    let out: Out = Arc::new(Mutex::new(io::stdout()));
    let kept: Arc<Mutex<Option<Storage>>> = Arc::new(Mutex::new(None));
    let (asks, asked): (Sender<ToTab>, Receiver<ToTab>) = channel();
    std::thread::spawn(move || {
        let mut input = io::stdin();
        while let Ok(Some(m)) = proto::read_to_tab(&mut input) {
            if asks.send(m).is_err() {
                return;
            }
        }
        let _ = asks.send(ToTab::Quit);
    });
    // Pictures on a thread of their own, newest asking first, dropped when the place changes.
    let (pictures_tx, pictures_rx) = channel::<(Place, usize, Place, Kind, u32)>();
    {
        let out = out.clone();
        let kept = kept.clone();
        std::thread::spawn(move || {
            let mut queue = Vec::new();
            loop {
                match pictures_rx.recv() {
                    Ok(job) => queue.push(job),
                    Err(_) => return,
                }
                while let Ok(job) = pictures_rx.try_recv() {
                    queue.push(job);
                }
                // Only for the place most recently asked about: what was scrolled past or
                // left behind is not worth making.
                if let Some(current) = queue.last().map(|j| j.0.clone()) {
                    queue.retain(|j| j.0 == current);
                }
                while !queue.is_empty() {
                    let (folder, index, file, kind, size) = queue.remove(0);
                    if let Some((width, height, rgba)) = picture(&file, kind, size, &kept) {
                        if !tell(&out, &FromTab::Picture { place: folder, index, width, height, rgba }) {
                            return;
                        }
                    }
                    if let Ok(job) = pictures_rx.try_recv() {
                        queue.push(job);
                        let current = queue.last().map(|j| j.0.clone()).unwrap();
                        queue.retain(|j| j.0 == current);
                    }
                }
            }
        });
    }
    let mut here: Option<Place> = None;
    let mut items: Vec<Item> = Vec::new();
    let list = |place: &Place, kept: &Mutex<Option<Storage>>| -> Result<(Vec<Item>, Option<u64>), String> {
        match place {
            Place::Local(p) => list_local(p),
            Place::Remote(r, p) => list_remote(&storage(kept)?, r, p),
        }
    };
    while let Ok(message) = asked.recv() {
        match message {
            ToTab::Go(place) => {
                here = Some(place.clone());
                match list(&place, &kept) {
                    Ok((listed, free)) => {
                        items = listed.clone();
                        if !tell(&out, &FromTab::Listed { place, items: listed, free }) {
                            return Ok(());
                        }
                    }
                    Err(error) => {
                        items.clear();
                        if !tell(&out, &FromTab::Failed { place, error }) {
                            return Ok(());
                        }
                    }
                }
            }
            ToTab::Refresh => {
                if let Some(place) = here.clone() {
                    match list(&place, &kept) {
                        Ok((listed, free)) => {
                            items = listed.clone();
                            tell(&out, &FromTab::Listed { place, items: listed, free });
                        }
                        Err(error) => {
                            tell(&out, &FromTab::Failed { place, error });
                        }
                    }
                }
            }
            ToTab::Pictures { indices, size } => {
                let Some(place) = here.clone() else { continue };
                for index in indices {
                    if let Some(item) = items.get(index).filter(|i| i.kind.pictured()) {
                        let _ = pictures_tx.send((place.clone(), index, place.join(&item.name), item.kind, size));
                    }
                }
            }
            ToTab::Do { id, op } => {
                // Done on a thread, so that the tab goes on listing while a large copy runs.
                let out = out.clone();
                let kept = kept.clone();
                std::thread::spawn(move || {
                    let mut last = Instant::now() - Duration::from_secs(1);
                    let mut report = |done: u64, total: u64, what: &str| {
                        if last.elapsed() >= Duration::from_millis(200) {
                            last = Instant::now();
                            tell(&out, &FromTab::Progress { id, done, total, what: what.to_string() });
                        }
                    };
                    let result = run_op(op, &kept, &mut report);
                    tell(&out, &FromTab::Done { id, error: result.err() });
                });
            }
            ToTab::Quit => return Ok(()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rclones_dates_read_as_seconds() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-10-14T14:48:03Z"), Some(1791989283));
        assert_eq!(parse_time("garbage"), None);
    }

    #[test]
    fn a_name_already_taken_is_numbered() {
        let dir = std::env::temp_dir().join(format!("spatishelf-free-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), b"").unwrap();
        std::fs::write(dir.join("a 2.jpg"), b"").unwrap();
        assert_eq!(free_name(&dir, "a.jpg"), dir.join("a 3.jpg"));
        assert_eq!(free_name(&dir, "b.jpg"), dir.join("b.jpg"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_copies_and_moves_go_where_they_are_sent() {
        let dir = std::env::temp_dir().join(format!("spatishelf-ops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/inner")).unwrap();
        std::fs::create_dir_all(dir.join("dst")).unwrap();
        std::fs::write(dir.join("src/inner/f.txt"), b"hello").unwrap();
        let kept = Mutex::new(None);
        let place = |p: &str| Place::Local(dir.join(p).to_string_lossy().into_owned());
        let mut quiet = |_: u64, _: u64, _: &str| {};
        run_op(Op::Copy { from: vec![place("src")], to: place("dst") }, &kept, &mut quiet).unwrap();
        assert_eq!(std::fs::read(dir.join("dst/src/inner/f.txt")).unwrap(), b"hello");
        run_op(Op::Rename { item: place("dst/src"), name: "copy".into() }, &kept, &mut quiet).unwrap();
        run_op(Op::Move { from: vec![place("dst/copy/inner/f.txt")], to: place("dst") }, &kept, &mut quiet).unwrap();
        assert!(dir.join("dst/f.txt").exists() && !dir.join("dst/copy/inner/f.txt").exists());
        run_op(Op::Mkdir { inside: place("dst"), name: "new".into() }, &kept, &mut quiet).unwrap();
        assert!(dir.join("dst/new").is_dir());
        assert!(run_op(Op::Copy { from: vec![place("src")], to: place("src/inner") }, &kept, &mut quiet).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
