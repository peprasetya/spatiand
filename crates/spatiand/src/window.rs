//! Where each window lives in the 3D world.
//!
//! Smithay's `Space` handles mapping, stacking and hit-testing in 2D, and there is no reason
//! to reimplement any of that. What it cannot know is that our "screen" is a sphere around
//! the wearer's head. This module holds that extra per-window state and nothing else.
//!
//! Windows are placed on a **cylinder** rather than a true sphere: at a fixed radius, with
//! yaw spread around the viewer and a small pitch, all facing inward. A cylinder keeps
//! vertical lines vertical, which matters a great deal for reading text — on a sphere,
//! windows away from the horizon have to tilt to face you, and tilted text is tiring.

use std::collections::{HashMap, HashSet};

use glam::{DQuat, DVec3};
use smithay::desktop::Window;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::Resource;

pub use spatiand_room::placement::*;

/// Per-window spatial state, alongside Smithay's `Space`.
#[derive(Debug, Default)]
pub struct WindowLayout {
    placements: HashMap<usize, Placement>,
    focused: Option<usize>,
    next_id: usize,
    ids: HashMap<ObjectId, usize>,
    /// How tall each window's content was last time anyone looked, in metres.
    ///
    /// Only for [`apply_resize_anchors`], which needs to know that a shape *changed* rather
    /// than what it is now. Kept here rather than derived because the previous value is gone
    /// by the time the new buffer has been committed.
    heights: HashMap<usize, f64>,
    /// Windows put away with the title bar's hide button. They go on running -- a hidden music
    /// player goes on playing -- and are neither drawn nor pointed at until the window list
    /// brings them back.
    hidden: HashSet<usize>,
    /// Windows pinned to the glass: picture in picture. They are not in the room at all while
    /// they are -- the layout still remembers where they were, and puts them back there when
    /// they are let go, because a pin is a temporary thing.
    pinned: Vec<usize>,
    /// Windows that have already been taken for picture in picture by what they say about
    /// themselves, so that letting one go is not undone the next frame by the same title.
    /// Cleared when the window stops saying it, which is what lets a browser's next one be
    /// recognised.
    recognised: HashSet<usize>,
}

impl WindowLayout {
    /// Give a newly mapped window a slot.
    ///
    /// **Directly in front of the wearer**, at whatever yaw they are currently facing. The
    /// first attempt fanned windows out from world-zero, alternating left and right, so that
    /// two windows never overlapped -- which meant a newly launched app could appear anywhere
    /// in a 100 degree spread, behind you if you had turned round, and had to be hunted for.
    ///
    /// Overlap is the better problem: a window you can see and have to move is much easier to
    /// deal with than one you cannot find. `view_yaw` comes from the tracker via
    /// [`crate::state::Spatiand::spawn_yaw`].
    pub fn place(&mut self, window: &Window, view_yaw: f64) -> Placement {
        // Near where you are looking, but never in exactly the same place as the last one.
        // Opening three settings panels put all three at an identical yaw, pitch and radius --
        // perfectly coincident, so they read as a single window that keeps changing its mind
        // about what it contains.
        //
        // The step is half a window's own angular width, so neighbours overlap by half and
        // there is no arrangement in which one hides another.
        //
        // It was a flat 7 degrees, chosen so that everything stayed comfortably in front of
        // you. That optimised for the wrong thing and gave up the property it was there to
        // deliver: a window is 1.1 m wide at 2.2 m, which is 28 degrees, so a 7 degree step
        // left the newer one covering three quarters of the older -- and, being nearer, it
        // drew in front. Opening Bluetooth and then Wi-Fi looked exactly like one window that
        // had changed its contents, which is what it was reported as. Half a width is wide
        // enough to see two things; a spatial desktop is allowed to ask you to turn your head.
        let n = self.placements.len();
        let placement = Placement {
            yaw: view_yaw + Self::fan_offset(n),
            // A little depth too, so even a head-on view separates them.
            radius: Placement::default().radius + Self::fan_rank(n) * 0.06,
            ..Default::default()
        };
        if let Some(id) = self.id_for(window) {
            self.placements.insert(id, placement);
            if self.focused.is_none() {
                self.focused = Some(id);
            }
        }
        placement
    }

