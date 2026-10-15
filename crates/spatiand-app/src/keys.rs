//! What a key means.
//!
//! The wire carries a key's position; what is printed on it is the keymap's to say, and the
//! compositor sends its keymap for exactly that. libxkbcommon reads it, as it does for every
//! other Wayland client, so a Spatiand application types what the rest of the session types.

use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use xkbcommon::xkb;

use crate::{Event, Key, Modifiers, WindowId};

pub(crate) struct Keyboard {
    context: xkb::Context,
    state: Option<xkb::State>,
    /// Keys a second, and the wait before the first repeat.
    rate: u32,
    delay: Duration,
    /// The key being held, and when it next repeats.
    held: Option<(u32, Instant)>,
}

impl Default for Keyboard {
    fn default() -> Self {
        Keyboard {
            context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            state: None,
            rate: 25,
            delay: Duration::from_millis(400),
            held: None,
        }
    }
}

impl Keyboard {
    pub(crate) fn keymap(&mut self, fd: OwnedFd, size: u32) {
        // SAFETY: the compositor's descriptor is a keymap of `size` bytes, as the protocol says.
        let keymap = unsafe {
            xkb::Keymap::new_from_fd(&self.context, fd, size as usize, xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS)
        };
        match keymap {
            Ok(Some(keymap)) => self.state = Some(xkb::State::new(&keymap)),
            _ => log::warn!("the compositor's keymap could not be read; keys will have no letters"),
        }
    }

    pub(crate) fn modifiers(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        if let Some(state) = self.state.as_mut() {
            state.update_mask(depressed, latched, locked, 0, 0, group);
        }
    }

    /// The modifiers held now, as the compositor last said: for a click, which a key event does
    /// not come with.
    pub(crate) fn held_modifiers(&self) -> Modifiers {
        let Some(state) = self.state.as_ref() else { return Modifiers::default() };
        let on = |name: &str| state.mod_name_is_active(name, xkb::STATE_MODS_DEPRESSED) || state.mod_name_is_active(name, xkb::STATE_MODS_LATCHED);
        Modifiers { shift: on(xkb::MOD_NAME_SHIFT), control: on(xkb::MOD_NAME_CTRL), alt: on(xkb::MOD_NAME_ALT), logo: on(xkb::MOD_NAME_LOGO) }
    }

    pub(crate) fn repeat_info(&mut self, rate: i32, delay: i32) {
        self.rate = rate.max(0) as u32;
        self.delay = Duration::from_millis(delay.max(0) as u64);
    }

    /// Focus has gone: nothing is held any more, whatever the hand is doing.
    pub(crate) fn let_go(&mut self) {
        self.held = None;
    }

    fn event(&self, code: u32, pressed: bool, repeat: bool, window: WindowId) -> Event {
        let mut key = Key::Other;
        let mut text = None;
        let mut modifiers = Modifiers::default();
        if let Some(state) = self.state.as_ref() {
            // An evdev code, eight short of the keymap's own numbering.
            let keycode = xkb::Keycode::new(code + 8);
            let on = |name: &str| state.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE);
            modifiers = Modifiers {
                shift: on(xkb::MOD_NAME_SHIFT),
                control: on(xkb::MOD_NAME_CTRL),
                alt: on(xkb::MOD_NAME_ALT),
                logo: on(xkb::MOD_NAME_LOGO),
            };
            key = named(state.key_get_one_sym(keycode));
            if pressed && !modifiers.control && !modifiers.logo {
                let typed = state.key_get_utf8(keycode);
                // A key that is an action -- Return, Escape, Backspace -- has a control
                // character for its text, which nobody means to type into a field.
                if !typed.is_empty() && !typed.chars().any(char::is_control) {
                    text = Some(typed);
                }
            }
        }
        Event::Key { window, key, code, pressed, repeat, text, modifiers }
    }

    pub(crate) fn key(&mut self, code: u32, pressed: bool, window: WindowId, events: &mut Vec<Event>) {
        events.push(self.event(code, pressed, false, window));
        let repeats = self
            .state
            .as_ref()
            .map_or(true, |state| state.get_keymap().key_repeats(xkb::Keycode::new(code + 8)));
        if pressed && repeats && self.rate > 0 {
            self.held = Some((code, Instant::now() + self.delay));
        } else if self.held.is_some_and(|(held, _)| held == code) || pressed {
            self.held = None;
        }
    }

    pub(crate) fn next_repeat(&self) -> Option<Instant> {
        self.held.map(|(_, at)| at)
    }

    pub(crate) fn repeat(&mut self, now: Instant, window: WindowId, events: &mut Vec<Event>) {
        let Some((code, mut at)) = self.held else { return };
        let step = Duration::from_secs_f64(1.0 / self.rate.max(1) as f64);
        // At most a few at once: an application that stalled is not owed every repeat it missed.
        let mut made = 0;
        while at <= now && made < 4 {
            events.push(self.event(code, true, true, window));
            at += step;
            made += 1;
        }
        self.held = Some((code, at.max(now)));
    }
}

fn named(sym: xkb::Keysym) -> Key {
    use xkb::Keysym as S;
    match sym {
        S::Return | S::KP_Enter => Key::Enter,
        S::Escape => Key::Escape,
        S::BackSpace => Key::Backspace,
        S::Delete | S::KP_Delete => Key::Delete,
        S::Tab | S::ISO_Left_Tab => Key::Tab,
        S::space => Key::Space,
        S::Left | S::KP_Left => Key::Left,
        S::Right | S::KP_Right => Key::Right,
        S::Up | S::KP_Up => Key::Up,
        S::Down | S::KP_Down => Key::Down,
        S::Home | S::KP_Home => Key::Home,
        S::End | S::KP_End => Key::End,
        S::Page_Up | S::KP_Page_Up => Key::PageUp,
        S::Page_Down | S::KP_Page_Down => Key::PageDown,
        other => match other.key_char() {
            Some(c) if !c.is_control() => Key::Char(c.to_lowercase().next().unwrap_or(c)),
            _ => Key::Other,
        },
    }
}
