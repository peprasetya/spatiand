//! SpatiPal's mind: a turn of conversation, on a thread of its own.
//!
//! **The small model first, always** (`neohorse-1:4b` by default): it answers at once, or acts
//! with its tools, or -- when the request wants thought, working out, code, a long or exact
//! answer -- calls `hand_over`, and the same conversation goes to the larger model with thinking
//! on (the owner's decision: the small one decides, quickly, by itself). While the larger one
//! thinks, the figure shows it; a soft tone plays, or its thinking is read aloud, as the wearer
//! chose.
//!
//! What is written is spoken as it is written, a sentence at a time, so the first words are heard
//! while the rest is still coming.
//!
//! **Quick to the first word** (the owner found 10-20 s on 2026-10-29): the small model's decision
//! is asked beside its answer, not before it, the answer held back until the decision comes; and
//! what is sent begins the same way every turn -- tools, then a system message that never changes,
//! then the chat -- so the server reads only what is new. The room as it is now (the time, the
//! windows) goes with the wearer's words, not in the system message, where a minute passing made
//! the server read everything after it again.
//!
//! **What the wearer sees comes first** (the owner, 2026-11-15: "the priority is what the user
//! see ... the hidden things -- the page or the other application -- become additional
//! information if they are related to the question"). Every turn knows which window they are
//! looking at and how much of it is in view (the room's `seen`). A request about what is seen --
//! "describe this", "what am I looking at", "apa ini" -- comes with the room's picture of it as it
//! is shown: cropped to the part in view, at the zoom it is drawn at, so a page zoomed in on one
//! picture is that picture ([`about_the_view`]). A window's own text (`describe`: a whole page, a
//! terminal's scroll-back) is given only when the request reaches past what is on screen -- "the
//! rest of the article" -- and marked as not on screen ([`past_the_view`]); other windows only
//! when asked for.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use spatiand_voice::chat::{converse, Ask, Piece};
use spatiand_voice::speech::sentence_end;
use spatiand_voice::{Hemera, VoiceSettings};

use crate::store::{Said, Settings};
use crate::tools::{self, Held, Room};
use crate::voice::Voice;

/// One turn: what the wearer said, the chat so far, and whether to show the model the window
/// they are looking at.
pub struct Turn {
    /// Its number, given by [`Brain::ask`]: what it tells the window is marked with it.
    pub number: u64,
    pub text: String,
    pub history: Vec<Said>,
    pub look: bool,
    pub settings: Settings,
    /// A pal's `appearance` tool, when its figure offers anything, and how it looks now in words
    /// (`crate::pack`): the tool is the same every turn, the look goes with the wearer's words.
    pub appearance: Option<Value>,
    pub looks: String,
    /// When the wearer stopped speaking, when it was said aloud: how long the answer took is
    /// counted from then.
    pub ended: Option<Instant>,
}

/// What the mind has to say to the window.
pub enum Told {
    Text(String),
    Thinking(String),
    /// A tool is being done, by name: the figure works.
    Doing(String),
    Did(String),
    /// The wearer dismissed it: it goes once it has said so.
    Dismissed,
    /// The larger model has it now.
    HandedOver(String),
    Done(Said),
    Trouble(String),
}

pub struct Brain {
    turns: Sender<Turn>,
    /// The last turn asked for.
    asked: AtomicU64,
    /// Every turn up to this one is stopped: an answer stops when its turn is.
    stopped: Arc<AtomicU64>,
    /// What waits for the wearer's yes is to be let go before the next turn.
    let_go: Arc<AtomicBool>,
}

impl Brain {
    /// The brain, on a thread of its own; what it tells the window comes marked with the
    /// number of the turn it is about.
    pub fn start(room: Room, voice: Voice, told: Sender<(u64, Told)>) -> Brain {
        let (turns, waiting) = channel::<Turn>();
        let stop = Arc::new(AtomicU64::new(0));
        let stopped = stop.clone();
        let let_go = Arc::new(AtomicBool::new(false));
        let gone = let_go.clone();
        std::thread::Builder::new()
            .name("brain".into())
            .spawn(move || think(waiting, room, voice, told, stopped, gone))
            .expect("a thread for the brain");
        Brain { turns, asked: AtomicU64::new(0), stopped: stop, let_go }
    }

    /// Ask for a turn; its number, or `None` when the brain has gone (its thread ended).
    pub fn ask(&self, mut turn: Turn) -> Option<u64> {
        let number = self.asked.fetch_add(1, Ordering::Relaxed) + 1;
        turn.number = number;
        self.turns.send(turn).ok().map(|_| number)
    }

