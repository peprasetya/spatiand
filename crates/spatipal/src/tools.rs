//! What SpatiPal can do: the tools its models are given, and doing them.
//!
//! Few and plain, because the everyday model is a small one: one tool for each thing a person
//! does with a window -- read it, look at it, click it, scroll it, type into it -- rather than a
//! protocol's worth of options. The room does them (`spatiand_assist_v1`, through [`Room`]);
//! reminders are kept here.
//!
//! Places on a window are fractions of it, 0 to 1 from its top left: the model is shown a window's
//! picture scaled to fit, and a fraction survives the scaling where a pixel would not.

use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use serde_json::{json, Value};
use spatiand_voice::chat::{tool, ToolCall};

use crate::store::{self, Reminder};

/// The way to the room, from the brain's thread: a question goes to the window's thread, which
/// alone speaks Wayland, and the answer comes back.
#[derive(Clone)]
pub struct Room {
    pub ask: Sender<(String, Sender<String>)>,
    pub wake: std::sync::Arc<dyn Fn() + Send + Sync>,
}

impl Room {
    pub fn ask(&self, what: Value) -> Value {
        let (tx, rx) = channel();
        if self.ask.send((what.to_string(), tx)).is_err() {
            return json!({"error": "the window has gone"});
        }
        (self.wake)();
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(answer) => serde_json::from_str(&answer).unwrap_or_else(|_| json!({"error": "the room's answer was not readable"})),
            Err(_) => json!({"error": "the room did not answer"}),
        }
    }
}

/// The tools, for the small model (`hand_over` among them) or the large.
pub fn all(small: bool) -> Vec<Value> {
    let window = json!({"type": "integer", "description": "The window's number, from the list of windows. Left out: the window the wearer is looking at."});
    let fraction = |what: &str| json!({"type": "number", "description": format!("{what}, as a fraction of the window from 0 to 1, from its top left")});
    let mut tools = vec![
        tool("open_app", "Open an application in the room, by its name.", json!({"name": {"type": "string"}}), &["name"]),
        tool("list_apps", "The applications that can be opened.", json!({}), &[]),
        tool("list_windows", "The windows there are, with their numbers: those on show in the room first, then the hidden ones.", json!({}), &[]),
        tool(
            "window",
            "Do something to a window: focus it, hide it, show it again, pin it to the view, unpin it, or close it. Closing waits for the wearer's yes.",
            json!({"action": {"type": "string", "enum": ["focus", "hide", "show", "pin", "unpin", "close"]}, "window": window}),
            &["action"],
        ),
        tool(
            "look_at_window",
            "See what the wearer sees: a picture of a window as it is shown to them, only the part in view, at the zoom they have. Use this for anything about what they see, and to find where something is before clicking. With view true: everything in view, all the windows.",
            json!({"window": window, "view": {"type": "boolean"}}),
            &[],
        ),
        tool(
            "read_window",
            "A window's own text: a whole web page's words, a terminal's screen and last commands, a folder's names, the field being typed in. It is often much more than is on screen: use it for exact words, or when the wearer asks about what is not in view (the rest of a page, another window). Not to describe what they see.",
            json!({"window": window}),
            &[],
        ),
        tool(
            "click",
            "Click on a window at a place, as the wearer would. Right button for a context menu, count 2 for a double click.",
            json!({"window": window, "x": fraction("Across"), "y": fraction("Down"), "button": {"type": "string", "enum": ["left", "right", "middle"]}, "count": {"type": "integer"}}),
            &["x", "y"],
        ),
        tool(
            "scroll",
            "Scroll a window up, down, left or right, by pages (1 is most of a window).",
            json!({"window": window, "direction": {"type": "string", "enum": ["up", "down", "left", "right"]}, "pages": {"type": "number"}, "x": fraction("Across"), "y": fraction("Down")}),
            &["direction"],
        ),
        tool(
            "drag",
            "Drag on a window from one place to another, the left button held.",
            json!({"window": window, "x": fraction("From across"), "y": fraction("From down"), "to_x": fraction("To across"), "to_y": fraction("To down")}),
            &["x", "y", "to_x", "to_y"],
        ),
        tool("type_text", "Type text into a window, where its cursor is. Does not press Enter.", json!({"text": {"type": "string"}, "window": window}), &["text"]),
        tool(
            "press_keys",
            "Press keys on a window: enter, escape, tab, backspace, up, down, pagedown, f5, or shortcuts like ctrl+l, cmd+c, cmd+shift+t. Enter in a terminal waits for the wearer's yes.",
            json!({"keys": {"type": "string"}, "window": window}),
            &["keys"],
        ),
        tool("recentre", "Bring the room back in front of the wearer.", json!({}), &[]),
        tool(
            "dismiss",
            "Go away: stop listening and leave the room, until the wearer opens you again. For \"you may go\", \"go away\", \"that's all\", \"pergi\", \"sudah, makasih\", \"shut down\". Then say only a very short goodbye.",
            json!({}),
            &[],
        ),
        tool(
            "room_control",
            "Use one of the room's own controls: show or hide the keyboard, move the pinned windows to the next corner, change their size, change the room's look (light, dark), take a screenshot, start or stop recording, show the list of windows, open the Wi-Fi or Bluetooth settings.",
            json!({"action": {"type": "string", "enum": ["keyboard", "pinned_corner", "pinned_size", "look", "screenshot", "record", "windows", "wifi", "bluetooth"]}}),
            &["action"],
        ),
        tool(
            "set_reminder",
            "Remind the wearer of something later: at a time (\"20:00\", \"2026-10-29 08:30\") or in a number of minutes.",
            json!({"text": {"type": "string"}, "at": {"type": "string"}, "in_minutes": {"type": "number"}}),
            &["text"],
        ),
        tool("list_reminders", "The reminders still to come.", json!({}), &[]),
        tool("cancel_reminder", "Cancel a reminder by its number.", json!({"id": {"type": "integer"}}), &["id"]),
    ];
    if small {
        tools.push(tool(
            "hand_over",
            "Hand this request to the larger model, which thinks: for anything that needs careful reasoning, working out, writing code, a long or exact answer, or several steps you are unsure of. Call it at once, saying nothing else.",
            json!({"reason": {"type": "string"}}),
            &[],
        ));
    }
    tools
}

