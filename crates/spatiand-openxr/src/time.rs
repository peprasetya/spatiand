//! `XrTime`, which here is CLOCK_MONOTONIC nanoseconds: the clock the pose ring's timestamps and a
//! Wayland frame callback's time already use, so an application's idea of "now" and the
//! compositor's are directly comparable.

pub fn now_ns() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a libc call filling a struct we own.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64
}

/// The `timespec` that names the same moment, for `XR_KHR_convert_timespec_time`.
pub fn to_timespec(ns: i64) -> libc::timespec {
    libc::timespec {
        tv_sec: (ns / 1_000_000_000) as libc::time_t,
        tv_nsec: (ns % 1_000_000_000) as _,
    }
}

pub fn from_timespec(ts: &libc::timespec) -> i64 {
    ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timespec_round_trips() {
        let ns = 123_456_789_012;
        assert_eq!(from_timespec(&to_timespec(ns)), ns);
    }

    #[test]
    fn time_does_not_go_backwards() {
        let a = now_ns();
        assert!(now_ns() >= a);
    }
}