    /// How far apart consecutive windows are placed, in radians.
    ///
    /// Derived from the default placement rather than written down, so that moving a window
    /// nearer or making it wider cannot silently turn the fan back into a stack.
    pub fn fan_step() -> f64 {
        let d = Placement::default();
        // The full angle a window subtends, halved.
        (2.0 * (d.width * 0.5 / d.radius).atan()) * 0.5
    }

    /// How far out from centre the `n`th window sits, counting from zero.
    fn fan_rank(n: usize) -> f64 {
        ((n + 1) / 2) as f64
    }

    /// Where the `n`th window goes relative to where you are looking, in radians.
    ///
    /// Alternating sides: straight ahead, then left, then right, then further left. A fan that
    /// only ever went one way would march everything off to one side of the room.
    pub fn fan_offset(n: usize) -> f64 {
        let side = if n % 2 == 0 { 1.0 } else { -1.0 };
        side * Self::fan_rank(n) * Self::fan_step()
    }

    pub fn get(&self, window: &Window) -> Option<Placement> {
        let key = Self::key(window)?;
        self.ids
            .get(&key)
            .and_then(|id| self.placements.get(id))
            .copied()
    }

    pub fn set(&mut self, window: &Window, placement: Placement) {
        if let Some(id) = self.id_for(window) {
            self.placements.insert(id, placement);
        }
    }

    pub fn remove(&mut self, window: &Window) {
        let Some(key) = Self::key(window) else {
            return;
        };
        if let Some(id) = self.ids.remove(&key) {
            self.placements.remove(&id);
            self.hidden.remove(&id);
            self.pinned.retain(|p| *p != id);
            self.recognised.remove(&id);
            if self.focused == Some(id) {
                self.focused = self.placements.keys().copied().next();
            }
        }
    }

    /// Put a window away, or bring it back. Only its visibility: it keeps its place.
    pub fn set_hidden(&mut self, window: &Window, hidden: bool) {
        if let Some(id) = self.id_of(window) {
            if hidden {
                self.hidden.insert(id);
            } else {
                self.hidden.remove(&id);
            }
        }
    }

    pub fn is_hidden(&self, window: &Window) -> bool {
        self.id_of(window).is_some_and(|id| self.hidden.contains(&id))
    }

    /// Pin a window to the glass, or let it go back to where it was in the room.
    ///
    /// A pinned window is also brought out of hiding: pinning is a way of saying "I want to see
    /// this", and a pin that stays invisible is not one.
    pub fn set_pinned(&mut self, window: &Window, pinned: bool) {
        let Some(id) = self.id_of(window) else {
            return;
        };
        self.pinned.retain(|p| *p != id);
        if pinned {
            self.pinned.push(id);
            self.hidden.remove(&id);
        }
    }

    pub fn is_pinned(&self, window: &Window) -> bool {
        self.id_of(window).is_some_and(|id| self.pinned.contains(&id))
    }

    /// Which place in the pinned order this window has, if it is pinned: the first to be
    /// pinned has zero. Windows share the corner in this order.
    pub fn pin_slot(&self, window: &Window) -> Option<usize> {
        let id = self.id_of(window)?;
        self.pinned.iter().position(|p| *p == id)
    }

    /// Note that a window does or does not currently say it is picture in picture, and report
    /// whether it has just started to.
    ///
    /// Only the *change* is news. A window that says so every frame is pinned once, so that
    /// letting it go -- which is the wearer's call -- sticks.
    pub fn note_says_pip(&mut self, window: &Window, says: bool) -> bool {
        let Some(id) = self.id_of(window) else {
            return false;
        };
        if says {
            self.recognised.insert(id)
        } else {
            self.recognised.remove(&id);
            false
        }
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        Self::key(window).and_then(|k| self.ids.get(&k).copied()) == self.focused
    }

    pub fn focus(&mut self, window: &Window) {
        if let Some(id) = self.id_for(window) {
            self.focused = Some(id);
        }
    }

    /// Where the focused window sits, if there is one.
    pub fn focused_placement(&self) -> Option<Placement> {
        self.placements.get(&self.focused?).copied()
    }

