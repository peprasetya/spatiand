//! Pages' films and sounds, played here by Spatiand's own player (docs/spatimesh.md, "Media").
//!
//! SpatiMesh's script in every page (`spatimesh::media::PAGE_JS`) hands every media element to
//! this window: it opens what the element is to play with `spatiand_media`'s player (FFmpeg and
//! the platform's decoder; the sound to the room, at this window), tells the page what the
//! element would have told it (its length and size, ready, the time as it goes, seeks, the end),
//! and draws the picture where the element's box is -- under the page, which is drawn over it
//! with the box's key colour see-through, so whatever the page puts over the film stays over it.
//! What the player cannot open (a server that wants the page's cookies, say) is handed back to
//! the engine, which plays it as it always did.

use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use spatiand_app::glow::{self, HasContext};
use spatiand_app::ui::{theme, Painter, Rect};
use spatiand_app::Gl;
use spatiand_media::player::{Picture, Player};
use spatiand_media::screen::Screen;
use spatimesh::engine::TabId;
use spatimesh::media::KEY;

/// One element of a page's, by its tab, its frame's key and its number there.
type Key = (TabId, String, u64);

/// Where an element's box is, in the top frame's CSS pixels, and how the film fits in it.
#[derive(Debug, Clone, Default)]
struct Place {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fit: String,
    pos: (f32, f32, bool, bool),
    controls: bool,
    video: bool,
}

struct Item {
    url: String,
    player: Option<Player>,
    opening: Option<mpsc::Receiver<Result<Player, String>>>,
    playing: bool,
    muted: bool,
    volume: f32,
    rate: f32,
    looped: bool,
    video: bool,
    place: Option<Place>,
    told_meta: bool,
    shown: bool,
    seek: Option<(f64, Instant)>,
    last_tick: Instant,
    last_at: f64,
    moved_at: Instant,
    waiting: bool,
    ended: bool,
    picture: Option<Arc<Picture>>,
    screen: Option<Screen>,
    /// When the pointer was last over its box: its controls show for a while after.
    hovered: Option<Instant>,
}

impl Item {
    fn new(url: String) -> Item {
        let now = Instant::now();
        Item {
            url,
            player: None,
            opening: None,
            playing: false,
            muted: false,
            volume: 1.0,
            rate: 1.0,
            looped: false,
            video: true,
            place: None,
            told_meta: false,
            shown: false,
            seek: None,
            last_tick: now,
            last_at: 0.0,
            moved_at: now,
            waiting: false,
            ended: false,
            picture: None,
            screen: None,
            hovered: None,
        }
    }

    fn apply_volume(&self) {
        if let Some(player) = self.player.as_ref() {
            player.set_volume(if self.muted { 0.0 } else { self.volume });
        }
    }
}

/// The pages' media, and what is to be told to them.
#[derive(Default)]
pub struct Media {
    items: HashMap<Key, Item>,
    out: Vec<(TabId, String)>,
    keyed: Option<Keyed>,
    /// Tabs whose films have a new picture to be drawn.
    fresh: Vec<TabId>,
}

/// How long a film's controls stay after the pointer leaves, or after it starts.
const CONTROLS_LINGER: Duration = Duration::from_millis(2500);
/// How high a film's own controls are, in the page's CSS pixels.
const BAR: f32 = 44.0;