/// What a tool did: what goes back to the model, a picture for it to look at, what the wearer
/// is told it did -- and an action held until the wearer says yes.
pub struct Done {
    pub content: String,
    pub picture: Option<String>,
    pub did: Option<String>,
    pub held: Option<Held>,
}

/// **An action that waits for the wearer's yes**: closing a window, Enter in a terminal. Held
/// here, not by the model -- a model told to ask first and given a "confirmed" to say it had
/// was seen to say it had, and press Enter, without asking. The next thing the wearer says is
/// the answer (`crate::brain`).
#[derive(Clone, Debug)]
pub struct Held {
    pub call: ToolCall,
    /// What it will do, in words, for the wearer.
    pub what: String,
}

fn done(content: impl Into<String>, did: Option<String>) -> Done {
    Done { content: content.into(), picture: None, did, held: None }
}

/// Held, on the window it was meant for: the window resolved now is written into the call, so
/// that a yes acts on the window the wearer was asked about, not on whichever is looked at then.
fn held(call: &ToolCall, what: String, window: Option<u64>) -> Done {
    let mut call = call.clone();
    if let Some(w) = window {
        let mut args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
        if !args.is_object() {
            args = json!({});
        }
        args["window"] = json!(w);
        call.arguments = args.to_string();
    }
    Done {
        content: format!("Held: {what} waits for the wearer's yes. Ask them, in one short sentence, and do nothing else."),
        picture: None,
        did: None,
        held: Some(Held { call, what }),
    }
}

/// Do a tool call; what must wait for the wearer's yes is held.
pub fn run(call: &ToolCall, room: &Room) -> Done {
    run_as(call, room, false)
}

/// Do a held action: the wearer said yes.
pub fn run_held(held: &Held, room: &Room) -> Done {
    run_as(&held.call, room, true)
}

