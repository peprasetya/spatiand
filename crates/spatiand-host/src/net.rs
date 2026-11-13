//! The host's half of the link, on its own thread.
//!
//! The compositor thread must never wait for a network, and a network must never wait for a
//! GPU. So they are separate, and everything between them is a message on a queue: the
//! compositor posts encoded pictures and window news, and takes whatever the session has said
//! since it last looked.
//!
//! ## What goes which way
//!
//! * **Control** — windows opening and closing, the catalogue, stream settings — goes on a
//!   reliable QUIC stream, length-prefixed. It is small and it must arrive.
//! * **Pictures** go as datagrams, cut to whatever the path will carry in one packet. A piece
//!   that goes missing loses its frame and nothing else; see [`spatiand_stream::video`].
//!
//! ## Only one session at a time, and the newest wins
//!
//! A second one taking over is the useful behaviour — you have put the Deck down and picked up
//! the Beam Pro — and two at once is not: they would fight over window sizes, which window has
//! focus and what the bitrate should be. So the newcomer wins: the old connection is closed
//! with [`CLOSE_TAKEN_OVER`], and every application still running here comes back on the new
//! one exactly as a reconnect brings them back. Nothing here belongs to a device: the
//! applications are the host's, and whoever is connected is looking at them.
//!
//! This used to say the same and do otherwise: the accept loop sat inside the running session,
//! so a second device waited, connected and seeing nothing, until the first went away.
//!
//! ## A newcomer that is not paired takes nothing over
//!
//! While a pairing window is open the gate lets every certificate through the handshake, and
//! "the newest wins" then meant anybody: a machine that did no more than connect closed the
//! session that was in with [`CLOSE_TAKEN_OVER`] -- which a session does not come back from by
//! itself -- before it had said one byte of the pairing code, and could do it again for as
//! long as the window stood open. So there are two seats, kept by [`Seats`]:
//!
//! * **the session**: a connection whose certificate is paired sits here the moment it
//!   arrives, taking over as above;
//! * **the one waiting**: a connection whose certificate is not paired is held *beside* the
//!   session. It is heard for its pairing messages and told the pairing replies, and that is
//!   all (see [`waiting`]); the session goes on undisturbed. Only when the compositor says it
//!   has proved the code ([`ToSession::Proved`]) does it take the session's seat. A wrong
//!   code, a locked or closed window ([`ToSession::TurnAway`]), or its own time running out,
//!   and it is hung up on with nobody else having noticed it.
//!
//! One waits at a time, and there the newest wins: strangers can push only each other out.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};

use quinn::Connection;
use spatiand_stream::transport::{peer_fingerprint, Trust, CLOSE_TAKEN_OVER};
use spatiand_stream::video::{split, Packet, FLAG_KEYFRAME, FLAG_LAST};
use spatiand_stream::{ClientMessage, Fingerprint, HostMessage, Identity};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// What the compositor sends out.
pub enum ToSession {
    Control(HostMessage),
    /// Hang up on it: it was refused.
    Close,
    /// For the connection waiting to pair, by its number, and never for the session: the
    /// host's half of the pairing exchange or its refusal. Anything else is not sent it,
    /// whatever is put here.
    Stranger { session: u64, message: HostMessage },
    /// Hang up on the connection waiting to pair, after whatever it was just told: its code
    /// was wrong, or pairing has closed. Said of any other connection, it does nothing.
    TurnAway { session: u64 },
    /// The connection waiting to pair has proved the code (or turned out to be paired): it is
    /// the session from here on, and whoever was is taken over. Said of any other connection,
    /// it does nothing.
    Proved { session: u64 },
    /// One encoded picture, already whole.
    Video {
        window: u16,
        frame: u32,
        viewport: u32,
        keyframe: bool,
        captured_us: u64,
        bytes: Vec<u8>,
    },
    /// Ten milliseconds of one application's sound. See `audio`.
    Audio {
        app: String,
        /// How many channels the samples are, which is how wide the application's sink is.
        channels: u16,
        pcm: Vec<u8>,
    },
}

/// What arrives.
///
/// Everything carries which connection it is about, because there may be two: the session,
/// and one waiting to pair.
pub enum FromSession {
    /// Somebody is here, as the session. Carries who, so pairing can write it down, and which
    /// connection this is, to let it in by; see [`Admission`].
    Joined {
        session: u64,
        who: Fingerprint,
        address: SocketAddr,
    },
    /// Somebody the network does not know as paired has connected, and is held waiting: it
    /// has taken nothing over and is told nothing until the compositor answers its pairing
    /// messages ([`ToSession::Stranger`]), lets it be the session ([`ToSession::Proved`]) or
    /// hangs up on it ([`ToSession::TurnAway`]).
    Knocked {
        session: u64,
        who: Fingerprint,
        address: SocketAddr,
    },
    /// What a connection said. From the session, anything; from the one waiting, only its
    /// pairing messages ever get this far ([`heard_from`]).
    Said { session: u64, message: ClientMessage },
    /// That connection is gone, whichever of the two it was.
    Left { session: u64 },
}

/// Which session has been let in, if one has.
///
/// **A connection, not a yes or no.** The compositor hears of a session some turns of its loop
/// after the network has it, and by then the network may have another: a paired device joins,
/// a stranger comes through an open pairing gate and takes over before the first's arrival
/// has been read, and the "let it in" meant for the first lands on the stranger -- which was
/// then told the windows and the clipboard and had its microphone played, without the code.
/// So every connection has a number, the compositor lets a number in, and a session is in
/// only while the number is its own.
#[derive(Clone, Default)]
pub struct Admission(Arc<AtomicU64>);

impl Admission {
    /// No connection has this number: they count from one.
    const NOBODY: u64 = 0;

    fn admit(&self, session: u64) {
        self.0.store(session, Ordering::SeqCst);
    }

    /// Take back what `session` was given; nothing, if it is another that is in.
    fn withdraw(&self, session: u64) {
        let _ = self
            .0
            .compare_exchange(session, Self::NOBODY, Ordering::SeqCst, Ordering::SeqCst);
    }

    fn is(&self, session: u64) -> bool {
        session != Self::NOBODY && self.0.load(Ordering::SeqCst) == session
    }
}

/// The paired list, as the network thread reads it when a connection arrives.
///
/// The gate has the same list, but while pairing is open it answers yes for everybody, and the
/// network has to tell the two apart: a paired certificate takes the session's seat at once, a
/// stranger's waits. If this copy is behind, nothing is given away either way: a paired device
/// it does not know yet waits one turn of the compositor's loop, which says [`ToSession::Proved`]
/// on finding it in its own list; a removed one it still believes takes the seat and is told
/// nothing, because being let in is the compositor's to say ([`Admission`]).
#[derive(Clone, Default)]
struct Known(Arc<RwLock<Vec<Fingerprint>>>);

impl Known {
    fn set(&self, paired: Vec<Fingerprint>) {
        if let Ok(mut list) = self.0.write() {
            *list = paired;
        }
    }

    fn contains(&self, who: &Fingerprint) -> bool {
        self.0.read().is_ok_and(|list| list.contains(who))
    }
}

/// Whether a connection is the session or is still waiting to pair, shared with the tasks
/// that read what it says: they are started when it arrives, and it may become the session
/// while they are reading.
#[derive(Clone)]
struct Standing(Arc<AtomicBool>);

impl Standing {
    fn new(session: bool) -> Standing {
        Standing(Arc::new(AtomicBool::new(session)))
    }