impl Media {
    /// Something the media script in a tab's page said.
    pub fn said(&mut self, tab: TabId, json: &str) {
        let Ok(m) = serde_json::from_str::<Value>(json) else { return };
        let frame = m["f"].as_str().unwrap_or("").to_string();
        let id = m["id"].as_u64().unwrap_or(0);
        let key = (tab, frame.clone(), id);
        match m["t"].as_str().unwrap_or("") {
            "open" => {
                let url = m["url"].as_str().unwrap_or("").to_string();
                let page = m["page"].as_str().unwrap_or("").to_string();
                let mut item = Item::new(url.clone());
                item.muted = m["muted"].as_bool().unwrap_or(false);
                item.volume = m["volume"].as_f64().unwrap_or(1.0) as f32;
                item.rate = m["rate"].as_f64().unwrap_or(1.0) as f32;
                item.looped = m["loop"].as_bool().unwrap_or(false);
                item.video = m["video"].as_bool().unwrap_or(true);
                let start = m["at"].as_f64().unwrap_or(0.0);
                match target(&url) {
                    Some(name) => item.opening = Some(open(name, page, start)),
                    None => {
                        log::info!("media: {url} is the engine's to play");
                        self.tell(&key, json!({ "t": "release" }));
                        return;
                    }
                }
                self.items.insert(key, item);
            }
            "close" => {
                self.items.remove(&key);
            }
            "play" => {
                if let Some(item) = self.items.get_mut(&key) {
                    item.playing = true;
                    item.ended = false;
                    item.last_tick = Instant::now();
                    item.moved_at = Instant::now();
                    item.hovered = Some(Instant::now());
                    if let Some(player) = item.player.as_mut() {
                        player.set_paused(false);
                    }
                }
            }
            "pause" => {
                if let Some(item) = self.items.get_mut(&key) {
                    item.playing = false;
                    item.waiting = false;
                    if let Some(player) = item.player.as_mut() {
                        player.set_paused(true);
                    }
                }
            }
            "seek" => {
                let at = m["at"].as_f64().unwrap_or(0.0);
                if let Some(item) = self.items.get_mut(&key) {
                    item.ended = false;
                    item.seek = Some((at, Instant::now()));
                    if let Some(player) = item.player.as_mut() {
                        player.seek(at);
                    }
                }
            }
            "volume" => {
                if let Some(item) = self.items.get_mut(&key) {
                    item.volume = m["volume"].as_f64().unwrap_or(1.0) as f32;
                    item.muted = m["muted"].as_bool().unwrap_or(false);
                    item.apply_volume();
                }
            }
            "rate" => {
                if let Some(item) = self.items.get_mut(&key) {
                    item.rate = m["rate"].as_f64().unwrap_or(1.0) as f32;
                    if let Some(player) = item.player.as_mut() {
                        if item.rate > 0.0 {
                            player.set_speed(item.rate);
                        }
                    }
                }
            }
            "loop" => {
                if let Some(item) = self.items.get_mut(&key) {
                    item.looped = m["loop"].as_bool().unwrap_or(false);
                }
            }
            "places" => {
                let list = m["places"].as_array().cloned().unwrap_or_default();
                for p in list {
                    let id = p["id"].as_u64().unwrap_or(0);
                    let Some(item) = self.items.get_mut(&(tab, frame.clone(), id)) else { continue };
                    item.place = (p["shown"].as_bool() == Some(true)).then(|| Place {
                        x: p["x"].as_f64().unwrap_or(0.0) as f32,
                        y: p["y"].as_f64().unwrap_or(0.0) as f32,
                        w: p["w"].as_f64().unwrap_or(0.0) as f32,
                        h: p["h"].as_f64().unwrap_or(0.0) as f32,
                        fit: p["fit"].as_str().unwrap_or("contain").to_string(),
                        pos: position(p["pos"].as_str().unwrap_or("50% 50%")),
                        controls: p["controls"].as_bool().unwrap_or(false),
                        video: p["video"].as_bool().unwrap_or(true),
                    });
                    self.fresh.push(tab);
                }
            }
            "bye" => {
                self.items.retain(|(t, f, _), _| !(*t == tab && *f == frame));
                self.fresh.push(tab);
            }
            "hello" if m["top"].as_bool() == Some(true) => {
                // A new document in the tab's top frame: everything of the one before is gone.
                self.items.retain(|(t, f, _), _| *t != tab || *f == frame);
                self.fresh.push(tab);
            }
            _ => {}
        }
    }

    /// A tab closed, crashed or went to another engine: its media stops.
    pub fn forget(&mut self, tab: TabId) {
        self.items.retain(|(t, _, _), _| *t != tab);
    }

    fn tell(&mut self, key: &Key, mut m: Value) {
        m["f"] = json!(key.1);
        m["id"] = json!(key.2);
        self.out.push((key.0, m.to_string()));
    }

