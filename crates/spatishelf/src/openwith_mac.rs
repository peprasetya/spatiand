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

type Pixels = (u32, u32, Vec<u8>);

/// The application's icon as Finder draws it, drawn at `size` pixels a side, and kept: in
/// memory for this run, and on disk (`~/.cache/spatishelf/app-icons`) until the application
/// changes, so that "Other..."'s eighty-odd are there at once the next time.
///
/// Not by the icon's `TIFFRepresentation`: that renders every one of its sizes, the first
/// 1024x1024 in 16-bit floats (Preview's TIFF is 74 MB, 32 pictures), which `image` cannot
/// read, at some 130 ms and tens of MB an icon. Drawn instead into a bitmap of the size wanted,
/// AppKit choosing and scaling the icon's own picture for it.
pub fn icon(app: &App, size: u32) -> Option<Pixels> {
    static KEPT: std::sync::Mutex<Option<std::collections::HashMap<(String, u32), Option<Pixels>>>> = std::sync::Mutex::new(None);
    let key = (app.id.clone(), size);
    if let Some(had) = KEPT.lock().ok().and_then(|k| k.as_ref().and_then(|k| k.get(&key).cloned())) {
        return had;
    }
    let bundle = Path::new(&app.id);
    let file = cached_file(bundle, size);
    let got = file.as_deref().and_then(read_cached).or_else(|| {
        let drawn = draw(bundle, size)?;
        if let Some(file) = &file {
            write_cached(file, &drawn);
        }
        Some(drawn)
    });
    if let Ok(mut kept) = KEPT.lock() {
        kept.get_or_insert_with(Default::default).insert(key, got.clone());
    }
    got
}

/// The icon drawn at `size` pixels a side, as RGBA (alpha not multiplied in).
///
/// Off the main thread (SpatiShelf's worker), which AppKit allows for what is used here: none of
/// `NSWorkspace`, `NSImage`, `NSBitmapImageRep` and `NSGraphicsContext` is main-thread-only (the
/// SDK's headers mark none of them `NS_SWIFT_UI_ACTOR`, as they mark `NSView`; objc2 has none as
/// `MainThreadOnly`); an `NSImage` may be drawn on any thread so long as no thread changes it
/// meanwhile (this one is made here and only read); and the graphics context is this thread's
/// own: a bitmap context of its own, made current on this thread's context stack
/// (`+saveGraphicsState`/`+setCurrentContext:`/`+restoreGraphicsState`, which are per thread)
/// and taken off again before anything else is drawn on it. Everything Objective-C hands back
/// is let go within an autorelease pool of this call's own, the worker having none.
fn draw(bundle: &Path, size: u32) -> Option<Pixels> {
    use objc2_app_kit::{NSBitmapFormat, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace, NSGraphicsContext, NSImageInterpolation};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use objc2::AnyThread;
    if size == 0 || size > 1024 {
        return None;
    }
    objc2::rc::autoreleasepool(|_| {
        let icon = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&bundle.to_string_lossy()));
        let side = size as isize;
        // 8 bits a sample, RGBA, alpha last and multiplied in (what a Quartz bitmap context takes).
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                side,
                side,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                NSBitmapFormat(0),
                0,
                0,
            )
        }?;
        let (data, row) = (rep.bitmapData(), rep.bytesPerRow() as usize);
        if data.is_null() || row < size as usize * 4 {
            return None;
        }
        // Clear: a new bitmap's memory is not promised to be.
        unsafe { std::ptr::write_bytes(data, 0, row * size as usize) };
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        context.setImageInterpolation(NSImageInterpolation::High);
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(size as f64, size as f64));
        icon.drawInRect_fromRect_operation_fraction(rect, NSRect::ZERO, NSCompositingOperation::SourceOver, 1.0);
        context.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        // The bitmap's rows top first, as a texture's are.
        let mut rgba = Vec::with_capacity(size as usize * size as usize * 4);
        for y in 0..size as usize {
            let line = unsafe { std::slice::from_raw_parts(data.add(y * row), size as usize * 4) };
            rgba.extend_from_slice(line);
        }
        unpremultiply(&mut rgba);
        rgba.chunks_exact(4).any(|p| p[3] != 0).then_some((size, size, rgba))
    })
}

