//! Combat log timestamps, turned into ISO 8601 for the API.
//!
//! Current retail writes `9/27/2026 21:35:45.1241`: month/day/year, the time,
//! three digits of milliseconds, then the UTC offset in hours ("1" is BST,
//! "-4" is EDT). Older logs write `11/21 12:01:34.071`, with no year and no
//! offset: the year then comes from the file's name or date, and the time is
//! sent without an offset for the server to place (it says such times are
//! approximate).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    pub year: Option<i32>,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
    /// Minutes east of UTC, when the log says.
    pub offset_minutes: Option<i32>,
}

pub fn parse(s: &str) -> Option<Timestamp> {
    let (date, time) = s.trim().split_once(' ')?;
    let mut d = date.split('/');
    let month: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let year: Option<i32> = match d.next() {
        Some(y) => Some(y.parse().ok()?),
        None => None,
    };
    let (hms, frac) = time.split_once('.').unwrap_or((time, ""));
    let mut t = hms.split(':');
    let hour: u32 = t.next()?.parse().ok()?;
    let minute: u32 = t.next()?.parse().ok()?;
    let second: u32 = t.next()?.parse().ok()?;
    let digits = frac.bytes().take_while(u8::is_ascii_digit).count().min(3);
    let millis = if digits == 0 {
        0
    } else {
        let v: u32 = frac[..digits].parse().ok()?;
        v * 10u32.pow(3 - digits as u32)
    };
    let tz = &frac[digits..];
    let offset_minutes = if tz.is_empty() {
        None
    } else {
        tz.parse::<f32>().ok().map(|h| (h * 60.0).round() as i32)
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    Some(Timestamp {
        year,
        month,
        day,
        hour,
        minute,
        second,
        millis,
        offset_minutes,
    })
}

impl Timestamp {
    /// ISO 8601, with the offset when the log gave one. `year_hint` fills in a
    /// missing year (old logs).
    pub fn to_iso(&self, year_hint: Option<i32>) -> String {
        let year = self.year.or(year_hint).unwrap_or(1970);
        let mut s = format!(
            "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            self.month, self.day, self.hour, self.minute, self.second, self.millis
        );
        if let Some(off) = self.offset_minutes {
            let sign = if off < 0 { '-' } else { '+' };
            let a = off.unsigned_abs();
            s.push_str(&format!("{sign}{:02}:{:02}", a / 60, a % 60));
        }
        s
    }

    /// ISO 8601 for the API, which refuses a time without an offset. An old
    /// log's lines have none: they're this computer's local time, so its
    /// time zone's offset on that date is used (the server takes an old log's
    /// year and offset from `start_time`).
    pub fn to_api_iso(&self, year_hint: Option<i32>) -> String {
        if self.offset_minutes.is_some() {
            return self.to_iso(year_hint);
        }
        let local = self.to_iso(year_hint);
        chrono::NaiveDateTime::parse_from_str(&local, "%Y-%m-%dT%H:%M:%S%.3f")
            .ok()
            .and_then(|n| {
                use chrono::TimeZone;
                chrono::Local.from_local_datetime(&n).earliest()
            })
            .map(|t| t.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string())
            .unwrap_or_else(|| format!("{local}+00:00"))
    }

    /// Milliseconds since an arbitrary epoch, good for differences within one
    /// log (durations), not for absolute times.
    pub fn rough_millis(&self) -> i64 {
        let days = days_from_civil(self.year.unwrap_or(2000), self.month, self.day);
        let secs = days * 86_400 + (self.hour * 3600 + self.minute * 60 + self.second) as i64;
        secs * 1000 + self.millis as i64
    }
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The year in a `WoWCombatLog-MMDDYY_HHMMSS.txt` name, for old logs whose
/// lines carry none. The scope doc notes one guide says YYMMDD; the files seen
/// in the wild are MMDDYY, so that's what this reads, and only if it's a
/// plausible date.
pub fn year_from_file_name(name: &str) -> Option<i32> {
    let stem = name.strip_prefix("WoWCombatLog-")?;
    let digits: String = stem.chars().take(6).collect();
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mm: u32 = digits[0..2].parse().ok()?;
    let dd: u32 = digits[2..4].parse().ok()?;
    let yy: i32 = digits[4..6].parse().ok()?;
    ((1..=12).contains(&mm) && (1..=31).contains(&dd)).then_some(2000 + yy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_retail_with_offset() {
        let t = parse("9/27/2026 21:35:45.1241").unwrap();
        assert_eq!(t.to_iso(None), "2026-09-27T21:35:45.124+01:00");
        let t = parse("9/28/2026 20:15:01.123-4").unwrap();
        assert_eq!(t.to_iso(None), "2026-09-28T20:15:01.123-04:00");
    }

    #[test]
    fn old_logs_take_the_year_hint() {
        let t = parse("11/21 12:01:34.071").unwrap();
        assert_eq!(t.year, None);
        assert_eq!(t.to_iso(Some(2021)), "2021-11-21T12:01:34.071");
    }

    #[test]
    fn the_api_always_gets_an_offset() {
        let t = parse("9/27/2026 21:35:45.1241").unwrap();
        assert_eq!(t.to_api_iso(None), "2026-09-27T21:35:45.124+01:00");
        let old = parse("11/21 12:01:34.071").unwrap().to_api_iso(Some(2021));
        assert!(old.starts_with("2021-11-21T12:01:34.071"), "{old}");
        let off = &old[old.len() - 6..];
        assert!(off.starts_with('+') || off.starts_with('-'), "{old}");
        assert!(chrono::DateTime::parse_from_rfc3339(&old).is_ok(), "{old}");
    }

    #[test]
    fn durations_across_midnight() {
        let a = parse("9/27/2026 23:59:59.500").unwrap();
        let b = parse("9/28/2026 00:00:01.000").unwrap();
        assert_eq!(b.rough_millis() - a.rough_millis(), 1500);
    }

    #[test]
    fn year_from_names() {
        assert_eq!(
            year_from_file_name("WoWCombatLog-092726_213545.txt"),
            Some(2026)
        );
        assert_eq!(year_from_file_name("WoWCombatLog.txt"), None);
        assert_eq!(year_from_file_name("WoWCombatLog-999999_000000.txt"), None);
    }
}
