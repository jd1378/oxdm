//! One download, one name.
//!
//! Two jobs writing `foo.zip` are two jobs nobody can tell apart: the
//! list shows the same row twice, "open containing folder" is a coin
//! toss, and if they share a folder the second one silently eats the
//! first. So the name is unique across the table — not per folder,
//! because the confusion is in the list as much as on disk.
//!
//! Names are compared the way a person reads them: trimmed, and
//! without regard to case. Linux would happily keep `Foo.zip` beside
//! `foo.zip`; a user looking at the list would not thank it.

use std::path::Path;

/// The form a name is compared in. Two names clash when their keys are
/// equal.
pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Characters no filename may carry. `/` and `\` because a name is
/// never a path; `:` because of Windows drive letters and alternate
/// data streams; the rest because Windows refuses them outright and a
/// name that works on one machine should work on the next.
const FORBIDDEN: &[char] = &['/', '\\', ':', '<', '>', '"', '|', '?', '*'];

/// Names Windows will not let a file have, whatever the extension.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Reduce anything claiming to be a filename to a name that can only
/// ever be a leaf inside the folder the user picked.
///
/// The name of a download is rarely the user's: it arrives in a
/// `Content-Disposition` header, or in a URL path, both of which the
/// server writes. `attachment; filename="../../../.bashrc"` used to
/// reach `save_dir.join(name)` unchanged, so a hostile server could
/// choose where on the disk oxdm wrote — and, through "delete file",
/// what it deleted.
///
/// Returns `None` when nothing usable is left, which callers treat the
/// same as a response that named no file at all.
pub fn sanitize(name: &str) -> Option<String> {
    // Take the last component under either separator, so a name is a
    // name whichever platform's convention the server used.
    let leaf = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim()
        // A trailing dot or space is dropped by Windows at create
        // time, which turns "foo.exe " into a different file than the
        // one that was checked.
        .trim_end_matches(['.', ' ']);

    let cleaned: String = leaf
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if FORBIDDEN.contains(&c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim();

    // `.` and `..` survive the filter above and are still traversal.
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        return None;
    }

    let stem = cleaned.split('.').next().unwrap_or(cleaned);
    let cleaned = if RESERVED.contains(&stem.to_ascii_lowercase().as_str()) {
        format!("_{cleaned}")
    } else {
        cleaned.to_owned()
    };

    Some(truncate_bytes(&cleaned, MAX_NAME_BYTES))
}

/// Most filesystems stop at 255 bytes per component; a server can send
/// more, and the failure lands at assembly time with the download
/// already paid for.
const MAX_NAME_BYTES: usize = 255;

/// Trim to `max` bytes on a character boundary, keeping the extension
/// so the file still opens with the right thing.
fn truncate_bytes(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    let ext = Path::new(name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .filter(|e| e.len() <= 16)
        .unwrap_or_default();
    let room = max.saturating_sub(ext.len());
    let mut cut = room.min(name.len());
    while cut > 0 && !name.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{ext}", &name[..cut])
}

/// A name near `desired` that `taken` does not claim.
///
/// `foo.zip` becomes `foo_1.zip`, then `foo_2.zip`. The extension
/// stays on the end where the system looks for it, and the suffix
/// carries no spaces or brackets, so the name stays easy to type at a
/// shell and needs no quoting.
///
/// An empty `desired` comes back empty: a job with no name yet has
/// nothing to make unique, and the daemon names it when the run
/// resolves one.
pub fn unique_name(desired: &str, taken: impl Fn(&str) -> bool) -> String {
    let desired = desired.trim();
    if desired.is_empty() || !taken(desired) {
        return desired.to_owned();
    }
    let path = Path::new(desired);
    // `file_stem`/`extension` split at the *last* dot, so `foo.tar.gz`
    // numbers as `foo.tar_1.gz`. That keeps the extension the system
    // dispatches on intact, which is the part that matters.
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| desired.to_owned());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    // Bounded so a pathological taken-set cannot spin forever; the
    // ceiling is far past any real download list, and the last
    // candidate is returned even if it clashes rather than looping.
    for n in 1..10_000 {
        let candidate = format!("{stem}_{n}{ext}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{stem}_{}{ext}", 10_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(names: &[&str]) -> impl Fn(&str) -> bool + use<> {
        let keys: Vec<String> = names.iter().map(|n| name_key(n)).collect();
        move |n: &str| keys.contains(&name_key(n))
    }

    #[test]
    fn a_free_name_is_left_alone() {
        assert_eq!(unique_name("foo.zip", taken(&["bar.zip"])), "foo.zip");
    }

    #[test]
    fn a_taken_name_is_numbered() {
        assert_eq!(unique_name("foo.zip", taken(&["foo.zip"])), "foo_1.zip");
    }

    #[test]
    fn numbering_skips_the_numbers_already_out_there() {
        let t = taken(&["foo.zip", "foo_1.zip", "foo_2.zip"]);
        assert_eq!(unique_name("foo.zip", t), "foo_3.zip");
    }

    #[test]
    fn a_name_without_an_extension_still_numbers() {
        assert_eq!(unique_name("foo", taken(&["foo"])), "foo_1");
    }

    /// The extension the system dispatches on stays last.
    #[test]
    fn only_the_final_extension_is_kept_on_the_end() {
        assert_eq!(
            unique_name("archive.tar.gz", taken(&["archive.tar.gz"])),
            "archive.tar_1.gz"
        );
    }

    #[test]
    fn case_and_padding_do_not_hide_a_clash() {
        assert_eq!(unique_name("Foo.ZIP", taken(&["  foo.zip  "])), "Foo_1.ZIP");
    }

    #[test]
    fn a_nameless_job_has_nothing_to_make_unique() {
        assert_eq!(unique_name("   ", taken(&["foo.zip"])), "");
    }

    #[test]
    fn a_name_the_user_would_recognise_survives_sanitising() {
        assert_eq!(
            sanitize("report 2026.tar.gz").as_deref(),
            Some("report 2026.tar.gz")
        );
        assert_eq!(
            sanitize("Résumé — final.pdf").as_deref(),
            Some("Résumé — final.pdf")
        );
    }

    #[test]
    fn a_name_is_never_a_path() {
        assert_eq!(sanitize("../../../.bashrc").as_deref(), Some(".bashrc"));
        assert_eq!(sanitize("/etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(
            sanitize(r"..\..\Windows\System32\evil.dll").as_deref(),
            Some("evil.dll")
        );
        assert_eq!(
            sanitize(r"C:\Users\me\thing.zip").as_deref(),
            Some("thing.zip")
        );
    }

    #[test]
    fn nothing_usable_is_nothing() {
        assert_eq!(sanitize(""), None);
        assert_eq!(sanitize("   "), None);
        assert_eq!(sanitize(".."), None);
        assert_eq!(sanitize("../"), None);
        assert_eq!(sanitize("."), None);
    }

    #[test]
    fn characters_that_break_a_filesystem_are_replaced() {
        assert_eq!(
            sanitize("a:b*c?d\"e|f<g>h.bin").as_deref(),
            Some("a_b_c_d_e_f_g_h.bin")
        );
        assert_eq!(sanitize("nul\0byte.bin").as_deref(), Some("nulbyte.bin"));
    }

    #[test]
    fn a_reserved_windows_name_is_pushed_out_of_the_way() {
        assert_eq!(sanitize("CON").as_deref(), Some("_CON"));
        assert_eq!(sanitize("com1.txt").as_deref(), Some("_com1.txt"));
        assert_eq!(sanitize("console.txt").as_deref(), Some("console.txt"));
    }

    /// Windows drops these at create time, so a name ending in one is
    /// not the name that was checked.
    #[test]
    fn trailing_dots_and_spaces_go() {
        assert_eq!(sanitize("payload.exe ").as_deref(), Some("payload.exe"));
        assert_eq!(sanitize("payload.exe.").as_deref(), Some("payload.exe"));
    }

    #[test]
    fn an_overlong_name_is_cut_but_keeps_its_extension() {
        let long = format!("{}.zip", "a".repeat(400));
        let out = sanitize(&long).unwrap();
        assert!(out.len() <= 255, "{} bytes", out.len());
        assert!(out.ends_with(".zip"));
    }

    #[test]
    fn cutting_never_splits_a_character() {
        let long = format!("{}.zip", "é".repeat(300));
        let out = sanitize(&long).unwrap();
        assert!(out.len() <= 255);
        assert!(out.ends_with(".zip"));
    }
}
