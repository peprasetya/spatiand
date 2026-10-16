//! SpatiMesh: Spatiand's web browser.
//!
//! A full browser (docs/spatimesh.md), drawn straight into the room, on the platform's own web
//! engine. This is its first step: a window of its own in the room.

use std::time::Duration;

use spatiand_app::ui::{look, theme, Painter, Rect};
use spatiand_app::{App, Event};

const WINDOW: (u32, u32) = (1600, 1000);

fn run() -> Result<(), String> {
    let mut app = App::connect("id.prasetya.spatiand.spatimesh")?;
    let window = app.open("SpatiMesh", WINDOW);
    let mut painter: Option<Painter> = None;
    let mut size = WINDOW;
    let mut dirty = false;
    loop {
        for event in app.wait(Some(Duration::from_millis(1000)))? {
            match event {
                Event::Ready { .. } => dirty = true,
                Event::Resized { width, height, .. } => {
                    size = (width.max(320), height.max(240));
                    app.resize(window, size);
                    dirty = true;
                }
                Event::CloseRequested { .. } => {
                    app.close(window);
                    return Ok(());
                }
                _ => {}
            }
        }
        if dirty {
            dirty = false;
            app.draw(window, |gl, picture| {
                let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
                p.begin(gl, picture, picture.1 as f32 / size.1 as f32, theme::GROUND);
                look::backdrop(p, gl);
                let all = p.bounds();
                let line = Rect::new(0.0, all.h * 0.5 - 40.0, all.w, 80.0);
                p.label(gl, "SpatiMesh", line, theme::TITLE_EM, theme::INK, 0.5);
                p.end(gl);
            })?;
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if let Err(e) = run() {
        log::error!("{e}");
        std::process::exit(1);
    }
}