    /// What is to be told to the pages, as code for each tab's top frame.
    pub fn take_out(&mut self) -> Vec<(TabId, String)> {
        std::mem::take(&mut self.out)
            .into_iter()
            .map(|(tab, json)| {
                let quoted = serde_json::to_string(&json).unwrap_or_default();
                (tab, format!("window.__spatimeshMediaIn&&window.__spatimeshMediaIn({quoted})"))
            })
            .collect()
    }

    /// Tabs with a new picture since last asked.
    pub fn take_fresh(&mut self) -> Vec<TabId> {
        let mut fresh = std::mem::take(&mut self.fresh);
        fresh.sort_unstable();
        fresh.dedup();
        fresh
    }

    /// Whether a tab has something of its own playing (its title says so: `browser::draw`).
    pub fn playing_in(&self, tab: TabId) -> bool {
        self.items.iter().any(|((t, _, _), i)| *t == tab && i.playing && !(i.muted || i.volume == 0.0))
    }

    /// Whether anything is playing: the window then goes round at the films' pace.
    pub fn busy(&self) -> bool {
        self.items.values().any(|i| i.playing || i.opening.is_some() || i.seek.is_some())
    }

    /// Once a turn: players opened, their pictures and times, and what the pages are told.
    pub fn tick(&mut self) {
        let keys: Vec<Key> = self.items.keys().cloned().collect();
        for key in keys {
            let mut said: Vec<Value> = Vec::new();
            let mut gone = false;
            let mut fresh = false;
            {
                let Some(item) = self.items.get_mut(&key) else { continue };
                if let Some(rx) = item.opening.as_ref() {
                    match rx.try_recv() {
                        Ok(Ok(mut player)) => {
                            item.opening = None;
                            if item.rate > 0.0 && item.rate != 1.0 {
                                player.set_speed(item.rate);
                            }
                            if let Some((at, _)) = item.seek {
                                player.seek(at);
                            }
                            player.set_paused(!item.playing);
                            item.player = Some(player);
                            item.apply_volume();
                        }
                        Ok(Err(why)) => {
                            log::info!("media: {} could not be opened here ({why}); the engine plays it", item.url);
                            said.push(json!({ "t": "release" }));
                            gone = true;
                        }
                        Err(mpsc::TryRecvError::Empty) => {}
                        Err(mpsc::TryRecvError::Disconnected) => gone = true,
                    }
                }
                if let Some(player) = item.player.as_mut() {
                    let info = player.info().clone();
                    if !item.told_meta {
                        item.told_meta = true;
                        let duration = if info.duration > 0.0 { json!(info.duration) } else { json!(-1.0) };
                        said.push(json!({ "t": "meta", "duration": duration, "width": info.width, "height": info.height }));
                        said.push(json!({ "t": "state", "ready": true, "at": player.position(), "buffered": buffered(&item.url, info.duration, player.position()) }));
                    }
                    let (picture, new) = player.picture();
                    if new {
                        if let Some(picture) = picture {
                            item.picture = Some(picture);
                            fresh = true;
                            if !item.shown {
                                item.shown = true;
                                said.push(json!({ "t": "state", "shown": true }));
                            }
                        }
                    }
                    let at = player.position();
                    if let Some((_, since)) = item.seek {
                        let settled = (item.video && new) || (!item.video && since.elapsed() > Duration::from_millis(150)) || since.elapsed() > Duration::from_secs(4);
                        if settled {
                            item.seek = None;
                            item.last_at = at;
                            item.moved_at = Instant::now();
                            said.push(json!({ "t": "state", "seeked": true, "at": at, "buffered": buffered(&item.url, info.duration, at) }));
                        }
                    }
                    if item.playing && item.seek.is_none() {
                        if (at - item.last_at).abs() > 1e-3 {
                            item.last_at = at;
                            item.moved_at = Instant::now();
                            if item.waiting {
                                item.waiting = false;
                                said.push(json!({ "t": "state", "waiting": false, "at": at }));
                            }
                        } else if !item.waiting && item.moved_at.elapsed() > Duration::from_millis(700) && !player.ended() {
                            item.waiting = true;
                            said.push(json!({ "t": "state", "waiting": true, "at": at }));
                        }
                        if player.ended() {
                            if item.looped {
                                player.seek(0.0);
                                item.last_at = 0.0;
                                said.push(json!({ "t": "state", "tick": true, "at": 0.0 }));
                            } else {
                                item.playing = false;
                                item.ended = true;
                                player.set_paused(true);
                                said.push(json!({ "t": "state", "ended": true, "at": info.duration.max(at) }));
                            }
                        } else if item.last_tick.elapsed() >= Duration::from_millis(250) {
                            item.last_tick = Instant::now();
                            said.push(json!({ "t": "state", "tick": true, "at": at, "buffered": buffered(&item.url, info.duration, at) }));
                        }
                    }
                }
            }
            if fresh {
                self.fresh.push(key.0);
            }
            for m in said {
                self.tell(&key, m);
            }
            if gone {
                self.items.remove(&key);
            }
        }
    }

