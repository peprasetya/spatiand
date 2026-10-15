//! SpatiReel: Spatiand's video player.
//!
//! The Vibrefy Rust port, moved into Spatiand (docs/spatiand-apps.md): what it knew about
//! films -- their layouts, their names, their sound -- is `spatiand-media`, and what it drew
//! with is replaced by the room's own widgets. Vibrefy is one kind of server among four.
//!
//! Three screens and a film. **Home** has what is part watched, from every source at once, and
//! the sources themselves: this machine, each server signed in to, whatever DLNA server is on
//! the network. **A source's folders** are browsed as tiles. **Adding a server** asks what kind
//! and how to sign in. And a film being watched is `play`.
//!
//! ```text
//! spatireel                 home
//! spatireel <file>          that film, at once
//! spatireel <folder>        this machine's films, at that folder
//! ```

mod library;
mod play;
mod screens;
mod source;
mod store;
mod work;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_app::ui::{look, theme, Input, Painter};
use spatiand_app::{App, Event, WindowId};

use play::{Outcome, Playing};
use screens::{Act, Add, Browse, Home, Screen};
use source::local::Local;
use source::{dlna, jellyfin, vibrefy, Item, Source, Watching};
use store::Store;
use work::{Done, Work};

pub const WINDOW: (u32, u32) = (1440, 900);

/// A server signed in to, as it is kept.
#[derive(Debug, Clone, PartialEq)]
pub enum Account {
    Jellyfin(jellyfin::Account),
    Vibrefy(vibrefy::Account),
}

/// What a source is, for how it is drawn and whether it can be forgotten.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Local,
    Server(Account),
    Dlna,
}

pub struct Entry {
    pub source: Arc<dyn Source>,
    pub kind: Kind,
}

/// The film being watched, and who is to be told how far it has got.
struct Watched {
    source: Arc<dyn Source>,
    item: Item,
    told: Instant,
    /// Whether it was paused when last told, so that pausing is told at once.
    paused: bool,
}

struct Reel {
    app: App,
    window: WindowId,
    painter: Option<Painter>,
    work: Work,
    store: Arc<Mutex<Store>>,
    sources: Vec<Entry>,
    /// The DLNA servers that last answered.
    found: Vec<dlna::Found>,
    screen: Screen,
    playing: Option<(Playing, Watched)>,
    /// The next number for an asking whose answer may come too late to be wanted.
    ticket: u64,
    looked_for_servers: Option<Instant>,
    ready: bool,
    dirty: bool,
}

impl Reel {
    fn ticket(&mut self) -> u64 {
        self.ticket += 1;
        self.ticket
    }

    /// The sources, from what is kept: this machine first, then each server, then whatever
    /// is on the network.
    fn gather(&mut self) {
        let local = self.sources.iter().position(|e| e.kind == Kind::Local).map(|i| self.sources.swap_remove(i));
        self.sources.clear();
        self.sources.extend(local);
        let store = self.store.lock().unwrap().clone();
        for account in store.jellyfin {
            self.sources.push(Entry { source: Arc::new(jellyfin::Server::new(account.clone())), kind: Kind::Server(Account::Jellyfin(account)) });
        }
        for account in store.vibrefy {
            self.sources.push(Entry { source: Arc::new(vibrefy::Server::new(account.clone())), kind: Kind::Server(Account::Vibrefy(account)) });
        }
        for server in self.found.clone() {
            self.sources.push(Entry { source: Arc::new(dlna::Server::new(server)), kind: Kind::Dlna });
        }
    }

    fn go_home(&mut self) {
        let ticket = self.ticket();
        let home = Home::new(&self.sources, ticket);
        // What is part watched, asked of every source at once; each answers when it can.
        for (index, entry) in self.sources.iter().enumerate() {
            let source = entry.source.clone();
            self.work.call(move || Some(Done::Unfinished { ticket, source: index, items: source.unfinished() }));
        }
        // Who is on the network changes slowly, and asking takes two seconds of listening.
        let stale = match self.looked_for_servers {
            Some(at) => at.elapsed() > Duration::from_secs(120),
            None => true,
        };
        if stale && std::env::var("SPATIREEL_DLNA").as_deref() != Ok("off") {
            self.looked_for_servers = Some(Instant::now());
            self.work.slow(|| Some(Done::Found(dlna::discover(Duration::from_millis(2200)))));
        }
        self.screen = Screen::Home(home);
        self.app.set_title(self.window, "SpatiReel");
        self.dirty = true;
    }

    fn list(&mut self, browse: &mut Browse) {
        browse.ticket = self.ticket();
        browse.loading();
        let (source, folder, ticket) = (self.sources[browse.source].source.clone(), browse.folder().cloned(), browse.ticket);
        self.work.call(move || Some(Done::Listed { ticket, result: source.list(folder.as_ref()) }));
    }

