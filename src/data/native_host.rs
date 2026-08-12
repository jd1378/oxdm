//! Registering oxdm as a native-messaging host.
//!
//! A browser will only launch a helper it has been told about: a JSON
//! manifest in a per-browser directory (or, on Windows, a registry
//! value pointing at one) naming the binary and which extensions may
//! talk to it. Until that exists, the extension's `connectNative` call
//! fails and oxdm never hears about a download.
//!
//! This used to be `tools/install-native-host.sh` and its PowerShell
//! twin, run by hand. They are still there, but as thin callers of
//! this code: a shell script and an app that disagree about where a
//! manifest goes is a bug nobody can see until a browser quietly stops
//! capturing.
//!
//! What it writes is deliberately small: manifests, and — for Flatpak
//! browsers, which cannot execute a path outside their sandbox without
//! help — a wrapper script inside the sandbox's own data directory.
//! The filesystem grant that makes the wrapper reachable is printed
//! for the user to run, never run for them.

use std::path::{Path, PathBuf};

use crate::domain::native_host::{
    CHROMIUM_EXTENSION_ID, FIREFOX_EXTENSION_ID, Family, HOST_NAME, HostEntry, HostOutcome,
    HostReport, Packaging,
};

/// Which extensions the manifests will name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ids {
    pub chromium: Vec<String>,
    pub firefox: Vec<String>,
}

impl Default for Ids {
    fn default() -> Self {
        Self {
            chromium: vec![CHROMIUM_EXTENSION_ID.to_owned()],
            firefox: vec![FIREFOX_EXTENSION_ID.to_owned()],
        }
    }
}

/// A place a manifest can go, and what it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub browser: &'static str,
    pub family: Family,
    pub packaging: Packaging,
    /// The manifest directory itself. Written only when its *parent*
    /// exists: that parent is the browser's own config directory, so
    /// its absence means the browser is not installed and creating it
    /// would litter the home directory with dirs for browsers the user
    /// does not have.
    pub dir: PathBuf,
    /// For Flatpak: the app id whose sandbox needs the wrapper.
    pub flatpak_id: Option<&'static str>,
}

/// The canonical `oxdm-native-host`, as an absolute path a browser can
/// execute.
///
/// Normally it sits beside the running binary. An AppImage is the
/// exception worth caring about: its contents live in a mount point
/// that changes on every launch, so a manifest naming the path inside
/// it works exactly once. There the host is copied out to oxdm's own
/// data directory, which does not move.
pub fn host_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "the running binary has no parent directory".to_string())?;
    let name = format!("oxdm-native-host{}", std::env::consts::EXE_SUFFIX);
    let beside = dir.join(&name);
    if crate::data::update_channel::running_as_appimage().is_some() {
        return persist_host_copy(&beside, &name);
    }
    std::fs::canonicalize(&beside)
        .map_err(|e| format!("expected oxdm-native-host at {}: {e}", beside.display()))
}

