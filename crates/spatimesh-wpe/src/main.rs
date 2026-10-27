//! spatimesh-wpe: SpatiMesh's engine host on WPE WebKit.
//!
//! Started by SpatiMesh's window process, one per profile in use, with its end of a socket at
//! descriptor 3 (`spatimesh::host`); WPE draws each tab's page through SpatiMesh's own WPE
//! platform into memory the window reads, WebKit's own processes holding the pages. See
//! docs/spatimesh.md.

/// What WPE reads from the environment and is to find in Spatiand's own copy: GIO's TLS module
/// (glib-networking, on OpenSSL), GStreamer's plugins, the developer tools' pages and the font
/// configuration: on a Mac the one that gives every script its font (third_party/wpe/mac/
/// fonts.conf), on Linux the system's. In the app beside this program (in a Mac's
/// Contents/Frameworks/wpe, Linux's wpe/ folder); while developing, where
/// third_party/wpe/build.sh put them. Set before any thread is.
#[cfg(wpe)]
fn find_own() {
    #[cfg(target_os = "android")]
    android_own();
    let exe = std::env::current_exe().ok();
    let own = if cfg!(target_os = "macos") { "../Frameworks/wpe/lib" } else { "wpe/lib" };
    let beside = exe.as_ref().and_then(|e| e.parent()).map(|d| d.join(own)).filter(|d| d.is_dir());
    let built = std::path::PathBuf::from(env!("SPATIMESH_WPE_DIR")).join("lib");
    if std::env::var_os("GIO_EXTRA_MODULES").is_none() {
        let modules = [beside.as_ref().map(|d| d.join("gio/modules")), Some(built.join("gio/modules"))];
        if let Some(found) = modules.into_iter().flatten().find(|m| m.is_dir()) {
            std::env::set_var("GIO_EXTRA_MODULES", found);
        }
    }
    // GStreamer's plugins and the program that looks into them, beside WPE's libraries; its
    // list of them where caches go (made again when it is missing or stale).
    if std::env::var_os("GST_PLUGIN_SYSTEM_PATH").is_none() {
        // In the app the folder is gstreamer-1_0 (no dot: codesign takes a dotted folder for a
        // bundle, mac/make-app.sh).
        let found = [beside.clone(), Some(built.clone())].into_iter().flatten().find_map(|lib| {
            ["gstreamer-1_0", "gstreamer-1.0"].into_iter().find(|name| lib.join(name).is_dir()).map(|name| (lib, name))
        });
        // GStreamer's own plugins there (WPE built with its own GStreamer): those, and none of
        // the system's. Only Spatiand's sink there (WPE built on the system's): the system's
        // plugins, and it besides.
        let own = |dir: &std::path::Path| ["libgstcoreelements.so", "libgstcoreelements.dylib"].iter().any(|f| dir.join(f).is_file());
        if let Some((lib, name)) = found.as_ref().filter(|(lib, name)| !own(&lib.join(name))) {
            std::env::set_var("GST_PLUGIN_PATH", lib.join(name));
        } else if let Some((lib, name)) = found {
            std::env::set_var("GST_PLUGIN_SYSTEM_PATH", lib.join(name));
            std::env::set_var("GST_PLUGIN_PATH", "");
            let scanner = lib.join("../libexec").join(name).join("gst-plugin-scanner");
            if scanner.is_file() {
                std::env::set_var("GST_PLUGIN_SCANNER", scanner);
            }
            let cache = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache/spatiand/gstreamer-1.0"));
            if let Some(cache) = cache {
                let _ = std::fs::create_dir_all(&cache);
                std::env::set_var("GST_REGISTRY", cache.join("registry.bin"));
            }
        }
    }
    // The developer tools' own pages (WebKit's inspector.gresource): in the app's Resources,
    // or where the build put them.
    if std::env::var_os("WEBKIT_INSPECTOR_RESOURCES_PATH").is_none() {
        let own = if cfg!(target_os = "macos") { "../Resources/wpe" } else { "wpe/share/wpe-webkit-2.0" };
        let resources = exe.as_ref().and_then(|e| e.parent()).map(|d| d.join(own));
        let built = std::path::PathBuf::from(env!("SPATIMESH_WPE_DIR")).join("share/wpe-webkit-2.0");
        if let Some(found) = [resources, Some(built)].into_iter().flatten().find(|d| d.join("inspector.gresource").is_file()) {
            std::env::set_var("WEBKIT_INSPECTOR_RESOURCES_PATH", found);
        }
    }
    // The bundle's fontconfig is told of the system's fonts and their configuration, which it
    // was not built to look for (it was built for its own folder).
    #[cfg(target_os = "linux")]
    if std::env::var_os("FONTCONFIG_FILE").is_none() && beside.is_some() && std::path::Path::new("/etc/fonts/fonts.conf").is_file() {
        std::env::set_var("FONTCONFIG_FILE", "/etc/fonts/fonts.conf");
    }
    #[cfg(target_os = "macos")]
    if std::env::var_os("FONTCONFIG_FILE").is_none() {
        let resources = exe.as_ref().and_then(|e| e.parent()).map(|d| d.join("../Resources/fonts.conf"));
        let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/wpe/mac/fonts.conf");
        if let Some(found) = [resources, Some(repo)].into_iter().flatten().find(|f| f.is_file()) {
            std::env::set_var("FONTCONFIG_FILE", found);
        }
    }
}

