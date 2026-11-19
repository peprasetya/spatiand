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
    let read = std::fs::read_dir(proto::path_of(path)).map_err(|e| match e.kind() {
        io::ErrorKind::PermissionDenied => "This folder may not be opened.".to_string(),
        io::ErrorKind::NotFound => "This folder is not there any more.".to_string(),
        _ => e.to_string(),
    })?;
    let mut items: Vec<Item> = read
        .flatten()
        .map(|entry| {
            let name = proto::text_of(&entry.file_name());
            let meta = std::fs::metadata(entry.path()).or_else(|_| entry.metadata());
            let folder = meta.as_ref().is_ok_and(|m| m.is_dir()) && !is_package(&name);
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta.ok().and_then(|m| seconds(m.modified()));
            Item { hidden: name.starts_with('.'), kind: Kind::of(&name, folder), name, size, modified, trashed: None }
        })
        .collect();
    sort(&mut items);
    Ok((items, free_space(path)))
}

/// What is in the trash, each thing by its own name and with where it came from; when it was
/// put there is its date.
fn list_trash() -> Result<(Vec<Item>, Option<u64>), String> {
    let mut items: Vec<Item> = crate::trash::list()?
        .into_iter()
        .map(|e| Item {
            kind: Kind::of(&e.name, e.folder),
            size: e.size,
            modified: e.deleted,
            hidden: false,
            trashed: Some(crate::proto::Trashed { file: crate::proto::text_of(e.file.as_os_str()), origin: e.origin.map(|o| crate::proto::text_of(o.as_os_str())), deleted: e.deleted }),
            name: e.name,
        })
        .collect();
    sort(&mut items);
    Ok((items, None))
}

fn free_space(path: &str) -> Option<u64> {
    let c = std::ffi::CString::new(std::os::unix::ffi::OsStringExt::into_vec(proto::os_of(path))).ok()?;
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
            trashed: None,
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
    // The zone it was written in: `Z`, or `+07:00` after the seconds and their fraction (a
    // local disk's times, as rclone gives them, are the machine's own).
    let zone = time.get(8..).unwrap_or("").trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone.as_bytes().first() {
        Some(sign @ (b'+' | b'-')) => {
            let mut parts = zone[1..].split(':').map(|n| n.parse::<i64>().ok());
            let (oh, om) = (parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0));
            (oh * 3600 + om * 60) * if *sign == b'-' { -1 } else { 1 }
        }
        _ => 0,
    };
    Some(days * 86400 + h * 3600 + min * 60 + s - offset)
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
            let reader = image::ImageReader::open(proto::path_of(path)).ok()?.with_guessed_format().ok()?;
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
        Place::Trash => (String::new(), String::new()),
    }
}

fn whole(place: &Place) -> String {
    match place {
        Place::Local(p) => p.clone(),
        Place::Remote(r, p) => format!("{r}:{p}"),
        Place::Trash => String::new(),
    }
}

/// Remote folders rclone has just changed, let go of in the storage service's WebDAV view
/// (`Storage::forget_folders`): without it SpatiLoupe or SpatiReel reading a file there through
/// it, or a listing through it, is up to half a minute behind -- a file moved over another read
/// at the old one's length. Local places are passed over.
fn forget(storage: &Storage, folders: &[Place]) {
    let mut by_remote: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for folder in folders {
        if let Place::Remote(remote, path) = folder {
            by_remote.entry(remote.as_str()).or_default().push(path.as_str());
        }
    }
    for (remote, paths) in by_remote {
        storage.forget_folders(remote, &paths);
    }
}

/// The folder a place is in (the remote's top for a place at its top).
fn folder_of(place: &Place) -> Place {
    place.parent().unwrap_or_else(|| place.clone())
}

fn is_dir(place: &Place, storage: &Storage) -> bool {
    match place {
        Place::Trash => false,
        Place::Local(p) => proto::path_of(p).is_dir(),
        Place::Remote(r, p) => {
            let (fs, name) = fs_and_name(&Place::Remote(r.clone(), p.clone()));
            storage.rc("operations/stat", json!({"fs": fs, "remote": name})).ok().and_then(|v| v.get("item").and_then(|i| i.get("IsDir")).and_then(|b| b.as_bool())).unwrap_or(false)
        }
    }
}

