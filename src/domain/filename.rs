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
}
