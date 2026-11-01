//! Dictation on the Deck's desktop, with no room: `spatiand --dictation-service`, switched on and
//! off from the desktop's menu (`tools/install-desktop-apps.sh`). While it runs the microphone is
//! open and what is said is typed into the front window. A terminal's words wait on a notification,
//! as they wait on the room's card. Ends when the desktop's compositor does.
//!
//! Plasma lets no ordinary program type into other windows, so the keys come from a keyboard of
//! its own on uinput, which SteamOS lets the desktop's user make; its keys are read through the
//! layout set in Plasma, and are chosen as for US. The front window is had from a KWin script,
//! whose only way out is D-Bus: it calls the bus itself with a name nothing answers, and
//! `dbus-monitor` here reads the call.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use spatiand_shell::keyboard as kb;
use spatiand_voice::Description;

use crate::dictate::{self, Asked, Said};

const SCRIPT: &str = r#"function tell(w) {
    if (w) callDBus("org.freedesktop.DBus", "/", "id.prasetya.spatiand.Dictation", "Focus",
                    String(w.resourceClass), String(w.caption), String(w.internalId));
}
workspace.windowActivated.connect(tell);
tell(workspace.activeWindow);
"#;
const SCRIPT_NAME: &str = "spatiand-dictation";

pub fn run() -> Result<(), String> {
    let desktop = crate::spatikey::desktop_connection()?;
    let keyboard = Arc::new(Mutex::new(Keyboard::new()?));
    let (ask, asked) = mpsc::channel();
    follow_the_front_window(ask.clone())?;
    let _ = ask.send(Asked::Listen(true));
    std::thread::spawn(move || {
        dictate::on_a_desktop(asked, move |said| match said {
            Said::Text(words) => keyboard.lock().unwrap().type_text(&words),
            Said::Review(words) => {
                let keyboard = keyboard.clone();
                std::thread::spawn(move || review(&keyboard, &words));
            }
            Said::Trouble(why) => notify(&why),
            Said::Refused(why) => {
                notify(&why);
                std::process::exit(1);
            }
        });
    });
    crate::spatikey::until_it_ends(desktop);
    Ok(())
}

fn notify(words: &str) {
    let _ = Command::new("notify-send").args(["-a", "Dictation", "Dictation", words]).status();
}

/// A terminal's words: typed if its owner says so, where a misheard `rm` can be seen first.
fn review(keyboard: &Mutex<Keyboard>, words: &str) {
    let answer = Command::new("notify-send")
        .args(["-a", "Dictation", "-u", "critical", "-w", "-A", "type=Type", "-A", "discard=Discard"])
        .args(["Type into the terminal?", words])
        .output();
    if answer.is_ok_and(|a| String::from_utf8_lossy(&a.stdout).trim() == "type") {
        keyboard.lock().unwrap().type_text(words);
    }
}

fn follow_the_front_window(ask: Sender<Asked>) -> Result<(), String> {
    let mut monitor = Command::new("dbus-monitor")
        .args(["--session", "interface=id.prasetya.spatiand.Dictation"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start dbus-monitor: {e}"))?;
    let out = monitor.stdout.take().ok_or("dbus-monitor has no output")?;
    std::thread::spawn(move || {
        let mut said: Vec<String> = Vec::new();
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let line = line.trim();
            if line.starts_with("method call") {
                said.clear();
            } else if let Some(words) = line.strip_prefix("string \"").and_then(|l| l.strip_suffix('"')) {
                said.push(words.to_string());
                if let [class, caption, id] = said.as_slice() {
                    log::info!("dictation: {class} is in front");
                    let _ = ask.send(Asked::Focus(described(class, caption), number_of(id)));
                }
            }
        }
    });
    // Given time to listen, as the script says what is in front only as it starts.
    std::thread::sleep(Duration::from_millis(200));
    load_script()
}

fn described(class: &str, caption: &str) -> Description {
    let mut focus = Description::titled(class, caption);
    if dictate::is_terminal(class) {
        focus.kind = "terminal".into();
    }
    focus
}

fn number_of(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hash);
    hash.finish()
}

