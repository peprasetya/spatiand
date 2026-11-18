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
            trashed: Some(crate::proto::Trashed { file: e.file.to_string_lossy().into_owned(), origin: e.origin.map(|o| o.to_string_lossy().into_owned()), deleted: e.deleted }),
            name: e.name,
        })
        .collect();
    sort(&mut items);
    Ok((items, None))
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

fn is_dir(place: &Place, storage: &Storage) -> bool {
    match place {
        Place::Trash => false,
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
        let from = Path::new(from);
        let target = free_name(Path::new(to), &name);
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

fn delete(place: &Place, storage: &Mutex<Option<Storage>>) -> Result<(), String> {
    match place {
        Place::Trash => Ok(()),
        // From the trash, with what it knew of it; anywhere else, just gone.
        Place::Local(p) => crate::trash::purge(Path::new(p)),
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
        // Moved onto the trash: put in it.
        Op::Move { from, to: Place::Trash } => run_op(Op::Trash(from), storage, report),
        Op::Move { from, to } => from.iter().try_for_each(|item| {
            if item.parent().as_ref() == Some(&to) {
                return Ok(());
            }
            transfer(item, &to, true, storage, report)?;
            // Taken out of the trash by a move: what the trash knew of it goes too.
            if let Place::Local(p) = item {
                crate::trash::moved_out(Path::new(p));
            }
            Ok(())
        }),
        Op::Trash(items) => items.iter().try_for_each(|item| match item {
            Place::Local(p) => crate::trash::put(Path::new(p)),
            Place::Remote(..) => Err("A remote has no trash here: delete it instead.".into()),
            Place::Trash => Ok(()),
        }),
        Op::Delete(items) => items.iter().try_for_each(|item| delete(item, storage)),
        Op::Restore(items) => items.iter().try_for_each(|item| match item {
            Place::Local(p) => crate::trash::restore(Path::new(p)).map(|_| ()),
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
            Place::Trash => Err("No folder is made in the Trash.".into()),
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
    let mut here: Option<Place> = None;
    let mut items: Vec<Item> = Vec::new();
    let list = |place: &Place, kept: &Mutex<Option<Storage>>| -> Result<(Vec<Item>, Option<u64>), String> {
        match place {
            Place::Local(p) => list_local(p),
            Place::Remote(r, p) => list_remote(&storage(kept)?, r, p),
            Place::Trash => list_trash(),
        }
    };
    // The folders on show, watched: a change from anywhere is told to the window.
    let watching = {
        let out = out.clone();
        crate::watch::start(move |place| tell(&out, &FromTab::Changed(place)))
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
            ToTab::List(place) => {
                // On a thread: a remote's folder may take its time, or never answer, and the
                // tab goes on meanwhile.
                let out = out.clone();
                let kept = kept.clone();
                std::thread::spawn(move || {
                    let items = match &place {
                        Place::Local(p) => list_local(p),
                        Place::Remote(r, p) => storage(&kept).and_then(|s| list_remote(&s, r, p)),
                        Place::Trash => list_trash(),
                    };
                    tell(&out, &FromTab::Contents { place, items: items.map(|(items, _)| items) });
                });
            }
            ToTab::Pictures { indices, size } => {
                let Some(place) = here.clone() else { continue };
                for index in indices {
                    if let Some(item) = items.get(index).filter(|i| i.kind.pictured()) {
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