fn run_as(call: &ToolCall, room: &Room, agreed: bool) -> Done {
    let args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
    let get = |k: &str| args.get(k).cloned().unwrap_or(Value::Null);
    // The window acted on: the one named, or the one the room would choose -- the wearer's, never
    // this chat's own -- asked once and used for every step of the call.
    let named = args.get("window").and_then(Value::as_u64);
    let resolved = std::cell::OnceCell::new();
    let window = || *resolved.get_or_init(|| named.or_else(|| room.ask(json!({"do": "target"})).get("window").and_then(Value::as_u64)));
    let with_window = |mut what: Value| {
        if let Some(w) = window() {
            what["window"] = json!(w);
        }
        what
    };
    let answer_of = |answer: Value, did: String| -> Done {
        match answer.get("error").and_then(Value::as_str) {
            Some(e) => done(format!("That did not work: {e}"), None),
            None => done(answer.to_string(), Some(did)),
        }
    };
    match call.name.as_str() {
        "open_app" => {
            let name = get("name").as_str().unwrap_or_default().to_string();
            let answer = room.ask(json!({"do": "open_app", "name": name}));
            let opened = answer.get("opened").and_then(Value::as_str).unwrap_or(&name).to_string();
            answer_of(answer, format!("opened {opened}"))
        }
        "list_apps" => done(room.ask(json!({"do": "apps"})).to_string(), None),
        "dismiss" => done("You will leave once you have said goodbye.".to_string(), Some("left the room".to_string())),
        "list_windows" => {
            let mut answer = room.ask(json!({"do": "windows"}));
            if let Some(list) = answer.get_mut("windows").and_then(Value::as_array_mut) {
                list.sort_by_key(|w| w.get("hidden").and_then(Value::as_bool) == Some(true));
            }
            done(answer.to_string(), None)
        }
        "window" => {
            let action = get("action").as_str().unwrap_or("focus").to_string();
            if action == "close" && !agreed {
                return held(call, format!("closing {}", window_name(room, window())), window());
            }
            let answer = room.ask(with_window(json!({"do": action})));
            answer_of(answer, format!("{action} a window"))
        }
        "read_window" => {
            let mut answer = room.ask(with_window(json!({"do": "describe"})));
            if answer.get("error").is_none() {
                answer["note"] = json!("The window's own text: more than is on screen, perhaps. What the wearer sees is what look_at_window shows.");
            }
            // What a window shows can be long: the end of it is what is being worked on.
            if let Some(text) = answer.get("text").and_then(Value::as_str) {
                if text.chars().count() > 4000 {
                    let tail: String = text.chars().rev().take(4000).collect::<Vec<_>>().into_iter().rev().collect();
                    answer["text"] = json!(tail);
                }
            }
            if let Some(v) = answer.get_mut("vocabulary").and_then(Value::as_array_mut) {
                v.truncate(40);
            }
            done(answer.to_string(), Some("read a window".into()))
        }
        "look_at_window" => {
            // What the room shows the wearer of it -- the part in view -- not the whole window.
            let view = get("view").as_bool() == Some(true);
            let mut what = json!({"do": "seen", "largest": 1024, "view": view});
            if let Some(w) = named {
                what["window"] = json!(w);
            }
            let answer = room.ask(what);
            let looked = answer.get("looking_at").cloned().unwrap_or(Value::Null);
            let (png, about) = match (answer.get("view_png").and_then(Value::as_str), looked.get("png").and_then(Value::as_str)) {
                (Some(png), _) => (Some(png.to_string()), "everything in view, as the wearer sees it".to_string()),
                (None, Some(png)) => {
                    let part = crate::brain::part_words(looked.get("part"));
                    let in_view = looked.get("in_view").and_then(Value::as_bool) != Some(false);
                    let w = looked.get("window").and_then(Value::as_u64).unwrap_or(0);
                    (
                        Some(png.to_string()),
                        if in_view { format!("window {w} as the wearer sees it: {part}") } else { format!("window {w}, all of it -- it is not in view now") },
                    )
                }
                _ => (None, String::new()),
            };
            match png {
                Some(png) => {
                    // Places are fractions of the window, so a picture of a part says which.
                    let part = looked.get("part").cloned().unwrap_or(json!([0.0, 1.0, 0.0, 1.0]));
                    Done {
                        content: format!("The picture follows: {about}. Places to click are fractions of the whole window from its top left; this picture is its part {part} (u0, u1, v0, v1)."),
                        picture: Some(format!("data:image/png;base64,{png}")),
                        did: Some("looked at what the wearer sees".into()),
                        held: None,
                    }
                }
                None => answer_of(answer, String::new()),
            }
        }
        "click" => {
            let button = get("button").as_str().unwrap_or("left").to_string();
            let what = with_window(json!({"do": "click", "x": get("x"), "y": get("y"), "button": button, "count": get("count").as_u64().unwrap_or(1)}));
            answer_of(room.ask(what), if button == "right" { "right-clicked".into() } else { "clicked".into() })
        }
        "scroll" => {
            // A page is most of the window's height; the room scrolls in pixels.
            let size = window_size(room, window());
            let pages = get("pages").as_f64().unwrap_or(1.0).clamp(0.1, 20.0);
            let (dx, dy) = match get("direction").as_str().unwrap_or("down") {
                "up" => (0.0, -pages * size.1 * 0.8),
                "left" => (-pages * size.0 * 0.8, 0.0),
                "right" => (pages * size.0 * 0.8, 0.0),
                _ => (0.0, pages * size.1 * 0.8),
            };
            let what = with_window(json!({"do": "scroll", "x": get("x").as_f64().unwrap_or(0.5), "y": get("y").as_f64().unwrap_or(0.5), "dx": dx, "dy": dy}));
            answer_of(room.ask(what), "scrolled".into())
        }
        "drag" => {
            let what = with_window(json!({"do": "drag", "x": get("x"), "y": get("y"), "to_x": get("to_x"), "to_y": get("to_y")}));
            answer_of(room.ask(what), "dragged".into())
        }
        "type_text" => {
            let text = get("text").as_str().unwrap_or_default().to_string();
            // A line's end typed is Enter pressed (the room types '\n' as Enter): in a terminal it
            // runs what was typed, so it waits for the wearer's yes as Enter does.
            if text.contains(['\n', '\r']) && !agreed && focused_is_terminal(room, window()) {
                return held(call, format!("typing a command and pressing Enter in {}", window_name(room, window())), window());
            }
            // The room's messages are small: long text goes in pieces.
            let chunks: Vec<String> = text.chars().collect::<Vec<_>>().chunks(1500).map(|c| c.iter().collect()).collect();
            let mut last = json!({});
            for chunk in chunks {
                last = room.ask(with_window(json!({"do": "type", "text": chunk})));
                if last.get("error").is_some() {
                    break;
                }
            }
            answer_of(last, "typed".into())
        }
        "press_keys" => {
            let keys = get("keys").as_str().unwrap_or_default().to_string();
            if is_enter(&keys) && !agreed && focused_is_terminal(room, window()) {
                return held(call, format!("pressing Enter in {}", window_name(room, window())), window());
            }
            answer_of(room.ask(with_window(json!({"do": "key", "keys": keys}))), format!("pressed {keys}"))
        }
        "recentre" => answer_of(room.ask(json!({"do": "recentre"})), "recentred the room".into()),
        "room_control" => {
            let action = get("action").as_str().unwrap_or_default().to_string();
            answer_of(room.ask(json!({"do": "control", "action": action})), format!("used the room's {action}"))
        }
        "set_reminder" => {
            let text = get("text").as_str().unwrap_or_default().trim().to_string();
            let due = due(get("in_minutes").as_f64(), get("at").as_str(), store::now());
            match due {
                Some(due) if !text.is_empty() => {
                    let added = store::change_reminders(|all| {
                        let id = all.iter().map(|r| r.id).max().unwrap_or(0) + 1;
                        all.push(Reminder { id, text: text.clone(), due, done: false });
                        id
                    });
                    match added {
                        Ok(id) => done(format!("Reminder {id} set for {}.", crate::time::spoken(due)), Some(format!("set a reminder for {}", crate::time::spoken(due)))),
                        Err(e) => done(format!("The reminder could not be kept: {e}"), None),
                    }
                }
                _ => done("Say when: a time like 20:00 or 2026-10-29 08:30, or a number of minutes.", None),
            }
        }
        "list_reminders" => {
            let upcoming: Vec<Value> = store::reminders()
                .into_iter()
                .filter(|r| !r.done)
                .map(|r| json!({"id": r.id, "text": r.text, "when": crate::time::spoken(r.due)}))
                .collect();
            done(json!({"reminders": upcoming}).to_string(), None)
        }
        "cancel_reminder" => {
            let id = get("id").as_u64().unwrap_or(0);
            let cancelled = store::change_reminders(|all| {
                let before = all.len();
                all.retain(|r| r.id != id);
                all.len() != before
            });
            match cancelled {
                Ok(true) => done(format!("Reminder {id} cancelled."), Some("cancelled a reminder".into())),
                Ok(false) => done(format!("There is no reminder {id}."), None),
                Err(e) => done(format!("The reminder could not be cancelled: {e}"), None),
            }
        }
        other => done(format!("There is no tool {other:?}."), None),
    }
}