    /// Stop the answer being given, and any asked for before it. A turn asked for after this
    /// is not stopped by it.
    pub fn stop(&self) {
        self.stopped.fetch_max(self.asked.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// Forget what waits for the wearer's yes: told to be quiet, Stop pressed, spoken over -- a
    /// later "okay" is not the answer to it.
    pub fn let_go(&self) {
        self.let_go.store(true, Ordering::Relaxed);
    }
}

fn think(turns: Receiver<Turn>, room: Room, voice: Voice, told: Sender<(u64, Told)>, stop: Arc<AtomicU64>, let_go: Arc<AtomicBool>) {
    // What waits for the wearer's yes, and since when.
    let mut held: Option<(Held, std::time::Instant)> = None;
    // One client for every turn, so its connections to hemera are kept from one to the next.
    let mut client: Option<(String, Hemera)> = None;
    while let Ok(turn) = turns.recv() {
        if let_go.swap(false, Ordering::Relaxed) {
            if let Some((h, _)) = held.take() {
                log::info!("brain: {} let go: the wearer stopped it", h.what);
            }
        }
        let address = VoiceSettings::load(&crate::store::shared()).hemera;
        let hemera = match &client {
            Some((at, h)) if *at == address => h.clone(),
            _ => {
                let h = Hemera::new(&address);
                client = Some((address, h.clone()));
                h
            }
        };
        let number = turn.number;
        // Stopped before it began: nothing of it is said or done.
        let stopped = || stop.load(Ordering::Relaxed) >= number;
        if stopped() {
            continue;
        }
        let wake = room.wake.clone();
        let say = |t: Told| {
            let _ = told.send((number, t));
            wake();
        };
        // The answer to a question it asked: what was held is done, or let go.
        if let Some((h, at)) = held.take().filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(180)) {
            let _ = at;
            let indonesian = looks_indonesian(&turn.text);
            match agreement(&turn.text) {
                Some(true) => {
                    let done = tools::run_held(&h, &room);
                    log::info!("brain: the wearer agreed to {}: {}", h.what, done.content);
                    let words = if done.content.starts_with("That did not work") {
                        done.content.clone()
                    } else if indonesian {
                        "Oke, sudah.".to_string()
                    } else {
                        "Done.".to_string()
                    };
                    voice.say(&words);
                    let said = Said { role: "assistant".into(), text: words, did: done.did.into_iter().collect(), at: crate::store::now(), ..Default::default() };
                    say(Told::Done(said));
                    continue;
                }
                Some(false) => {
                    let words = if indonesian { "Oke, tidak jadi." } else { "All right, I won't." };
                    voice.say(words);
                    say(Told::Done(Said { role: "assistant".into(), text: words.into(), at: crate::store::now(), ..Default::default() }));
                    continue;
                }
                // Something else entirely: what was held is let go, and this is answered.
                None => {}
            }
        }
        match answer(&turn, &hemera, &room, &voice, &say, &stopped, &mut held) {
            Ok(said) => say(Told::Done(said)),
            Err(why) => {
                log::warn!("brain: {why}");
                voice.set_thinking(false);
                say(Told::Trouble(why));
            }
        }
    }
}

/// How many times a turn may go back to the model with what its tools did.
const ROUNDS: usize = 10;
/// How much of the chat the model is given.
const REMEMBERED: usize = 16;

fn answer(turn: &Turn, hemera: &Hemera, room: &Room, voice: &Voice, say: &dyn Fn(Told), stopped: &dyn Fn() -> bool, held: &mut Option<(Held, std::time::Instant)>) -> Result<Said, String> {
    let s = &turn.settings;
    let started = Instant::now();
    if let Some(ended) = turn.ended {
        voice.mark(ended);
    }
    // **The small model decides who answers**, in one word -- left to a tool the small model
    // might call, it was seen to write a long program itself instead. Asked beside its answer:
    // asked first, it put a round trip and a second before every answer.
    let decision = if turn.look { None } else { decide(hemera, s, turn) };
    let windows = room.ask(json!({"do": "windows"}));
    // What the wearer sees: the window looked at and the others in view -- with its picture, or
    // the whole view's, when what they ask is about it.
    let about = if turn.look { About::Window } else { about_the_view(&turn.text) };
    let seen = room.ask(json!({"do": "seen", "picture": about != About::Nothing, "view": about == About::Room, "largest": 1024}));
    let mut messages = vec![json!({"role": "system", "content": system(s, false)})];
    let mut past = conversation(remembered(&turn.history));
    // A question left unanswered (an error, a stop): it goes with this one, not as a second
    // message from the wearer in a row.
    let unanswered = if past.last().is_some_and(|m| m["role"] == "user") { past.pop().and_then(|m| m["content"].as_str().map(str::to_string)) } else { None };
    messages.extend(past);
    // The room as it is, with the wearer's words: the picture of what they see first, when the
    // request is about it; then the window's own text, when the request reaches past the screen.
    // A pal's look is part of the room as it is.
    let note = if turn.looks.is_empty() { room_note(&windows, &seen) } else { format!("{}\n{}", room_note(&windows, &seen), turn.looks) };
    let mut words = format!("{note}\n{}", turn.text);
    let looked = seen.pointer("/looking_at/window").and_then(Value::as_u64);
    if let (Some(window), true) = (looked, past_the_view(&turn.text)) {
        let own = room.ask(json!({"do": "describe", "window": window}));
        if let Some(text) = own.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
            let text: String = text.chars().take(3000).collect();
            words = format!(
                "{note}\n[Window {window}'s own text: all of it, much of it NOT on screen now. The picture is what the wearer sees; for what they ask about beyond it -- the rest of the page, what is above or below -- answer from this text:]\n{text}\n[End of window {window}'s text.]\n{}",
                turn.text
            );
            log::info!("brain: given window {window}'s own text too ({} characters): the request reaches past the screen", text.chars().count());
        }
    }
    if let Some(before) = unanswered {
        words = format!("{before}\n\n{words}");
    }
    let mut content = Vec::new();
    if let Some(png) = seen.get("view_png").and_then(Value::as_str) {
        content.push(json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{png}")}}));
        log::info!("brain: given the wearer's view");
    } else if let Some(png) = seen.pointer("/looking_at/png").and_then(Value::as_str) {
        content.push(json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{png}")}}));
        log::info!("brain: given what the wearer sees of window {} ({})", looked.unwrap_or(0), part_words(seen.pointer("/looking_at/part")));
    }
    // `SPATIPAL_TEST_RECORD=<folder>`: the picture the model is given, kept for a test to look at.
    if let (Some(dir), Some(png)) = (std::env::var_os("SPATIPAL_TEST_RECORD"), seen.get("view_png").or(seen.pointer("/looking_at/png")).and_then(Value::as_str)) {
        if let Some(bytes) = spatiand_voice::speech::decode_base64(png) {
            let _ = std::fs::write(std::path::Path::new(&dir).join(format!("seen-{}.png", crate::store::now())), bytes);
        }
    }
    let user = if content.is_empty() {
        json!({"role": "user", "content": words})
    } else {
        content.push(json!({"type": "text", "text": words}));
        json!({"role": "user", "content": content})
    };
    messages.push(user);

    let mut large = false;
    let mut model = s.model.clone();
    let mut said = Said { role: "assistant".into(), model: model.clone(), at: crate::store::now(), ..Default::default() };
    // Until the decision comes, what the small model writes is held back: shown and said only
    // once the answer is its to give.
    let mut decided: Option<bool> = if decision.is_some() { None } else { Some(false) };
    let mut first_words = false;
    for _ in 0..ROUNDS {
        if stopped() {
            break;
        }
        let ask = Ask {
            model: model.clone(),
            messages: messages.clone(),
            tools: tools::all(!large, turn.appearance.as_ref()),
            max_tokens: if large { 4096 } else { 700 },
            temperature: if large { 0.6 } else { 0.4 },
            thinking: large,
        };
        let mut speaking = String::new();
        let mut in_code = false;
        let mut thought = String::new();
        let mut held_back = String::new();
        let read_thinking = large && s.thinking == "read";
        let asked_at = std::time::Instant::now();
        log::info!("brain: asking {model} ({} messages)", messages.len());
        voice.set_generating(true);
        let answer = converse(hemera, &ask, |piece| {
            if stopped() {
                return false;
            }
            if decided.is_none() {
                decided = poll(&decision);
            }
            match decided {
                // The larger model's: this answer stops, unseen and unsaid.
                Some(true) => return false,
                None => {
                    if let Piece::Text(t) = piece {
                        held_back.push_str(t);
                    }
                    return true;
                }
                Some(false) if !held_back.is_empty() => {
                    let t = std::mem::take(&mut held_back);
                    say(Told::Text(t.clone()));
                    speaking.push_str(&t);
                }
                Some(false) => {}
            }
            match piece {
                Piece::Text(t) => {
                    if !std::mem::replace(&mut first_words, true) {
                        log::info!("brain: first words from {model} {:.2} s into the turn", started.elapsed().as_secs_f32());
                    }
                    say(Told::Text(t.clone()));
                    speaking.push_str(t);
                    speak_out(&mut speaking, &mut in_code, voice, false);
                }
                Piece::Thinking(t) => {
                    say(Told::Thinking(t.clone()));
                    if read_thinking {
                        thought.push_str(t);
                        while let Some(end) = sentence_end(&thought) {
                            voice.say(&thought[..end]);
                            thought.drain(..end);
                        }
                    }
                }
                Piece::Calling(_) => {}
            }
            true
        });
        voice.set_generating(false);
        let answer = answer?;
        // Stopped -- spoken over, told to be quiet, Stop --: nothing more of it is said, not the
        // end of the sentence that was cut, nor what was held back or thought.
        if stopped() {
            said.text.push_str(&answer.text);
            said.thinking.push_str(&answer.thinking);
            break;
        }
        if decided.is_none() {
            // Answered before the decision came: it comes in a moment.
            decided = Some(decision.as_ref().and_then(|rx| rx.recv_timeout(Duration::from_secs(3)).ok()).unwrap_or(false));
        }
        if decided == Some(true) && !large {
            // Decided, and done with: the larger model's answer is its own, not held or cut.
            decided = Some(false);
            large = true;
            model = s.large_model.clone();
            said.model = model.clone();
            messages[0] = json!({"role": "system", "content": system(s, true)});
            log::info!("brain: {} decided the larger model answers", s.model);
            say(Told::HandedOver(model.clone()));
            voice.set_thinking(s.thinking == "tone");
            continue;
        }
        if !held_back.is_empty() {
            say(Told::Text(held_back.clone()));
            speaking.push_str(&held_back);
        }
        speak_out(&mut speaking, &mut in_code, voice, true);
        if read_thinking {
            voice.say(&thought);
        }
        log::info!(
            "brain: {model} answered in {:.1} s: {} characters, {} of thinking, calls {:?}",
            asked_at.elapsed().as_secs_f32(),
            answer.text.len(),
            answer.thinking.len(),
            answer.calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
        );
        said.text.push_str(&answer.text);
        said.thinking.push_str(&answer.thinking);
        if answer.cut {
            break;
        }
        // The small model hands it over: the same conversation, to the larger, thinking.
        if !large && answer.calls.iter().any(|c| c.name == "hand_over") {
            large = true;
            model = s.large_model.clone();
            said.model = model.clone();
            messages[0] = json!({"role": "system", "content": system(s, true)});
            log::info!("brain: handed over to {model}");
            say(Told::HandedOver(model.clone()));
            voice.set_thinking(s.thinking == "tone");
            continue;
        }
        if answer.calls.is_empty() {
            break;
        }
        messages.push(answer.message());
        let mut pictures = Vec::new();
        for call in &answer.calls {
            if call.name == "hand_over" {
                messages.push(json!({"role": "tool", "tool_call_id": call.id, "content": "You are the larger model already: answer."}));
                continue;
            }
            say(Told::Doing(call.name.clone()));
            if call.name == "dismiss" {
                say(Told::Dismissed);
            }
            let done = tools::run(call, room);
            // What it typed and what it read are the wearer's: only the tool, what it was given
            // (by name) and how it went are logged.
            let given: Vec<String> = serde_json::from_str::<Value>(&call.arguments).ok().and_then(|a| a.as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default();
            log::info!("brain: {}({}) -> {} characters{}", call.name, given.join(", "), done.content.chars().count(), if done.held.is_some() { ", held for a yes" } else { "" });
            if let Some(h) = done.held.clone() {
                *held = Some((h, std::time::Instant::now()));
            }
            if let Some(did) = &done.did {
                said.did.push(did.clone());
                say(Told::Did(did.clone()));
            }
            messages.push(json!({"role": "tool", "tool_call_id": call.id, "content": done.content}));
            if let Some(picture) = done.picture {
                pictures.push(picture);
            }
        }
        // A picture cannot go in a tool's answer: it comes as what the wearer showed.
        for picture in pictures {
            messages.push(json!({"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": picture}},
                {"type": "text", "text": "What you looked at, as the wearer sees it."},
            ]}));
        }
    }
    voice.set_thinking(false);
    said.at = crate::store::now();
    Ok(said)
}

