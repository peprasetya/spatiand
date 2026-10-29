//! spatikey-host: how the browsers reach SpatiKey (docs/spatikey.md, "The extensions").
//!
//! Chrome and Firefox start it for the SpatiKey extension and speak *native messaging* to it on
//! its standard input and output: each message is a 4-byte length in this machine's own byte
//! order, then that much JSON. It hands each one to SpatiKey's service over the service's local
//! socket and the answer back, and that is all: it keeps nothing, reads nothing of the vault, and
//! writes no message into a log, since any of them may carry a password.
//!
//! Who is asking is the browser's to say. Chrome starts a host with the calling extension's
//! origin (`chrome-extension://<id>/`) as its argument, Firefox with the host manifest's path and
//! the extension's id; a message's own `who` is replaced with that, so that neither a page nor
//! the extension can give itself another name in the Touch ID prompt, or claim to be another
//! device.
//!
//! The service is connected to when the first message comes, and once more if that connection
//! has gone when the next does (Spatiand restarted meanwhile). When it is not there at all, the
//! extension is told so in the protocol's own words, `not_here`, rather than left waiting.

use std::io::{self, Read, Write};

use serde_json::{json, Value};
use spatikey_core::socket::Client;
use spatikey_core::wire::{Code, Refusal, Reply};

/// The most a browser may send in one message. SpatiKey's requests are small: this is room for
/// any of them, and a guard against a length that is nonsense.
const MAX_IN: usize = 1 << 20;
/// The most a browser takes in one message: Chrome refuses a host's message over 1 MiB.
const MAX_OUT: usize = 1 << 20;

/// What the service is told the caller is when the browser cannot be told from the arguments.
const UNKNOWN: &str = "Browser";

/// The browser that started this, by what it passed: Chrome (and every browser built on
/// Chromium) the extension's origin, Firefox its manifest's path and the extension's id.
fn browser(args: &[String]) -> &'static str {
    if args.iter().any(|a| a.starts_with("chrome-extension://")) {
        return "Chrome";
    }
    let manifest = args.iter().any(|a| a.ends_with(".json"));
    // An add-on's id is an address-like name (spatikey@prasetya.id) or a GUID in braces.
    let addon = args.iter().any(|a| a.contains('@') || (a.starts_with('{') && a.ends_with('}')));
    if manifest || addon {
        return "Firefox";
    }
    UNKNOWN
}

/// One message from the browser; None when it has closed the pipe between two, which is how it
/// says the extension is done with this host.
fn read_message(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0u8; 4];
    let mut got = 0;
    while got < length.len() {
        match input.read(&mut length[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the browser stopped in the middle of a message's length")),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let length = u32::from_ne_bytes(length) as usize;
    if length > MAX_IN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a message of {length} bytes is larger than SpatiKey takes")));
    }
    let mut body = vec![0u8; length];
    input.read_exact(&mut body)?;
    Ok(Some(body))
}

/// One message to the browser.
fn write_message(output: &mut impl Write, json: &[u8]) -> io::Result<()> {
    if json.len() > MAX_OUT {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "an answer larger than a browser takes"));
    }
    output.write_all(&(json.len() as u32).to_ne_bytes())?;
    output.write_all(json)?;
    output.flush()
}

/// The message with `who` saying it is `browser` and nothing else, and its id, for an answer of
/// this host's own. Anything else in it is the service's to judge.
fn stamped(message: &[u8], browser: &str) -> Result<(Vec<u8>, u64), String> {
    let mut value: Value = serde_json::from_slice(message).map_err(|_| "a message that is not JSON".to_string())?;
    let Some(object) = value.as_object_mut() else { return Err("a message that is not a request".into()) };
    let id = object.get("id").and_then(Value::as_u64).ok_or("a request with no id")?;
    object.insert("who".into(), json!({ "app": browser }));
    let stamped = serde_json::to_vec(&value).map_err(|e| format!("the request could not be written again: {e}"))?;
    Ok((stamped, id))
}

/// An answer of this host's own: a refusal, in the protocol's words.
fn refusal(id: u64, code: Code, message: &str) -> Vec<u8> {
    serde_json::to_vec(&Reply::err(id, Refusal::new(code, message))).unwrap_or_default()
}

/// Something a request can be handed to and an answer had back from: the service's client.
trait Relay {
    fn relay(&mut self, json: &[u8]) -> io::Result<Vec<u8>>;
}

impl Relay for Client {
    fn relay(&mut self, json: &[u8]) -> io::Result<Vec<u8>> {
        Client::relay(self, json)
    }
}

