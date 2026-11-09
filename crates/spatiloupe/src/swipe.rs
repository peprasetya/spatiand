//! Going from one picture to the next by hand, as a phone's photographs do.
//!
//! In the fitted view a picture moves sideways under the hand, one to one -- two fingers on a
//! trackpad, a thumb on a pad or on the Beam Pro's touch area, or the button held and dragged --
//! and the next picture or the one before comes in beside it. Let go, it settles: on that
//! neighbour if it was a quarter of the way there or was thrown towards it, back in its place if
//! not. Past the folder's first and last pictures it gives, as a band does, and comes back.
//!
//! **Nothing says when a finger lifts.** A scroll is only how far it went. Spatiand's Wayland says
//! `axis_stop` from a pad only when the thumb ran off its rim (`spatiand_input::scroll`), a Mac
//! never says it, and `spatiand_app` passes neither on. So a scroll is over when it stops coming:
//! soon when it was going briskly ([`LETS_GO_MOVING`]), which is a hand that lifted, and later
//! when it was creeping ([`LETS_GO_RESTING`]), which may be a thumb resting on the picture. A held
//! button is the one hand that says when it lets go.
//!
//! The carousel is in pages, each the window's width and a gap: the file being looked at is on
//! page 0, the next on page 1, the one before on -1. This is its state alone, in logical pixels
//! and instants, so that it is tested without a window; [`crate::view`] feeds it and draws it.

use std::time::{Duration, Instant};

/// How far a scroll goes, either way, before it is decided whether it slides or zooms. A wheel's
/// notch is past it at once; a trackpad's first wobbles are not, and count for whichever way the
/// scroll then goes.
const DECIDE_AFTER: f32 = 8.0;

/// How fast is brisk, logical pixels a millisecond: a hand going at least this fast and then not
/// heard from has most likely lifted; one going slower may be resting.
const BRISK: f32 = 0.3;

/// How long a brisk scroll may stop before it is over: two of a pad's frames, which send one each
/// while the thumb moves, and a little. Shorter than the 80 ms and more a Mac waits once its
/// fingers lift before it glides the scroll on by itself, which is then let go by ([`Glide`]).
const LETS_GO_MOVING: Duration = Duration::from_millis(45);

/// How long a creeping scroll may stop before it is over: long enough for a thumb resting on the
/// picture to keep it there a moment, and no longer than a slow lift should wait.
const LETS_GO_RESTING: Duration = Duration::from_millis(120);

/// A brisk scroll not heard from for more than a frame coasts on at its speed, as the hand most
/// likely did: a pad that skips a frame's movement loses it, and a hand that lifted is not shown
/// stopping dead before the picture settles.
const COASTS_AFTER: Duration = Duration::from_millis(20);

/// How far towards a neighbour, as a part of the page, a picture let go at rest goes on to it.
const ONWARD: f32 = 0.25;

/// How fast the hand must be going as it lets go to throw the picture on to the page it is going
/// towards, however little of the way it has got: logical pixels a millisecond. A thumb flicked
/// across the Beam Pro's touch area goes at one and a half to two; one placing the picture with
/// care, at under a half.
const THROWN: f32 = 0.6;

/// How far a throw must have gone, so that a twitch is not one.
const THROWN_AFTER: f32 = 20.0;

/// The hand's speed is how far it went over about this long: its last moments, not the whole way.
const SPEED_OVER: Duration = Duration::from_millis(100);

/// The least time a movement is taken to have taken: two scrolls handled together came a frame
/// apart, not at the same moment.
const FRAME: Duration = Duration::from_millis(16);

/// A drag held still this long before its button is let go is let go at rest, not thrown.
const HELD_STILL: Duration = Duration::from_millis(60);

/// Past an edge the picture follows the hand by this much at first and less and less after,
/// never going more than [`GIVE_MOST`] of the page past it.
const GIVE: f32 = 0.5;
const GIVE_MOST: f32 = 0.3;

/// After a throw, scrolls the same way that begin within this long of its last are its glide --
/// a Mac's, which carries a scroll on after the fingers lift -- and are let go by: the picture is
/// already going there, and taken for the hand they would carry it a page further. A new swipe
/// cannot come so soon, as the fingers have to lift and land again first.
const GLIDE_AFTER: Duration = Duration::from_millis(150);

/// A glide comes a frame at a time: one not heard from for this long has stopped.
const GLIDE_GAP: Duration = Duration::from_millis(60);

/// A glide only slows: a scroll more than this many times its slowest yet is a hand pushing again.
const GLIDE_SPEEDS_UP: f32 = 2.0;

/// How far either side of its page the carousel is moving towards a neighbour, as a part of the
/// page, for that neighbour to be worth reading ahead.
const HEADING: f32 = 0.125;

/// How many pages there are either side of the one being looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reach {
    pub before: usize,
    pub after: usize,
}

/// What became of a scroll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scrolled {
    /// The carousel's: it slid the picture, is still deciding which way the scroll goes, or let a
    /// glide go by.
    Taken,
    /// Up or down, with the picture in its place: zoom by this, as a wheel's `dy`.
    Zoom(f32),
}

/// What a throw may be followed by: see [`GLIDE_AFTER`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glide {
    /// Which way its scrolls go: the sign of their `dx`.
    way: f32,
    last: Instant,
    /// Its slowest scroll so far, once it has begun.
    slowest: Option<f32>,
}

