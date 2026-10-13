//! Every widget on one window: how the toolkit is looked at.
//!
//! ```text
//! cd crates/spatiand-mac && SPATIAND_CLIENT=../../target/debug/examples/gallery \
//!     SPATIAND_SNAPSHOT=/tmp/gallery.png cargo run
//! ```
//!
//! `GALLERY_TAB=1` starts on the grid, `GALLERY_TYPE=text` types into the field, and
//! `GALLERY_PNG=file` writes what was drawn to a file with no compositor's help.

use spatiand_app::ui::{theme, Button, Field, Grid, Input, List, Painter, Rect, Row, Shape, Tabs, Tile};
use spatiand_app::{App, Event, Key, Mode};

fn main() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut app = App::connect("id.prasetya.spatiand.gallery")?;
    let window = app.open("Gallery", (1280, 800));
    let mut painter: Option<Painter> = None;

    let mut tabs = Tabs::default();
    tabs.active = std::env::var("GALLERY_TAB").ok().and_then(|t| t.parse().ok()).unwrap_or(0);
    let mut list = List::default();
    let mut grid = Grid::default();
    grid.shape = Shape::Tall;
    let mut field = Field::default();
    field.hint = "Search".into();
    if let Ok(typed) = std::env::var("GALLERY_TYPE") {
        field.set(typed);
    }
    let mut button = Button::default();
    let mut presses = 0;

    let rows: Vec<Row> = std::iter::once(Row::heading("Films"))
        .chain((1..=14).map(|n| Row::with(format!("A film with a name, number {n}"), format!("{} min", 80 + n * 3))))
        .chain(std::iter::once(Row::heading("Shows")))
        .chain((1..=9).map(|n| Row::new(format!("A show, season {n}"))))
        .collect();
    let mut tiles: Vec<Tile> = (1..=40)
        .map(|n| Tile {
            title: format!("{} of forty", ["Arrival", "Blade Runner", "Contact", "Dune", "Ex Machina"][n % 5]),
            detail: Some(format!("{}", 1980 + n)),
            picture: None,
            progress: (n % 4 == 1).then_some(n as f32 / 40.0),
        })
        .collect();

    loop {
        let mut dirty = false;
        for event in app.wait(None)? {
            match &event {
                Event::CloseRequested { .. } => return Ok(()),
                Event::Mode(mode) => log::info!("the wearer now sees {}", if *mode == Mode::Stereo { "two pictures" } else { "one" }),
                Event::Resized { width, height, .. } => app.resize(window, (*width, *height)),
                _ => {}
            }
            if let Some(input) = Input::of(&event, 1.0) {
                if let Input::Key { key: Key::Tab, .. } = input {
                    tabs.active = (tabs.active + 1) % 2;
                }
                tabs.input(&input);
                list.focused = tabs.active == 0;
                grid.focused = tabs.active == 1;
                if let Some(row) = list.input(&input) {
                    log::info!("chose row {row}: {}", rows[row].label);
                }
                if let Some(tile) = grid.input(&input) {
                    log::info!("chose tile {tile}: {}", tiles[tile].title);
                }
                field.input(&input);
                if button.input(&input) {
                    presses += 1;
                    log::info!("the button was pressed");
                }
            }
            dirty = true;
        }
        if !dirty {
            continue;
        }
        let label = if presses == 0 { "Press".to_string() } else { format!("Pressed {presses}") };
        let mode = app.mode();
        app.draw(window, |gl, size| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            // A few tiles get a picture, made here, so that both kinds are on show.
            for (n, tile) in tiles.iter_mut().enumerate().take(6) {
                if tile.picture.is_none() {
                    let (w, h) = (64u32, 96u32);
                    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                    for y in 0..h {
                        for x in 0..w {
                            rgba.extend_from_slice(&[(40 + n * 35) as u8, (x * 3) as u8, (255 - y * 2) as u8, 255]);
                        }
                    }
                    tile.picture = p.texture(gl, &rgba, w, h).ok();
                }
            }
            p.begin(gl, size, 1.0, theme::GROUND);
            let mut all = p.bounds().inset(theme::PAD);
            let mut top = all.take_top(Tabs::HEIGHT);
            let pressed = top.take_left(top.w - 620.0);
            tabs.draw(p, gl, pressed, &["A list", "A grid"]);
            let press = Rect { x: top.right() - Button::width(p, gl, &label), w: Button::width(p, gl, &label), ..top };
            button.draw(p, gl, press, &label);
            field.focused = true;
            field.draw(p, gl, Rect { w: press.x - top.x - 16.0, ..top });
            all.take_top(18.0);
            let foot = all.take_bottom(theme::FOOTER_EM * 1.6);
            let said = format!("{}   \u{2022}   {} x {}", if mode == Mode::Stereo { "two eyes" } else { "one picture" }, size.0, size.1);
            p.label(gl, &said, foot, theme::FOOTER_EM, theme::INK_HINT, 0.0);
            if tabs.active == 0 {
                list.draw(p, gl, all, &rows);
            } else {
                grid.draw(p, gl, all, &tiles);
            }
            p.end(gl);
        })?;
    }
}