/// The connection to the service: made when first wanted, and made again once when it fails.
struct Link<R, C> {
    connect: C,
    held: Option<R>,
}

impl<R: Relay, C: FnMut() -> io::Result<R>> Link<R, C> {
    fn new(connect: C) -> Self {
        Link { connect, held: None }
    }

    /// Hand `json` to the service and wait for its answer: on the connection held, or a new
    /// one; and if the one held has gone, on one new one. A request too large for the service
    /// is not the connection's fault and is not tried again.
    fn relay(&mut self, json: &[u8]) -> io::Result<Vec<u8>> {
        let mut tries = 0;
        loop {
            tries += 1;
            let fresh = self.held.is_none();
            if fresh {
                self.held = Some((self.connect)()?);
            }
            let Some(held) = self.held.as_mut() else { unreachable!() };
            match held.relay(json) {
                Ok(answer) => return Ok(answer),
                Err(e) if e.kind() == io::ErrorKind::InvalidInput => return Err(e),
                Err(e) => {
                    self.held = None;
                    if fresh || tries >= 2 {
                        return Err(e);
                    }
                }
            }
        }
    }
}

/// What the browser is answered for `message`.
fn answer<R: Relay, C: FnMut() -> io::Result<R>>(link: &mut Link<R, C>, message: &[u8], browser: &str) -> Vec<u8> {
    let (stamped, id) = match stamped(message, browser) {
        Ok(stamped) => stamped,
        Err(why) => return refusal(0, Code::BadRequest, &why),
    };
    match link.relay(&stamped) {
        Ok(answer) if answer.len() <= MAX_OUT => answer,
        Ok(_) => refusal(id, Code::Failed, "SpatiKey's answer is larger than a browser takes"),
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => refusal(id, Code::BadRequest, "a request larger than SpatiKey takes"),
        Err(_) => refusal(id, Code::NotHere, "Spatiand is not running on this computer"),
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let browser = browser(&args);
    let mut link = Link::new(|| Client::connect(browser));
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let message = match read_message(&mut input) {
            Ok(Some(message)) => message,
            Ok(None) => return,
            Err(e) => {
                // Never the message itself: only that the pipe made no sense.
                log::warn!("spatikey-host: {e}; stopping");
                std::process::exit(1);
            }
        };
        let answer = answer(&mut link, &message, browser);
        if let Err(e) = write_message(&mut output, &answer) {
            log::warn!("spatikey-host: the browser could not be answered: {e}; stopping");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_a_native_length_then_json() {
        let mut pipe = Vec::new();
        write_message(&mut pipe, br#"{"id":1}"#).unwrap();
        write_message(&mut pipe, b"{}").unwrap();
        assert_eq!(pipe[..4], 8u32.to_ne_bytes());
        let mut reading = pipe.as_slice();
        assert_eq!(read_message(&mut reading).unwrap().unwrap(), br#"{"id":1}"#);
        assert_eq!(read_message(&mut reading).unwrap().unwrap(), b"{}");
        // The browser closing the pipe between messages is the end, not an error.
        assert!(read_message(&mut reading).unwrap().is_none());
    }

    #[test]
    fn a_message_cut_short_or_too_large_is_an_error() {
        assert!(read_message(&mut [8u8, 0].as_slice()).is_err());
        let mut short = 8u32.to_ne_bytes().to_vec();
        short.extend_from_slice(b"{}");
        assert!(read_message(&mut short.as_slice()).is_err());
        let large = ((MAX_IN + 1) as u32).to_ne_bytes();
        assert_eq!(read_message(&mut large.as_slice()).unwrap_err().kind(), io::ErrorKind::InvalidData);
        // Exactly the most is taken.
        let mut most = (MAX_IN as u32).to_ne_bytes().to_vec();
        most.resize(4 + MAX_IN, b' ');
        assert_eq!(read_message(&mut most.as_slice()).unwrap().unwrap().len(), MAX_IN);
        // And nothing larger than a browser takes is sent to it.
        assert!(write_message(&mut Vec::new(), &vec![b' '; MAX_OUT + 1]).is_err());
    }

    #[test]
    fn who_asks_is_the_browser_whatever_the_message_says() {
        let message = br#"{"id":5,"op":"fill","origin":"https://github.com","who":{"app":"Bank","device":"Steam Deck"}}"#;
        let (sent, id) = stamped(message, "Firefox").unwrap();
        assert_eq!(id, 5);
        let value: Value = serde_json::from_slice(&sent).unwrap();
        assert_eq!(value["who"], json!({ "app": "Firefox" }));
        assert_eq!(value["origin"], "https://github.com");
        assert_eq!(value["op"], "fill");
        // With none given it is added.
        let (sent, _) = stamped(br#"{"id":6,"op":"status"}"#, "Chrome").unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&sent).unwrap()["who"]["app"], "Chrome");
        assert!(stamped(b"not json", "Chrome").is_err());
        assert!(stamped(br#"{"op":"status"}"#, "Chrome").is_err());
        assert!(stamped(b"[1,2]", "Chrome").is_err());
    }

    #[test]
    fn browsers_are_told_apart_by_what_they_start_it_with() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(browser(&args(&["chrome-extension://abcdefghijklmnopabcdefghijklmnop/"])), "Chrome");
        assert_eq!(browser(&args(&["chrome-extension://abcd/", "--parent-window=0"])), "Chrome");
        assert_eq!(
            browser(&args(&["/Users/p/Library/Application Support/Mozilla/NativeMessagingHosts/id.prasetya.spatikey.json", "spatikey@prasetya.id"])),
            "Firefox"
        );
        assert_eq!(browser(&args(&["{0f1e2d3c-1111-2222-3333-444455556666}"])), "Firefox");
        assert_eq!(browser(&args(&[])), UNKNOWN);
    }

    /// A stand-in for the service: answers by echoing, or fails as told.
    struct Fake {
        fails: bool,
    }

    impl Relay for Fake {
        fn relay(&mut self, json: &[u8]) -> io::Result<Vec<u8>> {
            if self.fails {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"));
            }
            let value: Value = serde_json::from_slice(json).unwrap();
            Ok(serde_json::to_vec(&json!({ "id": value["id"], "ok": { "who": value["who"] } })).unwrap())
        }
    }

    #[test]
    fn not_here_when_the_service_is_not_running() {
        let mut link = Link::new(|| -> io::Result<Fake> { Err(io::Error::new(io::ErrorKind::NotFound, "no socket")) });
        let said = answer(&mut link, br#"{"id":42,"op":"status"}"#, "Chrome");
        assert_eq!(String::from_utf8(said).unwrap(), r#"{"id":42,"err":{"code":"not_here","message":"Spatiand is not running on this computer"}}"#);
    }

    #[test]
    fn the_service_is_connected_to_once_and_again_once_when_it_went() {
        // Connected to when the first message comes, and the connection kept for the next.
        let mut connects = 0;
        let mut link = Link::new(|| {
            connects += 1;
            Ok(Fake { fails: false })
        });
        let said: Value = serde_json::from_slice(&answer(&mut link, br#"{"id":1,"op":"status"}"#, "Chrome")).unwrap();
        assert_eq!(said["ok"]["who"]["app"], "Chrome");
        assert_eq!(said["id"], 1);
        // Lazily, and kept: a second message goes on the same connection.
        answer(&mut link, br#"{"id":2,"op":"status"}"#, "Chrome");
        drop(link);
        assert_eq!(connects, 1);

        // The connection held from before has gone; a new one answers.
        let mut made = 0;
        let mut link = Link::new(|| {
            made += 1;
            Ok(Fake { fails: false })
        });
        link.held = Some(Fake { fails: true });
        let said: Value = serde_json::from_slice(&answer(&mut link, br#"{"id":3,"op":"status"}"#, "Firefox")).unwrap();
        assert_eq!(said["id"], 3);
        assert!(said.get("ok").is_some(), "the request was tried again on a new connection");
        drop(link);
        assert_eq!(made, 1, "one new connection, no more");
    }

    #[test]
    fn a_service_that_fails_twice_is_not_here() {
        let mut link = Link::new(|| Ok(Fake { fails: true }));
        link.held = Some(Fake { fails: true });
        let said: Value = serde_json::from_slice(&answer(&mut link, br#"{"id":9,"op":"status"}"#, "Chrome")).unwrap();
        assert_eq!(said["err"]["code"], "not_here");
        assert_eq!(said["id"], 9);
    }

    #[test]
    fn what_is_not_a_request_is_refused_without_the_service() {
        let mut link = Link::new(|| -> io::Result<Fake> { panic!("the service is not asked") });
        let said: Value = serde_json::from_slice(&answer(&mut link, b"hello", "Chrome")).unwrap();
        assert_eq!(said["err"]["code"], "bad_request");
    }
}