/// When a reminder is due: in a number of minutes, or at a time. Minutes of 0 or less beside a
/// time are the model filling in both: the time is meant.
fn due(in_minutes: Option<f64>, at: Option<&str>, now: u64) -> Option<u64> {
    match (in_minutes, at) {
        (Some(m), _) if m > 0.0 => Some(now.saturating_add((m * 60.0) as u64)),
        (_, Some(at)) => crate::time::parse_local(at, now),
        (Some(_), None) => Some(now),
        (None, None) => None,
    }
}

/// A window by the name the wearer knows it by: its application and title.
fn window_name(room: &Room, window: Option<u64>) -> String {
    let list = room.ask(json!({"do": "windows"}));
    let windows = list.get("windows").and_then(Value::as_array).cloned().unwrap_or_default();
    let it = match window {
        Some(w) => windows.iter().find(|x| x.get("window").and_then(Value::as_u64) == Some(w)),
        None => windows.iter().filter(|x| x.get("own").and_then(Value::as_bool) != Some(true)).find(|x| x.get("focused").and_then(Value::as_bool) == Some(true)),
    };
    let app = it.and_then(|x| x.get("app")).and_then(Value::as_str).unwrap_or("the window");
    let app = app.rsplit('.').next().unwrap_or(app);
    match it.and_then(|x| x.get("title")).and_then(Value::as_str).filter(|t| !t.is_empty()) {
        Some(title) => format!("{app} ({title})"),
        None => app.to_string(),
    }
}

