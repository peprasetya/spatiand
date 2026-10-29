//! SpatiKey in SpatiMesh's window (docs/spatikey.md, "SpatiMesh"): what the frames' isolated world
//! says, the key button, choosing between accounts, and asking to save a login.
//!
//! The pages' half is `webauthn.js` (the page's world) and `forms.js` + `cred-world.js` (the
//! isolated world `spatimesh-cred`); their messages come as `Happened::Cred`, stamped by that
//! world with the frame's origin, and each waits for an answer (`ToHost::CredReply`). What needs
//! the vault is asked of the SpatiKey service on a thread of its own -- a person may be touching
//! a sensor, here or on a Mac across the room -- and comes back through `Keys::rx`.

use std::collections::HashMap;
use std::sync::mpsc;

use serde_json::{json, Value};
use spatikey_core::b64;
use spatikey_core::site;
use spatikey_core::wire::{self, Code, Refusal, Request};
use spatiand_app::ui::Row;
use spatimesh::engine::{Secret, TabId};
use spatimesh::wire::ToHost;

use super::{Action, Browser, Own};

/// A frame's way to be told something later: its `ready` message, not yet answered.
struct Frame {
    request: u64,
    origin: String,
}

#[derive(Default)]
pub(super) struct TabKeys {
    /// Every frame of the tab with SpatiKey's script in it, by its own id.
    frames: HashMap<String, Frame>,
    /// The frame with a login field in focus, and its origin.
    focused: Option<(String, String)>,
    /// A passkey offered in a login field (`mediation: "conditional"`): it waits for the key
    /// button. The request, its frame's origin, and what the page asked.
    conditional: Option<(u64, String, Value)>,
    /// The page's own ids for what it asked and has not been answered, by frame: so an abort
    /// can answer them.
    asked: HashMap<(String, String), u64>,
    /// The login SpatiKey last filled in this tab: sending it is not a login to save.
    filled: Option<(String, Secret)>,
}

pub(super) enum Done {
    /// Answer a frame's request.
    Reply { tab: TabId, request: u64, ok: bool, json: String },
    /// The logins for the key button.
    Logins { tab: TabId, frame: String, origin: String, logins: Result<Vec<wire::Login>, Refusal> },
    /// A passkey with several accounts: the wearer chooses which.
    Choices { tab: TabId, request: u64, client_data: String, choices: Vec<wire::Choice> },
    /// Something to say in a toast.
    Said(String),
}

/// What the account list open over a window is for.
enum Choosing {
    Logins { tab: TabId, frame: String, logins: Vec<wire::Login> },
    Passkeys { tab: TabId, request: u64, client_data: String, choices: Vec<wire::Choice> },
}

pub(super) struct Keys {
    tx: mpsc::Sender<Done>,
    rx: mpsc::Receiver<Done>,
    tabs: HashMap<TabId, TabKeys>,
    /// Requests out at the service, not yet back.
    asking: usize,
    choosing: Option<Choosing>,
}

impl Keys {
    pub(super) fn new() -> Keys {
        let (tx, rx) = mpsc::channel();
        Keys { tx, rx, tabs: HashMap::new(), asking: 0, choosing: None }
    }

    /// Waiting on the service: the window looks for its answers often.
    pub(super) fn busy(&self) -> bool {
        self.asking > 0
    }

    /// The tab has a login field in focus (its key button lit).
    pub(super) fn has_login(&self, tab: TabId) -> bool {
        self.tabs.get(&tab).is_some_and(|t| t.focused.is_some() || t.conditional.is_some())
    }

    pub(super) fn forget(&mut self, tab: TabId) {
        self.tabs.remove(&tab);
    }

    /// Ask the service on a thread of its own; `then` makes what comes back of the answer.
    fn ask(&mut self, request: Request, then: impl FnOnce(Result<Value, Refusal>) -> Done + Send + 'static) {
        let tx = self.tx.clone();
        self.asking += 1;
        std::thread::spawn(move || {
            let answer = match spatikey_core::socket::Client::connect("SpatiMesh") {
                Ok(mut client) => client.ask(request),
                Err(e) => Err(Refusal::new(Code::NotHere, format!("SpatiKey is not running here: {e}"))),
            };
            let _ = tx.send(then(answer));
        });
    }
}

