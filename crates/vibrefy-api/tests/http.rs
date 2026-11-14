//! The client against a server on this machine: what it does with a redirect, a 401, a server
//! that is not there, cookies and artwork. Written when ureq 2 was replaced by ureq 3, and run
//! against both, so that what a caller sees stayed the same.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vibrefy_api::{ApiError, SessionState, VibrefyClient};

/// A server on 127.0.0.1 answering each request with what `answer` makes of it (the request's
/// head and body as text), one request a connection; its address, and the requests it saw.
fn serve(answer: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
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
                let _ = stream.write_all(&answer(&request));
            });
        }
    });
    (address, seen)
}

/// A header of a request the server saw, its name in any case (ureq 3 sends names in small
/// letters, ureq 2 sent them as written; HTTP reads them either way).
fn header(request: &str, name: &str) -> Option<String> {
    request
        .lines()
        .find_map(|line| line.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim().to_string()))
}

fn reply(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

/// A redirect is not followed: whatever it points at is never asked, so it is never handed the
/// cookie; and the caller is told the answer was unreadable (a redirect is no JSON), not that
/// the server was unreachable or the session over.
#[test]
fn a_redirect_is_refused_and_its_target_never_asked() {
    let (elsewhere, asked_elsewhere) = serve(|_| reply("200 OK", &[("Content-Type", "application/json")], b"[]"));
    let target = format!("{elsewhere}/login");
    let (address, seen) = serve(move |_| reply("302 Found", &[("Location", &target), ("Set-Cookie", "JSESSIONID=ABC; Path=/")], b""));
    let client = VibrefyClient::new(&address);
    client.restore_cookies([("JSESSIONID".to_string(), "OLD".to_string())].into());

    let answer = client.browse("");
    assert!(matches!(answer, Err(ApiError::Unreadable(_))), "{answer:?}");
    assert_eq!(1, seen.lock().unwrap().len());
    assert_eq!(Some("JSESSIONID=OLD"), header(&seen.lock().unwrap()[0], "cookie").as_deref());
    assert!(asked_elsewhere.lock().unwrap().is_empty(), "the redirect was followed");
    // The cookie the redirect set is taken, as it always was.
    assert_eq!(Some("ABC"), client.cookies().get("JSESSIONID").map(String::as_str));
    // A redirect says nothing about the session: it is not thrown away over one.
    assert_eq!(SessionState::Active, client.session());
}

#[test]
fn a_401_is_the_session_ended_and_another_status_is_http() {
    let (address, _) = serve(|request| {
        if request.starts_with("POST /files/") {
            reply("401 Unauthorized", &[("Content-Type", "application/json")], br#"{"error":"login"}"#)
        } else {
            reply("503 Service Unavailable", &[], b"down")
        }
    });
    let client = VibrefyClient::new(&address);
    assert!(matches!(client.browse(""), Err(ApiError::NotAuthenticated)));
    assert_eq!(SessionState::LoggedOut, client.session());
    match client.watching() {
        Err(ApiError::Http { status: 503, command }) => assert_eq!("home", command),
        other => panic!("{other:?}"),
    }
}

/// Nothing listening: unreachable, said readably, and asked three times more first (the
/// network waking from sleep refuses the first connection, `again`).
#[test]
fn a_server_not_there_is_unreachable_after_three_more_tries() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let client = VibrefyClient::new(&format!("http://127.0.0.1:{port}"));
    let started = Instant::now();
    let answer = client.browse("");
    // 300 + 600 + 900 ms between the four tries.
    assert!(started.elapsed() >= Duration::from_millis(1800), "{:?}", started.elapsed());
    let Err(e @ ApiError::Unreachable { .. }) = answer else { panic!("{answer:?}") };
    let said = e.to_string();
    assert!(said.starts_with(&format!("could not reach http://127.0.0.1:{port}/files/: ")), "{said}");
    assert!(said.contains("Connection refused"), "{said}");
    assert_eq!(SessionState::Unreachable, client.session());
}

#[test]
fn cookies_are_kept_and_sent_back() {
    let (address, seen) = serve(|_| {
        let mut out = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: JSESSIONID=S1; Path=/; HttpOnly\r\nSet-Cookie: remember=R2; Path=/\r\nConnection: close\r\n".to_vec();
        let body = br#"{"session":"6E3A","folders":[],"files":[]}"#;
        out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        out.extend_from_slice(body);
        out
    });
    let client = VibrefyClient::new(&address);
    client.browse("").unwrap();
    assert_eq!(Some("6E3A".to_string()), client.session_id());
    let cookies = client.cookies();
    assert_eq!(Some("S1"), cookies.get("JSESSIONID").map(String::as_str));
    assert_eq!(Some("R2"), cookies.get("remember").map(String::as_str));
    client.browse("Films").unwrap();
    let second = seen.lock().unwrap()[1].clone();
    let cookie = header(&second, "cookie").unwrap_or_default();
    assert!(cookie.contains("JSESSIONID=S1") && cookie.contains("remember=R2"), "{second}");
    assert!(header(&second, "user-agent").unwrap_or_default().starts_with("Vibrefy/"), "{second}");
    assert_eq!(Some("application/json"), header(&second, "accept").as_deref(), "{second}");
    assert_eq!(Some(format!("{address}/")), header(&second, "referer"), "{second}");
}

/// Artwork goes by its content type: an image is the picture, a page is a session gone, and a
/// status is "there is none".
#[test]
fn artwork_is_known_by_its_content_type() {
    let picture: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let served = picture.clone();
    let (address, _) = serve(move |request| {
        if request.starts_with("GET /thumbmp4/a.mp4") {
            reply("200 OK", &[("Content-Type", "image/jpeg")], &served)
        } else if request.starts_with("GET /thumbmp4/b.mp4") {
            reply("200 OK", &[("Content-Type", "text/html; charset=utf-8")], b"<html>welcome</html>")
        } else {
            reply("404 Not Found", &[], b"")
        }
    });
    let client = VibrefyClient::new(&address);
    assert_eq!(Some(picture), client.image(&client.thumb_url("a.mp4")).unwrap());
    assert!(matches!(client.image(&client.thumb_url("b.mp4")), Err(ApiError::NotAuthenticated)));
    assert_eq!(None, client.image(&client.thumb_url("c.mp4")).unwrap());
}

/// The session-in-the-path lists: a status is "nothing", an empty body a session gone.
#[test]
fn the_side_lists_answer_nothing_for_a_status() {
    let (address, _) = serve(|request| {
        if request.starts_with("GET /subtitles/") {
            reply("200 OK", &[], br#"[{"index":3,"label":"English","lang":"en"}]"#)
        } else if request.starts_with("GET /medialist/") {
            reply("500 Internal Server Error", &[], b"oops")
        } else {
            reply("200 OK", &[("Content-Type", "application/json")], br#"{"session":"S"}"#)
        }
    });
    let client = VibrefyClient::new(&address);
    client.browse("").unwrap();
    let tracks = client.subtitle_tracks("a.mkv").unwrap();
    assert_eq!(1, tracks.len());
    assert_eq!(3, tracks[0].index);
    assert!(client.sibling_media("a.mkv").unwrap().is_empty());
}

/// A server that takes the request and says nothing is given up on after 30 seconds of
/// silence. Half a minute, so not run by default:
/// `cargo test -p vibrefy-api --test http -- --ignored`.
#[test]
#[ignore]
fn a_silent_server_is_given_up_after_thirty_seconds() {
    let (address, _) = serve(|_| {
        std::thread::sleep(Duration::from_secs(45));
        Vec::new()
    });
    let client = VibrefyClient::new(&address);
    let started = Instant::now();
    let answer = client.browse("");
    let took = started.elapsed();
    assert!(matches!(answer, Err(ApiError::Unreachable { .. })), "{answer:?}");
    assert!(took >= Duration::from_secs(29) && took < Duration::from_secs(40), "{took:?}");
    println!("{took:?}: {}", answer.unwrap_err());
}