/// A name in `folder` not yet taken: "a.jpg", then "a 2.jpg", "a 3.jpg". Something there by
/// the name is anything at all, a link that leads nowhere included.
fn free_name(folder: &Path, name: &str) -> PathBuf {
    folder.join(proto::os_of(&free_among(name, |n| Ok::<_, ()>(folder.join(proto::os_of(n)).symlink_metadata().is_ok())).unwrap_or_else(|_| name.to_string())))
}

/// The first of "a.jpg", "a 2.jpg", "a 3.jpg" that `taken` says is free; its error if it
/// cannot say.
fn free_among<E>(name: &str, mut taken: impl FnMut(&str) -> Result<bool, E>) -> Result<String, E> {
    if !taken(name)? {
        return Ok(name.to_string());
    }
    let (stem, ending) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 2.. {
        let candidate = format!("{stem} {n}{ending}");
        if !taken(&candidate)? {
            return Ok(candidate);
        }
    }
    unreachable!()
}

/// What is at a place, if anything: a remote's asked with `operations/stat`, whose `item` is
/// null when nothing is there (rclone's StatJSON, a missing folder on the way included); this
/// machine's by lstat, so that a link leading nowhere is something.
fn found(place: &Place, storage: &Storage) -> Result<Option<serde_json::Value>, String> {
    match place {
        Place::Trash => Ok(None),
        Place::Local(p) => Ok(proto::path_of(p).symlink_metadata().ok().map(|_| json!({"Name": place.name()}))),
        Place::Remote(..) => {
            let (fs, name) = fs_and_name(place);
            let answer = storage.rc("operations/stat", json!({"fs": fs, "remote": name}))?;
            Ok(answer.get("item").filter(|i| !i.is_null()).cloned())
        }
    }
}