/// What the page is told when SpatiKey says no: a DOMException's name, as WebAuthn names it.
fn dom_error(refusal: &Refusal) -> String {
    let name = match refusal.code {
        Code::Excluded => "InvalidStateError",
        Code::BadRequest => "NotSupportedError",
        Code::Failed => "UnknownError",
        _ => "NotAllowedError",
    };
    json!({"err": {"name": name, "code": refusal.code, "message": refusal.message}}).to_string()
}

fn refused(name: &str, message: &str) -> String {
    json!({"err": {"name": name, "message": message}}).to_string()
}

/// What to call a site in a sentence: its host.
fn site_name(origin: &str) -> String {
    site::host_of(origin).unwrap_or_else(|| origin.to_string())
}

impl Browser {
    fn cred_reply(&mut self, tab: TabId, request: u64, ok: bool, json: String) {
        self.tell(tab, ToHost::CredReply { request, ok, json: Secret(json) });
    }

    /// A frame's message (`cred-world.js`).
    pub(super) fn heard_cred(&mut self, tab: TabId, request: u64, json: &str) {
        let Ok(body) = serde_json::from_str::<Value>(json) else {
            return self.cred_reply(tab, request, false, "not JSON".into());
        };
        let text = |name: &str| body.get(name).and_then(Value::as_str).unwrap_or("").to_string();
        let (kind, origin, frame) = (text("t"), text("origin"), text("frame"));
        let main = body.get("main").and_then(Value::as_bool).unwrap_or(false);
        let top_origin = text("top_origin");
        match kind.as_str() {
            "ready" => {
                // A frame's way to be told later. A frame posting again has had its last one
                // answered (or is a new document of it).
                let keys = self.keys.tabs.entry(tab).or_default();
                if let Some(old) = keys.frames.insert(frame, Frame { request, origin }) {
                    self.cred_reply(tab, old.request, true, json!({"stop": true}).to_string());
                }
            }
            "fields" => {
                // `kind` is the login field that has the focus now ("username", "password",
                // "new-password", "otp"), or empty once the focus has left them.
                let keys = self.keys.tabs.entry(tab).or_default();
                let focused = !text("kind").is_empty();
                if focused && site::is_secure_origin(&origin) {
                    keys.focused = Some((frame, origin));
                } else if keys.focused.as_ref().is_some_and(|(f, _)| *f == frame) {
                    keys.focused = None;
                }
                self.cred_reply(tab, request, true, "{}".into());
                self.dirty_tab(tab);
            }
            "submitted" => {
                self.cred_reply(tab, request, true, "{}".into());
                let (username, password) = (text("username"), text("password"));
                let filled = self.keys.tabs.get(&tab).and_then(|k| k.filled.as_ref());
                let ours = filled.is_some_and(|(u, p)| *u == username && p.0 == password);
                if password.is_empty() || ours || !site::is_secure_origin(&origin) {
                    return;
                }
                let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) else { return };
                let who = if username.is_empty() { String::new() } else { format!(" as {username}") };
                let message = format!("Keep the password you just used for {}{who} in SpatiKey?", site_name(&origin));
                self.ask_own_confirm(w, Own::SaveLogin(origin, username, Secret(password)), &message);
            }
            "abort" => {
                self.cred_reply(tab, request, true, "{}".into());
                let id = page_id(&body);
                let keys = self.keys.tabs.entry(tab).or_default();
                if let Some(asked) = keys.asked.remove(&(frame, id)) {
                    if keys.conditional.as_ref().is_some_and(|(r, _, _)| *r == asked) {
                        keys.conditional = None;
                    }
                    self.cred_reply(tab, asked, true, refused("AbortError", "aborted"));
                    self.dirty_tab(tab);
                }
            }
            "create" | "get" => {
                // Only for a secure page, only from its top frame or a frame of the same origin:
                // without WebKit's own WebAuthn there is no permissions policy to say a
                // cross-origin frame may.
                if !site::is_secure_origin(&origin) {
                    return self.cred_reply(tab, request, true, refused("SecurityError", "not a secure page"));
                }
                if !main && top_origin != origin {
                    return self.cred_reply(tab, request, true, refused("NotAllowedError", "SpatiKey does not answer a frame of another site"));
                }
                let id = page_id(&body);
                self.keys.tabs.entry(tab).or_default().asked.insert((frame.clone(), id), request);
                if kind == "create" {
                    self.passkey_create(tab, request, origin, &body);
                } else if body.get("conditional").and_then(Value::as_bool).unwrap_or(false) {
                    // Offered in the login field: it waits for the key button.
                    self.keys.tabs.entry(tab).or_default().conditional = Some((request, origin, body));
                    self.dirty_tab(tab);
                } else {
                    self.passkey_get(tab, request, origin, &body, false);
                }
            }
            _ => self.cred_reply(tab, request, false, format!("SpatiKey does not know {kind:?}")),
        }
    }

    fn passkey_create(&mut self, tab: TabId, request: u64, origin: String, body: &Value) {
        let host = site::host_of(&origin).unwrap_or_default();
        let rp = body.get("rp").cloned().unwrap_or(Value::Null);
        let rp_id = rp.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(&host).to_string();
        if !site::rp_id_ok(&rp_id, &origin) {
            return self.cred_reply(tab, request, true, refused("SecurityError", "that relying party is not this site"));
        }
        let user = body.get("user").cloned().unwrap_or(Value::Null);
        let field = |v: &Value, name: &str| v.get(name).and_then(Value::as_str).unwrap_or("").to_string();
        let challenge = field(body, "challenge");
        let client_data = spatikey_core::webauthn::client_data_json("webauthn.create", &challenge, &origin, false, None);
        let hash = sha256(client_data.as_bytes());
        let ids = |name: &str| -> Vec<String> {
            body.get(name).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        let algs = body.get("algs").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
        let ask = wire::PasskeyCreate {
            origin,
            rp_id,
            rp_name: field(&rp, "name"),
            user_id: field(&user, "id"),
            user_name: field(&user, "name"),
            user_display: field(&user, "displayName"),
            client_data_hash: b64::encode_url(&hash),
            exclude: ids("exclude"),
            algs,
        };
        self.keys.ask(Request::PasskeyCreate(ask), move |answer| {
            let json = match answer.and_then(|v| serde_json::from_value::<wire::CreatedAnswer>(v).map_err(|e| Refusal::new(Code::Failed, e.to_string()))) {
                Ok(made) => json!({"ok": {
                    "credential_id": made.credential_id,
                    "client_data_json": b64::encode_url(client_data.as_bytes()),
                    "attestation_object": made.attestation_object,
                    "authenticator_data": made.authenticator_data,
                    "public_key": made.public_key,
                    "alg": made.alg,
                    "transports": made.transports,
                }})
                .to_string(),
                Err(refusal) => dom_error(&refusal),
            };
            Done::Reply { tab, request, ok: true, json }
        });
    }

    fn passkey_get(&mut self, tab: TabId, request: u64, origin: String, body: &Value, conditional: bool) {
        let host = site::host_of(&origin).unwrap_or_default();
        let rp_id = body.get("rpId").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(&host).to_string();
        if !site::rp_id_ok(&rp_id, &origin) {
            return self.cred_reply(tab, request, true, refused("SecurityError", "that relying party is not this site"));
        }
        let challenge = body.get("challenge").and_then(Value::as_str).unwrap_or("").to_string();
        let client_data = spatikey_core::webauthn::client_data_json("webauthn.get", &challenge, &origin, false, None);
        let hash = sha256(client_data.as_bytes());
        let allow = body.get("allow").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let ask = wire::PasskeyGet { origin, rp_id, client_data_hash: b64::encode_url(&hash), allow, conditional };
        self.keys.ask(Request::PasskeyGet(ask), move |answer| {
            match answer.and_then(|v| serde_json::from_value::<wire::ChoicesAnswer>(v).map_err(|e| Refusal::new(Code::Failed, e.to_string()))) {
                Ok(mut found) if found.choices.len() == 1 => {
                    let choice = found.choices.remove(0);
                    Done::Reply { tab, request, ok: true, json: assertion_json(&choice, &client_data) }
                }
                Ok(found) if !found.choices.is_empty() => Done::Choices { tab, request, client_data, choices: found.choices },
                Ok(_) => Done::Reply { tab, request, ok: true, json: dom_error(&Refusal::new(Code::NoMatch, "no passkey for this site")) },
                Err(refusal) => Done::Reply { tab, request, ok: true, json: dom_error(&refusal) },
            }
        });
    }

    /// The key button: the focused login filled, or the passkey offered in it given.
    pub(super) fn key_button(&mut self, w: usize) {
        let Some(tab) = self.tabs.active(self.wins[w].key) else { return };
        let keys = self.keys.tabs.entry(tab).or_default();
        if let Some((request, origin, body)) = keys.conditional.take() {
            self.passkey_get(tab, request, origin, &body, true);
            return;
        }
        let Some((frame, origin)) = keys.focused.clone().or_else(|| {
            // No field said it has the focus: the top frame's page, if it said it has a login.
            None
        }) else {
            self.say("SpatiKey: put the cursor in the page's login field first".into());
            return;
        };
        self.keys.ask(Request::Fill { origin: origin.clone() }, move |answer| {
            let logins = answer.and_then(|v| {
                serde_json::from_value::<wire::FillAnswer>(v).map(|f| f.logins).map_err(|e| Refusal::new(Code::Failed, e.to_string()))
            });
            Done::Logins { tab, frame, origin, logins }
        });
    }

    /// Answers back from the service, each frame told.
    pub(super) fn keys_tick(&mut self) {
        while let Ok(done) = self.keys.rx.try_recv() {
            self.keys.asking = self.keys.asking.saturating_sub(1);
            match done {
                Done::Reply { tab, request, ok, json } => {
                    if let Some(keys) = self.keys.tabs.get_mut(&tab) {
                        keys.asked.retain(|_, r| *r != request);
                    }
                    self.cred_reply(tab, request, ok, json);
                }
                Done::Said(text) => self.say(text),
                Done::Logins { tab, frame, origin, logins } => match logins {
                    Ok(logins) if logins.is_empty() => self.say(format!("SpatiKey has no password for {}", site_name(&origin))),
                    Ok(mut logins) if logins.len() == 1 => {
                        let login = logins.remove(0);
                        self.fill(tab, &frame, login);
                    }
                    Ok(logins) => self.choose_login(tab, frame, logins),
                    Err(refusal) => self.say(format!("SpatiKey: {}", plain(&refusal))),
                },
                Done::Choices { tab, request, client_data, choices } => self.choose_passkey(tab, request, client_data, choices),
            }
        }
    }

    fn fill(&mut self, tab: TabId, frame: &str, login: wire::Login) {
        let Some(keys) = self.keys.tabs.get_mut(&tab) else { return };
        let Some(target) = keys.frames.remove(frame) else {
            self.say("SpatiKey: that page has gone".into());
            return;
        };
        keys.filled = Some((login.username.clone(), Secret(login.password.clone())));
        let command = json!({"fill": {"username": login.username, "password": login.password}}).to_string();
        let _ = target.origin;
        self.cred_reply(tab, target.request, true, command);
    }

    fn choose_login(&mut self, tab: TabId, frame: String, logins: Vec<wire::Login>) {
        let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) else { return };
        let rows = logins.iter().map(|l| Row::with(&if l.username.is_empty() { l.title.clone() } else { l.username.clone() }, l.title.clone())).collect::<Vec<_>>();
        let actions = (0..logins.len()).map(Action::KeyChoice).collect();
        self.keys.choosing = Some(Choosing::Logins { tab, frame, logins });
        self.open_key_list(w, rows, actions);
    }

    fn choose_passkey(&mut self, tab: TabId, request: u64, client_data: String, choices: Vec<wire::Choice>) {
        let Some(w) = self.tabs.window_of(tab).and_then(|k| self.win_of_key(k)) else {
            return self.cred_reply(tab, request, true, refused("NotAllowedError", "the window has gone"));
        };
        let rows = choices.iter().map(|c| Row::with(&c.user_name, c.user_display.clone())).collect::<Vec<_>>();
        let actions = (0..choices.len()).map(Action::KeyChoice).collect();
        self.keys.choosing = Some(Choosing::Passkeys { tab, request, client_data, choices });
        self.open_key_list(w, rows, actions);
    }

    fn open_key_list(&mut self, w: usize, rows: Vec<Row>, actions: Vec<Action>) {
        let page = self.wins[w].page;
        let width = 420.0f32.min(page.w - 16.0);
        let height = super::menus::rows_height(&rows).min(page.h);
        let rect = spatiand_app::ui::Rect::new(page.right() - width - 8.0, page.y + 4.0, width, height);
        self.open_popup(w, rows, actions, rect);
    }

    /// An account chosen from the list ([`Action::KeyChoice`]).
    pub(super) fn key_chosen(&mut self, index: usize) {
        match self.keys.choosing.take() {
            Some(Choosing::Logins { tab, frame, mut logins }) if index < logins.len() => {
                let login = logins.swap_remove(index);
                self.fill(tab, &frame, login);
            }
            Some(Choosing::Passkeys { tab, request, client_data, choices }) => match choices.get(index) {
                Some(choice) => self.cred_reply(tab, request, true, assertion_json(choice, &client_data)),
                None => self.cred_reply(tab, request, true, refused("NotAllowedError", "none chosen")),
            },
            _ => {}
        }
    }

    /// The account list closed with nothing chosen.
    pub(super) fn key_list_closed(&mut self) {
        if let Some(Choosing::Passkeys { tab, request, .. }) = self.keys.choosing.take() {
            self.cred_reply(tab, request, true, refused("NotAllowedError", "none chosen"));
        }
    }

    /// The wearer said yes to keeping a login ([`Own::SaveLogin`]).
    pub(super) fn save_login(&mut self, origin: String, username: String, password: Secret) {
        let site = site_name(&origin);
        self.keys.ask(Request::Save { origin, title: String::new(), username, password: password.0 }, move |answer| match answer {
            Ok(_) => Done::Said(format!("Kept in SpatiKey: the password for {site}")),
            Err(refusal) => Done::Said(format!("Not kept in SpatiKey: {}", plain(&refusal))),
        });
    }
}

