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

use std::sync::atomic::{AtomicBool, Ordering};
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
    pub text: String,
    pub history: Vec<Said>,
    pub look: bool,
    pub settings: Settings,
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
    stop: Arc<AtomicBool>,
}

impl Brain {
    pub fn start(room: Room, voice: Voice, told: Sender<Told>) -> Brain {
        let (turns, waiting) = channel::<Turn>();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("brain".into())
            .spawn(move || think(waiting, room, voice, told, stopped))
            .expect("a thread for the brain");
        Brain { turns, stop }
    }

    pub fn ask(&self, turn: Turn) {
        self.stop.store(false, Ordering::Relaxed);
        let _ = self.turns.send(turn);
    }

    /// Stop the answer being given.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn think(turns: Receiver<Turn>, room: Room, voice: Voice, told: Sender<Told>, stop: Arc<AtomicBool>) {
    // What waits for the wearer's yes, and since when.
    let mut held: Option<(Held, std::time::Instant)> = None;
    // One client for every turn, so its connections to hemera are kept from one to the next.
    let mut client: Option<(String, Hemera)> = None;
    while let Ok(turn) = turns.recv() {
        let address = VoiceSettings::load(&crate::store::shared()).hemera;
        let hemera = match &client {
            Some((at, h)) if *at == address => h.clone(),
            _ => {
                let h = Hemera::new(&address);
                client = Some((address, h.clone()));
                h
            }
        };
        let wake = room.wake.clone();
        let say = |t: Told| {
            let _ = told.send(t);
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
        match answer(&turn, &hemera, &room, &voice, &say, &stop, &mut held) {
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

fn answer(turn: &Turn, hemera: &Hemera, room: &Room, voice: &Voice, say: &dyn Fn(Told), stop: &AtomicBool, held: &mut Option<(Held, std::time::Instant)>) -> Result<Said, String> {
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
    let mut messages = vec![json!({"role": "system", "content": system(s, false)})];
    for said in remembered(&turn.history) {
        let mut text = said.text.clone();
        if !said.did.is_empty() {
            text.push_str(&format!("\n(Done in the room: {}.)", said.did.join("; ")));
        }
        messages.push(json!({"role": said.role, "content": text}));
    }
    // The room as it is, with the wearer's words; the window being looked at, shown, when the
    // wearer asked about "this".
    let words = format!("{}\n{}", room_note(&windows), turn.text);
    let mut user = json!({"role": "user", "content": words});
    if turn.look {
        let picture = room.ask(json!({"do": "picture", "largest": 1024}));
        if let Some(png) = picture.get("png").and_then(Value::as_str) {
            user["content"] = json!([
                {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{png}")}},
                {"type": "text", "text": words},
            ]);
        }
    }
    messages.push(user);

    let mut large = false;
    let mut model = s.model.clone();
    let mut said = Said { role: "assistant".into(), model: model.clone(), at: crate::store::now(), ..Default::default() };
    // Until the decision comes, what the small model writes is held back: shown and said only
    // once the answer is its to give.
    let mut decided: Option<bool> = if decision.is_some() { None } else { Some(false) };
    let mut first_words = false;
    for _ in 0..ROUNDS {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let ask = Ask {
            model: model.clone(),
            messages: messages.clone(),
            tools: tools::all(!large),
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
            if stop.load(Ordering::Relaxed) {
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
            log::info!("brain: {}({}) -> {}", call.name, call.arguments, done.content.chars().take(160).collect::<String>());
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
                {"type": "text", "text": "The window you looked at."},
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
    format!(
        "You are {name}, the wearer's assistant in Spatiand, a 3D desktop seen through glasses. {persona}\n\
         \n\
         Talk like a person: short, natural sentences. Answer in the language of the wearer's last message: English to English, Indonesian to Indonesian; only when they mix the two in that message, mix them too. What you write is spoken aloud: no markdown, no lists unless asked, no emoji.\n\
         \n\
         Each message from the wearer begins with a note in square brackets on the room as it is then: the time, the windows on show, and the hidden ones. The wearer did not say it; use it when it helps.\n\
         The windows on show are what the wearer sees and means. A hidden window is not in the room, and may be something on their computer's own screen: read it, look at it or bring it out only when they name it or ask for it. Asked what is open, talk about what is on show, and mention the hidden ones only if asked.\n\
         Told to go away or that it is all for now, call dismiss, and do nothing to any window.\n\
         \n\
         You act in the room with your tools: open applications, work windows as the wearer would (read them, look at them, click, right-click, scroll, drag, type, press keys), bring the room back in front of the wearer, and keep reminders. To find where to click, look at the window first; places are fractions of the window from 0 to 1, from its top left. Say briefly what you are doing.\n\
         Ask the wearer first before closing a window, deleting anything, sending a message, buying anything, or pressing Enter in a terminal.\n\
         {route}",
        name = s.name,
        persona = s.persona,
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
pub fn prime(s: &Settings, larger: bool) -> Value {
    json!({
        "model": if larger { &s.large_model } else { &s.model },
        "messages": [
            {"role": "system", "content": system(s, larger)},
            {"role": "user", "content": "[Getting ready: nothing is asked yet.]\nHi."},
        ],
        "tools": tools::all(!larger),
        "max_tokens": 1,
        "temperature": 0,
        "stream": false,
        // As its requests ask: a template may write the system message differently for it.
        "chat_template_kwargs": {"enable_thinking": larger},
    })
}

/// The room as it is now, for the wearer's message: the time, and the windows in it.
fn room_note(windows: &Value) -> String {
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

/// How much of the chat the model is given, at the least, and the steps its start moves in: the
/// start moving every turn would have the server read the whole chat again every turn.
const STEP: usize = 8;

/// The part of the chat the model is given: the last [`REMEMBERED`] things said or a few more.
fn remembered(history: &[Said]) -> &[Said] {
    let start = history.len().saturating_sub(REMEMBERED) / STEP * STEP;
    &history[start..]
}

/// Whether what the wearer said is a yes (`Some(true)`), a no (`Some(false)`), or neither: in
/// either language, as briefly as people say it.
fn agreement(text: &str) -> Option<bool> {
    let words: Vec<String> = text.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() || words.len() > 6 {
        return None;
    }
    const YES: &[&str] = &["ya", "iya", "yes", "yep", "yup", "yeah", "boleh", "oke", "ok", "okay", "lanjut", "lanjutkan", "silakan", "silahkan", "sure", "betul", "benar", "setuju", "gas", "go", "tekan", "tutup", "please"];
    const NO: &[&str] = &["tidak", "nggak", "ngga", "gak", "enggak", "jangan", "no", "nope", "batal", "batalkan", "cancel", "stop", "tunggu", "wait"];
    let first = words[0].as_str();
    if NO.contains(&first) || words.iter().any(|w| w == "jangan" || w == "batal") {
        Some(false)
    } else if YES.contains(&first) {
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
    use super::*;

    #[test]
    fn what_is_sent_begins_the_same_whatever_the_room() {
        let s = Settings::default();
        let a = system(&s, false);
        let note = room_note(&json!({"windows": [{"window": 3, "app": "id.prasetya.spatiand.spatiquill", "title": "~", "focused": true}]}));
        assert!(!a.contains("window 3") && note.contains("window 3: spatiquill"));
        assert_eq!(a, system(&s, false));
        assert!(note.starts_with("[It is ") && note.ends_with("(focused)]"));
        let both = room_note(&json!({"windows": [
            {"window": 6, "app": "Terminal", "title": "temp", "hidden": true},
            {"window": 3, "app": "spatiquill", "title": "~"},
        ]}));
        let (on_show, kept) = both.split_once("Hidden, not in the room:").unwrap();
        assert!(on_show.contains("window 3") && !on_show.contains("window 6") && kept.contains("window 6"));
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
                  and actions in the room: opening applications, working windows, clicking, typing, reminders, settings. \
                  The larger model answers what needs careful thought: writing or explaining code, calculations of several steps, \
                  analysis, planning, comparing, long or exact writing, translating long text. \
                  The quick model is told the time, the date and what is in the room, so questions about them are quick. \
                  For example: \"what time is it?\" quick; \"what day is it tomorrow?\" quick; \"buka browser\" quick; \
                  \"how many days until Friday?\" quick; \"write a script that backs up my photos\" larger; \
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
                    // Kept until the fence closes; a partial fence ("``") must not be lost.
                    let keep = pending.len().saturating_sub(2);
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
