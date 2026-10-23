//! spatimesh-wpe: SpatiMesh's engine host on WPE WebKit.
//!
//! Started by SpatiMesh's window process, one per profile in use, with its end of a socket at
//! descriptor 3 (`spatimesh::host`); WPE draws each tab's page through SpatiMesh's own WPE
//! platform into memory the window reads, WebKit's own processes holding the pages. See
//! docs/spatimesh.md.

/// What WPE reads from the environment and is to find in Spatiand's own copy: GIO's TLS module
/// (glib-networking, on OpenSSL) and, on a Mac, the font configuration that gives every script
/// its font (third_party/wpe/mac/fonts.conf). In the app beside this program; while developing,
/// where third_party/wpe/build.sh put them. Set before any thread is.
#[cfg(wpe)]
fn find_own() {
    let exe = std::env::current_exe().ok();
    let beside = exe.as_ref().and_then(|e| e.parent()).map(|d| d.join("../Frameworks"));
    let built = std::path::PathBuf::from(env!("SPATIMESH_WPE_DIR")).join("lib");
    if std::env::var_os("GIO_EXTRA_MODULES").is_none() {
        let modules = [beside.as_ref().map(|d| d.join("gio/modules")), Some(built.join("gio/modules"))];
        if let Some(found) = modules.into_iter().flatten().find(|m| m.is_dir()) {
            std::env::set_var("GIO_EXTRA_MODULES", found);
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