/// Who it is and what it can do: the system's part of every turn, **the same every turn** -- the
/// server keeps what it has read and reads again only from where a request first differs, so
/// nothing that changes (the time, the windows) belongs here; that goes with the wearer's words
/// ([`room_note`]). The larger model is not told about handing over -- told, it spent its thinking
/// on whether it should, and opened an application to be sure.
fn system(s: &Settings, larger: bool) -> String {
    // A pal's persona is pages of its own (`crate::pack`): a paragraph of its own, after who it is.
    let persona = if s.persona.contains('\n') { format!("\n\n{}\n", s.persona.trim()) } else { s.persona.clone() };
    format!(
        "You are {name}, the wearer's assistant in Spatiand, a 3D desktop seen through glasses. {persona}\n\
         \n\
         Talk like a person: short, natural sentences. Answer in the language of the wearer's last message: English to English, Indonesian to Indonesian; only when they mix the two in that message, mix them too. What you write is spoken aloud: no markdown, no lists unless asked, no emoji, no actions or stage directions such as *smiles*.\n\
         \n\
         Each message from the wearer begins with a note in square brackets on the room as it is then: the time, the window they are looking at and how much of it is in view, the other windows on show, and the hidden ones. The wearer did not say it; use it when it helps.\n\
         What the wearer sees comes first. \"This\", \"here\", \"what I see\" mean the window they are looking at, and only the part of it in view. When they ask about it, a picture of exactly what they see comes with their message: describe and answer from that picture -- if it shows one image, it is that image -- not from what you know of the page or the application. Text marked as more than is on screen is there only for what the request needs beyond the view (the rest of an article, the other window); do not describe it as what they see.\n\
         Go to other windows or applications only when the wearer asks for one (\"the other window\", \"open the browser\", \"in the terminal\") or the request cannot be done in the one they are looking at; then you may use that window, or show a result in it. A hidden window is not in the room, and may be something on their computer's own screen: read it, look at it or bring it out only when they name it or ask for it. Asked what is open, talk about what is on show, and mention the hidden ones only if asked.\n\
         Told to go away or that it is all for now, call dismiss, and do nothing to any window.\n\
         \n\
         You act in the room with your tools: open applications, work windows as the wearer would (read them, look at them, click, right-click, scroll, drag, type, press keys), bring the room back in front of the wearer, and keep reminders. To find where to click, look at the window first; places are fractions of the window from 0 to 1, from its top left. Say briefly what you are doing.\n\
         Ask the wearer first before closing a window, deleting anything, sending a message, buying anything, or pressing Enter in a terminal.\n\
         {route}",
        name = s.name,
        route = if larger {
            "You are the larger model: think it through, then answer. Use tools only when the request is about the room. Code goes in a code block; say the rest briefly."
        } else {
            "When a request needs careful thought, working out, code, a long or exact answer, or several steps you are unsure of, call hand_over at once instead of answering."
        },
    )
}