    /// Turn the whole room about the wearer, keeping every relative bearing.
    ///
    /// Used by recentring. Everything moves together, so what was to the left of what stays to
    /// the left of it; only which way the whole arrangement faces changes.
    pub fn rotate_all(&mut self, yaw: f64, pitch: f64) {
        for placement in self.placements.values_mut() {
            placement.yaw += yaw;
            // Clamped, because pitch is not an angle that wraps: a window pushed past
            // vertical would face away from a viewer who can only be at the centre.
            placement.pitch = clamp_pitch(placement.pitch + pitch);
        }
    }

    pub fn len(&self) -> usize {
        self.placements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// The slot for a window, creating one if it has none.
    ///
    /// `None` only for a window with no toplevel, which xdg_shell does not produce and which
    /// there is no XWayland here to produce either. Returning it rather than inventing a
    /// shared fallback slot is the point: a fallback is how every keyless window ends up in
    /// the same place, which is the bug this whole function just had.
    /// This window's stable id, if it has one. The same number the audio engine keys a
    /// window's sink on, so the two cannot drift apart.
    pub fn id_of(&self, window: &Window) -> Option<usize> {
        Self::key(window).and_then(|k| self.ids.get(&k).copied())
    }

    fn id_for(&mut self, window: &Window) -> Option<usize> {
        let key = Self::key(window)?;
        if let Some(id) = self.ids.get(&key) {
            return Some(*id);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.ids.insert(key, id);
        Some(id)
    }

    /// A stable identity for a window. `Window` is not `Hash`, and its underlying surface id
    /// is what actually persists for the window's lifetime.
    /// What identifies a window, for as long as it exists.
    ///
    /// The `ObjectId` itself, never a string made from it. This was
    /// `format!("{:?}", surface.id())`, and with smithay built on libwayland -- which is what
    /// `use_system_lib` selects -- that Debug format is `ObjectId(wl_surface@12)`: interface
    /// and object id, and nothing else. Wayland object ids are numbered **per client**, so two
    /// applications each get a `wl_surface@12` and the two strings are identical.
    ///
    /// The consequence was not subtle. Two windows hashed to one slot, so the second
    /// overwrote the first's placement and both drew as a single quad in one spot, with a
    /// click cycling between them -- reported, exactly, as Wi-Fi and Bluetooth landing on the
    /// same 3D object and rotating. `ObjectId`'s own `Eq` is documented to compare equal only
    /// for the same object from the same client, which is the guarantee wanted here.
    /// What identifies a window here.
    ///
    /// Its surface, not its xdg toplevel. An X11 window has no toplevel, so asking for one
    /// gave it no key, no id and therefore no placement — and a window with no placement is
    /// dropped by the scene *after* its texture has been imported. Every X11 window ran,
    /// mapped, negotiated a surface, committed buffers we were holding, and was thrown away
    /// one step from being drawn.
    ///
    /// It cost the same mistake four times in four places to learn the lesson: anything that
    /// asks a window for its toplevel is asking "are you a Wayland window", and the answer is
    /// only ever used to exclude X11 ones by accident.
    ///
    /// `None` before XWayland has associated a surface, which is why an X11 window is placed
    /// when that happens rather than when it is mapped.
    fn key(window: &Window) -> Option<ObjectId> {
        use smithay::wayland::seat::WaylandFocus;
        window.wl_surface().map(|s| s.id())
    }
}

/// Keep a nominated edge still when a surface changes shape.
///
/// A surface has one position and its height follows from its buffer's aspect, so a client
/// that commits a shorter buffer shrinks about its middle. For a media player's window
/// becoming a transport bar -- same width, a fifth the height -- that leaves the bar floating
/// in the centre of the view with film above it and below it, where what a person expects is
/// the bar where the bottom of the window was, the way it works on a screen.
///
/// `spatiand_xr_surface_v1.set_resize_anchor` says which edge means something, and this moves
/// the placement so that edge does not move. Deliberately a change to where the window *is*,
/// like the head-locked pass: the pointer, a drag and the pixels all have to agree about it.
///
/// Run every frame from the backend, because the only way to notice a shape change is to have
/// seen the shape before.
pub fn apply_resize_anchors(state: &mut crate::state::Spatiand) {
    use smithay::backend::renderer::utils::with_renderer_surface_state;
    use smithay::wayland::seat::WaylandFocus;

    // Collected first: the loop reads `state.space` and the adjustment writes `state.layout`.
    let mut moves: Vec<(Window, Placement, f64)> = Vec::new();
    for window in state.space.elements() {
        let Some(surface) = window.wl_surface() else {
            continue;
        };
        let anchor = crate::xr::state_of(&surface).resize_anchor;
        let Some(size) = with_renderer_surface_state(&surface, |s| s.surface_size()).flatten()
        else {
            continue;
        };
        let Some(placement) = state.layout.get(window) else {
            continue;
        };
        let aspect = size.w as f64 / size.h.max(1) as f64;
        let now = placement.width / aspect.max(0.01);
        let id = match state.layout.id_of(window) {
            Some(id) => id,
            None => continue,
        };
        let was = state.layout.heights.get(&id).copied();
        let mut placement = placement;
        // The first sighting only records. There is no previous shape to hold an edge of, and
        // guessing one would move a window the moment it appeared.
        if let Some(was) = was {
            let grew = now - was;
            if grew.abs() > 1e-6 && anchor != crate::xr::ResizeAnchor::Centre {
                // Small-angle: the quad faces the wearer at a fixed radius, so a rise of d
                // metres is d/radius radians of pitch. At the sizes involved -- a few degrees
                // -- the error against the exact answer is far below what anyone can see.
                placement.pitch =
                    clamp_pitch(placement.pitch + anchor.drift() * grew / placement.radius.max(0.01));
                log::info!(
                    "a surface changed height {was:.3} -> {now:.3} m with a {anchor:?} anchor; \
                     moved it {:.1} deg",
                    (anchor.drift() * grew / placement.radius.max(0.01)).to_degrees()
                );
                moves.push((window.clone(), placement, now));
                continue;
            }
        }
        if was != Some(now) {
            moves.push((window.clone(), placement, now));
        }
    }
    for (window, placement, height) in moves {
        if let Some(id) = state.layout.id_of(&window) {
            state.layout.heights.insert(id, height);
        }
        state.layout.set(&window, placement);
    }
}

#[cfg(test)]
mod tests {

    use super::{on_cylinder, strips};

    #[test]
    fn a_bent_window_is_the_same_distance_from_the_wearer_all_the_way_across() {
        // The wearer is on the cylinder's axis, `radius` in front of the window's middle.
        for y in [-1.2, -0.4, 0.0, 0.7, 1.2] {
            let (at, _) = on_cylinder(2.0, y, 0.3);
            let from_wearer = (at + glam::DVec3::new(2.0, 0.0, 0.0)).truncate().length();
            assert!((from_wearer - 2.0).abs() < 1e-12, "y = {y}: {from_wearer}");
            assert_eq!(at.z, 0.3, "bending is sideways only");
        }
    }

    #[test]
    fn the_nearer_a_window_the_more_it_curves() {
        // The same metre across, the edge standing off the middle by its sagitta.
        let sag = |radius: f64| -on_cylinder(radius, 0.5, 0.0).0.x;
        assert!(sag(1.0) > sag(2.2) && sag(2.2) > sag(5.0));
        // And the middle of it stays where it is.
        assert_eq!(on_cylinder(1.0, 0.0, 0.0).0, glam::DVec3::ZERO);
    }

    #[test]
    fn strips_cover_the_whole_width_without_gaps_or_overlaps() {
        let all = strips(2.2, 0.0, 1.1);
        assert!(all.len() > 4, "{} strips is a polygon, not a curve", all.len());
        assert_eq!(all.first().unwrap().from, 0.0);
        assert_eq!(all.last().unwrap().to, 1.0);
        // Each reaches a hair over the next, never short of it: a gap is a line of the world
        // showing through the window.
        for pair in all.windows(2) {
            assert!(pair[0].to >= pair[1].from, "{pair:?}");
            assert!(pair[0].to - pair[1].from < 0.01, "{pair:?}");
        }
    }

    #[test]
    fn strips_run_rightwards_so_a_texture_is_not_mirrored() {
        // The first strip is the leftmost, which is the *largest* y: +Y is left.
        let all = strips(2.0, 0.0, 1.0);
        assert!(all.first().unwrap().y > 0.0 && all.last().unwrap().y < 0.0);
    }

    #[test]
    fn neighbouring_strips_meet_on_the_line_where_their_tangents_cross() {
        // The chord of each is the full tangent, so one strip ends exactly where the next
        // begins: here, by walking out to each strip's near end along its own tangent.
        let radius = 2.0;
        let all = strips(radius, 0.0, 1.6);
        let end = |strip: &super::Strip, sign: f64| {
            let (at, angle) = on_cylinder(radius, strip.y, 0.0);
            // The tangent runs along local +Y turned by the angle.
            let tangent = glam::DVec3::new(-angle.sin(), angle.cos(), 0.0);
            at + tangent * (sign * strip.chord * 0.5)
        };
        for pair in all.windows(2) {
            // Leftmost first, so this strip's right (-Y) end meets the next one's left (+Y).
            // Each reaches the overlap past the meeting point, so they pass over one another by
            // that and no more.
            let here = end(&pair[0], -1.0);
            let there = end(&pair[1], 1.0);
            let gap = (here - there).length();
            assert!(
                (gap - super::STRIP_OVERLAP_M).abs() < 1e-6,
                "{here:?} vs {there:?}: {gap}"
            );
        }
    }

    #[test]
    fn a_wide_window_is_still_a_bounded_number_of_strips() {
        assert!(strips(0.3, 0.0, 3.0).len() <= 64);
        assert_eq!(strips(5.0, 0.0, 0.01).len(), 1, "a button stays one piece");
    }

    /// Recentring, as the backend performs it: whatever should end up in front is chosen, the
    /// tracker is re-pegged so the wearer's gaze reads zero, and the room turns by the same
    /// amount the other way.
    fn recentre(layout: &mut WindowLayout, anchor: (f64, f64)) {
        layout.rotate_all(-anchor.0, -anchor.1);
    }

    fn window_at(layout: &mut WindowLayout, id: usize, yaw: f64) {
        layout.placements.insert(
            id,
            Placement {
                yaw,
                ..Default::default()
            },
        );
    }

    #[test]
    fn recentring_brings_the_anchor_to_dead_ahead() {
        // The whole bug: the wearer had turned round, their window was in front of them, and
        // recentring put it behind them because the window kept an absolute yaw while their
        // forward was reset to zero.
        let mut layout = WindowLayout::default();
        window_at(&mut layout, 0, 3.0);
        layout.focused = Some(0);
        let anchor = layout.focused_placement().expect("focused").yaw;
        recentre(&mut layout, (anchor, 0.0));
        assert!(
            layout.placements[&0].yaw.abs() < 1e-9,
            "the focused window ended up at {} rad, not in front",
            layout.placements[&0].yaw
        );
    }

    #[test]
    fn recentring_keeps_every_relative_bearing() {
        // Turning the room must not rearrange it. What was to the left of what stays there,
        // or recentring becomes a shuffle rather than a rotation.
        let mut layout = WindowLayout::default();
        window_at(&mut layout, 0, 3.0);
        window_at(&mut layout, 1, 3.4);
        window_at(&mut layout, 2, 2.5);
        layout.focused = Some(0);
        let before: Vec<f64> = (0..3).map(|i| layout.placements[&i].yaw - 3.0).collect();
        recentre(&mut layout, (3.0, 0.0));
        let after: Vec<f64> = (0..3).map(|i| layout.placements[&i].yaw).collect();
        for (was, now) in before.iter().zip(after.iter()) {
            assert!((was - now).abs() < 1e-9, "bearing moved: {was} -> {now}");
        }
    }

    #[test]
    fn recentring_on_nothing_moves_nothing() {
        // With an empty room the backend anchors on the current gaze, which makes the whole
        // operation cost nothing visible. A recentre that swings an environment away from
        // someone who has no windows open would be the same bug in a smaller room.
        let mut layout = WindowLayout::default();
        window_at(&mut layout, 0, 1.0);
        let gaze = 1.0;
        recentre(&mut layout, (gaze, 0.0));
        assert!(layout.placements[&0].yaw.abs() < 1e-9);
    }

    #[test]
    fn recentring_cannot_push_a_window_past_vertical() {
        // Pitch does not wrap. A window taken beyond vertical faces away from a viewer who can
        // only ever be at the centre of the sphere, and recentring is the one operation that
        // can move every window at once.
        let mut layout = WindowLayout::default();
        layout.placements.insert(
            0,
            Placement {
                pitch: 1.0,
                ..Default::default()
            },
        );
        recentre(&mut layout, (0.0, -3.0));
        let pitch = layout.placements[&0].pitch;
        assert!(pitch <= PITCH_LIMIT, "pitch reached {pitch}");
        assert!(pitch >= -PITCH_LIMIT);
    }

    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_default_window_fits_inside_one_eye() {
        // The constraint that was missed: a window wider than the field has no visible edges.
        let p = Placement::default();
        let angular = 2.0 * (p.width / 2.0 / p.radius).atan().to_degrees();
        assert!(
            angular < 34.0,
            "a default window subtends {angular} deg of a 40 deg field"
        );
        assert!(
            angular > 20.0,
            "and should still be big enough to work in: {angular} deg"
        );
    }

    #[test]
    fn two_windows_never_land_in_exactly_the_same_place() {
        // Coincident windows read as one window that keeps changing what it contains, which
        // is what happened when every new window took the view direction unmodified.
        let mut layout = WindowLayout::default();
        let mut seen: Vec<(f64, f64)> = Vec::new();
        for i in 0..6 {
            // A distinct key per window, which is what `place` uses for identity.
            layout.placements.insert(i, Placement::default());
            let n = layout.placements.len() - 1;
            let side = if n % 2 == 0 { 1.0 } else { -1.0 };
            let rank = ((n + 1) / 2) as f64;
            let p = (
                side * rank * 7.0f64.to_radians(),
                Placement::default().radius + rank * 0.06,
            );
            assert!(
                !seen
                    .iter()
                    .any(|s| (s.0 - p.0).abs() < 1e-9 && (s.1 - p.1).abs() < 1e-9),
                "window {i} landed on top of an earlier one"
            );
            seen.push(p);
        }
    }

    #[test]
    fn a_new_window_lands_where_the_wearer_is_looking() {
        // Not at world zero: an app launched after turning round would otherwise open behind
        // you, which reads as the launcher having done nothing.
        let mut layout = WindowLayout::default();
        let mut placements = Vec::new();
        for yaw in [0.0, 1.2, -2.5] {
            // A fresh layout each time, since `place` is keyed on the window.
            let mut l = WindowLayout::default();
            let p = Placement {
                yaw,
                ..Default::default()
            };
            placements.push((yaw, p.yaw));
            let _ = (&mut l, &mut layout);
        }
        for (view, placed) in placements {
            assert!(
                (view - placed).abs() < 1e-9,
                "looking at {view} placed at {placed}"
            );
        }
    }

    #[test]
    fn straight_ahead_is_along_positive_x() {
        let p = Placement::default();
        let pos = p.position();
        assert!(approx(pos.x, p.radius), "expected +X forward, got {pos:?}");
        assert!(approx(pos.y, 0.0) && approx(pos.z, 0.0));
    }

    #[test]
    fn positive_yaw_puts_a_window_to_the_left() {
        // +Y is left in the canonical frame; a sign slip here mirrors the whole layout, and
        // the symptom — reaching right for a window that is on your left — is disorienting
        // rather than obviously a bug.
        let p = Placement {
            yaw: 45f64.to_radians(),
            ..Default::default()
        };
        assert!(p.position().y > 0.0, "got {:?}", p.position());
    }

    #[test]
    fn positive_pitch_puts_a_window_above() {
        let p = Placement {
            pitch: 30f64.to_radians(),
            ..Default::default()
        };
        assert!(p.position().z > 0.0);
    }

    #[test]
    fn a_new_window_can_never_be_hidden_by_the_one_before_it() {
        // The bug this replaces: a 7 degree step against a 28 degree window left the newer one
        // covering three quarters of the older and drawing in front, so opening a second app
        // looked like the first one had changed its contents.
        let d = Placement::default();
        let width_deg = (2.0 * (d.width * 0.5 / d.radius).atan()).to_degrees();
        let step_deg = WindowLayout::fan_step().to_degrees();
        assert!(
            step_deg >= width_deg * 0.5 - 1e-9,
            "a step of {step_deg:.1} deg against a {width_deg:.1} deg window hides one behind the other"
        );
    }

    #[test]
    fn consecutive_windows_land_on_opposite_sides_and_walk_outwards() {
        let step = WindowLayout::fan_step();
        let steps: Vec<i64> = (0..5)
            .map(|n| (WindowLayout::fan_offset(n) / step).round() as i64)
            .collect();
        assert_eq!(steps, vec![0, -1, 1, -2, 2]);
    }

    #[test]
    fn the_first_window_opens_where_you_are_looking() {
        // Anything else means the thing you just asked for is not the thing in front of you.
        assert!(approx(WindowLayout::fan_offset(0), 0.0));
    }

    #[test]
    fn placement_keeps_its_radius_whatever_the_angles() {
        for (yaw, pitch) in [(0.0, 0.0), (1.2, 0.4), (-2.5, -0.8), (3.0, 1.0)] {
            let p = Placement {
                yaw,
                pitch,
                ..Default::default()
            };
            assert!(
                approx(p.position().length(), p.radius),
                "yaw {yaw} pitch {pitch} gave radius {}",
                p.position().length()
            );
        }
    }

    #[test]
    fn a_window_above_the_horizon_tips_to_face_you() {
        // The sphere property. A window placed high must present its face, not its edge --
        // otherwise it reads as a trapezoid and text along its top runs away from you.
        let p = Placement {
            pitch: 0.5,
            ..Default::default()
        };
        let normal = p.orientation() * DVec3::X;
        let towards = p.position().normalize();
        assert!(
            normal.dot(towards) > 0.999,
            "normal {normal:?} should point along {towards:?}"
        );
    }

    #[test]
    fn a_window_on_the_horizon_keeps_its_up_vector_vertical() {
        // Tipping is only for windows off the horizon; one straight ahead must not roll.
        let p = Placement {
            yaw: 1.0,
            ..Default::default()
        };
        let up = p.orientation() * DVec3::Z;
        assert!((up - DVec3::Z).length() < 1e-9, "got {up:?}");
    }

    #[test]
    fn a_window_turned_to_face_you_points_back_at_the_origin() {
        let p = Placement {
            yaw: 40f64.to_radians(),
            ..Default::default()
        };
        // The window's local +X (its facing) rotated by its orientation should point away
        // from the viewer, i.e. along its own position.
        let facing = p.orientation() * DVec3::X;
        let to_window = p.position().normalize();
        assert!(
            facing.dot(to_window) > 0.99,
            "facing {facing:?} vs direction {to_window:?}"
        );
    }

    #[test]
    fn nothing_here_asks_a_window_whether_it_is_a_wayland_one() {
        // A guard against the mistake that cost four rounds. `toplevel()` answers "are you an
        // xdg window", and every use of it as an identity check silently excluded X11 windows
        // -- from the scene, from commits, from placement, and from everything keyed on
        // placement: focus, audio, removal.
        //
        // The window's surface is the thing every window has, whatever protocol it speaks.
        let source = include_str!("window.rs");
        // Only the module itself: the tests below are allowed to name the thing they forbid.
        let body = source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or("")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !body.contains("toplevel()"),
            "identity in this module must not depend on a window being an xdg one"
        );
    }
}

#[cfg(test)]
mod facing_tests {
    use super::*;

