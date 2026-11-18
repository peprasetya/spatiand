//! The persistent status readout: time, battery, and how the session is doing.
//!
//! Head-locked in the upper right, small, and only to be looked at -- it is not pointed at -- because the two things it carries are the two things you
//! look up mid-task without wanting to leave what you are doing — what time it is, and whether
//! the battery is about to end the session. Anything world-locked would have to be hunted for,
//! which defeats the point.
//!
//! Deliberately terse. It sits over the world permanently, so every character has to earn its
//! place; the pose numbers that used to occupy the middle of the view moved here and shrank.

#[cfg(not(any(target_os = "macos", target_os = "android")))]
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

impl Battery {
    /// What Android's `ACTION_BATTERY_CHANGED` says, read as the Deck's sysfs is: its
    /// `EXTRA_LEVEL` out of `EXTRA_SCALE`, and `EXTRA_STATUS` with `EXTRA_PLUGGED`: charging is
    /// `BATTERY_STATUS_CHARGING` (2) or `BATTERY_STATUS_FULL` (5), or plugged in at all (a phone
    /// holding its charge on the cable says "not charging", 4), as on the Deck and the Mac: the
    /// question is whether it is going down. `None` for a scale of 0 or less, which is how "no
    /// battery, or not heard yet" is sent.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn from_android(level: i32, scale: i32, status: i32, plugged: i32) -> Option<Battery> {
        if scale <= 0 || level < 0 {
            return None;
        }
        let percent = ((level as i64 * 100 + scale as i64 / 2) / scale as i64).clamp(0, 100) as u8;
        Some(Battery {
            percent,
            charging: status == 2 || status == 5 || plugged != 0,
        })
    }

    /// Whether sysfs's `status` of a battery means power is going in or held: "Not charging" is
    /// a battery on the cable that is not taking charge (a charge limit, or full).
    #[cfg_attr(any(target_os = "macos", target_os = "android"), allow(dead_code))]
    fn on_power(status: &str) -> bool {
        matches!(status, "Charging" | "Full" | "Not charging")
    }
}

/// The phone's battery, as the app's Java side last heard it (`Native.battery`): an app on
/// Android 12 and later may not read `/sys/class/power_supply`, so the Deck's way finds nothing
/// there, and the pill showed only the time. 0 is none; else 0x100, the percent in the low byte
/// and 0x200 for charging.
#[cfg(target_os = "android")]
static ANDROID_BATTERY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Hand the phone's battery to the status line; `None` takes it away.
#[cfg(target_os = "android")]
pub fn set_android_battery(battery: Option<Battery>) {
    let packed = battery.map_or(0, |b| 0x100 | b.percent as u32 | if b.charging { 0x200 } else { 0 });
    ANDROID_BATTERY.store(packed, std::sync::atomic::Ordering::Relaxed);
}

/// The phone's battery, from what the Java side handed over.
#[cfg(target_os = "android")]
pub fn battery() -> Option<Battery> {
    let packed = ANDROID_BATTERY.load(std::sync::atomic::Ordering::Relaxed);
    (packed & 0x100 != 0).then(|| Battery {
        percent: (packed & 0xff) as u8,
        charging: packed & 0x200 != 0,
    })
}

/// Read the battery from sysfs.
///
/// `None` on a machine without one — a desktop, or a Deck whose battery node has moved. The
/// status bar simply omits it rather than showing a zero, which would look like an emergency.
#[cfg(not(any(target_os = "macos", target_os = "android")))]
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
            // "Full" or "Not charging" while plugged in counts as charging: the useful question
            // is "is it going down", not "are electrons moving".
            charging: Battery::on_power(&status),
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
        fn CFEqual(a: Ref, b: Ref) -> u8;
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
                // On mains counts as charging, full or held at a limit, as on the Deck: the
                // question is whether it is going down.
                let ac_c = CString::new("AC Power").unwrap();
                let ac = CFStringCreateWithCString(std::ptr::null(), ac_c.as_ptr(), 0x0800_0100);
                let state = value(description, "Power Source State");
                let on_mains = !state.is_null() && !ac.is_null() && CFEqual(state, ac) != 0;
                if !ac.is_null() {
                    CFRelease(ac);
                }
                let charging = on_mains
                    || value(description, "Is Charging") == kCFBooleanTrue
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

#[cfg(not(any(target_os = "macos", target_os = "android")))]
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
    line_with(battery())
}

/// The line with this battery, or none: what [`line`] draws, and what the snapshot room draws
/// with a battery of its own choosing (`SPATIAND_SNAPSHOT_BATTERY`).
pub fn line_with(battery: Option<Battery>) -> String {
    let mut parts = vec![clock()];
    if let Some(b) = battery {
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
    fn android_reads_as_the_deck_does() {
        // The Beam Pro's level is out of a scale of 100, but Android lets a phone pick another.
        assert_eq!(Battery::from_android(82, 100, 3, 0), Some(Battery { percent: 82, charging: false }));
        assert_eq!(Battery::from_android(41, 50, 2, 1), Some(Battery { percent: 82, charging: true }));
        assert_eq!(Battery::from_android(1, 3, 3, 0).unwrap().percent, 33);
        assert_eq!(Battery::from_android(2, 3, 3, 0).unwrap().percent, 67);
        // Full on the charger is charging, as "Full" is on the Deck; so is "not charging" (4) on
        // the cable, a phone holding its charge, as "Not charging" is on the Deck: it is not
        // going down. Off the cable, "not charging" and "unknown" (1) are not.
        assert!(Battery::from_android(100, 100, 5, 2).unwrap().charging);
        assert!(Battery::from_android(80, 100, 4, 2).unwrap().charging);
        assert!(Battery::from_android(80, 100, 1, 1).unwrap().charging);
        assert!(!Battery::from_android(80, 100, 4, 0).unwrap().charging);
        assert!(!Battery::from_android(80, 100, 1, 0).unwrap().charging);
        assert!(Battery::on_power("Not charging") && Battery::on_power("Full") && Battery::on_power("Charging"));
        assert!(!Battery::on_power("Discharging") && !Battery::on_power("Unknown"));
        // Nothing heard, or a nonsense intent, is no battery rather than 0%.
        assert_eq!(Battery::from_android(0, 0, 0, 0), None);
        assert_eq!(Battery::from_android(-1, 100, 2, 0), None);
        assert_eq!(Battery::from_android(150, 100, 2, 0).unwrap().percent, 100);
    }

    #[test]
    fn the_line_with_a_battery_is_the_time_then_the_charge() {
        let b = Battery { percent: 82, charging: false };
        assert_eq!(line_with(Some(b)), format!("{}  82%", clock()));
        let b = Battery { percent: 100, charging: true };
        assert_eq!(line_with(Some(b)), format!("{}  100%\u{26a1}", clock()));
        assert!(line_with(Some(b)).chars().count() <= 14);
        assert_eq!(line_with(None), clock());
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
