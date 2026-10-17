//! The Mac's pasteboard as a peer of the session's clipboard, the way a host is one.
//!
//! On the Deck every application is the session's client, so its clipboard is the session's.
//! On a Mac the Mac's own applications copy to the Mac's pasteboard, which the session cannot
//! see, while Spatiand's own applications and the hosts' copy to the session's. So the
//! pasteboard is joined to the session's clipboard as a host named `mac` is: it offers what is
//! copied on the Mac, is offered what is copied anywhere else, and answers and asks for the
//! bytes of what is too big to send unasked (`crate::clipboard`). The app's end is
//! `ClipboardSync`, the same that keeps the pasteboard and a host in step in 2D mode, in the same
//! words: the 2D link's JSON (`spatiand_mac_core::{client_json, host_json}`).

use std::ffi::{c_char, CStr};
use std::sync::Mutex;

use spatiand_stream::control::{Clipboard, ClientMessage, HostMessage};

use super::ffi::{self, Asked};

pub const NAME: &str = "mac";

static SAID: Mutex<Vec<Clipboard>> = Mutex::new(Vec::new());

fn take() -> Vec<Clipboard> {
    std::mem::take(&mut *SAID.lock().unwrap())
}

fn say(what: Clipboard) {
    let json = spatiand_mac_core::host_json(&HostMessage::Clipboard(what));
    ffi::tell(Asked::Clipboard, &json.to_string());
}

pub fn peer() -> crate::remote::LocalClipboard {
    crate::remote::LocalClipboard { name: NAME, take, say }
}

/// What the Mac's pasteboard has to say to the session: `{"Clipboard": {...}}`, as the 2D link
/// says it to a host.
///
/// # Safety
/// `json` is a C string.
#[no_mangle]
pub unsafe extern "C" fn sp_clipboard_said(json: *const c_char) {
    if json.is_null() {
        return;
    }
    let text = CStr::from_ptr(json).to_string_lossy();
    match spatiand_mac_core::client_json(&text) {
        Ok(ClientMessage::Clipboard(what)) => SAID.lock().unwrap().push(what),
        Ok(_) => log::warn!("pasteboard: not a clipboard message"),
        Err(e) => log::warn!("pasteboard: {e}"),
    }
}