/// The picture easing from where the hand left it to rest on a page.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settle {
    from: f32,
    /// The page it comes to rest on: 0 is back in its place.
    to: i32,
    /// How fast it was going as it began, logical pixels a millisecond.
    speed: f32,
    since: Instant,
    length: Duration,
}

/// How long settling takes: a fifth of a second, and more the further it goes, to three tenths
/// for a whole page.
fn settle_length(distance: f32, page: f32) -> Duration {
    Duration::from_secs_f32(0.2 + 0.1 * (distance.abs() / page).min(1.0))
}

impl Settle {
    /// From `from`, going at `speed`, to rest on page `to`.
    fn new(from: f32, to: i32, speed: f32, since: Instant, page: f32) -> Settle {
        let length = settle_length(-(to as f32) * page - from, page);
        Settle { from, to, speed, since, length }
    }

    /// The same for a key: off at once, as though pushed, rather than easing away from rest.
    fn eager(from: f32, to: i32, speed: f32, since: Instant, page: f32) -> Settle {
        let mut settle = Settle::new(from, to, speed, since, page);
        let distance = -(to as f32) * page - from;
        let push = 1.5 * distance / (settle.length.as_secs_f32() * 1000.0);
        if speed * distance.signum() < push.abs() {
            settle.speed = push;
        }
        settle
    }

    /// Where it is at `now`, and how fast it is going there.
    ///
    /// A cubic from where it began, at the speed it began with, to rest on its page: the hand's
    /// own speed carried on, so that a throw does not stop and start again. Begun no faster than
    /// three times its distance over its length -- the ease out a cubic can be without passing its
    /// page and coming back -- and never begun backwards, which it would come back from too.
    fn at(&self, now: Instant, page: f32) -> (f32, f32) {
        let to = -(self.to as f32) * page;
        let length = self.length.as_secs_f32() * 1000.0;
        let s = (now.saturating_duration_since(self.since).as_secs_f32() * 1000.0 / length).clamp(0.0, 1.0);
        let distance = to - self.from;
        let slope = if self.speed * distance > 0.0 { (self.speed * length).abs().min(3.0 * distance.abs()) * distance.signum() } else { 0.0 };
        let (rise, lean) = (s * s * (3.0 - 2.0 * s), s * (1.0 - s) * (1.0 - s));
        let (rising, leaning) = (6.0 * s * (1.0 - s), (1.0 - s) * (1.0 - 3.0 * s));
        (self.from + distance * rise + slope * lean, (distance * rising + slope * leaning) / length)
    }

    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.since) >= self.length
    }
}

/// A hand along the carousel.
#[derive(Debug, Clone, PartialEq)]
struct Slide {
    /// A held button's drag, which ends when the button is let go; otherwise a scroll's, which
    /// ends when it stops coming.
    held: bool,
    /// Where the hand has taken the picture, before an edge holds it back.
    raw: f32,
    /// Where it took hold, to tell a twitch from a throw, and which way it has gone.
    began: f32,
    /// The page it took hold on: it goes no further than the pages either side.
    home: i32,
    /// Where the hand was, and when, over its last moments.
    track: Vec<(Instant, f32)>,
    /// How fast it was going at its last movement, logical pixels a millisecond, + right.
    speed: f32,
    last: Instant,
}

impl Slide {
    /// How far a brisk scroll has coasted on since it was last heard from: see [`COASTS_AFTER`].
    fn coast(&self, now: Instant) -> f32 {
        if self.held || self.speed.abs() < BRISK {
            return 0.0;
        }
        let quiet = now.saturating_duration_since(self.last).saturating_sub(COASTS_AFTER);
        self.speed * quiet.min(LETS_GO_MOVING - COASTS_AFTER).as_secs_f32() * 1000.0
    }

    /// When a scroll that is not heard from again is over. A drag is over when it is let go.
    fn lets_go_at(&self) -> Option<Instant> {
        if self.held {
            return None;
        }
        Some(self.last + if self.speed.abs() >= BRISK { LETS_GO_MOVING } else { LETS_GO_RESTING })
    }

    /// The hand moved by `by` at `now`.
    fn moved(&mut self, by: f32, now: Instant) {
        self.raw += self.coast(now) + by;
        self.last = now;
        self.track.push((now, self.raw));
        // The oldest kept is the last from before the measure began: its last moments, all of them.
        while self.track.len() > 2 && self.track[1].0 + SPEED_OVER <= now {
            self.track.remove(0);
        }
        self.speed = speed(&self.track);
    }
}

/// How fast the hand went over what is kept of its way, logical pixels a millisecond.
fn speed(track: &[(Instant, f32)]) -> f32 {
    match (track.first(), track.last()) {
        (Some(&(then, from)), Some(&(now, to))) if track.len() >= 2 => (to - from) / (now.saturating_duration_since(then).max(FRAME).as_secs_f32() * 1000.0),
        _ => 0.0,
    }
}

/// How far past an edge the picture goes for the hand going `over` past it.
fn give(over: f32, page: f32) -> f32 {
    let most = GIVE_MOST * page;
    most * (1.0 - 1.0 / (GIVE * over / most + 1.0))
}

/// How far past an edge the hand must have gone for the picture to be `given` past it.
fn taken(given: f32, page: f32) -> f32 {
    let most = GIVE_MOST * page;
    let given = given.min(most * 0.999);
    most / GIVE * given / (most - given)
}

