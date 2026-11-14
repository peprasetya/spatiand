//! How a Spatiand application reaches the storage service.
//!
//! The service (`storage/`, Go, with rclone linked in) holds the remotes; this finds it --
//! starting it if nobody has -- and speaks to it. A remote is a **virtual place**: a path
//! written `remote:folder/file`, which every Spatiand application understands and no operating
//! system has mounted. Its files are listed by rclone and streamed over the service's WebDAV;
//! nothing of them is kept on disk.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

/// Where the service is, as it wrote it down.
#[derive(Debug, Clone, Deserialize)]
struct Place {
    port: u16,
    token: String,
    #[serde(default)]
    pid: i32,
    /// The program it is and which build of it (`storage/main.go`, `programStamp`); missing
    /// from a service older than these.
    #[serde(default)]
    program: Option<String>,
    #[serde(default)]
    stamp: Option<String>,
}

impl Place {
    /// Whether the running service is an older build of the program installed beside this one:
    /// Spatiand was updated, and the service, which outlives the applications, was not. A
    /// service from somewhere else (another build, a test's) is left alone.
    fn outdated_by(&self, ours: &std::path::Path) -> bool {
        let Some(program) = &self.program else { return true };
        let ours = std::fs::canonicalize(ours).unwrap_or_else(|_| ours.to_path_buf());
        std::path::Path::new(program) == ours && self.stamp.as_deref() != stamp(&ours).as_deref()
    }
}

/// A program's size and modification time, as the service writes its own.
fn stamp(program: &std::path::Path) -> Option<String> {
    let meta = std::fs::metadata(program).ok()?;
    let nanos = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{}-{nanos}", meta.len()))
}

/// A connection to the service.
#[derive(Debug, Clone)]
pub struct Storage {
    port: u16,
    token: String,
}

/// One thing in a remote folder.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Entry {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "Size", default)]
    pub size: i64,
    #[serde(rename = "IsDir", default)]
    pub is_dir: bool,
    #[serde(rename = "ModTime", default)]
    pub modified: String,
    #[serde(rename = "MimeType", default)]
    pub mime: String,
}

/// A remote, by name, and what kind it is (`drive`, `sftp`, `webdav`...).
#[derive(Debug, Clone, PartialEq)]
pub struct Remote {
    pub name: String,
    pub kind: String,
}

/// A place written `remote:path`, split: `None` for a path on this machine. A name that is
/// not a remote's is not a remote, so a file whose name has a colon in it is still a file.
pub fn virtual_place(text: &str) -> Option<(String, String)> {
    if text.starts_with('/') || text.starts_with("http://") || text.starts_with("https://") || text.starts_with("file:") {
        return None;
    }
    let (remote, path) = text.split_once(':')?;
    if remote.is_empty() || remote.contains('/') {
        return None;
    }
    Some((remote.to_string(), path.trim_start_matches('/').to_string()))
}

fn runtime_file() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("spatiand-storage.json")
}