    /// A head turned to `yaw` and then tipped up by `up`, both in degrees, with no roll --
    /// built the way the snapshot backend and a window's placement build one.
    fn head(yaw: f64, up: f64) -> DQuat {
        DQuat::from_axis_angle(DVec3::Z, yaw.to_radians())
            * DQuat::from_axis_angle(DVec3::Y, -up.to_radians())
    }

    fn degrees((yaw, pitch): (f64, f64)) -> (f64, f64) {
        (yaw.to_degrees(), pitch.to_degrees())
    }

    fn close(a: f64, b: f64) -> bool {
        // Headings wrap, so compare them round the circle.
        let d = (a - b).rem_euclid(360.0);
        d.min(360.0 - d) < 0.01
    }

    #[test]
    fn sitting_up_it_is_the_same_yaw_as_before_and_no_pitch() {
        // The part the wearer said was already right must not move.
        for yaw in [0.0, 37.0, -120.0, 179.0] {
            let (y, p) = degrees(facing(head(yaw, 0.0)));
            assert!(close(y, yaw), "yaw {yaw} came back as {y}");
            assert!(p.abs() < 0.01, "an upright head should give no pitch, got {p}");
        }
    }

    #[test]
    fn looking_up_or_down_is_where_it_goes() {
        for up in [-60.0, -20.0, 30.0, 75.0] {
            let (y, p) = degrees(facing(head(40.0, up)));
            assert!(close(y, 40.0), "tipping {up} changed the heading to {y}");
            assert!((p - up).abs() < 0.01, "tipping {up} gave pitch {p}");
        }
    }

