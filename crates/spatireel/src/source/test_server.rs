//! A server on this machine for the sources' tests: what a server source does with a status,
//! a redirect, a large answer, a silence.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// A server on 127.0.0.1 answering each request with what `answer` makes of it (the request's
/// head and body as text), one request a connection; its address, and the requests it saw.
pub fn serve(answer: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
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

pub fn reply(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

/// A server whose `/hop/<n>` redirects to `/hop/<n - 1>`, and `/hop/0` answers: a chain of `n`
/// redirects.
pub fn hops() -> String {
    serve(|request| {
        let path = request.split_whitespace().nth(1).unwrap_or("");
        match path.strip_prefix("/hop/").and_then(|n| n.parse::<u32>().ok()) {
            Some(0) => reply("200 OK", &[("Content-Type", "application/json")], b"{}"),
            Some(n) => reply("302 Found", &[("Location", &format!("/hop/{}", n - 1))], b""),
            None => reply("404 Not Found", &[], b""),
        }
    })
    .0
}
