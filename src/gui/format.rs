pub fn format_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

/// Like `format_bytes` but with two decimal places — used by the
/// Properties dialog where the design shows `2.34 GB`, not `2.3 GB`.
pub fn format_bytes_2(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", b, UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

/// Group an integer with comma thousands separators: `2516582400` →
/// `"2,516,582,400"`. Plain ASCII only — locale-aware grouping lives in
/// `format_locale` (TODO).
pub fn format_int_grouped(n: u64) -> String {
    let s = n.to_string();
    let len = s.len();
    let mut out = String::with_capacity(len + (len.saturating_sub(1)) / 3);
    // Insert a separator before digit `i` whenever the number of digits
    // remaining (`len - i`) is a positive multiple of 3. `len - i` never
    // underflows because `i < len`; the previous form computed `i - first`
    // before guarding `i >= first`, which panicked for inputs like
    // `52_428_800` (len 8 → first 2 → underflow at i = 1).
    for (i, b) in s.bytes().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

/// Replace `$HOME` prefix with `~`. Display-only.
pub fn abbreviate_home(p: &std::path::Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    p.display().to_string()
}

pub fn format_bytes_opt(b: Option<u64>) -> String {
    match b {
        Some(v) => format_bytes(v),
        None => "unknown".into(),
    }
}

pub fn format_speed(bps: f64) -> String {
    if bps < 1.0 {
        return "—".into();
    }
    format!("{}/s", format_bytes(bps as u64))
}

pub fn format_eta(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_int_grouped;

    #[test]
    fn groups_thousands_across_all_lengths() {
        assert_eq!(format_int_grouped(0), "0");
        assert_eq!(format_int_grouped(7), "7");
        assert_eq!(format_int_grouped(42), "42");
        assert_eq!(format_int_grouped(100), "100");
        assert_eq!(format_int_grouped(1_000), "1,000");
        assert_eq!(format_int_grouped(1_048_576), "1,048,576");
        // Regression: 8-digit value previously underflowed `i - first`.
        assert_eq!(format_int_grouped(52_428_800), "52,428,800");
        assert_eq!(format_int_grouped(104_857_600), "104,857,600");
        assert_eq!(format_int_grouped(2_516_582_400), "2,516,582,400");
    }
}

/// The label on a file's extension tile.
///
/// Takes the job's own name rather than whatever is on screen: a job
/// added before anything knew its name shows its URL there, and
/// `zeros-1g.bin?w=10354` has an "extension" of `bin?w=10354`. A link
/// is what it is until the download names it.
pub fn ext_label(filename: Option<&str>) -> String {
    let name = filename.map(str::trim).filter(|n| !n.is_empty());
    let Some(name) = name else {
        return "LINK".into();
    };
    std::path::Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "FILE".into())
}

#[cfg(test)]
mod ext_tests {
    use super::ext_label;

    #[test]
    fn a_query_string_is_not_an_extension() {
        assert_eq!(ext_label(Some("clip.mkv")), "MKV");
        assert_eq!(ext_label(Some("README")), "FILE");
        // Nothing has named it yet — the header is showing the URL.
        assert_eq!(ext_label(None), "LINK");
        assert_eq!(ext_label(Some("  ")), "LINK");
    }
}

/// When a one-off queue schedule is booked for, said the way a person
/// would say it.
///
/// "today" and "tomorrow" are what the near cases are actually called;
/// a bare date for them makes the reader work out which day that is.
/// Everything else gets the full date — including the year, because a
/// queue booked for a date without one is ambiguous the moment the
/// booking is months out.
///
/// `now` is passed in rather than read here so the boundaries are
/// testable: "tomorrow" is the calendar day after today's, not
/// twenty-four hours from this instant.
pub fn schedule_when(
    start: chrono::DateTime<chrono::Local>,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    use chrono::Datelike;
    let time = start.format("%H:%M");
    let (today, day) = (now.date_naive(), start.date_naive());
    if day == today {
        format!("today {time}")
    } else if day == today.succ_opt().unwrap_or(today) {
        format!("tomorrow {time}")
    } else {
        format!("{} {} {} {time}", day.day(), day.format("%B"), day.year())
    }
}

#[cfg(test)]
mod schedule_when_tests {
    use chrono::{Local, TimeZone};

    use super::schedule_when;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    /// The near days are named, and the boundary is the calendar day —
    /// 23:59 tonight is still today, and 00:01 is already tomorrow,
    /// however few minutes separate them.
    #[test]
    fn today_and_tomorrow_are_named() {
        let now = at(2026, 8, 14, 22, 30);
        assert_eq!(schedule_when(at(2026, 8, 14, 23, 59), now), "today 23:59");
        assert_eq!(schedule_when(at(2026, 8, 15, 0, 1), now), "tomorrow 00:01");
        // Earlier today: still today. The queue may be running already.
        assert_eq!(schedule_when(at(2026, 8, 14, 9, 0), now), "today 09:00");
    }

    /// Anything further out is a date, with the year on it.
    #[test]
    fn distant_days_get_the_full_date() {
        let now = at(2026, 8, 14, 10, 0);
        assert_eq!(
            schedule_when(at(2026, 8, 16, 10, 0), now),
            "16 August 2026 10:00"
        );
        assert_eq!(
            schedule_when(at(2027, 1, 2, 7, 5), now),
            "2 January 2027 07:05"
        );
        // A month boundary is not a special case, and neither is a
        // year one: the day after tomorrow is a date like any other.
        assert_eq!(
            schedule_when(at(2026, 9, 1, 18, 0), now),
            "1 September 2026 18:00"
        );
    }
}