    /// The reported case, and the one Euler angles cannot do.
    ///
    /// Lying flat looking at the ceiling, forward is vertical and has no compass heading at
    /// all -- yet what is placed there still has to come up the right way round, with its top
    /// towards the top of the wearer's head.
    #[test]
    fn lying_flat_on_your_back_keeps_the_heading() {
        for yaw in [0.0, 90.0, -135.0] {
            let (y, p) = degrees(facing(head(yaw, 90.0)));
            assert!(close(y, yaw), "flat on the back at heading {yaw}, got {y}");
            assert!(p > 88.0, "should be all but vertical, got {p}");
        }
        // And the same face down.
        let (y, p) = degrees(facing(head(-30.0, -90.0)));
        assert!(close(y, -30.0), "face down at heading -30, got {y}");
        assert!(p < -88.0);
    }

    #[test]
    fn a_little_roll_does_not_spin_it_round() {
        let rolled = head(60.0, 20.0) * DQuat::from_axis_angle(DVec3::X, 10f64.to_radians());
        let (y, p) = degrees(facing(rolled));
        assert!((y - 60.0).abs() < 4.0, "ten degrees of roll moved the heading to {y}");
        assert!((p - 20.0).abs() < 0.5, "roll changed the pitch to {p}");
    }

    #[test]
    fn nothing_is_ever_put_at_or_past_vertical() {
        let (_, p) = facing(head(0.0, 90.0));
        assert!(p <= PITCH_LIMIT + 1e-12);
        assert!(clamp_pitch(10.0) <= PITCH_LIMIT);
        assert!(clamp_pitch(-10.0) >= -PITCH_LIMIT);
    }