/// The page a picture let go `at` pages along comes to rest on. `way` is which way the hand had
/// taken it, + towards the next; `speed`, how fast it was going as it let go, in pages a
/// millisecond, + the same way. A throw goes on to the next page the way it was going; a picture
/// let go at rest goes on if it was a quarter past a page the way the hand went, and back if not.
fn choose(at: f32, way: f32, thrown: bool, speed: f32) -> i32 {
    if thrown {
        return if speed > 0.0 { at.floor() as i32 + 1 } else { at.ceil() as i32 - 1 };
    }
    if way > 0.0 {
        at.floor() as i32 + (at - at.floor() > ONWARD) as i32
    } else if way < 0.0 {
        at.ceil() as i32 - (at.ceil() - at > ONWARD) as i32
    } else {
        at.round() as i32
    }
}

/// What the hand is doing.
#[derive(Debug, Clone, PartialEq)]
enum Hand {
    /// Nothing that moves the carousel.
    Idle,
    /// A scroll that has gone so far either way, not yet decided.
    Deciding { dx: f32, dy: f32, last: Instant },
    /// A scroll up or down, which zooms, and stays a zoom until it stops.
    Zooming { last: Instant },
    Sliding(Slide),
}

/// The file being looked at, its neighbours beside it, and the hand moving between them.
#[derive(Debug, Clone)]
pub struct Carousel {
    reach: Reach,
    /// Where the picture is, slid from its place, logical pixels, + right: as of the last movement
    /// or [`Carousel::tick`].
    offset: f32,
    hand: Hand,
    settle: Option<Settle>,
    glide: Option<Glide>,
}

impl Carousel {
    pub fn new(reach: Reach) -> Carousel {
        Carousel { reach, offset: 0.0, hand: Hand::Idle, settle: None, glide: None }
    }

    /// Where the picture is, slid from its place: what is drawn.
    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// The picture is in its place, and neither a hand nor a settle is moving it.
    pub fn resting(&self) -> bool {
        self.settle.is_none() && !matches!(self.hand, Hand::Sliding(_)) && self.offset == 0.0
    }

    /// A hand is at it -- a scroll being decided, a zoom, a slide -- and what comes next of the
    /// scroll is the carousel's.
    pub fn busy(&self) -> bool {
        !matches!(self.hand, Hand::Idle)
    }

    /// When something happens if nothing more comes from the hand: a scroll is over.
    pub fn deadline(&self) -> Option<Instant> {
        match &self.hand {
            Hand::Idle => None,
            Hand::Deciding { last, .. } | Hand::Zooming { last } => Some(*last + LETS_GO_RESTING),
            Hand::Sliding(slide) => slide.lets_go_at(),
        }
    }

    /// The page a settle is taking the picture to.
    pub fn settling_to(&self) -> Option<i32> {
        self.settle.map(|s| s.to)
    }

    /// The page the picture is going to, or the neighbour it is being taken towards: what is
    /// worth reading ahead.
    pub fn heading(&self, page: f32) -> Option<i32> {
        if let Some(settle) = self.settle {
            return (settle.to != 0).then_some(settle.to);
        }
        if !matches!(self.hand, Hand::Sliding(_)) {
            return None;
        }
        let at = -self.offset / page.max(1.0);
        let to = if at > HEADING {
            at.ceil() as i32
        } else if at < -HEADING {
            at.floor() as i32
        } else {
            return None;
        };
        Some(self.clamp_page(to)).filter(|&to| to != 0)
    }

    /// The glide a throw may still be followed by: handed on to the view that comes after this
    /// one, as a glide outlasts the settle it follows.
    pub fn glide(&self) -> Option<Glide> {
        self.glide
    }

    pub fn carry(&mut self, glide: Option<Glide>) {
        self.glide = glide;
    }

    fn clamp_page(&self, page: i32) -> i32 {
        page.clamp(-(self.reach.before as i32), self.reach.after as i32)
    }

    /// The pages a hand that took hold on `home` may take the picture to: one either side, and
    /// none past the folder's ends.
    fn range(&self, home: i32) -> (i32, i32) {
        ((home - 1).max(-(self.reach.before as i32)), (home + 1).min(self.reach.after as i32))
    }

    /// Where the hand's `raw` puts the picture: where it says between those pages, held back past
    /// them.
    fn banded(&self, home: i32, raw: f32, page: f32) -> f32 {
        let (first, last) = self.range(home);
        let (least, most) = (-(last as f32) * page, -(first as f32) * page);
        if raw > most {
            most + give(raw - most, page)
        } else if raw < least {
            least - give(least - raw, page)
        } else {
            raw
        }
    }

    /// Where the hand would have to be for the picture to be at `offset`: [`Carousel::banded`]
    /// backwards, for a hand taking hold of a picture that is past an edge.
    fn unbanded(&self, home: i32, offset: f32, page: f32) -> f32 {
        let (first, last) = self.range(home);
        let (least, most) = (-(last as f32) * page, -(first as f32) * page);
        if offset > most {
            most + taken(offset - most, page)
        } else if offset < least {
            least - taken(least - offset, page)
        } else {
            offset
        }
    }