/// A thing copied as it is: a folder with what is in it, a link as the link it is -- not what
/// it leads to (a Proton prefix's `dosdevices/z:` leads to the whole disk, and a link that
/// leads nowhere is still a link). What is neither a file, a folder nor a link (a pipe, a
/// socket, a device) is refused: copying a pipe waits for ever.
fn copy_local(from: &Path, to: &Path, done: &mut u64, total: u64, report: &mut dyn FnMut(u64, u64, &str)) -> io::Result<()> {
    let meta = from.symlink_metadata()?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        return std::os::unix::fs::symlink(std::fs::read_link(from)?, to);
    }
    if kind.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_local(&entry.path(), &to.join(entry.file_name()), done, total, report)?;
        }
        return Ok(());
    }
    if !kind.is_file() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, format!("{} is not a file or a folder, and is not copied", from.display())));
    }
    report(*done, total, &from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    std::fs::copy(from, to)?;
    *done += meta.len();
    Ok(())
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
        let from = &proto::path_of(from);
        let target = free_name(&proto::path_of(to), &name);
        if target.starts_with(from) {
            return Err(format!("{name} cannot go inside itself"));
        }
        // Moved by renaming; copied and taken away only to another disk, which renaming cannot
        // reach. Any other refusal is the move's.
        if moving {
            match std::fs::rename(from, &target) {
                Ok(()) => return Ok(()),
                Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        // Links not followed, here as in the copy.
        let total = crate::trash::size_of(from);
        let mut done = 0;
        copy_local(from, &target, &mut done, total, report).map_err(|e| e.to_string())?;
        if moving {
            let folder = from.symlink_metadata().is_ok_and(|m| m.is_dir());
            if folder { std::fs::remove_dir_all(from) } else { std::fs::remove_file(from) }.map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    let storage = storage_or(storage)?;
    let dir = is_dir(item, &storage);
    let what = format!("{} {name}", if moving { "Moving" } else { "Copying" });
    // Beside what is there by the name already, numbered, as on this machine: rclone's copies
    // and moves write over a file of the name and merge into a folder of it.
    let name = free_among(&name, |n| found(&into.join(n), &storage).map(|f| f.is_some()))?;
    let done = if dir {
        let target = whole(&into.join(&name));
        let method = if moving { "sync/move" } else { "sync/copy" };
        rclone_job(&storage, method, json!({"srcFs": whole(item), "dstFs": target, "createEmptySrcDirs": true, "deleteEmptySrcDirs": moving}), &what, report)
    } else {
        let (src_fs, src_name) = fs_and_name(item);
        let method = if moving { "operations/movefile" } else { "operations/copyfile" };
        rclone_job(&storage, method, json!({"srcFs": src_fs, "srcRemote": src_name, "dstFs": whole(into), "dstRemote": name}), &what, report)
    };
    // Even after a failure part of it may have been done.
    let mut changed = vec![into.clone()];
    if moving {
        changed.push(folder_of(item));
    }
    forget(&storage, &changed);
    done
}

fn storage_or(kept: &Mutex<Option<Storage>>) -> Result<Storage, String> {
    storage(kept)
}

fn delete(place: &Place, storage: &Mutex<Option<Storage>>) -> Result<(), String> {
    match place {
        Place::Trash => Ok(()),
        // From the trash, with what it knew of it; anywhere else, just gone.
        Place::Local(p) => crate::trash::purge(&proto::path_of(p)),
        Place::Remote(..) => {
            let storage = storage_or(storage)?;
            let done = if is_dir(place, &storage) {
                storage.rc("operations/purge", json!({"fs": whole(place), "remote": ""})).map(|_| ())
            } else {
                let (fs, name) = fs_and_name(place);
                storage.rc("operations/deletefile", json!({"fs": fs, "remote": name})).map(|_| ())
            };
            forget(&storage, &[folder_of(place)]);
            done
        }
    }
}

/// `act` done to each of `items`, all of them, whatever fails on the way: one that cannot be
/// done does not keep the rest from being done. What failed said: one's own error, or how many
/// and the first's.
fn each(items: &[Place], mut act: impl FnMut(&Place) -> Result<(), String>) -> Result<(), String> {
    let mut failed = 0;
    let mut first = None;
    for item in items {
        if let Err(e) = act(item) {
            failed += 1;
            first.get_or_insert(e);
        }
    }
    match first {
        None => Ok(()),
        Some(e) if items.len() == 1 => Err(e),
        Some(e) => Err(format!("{failed} of {} could not be done; the first: {e}", items.len())),
    }
}

fn run_op(op: Op, storage: &Mutex<Option<Storage>>, report: &mut dyn FnMut(u64, u64, &str)) -> Result<(), String> {
    match op {
        Op::Copy { from, to } => each(&from, |item| transfer(item, &to, false, storage, report)),
        // Moved onto the trash: put in it.
        Op::Move { from, to: Place::Trash } => run_op(Op::Trash(from), storage, report),
        Op::Move { from, to } => each(&from, |item| {
            if item.parent().as_ref() == Some(&to) {
                return Ok(());
            }
            transfer(item, &to, true, storage, report)?;
            // Taken out of the trash by a move: what the trash knew of it goes too.
            if let Place::Local(p) = item {
                crate::trash::moved_out(&proto::path_of(p));
            }
            Ok(())
        }),
        Op::Trash(items) => each(&items, |item| match item {
            Place::Local(p) => crate::trash::put(&proto::path_of(p)),
            Place::Remote(..) => Err("A remote has no trash here: delete it instead.".into()),
            Place::Trash => Ok(()),
        }),
        Op::Delete(items) => each(&items, |item| delete(item, storage)),
        Op::Restore(items) => each(&items, |item| match item {
            Place::Local(p) => crate::trash::restore(&proto::path_of(p)).map(|_| ()),
            _ => Err("That is not in the Trash.".into()),
        }),
        Op::EmptyTrash => crate::trash::empty(),
        Op::Fetch { item, into } => {
            std::fs::create_dir_all(&into).map_err(|e| e.to_string())?;
            let storage = storage_or(storage)?;
            let (fs, name) = fs_and_name(&item);
            rclone_job(&storage, "operations/copyfile", json!({"srcFs": fs, "srcRemote": name, "dstFs": into, "dstRemote": name}), &format!("Fetching {name}"), report)
        }
        Op::Rename { item, name } => {
            if name.contains('/') || name.trim().is_empty() {
                return Err("A name may not be empty or have a / in it.".into());
            }
            match &item {
                Place::Trash => Err("The Trash keeps its name.".into()),
                Place::Local(p) => {
                    let to = proto::path_of(p).with_file_name(proto::os_of(&name));
                    // Anything by the name, a link leading nowhere too -- unless it is the thing
                    // itself, found by another case of its name on a disk that does not tell
                    // them apart (Photo.jpg to photo.jpg).
                    use std::os::unix::fs::MetadataExt;
                    let same = |a: &std::fs::Metadata, b: &std::fs::Metadata| a.dev() == b.dev() && a.ino() == b.ino();
                    if let Ok(there) = to.symlink_metadata() {
                        if !proto::path_of(p).symlink_metadata().is_ok_and(|own| same(&own, &there)) {
                            return Err(format!("There is already something called {name} here."));
                        }
                    }
                    std::fs::rename(proto::path_of(p), to).map_err(|e| e.to_string())
                }
                Place::Remote(..) => {
                    let storage = storage_or(storage)?;
                    let (fs, old) = fs_and_name(&item);
                    // rclone's moves write over a file of the name and merge into a folder of
                    // it: refused, as here. A remote that does not tell cases apart finds the
                    // thing itself by its new case, and says its own name for it.
                    let parent = item.parent().unwrap_or(item.clone());
                    if let Some(there) = found(&parent.join(&name), &storage)? {
                        let itself = name.to_lowercase() == old.to_lowercase() && there.get("Name").and_then(|n| n.as_str()) == Some(old.as_str());
                        if !itself {
                            return Err(format!("There is already something called {name} here."));
                        }
                    }
                    let done = if is_dir(&item, &storage) {
                        rclone_job(&storage, "sync/move", json!({"srcFs": whole(&item), "dstFs": whole(&parent.join(&name)), "deleteEmptySrcDirs": true}), "Renaming", report)
                    } else {
                        storage.rc("operations/movefile", json!({"srcFs": fs, "srcRemote": old, "dstFs": fs, "dstRemote": name})).map(|_| ())
                    };
                    forget(&storage, &[parent]);
                    done
                }
            }
        }
        // As a rename's: "a/b" would be made inside a folder "a", or fail without saying why.
        Op::Mkdir { name, .. } if name.contains('/') || name.trim().is_empty() => Err("A name may not be empty or have a / in it.".into()),
        Op::Mkdir { inside, name } => match &inside {
            Place::Trash => Err("No folder is made in the Trash.".into()),
            Place::Local(p) => std::fs::create_dir(proto::path_of(p).join(proto::os_of(&name))).map_err(|e| e.to_string()),
            Place::Remote(..) => {
                let storage = storage_or(storage)?;
                let target = inside.join(&name);
                let (fs, leaf) = fs_and_name(&target);
                let done = storage.rc("operations/mkdir", json!({"fs": fs, "remote": leaf})).map(|_| ());
                forget(&storage, &[inside.clone()]);
                done
            }
        },
    }
}

fn list(place: &Place, kept: &Mutex<Option<Storage>>) -> Result<(Vec<Item>, Option<u64>), String> {
    match place {
        Place::Local(p) => list_local(p),
        Place::Remote(r, p) => list_remote(&storage(kept)?, r, p),
        Place::Trash => list_trash(),
    }
}

/// The place on show, as the tab knows it.
#[derive(Default)]
struct Shown {
    place: Option<Place>,
    /// Counted up at each Go: a listing for an earlier one is of a place left, and dropped.
    generation: u64,
    /// Its listing, as last told: what pictures are asked of by number.
    items: Vec<Item>,
    /// A listing of it is under way; another was asked for meanwhile.
    listing: bool,
    again: bool,
}

/// The place on show listed, off the tab's own loop -- a remote that never answers keeps
/// nothing else waiting -- and told, unless the window has gone somewhere else meanwhile;
/// listed again if a refresh was asked for while it was.
fn listing(shown: &Mutex<Shown>, out: &Out, kept: &Mutex<Option<Storage>>, generation: u64, place: Place) {
    loop {
        let result = list(&place, kept);
        let mut now = shown.lock().unwrap();
        if now.generation != generation {
            return;
        }
        // Told while held, so that two listings are told in the order they were taken.
        match result {
            Ok((listed, free)) => {
                now.items = listed.clone();
                tell(out, &FromTab::Listed { place: place.clone(), items: listed, free });
            }
            Err(error) => {
                now.items.clear();
                tell(out, &FromTab::Failed { place: place.clone(), error });
            }
        }
        if !std::mem::take(&mut now.again) {
            now.listing = false;
            return;
        }
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
                    let (_folder, index, file, kind, size) = queue.remove(0);
                    if let Some((width, height, rgba)) = picture(&file, kind, size, &kept) {
                        if !tell(&out, &FromTab::Picture { place: file.clone(), index, width, height, rgba }) {
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
    // The place on show and its listing, listed on a thread (see `listing`).
    let shown: Arc<Mutex<Shown>> = Arc::new(Mutex::new(Shown::default()));
    // The folders on show, watched: a change from anywhere is told to the window.
    let watching = {
        let out = out.clone();
        crate::watch::start(move |place| tell(&out, &FromTab::Changed(place)))
    };
    while let Ok(message) = asked.recv() {
        match message {
            ToTab::Go(place) => {
                let mut now = shown.lock().unwrap();
                now.generation += 1;
                now.place = Some(place.clone());
                // What was listed of the place left is not this one's: no pictures of it.
                now.items.clear();
                now.again = false;
                now.listing = true;
                let generation = now.generation;
                drop(now);
                let (out, kept, shown) = (out.clone(), kept.clone(), shown.clone());
                std::thread::spawn(move || listing(&shown, &out, &kept, generation, place));
            }
            ToTab::Refresh => {
                let mut now = shown.lock().unwrap();
                let Some(place) = now.place.clone() else { continue };
                // One listing at a time: another asked for meanwhile is done once it is over.
                if now.listing {
                    now.again = true;
                    continue;
                }
                now.listing = true;
                let generation = now.generation;
                drop(now);
                let (out, kept, shown) = (out.clone(), kept.clone(), shown.clone());
                std::thread::spawn(move || listing(&shown, &out, &kept, generation, place));
            }
            ToTab::List(place) => {
                // On a thread: a remote's folder may take its time, or never answer, and the
                // tab goes on meanwhile.
                let out = out.clone();
                let kept = kept.clone();
                std::thread::spawn(move || {
                    let items = list(&place, &kept);
                    tell(&out, &FromTab::Contents { place, items: items.map(|(items, _)| items) });
                });
            }
            ToTab::Pictures { indices, size } => {
                let now = shown.lock().unwrap();
                let Some(place) = now.place.clone() else { continue };
                for index in indices {
                    if let Some(item) = now.items.get(index).filter(|i| i.kind.pictured()) {
                        let file = match &item.trashed {
                            Some(t) => Place::Local(t.file.clone()),
                            None => place.join(&item.name),
                        };
                        let _ = pictures_tx.send((place.clone(), index, file, item.kind, size));
                    }
                }
            }
            ToTab::Watch(places) => watching.set(places),
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
        // A local disk's, in its own zone, with a fraction of a second.
        assert_eq!(parse_time("2026-10-14T21:48:03.271+07:00"), Some(1791989283));
        assert_eq!(parse_time("2026-10-14T09:48:03-05:00"), Some(1791989283));
    }

    #[test]
    fn a_name_already_taken_is_numbered() {
        let dir = std::env::temp_dir().join(format!("spatishelf-free-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), b"").unwrap();
        std::fs::write(dir.join("a 2.jpg"), b"").unwrap();
        assert_eq!(free_name(&dir, "a.jpg"), dir.join("a 3.jpg"));
        assert_eq!(free_name(&dir, "b.jpg"), dir.join("b.jpg"));
        // A link that leads nowhere is something there.
        std::os::unix::fs::symlink("/no/such/thing", dir.join("c.jpg")).unwrap();
        assert_eq!(free_name(&dir, "c.jpg"), dir.join("c 2.jpg"));
        let _ = std::fs::remove_dir_all(&dir);
        // A remote's, as the storage service answers, and its error when it cannot.
        let there = ["Film.mkv", "Film 2.mkv", "Notes"];
        assert_eq!(free_among("Film.mkv", |n| Ok::<_, String>(there.contains(&n))), Ok("Film 3.mkv".into()));
        assert_eq!(free_among("Notes", |n| Ok::<_, String>(there.contains(&n))), Ok("Notes 2".into()));
        assert_eq!(free_among("new.txt", |n| Ok::<_, String>(there.contains(&n))), Ok("new.txt".into()));
        assert_eq!(free_among("Film.mkv", |_| Err::<bool, _>("no answer".to_string())), Err("no answer".into()));
    }

    #[test]
    fn a_rename_takes_no_name_already_there() {
        let dir = std::env::temp_dir().join(format!("spatishelf-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join("b.txt"), b"b").unwrap();
        std::os::unix::fs::symlink("/no/such/thing", dir.join("dangling")).unwrap();
        let kept = Mutex::new(None);
        let place = |p: &str| Place::Local(dir.join(p).to_string_lossy().into_owned());
        let mut quiet = |_: u64, _: u64, _: &str| {};
        assert!(run_op(Op::Rename { item: place("a.txt"), name: "b.txt".into() }, &kept, &mut quiet).is_err());
        assert!(run_op(Op::Rename { item: place("a.txt"), name: "dangling".into() }, &kept, &mut quiet).is_err(), "a link leading nowhere is not replaced");
        assert!(dir.join("dangling").symlink_metadata().unwrap().file_type().is_symlink());
        // Only its case changed: itself, not something else there.
        run_op(Op::Rename { item: place("a.txt"), name: "A.txt".into() }, &kept, &mut quiet).unwrap();
        assert_eq!(std::fs::read(dir.join("A.txt")).unwrap(), b"a");
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
        assert!(run_op(Op::Mkdir { inside: place("dst"), name: "new/inner".into() }, &kept, &mut quiet).is_err(), "a / is refused, as in a rename");
        assert!(!dir.join("dst/new/inner").exists());
        assert!(run_op(Op::Copy { from: vec![place("src")], to: place("src/inner") }, &kept, &mut quiet).is_err());
        // One of three that cannot go does not keep the others: they go, and the error says so.
        std::fs::write(dir.join("x"), b"x").unwrap();
        std::fs::write(dir.join("y"), b"y").unwrap();
        let error = run_op(Op::Move { from: vec![place("x"), place("gone"), place("y")], to: place("dst/new") }, &kept, &mut quiet).unwrap_err();
        assert!(error.starts_with("1 of 3 could not be done; the first: "), "{error}");
        assert!(dir.join("dst/new/x").exists() && dir.join("dst/new/y").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_not_utf8_are_listed_and_worked_on_by_their_own_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let dir = std::env::temp_dir().join(format!("spatishelf-bytes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dst")).unwrap();
        let os = |b: &[u8]| std::ffi::OsStr::from_bytes(b).to_os_string();
        std::fs::write(dir.join(os(b"caf\xe9.txt")), b"latin-1").unwrap();
        std::fs::write(dir.join(os(b"\xff\xfe")), b"bytes").unwrap();
        let folder = Place::Local(dir.to_string_lossy().into_owned());
        let kept = Mutex::new(None);
        let mut quiet = |_: u64, _: u64, _: &str| {};
        // Listed by names that are their bytes, shown lossily.
        let (items, _) = list_local(&dir.to_string_lossy()).unwrap();
        let cafe = items.iter().find(|i| proto::shown(&i.name) == "caf\u{FFFD}.txt").unwrap();
        let ff = items.iter().find(|i| proto::shown(&i.name) == "\u{FFFD}\u{FFFD}").unwrap();
        let cafe = folder.join(&cafe.name);
        let ff = folder.join(&ff.name);
        // The path to open is the file's own.
        assert_eq!(std::fs::read(cafe.path().unwrap()).unwrap(), b"latin-1");
        // Copied and moved, by the same bytes.
        run_op(Op::Copy { from: vec![cafe.clone()], to: folder.join("dst") }, &kept, &mut quiet).unwrap();
        assert_eq!(std::fs::read(dir.join("dst").join(os(b"caf\xe9.txt"))).unwrap(), b"latin-1");
        run_op(Op::Copy { from: vec![cafe.clone()], to: folder.join("dst") }, &kept, &mut quiet).unwrap();
        assert!(dir.join("dst").join(os(b"caf\xe9 2.txt")).exists(), "numbered beside, its bytes kept");
        run_op(Op::Move { from: vec![ff.clone()], to: folder.join("dst") }, &kept, &mut quiet).unwrap();
        assert_eq!(std::fs::read(dir.join("dst").join(os(b"\xff\xfe"))).unwrap(), b"bytes");
        assert!(dir.join(os(b"\xff\xfe")).symlink_metadata().is_err());
        // Renamed to a valid name, and from a valid name to one that is not.
        run_op(Op::Rename { item: cafe.clone(), name: "cafe.txt".into() }, &kept, &mut quiet).unwrap();
        assert_eq!(std::fs::read(dir.join("cafe.txt")).unwrap(), b"latin-1");
        run_op(Op::Rename { item: folder.join("cafe.txt"), name: proto::text_of(&os(b"caf\xe9.txt")) }, &kept, &mut quiet).unwrap();
        assert_eq!(std::fs::read(dir.join(os(b"caf\xe9.txt"))).unwrap(), b"latin-1");
        // Into a trash and back, the info file escaping the bytes.
        let can = crate::trash::Can { root: dir.join("Trash"), top: None };
        let put = can.put(&cafe.path().unwrap(), "2026-11-18T09:08:07", false).unwrap();
        let info = std::fs::read_to_string(can.info().join(os(b"caf\xe9.txt.trashinfo"))).unwrap();
        assert!(info.contains("caf%E9.txt"), "{info}");
        let listed = can.list();
        let trashed = Place::Local(proto::text_of(listed[0].file.as_os_str()));
        assert_eq!(trashed.path().unwrap(), put);
        can.restore(&trashed.path().unwrap()).unwrap();
        assert_eq!(std::fs::read(dir.join(os(b"caf\xe9.txt"))).unwrap(), b"latin-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_are_copied_as_links_and_a_pipe_is_refused() {
        let dir = std::env::temp_dir().join(format!("spatishelf-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("prefix/dosdevices")).unwrap();
        std::fs::create_dir_all(dir.join("dst")).unwrap();
        // A Proton prefix's: the whole disk, and a drive that is not there.
        std::os::unix::fs::symlink("/", dir.join("prefix/dosdevices/z:")).unwrap();
        std::os::unix::fs::symlink("/no/such/drive", dir.join("prefix/dosdevices/d:")).unwrap();
        let fifo = std::ffi::CString::new(dir.join("pipe").to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let kept = Mutex::new(None);
        let place = |p: &str| Place::Local(dir.join(p).to_string_lossy().into_owned());
        let mut quiet = |_: u64, _: u64, _: &str| {};
        run_op(Op::Copy { from: vec![place("prefix")], to: place("dst") }, &kept, &mut quiet).unwrap();
        assert_eq!(std::fs::read_link(dir.join("dst/prefix/dosdevices/z:")).unwrap(), Path::new("/"));
        assert_eq!(std::fs::read_link(dir.join("dst/prefix/dosdevices/d:")).unwrap(), Path::new("/no/such/drive"));
        assert!(run_op(Op::Copy { from: vec![place("pipe")], to: place("dst") }, &kept, &mut quiet).is_err(), "a pipe is refused, not waited on");
        assert!(dir.join("pipe").symlink_metadata().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
