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

/// A moment in a list, as every app says it: "Today 22:33", "28 Oct", "28 Oct 2025"
/// (`look::when`).
pub fn short(epoch: u64) -> String {
    use spatiand_app::ui::look;
    look::when(Some(look::local(epoch as i64)), look::local(crate::store::now() as i64))
}

/// A time the model wrote, as a moment: `20:00` (today, or tomorrow when that has passed),
/// `2026-10-29 08:30`, `2026-10-29T08:30(:00)`, `8:30pm`, `08:30 PM`. `None` for anything it
/// cannot read all of -- a part that is not a number is not taken as 0 -- for a day or month
/// that does not exist, and for a date and time already past.
pub fn parse_local(at: &str, now: u64) -> Option<u64> {
    let at = at.trim().replace('T', " ").to_lowercase();
    // Twelve hours or twenty-four: "8:30pm", "8 p.m.", "08:30 PM".
    let (at, half) = match ["a.m.", "p.m.", "am", "pm"].iter().find(|s| at.ends_with(*s)) {
        Some(s) => (at[..at.len() - s.len()].trim().to_string(), Some(s.starts_with('p'))),
        None => (at, None),
    };
    let at = at.replace('.', ":");
    let (date, clock) = match at.split_once(' ') {
        Some((d, c)) => (Some(d.to_string()), c.trim().to_string()),
        None => (None, at.clone()),
    };
    let parts: Vec<u32> = clock.split(':').map(|p| p.trim().parse::<u32>().ok()).collect::<Option<_>>()?;
    let (mut hour, minute) = match parts.as_slice() {
        [h] => (*h, 0),
        [h, m] => (*h, *m),
        [h, m, s] if *s <= 59 => (*h, *m),
        _ => return None,
    };
    if let Some(pm) = half {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = hour % 12 + if pm { 12 } else { 0 };
    }
    if hour > 23 || minute > 59 {
        return None;
    }
    match date {
        Some(d) => {
            let ymd: Vec<u32> = d.split('-').map(|p| p.parse::<u32>().ok()).collect::<Option<_>>()?;
            let [y, m, day] = ymd.as_slice() else { return None };
            let (y, m, day) = (*y, *m, *day);
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
            if !(1..=12).contains(&m) || day == 0 || day > days[m as usize - 1] {
                return None;
            }
            moment(y as i32, m, day, hour, minute).filter(|t| *t > now)
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
    fn a_time_is_read_whole_or_not_at_all() {
        let now = moment(2026, 10, 28, 9, 40).unwrap();
        // Twelve hours: the minutes and the half of the day are read.
        assert_eq!(parse_local("8:30pm", now), moment(2026, 10, 28, 20, 30));
        assert_eq!(parse_local("08:30 PM", now), moment(2026, 10, 28, 20, 30));
        assert_eq!(parse_local("12:15 am", now), moment(2026, 10, 29, 0, 15));
        assert_eq!(parse_local("12 pm", now), moment(2026, 10, 28, 12, 0));
        assert_eq!(parse_local("8 p.m.", now), moment(2026, 10, 28, 20, 0));
        assert_eq!(parse_local("2026-11-02 7:05 am", now), moment(2026, 11, 2, 7, 5));
        assert_eq!(parse_local("13:00 pm", now), None);
        // A part that is not a number is not 0.
        assert_eq!(parse_local("08:xx", now), None);
        assert_eq!(parse_local("2026-11-0x 08:30", now), None);
        // No 13th month, no 32nd day, no 29 February in 2027: not rolled over into another.
        assert_eq!(parse_local("2026-13-01 08:30", now), None);
        assert_eq!(parse_local("2026-10-32 08:30", now), None);
        assert_eq!(parse_local("2027-02-29 08:30", now), None);
        assert_eq!(parse_local("2028-02-29 08:30", now), moment(2028, 2, 29, 8, 30));
        // Already past: no reminder for then.
        assert_eq!(parse_local("2026-10-28 09:00", now), None);
        assert_eq!(parse_local("2025-01-01 09:00", now), None);
    }

    #[test]
    fn now_says_the_day_and_the_zone() {
        let line = now_line(moment(2026, 10, 28, 21, 40).unwrap());
        assert!(line.starts_with("Wednesday 2026-10-28 21:40 (UTC"), "{line}");
    }
}