/// On Android everything of WPE's is in the app's library folder, flat, each named lib*.so (the
/// only files an APK's libraries may be): its processes, its libraries, GIO's TLS module and
/// GStreamer's plugins together. GIO and GStreamer load every library in the folder they are
/// pointed at, so each is pointed at a folder of its own in the app's cache, of links to its
/// own files among them; fontconfig is given Android's fonts (there is no configuration of
/// them on Android) and GStreamer's registry is made in this process, there being no scanner.
#[cfg(all(wpe, target_os = "android"))]
fn android_own() {
    use std::path::{Path, PathBuf};
    let Some(lib) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) else { return };
    let cache = std::env::var_os("TMPDIR").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("spatimesh-wpe");
    let linked = |name: &str, wanted: &dyn Fn(&str) -> bool| -> Option<PathBuf> {
        let dir = cache.join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        for entry in std::fs::read_dir(&lib).ok()?.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if wanted(&file) {
                let _ = std::os::unix::fs::symlink(entry.path(), dir.join(&file));
            }
        }
        Some(dir)
    };
    if std::env::var_os("GIO_MODULE_DIR").is_none() {
        if let Some(dir) = linked("gio-modules", &|f| f == "libgioopenssl.so") {
            std::env::set_var("GIO_MODULE_DIR", dir);
        }
    }
    // Plugins are libgst<name>.so; GStreamer's own libraries are libgst<name>-1.0.so.
    if std::env::var_os("GST_PLUGIN_SYSTEM_PATH").is_none() {
        if let Some(dir) = linked("gstreamer-1.0", &|f| f.starts_with("libgst") && !f.contains("-1.0")) {
            std::env::set_var("GST_PLUGIN_SYSTEM_PATH", dir);
            std::env::set_var("GST_PLUGIN_PATH", "");
            std::env::set_var("GST_REGISTRY_FORK", "no");
            std::env::set_var("GST_REGISTRY", cache.join("gstreamer-registry.bin"));
        }
    }
    if std::env::var_os("FONTCONFIG_FILE").is_none() {
        let fonts = cache.join("fonts.conf");
        let text = include_str!("../../../third_party/wpe/android/fonts.conf").replace("CACHE", &cache.join("fontconfig").to_string_lossy());
        if std::fs::write(&fonts, text).is_ok() {
            std::env::set_var("FONTCONFIG_FILE", fonts);
        }
    }
}

fn main() {
    watch_parent();
    #[cfg(wpe)]
    {
        find_own();
        spatimesh::host::main(|setup| spatimesh_shim::Shim::new(setup).map(|engine| Box::new(engine) as Box<dyn spatimesh::engine::Engine>));
    }
    #[cfg(not(wpe))]
    {
        eprintln!("spatimesh-wpe was built where there was no WPE WebKit (third_party/wpe/build.sh), and cannot show pages");
        std::process::exit(1);
    }
}

/// Ends with the SpatiMesh that started it: orphaned, an engine would go on for good.
fn watch_parent() {
    let parent = unsafe { libc::getppid() };
    if parent <= 1 {
        return;
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if unsafe { libc::getppid() } != parent {
            std::process::exit(0);
        }
    });
}
