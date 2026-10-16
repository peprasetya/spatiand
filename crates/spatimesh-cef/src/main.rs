//! spatimesh-cef: SpatiMesh's engine host on SteamOS, with Steam's own Chromium.
//!
//! Started by SpatiMesh's window process, one per profile in use, with its end of a socket at
//! descriptor 3 (`spatimesh::host`); Chromium draws each tab's page windowless into memory, and
//! its own processes hold the pages -- this program again, started by Chromium with `--type=`.
//! See docs/spatimesh.md.

#[cfg(cef)]
mod cef {
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int};
    use std::os::unix::ffi::OsStringExt;

    extern "C" {
        fn spatimesh_cef_args(argc: c_int, argv: *mut *mut c_char);
        fn spatimesh_cef_subprocess() -> c_int;
        fn spatimesh_cef_error() -> *const c_char;
        fn spatimesh_cef_shutdown();
    }

    fn error() -> Option<String> {
        let p = unsafe { spatimesh_cef_error() };
        let text = if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() };
        (!text.is_empty()).then_some(text)
    }

    /// Where libcef.so is: SPATIMESH_CEF_DIR, or the Steam client's own (native/cef.c looks in
    /// the same places, SPATIMESH_CEF_DIR first).
    fn cef_dir() -> Option<std::path::PathBuf> {
        if let Some(dir) = std::env::var_os("SPATIMESH_CEF_DIR").filter(|d| !d.is_empty()) {
            return Some(dir.into());
        }
        let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
        [".local/share/Steam/ubuntu12_64", ".steam/steam/ubuntu12_64", ".steam/root/ubuntu12_64"]
            .into_iter()
            .map(|place| home.join(place))
            .find(|dir| dir.join("libcef.so").is_file())
    }

    /// libcef.so loaded with the program, before it starts: Chromium keeps thread-local data
    /// where a library loaded later (dlopen) cannot have it ("cannot allocate memory in static
    /// TLS block"). So the program starts itself again, once, with the library preloaded;
    /// Chromium's own processes, started from this one, inherit it.
    fn preload_cef() {
        use std::os::unix::process::CommandExt;
        if std::env::var_os("SPATIMESH_CEF_PRELOADED").is_some() {
            return;
        }
        let Some(dir) = cef_dir() else { return };
        let library = dir.join("libcef.so");
        if !library.is_file() {
            return;
        }
        let Ok(program) = std::env::current_exe() else { return };
        let mut preload = library.into_os_string();
        if let Some(more) = std::env::var_os("LD_PRELOAD").filter(|p| !p.is_empty()) {
            preload.push(":");
            preload.push(more);
        }
        let mut args = std::env::args_os();
        let name = args.next().unwrap_or_else(|| program.clone().into_os_string());
        let error = std::process::Command::new(&program)
            .arg0(name)
            .args(args)
            .env("LD_PRELOAD", preload)
            .env("SPATIMESH_CEF_DIR", &dir)
            .env("SPATIMESH_CEF_PRELOADED", "1")
            .exec();
        // Not started again: the library is loaded the usual way, and says what is wrong.
        eprintln!("spatimesh-cef: could not start itself with libcef.so preloaded: {error}");
    }

    pub fn main() {
        preload_cef();
        // Chromium reads the program's arguments, and keeps them: given for good.
        let args: Vec<CString> =
            std::env::args_os().map(|a| CString::new(a.into_vec()).unwrap_or_default()).collect();
        let args: &'static [CString] = Vec::leak(args);
        let mut argv: Vec<*mut c_char> = args.iter().map(|a| a.as_ptr() as *mut c_char).collect();
        argv.push(std::ptr::null_mut());
        let argv: &'static mut [*mut c_char] = Vec::leak(argv);
        unsafe { spatimesh_cef_args((argv.len() - 1) as c_int, argv.as_mut_ptr()) };
        // One of Chromium's own processes: its to run.
        if std::env::args().skip(1).any(|a| a.starts_with("--type=")) {
            std::process::exit(unsafe { spatimesh_cef_subprocess() });
        }
        spatimesh::host::main(|setup| {
            spatimesh_shim::Shim::new(setup)
                .map(|engine| Box::new(engine) as Box<dyn spatimesh::engine::Engine>)
                .map_err(|e| error().unwrap_or(e))
        });
        unsafe { spatimesh_cef_shutdown() };
    }
}

fn main() {
    #[cfg(cef)]
    cef::main();
    #[cfg(not(cef))]
    {
        eprintln!("spatimesh-cef was built without CEF's headers (CEF_ROOT), and cannot show pages");
        std::process::exit(1);
    }
}
