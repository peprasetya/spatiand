//! A place in a window for another application's window (`spatiand_xr_embed_v1`), both sides.
//!
//! Run without arguments it is the host: a blue window with a place in its middle, which starts
//! itself again as the window inside (`--inside`, given the place's token as
//! `SPATIAND_EMBED_TOKEN`). The window inside is orange; it says every key typed into it to the
//! host, and the host answers "green", which turns it green. Everything each side hears is logged
//! (`host:` and `inside:`), which is what `tools/spatiand-embed-e2e.py` reads.
//!
//! `--notify <words>`: only a notice on the room's card (`App::notify`).
//!
//! `--old-spatimesh`: a SpatiMesh too old for places, as the room sees one -- a window of
//! SpatiMesh's application id naming no place, from a client bound to `spatiand_xr_v1` before 13
//! (run it with `SPATIAND_XR_MOST=12`): what such a SpatiMesh, asked into a place, opens instead.
//!
//! `--page <address>`: the inside is a page of SpatiMesh's instead (`App::show_page`) -- a `file:`
//! address of a PDF shows SpatiMesh's PDF viewer in the place.
//!
//! ```text
//! SPATIAND_BACKEND=snapshot SPATIAND_CLIENT=target/debug/examples/embed target/debug/spatiand
//! ```

use spatiand_app::{App, EmbedId, Event, WindowId};

/// One colour over the whole window, blue, green, red, alpha. Drawn by the GPU, as Spatiand's
/// applications draw: on a Mac's desktop (2D mode) a window is shown only from a GPU's picture.
fn fill(app: &mut App, window: WindowId, bgra: [u8; 4]) {
    let [b, g, r, a] = bgra.map(|c| c as f32 / 255.0);
    let drawn = app.draw(window, |gl, _| unsafe {
        use spatiand_app::glow::HasContext;
        gl.api().clear_color(r, g, b, a);
        gl.api().clear(spatiand_app::glow::COLOR_BUFFER_BIT);
    });
    if let Err(e) = drawn {
        log::warn!("could not draw: {e}");
    }
}

const BLUE: [u8; 4] = [0xc0, 0x50, 0x20, 0xff];
const ORANGE: [u8; 4] = [0x20, 0x8c, 0xf5, 0xff];
const GREEN: [u8; 4] = [0x40, 0xc8, 0x30, 0xff];

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // `--notify <words>`: a notice on the room's card, and nothing else (App::notify).
    if let Some(words) = std::env::args().skip_while(|a| a != "--notify").nth(1) {
        let mut app = App::connect("id.prasetya.spatiand.embed-test")?;
        app.notify("Embed test", &words, "from the SDK's example")?;
        log::info!("notified");
        return Ok(());
    }
    if std::env::args().any(|a| a == "--old-spatimesh") {
        return old_spatimesh();
    }
    if std::env::args().any(|a| a == "--inside") {
        inside()
    } else {
        host()
    }
}