    fn is_session(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn now_the_session(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Which of the two seats a connection has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seat {
    Session,
    Waiting,
}

/// Who sits where: the whole of the decision about who takes over from whom, apart from the
/// sockets so that it can be tested without one.
///
/// Connections are named by their numbers. Every answer says who has to go, so the caller only
/// closes what it is told to.
#[derive(Debug, Default, PartialEq, Eq)]
struct Seats {
    session: Option<u64>,
    waiting: Option<u64>,
}

impl Seats {
    /// A connection has arrived. Paired, it is the session and whoever was is taken over;
    /// not paired, it waits, and only another that was waiting gives way to it. Gives the seat
    /// it got and who lost that seat to it.
    fn arrived(&mut self, newcomer: u64, paired: bool) -> (Seat, Option<u64>) {
        if paired {
            (Seat::Session, self.session.replace(newcomer))
        } else {
            (Seat::Waiting, self.waiting.replace(newcomer))
        }
    }

    /// The compositor says `who` has proved the pairing code. `None` if it is not the one
    /// waiting -- it left, or another pushed it out -- and then nothing changes; otherwise it
    /// is the session now, and inside is the session it takes over from, if there was one.
    fn proved(&mut self, who: u64) -> Option<Option<u64>> {
        if self.waiting != Some(who) {
            return None;
        }
        self.waiting = None;
        Some(self.session.replace(who))
    }

    /// The compositor says to hang up on `who`. Only the one waiting can be: `false`, and
    /// nothing changes, for anybody else.
    fn turned_away(&mut self, who: u64) -> bool {
        if self.waiting != Some(who) {
            return false;
        }
        self.waiting = None;
        true
    }

    /// `who` has gone by itself. Gives the seat it leaves empty; `None` if it had none any
    /// more, having been taken over or turned away already.
    fn ended(&mut self, who: u64) -> Option<Seat> {
        if self.session == Some(who) {
            self.session = None;
            Some(Seat::Session)
        } else if self.waiting == Some(who) {
            self.waiting = None;
            Some(Seat::Waiting)
        } else {
            None
        }
    }
}

/// What a connection that has not proved the pairing code may say and be heard: the two
/// halves of the exchange, and the `Hello` of a device from before codes were typed, which is
/// answered with how to update. Nothing else it says leaves the network thread.
fn pairing_message(message: &ClientMessage) -> bool {
    matches!(
        message,
        ClientMessage::PairStart { .. } | ClientMessage::PairConfirm { .. } | ClientMessage::Hello { .. }
    )
}

/// What a connection that has not proved the pairing code may be told: the host's half of the
/// exchange, and no.
fn for_the_unproven(message: &HostMessage) -> bool {
    matches!(message, HostMessage::PairReply { .. } | HostMessage::Refused { .. })
}

/// Pass on what a connection said, if it may say it: a session anything, one waiting to pair
/// only its pairing messages. `false` once the compositor is not there to hear.
///
/// Whether a session has been *let in* is not asked here but by the compositor, which hears
/// of its joining and of what it says in the order they happened.
fn heard_from(
    serial: u64,
    standing: &Standing,
    message: ClientMessage,
    inbox: &Sender<FromSession>,
) -> bool {
    if !standing.is_session() && !pairing_message(&message) {
        log::debug!("a connection waiting to pair said something else; not listened to");
        return true;
    }
    inbox.send(FromSession::Said { session: serial, message }).is_ok()
}

/// The network, as the compositor sees it: post, and poll.
pub struct Net {
    /// Which session has been let in. Until it has, nothing is sent it but its refusal, and
    /// nothing it sends is listened to, whoever in the compositor forgets to ask.
    admitted: Admission,
    /// Who is paired, for the network thread to seat arrivals by. See [`Known`].
    known: Known,
    out: UnboundedSender<ToSession>,
    inbox: Receiver<FromSession>,
}

impl Net {
    /// Start listening. The returned handle is the only thing the compositor touches.
    /// `poses` is written on the network thread from the viewports the session sends; see
    /// `crate::pose` for why there and not in the compositor's loop. `paired` is the paired
    /// list as it stands; [`Net::set_paired`] whenever it changes.
    pub fn start(
        bind: SocketAddr,
        identity: Identity,
        trust: Trust,
        paired: Vec<Fingerprint>,
        poses: Option<crate::pose::Poses>,
    ) -> Result<Net, String> {
        let (out, outbox) = unbounded_channel();
        let (inbox_tx, inbox) = std::sync::mpsc::channel();
        let admitted = Admission::default();
        let admitted_there = admitted.clone();
        let known = Known::default();
        known.set(paired);
        let known_there = known.clone();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            // Two: one for the socket, one for everything else. A host is not a web server.
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("could not start the network runtime: {e}"))?;
        // Inside the runtime, because a QUIC endpoint takes hold of the runtime it is created
        // in and refuses to exist without one — "no async runtime found" is what it says, from
        // a line that does not look asynchronous at all.
        let (endpoint, address) = {
            let _guard = runtime.enter();
            let endpoint = spatiand_stream::transport::server(bind, &identity, trust)?;
            let address = endpoint
                .local_addr()
                .map_err(|e| format!("could not read the listening address: {e}"))?;
            (endpoint, address)
        };
        log::info!("listening for a session on {address}");

        std::thread::Builder::new()
            .name("spatiand-host-net".into())
            .spawn(move || {
                runtime.block_on(serve(endpoint, outbox, inbox_tx, poses, admitted_there, known_there));
            })
            .map_err(|e| format!("could not start the network thread: {e}"))?;
        Ok(Net { admitted, known, out, inbox })
    }

    /// The paired list changed: said wherever the gate is told. A connection arriving from
    /// now on is seated by this one.
    pub fn set_paired(&self, paired: Vec<Fingerprint>) {
        self.known.set(paired);
    }

    /// The session that joined as `session` is let in, or is not any more. Said of one that
    /// has already gone, it lets nobody in.
    pub fn set_admitted(&self, session: u64, admitted: bool) {
        if admitted {
            self.admitted.admit(session);
        } else {
            self.admitted.withdraw(session);
        }
    }

    /// Post something. Never blocks, and never fails in a way the caller can do anything about:
    /// a session that has gone away is not an error the compositor should handle.
    pub fn send(&self, message: ToSession) {
        let _ = self.out.send(message);
    }

    /// A way to post from another thread: the sound readers each have one.
    pub fn sender(&self) -> UnboundedSender<ToSession> {
        self.out.clone()
    }

    /// Everything that has arrived since the last call.
    pub fn poll(&self) -> Vec<FromSession> {
        self.inbox.try_iter().collect()
    }
}

/// A connection the network thread has, as that thread keeps it: enough to close it, to say
/// who it is, and to feed the task that attends to it.
struct There {
    connection: Connection,
    who: Fingerprint,
    standing: Standing,
    feed: UnboundedSender<Fed>,
}

/// What the network thread hands the task attending to one connection.
enum Fed {
    /// From the compositor, for the session.
    Out(ToSession),
    /// From the compositor, for the one waiting: a pairing reply or a refusal, checked to be
    /// one before it is put here.
    Pairing(HostMessage),
    /// The one waiting has proved the code: it is the session from here on.
    Proved,
    /// The one waiting is to be hung up on, after what it has already been handed is written.
    TurnedAway,
}

/// What the network thread wakes for.
enum Event {
    Arrived(Connection, Fingerprint),
    Posted(ToSession),
    /// The task attending to the connection with this number has finished.
    Ended(u64),
}

/// The network thread's own loop: who sits where, and what goes to whom.
///
/// It never waits on a connection. Each one has a task of its own ([`attend`]) that does the
/// writing and the reading, and this only decides: an arrival is seated ([`Seats`]), what the
/// compositor posts is handed to whoever it is for, and a connection that ends leaves its
/// seat. So one that will not read what it is sent -- a stranger's is the case to think of --
/// holds up nothing but itself.
async fn serve(
    endpoint: quinn::Endpoint,
    mut outbox: UnboundedReceiver<ToSession>,
    inbox: Sender<FromSession>,
    poses: Option<crate::pose::Poses>,
    admitted: Admission,
    known: Known,
) {
    // Shared with whichever task is attending to the session: one at a time writes it, the
    // lock is only what lets the ring pass from one session's task to the next.
    let poses = poses.map(|poses| Arc::new(Mutex::new(poses)));
    let (arrived, mut arrivals) = unbounded_channel();
    tokio::spawn(doorway(endpoint, arrived));
    let (ended, mut endings) = unbounded_channel::<u64>();
    let mut seats = Seats::default();
    let mut there: HashMap<u64, There> = HashMap::new();
    // Which connection this is, counted from one. See `Admission`.
    let mut serial: u64 = 0;
    // What was found in the queue while it was being emptied for a new session and is not the
    // old session's to lose; see `stale`.
    let mut backlog: VecDeque<ToSession> = VecDeque::new();
    loop {
        let event = match backlog.pop_front() {
            Some(posted) => Event::Posted(posted),
            None => tokio::select! {
                arrived = arrivals.recv() => match arrived {
                    Some((connection, who)) => Event::Arrived(connection, who),
                    // The endpoint has closed.
                    None => return,
                },
                posted = outbox.recv() => match posted {
                    Some(posted) => Event::Posted(posted),
                    // The compositor has gone, and the process is going.
                    None => return,
                },
                Some(gone) = endings.recv() => Event::Ended(gone),
            },
        };
        match event {
            Event::Arrived(connection, who) => {
                serial += 1;
                let address = connection.remote_address();
                let (seat, displaced) = seats.arrived(serial, known.contains(&who));
                let standing = Standing::new(seat == Seat::Session);
                let (feed, fed) = unbounded_channel();
                match seat {
                    // Paired: one at a time, and the newest wins.
                    Seat::Session => {
                        match displaced {
                            Some(old) => taken_over(old, &who, &mut there, &inbox),
                            None => log::info!("session {} joined from {address}", who.short()),
                        }
                        stale(&mut outbox, &mut backlog);
                        reset(&poses);
                        let _ = inbox.send(FromSession::Joined { session: serial, who, address });
                    }
                    // Not paired: in through an open pairing gate, and held beside the
                    // session, which hears nothing of it.
                    Seat::Waiting => {
                        if let Some(old) = displaced.and_then(|old| Some((old, there.remove(&old)?))) {
                            log::info!(
                                "{} waits to pair in place of {}, which is hung up on",
                                who.short(),
                                old.1.who.short()
                            );
                            old.1.connection.close(1u32.into(), b"another device is pairing");
                            let _ = inbox.send(FromSession::Left { session: old.0 });
                        }
                        let _ = inbox.send(FromSession::Knocked { session: serial, who, address });
                    }
                }
                tokio::spawn(attend(
                    connection.clone(),
                    serial,
                    standing.clone(),
                    fed,
                    inbox.clone(),
                    poses.clone(),
                    admitted.clone(),
                    ended.clone(),
                ));
                there.insert(serial, There { connection, who, standing, feed });
            }
            Event::Posted(ToSession::Stranger { session, message }) => {
                // To the one waiting and to nobody else, and only what one waiting may hear.
                if seats.waiting != Some(session) || !for_the_unproven(&message) {
                    continue;
                }
                if let Some(waiting) = there.get(&session) {
                    let _ = waiting.feed.send(Fed::Pairing(message));
                }
            }
            Event::Posted(ToSession::TurnAway { session }) => {
                if !seats.turned_away(session) {
                    continue;
                }
                // Through its own task, so that the refusal it was handed a moment ago is
                // written before the line is cut.
                if let Some(waiting) = there.remove(&session) {
                    log::info!("{} is hung up on, not paired", waiting.who.short());
                    let _ = waiting.feed.send(Fed::TurnedAway);
                }
                // And said back like any other leaving: the compositor closes the attempt
                // that connection had under way when it hears this.
                let _ = inbox.send(FromSession::Left { session });
            }
            Event::Posted(ToSession::Proved { session }) => {
                let Some(displaced) = seats.proved(session) else { continue };
                let Some((who, address)) =
                    there.get(&session).map(|t| (t.who, t.connection.remote_address()))
                else {
                    continue;
                };
                // **Only now**, with the code proved, does anybody lose their seat to it.
                match displaced {
                    Some(old) => taken_over(old, &who, &mut there, &inbox),
                    None => log::info!("session {} joined from {address}, newly paired", who.short()),
                }
                stale(&mut outbox, &mut backlog);
                reset(&poses);
                // In this order: what it says from here on is passed along as a session's,
                // and must not reach the compositor ahead of the news that it is one.
                let _ = inbox.send(FromSession::Joined { session, who, address });
                if let Some(proved) = there.get(&session) {
                    proved.standing.now_the_session();
                    let _ = proved.feed.send(Fed::Proved);
                }
            }
            // Everything else is the session's. With nobody in that seat it was for a session
            // that has gone.
            Event::Posted(posted) => {
                if let Some(session) = seats.session.and_then(|session| there.get(&session)) {
                    let _ = session.feed.send(Fed::Out(posted));
                }
            }
            Event::Ended(gone) => {
                // One taken over or turned away has been said to have left already.
                if seats.ended(gone).is_none() {
                    continue;
                }
                if let Some(left) = there.remove(&gone) {
                    log::info!("session {} left", left.who.short());
                }
                let _ = inbox.send(FromSession::Left { session: gone });
            }
        }
    }
}

/// The session gives its seat to `by`: closed with [`CLOSE_TAKEN_OVER`], and said to have left.
fn taken_over(old: u64, by: &Fingerprint, there: &mut HashMap<u64, There>, inbox: &Sender<FromSession>) {
    let Some(old_one) = there.remove(&old) else { return };
    log::info!(
        "session {} takes over from {}; its windows go with it",
        by.short(),
        old_one.who.short()
    );
    old_one.connection.close(CLOSE_TAKEN_OVER.into(), b"another device took over");
    let _ = inbox.send(FromSession::Left { session: old });
}

/// Empty the queue for a session that is starting.
///
/// Whatever is still queued was meant for the session before this one -- pictures and news of
/// its windows, posted before the compositor heard it had gone. This one hears its windows
/// from the start, after its own hello. What is addressed to a connection by its number is
/// not the old session's and is kept, to be dealt with in its turn.
fn stale(outbox: &mut UnboundedReceiver<ToSession>, backlog: &mut VecDeque<ToSession>) {
    while let Ok(posted) = outbox.try_recv() {
        if matches!(
            posted,
            ToSession::Stranger { .. } | ToSession::TurnAway { .. } | ToSession::Proved { .. }
        ) {
            backlog.push_back(posted);
        }
    }
}

/// A new session's head starts from nothing.
fn reset(poses: &Option<Arc<Mutex<crate::pose::Poses>>>) {
    if let Some(Ok(mut poses)) = poses.as_ref().map(|poses| poses.lock()) {
        poses.reset();
    }
}

/// Take connections through their handshake and pass each on, with who it is, until the
/// endpoint closes.
///
/// On a task of its own because a handshake is not something to abandon half way: the loop
/// that seats them wakes for every picture posted, and waiting for a handshake there would
/// drop it each time.
async fn doorway(endpoint: quinn::Endpoint, arrived: UnboundedSender<(Connection, Fingerprint)>) {
    while let Some(incoming) = endpoint.accept().await {
        let connection = match incoming.await {
            Ok(connection) => connection,
            Err(e) => {
                log::warn!("a session did not get as far as connecting: {e}");
                continue;
            }
        };
        let Some(who) = peer_fingerprint(&connection) else {
            log::warn!("a session connected without a certificate; closing it");
            connection.close(1u32.into(), b"no identity");
            continue;
        };
        if arrived.send((connection, who)).is_err() {
            return;
        }
    }
}

/// Attend to one connection for as long as it lasts: waiting to pair if it came unpaired, then
/// -- or from the start -- as the session.
///
/// One task for both, because the device hears everything on the one control stream the host
/// opened first: its pairing reply, and then, on the same stream, its welcome.
#[allow(clippy::too_many_arguments)]
async fn attend(
    connection: Connection,
    serial: u64,
    standing: Standing,
    mut fed: UnboundedReceiver<Fed>,
    inbox: Sender<FromSession>,
    poses: Option<Arc<Mutex<crate::pose::Poses>>>,
    admitted: Admission,
    ended: UnboundedSender<u64>,
) {
    let mut control = match connection.open_uni().await {
        Ok(stream) => stream,
        Err(e) => {
            log::warn!("could not open the control stream: {e}");
            let _ = ended.send(serial);
            return;
        }
    };

    // What it says, read on its own task so that a slow reader cannot hold up pictures.
    // Whether what is read is passed on depends on its standing at that moment; see
    // `heard_from`.
    let reading = tokio::spawn({
        let connection = connection.clone();
        let inbox = inbox.clone();
        let admitted = admitted.clone();
        let standing = standing.clone();
        async move {
            loop {
                let stream = match connection.accept_uni().await {
                    Ok(stream) => stream,
                    Err(_) => return,
                };
                tokio::spawn(heard(stream, serial, inbox.clone(), admitted.clone(), standing.clone()));
            }
        }
    });

    if standing.is_session() || waiting(&connection, &mut control, &mut fed).await {
        session(&connection, serial, control, &mut fed, poses.as_deref(), &admitted).await;
    }
    reading.abort();
    let _ = ended.send(serial);
}

/// How long a connection may wait to pair before it is hung up on whatever else happens: no
/// pairing window is open for longer, and the compositor turns it away when its window closes.
/// This is for the day that goes wrong.
const LONGEST_WAIT: std::time::Duration = crate::pair::PAIRING_WINDOW;

/// How long one waiting may take to accept what it is told. Its replies are a few dozen bytes;
/// one that will not take them is not pairing.
const SLOWEST_REPLY: std::time::Duration = std::time::Duration::from_secs(2);

/// Everything a connection gets that has not proved the pairing code. `true` when it has, and
/// is the session from here on; `false` when it is gone.
///
/// **This is all of it.** What it is told: the pairing replies and refusals the compositor
/// addresses to it by number, on the control stream. Nothing posted for the session comes
/// here -- no welcome, no catalogue, no window, picture, sound or clipboard -- because the
/// network thread hands those to the session's task and this is not it. What it says is read
/// by the same tasks as a session's, which pass on its pairing messages and nothing else
/// ([`heard_from`]), each within the small limit for one not let in; its datagrams are
/// thrown away, and its sound is not played.
async fn waiting(
    connection: &Connection,
    control: &mut quinn::SendStream,
    fed: &mut UnboundedReceiver<Fed>,
) -> bool {
    let too_long = tokio::time::sleep(LONGEST_WAIT);
    tokio::pin!(too_long);
    loop {
        tokio::select! {
            handed = fed.recv() => match handed {
                Some(Fed::Proved) => return true,
                Some(Fed::Pairing(message)) => {
                    let told = tokio::time::timeout(
                        SLOWEST_REPLY,
                        spatiand_stream::link::tell(control, &message),
                    )
                    .await;
                    if !matches!(told, Ok(Ok(()))) {
                        connection.close(1u32.into(), b"not paired");
                        return false;
                    }
                }
                Some(Fed::TurnedAway) => {
                    connection.close(1u32.into(), b"not paired");
                    return false;
                }
                // The session's, and never handed to one waiting.
                Some(Fed::Out(_)) => {}
                // Pushed out by another that came to pair, and closed already.
                None => return false,
            },
            // Read so that they do not pile up, and dropped: a head is a session's to send.
            datagram = connection.read_datagram() => {
                if datagram.is_err() {
                    return false;
                }
            }
            _ = connection.closed() => return false,
            _ = &mut too_long => {
                log::info!("a connection waited to pair for longer than pairing is ever open; closing it");
                connection.close(1u32.into(), b"not paired");
                return false;
            }
        }
    }
}

/// The session: everything the compositor posts goes out, and its head comes in.
///
/// `control` is the stream the connection's task opened when it arrived -- for one that
/// paired a moment ago, the one its pairing reply went down.
async fn session(
    connection: &Connection,
    serial: u64,
    mut control: quinn::SendStream,
    fed: &mut UnboundedReceiver<Fed>,
    poses: Option<&Mutex<crate::pose::Poses>>,
    admitted: &Admission,
) {
    let mut frames_sent: u64 = 0;
    let mut dropped: u64 = 0;
    // One stream per application's sound, each written by a task of its own so that a sound
    // stream held up by flow control can never hold up pictures.
    // Each application's stream and the width it was opened at: a stream is one shape for its
    // whole life, so an application whose sink changed shape is given a new one.
    let mut sounds: HashMap<String, (u16, tokio::sync::mpsc::Sender<Vec<u8>>)> = HashMap::new();
    let mut sound_dropped: u64 = 0;
    loop {
        tokio::select! {
            handed = fed.recv() => {
                let Some(handed) = handed else { break };
                // Only what the compositor posts for the session; the rest was for it while
                // it waited to pair, and it is past that.
                let Fed::Out(outgoing) = handed else { continue };
                // **The only way out.** A session the compositor has not let in -- one the
                // network took for paired from a list that was behind -- gets its refusal and
                // nothing else: not a picture, a sound, a window's name or the clipboard.
                let for_anybody = matches!(
                    outgoing,
                    ToSession::Close | ToSession::Control(HostMessage::Refused { .. })
                );
                if !for_anybody && !admitted.is(serial) {
                    continue;
                }
                match outgoing {
                    // For a connection by its number: the network thread deals with these
                    // itself and hands none of them on.
                    ToSession::Stranger { .. } | ToSession::TurnAway { .. } | ToSession::Proved { .. } => {}
                    ToSession::Close => {
                        connection.close(1u32.into(), b"not paired");
                        break;
                    }
                    ToSession::Control(message) => {
                        let bytes = match spatiand_stream::to_bytes(&message) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                log::error!("could not encode a message for the session: {e}");
                                continue;
                            }
                        };
                        let length = (bytes.len() as u32).to_le_bytes();
                        if control.write_all(&length).await.is_err()
                            || control.write_all(&bytes).await.is_err()
                        {
                            break;
                        }
                    }
                    ToSession::Audio { app, channels, pcm } => {
                        let alive = sounds
                            .get(&app)
                            .is_some_and(|(width, s)| *width == channels && !s.is_closed());
                        if !alive {
                            sounds.insert(
                                app.clone(),
                                (channels, sound_stream(connection, app.clone(), channels)),
                            );
                        }
                        // A third of a second may wait; past that the listener is hearing the
                        // past, and the oldest is what should go — but a bounded queue can only
                        // refuse the newest, which at this size is the same few milliseconds.
                        if let Some(Err(tokio::sync::mpsc::error::TrySendError::Full(_))) =
                            sounds.get(&app).map(|(_, s)| s.try_send(pcm))
                        {
                            sound_dropped += 1;
                            if sound_dropped.is_power_of_two() {
                                log::info!("{sound_dropped} pieces of sound dropped: the link is behind");
                            }
                        }
                    }
                    ToSession::Video { window, frame, viewport, keyframe, captured_us, bytes } => {
                        // One frame, cut to the path's own packet size. Asking the connection
                        // each time rather than assuming an MTU: it changes when the route
                        // does, and a datagram over the limit is refused outright.
                        let room = connection
                            .max_datagram_size()
                            .unwrap_or(1200)
                            .min(1400);
                        let mut sent_all = true;
                        for (parts, part, chunk) in split(&bytes, room) {
                            let mut packet = Vec::with_capacity(room);
                            Packet {
                                window,
                                flags: if keyframe { FLAG_KEYFRAME } else { 0 }
                                    | if part + 1 == parts { FLAG_LAST } else { 0 },
                                frame,
                                viewport,
                                parts,
                                part,
                                captured_us,
                                payload: chunk,
                            }
                            .write(&mut packet);
                            if connection.send_datagram(packet.into()).is_err() {
                                sent_all = false;
                                break;
                            }
                        }
                        if sent_all {
                            frames_sent += 1;
                        } else {
                            // Not fatal and not worth a word each time: a datagram refused
                            // means the congestion controller is full, and the next frame is
                            // more useful than this one anyway.
                            dropped += 1;
                            if dropped.is_power_of_two() {
                                log::info!(
                                    "{dropped} frames dropped before sending ({frames_sent} sent)"
                                );
                            }
                        }
                    }
                }
            }
            // **The head.** The only thing the session sends as a datagram: a viewport is only
            // worth having while it is the newest, so it is sent unreliably and written into the
            // ring the moment it lands. Always read, even with no ring to write, so unwanted
            // datagrams are drained rather than left to fill the receive buffer.
            datagram = connection.read_datagram() => {
                let Ok(datagram) = datagram else { break };
                match spatiand_stream::from_bytes::<ClientMessage>(&datagram) {
                    Ok(ClientMessage::Viewport(viewport)) => {
                        if let (Some(Ok(mut poses)), true) =
                            (poses.map(|poses| poses.lock()), admitted.is(serial))
                        {
                            poses.heard(&viewport, crate::pose::now_ns());
                        }
                    }
                    Ok(_) => {}
                    Err(e) => log::debug!("a datagram from the session did not decode: {e}"),
                }
            }
            _ = connection.closed() => break,
        }
    }
}

