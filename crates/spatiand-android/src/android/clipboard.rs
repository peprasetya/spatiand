//! The phone's clipboard as a peer of the session's, the way the Mac's pasteboard is one
//! (`spatiand-mac`'s `pasteboard`) and a host is one: what is copied in one of the phone's apps is
//! offered to the room's windows and the hosts, and what is copied there is put on the phone's.
//!
//! The rules are the core's ([`crate::remote::local_board`]); the app only reads and writes
//! Android's clipboard (`PhoneClipboard.java`): it says what the clipboard holds whenever it
//! changes (`Native.clipboardText`, `Native.clipboardImage`), and is given what to put there
//! (`PhoneClipboard.putText`, `putImage`). Android lets an app read the clipboard only while it
//! has the focus; when Spatiand has not, Spatiand's shell helper reads it, as the shell may.

use std::sync::Mutex;

use jni_sys::{jbyteArray, jvalue, JNIEnv};
use spatiand_stream::control::Clipboard;

use super::jni;
use crate::remote::local_board::{Board, Put};

/// What the phone is called as a peer of the session's clipboard.
pub const NAME: &str = "android";

static BOARD: Mutex<Option<Board>> = Mutex::new(None);
/// What the phone has said to the session since the frame loop last looked.
static SAID: Mutex<Vec<Clipboard>> = Mutex::new(Vec::new());

fn with_board<R>(f: impl FnOnce(&mut Board) -> R) -> R {
    let mut board = BOARD.lock().unwrap();
    f(board.get_or_insert_with(Board::default))
}

fn take() -> Vec<Clipboard> {
    std::mem::take(&mut *SAID.lock().unwrap())
}

fn say(what: Clipboard) {
    let (put, answer) = with_board(|board| board.said(what));
    if let Some(answer) = answer {
        SAID.lock().unwrap().push(answer);
    }
    match put {
        Some(Put::Text(text)) => java_put_text(&text),
        Some(Put::Png(png)) => java_put_image(&png),
        None => {}
    }
}

pub fn peer() -> crate::remote::LocalClipboard {
    crate::remote::LocalClipboard { name: NAME, take, say }
}

/// The phone's clipboard holds this text now (`Native.clipboardText`).
pub fn copied_text(text: String) {
    if let Some(offer) = with_board(|board| board.copied_text(text)) {
        log::info!("clipboard: the phone's copy offered to the room");
        SAID.lock().unwrap().push(offer);
    }
}

/// The phone's clipboard holds this picture now, as PNG (`Native.clipboardImage`).
pub fn copied_png(png: Vec<u8>) {
    if let Some(offer) = with_board(|board| board.copied_png(png)) {
        log::info!("clipboard: the phone's picture offered to the room");
        SAID.lock().unwrap().push(offer);
    }
}

fn java_put_text(text: &str) {
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"putClipboardText", c"(Ljava/lang/String;)V") else { return };
        let args = [jvalue { l: jni::new_string(env, text) }];
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, args.as_ptr());
    });
}

fn java_put_image(png: &[u8]) {
    jni::with_shell(|env, class| unsafe {
        let Some(method) = jni::shell_method(env, class, c"putClipboardImage", c"([B)V") else { return };
        let array = new_bytes(env, png);
        if array.is_null() {
            return;
        }
        let args = [jvalue { l: array.cast() }];
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, method, args.as_ptr());
    });
}

/// A Java `byte[]` holding these bytes.
unsafe fn new_bytes(env: *mut JNIEnv, bytes: &[u8]) -> jbyteArray {
    let Ok(len) = i32::try_from(bytes.len()) else { return std::ptr::null_mut() };
    let array = ((**env).v1_1.NewByteArray)(env, len);
    if !array.is_null() {
        ((**env).v1_1.SetByteArrayRegion)(env, array, 0, len, bytes.as_ptr().cast());
    }
    array
}

/// A Java `byte[]` as a Rust one.
pub(super) unsafe fn java_bytes(env: *mut JNIEnv, array: jbyteArray) -> Vec<u8> {
    if array.is_null() {
        return Vec::new();
    }
    let len = ((**env).v1_1.GetArrayLength)(env, array).max(0) as usize;
    let mut bytes = vec![0u8; len];
    ((**env).v1_1.GetByteArrayRegion)(env, array, 0, len as i32, bytes.as_mut_ptr().cast());
    bytes
}
