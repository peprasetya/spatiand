//! spatimesh-wpe: SpatiMesh's engine host on WPE WebKit.
//!
//! Started by SpatiMesh's window process, one per profile in use, with its end of a socket at
//! descriptor 3 (`spatimesh::host`); WPE draws each tab's page through SpatiMesh's own WPE
//! platform into memory the window reads, WebKit's own processes holding the pages. See
//! docs/spatimesh.md.

/// What WPE reads from the environment and is to find in Spatiand's own copy: GIO's TLS module
/// (glib-networking, on OpenSSL), GStreamer's plugins, the developer tools' pages and, on a Mac, the font configuration that gives every script
/// its font (third_party/wpe/mac/fonts.conf). In the app beside this program; while developing,
/// where third_party/wpe/build.sh put them. Set before any thread is.
#[cfg(wpe)]
fn find_own() {
    let exe = std::env::current_exe().ok();
    let beside = exe.as_ref().and_then(|e| e.parent()).map(|d| d.join("../Frameworks/wpe/lib"));
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
        if let Some((lib, name)) = found {
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
        let resources = exe.as_ref().and_then(|e| e.parent()).map(|d| d.join("../Resources/wpe"));
        let built = std::path::PathBuf::from(env!("SPATIMESH_WPE_DIR")).join("share/wpe-webkit-2.0");
        if let Some(found) = [resources, Some(built)].into_iter().flatten().find(|d| d.join("inspector.gresource").is_file()) {
            std::env::set_var("WEBKIT_INSPECTOR_RESOURCES_PATH", found);
        }
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