/// Read one thing the session opened a stream for.
///
/// Almost everything is a single message — one per stream, which costs nothing in QUIC and
/// means a malformed message can never desynchronise the next one. The exception is sound
/// coming *up* from the headset, which is endless and so is recognised by its magic before
/// anything tries to read it to the end. See `spatiand_stream::audio` and `crate::microphone`.
async fn heard(
    mut stream: quinn::RecvStream,
    serial: u64,
    inbox: Sender<FromSession>,
    admitted: Admission,
    standing: Standing,
) {
    // Enough to tell the two apart, and read in a way that tolerates a message shorter than
    // the magic: `Detach` is a couple of bytes.
    let mut first = Vec::new();
    while first.len() < 8 {
        let mut buffer = [0u8; 8];
        match stream.read(&mut buffer).await {
            Ok(Some(n)) => first.extend_from_slice(&buffer[..n]),
            Ok(None) | Err(_) => break,
        }
    }
    // Everything the session says comes down one ordered stream; see
    // `spatiand_stream::CONTROL_MAGIC` for why it has to be one.
    if first.len() >= 8 && first[..4] == spatiand_stream::CONTROL_MAGIC {
        listen_control(stream, first[4..].to_vec(), serial, inbox, admitted, standing).await;
        return;
    }
    let magic = first.len() >= 8
        && spatiand_stream::audio::AudioHeader::length(&first[..8].try_into().unwrap()).is_some();
    // A microphone is played into this machine's sound only for a session that has been let
    // in: a stranger in the pairing window is heard for its pairing messages and nothing else.
    if magic && !admitted.is(serial) {
        log::warn!("a session not let in tried to send sound; ignored");
        return;
    }
    if !magic {
        match stream.read_to_end(64 * 1024).await {
            Ok(rest) => {
                first.extend_from_slice(&rest);
                match spatiand_stream::from_bytes::<ClientMessage>(&first) {
                    Ok(message) => {
                        heard_from(serial, &standing, message, &inbox);
                    }
                    Err(e) => log::warn!("a session sent something unreadable: {e}"),
                }
            }
            Err(e) => log::warn!("could not read what a session said: {e}"),
        }
        return;
    }
    listen(stream, &first).await;
}

