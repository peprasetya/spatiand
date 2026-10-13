//! Talks to one Vibrefy deployment.
//!
//! Ported from `data/VibrefyClient.kt`, trimmed to what a client with no browser needs.
//! Two surfaces, both the server's own (`CLIENT-API.md`):
//!
//!  - **Browsing, the watch list, progress and pairing** ask for `application/json`, which
//!    `Portal.wantsJson` routes to `content/<command>.jsp` and the bean's own `getJson()`.
//!    Authenticated by the `JSESSIONID` cookie.
//!  - **Streaming, subtitles and the sibling-file list** instead take the session id inline
//!    in the path and are declared public, which is what lets a media engine fetch a plain
//!    URL with no cookie plumbing at all - so [`VibrefyClient::session_id`] has to be known
//!    before anything can play.
//!
//! The Android client also parses the Ajax/XML `menu` and `welcome` fragments, to find out
//! who is signed in and which OpenID buttons a deployment offers. Neither is ported: there
//! is no embedded browser here to complete an OpenID flow, so sign-in is
//! [`crate::pairing`] and nothing else, and "is this session still alive" is answered by
//! asking `/files` - which is a question the JSON surface answers properly, with a 401.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::cookies::CookieJar;
use crate::json;
use crate::models::{FolderListing, MediaItem, SubtitleTrack};
use crate::pairing::{self, PairingLink, PairingOffer, PairingState};
use crate::paths::{encode_path, encode_segment};

/// What a call to the server says about the session behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// The server answered, and the answer was "not signed in". The session is gone.
    LoggedOut,
    /// The server did not answer at all - offline, wrong address, deployment down.
    ///
    /// Distinct from [`SessionState::LoggedOut`] on purpose. A lost session erases the
    /// stored account, and a passing network failure must never be able to trigger that.
    Unreachable,
    Active,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// A JSON 401. The server answers this in place of the welcome page precisely so a
    /// client with no browser can tell an ended session from an empty library.
    #[error("this Vibrefy session is not signed in")]
    NotAuthenticated,
    #[error("could not reach {url}: {source}")]
    Unreachable {
        url: String,
        #[source]
        source: Box<ureq::Transport>,
    },
    #[error("HTTP {status} for {command}")]
    Http { status: u16, command: String },
    #[error("Vibrefy sent no readable answer for {0}")]
    Unreadable(String),
    /// The request was understood and refused, in the server's own words. Passed through
    /// unchanged: the pairing throttle in particular is something the user needs told.
    #[error("{0}")]
    Server(String),
}

pub type Result<T> = std::result::Result<T, ApiError>;

/// The one thing about this client that reaches the server's user interface: `PairBean`
/// reads the agent to name the device on the approval screen, and special-cases these
/// words into "Steam Deck".
const USER_AGENT: &str = concat!(
    "Vibrefy/",
    env!("CARGO_PKG_VERSION"),
    " (SteamOS; Linux) vibrefy-linux"
);

pub struct VibrefyClient {
    /// Always ends in '/', which the server's `Referer`-derived redirect URI depends on.
    base_url: String,
    host: String,
    agent: ureq::Agent,
    jar: Mutex<CookieJar>,
    session_id: Mutex<Option<String>>,
}

impl VibrefyClient {
    pub fn new(base_url: &str) -> Self {
        let base_url = normalize(base_url);
        let host = host_of(&base_url);
        Self {
            base_url,
            host,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(30))
                // Redirects are refused rather than followed. `ureq` would re-send our
                // headers to wherever it landed, and a deployment that bounces to an
                // identity provider would be handed this deployment's session cookie.
                // The JSON surface has no legitimate redirect anyway - it answers 401.
                .redirects(0)
                .build(),
            jar: Mutex::new(CookieJar::new()),
            session_id: Mutex::new(None),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().unwrap().clone()
    }

    /// The account's stored cookies, for writing out between runs.
    pub fn cookies(&self) -> BTreeMap<String, String> {
        self.jar.lock().unwrap().snapshot()
    }

