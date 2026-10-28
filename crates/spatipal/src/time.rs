//! The wearer's own clock: the time as they say it and read it, in their time zone.
//!
//! The C library's, since the standard library has no idea of a time zone: `localtime_r` turns a
//! moment into the wall clock, `mktime` the wall clock back into a moment.

fn local(epoch: u64) -> libc::tm {
    let t = epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm
}

fn moment(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> Option<u64> {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day as i32;
    tm.tm_hour = hour as i32;
    tm.tm_min = minute as i32;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    (t >= 0).then_some(t as u64)
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// Now, as the wearer's clock and calendar say it: for the model, which knows no time of its own.
pub fn now_line(epoch: u64) -> String {
    let tm = local(epoch);
    let offset = tm.tm_gmtoff / 60;
    format!(
        "{} {:04}-{:02}-{:02} {:02}:{:02} (UTC{}{:02}:{:02})",
        DAYS[tm.tm_wday.clamp(0, 6) as usize],
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        if offset < 0 { '-' } else { '+' },
        offset.abs() / 60,
        offset.abs() % 60
    )
}

/// A moment as it would be said: "today at 20:00", "tomorrow at 08:30", "Friday 31-10 at 09:00".
pub fn spoken(epoch: u64) -> String {
    let now = local(crate::store::now());
    let then = local(epoch);
    let clock = format!("{:02}:{:02}", then.tm_hour, then.tm_min);
    let day = |tm: &libc::tm| (tm.tm_year, tm.tm_yday);
    if day(&then) == day(&now) {
        format!("today at {clock}")
    } else if day(&then) == day(&local(crate::store::now() + 86_400)) {
        format!("tomorrow at {clock}")
    } else {
        format!("{} {:02}-{:02} at {clock}", DAYS[then.tm_wday.clamp(0, 6) as usize], then.tm_mday, then.tm_mon + 1)
    }
}

/// A moment, as short as a list has room for: "22:33" today, "28/10" before.
pub fn short(epoch: u64) -> String {
    let now = local(crate::store::now());
    let then = local(epoch);
    if (then.tm_year, then.tm_yday) == (now.tm_year, now.tm_yday) {
        format!("{:02}:{:02}", then.tm_hour, then.tm_min)
    } else {
        format!("{}/{}", then.tm_mday, then.tm_mon + 1)
    }
}

/// A time the model wrote, as a moment: `20:00` (today, or tomorrow when that has passed),
/// `2026-10-29 08:30`, `2026-10-29T08:30(:00)`.
pub fn parse_local(at: &str, now: u64) -> Option<u64> {
    let at = at.trim().replace('T', " ").replace('.', ":");
    let (date, clock) = match at.split_once(' ') {
        Some((d, c)) => (Some(d.to_string()), c.to_string()),
        None => (None, at.clone()),
    };
    let mut parts = clock.split(':').map(|p| p.trim().parse::<u32>().ok());
    let hour = parts.next()??;
    let minute = parts.next().flatten().unwrap_or(0);
    if hour > 23 || minute > 59 {
        return None;
    }
    match date {
        Some(d) => {
            let mut ymd = d.split('-').map(|p| p.parse::<u32>().ok());
            let (y, m, day) = (ymd.next()??, ymd.next()??, ymd.next()??);
            moment(y as i32, m, day, hour, minute)
        }
        None => {
            let today = local(now);
            let t = moment(today.tm_year + 1900, today.tm_mon as u32 + 1, today.tm_mday as u32, hour, minute)?;
            Some(if t <= now { t + 86_400 } else { t })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_of_day_is_today_or_tomorrow() {
        let now = moment(2026, 10, 28, 21, 40).unwrap();
        assert_eq!(parse_local("22:15", now), moment(2026, 10, 28, 22, 15));
        assert_eq!(parse_local("08:30", now), moment(2026, 10, 29, 8, 30));
        assert_eq!(parse_local("2026-11-02 07:05", now), moment(2026, 11, 2, 7, 5));
        assert_eq!(parse_local("2026-11-02T07:05:00", now), moment(2026, 11, 2, 7, 5));
        assert_eq!(parse_local("25:00", now), None);
        assert_eq!(parse_local("soon", now), None);
    }

    #[test]
    fn now_says_the_day_and_the_zone() {
        let line = now_line(moment(2026, 10, 28, 21, 40).unwrap());
        assert!(line.starts_with("Wednesday 2026-10-28 21:40 (UTC"), "{line}");
    }
}