/// Read the session's control stream: length-prefixed messages, in order, until it ends.
///
/// In order is the point. A key's press and its release only mean anything in the sequence
/// they were done in, and this is the only thing that guarantees it — see
/// `spatiand_stream::CONTROL_MAGIC`.
async fn listen_control(
    mut stream: quinn::RecvStream,
    rest: Vec<u8>,
    serial: u64,
    inbox: Sender<FromSession>,
    admitted: Admission,
    standing: Standing,
) {
    /// All a session not let in may say at once: its pairing messages are a few dozen bytes,
    /// and anything longer would be an allocation a stranger chose.
    const LARGEST_UNADMITTED: usize = 64 << 10;
    let mut held = rest;
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        // Everything whole that is already in hand.
        loop {
            if held.len() < 4 {
                break;
            }
            let length = u32::from_le_bytes(held[..4].try_into().unwrap()) as usize;
            // A session let in may say anything up to the largest paste, which comes up this
            // stream like everything else; the limit is the one the session reads the host's
            // messages by. It was a megabyte, and a picture copied on the headset and pasted
            // here ended the stream: nothing the session said after it was heard. A length
            // beyond the limit is still a desynchronised stream, with nothing after it that
            // can be believed.
            let largest = if admitted.is(serial) {
                spatiand_stream::link::MAX_CONTROL
            } else {
                LARGEST_UNADMITTED
            };
            if length > largest {
                log::warn!("a session's control stream said a message was {length} bytes; closing it");
                return;
            }
            if held.len() < 4 + length {
                break;
            }
            match spatiand_stream::from_bytes::<ClientMessage>(&held[4..4 + length]) {
                Ok(message) => {
                    if !heard_from(serial, &standing, message, &inbox) {
                        return;
                    }
                }
                Err(e) => log::warn!("a session sent something unreadable: {e}"),
            }
            held.drain(..4 + length);
        }
        match stream.read(&mut buffer).await {
            Ok(Some(n)) => held.extend_from_slice(&buffer[..n]),
            Ok(None) | Err(_) => return,
        }
    }
}