/// A request that begins as every one of a model's does, for it to read before it is needed
/// (`crate::warm`): its tools and its system message, and a word from no one. The larger
/// model's when `larger`.
pub fn prime(s: &Settings, larger: bool, appearance: Option<&Value>) -> Value {
    json!({
        "model": if larger { &s.large_model } else { &s.model },
        "messages": [
            {"role": "system", "content": system(s, larger)},
            {"role": "user", "content": "[Getting ready: nothing is asked yet.]\nHi."},
        ],
        "tools": tools::all(!larger, appearance),
        "max_tokens": 1,
        "temperature": 0,
        "stream": false,
        // As its requests ask: a template may write the system message differently for it.
        "chat_template_kwargs": {"enable_thinking": larger},
    })
}

/// The room as it is now, for the wearer's message: the time, the window the wearer is looking
/// at and how much of it they see, and the windows in the room.
fn room_note(windows: &Value, seen: &Value) -> String {
    let looked = seen.pointer("/looking_at/window").and_then(Value::as_u64);
    let in_view: Vec<u64> = seen.get("others").and_then(Value::as_array).into_iter().flatten().filter_map(|o| o.get("window").and_then(Value::as_u64)).collect();
    let flat = seen.get("flat").and_then(Value::as_bool).unwrap_or(true);
    let (mut shown, mut hidden) = (String::new(), String::new());
    let (mut n_shown, mut n_hidden) = (0, 0);
    for w in windows.get("windows").and_then(Value::as_array).into_iter().flatten() {
        let id = w.get("window").and_then(Value::as_u64).unwrap_or(0);
        let app = w.get("app").and_then(Value::as_str).unwrap_or_default();
        let app = app.rsplit('.').next().unwrap_or(app);
        let title = w.get("title").and_then(Value::as_str).unwrap_or_default();
        let mut flags = Vec::new();
        for (key, word) in [("focused", "focused"), ("pinned", "pinned")] {
            if w.get(key).and_then(Value::as_bool) == Some(true) {
                flags.push(word);
            }
        }
        if w.get("own").and_then(Value::as_bool) == Some(true) {
            flags.push("your own chat window");
        }
        let seeing;
        if Some(id) == looked {
            seeing = format!("the wearer is looking at it, {}", if flat { "all of it".to_string() } else { part_words(seen.pointer("/looking_at/part")) });
            flags.push(&seeing);
        } else if !flat && w.get("hidden").and_then(Value::as_bool) != Some(true) {
            flags.push(if in_view.contains(&id) { "also in view" } else { "out of view" });
        }
        let flags = if flags.is_empty() { String::new() } else { format!(" ({})", flags.join(", ")) };
        let line = format!("\n- window {id}: {app} \u{2014} {title}{flags}");
        if w.get("hidden").and_then(Value::as_bool) == Some(true) {
            n_hidden += 1;
            if n_hidden <= 10 {
                hidden.push_str(&line);
            }
        } else {
            n_shown += 1;
            if n_shown <= 14 {
                shown.push_str(&line);
            }
        }
    }
    if shown.is_empty() {
        shown = " none".into();
    }
    let hidden = if hidden.is_empty() { String::new() } else { format!("\nHidden, not in the room:{hidden}") };
    format!("[It is {}. Windows on show in the room:{shown}{hidden}]", crate::time::now_line(crate::store::now()))
}

/// What a request is about, of what the wearer sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum About {
    /// Not what is on screen: the time, a reminder, a question of its own.
    Nothing,
    /// The window they are looking at: "this", "what I see", "the picture".
    Window,
    /// More than one window: "everything in view", "these windows".
    Room,
}