    /// Whether a tab has a film on show to draw under its page.
    pub fn films_in(&self, tab: TabId) -> bool {
        self.items.iter().any(|((t, _, _), i)| *t == tab && i.place.as_ref().is_some_and(|p| p.video || p.controls))
    }

    /// Draw a tab's films where their boxes are, then the page over them with its key colour
    /// see-through, then the films' own controls. `page` is the page's place in the window,
    /// `shown` the part of it the picture covers (`uv` of `texture`), `zoom` the page's CSS
    /// pixels to the window's.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, gl: &Gl, p: &mut Painter, tab: TabId, page: Rect, shown: Rect, uv: [f32; 4], texture: glow::Texture, zoom: f32) {
        let (view, origin, scale) = p.view();
        let to_px = |r: Rect| -> [f32; 4] { [origin.0 + r.x * scale.0, origin.1 + r.y * scale.1, r.w * scale.0, r.h * scale.1] };
        let clip = to_px(shown);
        let mut holes: Vec<[f32; 4]> = Vec::new();
        let mut bars: Vec<(Key, Rect)> = Vec::new();
        let g = gl.api();
        unsafe {
            g.enable(glow::SCISSOR_TEST);
            g.scissor(clip[0] as i32, clip[1] as i32, clip[2].ceil() as i32, clip[3].ceil() as i32);
        }
        let keys: Vec<Key> = self.items.keys().filter(|k| k.0 == tab).cloned().collect();
        for key in keys {
            let Some(item) = self.items.get_mut(&key) else { continue };
            let Some(place) = item.place.clone() else { continue };
            let boxed = Rect::new(page.x + place.x * zoom, page.y + place.y * zoom, place.w * zoom, place.h * zoom);
            if place.controls {
                bars.push((key.clone(), boxed));
            }
            if !place.video {
                continue;
            }
            let px = to_px(boxed);
            holes.push(px);
            unsafe {
                // What the box shows where the film does not reach: black, as a player's.
                g.enable(glow::SCISSOR_TEST);
                let (x0, y0) = (px[0].max(clip[0]), px[1].max(clip[1]));
                let (x1, y1) = ((px[0] + px[2]).min(clip[0] + clip[2]), (px[1] + px[3]).min(clip[1] + clip[3]));
                if x1 > x0 && y1 > y0 {
                    g.scissor(x0 as i32, y0 as i32, (x1 - x0).ceil() as i32, (y1 - y0).ceil() as i32);
                    g.clear_color(0.0, 0.0, 0.0, 1.0);
                    g.clear(glow::COLOR_BUFFER_BIT);
                }
                g.scissor(clip[0] as i32, clip[1] as i32, clip[2].ceil() as i32, clip[3].ceil() as i32);
            }
            let Some(picture) = item.picture.clone() else { continue };
            if item.screen.is_none() {
                match Screen::new(gl) {
                    Ok(screen) => item.screen = Some(screen),
                    Err(e) => {
                        log::warn!("media: {e}");
                        continue;
                    }
                }
            }
            let screen = item.screen.as_mut().unwrap();
            screen.load(gl, &picture);
            let natural = item.player.as_ref().map(|pl| (pl.info().width as f32, pl.info().height as f32)).filter(|(w, h)| *w > 0.0 && *h > 0.0).unwrap_or((picture.width as f32, picture.height as f32));
            let (rect, film_uv) = fit(&place, natural, zoom);
            let at = Rect::new(page.x + rect.x, page.y + rect.y, rect.w, rect.h);
            screen.draw(gl, (view.0 as u32, view.1 as u32), to_px(at), film_uv);
        }
        // The page over them, its key see-through inside the films' boxes.
        if self.keyed.is_none() {
            match Keyed::new(gl) {
                Ok(k) => self.keyed = Some(k),
                Err(e) => log::warn!("media: {e}"),
            }
        }
        if let Some(keyed) = self.keyed.as_ref() {
            keyed.draw(gl, (view.0, view.1), clip, uv, texture, &holes);
        }
        unsafe { g.disable(glow::SCISSOR_TEST) };
        p.rebind(gl);
        for (key, boxed) in bars {
            self.draw_controls(p, gl, &key, boxed, page, zoom);
        }
    }

    /// A film's own controls, for an element that asked for the browser's: SpatiMesh's.
    fn draw_controls(&mut self, p: &mut Painter, gl: &Gl, key: &Key, boxed: Rect, page: Rect, zoom: f32) {
        let Some(item) = self.items.get(key) else { return };
        let lingering = item.hovered.is_some_and(|t| t.elapsed() < CONTROLS_LINGER);
        if item.playing && !lingering && item.place.as_ref().is_some_and(|pl| pl.video) {
            return;
        }
        let bar = bar_of(boxed, zoom);
        if bar.bottom() < page.y || bar.y > page.bottom() {
            return;
        }
        p.fill(gl, bar, 0.0, [0.0, 0.0, 0.0, 0.62]);
        let h = bar.h;
        // Play or pause.
        let button = Rect::new(bar.x + h * 0.15, bar.y + h * 0.15, h * 0.7, h * 0.7);
        let ink = theme::INK;
        if item.playing {
            let w = button.w * 0.22;
            p.fill(gl, Rect::new(button.x + button.w * 0.22, button.y + button.h * 0.18, w, button.h * 0.64), 1.0, ink);
            p.fill(gl, Rect::new(button.x + button.w * 0.56, button.y + button.h * 0.18, w, button.h * 0.64), 1.0, ink);
        } else {
            // A triangle, drawn as narrowing bars.
            let steps = 8;
            for i in 0..steps {
                let t = i as f32 / steps as f32;
                let x = button.x + button.w * (0.28 + t * 0.5);
                let half = button.h * 0.32 * (1.0 - t);
                p.fill(gl, Rect::new(x, button.y + button.h * 0.5 - half, button.w * 0.5 / steps as f32 + 0.5, half * 2.0), 0.0, ink);
            }
        }
        // The time, and how far along.
        let (at, duration) = item.player.as_ref().map_or((0.0, 0.0), |pl| (pl.position(), pl.info().duration));
        let label = if duration > 0.0 { format!("{} / {}", clock(at), clock(duration)) } else { clock(at) };
        let text = Rect::new(button.right() + h * 0.2, bar.y, h * 3.6, h);
        p.label(gl, &label, text, (h * 0.36).max(10.0), ink, 0.0);
        let track = Rect::new(text.right() + h * 0.2, bar.y + h * 0.45, (bar.right() - text.right() - h * 1.6).max(10.0), h * 0.1);
        p.fill(gl, track, track.h * 0.5, [1.0, 1.0, 1.0, 0.3]);
        if duration > 0.0 {
            let done = Rect { w: track.w * (at / duration).clamp(0.0, 1.0) as f32, ..track };
            p.fill(gl, done, track.h * 0.5, theme::MARK);
        }
        // Sound on or off.
        let speaker = Rect::new(bar.right() - h * 0.95, bar.y + h * 0.3, h * 0.4, h * 0.4);
        p.fill(gl, Rect::new(speaker.x, speaker.y + speaker.h * 0.3, speaker.w * 0.35, speaker.h * 0.4), 0.0, ink);
        p.fill(gl, Rect::new(speaker.x + speaker.w * 0.3, speaker.y, speaker.w * 0.3, speaker.h), 0.0, ink);
        if item.muted || item.volume == 0.0 {
            p.fill(gl, Rect::new(speaker.x + speaker.w * 0.75, speaker.y + speaker.h * 0.45, speaker.w * 0.5, speaker.h * 0.1), 0.0, [1.0, 0.4, 0.4, 1.0]);
        } else {
            p.fill(gl, Rect::new(speaker.x + speaker.w * 0.8, speaker.y + speaker.h * 0.15, speaker.w * 0.12, speaker.h * 0.7), 0.0, ink);
        }
    }

    /// The pointer over a tab's page, at CSS pixels `(x, y)` of its top frame. Whether a film's
    /// own controls took it (then the page is not told).
    pub fn pointer(&mut self, tab: TabId, x: f32, y: f32, pressed: bool) -> bool {
        let mut took = None;
        for (key, item) in self.items.iter_mut().filter(|(k, _)| k.0 == tab) {
            let Some(place) = item.place.as_ref() else { continue };
            let inside = x >= place.x && x <= place.x + place.w && y >= place.y && y <= place.y + place.h;
            if inside {
                item.hovered = Some(Instant::now());
            }
            if !place.controls || !inside {
                continue;
            }
            let bar_top = place.y + place.h - BAR.min(place.h);
            if y < bar_top {
                if pressed && place.video {
                    took = Some((key.clone(), Action::Toggle));
                }
                continue;
            }
            if !pressed {
                return true;
            }
            let h = BAR.min(place.h);
            let action = if x < place.x + h {
                Action::Toggle
            } else if x > place.x + place.w - h * 1.1 {
                Action::Mute
            } else {
                let start = place.x + h * 0.85 + h * 3.8;
                let end = place.x + place.w - h * 1.4;
                Action::Seek(((x - start) / (end - start).max(1.0)).clamp(0.0, 1.0))
            };
            took = Some((key.clone(), action));
            break;
        }
        let Some((key, action)) = took else { return false };
        let Some(item) = self.items.get(&key) else { return false };
        // Done as the page would have done it, through the element, so its events and its own
        // idea of itself are as they would be.
        let code = match action {
            Action::Toggle if item.playing => "pause",
            Action::Toggle => "play",
            Action::Mute => "mute",
            Action::Seek(_) => "seekTo",
        };
        let at = match action {
            Action::Seek(f) => item.player.as_ref().map_or(0.0, |p| p.info().duration * f as f64),
            _ => 0.0,
        };
        self.tell(&key, json!({ "t": "control", "do": code, "at": at }));
        true
    }
}

