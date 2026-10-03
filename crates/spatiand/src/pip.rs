//! Windows pinned to the glass: picture in picture.
//!
//! The geometry -- where a window fixed to the head is in the room, which corner, which size --
//! is in [`spatiand_render::pip`] where it can be tested without a compositor. This is the part
//! that needs one: working out which windows *are* picture in picture, and putting those windows
//! where the geometry says.
//!
//! **Nothing here asks an application anything**, because there is nothing to ask. Wayland has
//! no request for "I am a picture-in-picture window" -- a browser's is an ordinary toplevel that
//! happens to be small -- so a window is pinned in one of three ways:
//!
//! * **by hand**, with the pin button on its bar or a row in the window list, which works for
//!   any window and is the one that always does;
//! * **by what it is called**, for the titles browsers give theirs; and
//! * **by what an X11 application asks for**: `_NET_WM_STATE_ABOVE`, the one place there *is*
//!   a request, read here from the X server.
//!
//! Pinning is never remembered against the application: a pin is temporary by nature, and the
//! next window starts in the room. What *is* remembered is where the corner is and how big.

use std::time::{Duration, Instant};

use glam::DQuat;
use smithay::desktop::Window;
use spatiand_render::pip::{self as geometry, View};

pub use geometry::{Corner, Size};

use crate::scene::WindowQuad;
use crate::state::Spatiand;
use crate::window::{Placement, WindowLayout};

/// Where pinned windows go and how big they are: the two things the wearer sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Settings {
    pub corner: Corner,
    pub size: Size,
}

impl Settings {
    /// The corner's number in the order the settings list shows them. `Corner::ALL`'s order,
    /// which `spatiand_shell`'s rows are written in; a test holds the two together.
    pub fn corner_index(&self) -> usize {
        Corner::ALL
            .iter()
            .position(|c| *c == self.corner)
            .unwrap_or(0)
    }
}

/// How the glasses present the world, as far as where a corner is.
pub fn view_of(stereo: &spatiand_render::StereoConfig) -> View {
    View {
        fov_deg: (stereo.h_fov_deg, stereo.v_fov_deg()),
        // The neck lever the eyes ride on, in the head's own frame (+X forward, +Z up).
        eye: glam::DVec3::new(stereo.neck_forward_m, 0.0, stereo.neck_up_m),
    }
}

/// Put every pinned window where it belongs for a head pointing along `head`.
///
/// Called from `collect_windows` each frame with the head the eyes are about to be built from,
/// so that the window and the view move as one. What it writes is an ordinary placement in the
/// room -- computed, not stored -- which is why the drawing and the pointer ray, both of which
/// start from a window's placement, need no idea that this window is special beyond
/// [`Placement::pip`].
pub fn lock(
    windows: &mut [WindowQuad],
    settings: Settings,
    layout: &WindowLayout,
    head: DQuat,
    view: &View,
) {
    // In the order they were pinned, so that the first is in the corner and the next beside it.
    // Counted among those actually drawn: a pinned window with nothing to show yet does not
    // leave a hole.
    let mut pinned: Vec<(usize, usize)> = windows
        .iter()
        .enumerate()
        .filter_map(|(i, w)| layout.pin_slot(&w.window).map(|slot| (slot, i)))
        .collect();
    pinned.sort_unstable();
    for (n, (_, i)) in pinned.into_iter().enumerate() {
        let quad = &mut windows[i];
        let aspect = quad.pixels.0 as f64 / quad.pixels.1.max(1) as f64;
        let pose = geometry::pose(head, view, settings.corner, settings.size, aspect, n);
        quad.placement = Placement {
            yaw: pose.yaw,
            pitch: pose.pitch,
            radius: pose.radius,
            width: pose.width,
            facing: Some(pose.facing),
            pip: true,
        };
    }
}

/// How often windows are looked at for saying they are picture in picture.
///
/// A title is rewritten when a page does, and a quarter of a second after a browser opens its
/// picture-in-picture window is no delay anyone sees. Every frame would be a string copy per
/// window, 72 times a second, to learn nothing.
const LOOK_EVERY: Duration = Duration::from_millis(250);

/// Pin whichever windows have just started saying they are picture in picture.
///
/// Only the *start* of saying it counts -- see [`WindowLayout::note_says_pip`] -- so a window the
/// wearer has let go stays in the room however long its title goes on saying so.
pub fn auto_pin(state: &mut Spatiand) {
    if state
        .pip_looked
        .is_some_and(|at: Instant| at.elapsed() < LOOK_EVERY)
    {
        return;
    }
    state.pip_looked = Some(Instant::now());
    let windows: Vec<Window> = state.space.elements().cloned().collect();
    for window in windows {
        if Spatiand::is_environment(&window) {
            continue;
        }
        let says = says_picture_in_picture(state, &window);
        if state.layout.note_says_pip(&window, says) {
            log::info!(
                "pinned to the glass: {:?} says it is picture in picture",
                state.title_of(&window).unwrap_or_default()
            );
            state.layout.set_pinned(&window, true);
        }
    }
}