/// Whether the wearer is asking about what they see, by their words, in either language: then
/// the room's picture of it goes with the request. Broad on purpose -- "ini" and "this" count --
/// because a picture of what is seen given needlessly costs a second, and one not given is the
/// whole page described instead of the picture zoomed into (the owner's test, 2026-11-15).
pub fn about_the_view(text: &str) -> About {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric() && c != '\'').map(|w| w.trim_matches('\'')).filter(|w| !w.is_empty()).collect();
    let has = |list: &[&str]| words.iter().any(|w| list.contains(w));
    let phrase = |list: &[&str]| list.iter().any(|p| lower.contains(p));
    const ROOM: &[&str] = &["windows", "jendela-jendela", "everything", "around", "sekeliling", "sekitar", "semuanya", "ruangan"];
    const ROOM_PHRASES: &[&str] = &["all the windows", "in the room", "in view", "semua jendela", "di ruangan", "di sekitar"];
    if has(ROOM) || phrase(ROOM_PHRASES) {
        return About::Room;
    }
    const WINDOW: &[&str] = &[
        "see", "seeing", "saw", "seen", "look", "looking", "looks", "view", "screen", "show", "showing", "shown", "display", "displayed", "describe", "description", "picture",
        "pictures", "image", "images", "photo", "photos", "page", "window", "this", "that", "these", "those", "here", "read", "reading", "says", "say", "written", "zoom",
        "zoomed", "visible", "front", "video", "chart", "graph", "map", "text", "button", "menu", "tab", "article", "link", "site", "website", "diagram", "table", "error",
        "lihat", "liat", "melihat", "terlihat", "kelihatan", "tampil", "tampilan", "layar", "gambar", "foto", "halaman", "jendela", "ini", "itu", "sini", "situ", "baca",
        "bacakan", "membaca", "tulisan", "tertulis", "jelaskan", "deskripsikan", "gambarkan", "depan", "tombol", "artikel", "situs", "web", "grafik", "peta", "tabel",
    ];
    if has(WINDOW) {
        About::Window
    } else {
        About::Nothing
    }
}

/// Whether the request reaches past what is on screen -- the rest of a page, all of a document,
/// what is above or below, another window's contents: then the window's own text is given too,
/// marked as not on screen.
pub fn past_the_view(text: &str) -> bool {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    const PAST: &[&str] = &[
        "rest", "whole", "entire", "full", "remaining", "summarize", "summarise", "summary", "below", "above", "further", "beyond", "earlier", "later", "previous", "next", "document",
        "seluruh", "seluruhnya", "keseluruhan", "lengkap", "sisanya", "sisa", "selanjutnya", "berikutnya", "sebelumnya", "ringkas", "ringkasan", "rangkum", "rangkuman", "dokumen",
        "bawah", "atas",
    ];
    words.iter().any(|w| PAST.contains(w)) || ["all of it", "the other window", "jendela lain", "semua isinya", "off screen", "not on screen"].iter().any(|p| lower.contains(p))
}

/// The part of a window in view, in words, from `seen`'s `[u0, u1, v0, v1]`.
pub fn part_words(part: Option<&Value>) -> String {
    let p: Vec<f64> = part.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    let [u0, u1, v0, v1] = match p.as_slice() {
        [a, b, c, d] => [*a, *b, *c, *d],
        _ => return "all of it".into(),
    };
    let share = ((u1 - u0) * (v1 - v0)).clamp(0.0, 1.0);
    if share > 0.97 {
        return "all of it".into();
    }
    let across = if u0 > 0.02 && u1 < 0.98 {
        "the middle"
    } else if u0 > 0.02 {
        "the right"
    } else if u1 < 0.98 {
        "the left"
    } else {
        ""
    };
    let down = if v0 > 0.02 && v1 < 0.98 {
        "the middle"
    } else if v0 > 0.02 {
        "the bottom"
    } else if v1 < 0.98 {
        "the top"
    } else {
        ""
    };
    let place = match (down, across) {
        ("", "") => "most".to_string(),
        ("", a) | (a, "") => a.to_string(),
        ("the middle", "the middle") => "the middle".to_string(),
        (d, a) => format!("{d} {}", a.trim_start_matches("the ")),
    };
    format!("{place} of it, {:.0}% of the window", share * 100.0)
}

/// How much of the chat the model is given, at the least, and the steps its start moves in: the
/// start moving every turn would have the server read the whole chat again every turn.
const STEP: usize = 8;

/// The part of the chat the model is given: the last [`REMEMBERED`] things said or a few more.
fn remembered(history: &[Said]) -> &[Said] {
    let start = history.len().saturating_sub(REMEMBERED) / STEP * STEP;
    &history[start..]
}

/// The chat as the model is given it: what was done in the room said with each answer, and
/// neighbours of one role made one message -- a question whose answer failed or was stopped
/// before a word, then the next, must not be two messages from the wearer in a row.
fn conversation(history: &[Said]) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    for said in history {
        let mut text = said.text.clone();
        if !said.did.is_empty() {
            text.push_str(&format!("\n(Done in the room: {}.)", said.did.join("; ")));
        }
        if text.trim().is_empty() {
            continue;
        }
        match messages.last_mut() {
            Some(last) if last["role"] == said.role.as_str() => {
                let joined = format!("{}\n\n{text}", last["content"].as_str().unwrap_or_default());
                last["content"] = json!(joined);
            }
            _ => messages.push(json!({"role": said.role, "content": text})),
        }
    }
    messages
}

/// Whether what the wearer said is a yes (`Some(true)`), a no (`Some(false)`), or neither: in
/// either language, as briefly as people say it.
///
/// **A yes is only a yes**: every word a yes or what comes with one ("yes please", "ya, tekan
/// aja", "go ahead") -- "Please scroll down instead" is not one, nor "Okay, don't close it". A
/// no anywhere in it is a no. Anything else is neither: what was held is let go, and it is
/// answered as a request of its own.
fn agreement(text: &str) -> Option<bool> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.replace('\u{2019}', "'").trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() || words.len() > 6 {
        return None;
    }
    const YES: &[&str] = &[
        "ya", "iya", "yes", "yep", "yup", "yeah", "boleh", "oke", "ok", "okay", "lanjut", "lanjutkan", "silakan", "silahkan", "sure", "betul", "benar", "setuju", "gas", "go",
        "do", "tekan", "tutup", "please", "press", "close",
    ];
    // What may come with a yes, and is not one by itself.
    const WITH: &[&str] = &["it", "ahead", "that", "now", "enter", "aja", "saja", "dong", "deh", "sih", "lah", "you", "thanks", "thank", "makasih", "tolong", "sekarang"];
    const NO: &[&str] = &[
        "tidak", "nggak", "ngga", "gak", "ga", "enggak", "tak", "belum", "jangan", "batal", "batalkan", "no", "nope", "nah", "not", "don't", "dont", "never", "nevermind",
        "cancel", "stop", "tunggu", "wait", "hold",
    ];
    if words.iter().any(|w| NO.contains(&w.as_str()) || w.ends_with("n't")) {
        Some(false)
    } else if words.iter().any(|w| YES.contains(&w.as_str())) && words.iter().all(|w| YES.contains(&w.as_str()) || WITH.contains(&w.as_str())) {
        Some(true)
    } else {
        None
    }
}