enum Action {
    Toggle,
    Mute,
    Seek(f32),
}

fn bar_of(boxed: Rect, zoom: f32) -> Rect {
    let h = (BAR * zoom).min(boxed.h);
    Rect::new(boxed.x, boxed.bottom() - h, boxed.w, h)
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// What FFmpeg is to open for an element's address, or none for what only the engine can play:
/// an object URL (a Blob's, or a MediaSource of the engine's own).
fn target(url: &str) -> Option<String> {
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("data:") {
        return Some(url.to_string());
    }
    if url.starts_with("file:") {
        return url::Url::parse(url).ok()?.to_file_path().ok().map(|p| p.to_string_lossy().into_owned());
    }
    None
}

/// Open on a thread of its own: a network's file may take seconds to answer. Paused at once,
/// so nothing is heard until the page says play.
fn open(name: String, page: String, start: f64) -> mpsc::Receiver<Result<Player, String>> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new().name("media-open".into()).spawn(move || {
        let mut options = vec![("user_agent".to_string(), USER_AGENT.to_string())];
        if page.starts_with("http") {
            options.push(("referer".to_string(), page.clone()));
        }
        let result = Player::open_with(&name, &options).map(|mut player| {
            player.set_paused(true);
            if start > 0.0 {
                player.seek(start);
            }
            player
        });
        let _ = tx.send(result);
    });
    rx
}

