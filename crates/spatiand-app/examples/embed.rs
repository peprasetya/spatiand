//! A place in a window for another application's window (`spatiand_xr_embed_v1`), both sides.
//!
//! Run without arguments it is the host: a blue window with a place in its middle, which starts
//! itself again as the window inside (`--inside`, given the place's token as
//! `SPATIAND_EMBED_TOKEN`). The window inside is orange; it says every key typed into it to the
//! host, and the host answers "green", which turns it green. Everything each side hears is logged
//! (`host:` and `inside:`), which is what `tools/spatiand-embed-e2e.py` reads.
//!
//! `--page <address>`: the inside is a page of SpatiMesh's instead (`App::show_page`) -- a `file:`
//! address of a PDF shows SpatiMesh's PDF viewer in the place.
//!
//! ```text
//! SPATIAND_BACKEND=snapshot SPATIAND_CLIENT=target/debug/examples/embed target/debug/spatiand
//! ```

use spatiand_app::{App, EmbedId, Event, WindowId};

/// One colour over the whole window, blue, green, red, alpha.
fn fill(app: &mut App, window: WindowId, bgra: [u8; 4]) {
    if let Err(e) = app.show_pixels(window, |pixels| {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&bgra);
        }
    }) {
        log::warn!("could not draw: {e}");
    }
}

const BLUE: [u8; 4] = [0xc0, 0x50, 0x20, 0xff];
const ORANGE: [u8; 4] = [0x20, 0x8c, 0xf5, 0xff];
const GREEN: [u8; 4] = [0x40, 0xc8, 0x30, 0xff];

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if std::env::args().any(|a| a == "--inside") {
        inside()
    } else {
        host()
    }
}

fn host() -> Result<(), String> {
    let mut app = App::connect("id.prasetya.spatiand.embed-test")?;
    let window = app.open("Embed host", (800, 500));
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
                        app.set_embed_rect(place, 200, 125, 400, 250);
                        let token = app.embed_token(place).unwrap_or_default();
                        log::info!("host: a place, its token {} long", token.len());
                        let page = std::env::args().skip_while(|a| a != "--page").nth(1);
                        if let Some(page) = page {
                            match app.show_page(place, &page, None) {
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