    pub fn restore_cookies(&self, cookies: BTreeMap<String, String>) {
        *self.jar.lock().unwrap() = CookieJar::restore(&self.host, cookies);
    }

    pub fn forget_session(&self) {
        *self.session_id.lock().unwrap() = None;
        self.jar.lock().unwrap().clear();
    }

    // ---- URLs the media engine consumes directly ---------------------------------------

    pub fn stream_url(&self, path: &str) -> Result<String> {
        self.session_url("stream", path)
    }

    pub fn subtitle_url(&self, path: &str, track: i64) -> Result<String> {
        Ok(format!("{}?track={track}", self.session_url("subtitle", path)?))
    }

    pub fn subtitle_list_url(&self, path: &str) -> Result<String> {
        self.session_url("subtitles", path)
    }

    pub fn media_list_url(&self, path: &str) -> Result<String> {
        self.session_url("medialist", path)
    }

    /// 320px wide. Cookie auth and **no session segment** - see [`Self::image`] for why
    /// that matters more than it looks.
    pub fn thumb_url(&self, path: &str) -> String {
        format!("{}thumbmp4/{}", self.base_url, encode_path(path))
    }

    pub fn cover_url(&self, path: &str) -> String {
        format!("{}covermp4/{}", self.base_url, encode_path(path))
    }

    fn session_url(&self, command: &str, path: &str) -> Result<String> {
        let id = self.session_id().ok_or(ApiError::NotAuthenticated)?;
        Ok(format!(
            "{}{command}/{}/{}",
            self.base_url,
            encode_segment(&id),
            encode_path(path)
        ))
    }

    // ---- The JSON surface ---------------------------------------------------------------

    /// `POST /<command>` asking for JSON. No `ajaxcall`: `Portal.wantsJson` refuses the
    /// JSON answer to anything carrying it, because that parameter means the browser's XML.
    fn post_json(&self, command: &str, form: &[(&str, &str)]) -> Result<Value> {
        let url = format!("{}{}", self.base_url, command.trim_start_matches('/'));
        let mut request = self
            .agent
            .post(&url)
            .set("Accept", "application/json")
            // Vibrefy derives the OpenID redirect URI from Referer and assumes it is there.
            .set("Referer", &self.base_url)
            .set("User-Agent", USER_AGENT);
        if let Some(cookie) = self.jar.lock().unwrap().header_for(&self.host) {
            request = request.set("Cookie", &cookie);
        }

        let response = match request.send_form(form) {
            Ok(response) => response,
            Err(ureq::Error::Status(401, _)) => return Err(ApiError::NotAuthenticated),
            Err(ureq::Error::Status(status, _)) => {
                return Err(ApiError::Http {
                    status,
                    command: command.to_string(),
                })
            }
            Err(ureq::Error::Transport(transport)) => {
                return Err(ApiError::Unreachable {
                    url,
                    source: Box::new(transport),
                })
            }
        };

        self.absorb_cookies(&response);
        let body = response
            .into_string()
            .map_err(|_| ApiError::Unreadable(command.to_string()))?;
        serde_json::from_str(body.trim()).map_err(|_| ApiError::Unreadable(command.to_string()))
    }

    fn absorb_cookies(&self, response: &ureq::Response) {
        let headers: Vec<String> = response
            .all("set-cookie")
            .into_iter()
            .map(str::to_string)
            .collect();
        if !headers.is_empty() {
            self.jar.lock().unwrap().store(&self.host, &headers);
        }
    }

    /// Is this session still worth anything? Refreshes [`Self::session_id`] on the way.
    ///
    /// Asked of `/files` rather than the Ajax `menu`, because `/files` is the command that
    /// carries the session id the stream URLs need, and because its 401 is unambiguous.
    pub fn session(&self) -> SessionState {
        match self.browse("") {
            Ok(_) => SessionState::Active,
            Err(ApiError::NotAuthenticated) => SessionState::LoggedOut,
            Err(ApiError::Unreachable { .. }) => SessionState::Unreachable,
            // A deployment that answered something unexpected is reachable and has not
            // said we are signed out, so the session is not thrown away over it.
            Err(_) => SessionState::Active,
        }
    }