/// What the player says it is to a server: a browser, as the page's own requests do.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

/// What of a film is in hand: all of a file here; from a network, what the reader keeps ahead.
fn buffered(url: &str, duration: f64, at: f64) -> Value {
    if duration <= 0.0 {
        return json!([]);
    }
    let end = if url.starts_with("http") { (at + 4.0).min(duration) } else { duration };
    json!([[0.0, end]])
}

/// `object-position` as computed: two lengths, each a percentage (`true`) or pixels.
fn position(text: &str) -> (f32, f32, bool, bool) {
    let mut parts = text.split_whitespace().map(|p| {
        if let Some(n) = p.strip_suffix('%') {
            (n.parse::<f32>().unwrap_or(50.0) / 100.0, true)
        } else {
            (p.trim_end_matches("px").parse::<f32>().unwrap_or(0.0), false)
        }
    });
    let (x, xp) = parts.next().unwrap_or((0.5, true));
    let (y, yp) = parts.next().unwrap_or((0.5, true));
    (x, y, xp, yp)
}

/// Where a film of `natural` size goes in its box, by `object-fit` and `object-position`: its
/// place relative to the page (window pixels), and the part of the picture shown.
fn fit(place: &Place, natural: (f32, f32), zoom: f32) -> (Rect, [f32; 4]) {
    let boxed = Rect::new(place.x * zoom, place.y * zoom, place.w * zoom, place.h * zoom);
    let (nw, nh) = (natural.0.max(1.0), natural.1.max(1.0));
    let contain = (boxed.w / nw).min(boxed.h / nh);
    let cover = (boxed.w / nw).max(boxed.h / nh);
    let (w, h) = match place.fit.as_str() {
        "fill" => (boxed.w, boxed.h),
        "cover" => (nw * cover, nh * cover),
        "none" => (nw * zoom, nh * zoom),
        "scale-down" => {
            let s = contain.min(zoom);
            (nw * s, nh * s)
        }
        _ => (nw * contain, nh * contain),
    };
    let (px, py, xp, yp) = place.pos;
    let x = boxed.x + if xp { (boxed.w - w) * px } else { px * zoom };
    let y = boxed.y + if yp { (boxed.h - h) * py } else { py * zoom };
    // What falls outside the box is cut off, and the picture with it.
    let (cx0, cy0) = (x.max(boxed.x), y.max(boxed.y));
    let (cx1, cy1) = ((x + w).min(boxed.right()), (y + h).min(boxed.bottom()));
    if cx1 <= cx0 || cy1 <= cy0 {
        return (Rect::new(0.0, 0.0, 0.0, 0.0), [0.0, 0.0, 1.0, 1.0]);
    }
    let uv = [(cx0 - x) / w, (cy0 - y) / h, (cx1 - x) / w, (cy1 - y) / h];
    (Rect::new(cx0, cy0, cx1 - cx0, cy1 - cy0), uv)
}