/// Copy the host out of an AppImage mount into a path that survives
/// the next launch. Re-copied whenever the sizes differ, which is the
/// cheap half of "did the bundle change" — an update replaces both
/// binaries at once, so a stale copy would be talking a protocol the
/// app has moved on from.
fn persist_host_copy(inside_bundle: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = dirs::data_dir()
        .ok_or_else(|| "no data directory".to_string())?
        .join("oxdm");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let dest = dir.join(name);
    let same_size = match (std::fs::metadata(inside_bundle), std::fs::metadata(&dest)) {
        (Ok(a), Ok(b)) => a.len() == b.len(),
        _ => false,
    };
    if !same_size {
        std::fs::copy(inside_bundle, &dest).map_err(|e| {
            format!(
                "copy {} -> {}: {e}",
                inside_bundle.display(),
                dest.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&dest)
                .map_err(|e| format!("stat {}: {e}", dest.display()))?
                .permissions();
            perm.set_mode(0o755);
            let _ = std::fs::set_permissions(&dest, perm);
        }
    }
    Ok(dest)
}

/// Install (or refresh) the manifests for every browser found.
pub fn install(ids: &Ids, dry_run: bool) -> Result<HostReport, String> {
    let binary = host_binary()?;
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    let mut report = HostReport::default();
    let mut flatpak_seen: Vec<&'static str> = Vec::new();

    for target in targets(&home) {
        // The browser's own config dir. Absent = not installed.
        if !target.dir.parent().is_some_and(|p| p.is_dir()) {
            continue;
        }
        if ids_for(&target.family, ids).is_empty() {
            continue;
        }
        let manifest = target.dir.join(format!("{HOST_NAME}.json"));
        // A Flatpak browser cannot run our binary directly; it runs a
        // wrapper inside its own sandbox, which then execs ours.
        let exec = match target.flatpak_id {
            Some(app) => match flatpak_wrapper(&home, app, &binary, dry_run) {
                Ok(path) => {
                    if !flatpak_seen.contains(&app) {
                        flatpak_seen.push(app);
                    }
                    path
                }
                Err(e) => {
                    report.entries.push(HostEntry {
                        browser: target.browser.to_owned(),
                        family: target.family,
                        packaging: target.packaging,
                        manifest: manifest.display().to_string(),
                        outcome: HostOutcome::Failed(e),
                    });
                    continue;
                }
            },
            None => binary.clone(),
        };
        let body = manifest_json(&exec, target.family, ids);
        let outcome = write_if_changed(&manifest, &body, dry_run);
        report.entries.push(HostEntry {
            browser: target.browser.to_owned(),
            family: target.family,
            packaging: target.packaging,
            manifest: manifest.display().to_string(),
            outcome,
        });
    }

    if !flatpak_seen.is_empty() {
        let grants = flatpak_grant_args(&binary);
        for app in flatpak_seen {
            report
                .flatpak_grants
                .push(format!("flatpak override --user {grants} {app}"));
        }
    }
    #[cfg(windows)]
    windows_registry::register(&mut report, &binary, ids, dry_run);

    report.no_browsers = report.entries.is_empty();
    Ok(report)
}

fn ids_for<'a>(family: &Family, ids: &'a Ids) -> &'a [String] {
    match family {
        Family::Chromium => &ids.chromium,
        Family::Firefox => &ids.firefox,
    }
}