/// The service's program: beside this one, or in `SPATIAND_APPS_DIR` (on Android, the APK's
/// library folder, as `libspatiand-storage.so`).
fn program() -> Option<PathBuf> {
    let name = if cfg!(target_os = "android") { "libspatiand-storage.so" } else { "spatiand-storage" };
    let mut places = Vec::new();
    if let Some(dir) = std::env::var_os("SPATIAND_APPS_DIR") {
        places.push(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        places.push(dir);
    }
    places.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

fn percent(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Storage {
    fn read_place() -> Option<(Storage, Place)> {
        let raw = std::fs::read_to_string(runtime_file()).ok()?;
        let place: Place = serde_json::from_str(&raw).ok()?;
        let storage = Storage { port: place.port, token: place.token.clone() };
        storage.ping().then_some((storage, place))
    }

    /// Stop a service that is out of date, and wait until it no longer answers.
    fn replace(storage: &Storage, place: &Place) {
        log::info!("the storage service running (pid {}) is older than the one installed: replacing it", place.pid);
        #[cfg(unix)]
        if place.pid > 0 {
            unsafe { libc::kill(place.pid, libc::SIGTERM) };
        }
        let started = Instant::now();
        while storage.ping() && started.elapsed() < Duration::from_secs(4) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_file(runtime_file());
    }

    fn ping(&self) -> bool {
        within(agent().get(&format!("{}/ping", self.base())).header("Authorization", &self.auth()), Duration::from_millis(800)).call().is_ok()
    }

    /// The service, started if it is not running. Errs only when it cannot be started.
    pub fn connect() -> Result<Storage, String> {
        let program = program();
        if let Some((storage, place)) = Storage::read_place() {
            match &program {
                Some(ours) if place.outdated_by(ours) => Storage::replace(&storage, &place),
                _ => return Ok(storage),
            }
        }
        let program = program.ok_or("the storage service is not installed beside this program")?;
        // On its own, not a child of this application: it serves native applications too, and
        // outlives whichever Spatiand application started it.
        let mut command = Command::new(&program);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
            // Nothing of this application's but the three standard ones: the service outlives
            // it, and a lock or a socket it inherited would be held as long as it runs.
            unsafe {
                command.pre_exec(|| {
                    let top = (libc::sysconf(libc::_SC_OPEN_MAX) as i32).clamp(64, 65536);
                    for fd in 3..top {
                        libc::close(fd);
                    }
                    Ok(())
                });
            }
        }
        command.spawn().map_err(|e| format!("the storage service would not start: {e}"))?;
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(8) {
            std::thread::sleep(Duration::from_millis(100));
            if let Some((storage, _)) = Storage::read_place() {
                return Ok(storage);
            }
        }
        Err("the storage service did not answer".into())
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn auth(&self) -> String {
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("spatiand:{}", self.token)))
    }

    /// One of rclone's remote-control calls.
    pub fn rc(&self, method: &str, input: Value) -> Result<Value, String> {
        self.rc_waiting(method, input, Duration::from_secs(60))
    }

    /// The same, waiting as long as `timeout` -- signing in through a browser is minutes.
    pub fn rc_waiting(&self, method: &str, input: Value, timeout: Duration) -> Result<Value, String> {
        // A refusal's body says why, so a 4xx or 5xx is an answer to read here, not ureq's error.
        let request = agent().post(&format!("{}/rc/{method}", self.base())).header("Authorization", &self.auth());
        let answer = within(request, timeout).config().http_status_as_error(false).build().send_json(input);
        match answer {
            Ok(response) if response.status().as_u16() >= 400 => {
                let body: Value = json_of(response).unwrap_or(Value::Null);
                Err(body.get("error").and_then(|e| e.as_str()).unwrap_or("the storage service refused").to_string())
            }
            Ok(response) => json_of(response).map_err(|e| words(&e)),
            Err(e) => Err(words(&e)),
        }
    }

    pub fn remotes(&self) -> Result<Vec<Remote>, String> {
        let dump = self.rc("config/dump", json!({}))?;
        let mut remotes: Vec<Remote> = dump
            .as_object()
            .map(|all| all.iter().map(|(name, v)| Remote { name: name.clone(), kind: v.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string() }).collect())
            .unwrap_or_default();
        remotes.sort_by_key(|r| r.name.to_lowercase());
        Ok(remotes)
    }

    /// What is in a remote folder.
    pub fn list(&self, remote: &str, path: &str) -> Result<Vec<Entry>, String> {
        let answer = self.rc("operations/list", json!({"fs": format!("{remote}:"), "remote": path, "opt": {"noMimeType": false}}))?;
        let list = answer.get("list").cloned().unwrap_or(Value::Array(Vec::new()));
        serde_json::from_value(list).map_err(|e| e.to_string())
    }

    /// The address a remote file is streamed from -- by FFmpeg, by a native application --
    /// with what lets it in written into it.
    pub fn url(&self, remote: &str, path: &str) -> String {
        format!("http://spatiand:{}@127.0.0.1:{}/dav/{}/{}", self.token, self.port, percent(remote), percent(path.trim_start_matches('/')))
    }

    /// The network folder of all the remotes, for a native file manager.
    pub fn dav_root(&self) -> (String, String, String) {
        (format!("http://127.0.0.1:{}/dav/", self.port), "spatiand".to_string(), self.token.clone())
    }

    /// A remote file, whole, into memory: no more than `limit` bytes of it.
    pub fn read(&self, remote: &str, path: &str, limit: u64) -> Result<Vec<u8>, String> {
        let url = format!("{}/dav/{}/{}", self.base(), percent(remote), percent(path.trim_start_matches('/')));
        let response = within(agent().get(&url).header("Authorization", &self.auth()), Duration::from_secs(120)).call().map_err(|e| words(&e))?;
        let mut bytes = Vec::new();
        // ureq's reader has no limit of its own (as in ureq 2): the caller's is the one.
        std::io::Read::read_to_end(&mut std::io::Read::take(response.into_body().into_reader(), limit), &mut bytes).map_err(|e| e.to_string())?;
        Ok(bytes)
    }

    /// Copy the user's own rclone remotes in. Returns their names.
    pub fn import(&self) -> Result<Vec<String>, String> {
        let response = agent().get(&format!("{}/import", self.base())).header("Authorization", &self.auth()).call().map_err(|e| words(&e))?;
        let answer: Value = json_of(response).map_err(|e| words(&e))?;
        Ok(answer.get("copied").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default())
    }
}

/// How the service is asked, as ureq 2's own `ureq::get` asked it: an agent for the one call,
/// 30 seconds to connect and no limit on a read (a call is given its whole time by
/// [`within`]), four redirects followed and an error at the fifth. Two of ureq 3's defaults
/// would differ: it takes a proxy from the environment, which on a machine with `HTTP_PROXY`
/// set would send these calls to 127.0.0.1 through it; and it follows ten redirects.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .max_redirects(4)
        .proxy(None)
        .build()
        .into()
}

