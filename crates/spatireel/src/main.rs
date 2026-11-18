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
mod next;
mod play;
mod screens;
mod source;
mod store;
mod work;

use std::path::PathBuf;
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
    /// Which asking opened it.
    ticket: u64,
    /// The episode after it: not known yet (`None`), then known to be that one or none. Found
    /// once, soon after it opens, for both the Next Episode button and its end.
    next: Option<Option<Item>>,
    /// The listing of its folder that the next was found in, for the one after that.
    listing: Option<next::Listing>,
    /// It is over, its source told so, and the next episode is being opened by this asking.
    chaining: Option<u64>,
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
    /// The film being opened, by its asking's number.
    opening: Option<u64>,
    /// The listing of the folder the film being opened was chosen in, by that asking's number:
    /// its next episode is looked for there before its folder is listed again.
    held: Option<(u64, next::Listing)>,
    looked_for_servers: Option<Instant>,
    ready: bool,
    dirty: bool,
    /// The size the window is when no film is in it: the wearer's, once they have resized it.
    home_size: (u32, u32),
    /// Pictures of a screen that has been replaced, to be let go of at the next drawing.
    stale: Vec<spatiand_app::ui::Texture>,
}

impl Reel {
    /// Put `screen` on show, the one it replaces giving its pictures up.
    fn show(&mut self, screen: Screen) {
        let old = std::mem::replace(&mut self.screen, screen);
        self.stale.extend(old.textures());
    }

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
        let home = Home::new(&self.sources, self.lapsed(), ticket);
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
        self.show(Screen::Home(home));
        self.app.set_title(self.window, "SpatiReel");
        self.dirty = true;
    }

    /// The Vibrefy server whose session ran out, while it is not paired with again.
    fn lapsed(&self) -> Option<String> {
        let store = self.store.lock().unwrap();
        let address = store.vibrefy_last.clone().filter(|_| store.vibrefy_lapsed)?;
        (!store.vibrefy.iter().any(|kept| kept.address == address)).then_some(address)
    }

    /// Servers that have said their sign-in is over are let go of, without being asked: a
    /// Vibrefy pairing lasts hours. Its address is kept, and home offers to pair again.
    fn drop_expired(&mut self) {
        if self.playing.is_some() {
            return;
        }
        let ended: Vec<(String, Account)> = self
            .sources
            .iter()
            .filter(|entry| entry.source.expired())
            .filter_map(|entry| match &entry.kind {
                Kind::Server(account) => Some((entry.source.label(), account.clone())),
                _ => None,
            })
            .collect();
        if ended.is_empty() {
            return;
        }
        {
            let mut store = self.store.lock().unwrap();
            for (label, account) in &ended {
                log::info!("{label}: the server says this session is over; it is let go of");
                if let Account::Vibrefy(account) = account {
                    store.vibrefy.retain(|kept| kept != account);
                    store.vibrefy_last = Some(account.address.clone());
                    store.vibrefy_lapsed = true;
                }
            }
            store.save();
        }
        self.gather();
        self.go_home();
        let names: Vec<&str> = ended.iter().map(|(label, _)| label.as_str()).collect();
        self.screen.trouble(format!("{}: signed out by the server. Reconnect asks for a new code.", names.join(", ")));
    }

    fn list(&mut self, browse: &mut Browse) {
        browse.ticket = self.ticket();
        browse.loading();
        let (source, folder, ticket) = (self.sources[browse.source].source.clone(), browse.folder().cloned(), browse.ticket);
        self.work.call(move || Some(Done::Listed { ticket, result: source.list(folder.as_ref()) }));
    }

    fn browse(&mut self, source: usize, folder: Option<Item>) {
        let Some(entry) = self.sources.get(source) else { return };
        let mut browse = Browse::new(source, entry.source.label(), entry.kind == Kind::Local, matches!(entry.kind, Kind::Server(_)));
        if let Some(folder) = folder {
            browse.enter(folder, 0);
        }
        self.list(&mut browse);
        self.show(Screen::Browse(browse));
        self.dirty = true;
    }

    /// Open a film on a thread of its own, and play it when it is open (`Done::Opened`): a long
    /// film's index may take many seconds to fetch, and a program that stops answering the room
    /// for as long is one the room lets go of.
    ///
    /// `chained` is an episode opened because the one before it ended (`chain`): it is taken up
    /// where it was left unless that was at its credits, and then from its beginning. `held` is
    /// the listing of the folder it was chosen in, if it was.
    fn open(&mut self, source: Arc<dyn Source>, item: Item, chained: bool, held: Option<next::Listing>) -> u64 {
        let ticket = self.ticket();
        self.opening = Some(ticket);
        self.held = held.map(|listing| (ticket, listing));
        if !chained {
            self.screen.trouble(format!("Opening {}\u{2026}", item.title));
        }
        self.work.alone(move || {
            let mut item = item;
            let result = source.play(&item).and_then(|play| {
                let player = if chained {
                    let mut player = Playing::open_player(&play, 0.0)?;
                    item.resume = next::start_at(item.resume, player.info().duration);
                    if item.resume > 5.0 {
                        player.seek(item.resume);
                    }
                    player
                } else {
                    Playing::open_player(&play, item.resume)?
                };
                Ok((player, play))
            });
            Some(Done::Opened { ticket, source, item, result })
        });
        ticket
    }

    fn begin(&mut self, ticket: u64, source: Arc<dyn Source>, item: Item, player: spatiand_media::player::Player, play: &source::Play) -> Result<(), String> {
        let playing = Playing::new(self.app.gl()?, player, play, &item.name, &item.title, item.layout)?;
        self.app.set_title(self.window, &item.title);
        log::info!("{}: playing, from {:.0} s", item.title, item.resume);
        let (told, started) = (source.clone(), item.clone());
        self.work.report(move || told.watching(&started, Watching::Started));
        // The episode after it, looked for once, now, off this thread: in the listing it was
        // chosen from if it was, its folder listed if not.
        let held = self.held.take().filter(|(asked, _)| *asked == ticket).map(|(_, listing)| listing);
        let (finder, current) = (source.clone(), item.clone());
        self.work.call(move || {
            let (next, listing) = next::find(&*finder, &current, held);
            Some(Done::Next { ticket, next, listing })
        });
        self.playing = Some((playing, Watched { source, item, told: Instant::now(), paused: false, ticket, next: None, listing: None, chaining: None }));
        self.dirty = true;
        Ok(())
    }

    /// The film playing is over -- played to its end, or the wearer pressed Next Episode -- and
    /// there is an episode after it: its source is told it is finished, and the next is opened
    /// while it stays on show, saying what comes next, so that no library comes between the two.
    /// False if there is no next episode.
    fn chain(&mut self) -> bool {
        let Some((playing, watched)) = self.playing.as_mut() else { return false };
        if watched.chaining.is_some() {
            return true;
        }
        let Some(Some(next)) = watched.next.clone() else { return false };
        let (source, item) = (watched.source.clone(), watched.item.clone());
        log::info!("{}: over; the next episode, {}, is opened", item.title, next.title);
        let told = source.clone();
        self.work.call(move || {
            told.watching(&item, Watching::Finished);
            None
        });
        playing.player().set_paused(true);
        playing.announce(&format!("Next: {}", next.title));
        let listing = watched.listing.clone();
        let ticket = self.open(source, next, true, listing);
        if let Some((_, watched)) = self.playing.as_mut() {
            watched.chaining = Some(ticket);
        }
        self.dirty = true;
        true
    }

    fn stop(&mut self, finished: bool) {
        let Some((mut playing, watched)) = self.playing.take() else { return };
        if watched.chaining.is_some() {
            // Already told it is finished; and the next episode being opened is not wanted now:
            // dropped when it comes.
            self.opening = None;
        } else {
            let (seconds, duration) = (playing.player().position(), playing.player().info().duration);
            let what = if finished { Watching::Finished } else { Watching::Stopped { seconds, duration } };
            // Told before anything is listed again, so that the list has it: both go through
            // threads, and the one that lists is asked to wait for this.
            let (source, item) = (watched.source, watched.item);
            self.work.call(move || {
                source.watching(&item, what);
                None
            });
        }
        playing.close(&mut self.app, self.window, &mut self.painter);
        self.back_to_library();
    }

    /// The window the library's again, and what is on show in it listed afresh.
    fn back_to_library(&mut self) {
        self.app.resize(self.window, self.home_size);
        self.app.set_title(self.window, "SpatiReel");
        // What was just left is now part watched: a list drawn before is stale.
        match std::mem::replace(&mut self.screen, Screen::Empty) {
            Screen::Browse(mut browse) => {
                browse.keep_cursor();
                self.list(&mut browse);
                self.screen = Screen::Browse(browse);
            }
            other => {
                self.stale.extend(other.textures());
                self.go_home();
            }
        }
        self.dirty = true;
    }

    /// Tell the film's source how far it has got, every ten seconds and whenever it is paused.
    fn keep_place(&mut self) {
        let Some((playing, watched)) = self.playing.as_mut() else { return };
        // Over, and told so: no place in it to tell.
        if watched.chaining.is_some() {
            return;
        }
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
                let held = match &self.screen {
                    Screen::Browse(browse) => browse.listing(),
                    _ => None,
                };
                self.open(entry.source.clone(), item, false, held);
            }
            Act::Add => {
                let add = Add::new(self.store.lock().unwrap().vibrefy_last.clone());
                self.show(Screen::Add(add));
            }
            Act::Reconnect(address) => {
                let mut add = Add::new(Some(address));
                let act = add.pair_again();
                self.show(Screen::Add(add));
                self.act(act);
            }
            Act::Dismiss => {
                let mut store = self.store.lock().unwrap();
                store.vibrefy_lapsed = false;
                store.save();
                drop(store);
                self.go_home();
            }
            Act::Forget(source) => {
                if let Some(Entry { kind: Kind::Server(account), .. }) = self.sources.get(source) {
                    {
                        let mut store = self.store.lock().unwrap();
                        match account {
                            Account::Jellyfin(account) => store.jellyfin.retain(|kept| kept != account),
                            Account::Vibrefy(account) => {
                                store.vibrefy.retain(|kept| kept != account);
                                // Removed on purpose: not offered back, but still the address
                                // the form starts with.
                                store.vibrefy_lapsed = false;
                            }
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
                Done::Passed { ticket, index } => self.screen.passed(ticket, index),
                Done::Unfinished { ticket, source, items } => {
                    if let Screen::Home(home) = &mut self.screen {
                        if home.ticket == ticket {
                            let label = self.sources.get(source).map(|e| e.source.label()).unwrap_or_default();
                            home.unfinished(source, &label, items);
                        }
                    }
                }
                Done::Opened { ticket, source, item, result } => {
                    // Asked for and then given up on (another film chosen, the next episode
                    // stopped while it opened, or the program closing): dropped here, which
                    // closes it.
                    let chained = self.playing.as_ref().is_some_and(|(_, watched)| watched.chaining == Some(ticket));
                    if self.opening != Some(ticket) || (self.playing.is_some() && !chained) {
                        continue;
                    }
                    self.opening = None;
                    if chained {
                        // The episode that ended gives way to the next in the same window.
                        if let Some((old, _)) = self.playing.take() {
                            old.close(&mut self.app, self.window, &mut self.painter);
                        }
                    }
                    self.screen.trouble(String::new());
                    if let Err(e) = result.and_then(|(player, play)| self.begin(ticket, source, item.clone(), player, &play)) {
                        log::warn!("{}: {e}", item.title);
                        if chained {
                            self.back_to_library();
                        }
                        self.screen.trouble(format!("{} would not play: {e}", item.title));
                        self.drop_expired();
                    }
                }
                Done::Next { ticket, next, listing } => {
                    if let Some((playing, watched)) = self.playing.as_mut().filter(|(_, watched)| watched.ticket == ticket) {
                        playing.offer(next.as_ref().map(|next| next.title.clone()));
                        watched.next = Some(next);
                        watched.listing = listing;
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
                                    // The same account paired again is the one kept anew; another
                                    // account on the same server is a source of its own.
                                    store.vibrefy.retain(|kept| !(kept.address == account.address && kept.account == account.account));
                                    store.vibrefy.push(account.clone());
                                    store.vibrefy_last = Some(account.address.clone());
                                    store.vibrefy_lapsed = false;
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

    /// What the window shows, for dictation and the assistant: the film and where it is, and the
    /// next episode while it is offered.
    fn description(&mut self) -> spatiand_app::Description {
        let (title, text) = match self.playing.as_mut() {
            Some((playing, _)) => (playing.name.clone(), playing.said()),
            None => ("SpatiReel".to_string(), String::new()),
        };
        spatiand_app::Description { kind: "video".into(), app: "SpatiReel".into(), title, text, ..Default::default() }
    }

    fn handle(&mut self, event: Event) -> bool {
        match &event {
            Event::CloseRequested { window } if *window == self.window => return false,
            // Command-Q or Control-Q (the SDK's), from the window or a film's sky: SpatiReel
            // goes, keeping the film's place first as when it is closed (`run`).
            Event::Quit { .. } => return false,
            // The film's own surface was closed: the film is over.
            Event::CloseRequested { .. } => self.stop(false),
            Event::Ready { window, .. } if *window == self.window => self.ready = true,
            // Resized -- a corner dragged in the room, or a Mac window in 2D mode -- the lists
            // are laid out again at the new size. Not while a film is in the window: its picture
            // is the film's own, which the room or the Mac scales to fit.
            Event::Resized { window, width, height } if *window == self.window => {
                // The library's size is the wearer's only when it is the library that was
                // resized: a film's window resized is the film's, and the library came back
                // at that shape.
                if self.playing.is_none() {
                    self.home_size = ((*width).max(640), (*height).max(400));
                    self.app.resize(self.window, self.home_size);
                }
            }
            // Dictation and the assistant ask what is on show.
            Event::Describe { window } => {
                let description = self.description();
                self.app.describe(*window, &description);
                return true;
            }
            Event::Mode(mode) => log::info!("the wearer now sees {mode:?}"),
            // Dark or light: drawn again in the theme's new colours.
            Event::Look { .. } => self.dirty = true,
            Event::Focus { window, focused } if *window == self.window => {
                if let Some((playing, _)) = self.playing.as_mut() {
                    playing.focus(*focused);
                }
            }
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
        // Held down, a key repeats: pausing, playing and the menu are pressed once, or a key held
        // a moment too long paused and played the film again and again, or opened the menu
        // over and over. The arrows still move on as they repeat.
        if let (Some(_), Event::Key { key: spatiand_app::Key::Space | spatiand_app::Key::Enter | spatiand_app::Key::Char(_), repeat: true, .. }) = (self.playing.as_ref(), &event) {
            return true;
        }
        let Some(input) = Input::of(&event, 1.0) else { return true };
        match self.playing.as_mut() {
            Some((playing, _)) => match playing.input(&input) {
                Outcome::Close => self.stop(false),
                // Watched to the credits: finished, and on to the next.
                Outcome::Next => {
                    self.chain();
                }
                Outcome::Nothing => {}
            },
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
        if let (false, Ok(gl), Some(p)) = (self.stale.is_empty(), self.app.gl(), self.painter.as_mut()) {
            for old in self.stale.drain(..) {
                p.forget(gl, old);
            }
        }
        if self.playing.is_some() {
            self.keep_place();
            let (playing, watched) = self.playing.as_mut().unwrap();
            // Over and its next being opened: it stays on show until the next is.
            if playing.player().ended() && watched.chaining.is_none() {
                let known = watched.next.is_some();
                let title = playing.name.clone();
                // Stopped short by what could not be read: left where it was, not finished, and
                // the wearer told why.
                match playing.player().failed() {
                    Some(why) => {
                        log::warn!("{title}: {why}");
                        self.stop(false);
                        self.screen.trouble(format!("{title} stopped: {why}"));
                    }
                    // Played to its end: the next episode if there is one, once it is known
                    // whether there is (a film of seconds may end before), and home if not.
                    None if known => {
                        if !self.chain() {
                            log::info!("{title}: played to its end, and no episode after it");
                            self.stop(true);
                        }
                    }
                    None => {}
                }
            }
            if let Some((playing, _)) = self.playing.as_mut() {
                playing.dirty |= std::mem::take(&mut self.dirty);
                playing.draw(&mut self.app, self.window, &mut self.painter)?;
                return Ok(());
            }
        }
        // Pictures wanted for what is on show are asked for, each once.
        for (ticket, index, source, item, shown) in self.screen.wanted() {
            let Some(entry) = self.sources.get(source) else { continue };
            let source = entry.source.clone();
            self.work.picture(move || {
                // Scrolled away from while it waited its turn: not worth the server's trouble.
                if shown.is_some_and(|shown| !shown.lock().unwrap().contains(&index)) {
                    return Some(Done::Passed { ticket, index });
                }
                Some(Done::Pictured { ticket, index, still: source.picture(&item).and_then(source::pixels) })
            });
        }
        if !std::mem::take(&mut self.dirty) && !self.screen.animating() {
            return Ok(());
        }
        let (screen, painter, sources) = (&mut self.screen, &mut self.painter, &self.sources);
        self.app.draw(self.window, |gl, size| {
            let p = painter.get_or_insert_with(|| Painter::new(gl).expect("the painter"));
            p.begin(gl, size, 1.0, theme::ground());
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
            self.drop_expired();
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
        let local: Arc<dyn Source> = Arc::new(Local::new(library::home()));
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
            opening: None,
            held: None,
            looked_for_servers: None,
            ready: false,
            dirty: true,
            home_size: WINDOW,
            stale: Vec::new(),
        };
        reel.gather();
        reel.go_home();
        // `SPATIREEL_ADD=jellyfin,<address>,<name>,<password>` (or `emby,...`, or
        // `vibrefy,<address>`) signs in to a server as the form would, for a test that cannot type.
        if let Ok(add) = std::env::var("SPATIREEL_ADD") {
            let parts: Vec<&str> = add.splitn(4, ',').collect();
            if let ["vibrefy", address] = parts[..] {
                reel.show(Screen::Add(Add::new(None)));
                reel.act(Act::Pair { address: address.into(), cancelled: Arc::default() });
            }
            if let [kind, address, user, password] = parts[..] {
                let flavour = if kind == "emby" { jellyfin::Flavour::Emby } else { jellyfin::Flavour::Jellyfin };
                reel.show(Screen::Add(Add::new(None)));
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
                if !path.exists() {
                    // Not a file: an address, played as it is.
                    item.key = path.to_string_lossy().into_owned();
                    item.path = None;
                }
                // Opened as one chosen is, on a thread of its own: an address that never answers
                // kept the window from ever being shown.
                reel.open(local, item, false, None);
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