fn load_script() -> Result<(), String> {
    let kwin = |object: &str, method: &str, args: &[&str]| {
        Command::new("gdbus")
            .args(["call", "--session", "-d", "org.kde.KWin", "-o", object, "-m", method])
            .args(args)
            .output()
            .map_err(|e| format!("could not reach KWin: {e}"))
    };
    let path = std::env::temp_dir().join(format!("{SCRIPT_NAME}-{}.js", std::process::id()));
    std::fs::write(&path, SCRIPT).map_err(|e| e.to_string())?;
    // One left by a dictation that was stopped: the name is still taken.
    kwin("/Scripting", "org.kde.kwin.Scripting.unloadScript", &[SCRIPT_NAME])?;
    let loaded = kwin("/Scripting", "org.kde.kwin.Scripting.loadScript", &[&path.to_string_lossy(), SCRIPT_NAME])?;
    let reply = String::from_utf8_lossy(&loaded.stdout).to_string();
    let number: String = reply.chars().filter(char::is_ascii_digit).collect();
    if number.is_empty() {
        return Err(format!("KWin would not load the script: {}", String::from_utf8_lossy(&loaded.stderr)));
    }
    kwin(&format!("/Scripting/Script{number}"), "org.kde.kwin.Script.run", &[])?;
    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// A keyboard on uinput.
struct Keyboard {
    device: std::fs::File,
}

const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_DEV_SETUP: libc::c_ulong = 0x405C_5503;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;

/// The kernel's `uinput_setup`.
#[repr(C)]
struct Setup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

/// The kernel's `input_event`, on a 64-bit machine.
#[repr(C)]
struct InputEvent {
    seconds: i64,
    microseconds: i64,
    kind: u16,
    code: u16,
    value: i32,
}

impl Keyboard {
    fn new() -> Result<Keyboard, String> {
        use std::os::fd::AsRawFd;
        let device = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .map_err(|e| format!("could not open /dev/uinput, to type with: {e}"))?;
        let fd = device.as_raw_fd();
        let mut setup = Setup { bustype: 0x06, vendor: 0x5350, product: 0x4449, version: 1, name: [0; 80], ff_effects_max: 0 };
        let name = b"Spatiand dictation";
        setup.name[..name.len()].copy_from_slice(name);
        // SAFETY: the file is open, and each call is given what the kernel's header says it takes.
        let made = unsafe {
            let mut made = libc::ioctl(fd, UI_SET_EVBIT, EV_KEY as libc::c_int) == 0;
            for key in 1..=248 {
                made &= libc::ioctl(fd, UI_SET_KEYBIT, key as libc::c_int) == 0;
            }
            made && libc::ioctl(fd, UI_DEV_SETUP, &setup as *const Setup) == 0 && libc::ioctl(fd, UI_DEV_CREATE) == 0
        };
        if !made {
            return Err(format!("could not make a keyboard: {}", std::io::Error::last_os_error()));
        }
        // The desktop has to find it before it can be listened to.
        std::thread::sleep(Duration::from_millis(500));
        Ok(Keyboard { device })
    }

    fn type_text(&mut self, words: &str) {
        for c in dictate::typeable(words).chars() {
            let Some(stroke) = kb::stroke_for(c) else {
                log::debug!("dictation: {c:?} is not on the keyboard");
                continue;
            };
            if stroke.shift {
                self.key(kb::KEY_LEFTSHIFT, true);
            }
            self.key(stroke.code, true);
            self.key(stroke.code, false);
            if stroke.shift {
                self.key(kb::KEY_LEFTSHIFT, false);
            }
        }
    }

    fn key(&mut self, code: u32, down: bool) {
        self.event(EV_KEY, code as u16, down as i32);
        self.event(EV_SYN, 0, 0);
        std::thread::sleep(Duration::from_millis(3));
    }

    fn event(&mut self, kind: u16, code: u16, value: i32) {
        let event = InputEvent { seconds: 0, microseconds: 0, kind, code, value };
        // SAFETY: a plain struct of integers, read as the bytes it is.
        let bytes = unsafe { std::slice::from_raw_parts((&event as *const InputEvent).cast::<u8>(), std::mem::size_of::<InputEvent>()) };
        if let Err(e) = self.device.write_all(bytes) {
            log::warn!("dictation: the keyboard refused a key: {e}");
        }
    }
}
