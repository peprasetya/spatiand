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
    /// The right Control key is down: a Mac's Command, as its room hands either Command key to
    /// its clients (`crates/spatiand-mac/src/mac/ffi.rs`), and the right Control elsewhere.
    right_control: bool,
    /// The left Control's Q is the application's, not a quit: a terminal's (XON, readline's
    /// quoted-insert). See [`quits`].
    pub(crate) control_q_kept: bool,
}

impl Default for Keyboard {
    fn default() -> Self {
        Keyboard {
            // Without the machine's XKB folders: the compositor's keymap is whole, and with
            // them a program that cannot find them (one started without XKB_CONFIG_ROOT on
            // Android) had no context, and crashed on its first keyboard.
            context: xkb::Context::new(xkb::CONTEXT_NO_DEFAULT_INCLUDES),
            state: None,
            rate: 25,
            delay: Duration::from_millis(400),
            held: None,
            right_control: false,
            control_q_kept: false,
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
        let control = on(xkb::MOD_NAME_CTRL);
        Modifiers { shift: on(xkb::MOD_NAME_SHIFT), control, alt: on(xkb::MOD_NAME_ALT), logo: on(xkb::MOD_NAME_LOGO), command: control && self.right_control }
    }

    pub(crate) fn repeat_info(&mut self, rate: i32, delay: i32) {
        self.rate = rate.max(0) as u32;
        self.delay = Duration::from_millis(delay.max(0) as u64);
    }

    /// Focus has gone: nothing is held any more, whatever the hand is doing.
    pub(crate) fn let_go(&mut self) {
        self.held = None;
        self.right_control = false;
    }

    /// Focus has come, with these keys already down (`wl_keyboard.enter`).
    pub(crate) fn entered(&mut self, down: &[u32]) {
        self.right_control = down.contains(&RIGHT_CONTROL);
    }

    fn event(&self, code: u32, pressed: bool, repeat: bool, window: WindowId) -> Event {
        let mut key = Key::Other;
        let mut text = None;
        let mut modifiers = Modifiers::default();
        if let Some(state) = self.state.as_ref() {
            // An evdev code, eight short of the keymap's own numbering.
            let keycode = xkb::Keycode::new(code + 8);
            let on = |name: &str| state.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE);
            let control = on(xkb::MOD_NAME_CTRL);
            modifiers = Modifiers {
                shift: on(xkb::MOD_NAME_SHIFT),
                control,
                alt: on(xkb::MOD_NAME_ALT),
                logo: on(xkb::MOD_NAME_LOGO),
                command: control && self.right_control,
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
        if code == RIGHT_CONTROL {
            self.right_control = pressed;
        }
        let event = self.event(code, pressed, false, window);
        // Quitting is the room's one chord for every application (`quits`): it comes as
        // `Event::Quit` and not as the key, and is not repeated for being held.
        if let Event::Key { key, pressed: true, modifiers, .. } = &event {
            if quits(*key, code, *modifiers, self.control_q_kept) {
                events.push(Event::Quit { window });
                self.held = None;
                return;
            }
        }
        events.push(event);
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

/// The right Control key's evdev code: the key a Mac's room hands either Command key to its
/// clients as.
pub(crate) const RIGHT_CONTROL: u32 = 97;

/// Whether a key pressed with these held quits the application (the owner, 2026-11-16:
/// "cmd/ctrl-Q to quit for all spatiand-app"):
///
/// * Command-Q -- the right Control's Q, as a Mac's Command reaches its clients -- always;
///   the logo key's Q too, where a keyboard's Super reaches the application.
/// * Control-Q (the left Control): unless the application keeps it for itself
///   ([`crate::App::keep_control_q`]: a terminal, whose programs have it as XON or
///   quoted-insert), and then Control-Shift-Q.
/// * Never with Alt held: a chord of its own.
/// The key is Q by [`chord_char`]: on a layout that is not Latin, the key where Q is.
pub(crate) fn quits(key: Key, code: u32, m: Modifiers, control_q_kept: bool) -> bool {
    if !matches!(key, Key::Char(c) if chord_char(c, code) == 'q') || m.alt {
        return false;
    }
    m.command || m.logo || (m.control && (m.shift || !control_q_kept))
}

/// The Latin letter on the key at evdev `code`, by its place on a US keyboard: the letter a
/// chord is read by when the layout's own is not Latin (Cyrillic, Greek, Hebrew, Arabic...),
/// as every terminal reads Control-C there.
fn letter_at(code: u32) -> Option<char> {
    const ROWS: [(u32, &str); 3] = [(16, "qwertyuiop"), (30, "asdfghjkl"), (44, "zxcvbnm")];
    ROWS.iter().find_map(|&(first, letters)| letters.chars().nth(code.checked_sub(first)? as usize))
}

/// **The character a chord is read by**, for every application: the key's own, in lower case;
/// and where that is a letter outside Latin's, the Latin letter in its place (by the key's
/// place on a US keyboard), so that Control-C, Command-Q and Control-V are where they always
/// are on a Russian, Greek or Hebrew layout. A layout's own Latin letters (AZERTY's A where
/// QWERTY's Q is, a German ü) stay theirs. `code` is the key's evdev code (`Event::Key`'s).
pub fn chord_char(c: char, code: u32) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    if c.is_ascii() || !c.is_alphabetic() {
        return c;
    }
    letter_at(code).filter(|_| !is_latin(c)).unwrap_or(c)
}

/// A letter of the Latin alphabet's own, with whatever marks: ü, é, ß, ø.
fn is_latin(c: char) -> bool {
    matches!(c as u32, 0x00C0..=0x024F | 0x1E00..=0x1EFF)
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

#[cfg(test)]
mod quit_tests {
    use super::*;

    fn held(shift: bool, control: bool, alt: bool, logo: bool, command: bool) -> Modifiers {
        Modifiers { shift, control, alt, logo, command }
    }

    #[test]
    fn control_q_and_command_q_quit() {
        let q = Key::Char('q');
        // The left Control, the right (a Mac's Command), Control-Shift, the logo key.
        assert!(quits(q, 16, held(false, true, false, false, false), false));
        assert!(quits(q, 16, held(false, true, false, false, true), false));
        assert!(quits(q, 16, held(true, true, false, false, false), false));
        assert!(quits(q, 16, held(false, false, false, true, false), false));
        // A plain Q, a Shift-Q, an Alt chord, another letter: no.
        assert!(!quits(q, 16, Modifiers::default(), false));
        assert!(!quits(q, 16, held(true, false, false, false, false), false));
        assert!(!quits(q, 16, held(false, true, true, false, false), false));
        assert!(!quits(q, 16, held(false, true, true, false, true), false));
        assert!(!quits(Key::Char('w'), 17, held(false, true, false, false, true), false));
    }

    #[test]
    fn command_q_quits_on_a_layout_that_is_not_latin() {
        // Russian: Й is on Q's key (16). AZERTY's A is there too, and is its own: no quit.
        assert!(quits(Key::Char('\u{439}'), 16, held(false, true, false, false, true), false));
        assert!(quits(Key::Char('\u{419}'), 16, held(false, true, false, false, false), false));
        assert!(!quits(Key::Char('a'), 16, held(false, true, false, false, true), false));
        assert!(!quits(Key::Char('\u{446}'), 17, held(false, true, false, false, true), false));
    }

    #[test]
    fn chords_go_by_the_key_s_place_where_the_layout_is_not_latin() {
        // Russian: С is on C's key (46), В on V's (47); Greek τ on T's (20).
        assert_eq!(chord_char('\u{441}', 46), 'c');
        assert_eq!(chord_char('\u{421}', 46), 'c');
        assert_eq!(chord_char('\u{432}', 47), 'v');
        assert_eq!(chord_char('\u{3c4}', 20), 't');
        // A layout's own Latin letters are theirs: AZERTY's A on Q's key, a German ü.
        assert_eq!(chord_char('a', 16), 'a');
        assert_eq!(chord_char('\u{fc}', 26), '\u{fc}');
    }

    #[test]
    fn a_terminal_keeps_the_left_control_q() {
        let q = Key::Char('q');
        // Its program has Control-Q (XON, quoted-insert) ...
        assert!(!quits(q, 16, held(false, true, false, false, false), true));
        // ... and Command-Q, Control-Shift-Q and the logo key's Q still quit.
        assert!(quits(q, 16, held(false, true, false, false, true), true));
        assert!(quits(q, 16, held(true, true, false, false, false), true));
        assert!(quits(q, 16, held(false, false, false, true, false), true));
    }

    #[test]
    fn the_quit_chord_comes_as_quit_and_once() {
        let mut keyboard = Keyboard::default();
        let mut events = Vec::new();
        // No keymap: the keys have no letters, and nothing quits.
        keyboard.key(16, true, WindowId(1), &mut events);
        assert!(matches!(events[0], Event::Key { .. }));
        assert!(!keyboard.right_control);
        keyboard.key(RIGHT_CONTROL, true, WindowId(1), &mut events);
        assert!(keyboard.right_control);
        keyboard.let_go();
        assert!(!keyboard.right_control);
        keyboard.entered(&[RIGHT_CONTROL]);
        assert!(keyboard.right_control);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::FromRawFd;

    /// A keyboard given a keymap the way the compositor gives one: as text in a file
    /// descriptor. The machine's own XKB data, for a German layout (a Z where a US Y is, an
    /// umlaut, AltGr) -- what a client of the Deck's room types with.
    fn keyboard() -> Option<Keyboard> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(&context, "evdev", "pc105", "de", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)?;
        let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        // SAFETY: a new anonymous file, owned from here on.
        let fd = unsafe { libc::memfd_create(c"keymap".as_ptr(), 0) };
        assert!(fd >= 0);
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.write_all(text.as_bytes()).unwrap();
        file.write_all(&[0]).unwrap();
        let mut keyboard = Keyboard::default();
        keyboard.keymap(file.into(), text.len() as u32 + 1);
        Some(keyboard)
    }

    fn typed(keyboard: &Keyboard, code: u32) -> (Key, Option<String>) {
        match keyboard.event(code, true, false, WindowId(1)) {
            Event::Key { key, text, .. } => (key, text),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn keys_mean_what_the_keymap_says() {
        let Some(mut keyboard) = keyboard() else {
            eprintln!("no XKB data on this machine; nothing to check");
            return;
        };
        // evdev codes: 21 Y (Z on a German keyboard), 26 [ (Ü), 28 Return, 16 Q (@ with AltGr).
        assert_eq!(typed(&keyboard, 21), (Key::Char('z'), Some("z".into())));
        assert_eq!(typed(&keyboard, 26), (Key::Char('ü'), Some("ü".into())));
        assert_eq!(typed(&keyboard, 28), (Key::Enter, None));
        // Shift (mod index 0) and AltGr (Mod5, index 7) as the compositor sends them.
        keyboard.modifiers(1 << 0, 0, 0, 0);
        assert_eq!(typed(&keyboard, 21), (Key::Char('z'), Some("Z".into())));
        assert!(keyboard.held_modifiers().shift);
        keyboard.modifiers(1 << 7, 0, 0, 0);
        assert_eq!(typed(&keyboard, 16).1.as_deref(), Some("@"));
        keyboard.modifiers(0, 0, 0, 0);
        assert!(!keyboard.held_modifiers().shift);
    }

    #[test]
    fn command_q_is_a_quit_and_the_right_control_is_command() {
        let Some(mut keyboard) = keyboard() else {
            eprintln!("no XKB data on this machine; nothing to check");
            return;
        };
        let mut events = Vec::new();
        // The right Control down (Control is mod index 2), then Q (evdev 16).
        keyboard.key(RIGHT_CONTROL, true, WindowId(1), &mut events);
        keyboard.modifiers(1 << 2, 0, 0, 0);
        assert!(keyboard.held_modifiers().command);
        events.clear();
        keyboard.key(16, true, WindowId(1), &mut events);
        assert_eq!(events, vec![Event::Quit { window: WindowId(1) }]);
        // Held, it does not quit again and again.
        assert!(keyboard.next_repeat().is_none());
        keyboard.key(16, false, WindowId(1), &mut events);
        // The right Control let go of, the left one's Q quits too -- unless it is kept.
        keyboard.key(RIGHT_CONTROL, false, WindowId(1), &mut events);
        assert!(!keyboard.held_modifiers().command);
        events.clear();
        keyboard.key(16, true, WindowId(1), &mut events);
        assert_eq!(events, vec![Event::Quit { window: WindowId(1) }]);
        keyboard.control_q_kept = true;
        events.clear();
        keyboard.key(16, true, WindowId(1), &mut events);
        assert!(matches!(&events[..], [Event::Key { key: Key::Char('q'), modifiers: Modifiers { control: true, command: false, .. }, .. }]));
        // Another letter with Command is the application's key, Command and all.
        keyboard.key(RIGHT_CONTROL, true, WindowId(1), &mut events);
        events.clear();
        keyboard.key(17, true, WindowId(1), &mut events);
        assert!(matches!(&events[..], [Event::Key { key: Key::Char('w'), modifiers: Modifiers { control: true, command: true, .. }, .. }]));
    }
}