    fn browse(&mut self, source: usize, folder: Option<Item>) {
        let Some(entry) = self.sources.get(source) else { return };
        let mut browse = Browse::new(source, entry.source.label(), entry.kind == Kind::Local);
        if let Some(folder) = folder {
            browse.enter(folder, 0);
        }
        self.list(&mut browse);
        self.screen = Screen::Browse(browse);
        self.dirty = true;
    }

    fn play(&mut self, source: Arc<dyn Source>, item: Item) -> Result<(), String> {
        let play = source.play(&item)?;
        let playing = Playing::open(self.app.gl()?, &play.url, &play.options, &item.name, &item.title, item.layout, item.resume)?;
        self.app.set_title(self.window, &item.title);
        let (told, started) = (source.clone(), item.clone());
        self.work.report(move || told.watching(&started, Watching::Started));
        self.playing = Some((playing, Watched { source, item, told: Instant::now(), paused: false }));
        self.dirty = true;
        Ok(())
    }

    fn stop(&mut self, finished: bool) {
        let Some((mut playing, watched)) = self.playing.take() else { return };
        let (seconds, duration) = (playing.player().position(), playing.player().info().duration);
        let what = if finished { Watching::Finished } else { Watching::Stopped { seconds, duration } };
        // Told before anything is listed again, so that the list has it: both go through
        // threads, and the one that lists is asked to wait for this.
        let (source, item) = (watched.source, watched.item);
        self.work.call(move || {
            source.watching(&item, what);
            None
        });
        playing.close(&mut self.app, self.window);
        self.app.resize(self.window, WINDOW);
        self.app.set_title(self.window, "SpatiReel");
        // What was just left is now part watched: a list drawn before is stale.
        match std::mem::replace(&mut self.screen, Screen::Empty) {
            Screen::Browse(mut browse) => {
                browse.keep_cursor();
                self.list(&mut browse);
                self.screen = Screen::Browse(browse);
            }
            _ => self.go_home(),
        }
        self.dirty = true;
    }

    /// Tell the film's source how far it has got, every ten seconds and whenever it is paused.
    fn keep_place(&mut self) {
        let Some((playing, watched)) = self.playing.as_mut() else { return };
        let paused = playing.player().paused();
        if watched.told.elapsed() < Duration::from_secs(10) && paused == watched.paused {
            return;
        }
        watched.told = Instant::now();
        watched.paused = paused;
        let (source, item, seconds) = (watched.source.clone(), watched.item.clone(), playing.player().position());
        self.work.report(move || source.watching(&item, Watching::At { seconds, paused }));
    }

