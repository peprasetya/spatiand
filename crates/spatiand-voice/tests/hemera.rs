//! Dictation end to end against a real hemera: speech made by hemera's own voice model, fed in
//! as a microphone would, written back by Whisper and the small model.
//!
//! Needs deepmagpie, so it is ignored by default:
//! `cargo test -p spatiand-voice --test hemera -- --ignored --nocapture`
//! (`SPATIAND_HEMERA` for another server).

use std::time::{Duration, Instant};

use spatiand_voice::dictation::{Dictation, Event};
use spatiand_voice::hemera::read_wav;
use spatiand_voice::{Description, VoiceSettings, RATE};

fn settings() -> VoiceSettings {
    let mut s = VoiceSettings::default();
    if let Ok(h) = std::env::var("SPATIAND_HEMERA") {
        s.hemera = h;
    }
    s
}

/// `text` said by hemera's voice model, at [`RATE`].
fn say(text: &str) -> Vec<i16> {
    let body = serde_json::json!({"model": "voxcpm2", "input": text, "response_format": "wav", "voice": "fleurs-id-male"});
    let reply = ureq::post(&format!("{}/v1/audio/speech", settings().hemera))
        .config()
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .send_json(body)
        .expect("hemera's voice");
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut reply.into_body().into_reader(), &mut bytes).unwrap();
    let (pcm, rate) = read_wav(&bytes).expect("a WAV");
    resample(&pcm, rate, RATE)
}

fn resample(pcm: &[i16], from: u32, to: u32) -> Vec<i16> {
    let n = (pcm.len() as u64 * to as u64 / from as u64) as usize;
    (0..n)
        .map(|i| {
            let x = i as f64 * from as f64 / to as f64;
            let (a, f) = (x.floor() as usize, x.fract());
            let s0 = pcm[a.min(pcm.len() - 1)] as f64;
            let s1 = pcm[(a + 1).min(pcm.len() - 1)] as f64;
            (s0 + (s1 - s0) * f) as i16
        })
        .collect()
}

/// A room's quiet: faint noise, never digital silence.
fn quiet(ms: u32, seed: &mut u32) -> Vec<i16> {
    (0..RATE * ms / 1000)
        .map(|_| {
            *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((*seed >> 16) as i16 % 40) as i16
        })
        .collect()
}

/// Dictate `phrases` into `focus`; what would be typed, in order.
fn dictate(focus: Description, phrases: &[&str]) -> Vec<(String, bool)> {
    let _ = env_logger::builder().is_test(true).filter_module("spatiand_voice", log::LevelFilter::Debug).try_init();
    let said: Vec<Vec<i16>> = phrases.iter().map(|p| say(p)).collect();
    let d = Dictation::start(settings(), focus, Vec::new());
    let feed = d.feeder();
    let mut seed = 11;
    let mut stream = quiet(600, &mut seed);
    for s in &said {
        stream.extend_from_slice(s);
        stream.extend(quiet(1200, &mut seed));
    }
    // In 10 ms pieces, at the microphone's pace, as the compositor will.
    for chunk in stream.chunks((RATE / 100) as usize) {
        feed.feed(chunk);
        std::thread::sleep(Duration::from_millis(2));
    }
    d.stop();
    let mut out = Vec::new();
    let until = Instant::now() + Duration::from_secs(60);
    'wait: while Instant::now() < until {
        for e in d.events() {
            match e {
                Event::Text { text, review } => out.push((text, review)),
                Event::Trouble(why) => panic!("{why}"),
                Event::Done => break 'wait,
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    out
}

#[test]
#[ignore]
fn mixed_sentences_are_written_as_said() {
    let typed = dictate(
        Description { kind: "text".into(), app: "SpatiMesh".into(), title: "Notes".into(), ..Default::default() },
        &["Tolong cek lagi schedule-nya, terus kirim ke email aku ya.", "Restart Spatiand di Deck, terus coba scroll pakai left pad."],
    );
    println!("{typed:#?}");
    assert_eq!(typed.len(), 2);
    assert!(typed[0].0.contains("schedule"), "{}", typed[0].0);
    assert!(typed[0].0.contains("email"), "{}", typed[0].0);
    assert!(typed[1].0.contains("Spatiand"), "the word list: {}", typed[1].0);
    assert!(!typed[0].1 && !typed[1].1, "prose is typed straight away");
}

#[test]
#[ignore]
fn a_terminal_gets_commands() {
    let focus = Description {
        kind: "terminal".into(),
        app: "SpatiQuill".into(),
        title: "~/Documents/Linux/Spatiand".into(),
        directory: "~/Documents/Linux/Spatiand".into(),
        commands: vec!["git status".into(), "cargo build --release".into()],
        vocabulary: vec!["Cargo.toml".into(), "crates".into(), "docs".into(), "mac".into(), "android".into()],
        text: "paulus@darkphoenix Spatiand % git status\nOn branch master\nnothing to commit".into(),
    };
    let typed = dictate(focus, &["l s slash e t c slash h t t p d", "git commit dash m quote fix scroll quote"]);
    println!("{typed:#?}");
    assert_eq!(typed.len(), 2);
    assert_eq!(typed[0].0, "ls /etc/httpd");
    assert_eq!(typed[1].0, "git commit -m \"fix scroll\"");
    assert!(typed.iter().all(|(_, review)| *review), "a terminal's text is shown first");
}

#[test]
#[ignore]
fn a_streamed_answer_calls_a_tool_and_speech_streams() {
    use spatiand_voice::chat::{converse, tool, Ask, Piece};
    use spatiand_voice::speech::{say, Voice};
    let hemera = spatiand_voice::Hemera::new(&settings().hemera);
    let ask = Ask {
        model: "neohorse-1:4b".into(),
        messages: vec![serde_json::json!({"role": "user", "content": "Tolong buka SpatiQuill"})],
        tools: vec![tool("open_app", "Open an application in the room", serde_json::json!({"name": {"type": "string"}}), &["name"])],
        max_tokens: 100,
        temperature: 0.2,
        thinking: false,
    };
    let mut pieces = 0;
    let answer = converse(&hemera, &ask, |_: &Piece| {
        pieces += 1;
        true
    })
    .unwrap();
    println!("{answer:#?}");
    assert_eq!(answer.calls.len(), 1);
    assert_eq!(answer.calls[0].name, "open_app");
    assert!(answer.calls[0].arguments.contains("SpatiQuill"));
    let mut samples = 0usize;
    let started = Instant::now();
    let mut first = None;
    say(&hemera, &Voice { model: "voxcpm2".into(), ..Default::default() }, "Siap, saya buka SpatiQuill.", |pcm| {
        first.get_or_insert(started.elapsed());
        samples += pcm.len();
        true
    })
    .unwrap();
    println!("speech: first sound after {:?}, {:.1} s of it", first, samples as f32 / 24_000.0);
    assert!(samples > 24_000 / 2);
}
