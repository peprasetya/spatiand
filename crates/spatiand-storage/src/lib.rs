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
    fn read_place() -> Option<Storage> {
        let raw = std::fs::read_to_string(runtime_file()).ok()?;
        let place: Place = serde_json::from_str(&raw).ok()?;
        let storage = Storage { port: place.port, token: place.token };
        storage.ping().then_some(storage)
    }

    fn ping(&self) -> bool {
        ureq::get(&format!("{}/ping", self.base())).set("Authorization", &self.auth()).timeout(Duration::from_millis(800)).call().is_ok()
    }

    /// The service, started if it is not running. Errs only when it cannot be started.
    pub fn connect() -> Result<Storage, String> {
        if let Some(storage) = Storage::read_place() {
            return Ok(storage);
        }
        let program = program().ok_or("the storage service is not installed beside this program")?;
        // On its own, not a child of this application: it serves native applications too, and
        // outlives whichever Spatiand application started it.
        let mut command = Command::new(&program);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        command.spawn().map_err(|e| format!("the storage service would not start: {e}"))?;
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(8) {
            std::thread::sleep(Duration::from_millis(100));
            if let Some(storage) = Storage::read_place() {
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
        let answer = ureq::post(&format!("{}/rc/{method}", self.base())).set("Authorization", &self.auth()).timeout(timeout).send_json(input);
        match answer {
            Ok(response) => response.into_json::<Value>().map_err(|e| e.to_string()),
            Err(ureq::Error::Status(_, response)) => {
                let body: Value = response.into_json().unwrap_or(Value::Null);
                Err(body.get("error").and_then(|e| e.as_str()).unwrap_or("the storage service refused").to_string())
            }
            Err(e) => Err(e.to_string()),
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
        let response = ureq::get(&url).set("Authorization", &self.auth()).timeout(Duration::from_secs(120)).call().map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(response.into_reader(), limit), &mut bytes).map_err(|e| e.to_string())?;
        Ok(bytes)
    }

    /// Copy the user's own rclone remotes in. Returns their names.
    pub fn import(&self) -> Result<Vec<String>, String> {
        let answer: Value = ureq::get(&format!("{}/import", self.base())).set("Authorization", &self.auth()).call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())?;
        Ok(answer.get("copied").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default())
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
}