/// The manifest a browser reads. `allowed_origins` for Chromium,
/// `allowed_extensions` for Firefox — naming the wrong one is a
/// manifest the browser accepts and then refuses to use.
fn manifest_json(exec: &Path, family: Family, ids: &Ids) -> String {
    let allow = ids_for(&family, ids);
    let list: Vec<String> = match family {
        Family::Chromium => allow
            .iter()
            .map(|id| format!("chrome-extension://{id}/"))
            .collect(),
        Family::Firefox => allow.to_vec(),
    };
    let key = match family {
        Family::Chromium => "allowed_origins",
        Family::Firefox => "allowed_extensions",
    };
    // Built through serde rather than by hand: a path with a quote or
    // a backslash in it (every Windows path) would otherwise produce a
    // manifest the browser cannot parse.
    let value = serde_json::json!({
        "name": HOST_NAME,
        "description": "oxdm download capture host",
        "path": exec.display().to_string(),
        "type": "stdio",
        key: list,
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// Write `body` to `path` only when it differs.
///
/// Reruns are meant to be boring: an unchanged manifest is left alone,
/// so nothing churns its mtime and no backup piles up. The first time
/// we overwrite something we did not write, the original is kept
/// beside it — a manifest we did not recognise may have been another
/// installation's, and it should be recoverable by hand.
fn write_if_changed(path: &Path, body: &str, dry_run: bool) -> HostOutcome {
    if std::fs::read_to_string(path).is_ok_and(|old| old.trim() == body.trim()) {
        return HostOutcome::Unchanged;
    }
    if dry_run {
        return HostOutcome::Written;
    }
    let backup = path.with_extension("json.oxdm.bak");
    if path.is_file() && !backup.exists() {
        let _ = std::fs::copy(path, &backup);
    }
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        return HostOutcome::Failed(format!("create {}: {e}", dir.display()));
    }
    // Written beside the target and renamed over it: a browser reading
    // the manifest while we write must not see half of one.
    let tmp = path.with_extension("json.oxdm.tmp");
    if let Err(e) = std::fs::write(&tmp, format!("{body}\n")) {
        return HostOutcome::Failed(format!("write {}: {e}", tmp.display()));
    }
    restrict(&tmp);
    match std::fs::rename(&tmp, path) {
        Ok(()) => HostOutcome::Written,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            HostOutcome::Failed(format!("install {}: {e}", path.display()))
        }
    }
}

/// Owner-only. The manifest holds no secret today, but it names the
/// binary a browser will execute, and a file another local user can
/// rewrite is a binary they choose.
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Plant the shim a Flatpak browser runs.
///
/// Two problems it solves. The browser cannot execute our binary
/// unless the path is granted into its sandbox, and even then the
/// host's own `dirs::config_dir()` would resolve to the sandbox's
/// `$HOME`, where no oxdm.db exists — so the wrapper passes the real
/// database path explicitly.
fn flatpak_wrapper(
    home: &Path,
    app: &str,
    binary: &Path,
    dry_run: bool,
) -> Result<PathBuf, String> {
    let dir = home.join(".var/app").join(app).join("data");
    let wrapper = dir.join("oxdm-native-host");
    let db = db_path()?;
    let body = format!(
        "#!/bin/sh\n\
         # Written by oxdm. The browser runs this from inside its\n\
         # Flatpak sandbox; the real host is bind-mounted at the path\n\
         # below, and --db-path stops it looking for oxdm.db under the\n\
         # sandbox's own $HOME.\n\
         exec {} --db-path {} \"$@\"\n",
        shell_quote(&binary.display().to_string()),
        shell_quote(&db.display().to_string()),
    );
    if dry_run {
        return Ok(wrapper);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    if std::fs::read_to_string(&wrapper).is_ok_and(|old| old == body) {
        return Ok(wrapper);
    }
    std::fs::write(&wrapper, &body).map_err(|e| format!("write {}: {e}", wrapper.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700));
    }
    Ok(wrapper)
}

/// Single-quote for `/bin/sh`: paths come from the user's home
/// directory and can hold spaces, quotes, anything.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Where the daemon keeps its database. The wrapper hands this to the
/// host so a sandboxed browser's `$HOME` cannot misdirect it.
fn db_path() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|d| d.join("oxdm").join("oxdm.db"))
        .ok_or_else(|| "no config directory".to_string())
}

/// The grants a Flatpak browser needs: the host binary, and the
/// database's *directory* — SQLite in WAL mode reads `-wal` and `-shm`
/// sidecars beside the file even when opening read-only.
fn flatpak_grant_args(binary: &Path) -> String {
    let db_dir = db_path()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    format!(
        "--filesystem={}:ro --filesystem={}:ro",
        binary.display(),
        db_dir.display()
    )
}

