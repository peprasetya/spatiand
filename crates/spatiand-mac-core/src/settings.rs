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
    let Ok(message) = serde_json::from_str::<ConfigSync>(&text) else { return reply(Vec::new()) };
    let mut settings = SETTINGS.lock().unwrap();
    let replies = settings.get_or_insert_with(Sync::open_default).handle(message);
    reply(replies)
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