/// Whether the wearer spoke Indonesian, to answer a yes in kind.
fn looks_indonesian(text: &str) -> bool {
    const WORDS: &[&str] = &["ya", "iya", "boleh", "lanjut", "lanjutkan", "silakan", "tidak", "jangan", "nggak", "gak", "oke", "sudah", "tolong", "dong", "aja", "saja", "batal"];
    text.split_whitespace().any(|w| WORDS.contains(&w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    /// **Against hemera**: a pal's model, asked in words, changes its clothes and twirls --
    /// through `appearance`, with SpatiPal's own system message and tools. A made-up pal.
    /// `cargo test -p spatipal -- --ignored --test-threads 1 appearance`
    #[test]
    #[ignore]
    fn a_pals_model_changes_its_clothes_when_asked_appearance() {
        use spatiand_voice::figure::Offers;
        let offers = Offers {
            groups: vec![("outfit".into(), vec!["casual".into(), "dress".into(), "pyjamas".into()])],
            animations: vec!["idle".into(), "talking".into(), "spin".into(), "wave".into(), "hand_on_hip".into()],
            expressions: vec![],
            materials: vec![],
            body: vec![],
        };
        let about = serde_json::from_value::<crate::pack::About>(serde_json::json!({
            "name": "Mira",
            "options": {"outfit": {"casual": "jeans and a hoodie", "dress": "a red evening dress", "pyjamas": "striped pyjamas"}},
            "moves": {"spin": "a twirl, to show off", "wave": "a wave"},
            "poses": {"hand_on_hip": "a hand on your hip"}
        }))
        .unwrap();
        let pack = crate::pack::Pack::made_up(about, "You are Mira, a cheerful woman who loves clothes and shows them off.", offers);
        let s = pack.over(&Settings::default());
        let hemera = Hemera::new(&VoiceSettings::default().hemera);
        let tools = tools::all(true, pack.tool().as_ref());
        for (asked, group, value, mv) in [("Put on your dress and give me a twirl!", "outfit", "dress", Some("spin")), ("Time for bed. Get into your pyjamas.", "outfit", "pyjamas", None)] {
            let ask = Ask {
                model: s.model.clone(),
                messages: vec![json!({"role": "system", "content": system(&s, false)}), json!({"role": "user", "content": format!("[It is 22:10.]\n{}\n{asked}", pack.note())})],
                tools: tools.clone(),
                max_tokens: 300,
                temperature: 0.2,
                thinking: false,
            };
            // A change of clothes is the quick model's: handed to the larger one, it waited for
            // the 27B to load (the snapshot room, 2026-11-19).
            assert!(!wants_larger(&hemera, &s.model, asked, ""), "{asked:?} went to the larger model");
            let answer = converse(&hemera, &ask, |_| true).expect("hemera answers");
            let call = answer.calls.iter().find(|c| c.name == "appearance").unwrap_or_else(|| panic!("{asked:?}: no appearance call: {:?} {:?}", answer.text, answer.calls));
            let args: Value = serde_json::from_str(&call.arguments).unwrap();
            eprintln!("{asked:?} -> appearance({args})");
            assert_eq!(args[group], value, "{asked:?}");
            if let Some(mv) = mv {
                assert_eq!(args["move"], mv, "{asked:?}");
            }
        }
    }

    /// **Against hemera**: a pal's model makes poses of its own as it is asked in words, through
    /// `appearance`'s `own_pose`, each read as the room reads it. A made-up pal with a body and
    /// no poses by name; `SPATIPAL_TEST_MODEL` asks another model than the settings' everyday one.
    /// `cargo test -p spatipal -- --ignored --test-threads 1 own_pose`
    #[test]
    #[ignore]
    fn a_pals_model_makes_a_pose_of_its_own_own_pose() {
        use spatiand_voice::figure::{Offers, BODY_PARTS};
        let offers = Offers { animations: vec!["idle".into(), "talking".into()], body: BODY_PARTS.iter().map(|p| p.to_string()).collect(), ..Default::default() };
        let about = serde_json::from_value::<crate::pack::About>(serde_json::json!({"name": "Mira"})).unwrap();
        let mut pack = crate::pack::Pack::made_up(about, "You are Mira, a playful woman who talks with her whole body.", offers);
        let s = pack.over(&Settings::default());
        let model = std::env::var("SPATIPAL_TEST_MODEL").unwrap_or_else(|_| s.model.clone());
        let hemera = Hemera::new(&VoiceSettings::default().hemera);
        let tools = tools::all(true, pack.tool().as_ref());
        let asks: [(&str, &[&str]); 8] = [
            ("Put your hands on your hips.", &["left_upper_arm", "left_forearm", "right_upper_arm", "right_forearm"]),
            ("Fold your arms.", &["left_forearm", "right_forearm"]),
            ("Strike a thinking pose, your hand on your chin.", &[]),
            ("Shrug.", &[]),
            ("Look up at the sky.", &["head"]),
            ("Raise your right arm, as if you had a question.", &["right_upper_arm"]),
            ("Put your right hand on your heart.", &["right_forearm"]),
            ("Turn your head and look at me over your right shoulder.", &["head"]),
        ];
        let mut misses = Vec::new();
        for (asked, wants) in asks {
            let ask = Ask {
                model: model.clone(),
                messages: vec![json!({"role": "system", "content": system(&s, false)}), json!({"role": "user", "content": format!("[It is 21:40.]\n{}\n{asked}", pack.note())})],
                tools: tools.clone(),
                max_tokens: 400,
                temperature: 0.2,
                thinking: false,
            };
            eprintln!("{asked:?}: to the larger model: {}", wants_larger(&hemera, &s.model, asked, ""));
            let started = std::time::Instant::now();
            let answer = converse(&hemera, &ask, |_| true).expect("hemera answers");
            let Some(call) = answer.calls.iter().find(|c| c.name == "appearance") else {
                misses.push(format!("{asked:?}: no appearance call: {:?} {:?}", answer.text, answer.calls));
                continue;
            };
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|e| {
                eprintln!("{asked:?}: its call is not JSON ({e}): {:?}", call.arguments);
                Value::Null
            });
            eprintln!("{asked:?} -> appearance({args}) in {:.1} s; said {:?}", started.elapsed().as_secs_f32(), answer.text);
            pack.look = Default::default();
            match pack.apply(&args) {
                Ok((_, did)) => {
                    let held = &pack.look.own_pose;
                    eprintln!("    held: {did} -- {held:?}");
                    if held.is_empty() {
                        misses.push(format!("{asked:?}: no pose held"));
                    }
                    for part in wants.iter().filter(|p| !held.contains_key(**p)) {
                        misses.push(format!("{asked:?}: no {part} in {held:?}"));
                    }
                }
                Err(why) => misses.push(format!("{asked:?}: {why}")),
            }
        }
        let _ = std::fs::remove_dir_all(&pack.dir);
        assert!(misses.is_empty(), "{} of {} missed:\n{}", misses.len(), asks.len(), misses.join("\n"));
    }

    use super::*;

    #[test]
    fn what_is_sent_begins_the_same_whatever_the_room() {
        let s = Settings::default();
        let a = system(&s, false);
        let note = room_note(&json!({"windows": [{"window": 3, "app": "id.prasetya.spatiand.spatiquill", "title": "~", "focused": true}]}), &json!({}));
        assert!(!a.contains("window 3") && note.contains("window 3: spatiquill"));
        assert_eq!(a, system(&s, false));
        assert!(note.starts_with("[It is ") && note.ends_with("(focused)]"));
        let both = room_note(&json!({"windows": [
            {"window": 6, "app": "Terminal", "title": "temp", "hidden": true},
            {"window": 3, "app": "spatiquill", "title": "~"},
        ]}), &json!({}));
        let (on_show, kept) = both.split_once("Hidden, not in the room:").unwrap();
        assert!(on_show.contains("window 3") && !on_show.contains("window 6") && kept.contains("window 6"));
    }

    #[test]
    fn what_is_seen_is_given_when_it_is_asked_about() {
        for said in ["describe what I see", "what am I looking at?", "what's this?", "jelaskan gambar ini", "apa ini?", "zoom in on the picture: what is it", "read me the title", "what does the error say"] {
            assert_eq!(about_the_view(said), About::Window, "{said:?}");
        }
        for said in ["what time is it?", "remind me at eight to call mum", "buka browser", "how are you", "berapa derajat di Jakarta besok?"] {
            assert_eq!(about_the_view(said), About::Nothing, "{said:?}");
        }
        for said in ["what are all these windows?", "describe everything in view", "apa saja di sekeliling saya"] {
            assert_eq!(about_the_view(said), About::Room, "{said:?}");
        }
    }

    #[test]
    fn the_window_s_own_text_only_when_the_request_goes_past_the_screen() {
        assert!(past_the_view("what does the rest of the article say?"));
        assert!(past_the_view("ringkas seluruh halaman ini"));
        assert!(past_the_view("read the other window"));
        assert!(!past_the_view("describe what I see"));
        assert!(!past_the_view("what is in this picture?"));
    }

    #[test]
    fn the_note_says_what_is_looked_at_and_how_much() {
        let windows = json!({"windows": [{"window": 3, "app": "spatimesh", "title": "Cats"}, {"window": 4, "app": "spatiquill", "title": "~"}, {"window": 5, "app": "spatishelf", "title": "Home"}]});
        let seen = json!({"flat": false, "looking_at": {"window": 3, "part": [0.25, 0.75, 0.0, 0.5]}, "others": [{"window": 4, "share": 0.1}]});
        let note = room_note(&windows, &seen);
        assert!(note.contains("window 3: spatimesh \u{2014} Cats (the wearer is looking at it, the top middle of it, 25% of the window)"), "{note}");
        assert!(note.contains("window 4: spatiquill \u{2014} ~ (also in view)") && note.contains("window 5: spatishelf \u{2014} Home (out of view)"), "{note}");
        assert_eq!(part_words(Some(&json!([0.0, 1.0, 0.0, 1.0]))), "all of it");
        assert_eq!(part_words(Some(&json!([0.4, 0.6, 0.4, 0.6]))), "the middle of it, 4% of the window");
    }

    #[test]
    fn the_chat_given_starts_in_steps() {
        let chat: Vec<Said> = (0..40).map(|i| Said { text: i.to_string(), ..Default::default() }).collect();
        assert_eq!(remembered(&chat[..10]).len(), 10);
        assert_eq!(remembered(&chat[..23]).len(), 23);
        assert_eq!(remembered(&chat[..24]).len(), 16);
        assert_eq!(remembered(&chat[..31])[0].text, "8");
        assert_eq!(remembered(&chat[..32])[0].text, "16");
    }

    #[test]
    fn yes_and_no_in_either_language() {
        assert_eq!(agreement("Ya, tekan aja"), Some(true));
        assert_eq!(agreement("yes please"), Some(true));
        assert_eq!(agreement("Boleh."), Some(true));
        assert_eq!(agreement("jangan dulu"), Some(false));
        assert_eq!(agreement("No"), Some(false));
        assert_eq!(agreement("ya tapi jangan sekarang"), Some(false));
        assert_eq!(agreement("buka browser dan cari cuaca hari ini di Jakarta"), None);
        assert_eq!(agreement("Go ahead."), Some(true));
        assert_eq!(agreement("Okay, do it"), Some(true));
    }

    #[test]
    fn the_chat_given_has_no_two_messages_of_one_role_in_a_row() {
        let said = |role: &str, text: &str| Said { role: role.into(), text: text.into(), ..Default::default() };
        let chat = [said("user", "what time is it"), said("user", "hello?"), said("assistant", ""), said("assistant", "It is eight."), said("user", "thanks")];
        let messages = conversation(&chat);
        let roles: Vec<&str> = messages.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "user"]);
        assert_eq!(messages[0]["content"], "what time is it\n\nhello?");
    }

    #[test]
    fn code_ending_in_a_character_of_several_bytes_is_kept_whole() {
        let voice = Voice::silent();
        for code in ["```\u{65e5}", "```let s = \"\u{e9}", "```\u{1f600}", "```x\u{e9}"] {
            let (mut pending, mut in_code) = (code.to_string(), false);
            speak_out(&mut pending, &mut in_code, &voice, false);
            assert!(in_code);
            // And the fence closing after it is still found.
            pending.push_str("```");
            speak_out(&mut pending, &mut in_code, &voice, false);
            assert!(!in_code, "{code:?}");
        }
    }

    #[test]
    fn a_yes_is_only_a_yes() {
        // Begun with a yes-word, but asking something else: neither, and what was held goes.
        assert_eq!(agreement("Please scroll down instead"), None);
        assert_eq!(agreement("Sure, but the other one"), None);
        assert_eq!(agreement("tutup yang lain"), None);
        assert_eq!(agreement("tekan escape"), None);
        assert_eq!(agreement("go back"), None);
        // A no anywhere in it is a no.
        assert_eq!(agreement("Okay, don't close it"), Some(false));
        assert_eq!(agreement("Okay, don\u{2019}t"), Some(false));
        assert_eq!(agreement("please not that one"), Some(false));
        assert_eq!(agreement("yes, wait"), Some(false));
        assert_eq!(agreement("ok I won't"), Some(false));
        assert_eq!(agreement("ya tidak usah"), Some(false));
        assert_eq!(agreement("oke batal"), Some(false));
        assert_eq!(agreement("never mind"), Some(false));
    }
}

