//! The phone's notifications on the room's card, as the Mac's banners are there.
//!
//! The app hears of each notification through its notification listener (`Notices.java`, once
//! the wearer has let Spatiand read notifications), keeps out the ones that would not interrupt
//! on the phone either -- ongoing, silent, held back by Do Not Disturb, Spatiand's own -- and
//! hands the rest here (`Native.notice`, `Native.noticeGone`). What the card shows and for how
//! long is the core's ([`crate::notices`]), as on the Mac.
//!
//! Pressed, the card opens what the notification opens, in the room: the notification's own
//! intent, sent by Spatiand's shell helper onto a display of the room's, in a window as one of
//! the phone's apps opened from the launcher would be (`apps::open_notice`).

use std::sync::Mutex;
use std::time::Instant;

use jni_sys::jvalue;

use super::{apps, jni};
use crate::notices::Card;

static CARD: Mutex<Card> = Mutex::new(Card::new());

fn with_card<R>(f: impl FnOnce(&mut Card) -> R) -> R {
    f(&mut CARD.lock().unwrap())
}

/// A notification was posted, or posted again.
pub fn posted(key: &str, app: &str, title: &str, text: &str) {
    let words = crate::notices::words(app, title, text);
    if with_card(|card| card.post(key, words, crate::notices::GLANCE, Instant::now())).shown {
        log::info!("notification from {app} on the card");
    }
}

/// A notification was taken away.
pub fn removed(key: &str) {
    with_card(|card| card.remove(key));
}

/// What the card says now.
pub fn text() -> String {
    with_card(|card| card.text(Instant::now()))
}

/// The card was pressed: what its notification opens is opened in the room. False when there
/// was no notification on it.
pub fn pressed() -> bool {
    let Some(key) = with_card(|card| card.press(Instant::now())) else { return false };
    let Some(said) = java_press(&key) else {
        log::info!("the pressed notification is gone");
        return true;
    };
    #[derive(serde::Deserialize)]
    struct Pressed {
        package: String,
        label: String,
        /// Whether it has an intent of its own to open (its content intent).
        #[serde(default)]
        intent: bool,
        /// Its app's launcher activity, for one that has none.
        #[serde(default)]
        launcher: String,
    }
    match serde_json::from_str::<Pressed>(&said) {
        Ok(p) if p.intent => apps::open_notice(&p.package, &p.label),
        Ok(p) if !p.launcher.is_empty() => apps::launch(&spatiand_shell::AppEntry {
            name: p.label,
            exec: p.launcher,
            icon: None,
            categories: Vec::new(),
        }),
        Ok(p) => log::info!("a notification of {} opens nothing", p.package),
        Err(e) => log::warn!("the pressed notification: {e}"),
    }
    true
}

/// `Notices.press(key)`: the notification's app and whether it opens something, as JSON; its
/// intent kept for the helper to send. `None` when it has gone.
fn java_press(key: &str) -> Option<String> {
    jni::with_shell(|env, class| unsafe {
        let method = jni::shell_method(env, class, c"pressNotice", c"(Ljava/lang/String;)Ljava/lang/String;")?;
        let args = [jvalue { l: jni::new_string(env, key) }];
        let said = ((**env).v1_1.CallStaticObjectMethodA)(env, class, method, args.as_ptr());
        if said.is_null() {
            return None;
        }
        jni::java_string(env, said.cast())
    })
    .flatten()
}