/// The request given `timeout` for all of it, from connecting to the answer's last byte: what
/// ureq 2's `Request::timeout` was.
fn within<B>(request: ureq::RequestBuilder<B>, timeout: Duration) -> ureq::RequestBuilder<B> {
    request.config().timeout_global(Some(timeout)).build()
}

/// The most of a JSON answer that is read. ureq 2 read JSON with no limit; ureq 3 stops at 10 MB
/// unless told. A listing of a folder of a hundred thousand files is some tens of megabytes, so
/// this is past anything real while still not letting a broken answer fill memory.
const JSON_LIMIT: u64 = 256 * 1024 * 1024;

fn json_of(response: ureq::http::Response<ureq::Body>) -> Result<Value, ureq::Error> {
    response.into_body().into_with_config().limit(JSON_LIMIT).read_json()
}

/// ureq's error in words fit to show: a socket's own error without the "io: " ureq 3 puts in
/// front of it.
fn words(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Io(e) => e.to_string(),
        ureq::Error::Timeout(_) => format!("no answer in time ({error})"),
        other => other.to_string(),
    }
}

/// Whether the user has remotes of their own in rclone's usual place, to offer to bring in.
pub fn own_rclone_config() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let path = home.join(".config/rclone/rclone.conf");
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_virtual_place_is_a_remote_and_a_path() {
        assert_eq!(virtual_place("Drive:Photos/2024/a.jpg"), Some(("Drive".into(), "Photos/2024/a.jpg".into())));
        assert_eq!(virtual_place("Drive:"), Some(("Drive".into(), String::new())));
        assert_eq!(virtual_place("/Users/me/a:b.txt"), None);
        assert_eq!(virtual_place("http://example.com/x"), None);
        assert_eq!(virtual_place("notes.txt"), None);
        assert_eq!(virtual_place("dir/a:b"), None);
    }

    #[test]
    fn addresses_are_escaped_and_carry_the_token() {
        let s = Storage { port: 47731, token: "t0k".into() };
        assert_eq!(s.url("My Drive", "a b/ü.jpg"), "http://spatiand:t0k@127.0.0.1:47731/dav/My%20Drive/a%20b/%C3%BC.jpg");
    }

    /// Known answer, as base64 0.22 wrote it: the header the service is asked with. Standard
    /// alphabet, padded, as HTTP's Basic wants.
    #[test]
    fn the_basic_header_is_byte_for_byte_what_it_was() {
        let s = Storage { port: 47731, token: "t0k?>".into() };
        assert_eq!(s.auth(), "Basic c3BhdGlhbmQ6dDBrPz4=");
    }

    #[test]
    fn an_older_build_of_our_own_service_is_replaced_and_anyone_elses_is_not() {
        let dir = std::env::temp_dir().join(format!("spatiand-storage-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ours = dir.join("spatiand-storage");
        std::fs::write(&ours, b"new build").unwrap();
        let ours = std::fs::canonicalize(&ours).unwrap();
        let place = |program: Option<&std::path::Path>, stamp: Option<String>| Place {
            port: 1,
            token: String::new(),
            pid: 0,
            program: program.map(|p| p.to_string_lossy().into_owned()),
            stamp,
        };
        assert!(place(None, None).outdated_by(&ours), "a service from before services said what they are");
        assert!(place(Some(&ours), Some("1-2".into())).outdated_by(&ours));
        assert!(!place(Some(&ours), stamp(&ours)).outdated_by(&ours));
        assert!(!place(Some(&dir.join("elsewhere")), Some("1-2".into())).outdated_by(&ours));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A stand-in for the service on 127.0.0.1, answering each request with what `answer` makes
    /// of it (head and body as text), one a connection; and the requests it saw.
    fn serve(answer: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> (Storage, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let answer = std::sync::Arc::new(answer);
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
        (Storage { port, token: "t0k".into() }, seen)
    }

    fn reply(status: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// The service's answers, its refusals in its own words, and a listing past ureq's own
    /// 10 MB limit read whole, as it was when JSON had no limit.
    #[test]
    fn the_service_is_asked_and_its_refusals_said() {
        let mut list = String::from(r#"{"list":["#);
        for n in 0..60_000 {
            if n > 0 {
                list.push(',');
            }
            list.push_str(&format!(r#"{{"Name":"f{n}","Path":"a/f{n}","Size":{n},"IsDir":false,"MimeType":"{}"}}"#, "x".repeat(150)));
        }
        list.push_str("]}");
        assert!(list.len() > 11 * 1024 * 1024);
        let (storage, seen) = serve(move |request| {
            let path = request.split_whitespace().nth(1).unwrap_or("");
            match path {
                "/rc/operations/list" if request.contains("\"Drive:\"") => reply("200 OK", list.as_bytes()),
                "/rc/operations/list" => reply("500 Internal Server Error", br#"{"error":"didn't find section in config file","status":500}"#),
                "/rc/config/dump" => reply("403 Forbidden", b"no"),
                "/import" => reply("200 OK", br#"{"copied":["Drive","Box"]}"#),
                "/ping" => reply("200 OK", b"{}"),
                _ => reply("404 Not Found", b""),
            }
        });
        let entries = storage.list("Drive", "a").unwrap();
        assert_eq!(entries.len(), 60_000);
        assert_eq!(entries[59_999].name, "f59999");
        assert_eq!(storage.list("Nowhere", "").unwrap_err(), "didn't find section in config file");
        assert_eq!(storage.remotes().unwrap_err(), "the storage service refused");
        assert_eq!(storage.import().unwrap(), vec!["Drive".to_string(), "Box".to_string()]);
        assert!(storage.ping());
        let asked = seen.lock().unwrap()[0].clone();
        let auth = format!("authorization: basic {}", base64::engine::general_purpose::STANDARD.encode("spatiand:t0k")).to_ascii_lowercase();
        assert!(asked.to_ascii_lowercase().contains(&auth), "{asked}");
    }

    /// A ping is given 0.8 seconds, all told, and a file's reading stops at its limit.
    #[test]
    fn a_ping_waits_no_longer_than_it_should_and_a_read_stops_at_its_limit() {
        let file: Vec<u8> = (0..3_000_000u32).map(|i| (i % 249) as u8).collect();
        let served = file.clone();
        let (storage, _) = serve(move |request| {
            if request.starts_with("GET /ping") {
                std::thread::sleep(Duration::from_secs(3));
                return reply("200 OK", b"{}");
            }
            let mut out = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", served.len()).into_bytes();
            out.extend_from_slice(&served);
            out
        });
        let started = Instant::now();
        assert!(!storage.ping());
        assert!(started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
        assert_eq!(storage.read("Drive", "a.bin", 1_000_000).unwrap(), file[..1_000_000]);
        assert_eq!(storage.read("Drive", "a.bin", 10_000_000).unwrap(), file);
        let nobody = Storage { port: std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port(), token: "t".into() };
        assert!(!nobody.ping());
        assert!(nobody.import().unwrap_err().contains("Connection refused"));
    }
}