/// The page's own id for a request, a number or a string, as one string to look it up by.
fn page_id(body: &Value) -> String {
    body.get("id").map(Value::to_string).unwrap_or_default()
}

fn assertion_json(choice: &wire::Choice, client_data: &str) -> String {
    let a = &choice.assertion;
    json!({"ok": {
        "credential_id": a.credential_id,
        "client_data_json": b64::encode_url(client_data.as_bytes()),
        "authenticator_data": a.authenticator_data,
        "signature": a.signature,
        "user_handle": a.user_handle,
    }})
    .to_string()
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).to_vec()
}

/// A refusal as the wearer is told it.
pub(super) fn plain(refusal: &Refusal) -> String {
    match refusal.code {
        Code::NoVault => "SpatiKey is not set up yet: open SpatiKey on your Mac".into(),
        Code::Denied => "Touch ID was cancelled".into(),
        Code::TouchIdUnavailable => "Touch ID cannot be used now (is the Mac's lid open?)".into(),
        Code::FingerprintsChanged => "the Mac's fingers changed: set Touch ID up again in SpatiKey".into(),
        Code::NoApprover => "this device has no Mac to ask: pair one in SpatiKey".into(),
        Code::NotReachable => "no Mac of yours answered on this network".into(),
        Code::NotSameLan => "the Mac is not on this network".into(),
        Code::NoMatch => "nothing for this site".into(),
        Code::Timeout => "the Mac did not answer in time".into(),
        Code::NotHere => "SpatiKey is not running here".into(),
        _ if !refusal.message.is_empty() => refusal.message.clone(),
        _ => format!("{:?}", refusal.code),
    }
}
