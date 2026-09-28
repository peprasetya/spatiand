//! How the bandwidth ceiling is shared out between the windows being sent.
//!
//! **By what each window is, now.** It used to be the ceiling over the number of windows, fixed
//! when a window's stream began: SpatiWorld, opened beside the host's settings, was held at half
//! of 25 Mbit/s long after the settings had closed -- 3840x1080 of both eyes at 60 frames a
//! second in 12.5 Mbit/s, and its text could not be read.
//!
//! A window's share follows its size in pixels, and the one the wearer is using -- the session's
//! keyboard focus -- counts [`FOCUS_WEIGHT`] times over: a VR world being played takes most of
//! the ceiling, and the chat window beside it keeps enough to be read.

use spatiand_stream::WindowId;

/// How much more the focused window's pixels count than anyone else's.
pub const FOCUS_WEIGHT: f64 = 3.0;
/// Nobody gets less than this, kbit/s: below it a window of text is a smear whatever its size.
pub const FLOOR_KBIT: u32 = 1_500;

/// Each window's share of `ceiling_kbit`, in the order given.
pub fn shares(ceiling_kbit: u32, windows: &[(WindowId, (u32, u32))], focused: Option<WindowId>) -> Vec<u32> {
    let weight = |(id, (w, h)): &(WindowId, (u32, u32))| {
        let pixels = (*w as f64 * *h as f64).max(1.0);
        if Some(*id) == focused {
            pixels * FOCUS_WEIGHT
        } else {
            pixels
        }
    };
    let total: f64 = windows.iter().map(weight).sum();
    windows
        .iter()
        .map(|window| {
            let share = ceiling_kbit as f64 * weight(window) / total.max(1.0);
            (share.round() as u32).max(FLOOR_KBIT.min(ceiling_kbit))
        })
        .collect()
}

/// Whether a stream running at `current` should be rebuilt for `wanted`: only for a change
/// worth a keyframe, since a new rate means a new encoder.
pub fn worth_changing(current: u32, wanted: u32) -> bool {
    let (a, b) = (current.max(1) as f64, wanted.max(1) as f64);
    (a - b).abs() / a > 0.25
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_alone_gets_everything() {
        assert_eq!(shares(25_000, &[(WindowId(1), (3840, 1080))], None), vec![25_000]);
        assert_eq!(shares(25_000, &[(WindowId(1), (3840, 1080))], Some(WindowId(1))), vec![25_000]);
    }

    #[test]
    fn a_focused_world_takes_most_beside_a_small_window() {
        let windows = [(WindowId(1), (3840, 1080)), (WindowId(2), (1280, 720))];
        let s = shares(25_000, &windows, Some(WindowId(1)));
        assert!(s[0] > 21_000, "{s:?}");
        assert!(s[1] >= FLOOR_KBIT, "{s:?}");
        // And the other way round, the small one being used gets more than its pixels.
        let s = shares(25_000, &windows, Some(WindowId(2)));
        assert!(s[1] as u64 > 25_000u64 * 1280 * 720 / (3840 * 1080 + 1280 * 720), "{s:?}");
    }

    #[test]
    fn equal_windows_share_equally_without_focus() {
        let windows = [(WindowId(1), (1920, 1080)), (WindowId(2), (1920, 1080))];
        assert_eq!(shares(20_000, &windows, None), vec![10_000, 10_000]);
    }

    #[test]
    fn small_changes_do_not_rebuild_an_encoder() {
        assert!(!worth_changing(20_000, 22_000));
        assert!(worth_changing(12_500, 25_000));
        assert!(worth_changing(25_000, 6_000));
    }
}
