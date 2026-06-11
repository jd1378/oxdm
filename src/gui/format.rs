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
        if i > 0 && (len - i) % 3 == 0 {
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
