//! Spatiand's applications' settings, for this Mac as a host: the same logic as every other
//! end's (`spatiand-sync`), reached from the Mac app's Swift host by two calls with JSON.

use std::ffi::{c_char, CStr, CString};
use std::sync::Mutex;

use spatiand_stream::ConfigSync;
use spatiand_sync::Sync;

static SETTINGS: Mutex<Option<Sync>> = Mutex::new(None);

fn reply(messages: Vec<ConfigSync>) -> *mut c_char {
    let text = serde_json::to_string(&messages).unwrap_or_else(|_| "[]".into());
    CString::new(text).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

/// The owner's devices, as a JSON array, to be freed with `sp_settings_free`.
#[no_mangle]
pub extern "C" fn sp_roster_json() -> *mut c_char {
    let entries = spatiand_sync::roster::Roster::open_default().all();
    let text = serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into());
    CString::new(text).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

/// A device or host of the owner's, as this Mac knows it. `own`: it is this Mac, saying its name.
///
/// # Safety
/// C strings.
#[no_mangle]
pub unsafe extern "C" fn sp_roster_note(fp: *const c_char, name: *const c_char, address: *const c_char, own: i32) {
    let text = |p: *const c_char| if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() };
    spatiand_sync::roster::Roster::open_default().note(&text(fp), &text(name), &text(address), own != 0);
}

/// Take a device off every machine's list. Whether it was on it.
///
/// # Safety
/// A C string.
#[no_mangle]
pub unsafe extern "C" fn sp_roster_remove(fp: *const c_char) -> i32 {
    if fp.is_null() {
        return 0;
    }
    let which = CStr::from_ptr(fp).to_string_lossy();
    match spatiand_sync::roster::Roster::open_default().remove(&which) {
        Ok(_) => 1,
        Err(e) => {
            log::warn!("{e}");
            0
        }
    }
}

/// `--devices` and the like, from the app's arguments as a JSON array. Whether one was asked for.
///
/// # Safety
/// A C string.
#[no_mangle]
pub unsafe extern "C" fn sp_roster_command_line(json: *const c_char) -> i32 {
    if json.is_null() {
        return 0;
    }
    let args: Vec<String> = serde_json::from_str(&CStr::from_ptr(json).to_string_lossy()).unwrap_or_default();
    i32::from(spatiand_sync::roster::command_line(&args))
}

/// A `ConfigSync` as the app passes it on: a file's bytes as base64, as the host's JSON has all
/// bytes for the app (`host.rs`, `to_app_json`), where serde wants a list of numbers.
fn said(text: &str) -> Option<ConfigSync> {
    use base64::Engine;
    let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
    if let Some(bytes) = value.pointer_mut("/File/bytes") {
        if let Some(encoded) = bytes.as_str() {
            *bytes = serde_json::to_value(base64::engine::general_purpose::STANDARD.decode(encoded).ok()?).ok()?;
        }
    }
    serde_json::from_value(value).ok()
}

/// A session said a `ConfigSync` (its JSON, as serde writes it). Returns the JSON array of
/// `ConfigSync`s to say back, to be freed with `sp_settings_free`.
///
/// # Safety
/// `json` must be a valid C string.
#[no_mangle]
pub unsafe extern "C" fn sp_settings_said(json: *const c_char) -> *mut c_char {
    if json.is_null() {
        return reply(Vec::new());
    }
    let text = CStr::from_ptr(json).to_string_lossy();
    let Some(message) = said(&text) else {
        log::warn!("settings: a message from the session could not be read");
        return reply(Vec::new());
    };
    let mut settings = SETTINGS.lock().unwrap();
    let replies = settings.get_or_insert_with(Sync::open_default).handle(message);
    reply(replies)
}

/// The manifest, said now whatever was said before: what a link to a host opens with. An array
/// of one `ConfigSync`, to be freed with `sp_settings_free`.
#[no_mangle]
pub extern "C" fn sp_settings_hello() -> *mut c_char {
    let mut settings = SETTINGS.lock().unwrap();
    let hello = settings.get_or_insert_with(Sync::open_default).hello();
    reply(vec![hello])
}

/// What changed here since last said, once a session has started keeping the settings: an
/// array of none or one `ConfigSync`, to be freed with `sp_settings_free`.
#[no_mangle]
pub extern "C" fn sp_settings_changed() -> *mut c_char {
    let mut settings = SETTINGS.lock().unwrap();
    let changed = settings.as_mut().and_then(|s| s.changed());
    reply(changed.into_iter().collect())
}

/// # Safety
/// `text` must be what one of the calls above returned, freed once.
#[no_mangle]
pub unsafe extern "C" fn sp_settings_free(text: *mut c_char) {
    if !text.is_null() {
        drop(CString::from_raw(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_said_by_a_session_arrives_with_its_bytes() {
        // What the app passes on: bytes as base64. Dropped whole, once, while deletions passed.
        let text = r#"{"File":{"path":"a/b.json","modified_ms":5,"bytes":"aGVsbG8="}}"#;
        match said(text) {
            Some(ConfigSync::File { path, modified_ms, bytes }) => {
                assert_eq!((path.as_str(), modified_ms), ("a/b.json", 5));
                assert_eq!(bytes.as_deref(), Some(&b"hello"[..]));
            }
            other => panic!("read as {other:?}"),
        }
        let deleted = r#"{"File":{"path":"a/b.json","modified_ms":5,"bytes":null}}"#;
        assert!(matches!(said(deleted), Some(ConfigSync::File { bytes: None, .. })));
        assert!(matches!(said(r#"{"Want":{"paths":["x"]}}"#), Some(ConfigSync::Want { .. })));
    }
}