/// The small model's decision whether the larger one answers, asked on a thread of its own while
/// the small one starts answering. `None` when there is no larger model to hand to.
fn decide(hemera: &Hemera, s: &Settings, turn: &Turn) -> Option<Receiver<bool>> {
    if s.large_model.is_empty() || s.large_model == s.model {
        return None;
    }
    let last = turn.history.iter().rev().find(|said| said.role == "assistant").map(|said| said.text.chars().take(300).collect::<String>()).unwrap_or_default();
    let (hemera, model, text) = (hemera.clone(), s.model.clone(), turn.text.clone());
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("decide".into())
        .spawn(move || {
            let _ = tx.send(wants_larger(&hemera, &model, &text, &last));
        })
        .ok()?;
    Some(rx)
}

/// The decision, if it has come. One that never will (its thread went) is the small model's.
fn poll(decision: &Option<Receiver<bool>>) -> Option<bool> {
    match decision.as_ref()?.try_recv() {
        Ok(larger) => Some(larger),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(false),
    }
}

/// Whether this request is the larger model's: the small one's decision, in one word, made in a
/// moment. A failed decision is the small model's turn: it can still hand over by itself.
fn wants_larger(hemera: &Hemera, model: &str, request: &str, last: &str) -> bool {
    // Told that the quick model knows the time, and shown a few of each: without them, after an
    // answer that gave the time, "what day is it tomorrow?" went to the larger model, which took
    // fifteen seconds over it (the test of 2026-10-29); with them 24 of 25 requests went where
    // they should.
    let system = "You decide who answers the wearer's request. The quick model answers chat, questions with a short answer, \
                  and actions in the room: opening applications, working windows, clicking, typing, reminders, settings, \
                  and changing its own figure -- clothes, a pose, a move. \
                  The larger model answers what needs careful thought: writing or explaining code, calculations of several steps, \
                  analysis, planning, comparing, long or exact writing, translating long text. \
                  The quick model is told the time, the date and what is in the room, and sees what the wearer sees, so questions about them are quick. \
                  For example: \"what time is it?\" quick; \"what day is it tomorrow?\" quick; \"buka browser\" quick; \
                  \"describe what I see\" quick; \"apa ini?\" quick; \
                  \"how many days until Friday?\" quick; \"put on your dress and give me a twirl\" quick; \"show me how you sulk\" quick; \
                  \"write a script that backs up my photos\" larger; \
                  \"explain how a VPN works\" larger; \"compare these two laptops\" larger. Reply with one word: quick or larger.";
    let user = if last.is_empty() { format!("Request: {request}") } else { format!("Your last answer: {last}\n\nRequest: {request}") };
    let started = std::time::Instant::now();
    let decided = hemera.complete(model, &[spatiand_voice::hemera::Message::system(system), spatiand_voice::hemera::Message::user(user)], 4, std::time::Duration::from_secs(3));
    log::info!("brain: decided {:?} in {:.2} s", decided, started.elapsed().as_secs_f32());
    decided.is_ok_and(|d| d.to_lowercase().contains("larger"))
}

