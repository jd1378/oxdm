//! Keeping the browser bridge registered, and noticing when it is not.
//!
//! A native-messaging manifest names the binary a browser will launch.
//! It goes stale for ordinary reasons — oxdm was moved or reinstalled
//! somewhere else — and each time the
//! extension silently stops being able to reach the app. It can also
//! go wrong for a hostile reason: anything that can write to the
//! user's home can point the manifest at a binary of its own, and the
//! browser will launch that instead. The manifest directories live
//! under `~/.config/<browser>/`, and on Windows the registry values
//! naming the manifest under HKCU; both are writable by the user
//! account, and the OS gives us nothing automatic here.
//!
//! So on startup: read every manifest we would have written, and if
//! one is missing or names something that is not our host, write ours
//! back. A wrong path is also reported to the user, because "oxdm
//! repaired this" and "something replaced this" look identical from
//! here, and only the user knows which they expected.
//!
//! Repairing does not win a race against an attacker who keeps
//! rewriting the file — nothing available here would. What it does is
//! make the common case self-healing and leave the uncommon one
//! visible.

use std::path::{Path, PathBuf};

use crate::data::native_host;

/// Run the check in the background. Logs on its own. Never panics.
pub fn spawn() {
    // A secondary instance shares the machine's browsers but not its
    // identity: `OXDM_INSTANCE_SUFFIX` exists so a sandboxed or
    // development copy can run beside the real one, and pointing every
    // browser at *that* copy is the one thing it must not do. Only the
    // primary daemon registers itself.
    if std::env::var_os("OXDM_INSTANCE_SUFFIX").is_some_and(|v| !v.is_empty()) {
        tracing::debug!("secondary instance: leaving the browser manifests alone");
        return;
    }
    tokio::spawn(async move {
        match tokio::task::spawn_blocking(run).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::debug!(error = %e, "manifest check skipped"),
            Err(e) => tracing::warn!(error = %e, "manifest check task failed"),
        }
    });
}

/// What a manifest we found is doing.
#[derive(Debug, PartialEq, Eq)]
enum State {
    /// Points at our host. Nothing to do.
    Ours,
    /// Not there yet: the browser has never been told about oxdm.
    Missing,
    /// Points somewhere else. Either oxdm moved, or something moved it.
    Wrong(String),
}

fn run() -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    // Without the host program there is nothing to point a browser at,
    // and writing a manifest naming a file that does not exist would
    // be worse than writing none. Which of those two situations this
    // is depends on whether anything was ever registered.
    let expected = match native_host::host_binary() {
        Ok(p) => p,
        Err(e) => {
            report_missing_host(&home, &e);
            return Ok(());
        }
    };
    let (missing, wrong) = classify(native_host::registered_manifests(&home), &expected);
    if missing == 0 && wrong.is_empty() {
        tracing::debug!("browser manifests are current");
        return Ok(());
    }

    // One install pass fixes both cases: it rewrites exactly the
    // manifests that differ and leaves the rest untouched.
    let report = native_host::install(&native_host::Options::default())?;
    tracing::info!(
        missing,
        wrong = wrong.len(),
        installed = report.installed(),
        failed = report.failures(),
        "browser manifests refreshed"
    );
    for (path, reason) in &wrong {
        tracing::warn!(?path, %reason, "manifest did not point at oxdm");
    }
    // Only a wrong path is worth interrupting for. A missing one is
    // the normal state of a fresh install, and the first thing this
    // function does about it is fix it.
    if !wrong.is_empty() {
        // Read back rather than trust the report: on Windows a failed
        // entry names our manifest, not the foreign one a registry
        // value still points at.
        let (_, still_wrong) = classify(native_host::registered_manifests(&home), &expected);
        notify_repaired(&wrong, still_wrong.is_empty());
    }
    Ok(())
}