/// The page drawn over its films: inside a film's box, where the page shows the key colour the
/// film is seen, and where the page draws over the key with something half see-through the
/// film is seen through it by as much (worked out from how much of the key is left in the
/// pixel, which is exact for anything not itself green).
struct Keyed {
    program: glow::Program,
    vao: glow::VertexArray,
    _corners: glow::Buffer,
    at: [Option<glow::UniformLocation>; 6],
}

const KEYED_VERTEX: &str = r#"#version 300 es
precision highp float;
layout(location = 0) in vec2 a_corner;
uniform vec4 u_rect;
uniform vec2 u_view;
uniform vec4 u_uv;
out vec2 v_uv;
out vec2 v_px;
void main() {
    vec2 pixel = u_rect.xy + a_corner * u_rect.zw;
    v_px = pixel;
    v_uv = mix(u_uv.xy, u_uv.zw, a_corner);
    gl_Position = vec4(pixel / u_view * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const KEYED_FRAGMENT: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv;
in vec2 v_px;
uniform sampler2D u_page;
uniform vec4 u_holes[8];
uniform int u_count;
uniform vec3 u_key;
out vec4 o_color;
void main() {
    vec4 page = texture(u_page, v_uv);
    bool hole = false;
    for (int i = 0; i < 8; i++) {
        if (i >= u_count) break;
        vec4 h = u_holes[i];
        if (v_px.x >= h.x && v_px.y >= h.y && v_px.x < h.x + h.z && v_px.y < h.y + h.w) hole = true;
    }
    if (!hole) {
        o_color = vec4(page.rgb, 1.0);
        return;
    }
    float left = clamp((page.g - max(page.r, page.b)) / (u_key.g - max(u_key.r, u_key.b)), 0.0, 1.0);
    // Within a step of the key itself: the key, and nothing of the page.
    if (left > 0.985) left = 1.0;
    if (left < 0.015) left = 0.0;
    vec3 over = max(page.rgb - left * u_key, vec3(0.0));
    o_color = vec4(over, 1.0 - left);
}
"#;

impl Keyed {
    fn new(gl: &Gl) -> Result<Keyed, String> {
        let g = gl.api();
        unsafe {
            let program = g.create_program()?;
            for (kind, source) in [(glow::VERTEX_SHADER, KEYED_VERTEX), (glow::FRAGMENT_SHADER, KEYED_FRAGMENT)] {
                let shader = g.create_shader(kind)?;
                g.shader_source(shader, source);
                g.compile_shader(shader);
                if !g.get_shader_compile_status(shader) {
                    return Err(format!("the page's key would not compile: {}", g.get_shader_info_log(shader)));
                }
                g.attach_shader(program, shader);
                g.delete_shader(shader);
            }
            g.link_program(program);
            if !g.get_program_link_status(program) {
                return Err(format!("the page's key would not link: {}", g.get_program_info_log(program)));
            }
            let vao = g.create_vertex_array()?;
            let corners = g.create_buffer()?;
            g.bind_vertex_array(Some(vao));
            g.bind_buffer(glow::ARRAY_BUFFER, Some(corners));
            let unit: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
            g.buffer_data_u8_slice(glow::ARRAY_BUFFER, std::slice::from_raw_parts(unit.as_ptr() as *const u8, 32), glow::STATIC_DRAW);
            g.enable_vertex_attrib_array(0);
            g.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);
            g.bind_vertex_array(None);
            let names = ["u_rect", "u_view", "u_uv", "u_holes", "u_count", "u_key"];
            Ok(Keyed { program, vao, _corners: corners, at: names.map(|n| g.get_uniform_location(program, n)) })
        }
    }

    fn draw(&self, gl: &Gl, view: (f32, f32), rect: [f32; 4], uv: [f32; 4], texture: glow::Texture, holes: &[[f32; 4]]) {
        let g = gl.api();
        let mut flat = [0f32; 32];
        for (i, h) in holes.iter().take(8).enumerate() {
            flat[i * 4..i * 4 + 4].copy_from_slice(h);
        }
        unsafe {
            g.use_program(Some(self.program));
            g.bind_vertex_array(Some(self.vao));
            g.enable(glow::BLEND);
            g.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            g.active_texture(glow::TEXTURE0);
            g.bind_texture(glow::TEXTURE_2D, Some(texture));
            g.uniform_4_f32(self.at[0].as_ref(), rect[0], rect[1], rect[2], rect[3]);
            g.uniform_2_f32(self.at[1].as_ref(), view.0, view.1);
            g.uniform_4_f32(self.at[2].as_ref(), uv[0], uv[1], uv[2], uv[3]);
            g.uniform_4_f32_slice(self.at[3].as_ref(), &flat);
            g.uniform_1_i32(self.at[4].as_ref(), holes.len().min(8) as i32);
            g.uniform_3_f32(self.at[5].as_ref(), KEY[0] as f32 / 255.0, KEY[1] as f32 / 255.0, KEY[2] as f32 / 255.0);
            g.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            g.bind_vertex_array(None);
        }
    }
}