    // ---- Browsing -----------------------------------------------------------------------

    /// `path` is empty for the library root, else `Library/sub/dir` or `~cloud/sub`.
    ///
    /// The listing carries the session id the stream URLs need, in a field put there for
    /// exactly this - one less thing a restyling of the web UI can break.
    pub fn browse(&self, path: &str) -> Result<FolderListing> {
        let reply = self.post_json(&format!("files/{}", encode_path(path)), &[])?;
        if let Some(session) = reply
            .get("session")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            *self.session_id.lock().unwrap() = Some(session.to_string());
        }
        if let Some(error) = reply
            .get("error")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return Err(ApiError::Server(error.to_string()));
        }
        Ok(json::folder(&reply))
    }

    /// What is part-watched, for the Home screen. The only shelf `/home` emits - and note
    /// it carries no `session` field, so [`Self::browse`] must have run at least once
    /// before any of these can be played.
    pub fn watching(&self) -> Result<Vec<MediaItem>> {
        Ok(json::watching(&self.post_json("home", &[])?))
    }

    // ---- Playback support ---------------------------------------------------------------

    /// Mirrors `saveProgress` in `scriptlib.js`. Anything below -10 is the server's signal
    /// to drop the entry; [`crate::progress::FINISHED`] is the value the web player sends.
    ///
    /// A refusal is raised rather than swallowed: a position that was not stored is one the
    /// Home shelf will not show, and the caller decides whether that is worth retrying.
    pub fn save_progress(&self, progress_key: &str, seconds: i64) -> Result<()> {
        let reply = self.post_json(
            "progress",
            &[("time", &seconds.to_string()), ("media", progress_key)],
        )?;
        if reply.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(())
        } else {
            Err(ApiError::Server(format!(
                "Vibrefy did not store the position for {progress_key}"
            )))
        }
    }

    pub fn subtitle_tracks(&self, path: &str) -> Result<Vec<SubtitleTrack>> {
        let Some(array) = self.get_json_array(&self.subtitle_list_url(path)?)? else {
            return Ok(Vec::new());
        };
        Ok(array
            .iter()
            .enumerate()
            .map(|(i, entry)| SubtitleTrack {
                index: entry
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or(i as i64),
                label: entry
                    .get("label")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("Subtitle {}", i + 1)),
                lang: entry
                    .get("lang")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
            .collect())
    }

    /// Names of the media files sitting beside `path`, for series auto-play.
    pub fn sibling_media(&self, path: &str) -> Result<Vec<String>> {
        let Some(array) = self.get_json_array(&self.media_list_url(path)?)? else {
            return Ok(Vec::new());
        };
        Ok(array
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// A GET on one of the session-in-the-path endpoints.
    ///
    /// These never answer 401 - `PathMap` resolves the id from the URL and simply returns
    /// when it does not resolve, so a dead session is **200 with an empty body**. That is
    /// reported here as [`ApiError::NotAuthenticated`], because it is the likeliest thing
    /// it means.
    ///
    /// The caller should not act on it alone, though. A connection reset part-way through
    /// reading produces the same empty body, and these endpoints are conveniences -
    /// subtitles and the sibling list. Whether the session is really over is a question
    /// for the JSON surface, which answers it with a 401 and cannot be mistaken.
    fn get_json_array(&self, url: &str) -> Result<Option<Vec<Value>>> {
        let response = self
            .agent
            .get(url)
            .set("Referer", &self.base_url)
            .set("User-Agent", USER_AGENT)
            .call();
        let body = match response {
            Ok(response) => response
                .into_string()
                .map_err(|_| ApiError::Unreadable(url.to_string()))?,
            Err(ureq::Error::Status(_, _)) => return Ok(None),
            Err(ureq::Error::Transport(transport)) => {
                return Err(ApiError::Unreachable {
                    url: url.to_string(),
                    source: Box::new(transport),
                })
            }
        };
        if body.trim().is_empty() {
            return Err(ApiError::NotAuthenticated);
        }
        Ok(serde_json::from_str::<Value>(body.trim())
            .ok()
            .and_then(|v| v.as_array().cloned()))
    }

    /// Artwork. Answers `None` for "there isn't one", which is ordinary and not an error.
    ///
    /// `Portal.doRequest` only sends the JSON 401 when the request asked for JSON, so an
    /// image fetch on a dead session falls through to the welcome command and comes back
    /// **200 with `text/html`**. The content type is therefore the check, not the status.
    pub fn image(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let mut request = self
            .agent
            .get(url)
            .set("Accept", "image/jpeg,image/*")
            .set("Referer", &self.base_url)
            .set("User-Agent", USER_AGENT);
        if let Some(cookie) = self.jar.lock().unwrap().header_for(&self.host) {
            request = request.set("Cookie", &cookie);
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::Status(_, _)) => return Ok(None),
            Err(ureq::Error::Transport(transport)) => {
                return Err(ApiError::Unreachable {
                    url: url.to_string(),
                    source: Box::new(transport),
                })
            }
        };
        if !response.content_type().starts_with("image/") {
            return Err(ApiError::NotAuthenticated);
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|_| ApiError::Unreadable(url.to_string()))?;
        Ok(Some(bytes))
    }

    // ---- Pairing ------------------------------------------------------------------------

    /// Asks for a code to put on screen, for someone to approve from a device that is
    /// already signed in. The countdown comes back with it, so nothing here has to guess
    /// at the life the server actually gave the code.
    ///
    /// The code is bound to *this* session, so the whole exchange - request, poll, and the
    /// sign-in that follows - has to run on one client.
    pub fn request_pairing_code(&self) -> Result<PairingOffer> {
        let reply = self.post_json("pair", &[("order", "Request")])?;
        let code = reply
            .get("code")
            .and_then(Value::as_str)
            .filter(|c| c.len() == crate::pairing::PairingCode::LENGTH);
        let Some(code) = code else {
            // The one reason to refuse is a throttle by address, which the user has to be
            // told about in the server's own words - waiting is the only thing that clears
            // it.
            return Err(ApiError::Server(
                reply
                    .get("message")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .unwrap_or("Vibrefy did not offer a pairing code")
                    .to_string(),
            ));
        };
        Ok(PairingOffer {
            code: code.to_string(),
            seconds_left: reply.get("seconds").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// Registers a key of our own, so a browser on this machine can approve the pairing.
    ///
    /// The other way in. `request_pairing_code` gives eight digits for somebody to type on
    /// a phone; this gives an address, and the device that approves it is the browser
    /// sitting on the same machine - which is what makes signing in with Google possible on
    /// a client that has no browser to keep a cookie in.
    ///
    /// The key goes in a path segment rather than a query string, and `PairBean.getLinkUrl`
    /// says why: a signed-out hit boots the server's own page shell, which resumes by
    /// re-requesting `window.location.pathname` - keeping the path and dropping the query.
    /// In the path the key survives the whole Google sign-in.
    ///
    /// The pairing is bound to *this* session exactly as a typed code is, so knowing the key
    /// is not enough to collect the approval: only the session that registered it can poll
    /// it. Which is also why the URL comes back from the server rather than being built
    /// here - `publicBase()` is the address the deployment answers to publicly, and it is
    /// not always the one this client was pointed at.
    pub fn request_pairing_link(&self, key: &str) -> Result<PairingLink> {
        let reply = self.post_json("pair", &[("order", "Link"), ("key", key)])?;
        let url = reply
            .get("url")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty());
        let Some(url) = url else {
            // No URL means the server would not take the key: it is already in flight, too
            // many pairings are open, or - the one that is not our fault to fix - this
            // deployment predates the link flow. Its own wording is the useful one.
            return Err(ApiError::Server(
                reply
                    .get("message")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .unwrap_or("This server does not offer a sign-in link.")
                    .to_string(),
            ));
        };
        Ok(PairingLink {
            key: reply
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or(key)
                .to_string(),
            url: url.to_string(),
            seconds_left: reply.get("seconds").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// Asks whether the code shown here has been approved yet. Cheap; meant for a loop.
    ///
    /// The session id rotates on the *successful* poll, so keep polling on the cookie this
    /// client already holds and re-read `/files` once the answer is `done`.
    pub fn poll_pairing(&self, code: &str) -> Result<PairingState> {
        Ok(pairing::parse(&self.post_json(
            "pair",
            &[("order", "Poll"), ("code", code)],
        )?))
    }

    /// Claims a code that an already signed-in device is showing. On success this session
    /// is signed in, its cookies are worth keeping, and the account it now belongs to comes
    /// back with the answer - so there is no second call to ask who we became.
    pub fn claim_pairing(&self, code: &str) -> Result<Option<String>> {
        match pairing::parse(&self.post_json("pair", &[("order", "Claim"), ("code", code)])?) {
            PairingState::Approved { account } => Ok(account),
            // Wrong and expired codes both come back with the server's own wording, and it
            // throttles guesses by address, so that wording is passed through unchanged.
            PairingState::Refused { reason } => Err(ApiError::Server(reason)),
            _ => Err(ApiError::Server(
                crate::pairing::REFUSED_DEFAULT.to_string(),
            )),
        }
    }
}

/// Always ends in '/', and gains a scheme when the user typed a bare host.
fn normalize(base_url: &str) -> String {
    let trimmed = base_url.trim();
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    if with_scheme.ends_with('/') {
        with_scheme
    } else {
        format!("{with_scheme}/")
    }
}

/// The host a URL addresses, for pinning the cookie jar to.
fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_url_always_ends_in_a_slash() {
        assert_eq!("https://media.example.com/", normalize("https://media.example.com"));
        assert_eq!("https://media.example.com/", normalize("https://media.example.com/"));
        assert_eq!("http://192.168.1.9:8080/", normalize("http://192.168.1.9:8080"));
    }

    /// Typing a bare host is the common case, and http would be the wrong guess.
    #[test]
    fn a_bare_host_is_assumed_to_be_https() {
        assert_eq!("https://media.example.com/", normalize("media.example.com"));
        assert_eq!("https://media.example.com/vibrefy/", normalize(" media.example.com/vibrefy "));
    }

    #[test]
    fn the_host_is_what_the_cookie_jar_pins_to() {
        assert_eq!("media.example.com", host_of("https://media.example.com/"));
        assert_eq!("192.168.1.9", host_of("http://192.168.1.9:8080/vibrefy/"));
        assert_eq!("media.example.com", host_of("https://user@media.example.com/"));
        assert_eq!("media.example.com", host_of("https://MEDIA.Example.com/"));
    }

    #[test]
    fn playback_urls_need_a_session_and_say_so() {
        let client = VibrefyClient::new("https://media.example.com");
        assert!(matches!(
            client.stream_url("Movies/A.mp4"),
            Err(ApiError::NotAuthenticated)
        ));
    }

    #[test]
    fn playback_urls_carry_the_session_and_the_encoded_path() {
        let client = VibrefyClient::new("https://media.example.com");
        *client.session_id.lock().unwrap() = Some("6E3A9F1C".into());

        assert_eq!(
            "https://media.example.com/stream/6E3A9F1C/Movies/Ep%201.mp4",
            client.stream_url("Movies/Ep 1.mp4").unwrap()
        );
        assert_eq!(
            "https://media.example.com/subtitle/6E3A9F1C/Movies/Ep%201.mp4?track=2",
            client.subtitle_url("Movies/Ep 1.mp4", 2).unwrap()
        );
        // Artwork has no session segment at all - it is authenticated by the cookie.
        assert_eq!(
            "https://media.example.com/thumbmp4/Movies/Ep%201.mp4",
            client.thumb_url("Movies/Ep 1.mp4")
        );
    }
}