/// The host program is gone. Say so only if a browser was told about
/// it, because then something that used to work has stopped: an
/// install that never registered anything is not broken, it is just an
/// install nobody has set up yet, and starting the app is not the
/// moment to nag about that.
fn report_missing_host(home: &Path, reason: &str) {
    let registered: Vec<PathBuf> = native_host::registered_manifests(home)
        .into_iter()
        .flatten()
        .filter(|p| p.is_file())
        .collect();
    if registered.is_empty() {
        tracing::debug!(%reason, "no browser host to register");
        return;
    }
    tracing::warn!(
        %reason,
        browsers = registered.len(),
        "the browser host is missing; capture will not work"
    );
    crate::platform::show_notification(
        "oxdm cannot capture browser downloads".to_owned(),
        format!(
            "{reason}\n\n{} browser manifest(s) still name it.",
            registered.len()
        ),
    );
}

/// How many browsers have no manifest yet, and which manifests name
/// something other than `expected`.
fn classify(
    manifests: impl IntoIterator<Item = Option<PathBuf>>,
    expected: &Path,
) -> (usize, Vec<(PathBuf, String)>) {
    let mut missing = 0usize;
    let mut wrong = Vec::new();
    for manifest in manifests {
        let Some(path) = manifest else {
            missing += 1;
            continue;
        };
        match inspect(&path, expected) {
            State::Ours => {}
            State::Missing => missing += 1,
            State::Wrong(reason) => wrong.push((path, reason)),
        }
    }
    (missing, wrong)
}

fn inspect(manifest: &Path, expected: &Path) -> State {
    let Ok(bytes) = std::fs::read(manifest) else {
        return State::Missing;
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return State::Wrong(format!("unreadable manifest: {e}")),
    };
    let Some(raw) = value.get("path").and_then(|v| v.as_str()) else {
        return State::Wrong("no path field".to_owned());
    };
    let claimed = PathBuf::from(raw);
    // Canonicalize both sides so symlinks / `..` / trailing slashes
    // cannot make a real mismatch look identical or vice-versa, and
    // strip the `\\?\` Windows resolution adds from both, so which
    // spelling the caller arrived with does not decide the answer.
    let expected = crate::platform::without_verbatim_prefix(expected);
    let expected = expected.as_path();
    let Ok(claimed_real) =
        std::fs::canonicalize(&claimed).map(|p| crate::platform::without_verbatim_prefix(&p))
    else {
        // A path that does not resolve cannot be launched, so this is
        // a broken registration rather than a hostile one — but it is
        // still not ours, and the user is told either way.
        return State::Wrong(format!("path '{}' does not exist", claimed.display()));
    };
    if claimed_real == expected {
        return State::Ours;
    }
    // A Flatpak browser is pointed at our wrapper inside its sandbox,
    // not at the host itself. The wrapper is ours if it execs the
    // binary we expect.
    if wrapper_execs(&claimed_real, expected) {
        return State::Ours;
    }
    State::Wrong(format!(
        "points at {} instead of {}",
        claimed_real.display(),
        expected.display()
    ))
}

/// Is this one of our Flatpak shims, pointing at the host we expect?
fn wrapper_execs(script: &Path, expected: &Path) -> bool {
    std::fs::read_to_string(script).is_ok_and(|body| {
        body.starts_with("#!/bin/sh") && body.contains(&expected.display().to_string())
    })
}