/// Say the whole sentences in `pending`, and with `all` the rest of it too -- but never code: a
/// fenced block is left to be read in the chat, and the voice says only that it is there.
fn speak_out(pending: &mut String, in_code: &mut bool, voice: &Voice, all: bool) {
    loop {
        if *in_code {
            match pending.find("```") {
                Some(end) => {
                    pending.drain(..end + 3);
                    *in_code = false;
                    // The language name or line after a closing fence is not prose either.
                    if let Some(line) = pending.find('\n') {
                        if pending[..line].trim().is_empty() {
                            pending.drain(..line + 1);
                        }
                    }
                }
                None => {
                    // Kept until the fence closes; a partial fence ("``") must not be lost. On a
                    // character's start: code ending in "é" or "日" is not cut inside it.
                    let mut keep = pending.len().saturating_sub(2);
                    while !pending.is_char_boundary(keep) {
                        keep -= 1;
                    }
                    pending.drain(..keep);
                    return;
                }
            }
            continue;
        }
        let fence = pending.find("```");
        let end = sentence_end(pending);
        match (fence, end) {
            (Some(f), Some(e)) if f < e => {
                voice.say(&pending[..f]);
                voice.say(code_said(&pending[..f]));
                pending.drain(..f + 3);
                *in_code = true;
            }
            (Some(f), None) => {
                voice.say(&pending[..f]);
                voice.say(code_said(&pending[..f]));
                pending.drain(..f + 3);
                *in_code = true;
            }
            (_, Some(e)) => {
                voice.say(&pending[..e]);
                pending.drain(..e);
            }
            (None, None) => {
                if all {
                    voice.say(pending);
                    pending.clear();
                }
                return;
            }
        }
    }
}

/// What the voice says where code is: in the language the text around it was in.
fn code_said(before: &str) -> &'static str {
    if looks_indonesian(before) || before.contains("ini") || before.contains("berikut") {
        "Kodenya ada di obrolan."
    } else {
        "The code is in the chat."
    }
}