/// Whether these keys press Enter as a terminal takes it: Enter or Return with anything held
/// (Shift-Enter, Control-Return), and Control-M and Control-J, which a terminal reads as the
/// carriage return and the line feed.
fn is_enter(keys: &str) -> bool {
    let lower = keys.trim().to_lowercase();
    let parts: Vec<&str> = lower.split('+').map(str::trim).collect();
    let enter = parts.iter().any(|p| matches!(*p, "enter" | "return" | "kp_enter" | "kpenter" | "numpad_enter" | "\n" | "\r"));
    let control = parts.iter().any(|p| matches!(*p, "ctrl" | "control" | "ctl"));
    enter || (control && parts.iter().any(|p| matches!(*p, "m" | "j")))
}

/// Whether the window acted on is a terminal, by the room's account of it. **Unsure is a
/// terminal**: a list the room did not give (it did not answer in time), or a window not in it,
/// and the action waits for the wearer's yes.
fn focused_is_terminal(room: &Room, window: Option<u64>) -> bool {
    let mut what = json!({"do": "windows"});
    if let Some(w) = window {
        what["window"] = json!(w);
    }
    let list = room.ask(what);
    let Some(windows) = list.get("windows").and_then(Value::as_array) else { return true };
    let it = match window {
        Some(w) => windows.iter().find(|x| x.get("window").and_then(Value::as_u64) == Some(w)),
        None => windows.iter().find(|x| x.get("focused").and_then(Value::as_bool) == Some(true)),
    };
    let Some(app) = it.and_then(|x| x.get("app")).and_then(Value::as_str) else { return true };
    is_terminal(app)
}

