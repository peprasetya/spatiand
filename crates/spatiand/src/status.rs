//! The persistent status readout: time, battery, and how the session is doing.
//!
//! The wording and the clock are shared with the Mac (`spatiand_room::status`); what is Linux's own is reading the
//! battery from sysfs.

use std::path::Path;

pub use spatiand_room::status::{clock, line_from, Battery};

/// Read the battery from sysfs.
///
/// `None` on a machine without one — a desktop, or a Deck whose battery node has moved. The
/// status bar simply omits it rather than showing a zero, which would look like an emergency.
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

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// The whole status line: the time and the battery. The count of windows it used to carry is gone, and the argument
/// stays only so that the callers need not change; it is not read.
pub fn line(_windows: usize) -> String {
    line_from(battery())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_survives_a_machine_with_no_battery() {
        // Desktops, and any Deck whose sysfs layout has moved. Omitting it beats showing 0%,
        // which reads as an emergency.
        let text = line(2);
        assert!(text.contains(&clock()));
        assert!(!text.contains("0%") || battery().is_some());
    }

    #[test]
    fn the_line_has_no_window_count() {
        assert!(!line(4).contains("window"));
    }
}