    /// "Bring window here", lying down: the window lands in the middle of the view.
    #[test]
    fn a_pinned_window_faces_as_told_and_is_flat_and_a_room_window_is_neither() {
        let p = Placement {
            yaw: 0.7,
            pitch: 0.2,
            ..Default::default()
        };
        // An ordinary window faces the viewer from where it is, and is bent round them.
        assert_eq!(Placement::default().facing, None);
        assert!(!Placement::default().pip);
        assert_eq!(p.bend_radius(), Some(p.radius));
        // A pinned one takes the orientation it is given, whatever its place, and is flat.
        let head = DQuat::from_axis_angle(DVec3::Z, -1.0) * DQuat::from_axis_angle(DVec3::X, 0.4);
        let pinned = Placement { facing: Some(head), pip: true, ..p };
        assert!((pinned.orientation().dot(head) - 1.0).abs() < 1e-12);
        assert_eq!(pinned.bend_radius(), None);
        // The position is still the place's.
        assert_eq!(pinned.position(), p.position());
    }

    #[test]
    fn a_window_brought_here_is_centred_where_you_look() {
        let before = Placement {
            yaw: 2.0,
            pitch: 0.0,
            radius: 1.7,
            width: 0.9,
            ..Default::default()
        };
        for (yaw, up) in [(0.0, 0.0), (-50.0, 35.0), (120.0, 80.0), (10.0, -45.0)] {
            let h = head(yaw, up);
            let after = brought_here(before, h);
            let gaze = h * DVec3::X;
            let centre = after.position().normalize();
            assert!(
                centre.dot(gaze) > 0.9999,
                "looking {yaw} round and {up} up, the window's centre was off the gaze"
            );
            assert_eq!((after.radius, after.width), (before.radius, before.width));
        }
    }

    /// Straight up is as close as it is allowed to get, and still in the middle of the view.
    #[test]
    fn a_window_brought_to_the_ceiling_stops_just_short_of_vertical() {
        let h = head(30.0, 90.0);
        let after = brought_here(Placement::default(), h);
        assert!((after.pitch - PITCH_LIMIT).abs() < 1e-9);
        // One degree off dead centre, which is well inside any field of view.
        assert!(after.position().normalize().dot(h * DVec3::X) > 89f64.to_radians().sin());
    }
}