    /// A scroll: across slides the picture, up and down zooms it, and which a scroll does is
    /// decided by its first movement and kept until it stops.
    pub fn scroll(&mut self, dx: f32, dy: f32, now: Instant, page: f32) -> Scrolled {
        let page = page.max(1.0);
        if self.gliding(dx, dy, now) {
            return Scrolled::Taken;
        }
        match &mut self.hand {
            // A button held has the carousel; a scroll meanwhile is nothing.
            Hand::Sliding(slide) if slide.held => Scrolled::Taken,
            Hand::Sliding(_) => {
                self.moved(-dx, now, page);
                Scrolled::Taken
            }
            Hand::Zooming { last } => {
                *last = now;
                self.zoom(dy)
            }
            Hand::Deciding { dx: sx, dy: sy, last } => {
                *sx += dx;
                *sy += dy;
                *last = now;
                self.decide(now, page)
            }
            Hand::Idle => {
                self.hand = Hand::Deciding { dx, dy, last: now };
                self.decide(now, page)
            }
        }
    }

    /// Whether a scroll has gone far enough to say which way it goes, and if so, it does.
    fn decide(&mut self, now: Instant, page: f32) -> Scrolled {
        let Hand::Deciding { dx, dy, .. } = self.hand else { return Scrolled::Taken };
        if dx.abs().max(dy.abs()) < DECIDE_AFTER {
            return Scrolled::Taken;
        }
        if dx.abs() > dy.abs() {
            self.take_hold(false, now, page);
            self.moved(-dx, now, page);
            Scrolled::Taken
        } else {
            self.hand = Hand::Zooming { last: now };
            self.zoom(dy)
        }
    }

    /// A zoom is for the picture in its place: one on its way somewhere is not zoomed.
    fn zoom(&self, dy: f32) -> Scrolled {
        if self.settle.is_none() && self.offset == 0.0 {
            Scrolled::Zoom(dy)
        } else {
            Scrolled::Taken
        }
    }

    /// Whether a scroll is the glide after a throw, which is let go by: see [`GLIDE_AFTER`].
    fn gliding(&mut self, dx: f32, dy: f32, now: Instant) -> bool {
        let Some(glide) = self.glide.as_mut() else { return false };
        let quiet = now.saturating_duration_since(glide.last);
        let soon = quiet <= if glide.slowest.is_some() { GLIDE_GAP } else { GLIDE_AFTER };
        let along = dx.abs() >= dy.abs() && dx * glide.way > 0.0;
        let slowing = glide.slowest.map_or(true, |slowest| dx.abs() <= slowest * GLIDE_SPEEDS_UP);
        if soon && along && slowing {
            glide.last = now;
            glide.slowest = Some(glide.slowest.map_or(dx.abs(), |slowest| slowest.min(dx.abs())));
            true
        } else {
            self.glide = None;
            false
        }
    }

    /// The hand takes the picture where it is: in its place, or wherever a settle had got it to.
    fn take_hold(&mut self, held: bool, now: Instant, page: f32) {
        if let Some(settle) = self.settle.take() {
            self.offset = settle.at(now, page).0;
        }
        self.glide = None;
        let home = self.clamp_page((-self.offset / page).round() as i32);
        let raw = self.unbanded(home, self.offset, page);
        self.hand = Hand::Sliding(Slide { held, raw, began: raw, home, track: Vec::new(), speed: 0.0, last: now });
    }

    fn moved(&mut self, by: f32, now: Instant, page: f32) {
        let Hand::Sliding(slide) = &mut self.hand else { return };
        slide.moved(by, now);
        let (home, raw) = (slide.home, slide.raw);
        self.offset = self.banded(home, raw, page);
    }

    /// The button went down and the pointer has gone far enough, mostly sideways, to be a drag.
    pub fn grab(&mut self, now: Instant, page: f32) {
        self.take_hold(true, now, page.max(1.0));
    }

    /// The pointer moved `dx` with the button held, the picture with it.
    pub fn drag(&mut self, dx: f32, now: Instant, page: f32) {
        if matches!(self.hand, Hand::Sliding(Slide { held: true, .. })) {
            self.moved(dx, now, page.max(1.0));
        }
    }

    /// The button was let go.
    pub fn release(&mut self, now: Instant, page: f32) {
        if matches!(self.hand, Hand::Sliding(Slide { held: true, .. })) {
            self.let_go(now, page.max(1.0));
        }
    }

    /// The hand lets go `at` that moment, and the picture settles from there on the page it was
    /// nearer or thrown towards. A scroll's moment is when it was due to be over, which the tick
    /// that finds it so comes a little after: the settle is begun then, not at the tick, so that
    /// a coasting picture does not wait for it.
    fn let_go(&mut self, at: Instant, page: f32) {
        let Hand::Sliding(slide) = std::mem::replace(&mut self.hand, Hand::Idle) else { return };
        let raw = slide.raw + slide.coast(at);
        self.offset = self.banded(slide.home, raw, page);
        let still = slide.held && at.saturating_duration_since(slide.last) >= HELD_STILL;
        let speed = if still { 0.0 } else { slide.speed };
        let thrown = speed.abs() >= THROWN && (raw - slide.began).abs() >= THROWN_AFTER;
        let (first, last) = self.range(slide.home);
        let to = choose(-self.offset / page, slide.began - raw, thrown, -speed / page).clamp(first, last);
        self.settle = Some(Settle::new(self.offset, to, speed, at, page));
        if thrown && !slide.held {
            self.glide = Some(Glide { way: -speed.signum(), last: slide.last, slowest: None });
        }
    }

