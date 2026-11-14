//! The hemera client against a server on this machine: hemera's own words for a refusal, a
//! server that is not there, a large answer, a time limit, a redirect. Written when ureq 2 was
//! replaced by ureq 3, and run against both, so that what a caller sees stayed the same.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use spatiand_voice::hemera::{Hemera, Message};

/// A server on 127.0.0.1 answering each request by writing what `answer` likes to the
/// connection (the request's head and body as text), one request a connection; its address,
/// and the requests it saw.
fn serve(answer: impl Fn(&str, &mut dyn Write) + Send + Sync + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let answer = Arc::new(answer);
    let kept = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (seen, answer) = (kept.clone(), answer.clone());
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let length = head
                    .lines()
                    .find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("content-length")))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                let _ = stream.read_exact(&mut body);
                let request = head + &String::from_utf8_lossy(&body);
                seen.lock().unwrap().push(request.clone());
                answer(&request, &mut stream);
            });
        }
    });
    (address, seen)
}

fn reply(out: &mut dyn Write, status: &str, content_type: &str, body: &[u8]) {
    let _ = out.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
    let _ = out.write_all(body);
}

fn path(request: &str) -> &str {
    request.split_whitespace().nth(1).unwrap_or("")
}

/// A refusal is said in hemera's own words, and a refusal without any by its number.
#[test]
fn a_refusal_is_said_in_hemeras_words() {
    let (address, _) = serve(|request, out| match path(request) {
        "/v1/chat/completions" => reply(out, "503 Service Unavailable", "application/json", br#"{"error":"the model is still loading"}"#),
        "/v1/audio/speech" => reply(out, "500 Internal Server Error", "application/json", br#"{"error":"no such voice"}"#),
        _ => reply(out, "404 Not Found", "text/plain", b"nothing here"),
    });
    let hemera = Hemera::new(&address);
    let said = hemera.complete("m", &[Message::user("hi")], 8, Duration::from_secs(5)).unwrap_err();
    assert_eq!(said, "the model is still loading");
    assert_eq!(hemera.get_json("/nothing").unwrap_err(), "hemera answered 404");
    assert_eq!(hemera.models("text").unwrap_err(), "hemera answered 404");
    let streamed = hemera.post_stream("/v1/audio/speech", &json!({}), Duration::from_secs(5));
    assert_eq!(streamed.err().unwrap(), "no such voice");
}

#[test]
fn a_server_not_there_is_said_to_be_unreachable() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let hemera = Hemera::new(&format!("http://127.0.0.1:{port}/"));
    let said = hemera.get_json("/v1/models").unwrap_err();
    assert!(said.starts_with(&format!("hemera at http://127.0.0.1:{port} could not be reached (")), "{said}");
    assert!(said.contains("Connection refused"), "{said}");
}

/// Speech comes back as base64 inside JSON, and a long answer is many megabytes: read whole,
/// as ureq 2 read JSON with no limit (ureq 3's own is 10 MB). The streamed answer likewise.
#[test]
fn a_large_answer_is_read_whole() {
    let audio = "QUJD".repeat(8 * 1024 * 1024); // 32 MB of base64
    let big = Arc::new(format!(r#"{{"audio":"{audio}","data":[{{"id":"voxcpm2","hemera":{{"kind":"speech"}}}},{{"id":"qwen","hemera":{{"kind":"text"}}}}]}}"#));
    let stream: Arc<Vec<u8>> = Arc::new((0..20_000_000u32).map(|i| (i % 253) as u8).collect());
    let (served, streamed) = (big.clone(), stream.clone());
    let (address, seen) = serve(move |request, out| match path(request) {
        "/v1/audio/speech" => reply(out, "200 OK", "text/event-stream", &streamed),
        _ => reply(out, "200 OK", "application/json", served.as_bytes()),
    });
    let hemera = Hemera::new(&address);
    let answer = hemera.post_json("/v1/audio/speech-json", &json!({"input": "hello"}), Duration::from_secs(60)).unwrap();
    assert_eq!(answer["audio"].as_str().map(str::len), Some(audio.len()));
    assert_eq!(hemera.get_json("/v1/models").unwrap()["data"][1]["id"], "qwen");
    assert_eq!(hemera.models("speech").unwrap(), vec!["voxcpm2".to_string()]);
    let mut got = Vec::new();
    hemera.post_stream("/v1/audio/speech", &json!({"input": "hello"}), Duration::from_secs(60)).unwrap().read_to_end(&mut got).unwrap();
    assert!(got == *stream, "the streamed answer came back changed ({} bytes)", got.len());
    let asked = seen.lock().unwrap().iter().find(|r| r.starts_with("POST /v1/audio/speech ")).cloned().unwrap();
    assert!(asked.to_ascii_lowercase().contains("accept: text/event-stream"), "{asked}");
}

/// A call's time is all of it, the answer's body too: a server that sends its head at once and
/// then a byte now and then is given up on when the time is out.
#[test]
fn a_calls_time_covers_its_whole_answer() {
    let (address, _) = serve(|_, out| {
        let _ = out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n{\"text\":\"");
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(100));
            if out.write_all(b"a").is_err() {
                return;
            }
        }
    });
    let hemera = Hemera::new(&address);
    let started = Instant::now();
    let said = hemera.post_json("/v1/chat/completions", &json!({}), Duration::from_millis(1500)).unwrap_err();
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(1400) && took < Duration::from_secs(4), "{took:?}: {said}");
    assert!(said.starts_with("hemera's answer was not readable: "), "{said}");
}

/// Four redirects followed, not five: ureq 2's default.
#[test]
fn four_redirects_are_followed_and_not_five() {
    let (address, _) = serve(|request, out| match path(request).strip_prefix("/hop/").and_then(|n| n.parse::<u32>().ok()) {
        Some(0) => reply(out, "200 OK", "application/json", b"{}"),
        Some(n) => {
            let _ = out.write_all(format!("HTTP/1.1 302 Found\r\nLocation: /hop/{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", n - 1).as_bytes());
        }
        None => reply(out, "404 Not Found", "text/plain", b""),
    });
    let hemera = Hemera::new(&address);
    assert!(hemera.get_json("/hop/4").is_ok());
    assert!(hemera.get_json("/hop/5").is_err());
}