/// A remote window's title without the computer's name it was given: `Picture in picture —
/// workshop` is what the browser there called it, and where.
fn without_host(title: &str) -> &str {
    title.rsplit_once(" — ").map_or(title, |(what, _)| what)
}

/// Set to pin every window, for a snapshot: there is no hand in one to press the pin button.
pub const PIN_ALL_ENV: &str = "SPATIAND_PIN_ALL";

fn says_picture_in_picture(state: &Spatiand, window: &Window) -> bool {
    if std::env::var_os(PIN_ALL_ENV).is_some() {
        return true;
    }
    // A window from another computer has that computer's name on the end of its title -- see
    // `remote::session::window_title` -- so what the browser called it is what comes before.
    let remote = state
        .app_id_of(window)
        .is_some_and(|id| id.starts_with("remote."));
    if state.title_of(window).is_some_and(|title| {
        geometry::title_is_pip(if remote { without_host(&title) } else { &title })
    }) {
        return true;
    }
    // An X11 application asking to be kept above the others. Not when it is also the whole
    // screen: a game that raises itself over the desktop is asking to be the room, not a
    // corner of it.
    let (Some(display), Some(x11)) = (state.x11_display, window.x11_surface()) else {
        return false;
    };
    !x11.is_fullscreen()
        && !x11.is_maximized()
        && crate::xwayland::asks_to_stay_above(display, x11.window_id())
}

/// The wearer chose the next corner: remember it, use it, and show it in the settings list.
pub fn next_corner(
    state: &mut Spatiand,
    prefs: &mut crate::prefs::Prefs,
    shell: &mut spatiand_shell::Shell,
) {
    prefs.pip_corner = prefs.pip_corner.next();
    adopt(state, prefs, shell);
    prefs.save();
}

/// The wearer chose the other size.
pub fn toggle_size(
    state: &mut Spatiand,
    prefs: &mut crate::prefs::Prefs,
    shell: &mut spatiand_shell::Shell,
) {
    prefs.pip_size = prefs.pip_size.toggle();
    adopt(state, prefs, shell);
    prefs.save();
}

/// Take the preferences' corner and size as the compositor's, and tell the settings list.
///
/// Also the first thing a session does with them, so that the list opens saying what is true
/// and not what is default. It does not write the file: a session that changed nothing has no
/// business rewriting it, and rewriting drops whatever a newer build put there.
pub fn adopt(
    state: &mut Spatiand,
    prefs: &crate::prefs::Prefs,
    shell: &mut spatiand_shell::Shell,
) {
    state.pip = prefs.pip();
    shell.set_pip(state.pip.corner_index(), state.pip.size == Size::Large);
    log::info!(
        "pinned windows: {} corner, {}",
        state.pip.corner.label().to_lowercase(),
        state.pip.size.label().to_lowercase()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_windows_title_is_read_without_its_computer() {
        assert_eq!(without_host("Picture in picture — workshop"), "Picture in picture");
        assert!(geometry::title_is_pip(without_host("Picture-in-Picture — deepbox")));
        // Only the last is the computer: a page title may have a dash of its own.
        assert_eq!(without_host("a — b — host"), "a — b");
        assert_eq!(without_host("no computer here"), "no computer here");
    }

    #[test]
    fn the_settings_list_and_the_corners_are_numbered_alike() {
        // The shell's rows are written in `Corner::ALL`'s order and know nothing of `Corner`.
        // If either is reordered the list names one corner and moves to another.
        let labels: Vec<String> = Corner::ALL.iter().map(|c| c.label().to_lowercase()).collect();
        let rows = [
            "Pinned windows: bottom right",
            "Pinned windows: bottom left",
            "Pinned windows: top left",
            "Pinned windows: top right",
        ];
        let mut hud = spatiand_shell::Hud::default();
        for (n, corner) in Corner::ALL.iter().enumerate() {
            let settings = Settings { corner: *corner, size: Size::Small };
            assert_eq!(settings.corner_index(), n);
            hud.set_pip(settings.corner_index(), false);
            let row = hud
                .items()
                .iter()
                .find(|i| i.action == spatiand_shell::HudAction::PipCorner)
                .unwrap();
            assert_eq!(row.label, rows[n]);
            assert!(row.label.to_lowercase().ends_with(&labels[n]), "{} vs {}", row.label, labels[n]);
        }
    }
}
