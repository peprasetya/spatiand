//! spatimesh-webkitgtk: SpatiMesh's engine host on a Linux that is not SteamOS.
//!
//! Started by SpatiMesh's window process, one per profile in use, with its end of a socket at
//! descriptor 3 (`spatimesh::host`); WebKitGTK draws each tab's page offscreen into shared
//! memory, and the engine's own processes hold the pages, one per tab. See docs/spatimesh.md.

#[cfg(webkitgtk)]
mod engine;

fn main() {
    #[cfg(webkitgtk)]
    spatimesh::host::main(|setup| {
        engine::WebKitGtk::new(setup).map(|engine| Box::new(engine) as Box<dyn spatimesh::engine::Engine>)
    });
    #[cfg(not(webkitgtk))]
    {
        eprintln!("spatimesh-webkitgtk was built where there was no WebKitGTK (webkit2gtk-4.1), and cannot show pages");
        std::process::exit(1);
    }
}