    fn act(&mut self, act: Act) {
        match act {
            Act::Nothing => {}
            Act::Home => self.go_home(),
            Act::Open { source, folder } => self.browse(source, folder),
            Act::List => {
                if let Screen::Browse(mut browse) = std::mem::replace(&mut self.screen, Screen::Empty) {
                    self.list(&mut browse);
                    self.screen = Screen::Browse(browse);
                }
            }
            Act::Play { source, item } => {
                let Some(entry) = self.sources.get(source) else { return };
                if let Err(e) = self.play(entry.source.clone(), item.clone()) {
                    log::warn!("{}: {e}", item.title);
                    self.screen.trouble(format!("{} would not play: {e}", item.title));
                }
            }
            Act::Add => self.screen = Screen::Add(Add::new()),
            Act::Forget(source) => {
                if let Some(Entry { kind: Kind::Server(account), .. }) = self.sources.get(source) {
                    {
                        let mut store = self.store.lock().unwrap();
                        match account {
                            Account::Jellyfin(account) => store.jellyfin.retain(|kept| kept != account),
                            Account::Vibrefy(account) => store.vibrefy.retain(|kept| kept != account),
                        }
                        store.save();
                    }
                    self.gather();
                    self.go_home();
                }
            }
            Act::SignIn { flavour, address, user, password } => {
                let device = self.store.lock().unwrap().device.clone();
                self.work.call(move || Some(Done::SignedIn(jellyfin::sign_in(flavour, &address, &user, &password, &device).map(Account::Jellyfin))));
            }
            Act::Pair { address, cancelled } => {
                // One client for the whole of it: the code is bound to the session that
                // asked, so asking, waiting and reading what was approved are one job.
                self.work.slow(move || {
                    use vibrefy_api::pairing::PairingState;
                    let client = vibrefy::Server::client(&address);
                    let offer = match client.request_pairing_code() {
                        Ok(offer) => offer,
                        Err(e) => return Some(Done::SignedIn(Err(e.to_string()))),
                    };
                    screens::show_code(&offer.code);
                    let started = Instant::now();
                    while started.elapsed() < Duration::from_secs(offer.seconds_left.max(30)) {
                        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                            return None;
                        }
                        std::thread::sleep(Duration::from_millis(1500));
                        match client.poll_pairing(&offer.code) {
                            Ok(PairingState::Approved { account }) => {
                                // The session's number changes on approval: read it afresh.
                                let _ = client.browse("");
                                return Some(Done::SignedIn(Ok(Account::Vibrefy(vibrefy::Server::account_of(&client, account)))));
                            }
                            Ok(PairingState::Waiting { .. }) => {}
                            Ok(PairingState::Expired) => break,
                            Ok(PairingState::Refused { reason }) => return Some(Done::SignedIn(Err(reason))),
                            Err(e) => return Some(Done::SignedIn(Err(e.to_string()))),
                        }
                    }
                    Some(Done::SignedIn(Err("the code ran out before anybody approved it".into())))
                });
            }
        }
        self.dirty = true;
    }

    /// What the threads have finished since last looked.
    fn answers(&mut self) {
        while let Ok(done) = self.work.done.try_recv() {
            self.dirty = true;
            match done {
                Done::Listed { ticket, result } => {
                    if let Screen::Browse(browse) = &mut self.screen {
                        if browse.ticket == ticket {
                            browse.listed(result);
                        }
                    }
                }
                Done::Pictured { ticket, index, still } => self.screen.pictured(ticket, index, still),
                Done::Unfinished { ticket, source, items } => {
                    if let Screen::Home(home) = &mut self.screen {
                        if home.ticket == ticket {
                            let label = self.sources.get(source).map(|e| e.source.label()).unwrap_or_default();
                            home.unfinished(source, &label, items);
                        }
                    }
                }
                Done::Found(found) => {
                    if found != self.found {
                        log::info!("on the network: {}", found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "));
                        self.found = found;
                        // Only where the sources are on show: elsewhere a source's place in
                        // the list is what a screen knows it by, and must not move under it.
                        if matches!(self.screen, Screen::Home(_)) && self.playing.is_none() {
                            self.gather();
                            self.go_home();
                        }
                    }
                }
                Done::SignedIn(result) => match result {
                    Ok(account) => {
                        {
                            let mut store = self.store.lock().unwrap();
                            match &account {
                                Account::Jellyfin(account) => {
                                    store.jellyfin.retain(|kept| !(kept.address == account.address && kept.user_id == account.user_id && kept.flavour == account.flavour));
                                    store.jellyfin.push(account.clone());
                                }
                                Account::Vibrefy(account) => {
                                    store.vibrefy.retain(|kept| kept.address != account.address);
                                    store.vibrefy.push(account.clone());
                                }
                            }
                            store.save();
                        }
                        self.gather();
                        // Straight into what was just added, which is why it was added.
                        match self.sources.iter().position(|e| e.kind == Kind::Server(account.clone())) {
                            Some(at) => self.browse(at, None),
                            None => self.go_home(),
                        }
                    }
                    Err(e) => {
                        if let Screen::Add(add) = &mut self.screen {
                            add.failed(e);
                        }
                    }
                },
            }
        }
    }

    fn handle(&mut self, event: Event) -> bool {
        match &event {
            Event::CloseRequested { window } if *window == self.window => return false,
            // The film's own surface was closed: the film is over.
            Event::CloseRequested { .. } => self.stop(false),
            Event::Ready { window, .. } if *window == self.window => self.ready = true,
            Event::Mode(mode) => log::info!("the wearer now sees {mode:?}"),
            Event::Hands => {
                if let Some((playing, _)) = self.playing.as_mut() {
                    playing.hands();
                }
                return true;
            }
            Event::LayerRefused { window, .. } | Event::LayerLost { window, .. } => {
                if let Some((playing, _)) = self.playing.as_mut().filter(|(p, _)| p.sky() == Some(*window)) {
                    playing.sky_was_refused();
                }
            }
            _ => {}
        }
        self.dirty = true;
        // Where the pointer is in the film's own surface is not where it is in the controls;
        // but a key is a key whichever of the two the room has given the keyboard to, and
        // the film's surface, being the newer, is usually the one.
        if event.window().is_some_and(|window| window != self.window) && !matches!(event, Event::Key { .. }) {
            return true;
        }
        let Some(input) = Input::of(&event, 1.0) else { return true };
        match self.playing.as_mut() {
            Some((playing, _)) => {
                if playing.input(&input) == Outcome::Close {
                    self.stop(false);
                }
            }
            None => {
                let act = self.screen.input(&input, &self.sources);
                self.act(act);
            }
        }
        true
    }

    fn draw(&mut self) -> Result<(), String> {
        if !self.ready {
            return Ok(());
        }
        if self.playing.is_some() {
            self.keep_place();
            let (playing, _) = self.playing.as_mut().unwrap();
            if playing.player().ended() {
                self.stop(true);
            } else {
                playing.dirty |= std::mem::take(&mut self.dirty);
                playing.draw(&mut self.app, self.window, &mut self.painter)?;
                return Ok(());
            }
        }
        // Pictures wanted for what is on show are asked for, each once.
        for (ticket, index, source, item) in self.screen.wanted() {
            let Some(entry) = self.sources.get(source) else { continue };
            let source = entry.source.clone();
            self.work.picture(move || Some(Done::Pictured { ticket, index, still: source.picture(&item).and_then(source::pixels) }));
        }
        if !std::mem::take(&mut self.dirty) && !self.screen.animating() {
            return Ok(());
        }
        let (screen, painter, sources) = (&mut self.screen, &mut self.painter, &self.sources);
        self.app.draw(self.window, |gl, size| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            p.begin(gl, size, 1.0, theme::GROUND);
            look::backdrop(p, gl);
            screen.draw(p, gl, sources);
            p.end(gl);
        })?;
        Ok(())
    }

    fn run(mut self) -> Result<(), String> {
        let ended = self.turn();
        // However it ends -- closed, or the room gone -- the film's place is kept first.
        self.stop(false);
        self.work.finish();
        ended
    }

    fn turn(&mut self) -> Result<(), String> {
        loop {
            // While a film plays there is a next picture to be on time for; otherwise a look
            // now and then for what the threads have brought back.
            let wait = match self.playing.as_ref() {
                Some((playing, _)) => playing.next_wake(),
                None => Duration::from_millis(if self.screen.animating() { 33 } else { 120 }),
            };
            for event in self.app.wait(Some(wait))? {
                if !self.handle(event) {
                    return Ok(());
                }
            }
            self.answers();
            self.draw()?;
        }
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let asked = std::env::args_os().nth(1).map(PathBuf::from);
    let run = || -> Result<(), String> {
        let store = Arc::new(Mutex::new(Store::load()));
        let mut app = App::connect("id.prasetya.spatiand.spatireel")?;
        let window = app.open("SpatiReel", WINDOW);
        let local: Arc<dyn Source> = Arc::new(Local::new(library::home(), store.clone()));
        let mut reel = Reel {
            app,
            window,
            painter: None,
            work: Work::new(),
            store,
            sources: vec![Entry { source: local.clone(), kind: Kind::Local }],
            found: Vec::new(),
            screen: Screen::Empty,
            playing: None,
            ticket: 0,
            looked_for_servers: None,
            ready: false,
            dirty: true,
        };
        reel.gather();
        reel.go_home();
        // `SPATIREEL_ADD=jellyfin,<address>,<name>,<password>` (or `emby,...`, or
        // `vibrefy,<address>`) signs in to a server as the form would, for a test that cannot type.
        if let Ok(add) = std::env::var("SPATIREEL_ADD") {
            let parts: Vec<&str> = add.splitn(4, ',').collect();
            if let ["vibrefy", address] = parts[..] {
                reel.screen = Screen::Add(Add::new());
                reel.act(Act::Pair { address: address.into(), cancelled: Arc::default() });
            }
            if let [kind, address, user, password] = parts[..] {
                let flavour = if kind == "emby" { jellyfin::Flavour::Emby } else { jellyfin::Flavour::Jellyfin };
                reel.screen = Screen::Add(Add::new());
                reel.act(Act::SignIn { flavour, address: address.into(), user: user.into(), password: password.into() });
            }
        }
        match asked {
            Some(path) if path.is_dir() => reel.browse(0, Some(Local::item(&path))),
            Some(path) => {
                // The film asked for, and its folder behind it to come back to.
                if let Some(folder) = path.parent().filter(|p| p.is_dir()) {
                    reel.browse(0, Some(Local::item(folder)));
                }
                let mut item = Local::item(&path);
                item.resume = local.unfinished().iter().find(|kept| kept.key == item.key).map_or(0.0, |kept| kept.resume);
                if !Path::new(&item.key).exists() {
                    // Not a file: an address, played as it is.
                    item.key = path.to_string_lossy().into_owned();
                }
                if let Err(e) = reel.play(local, item) {
                    log::warn!("{e}");
                    reel.screen.trouble(format!("{} would not play: {e}", path.display()));
                }
            }
            None => {}
        }
        reel.run()
    };
    if let Err(e) = run() {
        log::error!("{e}");
        std::process::exit(1);
    }
}