/// Play the wearer's microphone into the graph until the session stops sending it.
async fn listen(mut stream: quinn::RecvStream, first: &[u8]) {
    let microphone = crate::microphone::shared();
    use spatiand_stream::audio::{AudioHeader, Coding};
    let length = match AudioHeader::length(&first[..8].try_into().unwrap()) {
        Some(length) => length,
        None => return,
    };
    let mut body = first[8..].to_vec();
    while body.len() < length {
        let mut buffer = vec![0u8; length - body.len()];
        match stream.read(&mut buffer).await {
            Ok(Some(n)) => body.extend_from_slice(&buffer[..n]),
            Ok(None) | Err(_) => return,
        }
    }
    let rest = body.split_off(length);
    let Some(header) = AudioHeader::decode(&body) else {
        log::warn!("a session sent a sound stream with an unreadable header");
        return;
    };
    log::info!(
        "microphone: {} at {} Hz x {}",
        header.app,
        header.rate,
        header.channels
    );
    // Opus unless the session says otherwise, in which case what arrives is samples and goes
    // straight in. An end that cannot encode falls back and says so in its own log.
    let mut voice = match header.coding {
        Coding::Pcm => None,
        Coding::Opus => match crate::voice::Decoder::new(header.rate, header.channels) {
            Ok(decoder) => Some((decoder, spatiand_stream::audio::Packets::default())),
            Err(e) => {
                log::warn!("microphone: cannot decode what the session is sending ({e})");
                return;
            }
        },
    };
    let mut put = |bytes: &[u8]| -> bool {
        let Some((decoder, packets)) = voice.as_mut() else {
            if let Ok(mut microphone) = microphone.lock() {
                microphone.feed(bytes, header.rate, header.channels);
            }
            return true;
        };
        packets.feed(bytes);
        loop {
            match packets.next() {
                Ok(None) => return true,
                Err(e) => {
                    log::warn!("microphone: {e}");
                    return false;
                }
                Ok(Some(packet)) => match decoder.decode(&packet) {
                    // One bad packet is a click, not a reason to hang up.
                    Err(e) => log::warn!("microphone: a packet would not decode ({e})"),
                    Ok(pcm) => {
                        if let Ok(mut microphone) = microphone.lock() {
                            microphone.feed(&pcm, header.rate, header.channels);
                        }
                    }
                },
            }
        }
    };
    if !rest.is_empty() {
        put(&rest);
    }
    let mut buffer = vec![0u8; 8192];
    loop {
        match stream.read(&mut buffer).await {
            Ok(Some(n)) => {
                if !put(&buffer[..n]) {
                    break;
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    drop(put);
    if let Ok(mut microphone) = microphone.lock() {
        microphone.quiet();
    }
    log::info!("microphone: the session stopped sending; the device here stays and goes quiet");
}

/// Open a stream for one application's sound and keep writing to it, from a task of its own.
fn sound_stream(
    connection: &Connection,
    app: String,
    channels: u16,
) -> tokio::sync::mpsc::Sender<Vec<u8>> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    let connection = connection.clone();
    tokio::spawn(async move {
        let mut stream = match connection.open_uni().await {
            Ok(stream) => stream,
            Err(e) => {
                log::warn!("could not open a sound stream for {app}: {e}");
                return;
            }
        };
        // Sound goes ahead of anything else queued on the connection: a gap in it is heard, and
        // a late picture is only seen.
        let _ = stream.set_priority(10);
        let header = spatiand_stream::audio::AudioHeader {
            app: app.clone(),
            rate: spatiand_stream::audio::RATE,
            channels,
            // An application's sound stays raw: it is the downlink, which has room, and a
            // codec there would cost delay on every window that makes a noise.
            coding: spatiand_stream::audio::Coding::Pcm,
        };
        if stream.write_all(&header.encode()).await.is_err() {
            return;
        }
        log::info!("sound: sending {app}");
        while let Some(pcm) = rx.recv().await {
            if stream.write_all(&pcm).await.is_err() {
                return;
            }
        }
        let _ = stream.finish();
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nobody_is_in_until_let_in() {
        let admitted = Admission::default();
        assert!(!admitted.is(1));
        assert!(!admitted.is(Admission::NOBODY));
        admitted.admit(1);
        assert!(admitted.is(1));
        admitted.withdraw(1);
        assert!(!admitted.is(1));
    }

    /// The order it went wrong in: a paired session joins, a stranger takes over before the
    /// compositor has read that, and the compositor then lets in "the session here".
    #[test]
    fn letting_in_a_session_that_has_gone_lets_nobody_in() {
        let admitted = Admission::default();
        let (paired, stranger) = (1, 2);
        // The network is already on the stranger when the compositor says yes to the first.
        admitted.admit(paired);
        assert!(!admitted.is(stranger));
        // And hearing that the first left takes nothing from whoever is in by then.
        admitted.admit(stranger);
        admitted.withdraw(paired);
        assert!(admitted.is(stranger));
        assert!(!admitted.is(paired));
    }

    /// The defect: with pairing open, anybody who connected closed the session that was in.
    #[test]
    fn a_stranger_arriving_takes_nothing_from_the_session() {
        let mut seats = Seats::default();
        assert_eq!(seats.arrived(1, true), (Seat::Session, None));
        // Any number of them, one after another, for as long as the window is open.
        assert_eq!(seats.arrived(2, false), (Seat::Waiting, None));
        assert_eq!(seats.arrived(3, false), (Seat::Waiting, Some(2)), "strangers push out only each other");
        assert_eq!(seats.arrived(4, false), (Seat::Waiting, Some(3)));
        assert_eq!(seats.session, Some(1), "and the session is where it was");
    }

    #[test]
    fn a_paired_newcomer_takes_over_at_once_and_leaves_the_one_waiting_alone() {
        let mut seats = Seats::default();
        seats.arrived(1, true);
        seats.arrived(2, false);
        assert_eq!(seats.arrived(3, true), (Seat::Session, Some(1)));
        assert_eq!(seats, Seats { session: Some(3), waiting: Some(2) });
    }

    #[test]
    fn the_one_waiting_takes_over_only_when_it_has_proved_the_code() {
        let mut seats = Seats::default();
        seats.arrived(1, true);
        seats.arrived(2, false);
        assert_eq!(seats.proved(2), Some(Some(1)), "the session gives way now, and not before");
        assert_eq!(seats, Seats { session: Some(2), waiting: None });
        // With nobody in, it has nobody to take over from.
        let mut empty = Seats::default();
        empty.arrived(1, false);
        assert_eq!(empty.proved(1), Some(None));
        assert_eq!(empty, Seats { session: Some(1), waiting: None });
    }

    /// A yes that comes late -- for one that left, or that another stranger pushed out -- must
    /// not seat whoever is waiting by then, and must not unseat the session.
    #[test]
    fn a_proof_for_one_no_longer_waiting_changes_nothing() {
        let mut seats = Seats::default();
        seats.arrived(1, true);
        seats.arrived(2, false);
        seats.arrived(3, false);
        assert_eq!(seats.proved(2), None);
        assert_eq!(seats.proved(1), None, "the session is not waiting either");
        assert_eq!(seats.proved(9), None);
        assert_eq!(seats, Seats { session: Some(1), waiting: Some(3) });
    }

    #[test]
    fn only_the_one_waiting_can_be_turned_away() {
        let mut seats = Seats::default();
        seats.arrived(1, true);
        seats.arrived(2, false);
        assert!(!seats.turned_away(1), "the session is hung up on by other means");
        assert!(!seats.turned_away(7));
        assert!(seats.turned_away(2));
        assert!(!seats.turned_away(2), "once");
        assert_eq!(seats, Seats { session: Some(1), waiting: None });
        // And having gone, it cannot be let in after all.
        assert_eq!(seats.proved(2), None);
    }

    #[test]
    fn a_connection_that_ends_leaves_its_own_seat_and_no_other() {
        let mut seats = Seats::default();
        seats.arrived(1, true);
        seats.arrived(2, false);
        assert_eq!(seats.ended(2), Some(Seat::Waiting));
        assert_eq!(seats.ended(2), None);
        seats.arrived(3, true);
        // The one taken over ends a moment later: that is not the session leaving.
        assert_eq!(seats.ended(1), None);
        assert_eq!(seats.ended(3), Some(Seat::Session));
        assert_eq!(seats, Seats::default());
    }

    fn hello() -> ClientMessage {
        ClientMessage::Hello {
            version: spatiand_stream::VERSION,
            codecs: Vec::new(),
            max_size: (1920, 1080),
            refresh_mhz: 60_000,
            session: String::new(),
        }
    }

    fn welcome() -> HostMessage {
        HostMessage::Welcome { version: spatiand_stream::VERSION, host: "workshop".into(), reattached: false }
    }

    fn refused() -> HostMessage {
        HostMessage::Refused { version: spatiand_stream::VERSION, reason: "no".into() }
    }

    #[test]
    fn one_waiting_is_heard_for_its_pairing_and_nothing_else() {
        assert!(pairing_message(&ClientMessage::PairStart { spake: vec![1] }));
        assert!(pairing_message(&ClientMessage::PairConfirm { confirm: vec![1] }));
        assert!(pairing_message(&hello()), "to be told to update");
        assert!(!pairing_message(&ClientMessage::Detach));
        assert!(!pairing_message(&ClientMessage::Launch { app: "anything".into() }));
        assert!(!pairing_message(&ClientMessage::ForceQuit { app: "anything".into() }));
        assert!(!pairing_message(&ClientMessage::Focus { window: None }));
    }

    #[test]
    fn one_waiting_is_told_the_pairing_reply_or_no_and_nothing_else() {
        assert!(for_the_unproven(&HostMessage::PairReply { spake: vec![1], confirm: vec![2] }));
        assert!(for_the_unproven(&refused()));
        assert!(!for_the_unproven(&welcome()));
        assert!(!for_the_unproven(&HostMessage::Catalog { apps: Vec::new() }));
        assert!(!for_the_unproven(&HostMessage::Microphone { wanted: true }));
    }

    #[test]
    fn what_one_waiting_says_reaches_the_compositor_only_if_it_is_pairing() {
        let (inbox, heard) = std::sync::mpsc::channel();
        let standing = Standing::new(false);
        assert!(heard_from(5, &standing, ClientMessage::Launch { app: "anything".into() }, &inbox));
        assert!(heard_from(5, &standing, ClientMessage::Detach, &inbox));
        assert!(heard.try_recv().is_err(), "nothing was passed on");
        assert!(heard_from(5, &standing, ClientMessage::PairStart { spake: vec![1] }, &inbox));
        assert!(matches!(
            heard.try_recv(),
            Ok(FromSession::Said { session: 5, message: ClientMessage::PairStart { .. } })
        ));
        // Once it is the session, what it says is a session's.
        standing.now_the_session();
        assert!(heard_from(5, &standing, ClientMessage::Detach, &inbox));
        assert!(matches!(
            heard.try_recv(),
            Ok(FromSession::Said { session: 5, message: ClientMessage::Detach })
        ));
    }

    #[test]
    fn emptying_the_queue_for_a_new_session_keeps_what_is_addressed_by_number() {
        let (out, mut outbox) = unbounded_channel();
        let _ = out.send(ToSession::Control(welcome()));
        let _ = out.send(ToSession::Stranger { session: 2, message: refused() });
        let _ = out.send(ToSession::Close);
        let _ = out.send(ToSession::TurnAway { session: 2 });
        let _ = out.send(ToSession::Proved { session: 3 });
        let mut backlog = VecDeque::new();
        stale(&mut outbox, &mut backlog);
        assert!(outbox.try_recv().is_err());
        assert!(matches!(
            backlog.make_contiguous(),
            [
                ToSession::Stranger { session: 2, .. },
                ToSession::TurnAway { session: 2 },
                ToSession::Proved { session: 3 }
            ]
        ));
    }

    /// The network thread itself, over the loopback: a host with one paired device and
    /// pairing open, with this test standing where the compositor does.
    struct Rig {
        runtime: tokio::runtime::Runtime,
        address: SocketAddr,
        host: Identity,
        out: UnboundedSender<ToSession>,
        heard: Receiver<FromSession>,
        admitted: Admission,
        paired: Identity,
    }

    impl Rig {
        fn start() -> Rig {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a runtime");
            let host = Identity::new().unwrap();
            let paired = Identity::new().unwrap();
            let gate = Arc::new(spatiand_stream::Gate::new(vec![paired.fingerprint()]));
            gate.set_open(true);
            let endpoint = {
                let _guard = runtime.enter();
                spatiand_stream::transport::server("127.0.0.1:0".parse().unwrap(), &host, Trust::Gate(gate))
                    .unwrap()
            };
            let address = endpoint.local_addr().unwrap();
            let (out, outbox) = unbounded_channel();
            let (inbox, heard) = std::sync::mpsc::channel();
            let admitted = Admission::default();
            let known = Known::default();
            known.set(vec![paired.fingerprint()]);
            runtime.spawn(serve(endpoint, outbox, inbox, None, admitted.clone(), known));
            Rig { runtime, address, host, out, heard, admitted, paired }
        }

        fn next(&self) -> FromSession {
            self.heard
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("the network thread said something")
        }

        fn quiet(&self) -> bool {
            self.heard.recv_timeout(std::time::Duration::from_millis(300)).is_err()
        }

        /// A device connects, and has its end of the connection.
        fn connect(&self, endpoint: &quinn::Endpoint) -> Connection {
            self.runtime.block_on(async {
                endpoint.connect(self.address, "spatiand").unwrap().await.expect("connects")
            })
        }

        /// The paired device comes in and is let in, as the compositor would.
        fn paired_session(&self) -> (quinn::Endpoint, Connection, u64) {
            let endpoint = {
                let _guard = self.runtime.enter();
                spatiand_stream::transport::client(&self.paired, self.host.fingerprint()).unwrap()
            };
            let connection = self.connect(&endpoint);
            let FromSession::Joined { session, who, .. } = self.next() else { panic!("not a joining") };
            assert_eq!(who, self.paired.fingerprint());
            self.admitted.admit(session);
            (endpoint, connection, session)
        }

        fn stranger(&self) -> (quinn::Endpoint, Connection, u64) {
            let identity = Identity::new().unwrap();
            let endpoint = {
                let _guard = self.runtime.enter();
                spatiand_stream::transport::client_first_contact(&identity).unwrap()
            };
            let connection = self.connect(&endpoint);
            let FromSession::Knocked { session, who, .. } = self.next() else { panic!("not a knock") };
            assert_eq!(who, identity.fingerprint());
            (endpoint, connection, session)
        }

        fn say(&self, connection: &Connection, message: ClientMessage) {
            self.runtime
                .block_on(spatiand_stream::link::say(connection, &message))
                .expect("said");
        }

        /// The host's control stream as that device sees it, once something is on it.
        fn control(&self, connection: &Connection) -> Option<quinn::RecvStream> {
            self.runtime.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(5), connection.accept_uni())
                    .await
                    .ok()?
                    .ok()
            })
        }

        fn hear(&self, stream: quinn::RecvStream) -> Option<(HostMessage, quinn::RecvStream)> {
            self.runtime.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(5), spatiand_stream::link::hear(stream))
                    .await
                    .ok()?
            })
        }

        fn closed_with(&self, connection: &Connection) -> Option<u32> {
            let reason = self.runtime.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(5), connection.closed()).await.ok()
            })?;
            match reason {
                quinn::ConnectionError::ApplicationClosed(closed) => {
                    Some(u64::from(closed.error_code) as u32)
                }
                other => panic!("closed another way: {other}"),
            }
        }
    }

    #[test]
    fn over_the_loopback_a_stranger_is_held_beside_the_session_and_hears_only_its_pairing() {
        let rig = Rig::start();
        let (_paired_endpoint, paired, session) = rig.paired_session();
        let (_stranger_endpoint, stranger, waiting) = rig.stranger();
        assert_ne!(session, waiting);
        assert!(rig.quiet(), "the session was not said to have left");
        assert!(paired.close_reason().is_none(), "and it has not been closed");

        // What the compositor posts for the session goes to the session.
        let _ = rig.out.send(ToSession::Control(welcome()));
        let control = rig.control(&paired).expect("the session's control stream");
        let (told, _control) = rig.hear(control).expect("the session is told");
        assert_eq!(told, welcome());

        // What the stranger says: its pairing gets through, by its number, and nothing else.
        rig.say(&stranger, ClientMessage::Launch { app: "anything".into() });
        rig.say(&stranger, ClientMessage::PairStart { spake: vec![7; 33] });
        match rig.next() {
            FromSession::Said { session: from, message: ClientMessage::PairStart { .. } } => {
                assert_eq!(from, waiting);
            }
            _ => panic!("something other than its pairing was passed on"),
        }
        assert!(rig.quiet());

        // What it is told: a welcome addressed to it is not sent; its reply is, and is the
        // first thing on its control stream.
        let reply = HostMessage::PairReply { spake: vec![1; 33], confirm: vec![2; 32] };
        let _ = rig.out.send(ToSession::Stranger { session: waiting, message: welcome() });
        let _ = rig.out.send(ToSession::Stranger { session: waiting, message: reply.clone() });
        let control = rig.control(&stranger).expect("the stranger's control stream");
        let (told, _control) = rig.hear(control).expect("the stranger is told");
        assert_eq!(told, reply);

        // Turned away, it goes alone.
        let _ = rig.out.send(ToSession::TurnAway { session: waiting });
        assert!(matches!(rig.next(), FromSession::Left { session: gone } if gone == waiting));
        assert_eq!(rig.closed_with(&stranger), Some(1));
        assert!(rig.quiet());
        assert!(paired.close_reason().is_none(), "the session is still in");
    }

    #[test]
    fn over_the_loopback_a_stranger_takes_over_when_it_is_proved_and_not_before() {
        let rig = Rig::start();
        let (_paired_endpoint, paired, session) = rig.paired_session();
        let (_stranger_endpoint, stranger, waiting) = rig.stranger();
        // Its reply goes down the control stream it will be welcomed on.
        let reply = HostMessage::PairReply { spake: vec![1; 33], confirm: vec![2; 32] };
        let _ = rig.out.send(ToSession::Stranger { session: waiting, message: reply.clone() });
        let control = rig.control(&stranger).expect("the stranger's control stream");
        let (told, control) = rig.hear(control).expect("the stranger is told");
        assert_eq!(told, reply);
        assert!(paired.close_reason().is_none());

        let _ = rig.out.send(ToSession::Proved { session: waiting });
        assert!(matches!(rig.next(), FromSession::Left { session: gone } if gone == session));
        assert!(matches!(rig.next(), FromSession::Joined { session: now, .. } if now == waiting));
        assert_eq!(rig.closed_with(&paired), Some(CLOSE_TAKEN_OVER));

        // Let in, it is welcomed on the same stream, and heard as a session.
        rig.admitted.admit(waiting);
        let _ = rig.out.send(ToSession::Control(welcome()));
        let (told, _control) = rig.hear(control).expect("the new session is told");
        assert_eq!(told, welcome());
        rig.say(&stranger, ClientMessage::Detach);
        assert!(matches!(
            rig.next(),
            FromSession::Said { session: from, message: ClientMessage::Detach } if from == waiting
        ));
    }

    #[test]
    fn over_the_loopback_a_paired_device_takes_over_at_once() {
        let rig = Rig::start();
        let (_first_endpoint, first, session) = rig.paired_session();
        let endpoint = {
            let _guard = rig.runtime.enter();
            spatiand_stream::transport::client(&rig.paired, rig.host.fingerprint()).unwrap()
        };
        let _second = rig.connect(&endpoint);
        assert!(matches!(rig.next(), FromSession::Left { session: gone } if gone == session));
        assert!(matches!(rig.next(), FromSession::Joined { .. }));
        assert_eq!(rig.closed_with(&first), Some(CLOSE_TAKEN_OVER));
    }

    /// A device as it really pairs -- `spatiand_stream::link::pair`, the one every device
    /// runs -- against the network thread, with this test answering as the compositor does:
    /// the exchange by number, `Proved` on the right code, and the welcome once it has joined.
    #[test]
    fn over_the_loopback_a_device_pairs_as_devices_do_and_only_then_takes_over() {
        use spatiand_stream::pairing::{Verdict, Window};
        let rig = Rig::start();
        let (_paired_endpoint, paired, session) = rig.paired_session();
        let pairing = rig.runtime.spawn({
            let (device, address) = (Identity::new().unwrap(), rig.address.to_string());
            let fingerprint = device.fingerprint();
            async move {
                (fingerprint, spatiand_stream::link::pair(&device, &address, "482 913", |_| {}).await)
            }
        });
        let mut window = Window::with_code("482913".into(), std::time::Duration::from_secs(120));
        let mut waiting = None;
        loop {
            match rig.next() {
                FromSession::Knocked { session, who, .. } => waiting = Some((session, who)),
                FromSession::Said { session: from, message } => {
                    let (number, who) = waiting.expect("it knocked first");
                    assert_eq!(from, number);
                    match message {
                        ClientMessage::PairStart { spake } => {
                            assert!(paired.close_reason().is_none(), "nothing is taken over for asking");
                            let reply = window.start(rig.host.fingerprint(), who, &spake).expect("a reply");
                            let _ = rig.out.send(ToSession::Stranger { session: number, message: reply });
                        }
                        ClientMessage::PairConfirm { confirm } => {
                            assert!(paired.close_reason().is_none(), "nor before the proof is checked");
                            assert_eq!(window.confirm(who, &confirm), Verdict::Paired(who));
                            let _ = rig.out.send(ToSession::Proved { session: number });
                        }
                        _ => panic!("one waiting was heard for something else"),
                    }
                }
                FromSession::Left { session: gone } => assert_eq!(gone, session),
                FromSession::Joined { session: now, who, .. } => {
                    assert_eq!(Some((now, who)), waiting);
                    rig.admitted.admit(now);
                    let _ = rig.out.send(ToSession::Control(welcome()));
                    break;
                }
            }
        }
        let (fingerprint, outcome) = rig.runtime.block_on(pairing).expect("ran");
        assert_eq!(waiting.map(|(_, who)| who), Some(fingerprint));
        let outcome = outcome.expect("paired");
        assert_eq!(outcome.fingerprint, rig.host.fingerprint());
        assert_eq!(outcome.name, "workshop");
        assert_eq!(rig.closed_with(&paired), Some(CLOSE_TAKEN_OVER));
    }

    /// And with the wrong code: refused, hung up on, and the session never knew.
    #[test]
    fn over_the_loopback_a_wrong_code_is_refused_and_the_session_stays() {
        use spatiand_stream::pairing::{Window, WRONG_CODE};
        let rig = Rig::start();
        let (_paired_endpoint, paired, _session) = rig.paired_session();
        let pairing = rig.runtime.spawn({
            let (device, address) = (Identity::new().unwrap(), rig.address.to_string());
            async move { spatiand_stream::link::pair(&device, &address, "111111", |_| {}).await }
        });
        let mut window = Window::with_code("482913".into(), std::time::Duration::from_secs(120));
        let FromSession::Knocked { session: number, who, .. } = rig.next() else { panic!("not a knock") };
        let FromSession::Said { message: ClientMessage::PairStart { spake }, .. } = rig.next() else {
            panic!("not the start of a pairing")
        };
        let reply = window.start(rig.host.fingerprint(), who, &spake).expect("a reply");
        let _ = rig.out.send(ToSession::Stranger { session: number, message: reply });
        // The device sees the host's confirmation fail and hangs up itself, saying no more.
        let error = rig.runtime.block_on(pairing).expect("ran").unwrap_err();
        assert!(error.contains(WRONG_CODE), "{error}");
        assert!(matches!(rig.next(), FromSession::Left { session: gone } if gone == number));
        assert!(window.abandon(&who).is_some(), "and its attempt is spent");
        assert!(rig.quiet());
        assert!(paired.close_reason().is_none(), "the session is still in");
    }
}