    /// Go on `steps` pages from the one the picture is going to, as a key does: a key pressed
    /// again before it gets there goes one further. Says whether it is going anywhere.
    pub fn slide(&mut self, steps: i32, now: Instant, page: f32) -> bool {
        let page = page.max(1.0);
        let going = match self.settle {
            Some(settle) => settle.to,
            None => self.clamp_page((-self.offset / page).round() as i32),
        };
        steps != 0 && self.go_to(going + steps, now, page)
    }

    /// Go to page `to`, from wherever the picture is and at whatever speed it is going there, as
    /// the strip does. Not while a hand has it. Says whether it is going anywhere.
    pub fn go_to(&mut self, to: i32, now: Instant, page: f32) -> bool {
        let page = page.max(1.0);
        if matches!(self.hand, Hand::Sliding(_)) {
            return false;
        }
        let to = self.clamp_page(to);
        let (from, speed) = match self.settle {
            Some(settle) => settle.at(now, page),
            None => (self.offset, 0.0),
        };
        if self.settle.is_none() && from == -(to as f32) * page {
            return false;
        }
        self.offset = from;
        self.settle = Some(Settle::eager(from, to, speed, now, page));
        self.glide = None;
        true
    }

    /// Move on to `now`: a scroll that has stopped coming is let go, and a settle eases on.
    /// Returns the page arrived at, once, when a settle comes to rest on a page other than this
    /// one -- which the viewer then opens.
    pub fn tick(&mut self, now: Instant, page: f32) -> Option<i32> {
        let page = page.max(1.0);
        match &self.hand {
            Hand::Deciding { last, .. } | Hand::Zooming { last } if now >= *last + LETS_GO_RESTING => self.hand = Hand::Idle,
            Hand::Sliding(slide) => match slide.lets_go_at() {
                Some(at) if now >= at => self.let_go(at, page),
                _ => {
                    let (home, raw) = (slide.home, slide.raw + slide.coast(now));
                    self.offset = self.banded(home, raw, page);
                }
            },
            _ => {}
        }
        let settle = self.settle?;
        self.offset = settle.at(now, page).0;
        if !settle.done(now) {
            return None;
        }
        // Back in its place, or arrived: the next view is this one's neighbour in its place, and if
        // there is none this picture is back in its own rather than off the window.
        self.settle = None;
        self.offset = 0.0;
        (settle.to != 0).then_some(settle.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: f32 = 1464.0;

    fn middle() -> Carousel {
        Carousel::new(Reach { before: 3, after: 3 })
    }

    /// Milliseconds after a moment, for writing what happens when.
    struct Clock(Instant);

    impl Clock {
        fn new() -> Clock {
            Clock(Instant::now())
        }

        fn at(&self, ms: f32) -> Instant {
            self.0 + Duration::from_secs_f32(ms / 1000.0)
        }
    }

    /// A scroll of `steps` scrolls of `dx` each, a frame apart from `from` ms, ticked as the room
    /// draws. Returns when the last came.
    fn swipe(c: &mut Carousel, clock: &Clock, from: f32, steps: usize, dx: f32) -> f32 {
        let mut t = from;
        for _ in 0..steps {
            assert_eq!(c.scroll(dx, 0.0, clock.at(t), PAGE), Scrolled::Taken);
            c.tick(clock.at(t), PAGE);
            t += 16.0;
        }
        t - 16.0
    }

    /// Tick on, a frame at a time, until it rests or arrives; what it arrived at, and when.
    fn settle(c: &mut Carousel, clock: &Clock, from: f32) -> (Option<i32>, f32) {
        let mut t = from;
        while t < from + 2000.0 {
            t += 8.0;
            if let Some(page) = c.tick(clock.at(t), PAGE) {
                return (Some(page), t);
            }
            if c.resting() {
                return (None, t);
            }
        }
        panic!("never settled: offset {}", c.offset());
    }

    #[test]
    fn a_sideways_scroll_slides_the_picture_under_it_one_to_one_and_never_zooms() {
        let clock = Clock::new();
        let mut c = middle();
        swipe(&mut c, &clock, 0.0, 10, 5.0);
        // The scroll is a finger's, so the picture goes the way the fingers do: left, to the next.
        assert!((c.offset() + 50.0).abs() < 0.01, "{}", c.offset());
        // Up and down, meanwhile, is nothing: the swipe is a slide until it stops.
        assert_eq!(c.scroll(1.0, 30.0, clock.at(160.0), PAGE), Scrolled::Taken);
        assert!((c.offset() + 51.0).abs() < 0.01, "{}", c.offset());
    }

    #[test]
    fn an_up_and_down_scroll_zooms_and_never_slides() {
        let clock = Clock::new();
        let mut c = middle();
        // A wheel's notch decides at once.
        assert_eq!(c.scroll(0.0, 15.0, clock.at(0.0), PAGE), Scrolled::Zoom(15.0));
        // A trackpad's drift sideways is not a slide, however far it drifts while the zoom goes on.
        for n in 1..20 {
            let t = clock.at(n as f32 * 16.0);
            assert_eq!(c.scroll(6.0, 4.0, t, PAGE), Scrolled::Zoom(4.0));
            c.tick(t, PAGE);
        }
        assert_eq!(c.offset(), 0.0);
        assert!(c.resting());
    }

    #[test]
    fn a_first_wobble_waits_to_be_decided_and_then_counts() {
        let clock = Clock::new();
        let mut c = middle();
        assert_eq!(c.scroll(3.0, 2.0, clock.at(0.0), PAGE), Scrolled::Taken);
        assert_eq!(c.scroll(3.0, -1.0, clock.at(16.0), PAGE), Scrolled::Taken);
        assert_eq!(c.offset(), 0.0, "nothing moves while it is undecided");
        c.scroll(3.0, 0.5, clock.at(32.0), PAGE);
        // Decided sideways at nine across, and all nine of them slid.
        assert!((c.offset() + 9.0).abs() < 0.01, "{}", c.offset());
        // Nothing decided, and it stops: nothing happened.
        let mut d = middle();
        d.scroll(2.0, 2.0, clock.at(0.0), PAGE);
        assert_eq!(d.tick(clock.at(500.0), PAGE), None);
        assert!(d.resting() && !d.busy());
    }

    #[test]
    fn let_go_past_a_quarter_it_goes_on_and_short_of_one_it_comes_back() {
        let clock = Clock::new();
        // Slowly, at a quarter of a pixel a millisecond: not a throw.
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 100, 4.0);
        assert!(c.offset() < -PAGE * ONWARD);
        assert_eq!(settle(&mut c, &clock, last).0, Some(1));

        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 60, 4.0);
        assert!(c.offset() > -PAGE * ONWARD);
        assert_eq!(settle(&mut c, &clock, last).0, None);
        assert_eq!(c.offset(), 0.0);

        // The other way, to the one before.
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 100, -4.0);
        assert_eq!(settle(&mut c, &clock, last).0, Some(-1));
    }

    #[test]
    fn a_flick_goes_on_however_short_and_a_twitch_does_not() {
        let clock = Clock::new();
        // Two of a pad's scrolls of a flick, all its filter lets out of a short one.
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 2, 31.0);
        assert!(c.offset() > -PAGE * ONWARD);
        assert_eq!(settle(&mut c, &clock, last).0, Some(1));
        // A single scroll says nothing of speed; nor do two quick ones that went nowhere.
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 1, 31.0);
        assert_eq!(settle(&mut c, &clock, last).0, None);
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 2, 9.0);
        assert_eq!(settle(&mut c, &clock, last).0, None);
    }

    #[test]
    fn thrown_back_it_comes_back_though_past_a_quarter() {
        let clock = Clock::new();
        let mut c = middle();
        let t = swipe(&mut c, &clock, 0.0, 130, 4.0);
        let last = swipe(&mut c, &clock, t + 16.0, 3, -40.0);
        assert!(c.offset() < -PAGE * ONWARD, "still past a quarter: {}", c.offset());
        assert_eq!(settle(&mut c, &clock, last).0, None);
    }

    #[test]
    fn it_is_not_over_while_the_scrolls_keep_coming() {
        let clock = Clock::new();
        let mut c = middle();
        // A slow drag with a thumb resting on it for a tenth of a second between.
        let t = swipe(&mut c, &clock, 0.0, 30, 2.0);
        let held = c.offset();
        assert_eq!(c.tick(clock.at(t + 100.0), PAGE), None);
        assert_eq!(c.offset(), held, "a creeping hand resting keeps the picture where it is");
        assert!(c.settling_to().is_none());
        let t = swipe(&mut c, &clock, t + 100.0, 30, 2.0);
        assert!((c.offset() + 120.0).abs() < 0.01, "{}", c.offset());
        // Quiet for longer than a creeping hand rests, it lets go.
        c.tick(clock.at(t + LETS_GO_RESTING.as_millis() as f32 + 1.0), PAGE);
        assert_eq!(c.settling_to(), Some(0));
    }

    #[test]
    fn a_brisk_scroll_coasts_on_rather_than_stopping_dead() {
        let clock = Clock::new();
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 12, 16.0);
        let at_last = c.offset();
        // A frame on, nothing has come, and that is only a frame: it is where the hand left it.
        c.tick(clock.at(last + 16.0), PAGE);
        assert_eq!(c.offset(), at_last);
        // Longer, it goes on at the hand's speed, a pixel a millisecond.
        c.tick(clock.at(last + 40.0), PAGE);
        assert!((c.offset() - (at_last - 20.0)).abs() < 0.5, "{} {}", c.offset(), at_last);
        // And on into the settle without a stop or a jump: going at the hand's speed as it goes
        // over, and faster after, as it has the rest of a page to go.
        let mut before = c.offset();
        let mut t = last + 40.0;
        for n in 0..12 {
            t += 4.0;
            c.tick(clock.at(t), PAGE);
            let step = before - c.offset();
            assert!(step > 0.0 && step < 25.0, "a step of {step}");
            if n < 2 {
                assert!((step - 4.0).abs() < 1.5, "a step of {step} as it goes over");
            }
            before = c.offset();
        }
        assert_eq!(c.settling_to(), Some(1));
    }

    #[test]
    fn at_the_folder_s_ends_it_gives_and_comes_back() {
        let clock = Clock::new();
        let mut c = Carousel::new(Reach { before: 0, after: 2 });
        let mut was = 0.0;
        let mut t = 0.0;
        for n in 1..=60 {
            c.scroll(-8.0, 0.0, clock.at(t), PAGE);
            let given = c.offset();
            assert!(given > was, "it gives a little more as the hand goes on");
            assert!(given < n as f32 * 8.0 * GIVE + 0.01, "and less than the hand: {given}");
            assert!(given < GIVE_MOST * PAGE);
            was = given;
            t += 16.0;
        }
        assert_eq!(settle(&mut c, &clock, t).0, None, "there is nothing before the first");
        assert_eq!(c.offset(), 0.0);
        // A throw at the last is a throw at nothing.
        let mut c = Carousel::new(Reach { before: 2, after: 0 });
        let last = swipe(&mut c, &clock, 0.0, 6, 40.0);
        assert_eq!(settle(&mut c, &clock, last).0, None);
    }

    #[test]
    fn the_band_and_its_undoing_agree() {
        for over in [0.0, 1.0, 40.0, 300.0, 1500.0, 9000.0] {
            let given = give(over, PAGE);
            assert!(given <= over * GIVE + 1e-3 && given < GIVE_MOST * PAGE);
            assert!((taken(given, PAGE) - over).abs() < over * 1e-3 + 1e-2, "{over} {given} {}", taken(given, PAGE));
        }
    }

    #[test]
    fn settling_eases_into_place_and_arrives_once() {
        let clock = Clock::new();
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 100, 4.0);
        c.tick(clock.at(last + 200.0), PAGE);
        // A creeping hand let go a little after its last, and the settle is from then.
        let began = clock.at(last) + LETS_GO_RESTING;
        let mut before = c.offset();
        let mut t = last + 200.0;
        let arrived = loop {
            t += 5.0;
            if let Some(page) = c.tick(clock.at(t), PAGE) {
                break page;
            }
            assert!(c.offset() <= before, "it only goes the one way");
            before = c.offset();
        };
        assert_eq!(arrived, 1);
        let took = clock.at(t).duration_since(began);
        assert!(took >= Duration::from_millis(200) && took <= Duration::from_millis(306), "{took:?}");
        // Arrived, it is done: nothing more is said.
        assert_eq!(c.tick(clock.at(t + 16.0), PAGE), None);
        assert!(c.resting());
    }

    #[test]
    fn a_hand_takes_hold_of_a_settling_picture_where_it_is() {
        let clock = Clock::new();
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 100, 4.0);
        c.tick(clock.at(last + 200.0), PAGE);
        // A new swipe, well after: until it is decided the settle goes on, and then it holds the
        // picture where the settle had got it to.
        c.scroll(5.0, 0.0, clock.at(last + 250.0), PAGE);
        c.tick(clock.at(last + 266.0), PAGE);
        let caught = c.offset();
        assert!(caught < -PAGE * 0.3 && caught > -PAGE * 0.9, "{caught}");
        c.scroll(5.0, 0.0, clock.at(last + 266.0), PAGE);
        assert!((c.offset() - (caught - 10.0)).abs() < 0.01, "{} {caught}", c.offset());
        assert!(c.settling_to().is_none());
    }

    #[test]
    fn a_second_flick_while_the_first_settles_goes_on_from_where_it_got_to() {
        let clock = Clock::new();
        // The first's settle is under way when the second comes, a fifth of a second later.
        let flicked = |second: usize| {
            let mut c = middle();
            let last = swipe(&mut c, &clock, 0.0, 8, 30.0);
            let mut t = last;
            while t < last + 200.0 {
                t += 8.0;
                c.tick(clock.at(t), PAGE);
            }
            assert!(c.offset() < -PAGE * 0.5, "{}", c.offset());
            let last = swipe(&mut c, &clock, t, second, 45.0);
            settle(&mut c, &clock, last).0
        };
        // Carried past the next page, on to the one after; short of it, it only finishes the first.
        assert_eq!(flicked(16), Some(2));
        assert_eq!(flicked(3), Some(1));
    }

    #[test]
    fn one_swipe_goes_no_further_than_the_next_page() {
        let clock = Clock::new();
        let mut c = middle();
        // A Mac's trackpad, its own momentum and all, carried twice a page.
        let last = swipe(&mut c, &clock, 0.0, 60, 50.0);
        assert!(c.offset() > -PAGE * (1.0 + GIVE_MOST), "{}", c.offset());
        assert_eq!(settle(&mut c, &clock, last).0, Some(1));
    }

    #[test]
    fn a_glide_after_a_throw_is_let_go_by() {
        let clock = Clock::new();
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 8, 30.0);
        // The Mac's glide begins nearly a tenth of a second after the fingers lift, and slows.
        let mut t = last;
        while t < last + 90.0 {
            t += 8.0;
            c.tick(clock.at(t), PAGE);
        }
        assert_eq!(c.settling_to(), Some(1));
        let mut step = 28.0;
        while step > 1.0 {
            assert_eq!(c.scroll(step, 0.0, clock.at(t), PAGE), Scrolled::Taken);
            assert_eq!(c.settling_to(), Some(1), "the glide does not take hold of the picture");
            t += 16.0;
            if let Some(page) = c.tick(clock.at(t), PAGE) {
                assert_eq!(page, 1);
                // On to the next view, which is told of it, and lets the rest of it go by too.
                let mut next = middle();
                next.carry(c.glide());
                while step > 1.0 {
                    step *= 0.94;
                    assert_eq!(next.scroll(step, 0.0, clock.at(t), PAGE), Scrolled::Taken);
                    next.tick(clock.at(t), PAGE);
                    assert!(next.resting(), "{}", next.offset());
                    t += 16.0;
                }
                return;
            }
            step *= 0.94;
        }
        panic!("never arrived");
    }

    #[test]
    fn a_new_swipe_is_not_taken_for_a_glide() {
        let clock = Clock::new();
        // The same way, but after the fingers have had time to lift and land again.
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 8, 30.0);
        c.tick(clock.at(last + 100.0), PAGE);
        let start = last + GLIDE_AFTER.as_millis() as f32 + 40.0;
        swipe(&mut c, &clock, start, 2, 30.0);
        assert!(c.settling_to().is_none(), "it took hold");
        // Or the other way at once: a hand pulling it back.
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 8, 30.0);
        c.tick(clock.at(last + 60.0), PAGE);
        swipe(&mut c, &clock, last + 70.0, 2, -10.0);
        assert!(c.settling_to().is_none(), "it took hold");
        // Or a glide that speeds up: a hand pushing again.
        let mut c = middle();
        let last = swipe(&mut c, &clock, 0.0, 8, 30.0);
        c.tick(clock.at(last + 60.0), PAGE);
        assert_eq!(c.scroll(10.0, 0.0, clock.at(last + 90.0), PAGE), Scrolled::Taken);
        assert_eq!(c.settling_to(), Some(1));
        c.scroll(30.0, 0.0, clock.at(last + 106.0), PAGE);
        c.scroll(30.0, 0.0, clock.at(last + 122.0), PAGE);
        assert!(c.settling_to().is_none(), "it took hold");
    }

    #[test]
    fn keys_slide_and_a_key_again_goes_one_further() {
        let clock = Clock::new();
        let mut c = middle();
        assert!(c.slide(1, clock.at(0.0), PAGE));
        c.tick(clock.at(50.0), PAGE);
        assert!(c.offset() < 0.0, "it is off at once");
        assert!(c.slide(1, clock.at(100.0), PAGE));
        assert_eq!(c.settling_to(), Some(2));
        let (arrived, _) = settle(&mut c, &clock, 100.0);
        assert_eq!(arrived, Some(2));
        // At the last, a key goes nowhere; held, as far as the last and no further.
        let mut c = Carousel::new(Reach { before: 1, after: 0 });
        assert!(!c.slide(1, clock.at(0.0), PAGE));
        assert!(c.resting());
        let mut c = Carousel::new(Reach { before: 0, after: 2 });
        for n in 0..10 {
            c.slide(1, clock.at(n as f32 * 30.0), PAGE);
            c.tick(clock.at(n as f32 * 30.0), PAGE);
        }
        assert_eq!(c.settling_to(), Some(2));
    }

    #[test]
    fn a_held_drag_follows_and_is_let_go_by_its_button() {
        let clock = Clock::new();
        let mut c = middle();
        c.grab(clock.at(0.0), PAGE);
        for n in 1..=40 {
            c.drag(-10.0, clock.at(n as f32 * 16.0), PAGE);
            // However long between, a held button is not let go of by quiet.
            assert_eq!(c.tick(clock.at(n as f32 * 16.0 + 400.0), PAGE), None);
        }
        assert!((c.offset() + 400.0).abs() < 0.01, "{}", c.offset());
        assert!(c.settling_to().is_none());
        c.release(clock.at(700.0), PAGE);
        assert_eq!(c.settling_to(), Some(1));
        // Held still before it was let go, a quick drag is not a throw.
        let mut c = middle();
        c.grab(clock.at(0.0), PAGE);
        for n in 1..=4 {
            c.drag(-25.0, clock.at(n as f32 * 16.0), PAGE);
        }
        c.release(clock.at(64.0 + 200.0), PAGE);
        assert_eq!(c.settling_to(), Some(0));
        let mut c = middle();
        c.grab(clock.at(0.0), PAGE);
        for n in 1..=4 {
            c.drag(-25.0, clock.at(n as f32 * 16.0), PAGE);
        }
        c.release(clock.at(70.0), PAGE);
        assert_eq!(c.settling_to(), Some(1));
    }

    #[test]
    fn it_heads_for_the_neighbour_it_is_taken_towards() {
        let clock = Clock::new();
        let mut c = middle();
        swipe(&mut c, &clock, 0.0, 10, 5.0);
        assert_eq!(c.heading(PAGE), None, "not yet far enough to say");
        swipe(&mut c, &clock, 160.0, 40, 5.0);
        assert_eq!(c.heading(PAGE), Some(1));
        let mut c = middle();
        c.slide(-1, clock.at(0.0), PAGE);
        assert_eq!(c.heading(PAGE), Some(-1));
    }

    #[test]
    fn a_settle_begun_at_speed_neither_overshoots_nor_starts_backwards() {
        let clock = Clock::new();
        for speed in [-20.0, -3.0, -0.5, 0.0, 0.5, 3.0, 20.0] {
            let settle = Settle::new(-400.0, 1, speed, clock.at(0.0), PAGE);
            let mut before = -400.0;
            for n in 1..=100 {
                let (at, _) = settle.at(clock.at(n as f32 * 3.0), PAGE);
                assert!(at <= before + 1e-3 && at >= -PAGE - 1e-3, "{speed}: {at} after {before}");
                before = at;
            }
            assert!((before + PAGE).abs() < 1e-3);
        }
    }
}