/// Every place a manifest can go, for the browsers oxdm knows about.
///
/// Taken over from `tools/install-native-host.sh`, which is now a
/// caller of this code rather than a second copy of this list.
pub fn targets(home: &Path) -> Vec<Target> {
    let mut out = Vec::new();
    // Unused on Windows, whose browsers are pointed at a manifest by
    // the registry rather than by where the file sits.
    #[cfg_attr(windows, allow(unused_variables, unused_mut))]
    let mut push = |browser, family, packaging, dir: PathBuf, flatpak_id| {
        out.push(Target {
            browser,
            family,
            packaging,
            dir,
            flatpak_id,
        })
    };

    #[cfg(target_os = "macos")]
    {
        let app = home.join("Library/Application Support");
        for (name, sub) in [
            ("Chrome", "Google/Chrome"),
            ("Chromium", "Chromium"),
            ("Edge", "Microsoft Edge"),
            ("Brave", "BraveSoftware/Brave-Browser"),
            ("Vivaldi", "Vivaldi"),
            ("Opera", "com.operasoftware.Opera"),
        ] {
            push(
                name,
                Family::Chromium,
                Packaging::Native,
                app.join(sub).join("NativeMessagingHosts"),
                None,
            );
        }
        for (name, sub) in [
            ("Firefox", "Mozilla"),
            ("Zen", "zen"),
            ("LibreWolf", "LibreWolf"),
        ] {
            push(
                name,
                Family::Firefox,
                Packaging::Native,
                app.join(sub).join("NativeMessagingHosts"),
                None,
            );
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let config = home.join(".config");
        for (name, sub) in [
            ("Chrome", "google-chrome"),
            ("Chromium", "chromium"),
            ("Ungoogled Chromium", "ungoogled-chromium"),
            ("Edge", "microsoft-edge"),
            ("Brave", "BraveSoftware/Brave-Browser"),
            ("Vivaldi", "vivaldi"),
            ("Opera", "opera"),
        ] {
            push(
                name,
                Family::Chromium,
                Packaging::Native,
                config.join(sub).join("NativeMessagingHosts"),
                None,
            );
        }
        for (name, sub) in [
            ("Firefox", ".mozilla"),
            ("Zen", ".zen"),
            ("LibreWolf", ".librewolf"),
        ] {
            push(
                name,
                Family::Firefox,
                Packaging::Native,
                home.join(sub).join("native-messaging-hosts"),
                None,
            );
        }

        // Flatpak: same relative layout, under each app's own tree.
        let var = home.join(".var/app");
        for (name, app, sub) in [
            ("Chrome (Flatpak)", "com.google.Chrome", "google-chrome"),
            ("Chromium (Flatpak)", "org.chromium.Chromium", "chromium"),
            (
                "Ungoogled Chromium (Flatpak)",
                "io.github.ungoogled_software.ungoogled_chromium",
                "chromium",
            ),
            ("Edge (Flatpak)", "com.microsoft.Edge", "microsoft-edge"),
            (
                "Brave (Flatpak)",
                "com.brave.Browser",
                "BraveSoftware/Brave-Browser",
            ),
            ("Vivaldi (Flatpak)", "com.vivaldi.Vivaldi", "vivaldi"),
            ("Opera (Flatpak)", "com.opera.Opera", "opera"),
        ] {
            push(
                name,
                Family::Chromium,
                Packaging::Flatpak,
                var.join(app)
                    .join("config")
                    .join(sub)
                    .join("NativeMessagingHosts"),
                Some(app),
            );
        }
        for (name, app, sub) in [
            ("Firefox (Flatpak)", "org.mozilla.firefox", ".mozilla"),
            ("Zen (Flatpak)", "app.zen_browser.zen", ".zen"),
            (
                "LibreWolf (Flatpak)",
                "io.gitlab.librewolf-community",
                ".librewolf",
            ),
        ] {
            push(
                name,
                Family::Firefox,
                Packaging::Flatpak,
                var.join(app).join(sub).join("native-messaging-hosts"),
                Some(app),
            );
        }

        // Snap. The paths are right; whether confinement lets the
        // browser execute a host outside the snap is up to the snap.
        push(
            "Chromium (Snap)",
            Family::Chromium,
            Packaging::Snap,
            home.join("snap/chromium/current/.config/chromium/NativeMessagingHosts"),
            None,
        );
        push(
            "Firefox (Snap)",
            Family::Firefox,
            Packaging::Snap,
            home.join("snap/firefox/common/.mozilla/native-messaging-hosts"),
            None,
        );
    }

    #[cfg(windows)]
    {
        // Windows browsers are pointed at a manifest by a registry
        // value, not by where the file sits; `windows_registry` writes
        // one manifest per family and registers it.
        let _ = home;
    }

    out
}

/// Windows keeps native-messaging registrations in HKCU rather than in
/// per-browser directories: one manifest anywhere readable, and a
/// registry value per browser naming it.
#[cfg(windows)]
mod windows_registry {
    use super::*;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    };

    const CHROMIUM_VENDORS: &[(&str, &str)] = &[
        ("Chrome", r"Software\Google\Chrome"),
        ("Chromium", r"Software\Chromium"),
        ("Edge", r"Software\Microsoft\Edge"),
        ("Brave", r"Software\BraveSoftware\Brave-Browser"),
        ("Vivaldi", r"Software\Vivaldi"),
    ];
    const FIREFOX_VENDORS: &[(&str, &str)] = &[
        ("Firefox", r"Software\Mozilla"),
        ("LibreWolf", r"Software\LibreWolf"),
    ];

    pub fn register(report: &mut HostReport, binary: &Path, ids: &Ids, dry_run: bool) {
        let Some(dir) = dirs::data_local_dir().map(|d| d.join("oxdm")) else {
            report.entries.push(failed("Windows", "no local data dir"));
            return;
        };
        if !dry_run && let Err(e) = std::fs::create_dir_all(&dir) {
            report
                .entries
                .push(failed("Windows", &format!("create {}: {e}", dir.display())));
            return;
        }
        for (family, vendors, variant) in [
            (Family::Chromium, CHROMIUM_VENDORS, "chromium"),
            (Family::Firefox, FIREFOX_VENDORS, "firefox"),
        ] {
            if ids_for(&family, ids).is_empty() {
                continue;
            }
            let manifest = dir.join(format!("{HOST_NAME}.{variant}.json"));
            let body = manifest_json(binary, family, ids);
            let written = write_if_changed(&manifest, &body, dry_run);
            for (browser, key) in vendors {
                let outcome = match &written {
                    HostOutcome::Failed(e) => HostOutcome::Failed(e.clone()),
                    _ if dry_run => HostOutcome::Written,
                    _ => match set_default(
                        &format!(r"{key}\NativeMessagingHosts\{HOST_NAME}"),
                        &manifest,
                    ) {
                        Ok(()) => written.clone(),
                        Err(e) => HostOutcome::Failed(e),
                    },
                };
                report.entries.push(HostEntry {
                    browser: (*browser).to_owned(),
                    family,
                    packaging: Packaging::Native,
                    manifest: manifest.display().to_string(),
                    outcome,
                });
            }
        }
    }

    fn failed(browser: &str, reason: &str) -> HostEntry {
        HostEntry {
            browser: browser.to_owned(),
            family: Family::Chromium,
            packaging: Packaging::Native,
            manifest: String::new(),
            outcome: HostOutcome::Failed(reason.to_owned()),
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Create the key if needed and set its default value. HKCU only:
    /// a per-user registration needs no elevation, and writing under
    /// HKLM would register the host for every account on the machine.
    fn set_default(subkey: &str, manifest: &Path) -> Result<(), String> {
        let path = wide(subkey);
        let value = wide(&manifest.display().to_string());
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `path` is a NUL-terminated UTF-16 string that
        // outlives the call, and `key` is written only on success.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(format!("registry key {subkey}: error {rc}"));
        }
        // SAFETY: `value` is a NUL-terminated UTF-16 buffer; the byte
        // length includes its terminator, as REG_SZ requires.
        let rc = unsafe {
            RegSetValueExW(
                key,
                std::ptr::null(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        // SAFETY: `key` came from a successful RegCreateKeyExW above.
        unsafe { RegCloseKey(key) };
        if rc != 0 {
            return Err(format!("registry value {subkey}: error {rc}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Ids {
        Ids {
            chromium: vec!["abc".into()],
            firefox: vec!["oxdm@example".into()],
        }
    }

    #[test]
    fn chromium_and_firefox_get_the_key_each_expects() {
        let chromium = manifest_json(Path::new("/opt/oxdm-native-host"), Family::Chromium, &ids());
        let v: serde_json::Value = serde_json::from_str(&chromium).unwrap();
        assert_eq!(v["allowed_origins"][0], "chrome-extension://abc/");
        assert!(v.get("allowed_extensions").is_none());
        assert_eq!(v["path"], "/opt/oxdm-native-host");
        assert_eq!(v["type"], "stdio");
        assert_eq!(v["name"], HOST_NAME);

        let firefox = manifest_json(Path::new("/opt/oxdm-native-host"), Family::Firefox, &ids());
        let v: serde_json::Value = serde_json::from_str(&firefox).unwrap();
        assert_eq!(v["allowed_extensions"][0], "oxdm@example");
        assert!(v.get("allowed_origins").is_none());
    }

    /// Rewriting an identical manifest would churn a file a browser
    /// may be reading, and pile up backups of our own writes.
    #[test]
    fn an_unchanged_manifest_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{HOST_NAME}.json"));
        let body = manifest_json(Path::new("/opt/host"), Family::Chromium, &ids());

        assert_eq!(write_if_changed(&path, &body, false), HostOutcome::Written);
        assert_eq!(
            write_if_changed(&path, &body, false),
            HostOutcome::Unchanged
        );
        assert!(
            !path.with_extension("json.oxdm.bak").exists(),
            "nothing was overwritten, so nothing needed keeping"
        );
    }

    /// Someone else's manifest is not ours to lose.
    #[test]
    fn what_was_there_first_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{HOST_NAME}.json"));
        std::fs::write(&path, "{\"path\":\"/somewhere/else\"}").unwrap();

        let body = manifest_json(Path::new("/opt/host"), Family::Chromium, &ids());
        assert_eq!(write_if_changed(&path, &body, false), HostOutcome::Written);
        let backup = std::fs::read_to_string(path.with_extension("json.oxdm.bak")).unwrap();
        assert!(backup.contains("/somewhere/else"));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("/opt/host")
        );
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{HOST_NAME}.json"));
        let body = manifest_json(Path::new("/opt/host"), Family::Chromium, &ids());
        assert_eq!(write_if_changed(&path, &body, true), HostOutcome::Written);
        assert!(!path.exists());
    }

    /// A home directory with no browsers in it must produce no writes
    /// at all — the install is not allowed to create config dirs for
    /// browsers the user does not have.
    #[test]
    fn browsers_that_are_not_installed_are_skipped() {
        let home = tempfile::tempdir().unwrap();
        for t in targets(home.path()) {
            assert!(
                !t.dir.parent().is_some_and(|p| p.is_dir()),
                "{} would be written into an empty home",
                t.dir.display()
            );
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn every_flatpak_target_names_the_app_whose_sandbox_it_needs() {
        let home = PathBuf::from("/home/someone");
        for t in targets(&home) {
            assert_eq!(
                t.packaging == Packaging::Flatpak,
                t.flatpak_id.is_some(),
                "{}",
                t.browser
            );
            if let Some(app) = t.flatpak_id {
                assert!(
                    t.dir.starts_with(home.join(".var/app").join(app)),
                    "{} does not live in {app}'s tree",
                    t.dir.display()
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_path_with_a_quote_in_it_cannot_break_the_wrapper() {
        assert_eq!(shell_quote("/home/o'brien/bin"), r"'/home/o'\''brien/bin'");
        assert_eq!(shell_quote("/plain/path"), "'/plain/path'");
    }

    /// Firefox and Chromium manifests must not collide: they carry
    /// different keys, and the same directory never holds both.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn no_two_browsers_share_a_manifest_directory() {
        let home = PathBuf::from("/home/someone");
        let mut seen: Vec<PathBuf> = Vec::new();
        for t in targets(&home) {
            assert!(
                !seen.contains(&t.dir),
                "{} is listed twice",
                t.dir.display()
            );
            seen.push(t.dir);
        }
    }
}