/// Whether an application is a terminal, by its name or id.
fn is_terminal(app: &str) -> bool {
    const TERMINALS: &[&str] = &[
        "spatiquill", "terminal", "termux", "iterm", "ghostty", "kitty", "alacritty", "wezterm", "konsole", "ptyxis", "xterm", "rxvt", "foot", "tilix", "terminator",
        "kgx", "console", "guake", "yakuake", "tilda", "sakura", "tabby", "hyper", "warp", "contour", "blackbox", "cool-retro-term",
    ];
    let app = app.to_lowercase();
    TERMINALS.iter().any(|t| app.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A room that answers as `answer` says, on a thread of its own.
    fn room(answer: impl Fn(&Value) -> Value + Send + 'static) -> Room {
        let (ask, asked) = channel::<(String, Sender<String>)>();
        std::thread::spawn(move || {
            for (what, reply) in asked {
                let what: Value = serde_json::from_str(&what).unwrap();
                let _ = reply.send(answer(&what).to_string());
            }
        });
        Room { ask, wake: std::sync::Arc::new(|| {}) }
    }

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), arguments: arguments.to_string() }
    }

    #[test]
    fn a_reminder_is_due_when_it_was_asked_for() {
        let now = 1_000_000;
        assert_eq!(due(Some(10.0), None, now), Some(now + 600));
        // Huge minutes: as late as can be, not an overflow.
        assert_eq!(due(Some(1e300), None, now), Some(u64::MAX));
        // in_minutes 0 beside a time: the time.
        assert_eq!(due(Some(0.0), Some("20:00"), now), crate::time::parse_local("20:00", now));
        assert_eq!(due(Some(0.0), None, now), Some(now));
        assert_eq!(due(None, None, now), None);
    }

    #[test]
    fn enter_with_anything_held_or_as_a_control_key_is_enter() {
        for keys in ["enter", "Return", "shift+enter", "ctrl+Return", "ctrl+m", "Control+J", "ctrl + m", "kp_enter"] {
            assert!(is_enter(keys), "{keys:?}");
        }
        for keys in ["escape", "ctrl+l", "m", "j", "cmd+shift+t", "tab"] {
            assert!(!is_enter(keys), "{keys:?}");
        }
    }

    #[test]
    fn a_terminal_by_its_name_and_unsure_is_one() {
        assert!(is_terminal("com.termux") && is_terminal("org.gnome.Console") && is_terminal("foot"));
        assert!(!is_terminal("id.prasetya.spatiand.spatimesh"));
        // The room gone, or not answering: a terminal, to be sure.
        let (ask, asked) = channel();
        drop(asked);
        assert!(focused_is_terminal(&Room { ask, wake: std::sync::Arc::new(|| {}) }, Some(3)));
        let browser = room(|_| json!({"windows": [{"window": 3, "app": "spatimesh", "focused": true}]}));
        assert!(!focused_is_terminal(&browser, Some(3)));
        assert!(focused_is_terminal(&browser, Some(9)));
    }

    #[test]
    fn a_line_typed_into_a_terminal_waits_and_keeps_its_window() {
        let typed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let t = typed.clone();
        let terminal = room(move |what| match what["do"].as_str() {
            Some("target") => json!({"window": 7}),
            Some("windows") => json!({"windows": [{"window": 7, "app": "com.termux", "focused": true}]}),
            _ => {
                t.lock().unwrap().push(what.clone());
                json!({})
            }
        });
        for c in [call("type_text", json!({"text": "rm -rf build\n"})), call("press_keys", json!({"keys": "shift+enter"})), call("press_keys", json!({"keys": "ctrl+j"})), call("window", json!({"action": "close"}))] {
            let held = run(&c, &terminal).held.expect("held");
            // The window it was asked about, written into the call: a yes acts on that one.
            assert_eq!(serde_json::from_str::<Value>(&held.call.arguments).unwrap()["window"], 7, "{}", c.name);
        }
        assert!(typed.lock().unwrap().is_empty(), "nothing typed or pressed before the yes");
        // Typed without a line's end, it is typed.
        assert!(run(&call("type_text", json!({"text": "ls"})), &terminal).held.is_none());
        assert_eq!(typed.lock().unwrap().len(), 1);
    }
}

/// A window's size in pixels, for scrolling by pages.
fn window_size(room: &Room, window: Option<u64>) -> (f64, f64) {
    let list = room.ask(json!({"do": "windows"}));
    let windows = list.get("windows").and_then(Value::as_array).cloned().unwrap_or_default();
    let it = match window {
        Some(w) => windows.iter().find(|x| x.get("window").and_then(Value::as_u64) == Some(w)),
        None => windows.iter().find(|x| x.get("focused").and_then(Value::as_bool) == Some(true)),
    };
    it.and_then(|x| x.get("size"))
        .and_then(Value::as_array)
        .and_then(|s| Some((s.first()?.as_f64()?, s.get(1)?.as_f64()?)))
        .unwrap_or((1200.0, 800.0))
}