fn host() -> Result<(), String> {
    let mut app = App::connect("id.prasetya.spatiand.embed-test")?;
    // A page of SpatiMesh's fills the window, as a viewer's would; the window inside is a middle.
    let page = std::env::args().skip_while(|a| a != "--page").nth(1);
    let window = app.open("Embed host", if page.is_some() { (1280, 800) } else { (800, 500) });
    let place_of = |w: i32, h: i32| if page.is_some() { (0, 0, w, h) } else { (w / 4, h / 4, w / 2, h / 2) };
    let mut embed: Option<EmbedId> = None;
    loop {
        for event in app.wait(None)? {
            match event {
                Event::Ready { .. } => {
                    if embed.is_none() {
                        let Some(place) = app.embed(window) else {
                            log::error!("host: this room keeps no places");
                            return Ok(());
                        };
                        embed = Some(place);
                        let (w, h) = app.size(window);
                        let (x, y, pw, ph) = place_of(w as i32, h as i32);
                        app.set_embed_rect(place, x, y, pw, ph);
                        // In effect now, before the window inside arrives, not with this window's
                        // first picture, which the GPU may be a while making.
                        app.commit(window);
                        let token = app.embed_token(place).unwrap_or_default();
                        log::info!("host: a place, its token {} long", token.len());
                        if let Some(page) = page.as_deref() {
                            match app.show_page(place, page, None) {
                                Ok(()) => log::info!("host: SpatiMesh asked for the page"),
                                Err(e) => log::error!("host: SpatiMesh could not be asked: {e}"),
                            }
                        } else {
                            // Its own connection to the room: the one handed down is this program's.
                            let started = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
                                .arg("--inside")
                                .env("SPATIAND_EMBED_TOKEN", token)
                                .env_remove("SPATIAND_APP_FD")
                                .spawn();
                            if let Err(e) = started {
                                log::error!("host: could not start the window inside: {e}");
                            }
                        }
                    }
                    fill(&mut app, window, BLUE);
                }
                Event::Resized { width, height, .. } => {
                    app.resize(window, (width, height));
                    if let Some(place) = embed {
                        let (x, y, pw, ph) = place_of(width as i32, height as i32);
                        app.set_embed_rect(place, x, y, pw, ph);
                    }
                    fill(&mut app, window, BLUE);
                }
                Event::EmbedAttached { .. } => log::info!("host: a window is inside the place"),
                Event::EmbedDetached { .. } => log::info!("host: the window inside has gone"),
                Event::EmbedMessage { embed: place, message, .. } => {
                    log::info!("host: the window inside says {message:?}");
                    if message.starts_with("key") {
                        app.embed_send(place, "green");
                    }
                }
                Event::Key { pressed: true, code, .. } => log::info!("host: a key came to the host ({code})"),
                Event::Button { pressed: true, .. } => log::info!("host: a press on the host"),
                Event::Focus { focused, .. } => log::info!("host: keyboard {}", if focused { "here" } else { "gone" }),
                Event::CloseRequested { .. } | Event::Quit { .. } => return Ok(()),
                _ => {}
            }
        }
    }
}

fn old_spatimesh() -> Result<(), String> {
    let mut app = App::connect("spatimesh-chromium")?;
    let window = app.open("SpatiMesh stand-in", (640, 480));
    loop {
        for event in app.wait(None)? {
            match event {
                Event::Ready { .. } => {
                    log::info!("old SpatiMesh: a window of its own");
                    fill(&mut app, window, GREEN);
                }
                Event::Resized { width, height, .. } => {
                    app.resize(window, (width, height));
                    fill(&mut app, window, GREEN);
                }
                Event::CloseRequested { .. } | Event::Quit { .. } => return Ok(()),
                _ => {}
            }
        }
    }
}

fn inside() -> Result<(), String> {
    let mut app = App::connect("id.prasetya.spatiand.embed-test.inside")?;
    let window = app.open("Embed inside", (300, 200));
    let mut colour = ORANGE;
    loop {
        for event in app.wait(None)? {
            match event {
                Event::Ready { suggested, .. } => {
                    log::info!("inside: ready, offered {suggested:?}");
                    fill(&mut app, window, colour);
                }
                Event::Resized { width, height, .. } => {
                    log::info!("inside: sized {width}x{height}");
                    app.resize(window, (width, height));
                    fill(&mut app, window, colour);
                }
                Event::Key { pressed: true, code, .. } => {
                    log::info!("inside: a key ({code})");
                    app.post_to_host(window, &format!("key {code}"));
                }
                Event::Button { pressed: true, .. } => log::info!("inside: a press"),
                Event::Focus { focused, .. } => log::info!("inside: keyboard {}", if focused { "here" } else { "gone" }),
                Event::HostMessage { message, .. } => {
                    log::info!("inside: the host says {message:?}");
                    if message == "green" {
                        colour = GREEN;
                        fill(&mut app, window, colour);
                    }
                }
                Event::CloseRequested { .. } | Event::Quit { .. } => {
                    log::info!("inside: asked to close");
                    return Ok(());
                }
                _ => {}
            }
        }
    }
}