fn unpremultiply(rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// Where an application's icon at a size is kept on disk: named by its path and by when its
/// `Info.plist` (or the bundle) last changed, which an update of it changes.
fn cached_file(bundle: &Path, size: u32) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(bundle.join("Contents/Info.plist")).or_else(|_| std::fs::metadata(bundle)).ok()?;
    let stamp = format!("{}.{}", meta.mtime(), meta.mtime_nsec());
    Some(super::cache_dir().join("app-icons").join(format!("{:016x}-{size}-{stamp}.png", fnv(bundle.to_string_lossy().as_bytes()))))
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

fn read_cached(file: &Path) -> Option<Pixels> {
    let picture = image::open(file).ok()?.to_rgba8();
    let (w, h) = picture.dimensions();
    Some((w, h, picture.into_raw()))
}

/// Kept on disk, the same application's older pictures at this size taken away.
fn write_cached(file: &Path, (w, h, rgba): &Pixels) {
    let (Some(dir), Some(name)) = (file.parent(), file.file_name().map(|n| n.to_string_lossy().into_owned())) else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let same = name.splitn(3, '-').take(2).collect::<Vec<_>>().join("-") + "-";
    for old in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if old.file_name().to_string_lossy().starts_with(&same) {
            let _ = std::fs::remove_file(old.path());
        }
    }
    // Written beside and renamed, so that a reader never meets half of it.
    let part = dir.join(format!(".{name}.{}", std::process::id()));
    if image::save_buffer_with_format(&part, rgba, *w, *h, image::ExtendedColorType::Rgba8, image::ImageFormat::Png).is_ok() {
        let _ = std::fs::rename(&part, file);
    } else {
        let _ = std::fs::remove_file(&part);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_is_drawn_at_the_size_asked_on_any_thread() {
        let started = std::time::Instant::now();
        // The test's own thread is not the main one.
        let (w, h, rgba) = draw(Path::new("/System/Library/CoreServices/Finder.app"), 64).expect("Finder's icon drawn");
        assert_eq!((w, h, rgba.len()), (64, 64, 64 * 64 * 4));
        let opaque = rgba.chunks_exact(4).filter(|p| p[3] == 255).count();
        assert!(opaque > 64 * 64 / 3, "mostly opaque: {opaque}");
        assert!(rgba.chunks_exact(4).any(|p| p[3] == 0), "transparent about its edge");
        let colours: std::collections::HashSet<_> = rgba.chunks_exact(4).filter(|p| p[3] == 255).map(|p| [p[0] / 32, p[1] / 32, p[2] / 32]).collect();
        assert!(colours.len() > 4, "a picture, not a fill: {colours:?}");
        let other = std::thread::spawn(|| draw(Path::new("/System/Applications/Preview.app"), 128)).join().unwrap().expect("Preview's icon drawn");
        assert_eq!((other.0, other.1), (128, 128));
        eprintln!("two icons drawn in {:?}", started.elapsed());
    }

    #[test]
    fn icons_kept_on_disk_read_back_and_replace_older_ones() {
        let dir = std::env::temp_dir().join(format!("spatishelf-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let picture: Pixels = (2, 1, vec![255, 0, 0, 255, 0, 0, 255, 128]);
        let old = dir.join("00000000000000ab-64-1.0.png");
        let new = dir.join("00000000000000ab-64-2.0.png");
        let other = dir.join("00000000000000ab-128-1.0.png");
        write_cached(&old, &picture);
        write_cached(&other, &picture);
        write_cached(&new, &picture);
        assert_eq!(read_cached(&new), Some(picture));
        assert!(!old.exists(), "the application's older picture at this size went");
        assert!(other.exists(), "its other size stays");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
