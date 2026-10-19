//! spatimesh-webkit: SpatiMesh's engine host on the Mac.
//!
//! Started by SpatiMesh's window process, one per profile in use, with its end of a socket at
//! descriptor 3 (`spatimesh::host`); WebKit draws each tab's page into a window nobody sees,
//! WebKit's own processes holding the pages, one per tab. See docs/spatimesh.md.

fn main() {
    #[cfg(target_os = "macos")]
    spatimesh::host::main(|setup| spatimesh_shim::Shim::new(setup).map(|engine| Box::new(engine) as Box<dyn spatimesh::engine::Engine>));
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("spatimesh-webkit is SpatiMesh's engine on a Mac, and does nothing here");
        std::process::exit(1);
    }
}