/// `restored` is false when a manifest still names something else
/// after the rewrite, so the user is not told it was fixed when it
/// was not.
fn notify_repaired(wrong: &[(PathBuf, String)], restored: bool) {
    let (summary, outcome) = if restored {
        (
            "oxdm restored its browser integration",
            "oxdm has put its own back.",
        )
    } else {
        (
            "oxdm could not restore its browser integration",
            "oxdm could not put its own back everywhere; see the log, or \
             run `oxdm --install-native-host`.",
        )
    };
    let first = wrong
        .first()
        .map(|(p, r)| format!("{}\n{r}", p.display()))
        .unwrap_or_default();
    let more = match wrong.len() {
        0 | 1 => String::new(),
        n => format!("\n…and {} more.", n - 1),
    };
    let body = format!(
        "A browser was pointed at something other than oxdm's helper. \
         {outcome}\n\n{first}{more}\n\nIf you did not move or \
         reinstall oxdm, check what changed."
    );
    crate::platform::show_notification(summary.to_owned(), body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::HOST_NAME;

    /// Built through serde, not by hand: every path on Windows is
    /// full of backslashes, and `"C:\Users\..."` spliced into JSON
    /// by hand is an invalid escape rather than a path.
    fn manifest(dir: &Path, path_value: &str) -> PathBuf {
        let file = dir.join(format!("{HOST_NAME}.json"));
        let body = serde_json::json!({
            "name": HOST_NAME,
            "path": path_value,
            "type": "stdio",
        });
        std::fs::write(&file, serde_json::to_vec(&body).unwrap()).unwrap();
        file
    }

    #[test]
    fn a_manifest_naming_our_host_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("oxdm-native-host");
        std::fs::write(&host, b"binary").unwrap();
        let host = std::fs::canonicalize(&host).unwrap();
        let file = manifest(dir.path(), &host.display().to_string());
        assert_eq!(inspect(&file, &host), State::Ours);
    }

    #[test]
    fn a_manifest_naming_something_else_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("oxdm-native-host");
        std::fs::write(&host, b"binary").unwrap();
        let host = std::fs::canonicalize(&host).unwrap();
        let impostor = dir.path().join("evil");
        std::fs::write(&impostor, b"binary").unwrap();

        let file = manifest(dir.path(), &impostor.display().to_string());
        assert!(matches!(inspect(&file, &host), State::Wrong(_)));

        // A path that no longer exists is broken, not ours.
        let file = manifest(dir.path(), "/nowhere/oxdm-native-host");
        assert!(matches!(inspect(&file, &host), State::Wrong(_)));
    }

    #[test]
    fn nothing_written_yet_is_missing_rather_than_wrong() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            inspect(&dir.path().join("absent.json"), Path::new("/opt/host")),
            State::Missing
        );
    }

    /// A browser that was never told where to look (on Windows, no
    /// registry value) needs registering, the same as a manifest file
    /// that is not there yet.
    #[test]
    fn an_unregistered_browser_counts_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("oxdm-native-host");
        std::fs::write(&host, b"binary").unwrap();
        let host = std::fs::canonicalize(&host).unwrap();
        let ours = manifest(dir.path(), &host.display().to_string());

        let (missing, wrong) = classify([None, Some(ours.clone())], &host);
        assert_eq!((missing, wrong.len()), (1, 0));
        let (missing, wrong) = classify([Some(ours)], &host);
        assert_eq!((missing, wrong.len()), (0, 0));
    }

    /// A Flatpak browser is pointed at the shim, and the shim is ours
    /// only if it runs the host we expect — otherwise every sandboxed
    /// browser would report a mismatch on every startup.
    #[test]
    #[cfg(unix)]
    fn our_flatpak_wrapper_counts_as_ours() {
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("oxdm-native-host");
        std::fs::write(&host, b"binary").unwrap();
        let host = std::fs::canonicalize(&host).unwrap();

        let wrapper = dir.path().join("wrapper");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexec '{}' --db-path '/db' \"$@\"\n",
                host.display()
            ),
        )
        .unwrap();
        let file = manifest(dir.path(), &wrapper.display().to_string());
        assert_eq!(inspect(&file, &host), State::Ours);

        // A shim that runs something else is not ours.
        std::fs::write(&wrapper, "#!/bin/sh\nexec /usr/bin/evil \"$@\"\n").unwrap();
        assert!(matches!(inspect(&file, &host), State::Wrong(_)));
    }
}
