//! The persistent status readout: time, battery, and how the session is doing.
//!
//! Head-locked in the upper right, small, and only to be looked at -- it is not pointed at -- because the two things it carries are the two things you
//! look up mid-task without wanting to leave what you are doing — what time it is, and whether
//! the battery is about to end the session. Anything world-locked would have to be hunted for,
//! which defeats the point.
//!
//! Deliberately terse. It sits over the world permanently, so every character has to earn its
//! place; the pose numbers that used to occupy the middle of the view moved here and shrank.

use std::path::Path;

/// Local wall-clock time as `HH:MM`.
///
/// Uses `localtime_r` rather than a date-time crate: this needs the hours and minutes in the
/// wearer's own timezone and nothing else, and that is one libc call the project already links.
pub fn clock() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // localtime_r is the thread-safe form; the plain one hands back a shared static that any
    // other caller can overwrite between the call and reading it.
    let ok = unsafe { !libc::localtime_r(&now, &mut tm).is_null() };
    if !ok {
        return "--:--".into();
    }
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// Charge level and whether power is going in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

impl Battery {
    /// How it should read. A separate symbol for charging rather than a colour change, because
    /// this is rendered at a couple of degrees across and hue is the first thing to go.
    pub fn label(&self) -> String {
        let mark = if self.charging { "\u{26a1}" } else { "" };
        format!("{}%{mark}", self.percent)
    }
}

/// Read the battery from sysfs.
///
/// `None` on a machine without one — a desktop, or a Deck whose battery node has moved. The
/// status bar simply omits it rather than showing a zero, which would look like an emergency.
#[cfg(not(target_os = "macos"))]
pub fn battery() -> Option<Battery> {
    let supplies = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in supplies.flatten() {
        let path = entry.path();
        // Only real batteries: the same directory carries AC adapters and, on a Deck, the
        // controller's own cells, which would otherwise be reported as the system charge.
        if read_trimmed(&path.join("type")).as_deref() != Some("Battery") {
            continue;
        }
        let Some(capacity) = read_trimmed(&path.join("capacity")) else {
            continue;
        };
        let Ok(percent) = capacity.parse::<u8>() else {
            continue;
        };
        let status = read_trimmed(&path.join("status")).unwrap_or_default();
        return Some(Battery {
            percent: percent.min(100),
            // "Full" while plugged in counts as charging: the useful question is "is it going
            // down", not "are electrons moving".
            charging: status == "Charging" || status == "Full",
        });
    }
    None
}

/// The Mac's battery, from the system's own account of its power sources.
#[cfg(target_os = "macos")]
pub fn battery() -> Option<Battery> {
    use std::ffi::{c_char, c_void, CString};
    type Ref = *const c_void;
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> Ref;
        fn IOPSCopyPowerSourcesList(blob: Ref) -> Ref;
        fn IOPSGetPowerSourceDescription(blob: Ref, source: Ref) -> Ref;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: Ref;
        fn CFRelease(object: Ref);
        fn CFArrayGetCount(array: Ref) -> isize;
        fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
        fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
        fn CFStringCreateWithCString(allocator: Ref, text: *const c_char, encoding: u32) -> Ref;
        fn CFNumberGetValue(number: Ref, kind: isize, value: *mut c_void) -> u8;
    }
    // SAFETY: Core Foundation calls on objects made here and released here; what a "Get" hands
    // back belongs to the object it came from, which outlives its use.
    unsafe {
        let blob = IOPSCopyPowerSourcesInfo();
        if blob.is_null() {
            return None;
        }
        let list = IOPSCopyPowerSourcesList(blob);
        let mut found = None;
        if !list.is_null() {
            let value = |dictionary: Ref, key: &str| -> Ref {
                let key_c = CString::new(key).unwrap();
                let key = CFStringCreateWithCString(std::ptr::null(), key_c.as_ptr(), 0x0800_0100);
                let value = CFDictionaryGetValue(dictionary, key);
                CFRelease(key);
                value
            };
            let number = |value: Ref| -> Option<i32> {
                let mut out = 0i32;
                (!value.is_null() && CFNumberGetValue(value, 3, &mut out as *mut i32 as *mut c_void) != 0).then_some(out)
            };
            for i in 0..CFArrayGetCount(list) {
                let description = IOPSGetPowerSourceDescription(blob, CFArrayGetValueAtIndex(list, i));
                if description.is_null() {
                    continue;
                }
                let (Some(now), Some(full)) =
                    (number(value(description, "Current Capacity")), number(value(description, "Max Capacity")))
                else {
                    continue;
                };
                if full <= 0 {
                    continue;
                }
                // On mains and full counts as charging, as on the Deck: the question is whether
                // it is going down.
                let charging = value(description, "Is Charging") == kCFBooleanTrue
                    || value(description, "Is Charged") == kCFBooleanTrue;
                found = Some(Battery {
                    percent: ((now * 100 + full / 2) / full).clamp(0, 100) as u8,
                    charging,
                });
                break;
            }
            CFRelease(list);
        }
        CFRelease(blob);
        found
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// The whole status line: the time and the battery.
///
/// Kept short on purpose. The bar's letters are a fixed size and its width follows from how many
/// there are, so every extra word is more of the view covered. The environment's name and the
/// number of windows were here once; the first lives in the HUD next to the control that changes
/// it, and the second was never something to look up mid-task.
pub fn line() -> String {
    let mut parts = vec![clock()];
    if let Some(b) = battery() {
        parts.push(b.label());
    }
    parts.join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_is_always_five_characters() {
        // It sits in a fixed corner; a string that changes width makes the whole bar shuffle
        // every minute.
        let c = clock();
        assert_eq!(c.len(), 5, "got {c:?}");
        assert_eq!(c.as_bytes()[2], b':');
    }

    #[test]
    fn charging_is_marked_without_relying_on_colour() {
        // Two degrees across is where hue stops being readable.
        let charging = Battery {
            percent: 80,
            charging: true,
        };
        let discharging = Battery {
            percent: 80,
            charging: false,
        };
        assert_ne!(charging.label(), discharging.label());
        assert!(charging.label().starts_with("80%"));
    }

    #[test]
    fn a_full_battery_on_mains_counts_as_charging() {
        // The question the wearer is asking is "is this going to run out", and a Deck sitting
        // at 100% on the dock reports Full rather than Charging.
        let b = Battery {
            percent: 100,
            charging: true,
        };
        assert!(b.label().contains('\u{26a1}'));
    }

    #[test]
    fn the_line_stays_short_enough_to_read_in_a_corner() {
        // Its letters are a fixed size, so length is how much of the view it covers.
        //
        // Counted in characters, because that is what gets drawn. Counting bytes made this
        // pass or fail on the battery: the lightning bolt is three bytes, so a machine at
        // 100% was two over a limit that a machine at 97% met, and neither line was any wider
        // on the glasses than the other.
        let text = line();
        assert!(
            text.chars().count() <= 14,
            "status line is {text:?} ({} characters)",
            text.chars().count()
        );
    }

    #[test]
    fn the_line_survives_a_machine_with_no_battery() {
        // Desktops, and any Deck whose sysfs layout has moved. Omitting it beats showing 0%,
        // which reads as an emergency.
        let text = line();
        assert!(text.contains(&clock()));
        assert!(!text.contains("0%") || battery().is_some());
    }
}
