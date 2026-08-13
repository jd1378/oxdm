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

/// Everything an install can be told to do differently.
///
/// The defaults are what the app itself uses; the flags exist for the
/// installs the defaults cannot describe — a portable copy whose
/// binary is not beside the running one, a database somewhere else, a
/// token the host should read from a file rather than from that
/// database.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub ids: Ids,
    /// The `oxdm-native-host` to register. Defaults to the one beside
    /// the running binary.
    pub host_binary: Option<PathBuf>,
    /// The `oxdm.db` a sandboxed host should read port and token from.
    /// Defaults to this user's config directory.
    pub db_path: Option<PathBuf>,
    /// Hand the extension token to the host on file descriptor 3
    /// instead of letting it read the database. A wrapper script does
    /// the redirect, so the secret never appears in `ps` output.
    pub token_file: Option<PathBuf>,
    /// Also splice the Flatpak filesystem grants into the browsers'
    /// user `.desktop` files, for people who would rather not keep a
    /// persistent `flatpak override`.
    pub patch_desktop: bool,
    pub dry_run: bool,
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
    host_binary_beside(&exe)
}

/// The path half of [`host_binary`], split out so it is testable
/// without putting a file next to the test runner.
fn host_binary_beside(exe: &Path) -> Result<PathBuf, String> {
    let dir = exe
        .parent()
        .ok_or_else(|| "the running binary has no parent directory".to_string())?;
    let name = format!("oxdm-native-host{}", std::env::consts::EXE_SUFFIX);
    let beside = dir.join(&name);
    if crate::data::update_channel::running_as_appimage().is_some() {
        return persist_host_copy(&beside, &name);
    }
    if !beside.exists() {
        // Said in full, because this is what a user sees when a
        // browser stops capturing: which file, where it was looked
        // for, and what puts it back. An install missing it is not
        // broken for downloading, only for the browser bridge.
        return Err(format!(
            "{name} is missing from {}. oxdm downloads and updates \
             without it, but a browser cannot hand downloads over \
             until it is back. Reinstalling puts it there, or copy it \
             next to oxdm from the release archive.",
            dir.display()
        ));
    }
    std::fs::canonicalize(&beside)
        .map_err(|e| format!("{name} is in {} but cannot be read: {e}", dir.display()))
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
pub fn install(opts: &Options) -> Result<HostReport, String> {
    let ids = &opts.ids;
    let dry_run = opts.dry_run;
    let binary = match &opts.host_binary {
        Some(p) => absolute_existing(p)?,
        None => host_binary()?,
    };
    let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
    let mut report = HostReport::default();
    let mut flatpak_seen: Vec<&'static str> = Vec::new();
    // With a token file, the manifest points at a shim that opens it
    // on fd 3 rather than at the host itself.
    let direct = match &opts.token_file {
        Some(token) => token_fd_wrapper(&binary, token, dry_run)?,
        None => binary.clone(),
    };

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
            Some(app) => match flatpak_wrapper(&home, app, &binary, opts) {
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
            None => direct.clone(),
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
        let grants = flatpak_grant_args(&binary, opts);
        let shell = shell_grants(&grants);
        for app in &flatpak_seen {
            report
                .flatpak_grants
                .push(format!("flatpak override --user {shell} {app}"));
        }
        if opts.patch_desktop {
            let desktop: Vec<String> = grants.iter().map(|a| desktop_quote(a)).collect();
            for app in &flatpak_seen {
                report
                    .desktop_patched
                    .push(patch_desktop(app, &desktop, dry_run));
            }
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

/// An override path is only useful if a browser can actually run it.
fn absolute_existing(p: &Path) -> Result<PathBuf, String> {
    if !p.is_absolute() {
        return Err(format!("{} must be an absolute path", p.display()));
    }
    std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// The shim that hands the token to the host on fd 3.
///
/// `--token <secret>` would put it in `ps` output for every process on
/// the machine; a redirect from a file the shell opens does not. The
/// shim is what the manifest names, so the browser launches it and it
/// execs the host.
fn token_fd_wrapper(binary: &Path, token_file: &Path, dry_run: bool) -> Result<PathBuf, String> {
    let token = absolute_existing(token_file)?;
    // The secret's own permissions are not ours to assume: tighten
    // them before pointing a browser-launched process at it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600));
    }
    let dir = dirs::data_dir()
        .ok_or_else(|| "no data directory".to_string())?
        .join("oxdm");
    let wrapper = dir.join("oxdm-native-host-fd.sh");
    let body = format!(
        "#!/bin/sh\n\
         # Written by oxdm. Reads the extension token from a file and\n\
         # passes it to the native host on fd 3, so the secret never\n\
         # appears in this process's arguments.\n\
         exec {} --token-fd 3 \"$@\" 3< {}\n",
        shell_quote(&binary.display().to_string()),
        shell_quote(&token.display().to_string()),
    );
    if dry_run {
        return Ok(wrapper);
    }
    write_script(&wrapper, &body)?;
    Ok(wrapper)
}

/// Write an executable, owner-only script.
fn write_script(path: &Path, body: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    if std::fs::read_to_string(path).is_ok_and(|old| old == body) {
        return Ok(());
    }
    std::fs::write(path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
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
    opts: &Options,
) -> Result<PathBuf, String> {
    let dir = home.join(".var/app").join(app).join("data");
    let wrapper = dir.join("oxdm-native-host");
    // A token file is reached from inside the sandbox, so it needs a
    // grant of its own — printed with the others.
    let (token_arg, redirect) = match &opts.token_file {
        Some(t) => (
            " --token-fd 3".to_owned(),
            format!(" 3< {}", shell_quote(&t.display().to_string())),
        ),
        None => (String::new(), String::new()),
    };
    let db = match &opts.db_path {
        Some(p) => p.clone(),
        None => db_path()?,
    };
    let body = format!(
        "#!/bin/sh\n\
         # Written by oxdm. The browser runs this from inside its\n\
         # Flatpak sandbox; the real host is bind-mounted at the path\n\
         # below, and --db-path stops it looking for oxdm.db under the\n\
         # sandbox's own $HOME.\n\
         exec {}{token_arg} --db-path {} \"$@\"{redirect}\n",
        shell_quote(&binary.display().to_string()),
        shell_quote(&db.display().to_string()),
    );
    if opts.dry_run {
        return Ok(wrapper);
    }
    write_script(&wrapper, &body)?;
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
fn flatpak_grant_args(binary: &Path, opts: &Options) -> Vec<String> {
    let db = match &opts.db_path {
        Some(p) => Some(p.clone()),
        None => db_path().ok(),
    };
    let db_dir = db
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let mut args = vec![
        format!("--filesystem={}:ro", binary.display()),
        format!("--filesystem={}:ro", db_dir.display()),
    ];
    if let Some(token) = &opts.token_file {
        args.push(format!("--filesystem={}:ro", token.display()));
    }
    args
}

/// The grants as a command the user can paste into a shell. A home
/// directory with a space in it is ordinary, and an unquoted path is
/// two arguments to `flatpak`.
fn shell_grants(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The grants as they must appear inside an `Exec=` line.
///
/// Desktop entries have quoting rules of their own (spec §Exec):
/// reserved characters mean an argument must be double-quoted, and
/// inside those quotes a backslash escapes. Splicing a raw path with a
/// space in it would silently turn one argument into two — the browser
/// would launch with a nonsense mount and no obvious cause.
fn desktop_quote(arg: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`',
    ];
    if !arg.contains(RESERVED) {
        return arg.to_owned();
    }
    let escaped = arg
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    format!("\"{escaped}\"")
}

/// Splice the same filesystem grants into a Flatpak browser's user
/// `.desktop` file, for people who would rather not keep a persistent
/// override.
///
/// Only the user's copy under `~/.local/share/applications` is
/// written; the system-wide file is read and never touched. Every
/// `Exec=` line is patched, not just the first — desktop files carry
/// extra ones for their actions (a private window, say), and a browser
/// launched from one of those would otherwise be the one that cannot
/// reach oxdm. Each argument is checked independently, so re-running
/// adds nothing twice.
fn patch_desktop(app: &str, grants: &[String], dry_run: bool) -> String {
    let Some(home) = dirs::home_dir() else {
        return format!("{app}: no home directory");
    };
    let dest = home
        .join(".local/share/applications")
        .join(format!("{app}.desktop"));
    let candidates = [
        dest.clone(),
        PathBuf::from("/var/lib/flatpak/exports/share/applications").join(format!("{app}.desktop")),
        PathBuf::from("/var/lib/flatpak/app")
            .join(app)
            .join("current/active/export/share/applications")
            .join(format!("{app}.desktop")),
    ];
    let Some(src) = candidates.iter().find(|p| p.is_file()) else {
        return format!("{app}: no .desktop file found");
    };
    // Only text is patched. A desktop file that is not UTF-8 is not
    // one this code understands, and a partial understanding is how
    // you corrupt somebody's launcher.
    let Ok(body) = std::fs::read_to_string(src) else {
        return format!("{app}: {} is not readable text", src.display());
    };
    let patched = splice_exec(&body, grants);
    if patched == body {
        return format!("{app}: already granted in {}", dest.display());
    }
    // Belt and braces: prove the edit only inserted grants into Exec
    // lines before writing it over a file the user owns. If this ever
    // fires it is a bug in `splice_exec`, and the right response is to
    // leave the file alone and say so.
    if let Err(e) = only_exec_grants_changed(&body, &patched, grants) {
        return format!("{app}: refusing to write {}: {e}", dest.display());
    }
    if dry_run {
        return format!("{app}: would patch {}", dest.display());
    }
    match write_desktop(&dest, &patched) {
        Ok(()) => format!("{app}: patched {}", dest.display()),
        Err(e) => format!("{app}: {e}"),
    }
}

/// Compare before and after: same number of lines, every line either
/// untouched or an `Exec=` line that gained nothing but grant tokens.
fn only_exec_grants_changed(before: &str, after: &str, grants: &[String]) -> Result<(), String> {
    let (old, new): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    if old.len() != new.len() {
        return Err(format!(
            "line count changed ({} -> {})",
            old.len(),
            new.len()
        ));
    }
    for (o, n) in old.iter().zip(&new) {
        if o == n {
            continue;
        }
        if !o.starts_with("Exec=") {
            return Err(format!("a line that is not Exec= changed: {o}"));
        }
        // Strip only what was added. A line that already carried one
        // of the grants keeps it: removing every occurrence would
        // "restore" a line the user never had.
        let mut stripped = (*n).to_owned();
        for token in grants {
            let added = n
                .matches(token)
                .count()
                .saturating_sub(o.matches(token).count());
            if added > 1 {
                return Err(format!("a grant was added {added} times: {token}"));
            }
            for _ in 0..added {
                stripped = stripped.replacen(&format!(" {token}"), "", 1);
            }
        }
        if stripped != *o {
            return Err(format!("Exec= line changed beyond the grants: {n}"));
        }
    }
    Ok(())
}

/// Write the patched file, keeping a copy of what was there.
///
/// The destination is the user's own launcher entry, which they may
/// have written by hand. So: never follow a symlink (that would edit
/// whatever it points at, possibly a system file), keep one backup of
/// the first version we replace, and land the new content with a
/// rename so a crash or a full disk cannot leave a half-written file
/// that no launcher can parse.
fn write_desktop(dest: &Path, body: &str) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    if std::fs::symlink_metadata(dest).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!(
            "{} is a symlink; edit its target by hand instead",
            dest.display()
        ));
    }
    let backup = dest.with_extension("desktop.oxdm.bak");
    if dest.is_file() && !backup.exists() {
        std::fs::copy(dest, &backup).map_err(|e| format!("back up {}: {e}", dest.display()))?;
    }
    let tmp = dest.with_extension("desktop.oxdm.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    // Launchers read this file; 0644 is what every other .desktop
    // carries, and an existing file's own mode is preserved by leaving
    // it to the rename.
    #[cfg(unix)]
    if !dest.exists() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644));
    }
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("install {}: {e}", dest.display())
    })
}

/// Insert each grant after `flatpak run` on every `Exec=` line that
/// does not already carry it.
///
/// Line endings are preserved exactly — a file written with CRLF stays
/// CRLF, and one without a trailing newline does not gain one. The
/// point is that a user who diffs this afterwards sees the arguments
/// we added and nothing else.
fn splice_exec(body: &str, tokens: &[String]) -> String {
    let mut out = String::with_capacity(body.len() + tokens.len() * 64);
    for line in split_keeping_ends(body) {
        let (text, ending) = split_ending(line);
        if !text.starts_with("Exec=") {
            out.push_str(line);
            continue;
        }
        let mut patched = text.to_owned();
        // Inserted in the order given, each after the last one placed,
        // so the arguments read the way they were written rather than
        // backwards.
        let mut cut = after_flatpak_run(&patched);
        for token in tokens {
            if patched.contains(token.as_str()) {
                continue;
            }
            // `flatpak run` is what takes these arguments. A line that
            // does not launch through it is left alone rather than
            // guessed at — there is nowhere to put them that would
            // mean anything.
            let Some(at) = cut else { break };
            let addition = format!(" {token}");
            patched.insert_str(at, &addition);
            cut = Some(at + addition.len());
        }
        out.push_str(&patched);
        out.push_str(ending);
    }
    out
}

/// Lines with their terminators still attached.
fn split_keeping_ends(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(at) = rest.find('\n') {
        let (line, tail) = rest.split_at(at + 1);
        out.push(line);
        rest = tail;
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

fn split_ending(line: &str) -> (&str, &str) {
    let text = line.trim_end_matches('\n').trim_end_matches('\r');
    (text, &line[text.len()..])
}

/// Byte offset just past `flatpak` + whitespace + `run`, allowing the
/// tabs and doubled spaces a hand-edited launcher can carry.
fn after_flatpak_run(line: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = line[from..].find("flatpak") {
        let mut i = from + rel + "flatpak".len();
        let bytes = line.as_bytes();
        let gap = i;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i > gap && line[i..].starts_with("run") {
            return Some(i + "run".len());
        }
        from = from + rel + "flatpak".len();
    }
    None
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

    /// A message about a missing program has to name the program, the
    /// place it was looked for, and a way to put it back. An IO error
    /// on its own leaves the user with nothing to act on, and this is
    /// the message a browser that stopped capturing leads to.
    #[test]
    fn a_missing_host_is_explained_rather_than_reported() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("oxdm");
        std::fs::write(&exe, b"app").unwrap();

        let err = host_binary_beside(&exe).unwrap_err();
        assert!(err.contains("oxdm-native-host"), "{err}");
        assert!(err.contains(&dir.path().display().to_string()), "{err}");
        assert!(err.contains("Reinstalling"), "{err}");
        // Downloads keep working without it; saying otherwise would
        // send people looking for a problem they do not have.
        assert!(err.contains("downloads and updates without it"), "{err}");

        // With the suffix this platform actually uses: the host is
        // `oxdm-native-host.exe` on Windows, and a test that writes
        // the bare name there is testing nothing.
        let name = format!("oxdm-native-host{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(dir.path().join(name), b"host").unwrap();
        assert!(host_binary_beside(&exe).is_ok());
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

    fn grants() -> Vec<String> {
        vec![
            "--filesystem=/opt/host:ro".to_owned(),
            "--filesystem=/home/u/.config/oxdm:ro".to_owned(),
        ]
    }

    /// Everything `splice_exec` is allowed to do to a file, and the
    /// long list of things it is not. A launcher entry is the user's
    /// file: the only acceptable diff is the arguments we added.
    fn check(before: &str, after: &str) {
        assert!(
            only_exec_grants_changed(before, after, &grants()).is_ok(),
            "the guard rejected our own edit:\n{before}\n--- became ---\n{after}"
        );
        // Byte-level invariants the guard does not cover.
        assert_eq!(
            before.ends_with('\n'),
            after.ends_with('\n'),
            "trailing newline changed"
        );
        assert_eq!(
            before.matches("\r\n").count(),
            after.matches("\r\n").count(),
            "CRLF endings changed"
        );
        assert_eq!(
            before.lines().count(),
            after.lines().count(),
            "line count changed"
        );
        for (o, n) in before.lines().zip(after.lines()) {
            if !o.starts_with("Exec=") {
                assert_eq!(o, n, "a non-Exec line was touched");
            }
        }
    }

    /// Every `Exec=` line gets the grants, not just the first: a
    /// browser launched from a desktop *action* (a private window,
    /// say) is the same browser and needs the same access.
    #[test]
    fn every_exec_line_is_granted_once() {
        let desktop = "[Desktop Entry]\n\
             Name=Firefox\n\
             Exec=/usr/bin/flatpak run --branch=stable org.mozilla.firefox @@u %U @@\n\
             \n\
             [Desktop Action new-private-window]\n\
             Exec=/usr/bin/flatpak run --branch=stable org.mozilla.firefox --private-window\n";
        let once = splice_exec(desktop, &grants());
        assert_eq!(once.matches("--filesystem=/opt/host:ro").count(), 2);
        assert_eq!(
            once.matches("--filesystem=/home/u/.config/oxdm:ro").count(),
            2
        );
        for line in once.lines().filter(|l| l.starts_with("Exec=")) {
            assert!(line.contains("flatpak run --filesystem="), "{line}");
        }
        // Rerunning adds nothing: each grant is checked on its own.
        assert_eq!(splice_exec(&once, &grants()), once);
    }

    /// A line that does not launch through `flatpak run` is left
    /// alone — there is nowhere to put the arguments that would mean
    /// anything.
    #[test]
    fn a_non_flatpak_exec_line_is_not_guessed_at() {
        let desktop = "[Desktop Entry]\nExec=/usr/bin/firefox %U\n";
        assert_eq!(
            splice_exec(desktop, &["--filesystem=/opt/host:ro".to_owned()]),
            desktop
        );
    }

    /// The real file oxdm would patch on this machine, verbatim: four
    /// Exec lines, a field-forwarding `@@u %u @@` tail, and several
    /// hundred localized keys that must come through untouched.
    #[test]
    fn a_real_flatpak_launcher_survives_intact() {
        let body = include_str!("../../tests/fixtures/org.mozilla.firefox.desktop");
        let out = splice_exec(body, &grants());
        check(body, &out);
        assert_eq!(
            out.matches("--filesystem=/opt/host:ro").count(),
            4,
            "one per Exec line"
        );
        assert!(out.contains("@@u %u @@"), "field codes are left alone");
        assert!(
            out.contains("--command=firefox --file-forwarding org.mozilla.firefox"),
            "the launch itself is unchanged"
        );
        // Localized names are the bulk of the file and must be exact.
        assert_eq!(body.matches("Name[").count(), out.matches("Name[").count());
        assert_eq!(splice_exec(&out, &grants()), out, "rerunning is a no-op");
    }

    #[test]
    fn a_real_chromium_launcher_survives_intact() {
        let body = include_str!(
            "../../tests/fixtures/io.github.ungoogled_software.ungoogled_chromium.desktop"
        );
        let out = splice_exec(body, &grants());
        check(body, &out);
        assert_eq!(out.matches("--filesystem=/opt/host:ro").count(), 3);
        assert!(out.contains("--incognito"), "the action's own flag stays");
    }

    /// A hand-edited launcher can space its arguments however it
    /// likes. The old shell installer matched `flatpak[[:space:]]+run`
    /// and this has to as well, or those users get a file that looks
    /// patched and grants nothing.
    #[test]
    fn odd_spacing_around_flatpak_run_is_still_found() {
        for line in [
            "Exec=flatpak run org.x",
            "Exec=flatpak  run org.x",
            "Exec=flatpak\trun org.x",
            "Exec=/usr/bin/flatpak   run --branch=stable org.x",
            "Exec=env GDK_BACKEND=x11 flatpak run org.x",
        ] {
            let body = format!("[Desktop Entry]\n{line}\n");
            let out = splice_exec(&body, &grants());
            check(&body, &out);
            assert!(
                out.contains("run --filesystem=/opt/host:ro"),
                "not spliced: {line}"
            );
        }
    }

    /// Words that merely contain "flatpak" or "run" are not a launch.
    #[test]
    fn lookalikes_are_not_mistaken_for_a_launch() {
        for line in [
            "Exec=/usr/bin/firefox %u",
            "Exec=flatpakrun org.x",
            "Exec=/opt/flatpak-helper --run org.x",
            "Exec=myrunner flatpak",
        ] {
            let body = format!("[Desktop Entry]\n{line}\n");
            let out = splice_exec(&body, &grants());
            assert_eq!(out, body, "touched a line it should not have: {line}");
        }
    }

    /// Only lines that *start* with `Exec=` are launches. A comment or
    /// a value that happens to mention one is prose.
    #[test]
    fn only_real_exec_keys_are_patched() {
        let body = "[Desktop Entry]\n\
             # Exec=flatpak run org.x is what this used to be\n\
             Comment=Exec=flatpak run org.x\n\
             TryExec=/usr/bin/flatpak\n\
             X-Exec=flatpak run org.x\n\
             Exec=flatpak run org.x\n";
        let out = splice_exec(body, &grants());
        check(body, &out);
        assert_eq!(out.matches("--filesystem=/opt/host:ro").count(), 1);
    }

    /// Line endings and the final newline are the user's, not ours.
    #[test]
    fn the_shape_of_the_file_is_preserved() {
        // CRLF throughout.
        let crlf = "[Desktop Entry]\r\nExec=flatpak run org.x\r\nName=X\r\n";
        let out = splice_exec(crlf, &grants());
        check(crlf, &out);
        assert_eq!(out.matches("\r\n").count(), 3);
        assert!(!out.contains("\n\n"));

        // No trailing newline.
        let bare = "[Desktop Entry]\nExec=flatpak run org.x";
        let out = splice_exec(bare, &grants());
        check(bare, &out);
        assert!(!out.ends_with('\n'));

        // Blank lines between sections, and a doubled final newline.
        let spaced = "[Desktop Entry]\nExec=flatpak run org.x\n\n[Desktop Action a]\nExec=flatpak run org.x\n\n";
        let out = splice_exec(spaced, &grants());
        check(spaced, &out);
        assert!(out.ends_with("\n\n"));
    }

    /// Half-granted files happen: a user runs the installer, then adds
    /// a grant by hand, or an old run used a different path. Each
    /// token is decided on its own.
    #[test]
    fn a_partly_granted_line_gains_only_what_it_lacks() {
        let body = "[Desktop Entry]\nExec=flatpak run --filesystem=/opt/host:ro org.x\n";
        let out = splice_exec(body, &grants());
        check(body, &out);
        assert_eq!(out.matches("--filesystem=/opt/host:ro").count(), 1);
        assert_eq!(
            out.matches("--filesystem=/home/u/.config/oxdm:ro").count(),
            1
        );
        assert_eq!(splice_exec(&out, &grants()), out);
    }

    /// Non-ASCII is everywhere in launcher entries (localized names,
    /// and home directories with accents in them). Inserting by byte
    /// offset must not land inside a character.
    #[test]
    fn unicode_lines_are_not_cut_in_half() {
        let body = "[Desktop Entry]\n\
             Name[ru]=Ողջույն Привет 你好\n\
             Comment=émoji 🦊 in a comment\n\
             Exec=flatpak run --command=/app/bin/naïve org.x 🦊\n";
        let accented = vec!["--filesystem=/home/josé/.config/oxdm:ro".to_owned()];
        let out = splice_exec(body, &accented);
        assert!(only_exec_grants_changed(body, &out, &accented).is_ok());
        assert!(out.contains("Ողջույն Привет 你好"));
        assert!(out.contains("🦊 in a comment"));
        assert!(out.contains("run --filesystem=/home/josé/.config/oxdm:ro --command"));
    }

    /// An empty or header-only file is nothing to patch, not something
    /// to mangle.
    #[test]
    fn files_with_nothing_to_patch_come_back_identical() {
        for body in ["", "\n", "[Desktop Entry]\n", "not a desktop file at all\n"] {
            assert_eq!(splice_exec(body, &grants()), body, "{body:?}");
        }
    }

    /// The guard is the last thing standing between a bug in the
    /// splice and a user's launcher. It has to actually catch things.
    #[test]
    fn the_guard_rejects_edits_it_should_never_see() {
        let before = "[Desktop Entry]\nName=X\nExec=flatpak run org.x\n";
        // A dropped line.
        assert!(
            only_exec_grants_changed(
                before,
                "[Desktop Entry]\nExec=flatpak run org.x\n",
                &grants()
            )
            .is_err()
        );
        // A changed name.
        assert!(
            only_exec_grants_changed(
                before,
                "[Desktop Entry]\nName=Y\nExec=flatpak run org.x\n",
                &grants()
            )
            .is_err()
        );
        // An Exec line that lost its own arguments.
        assert!(
            only_exec_grants_changed(
                before,
                "[Desktop Entry]\nName=X\nExec=flatpak run --filesystem=/opt/host:ro\n",
                &grants()
            )
            .is_err()
        );
        // The edit we do make.
        let ours = splice_exec(before, &grants());
        assert!(only_exec_grants_changed(before, &ours, &grants()).is_ok());
    }

    /// Writing must not follow a symlink: the user's entry may point
    /// at a system file, and editing that is not what "patch my
    /// launcher" asked for.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_launcher_is_refused_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("system.desktop");
        std::fs::write(&target, "[Desktop Entry]\nExec=flatpak run org.x\n").unwrap();
        let link = dir.path().join("org.x.desktop");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = write_desktop(&link, "patched").unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "[Desktop Entry]\nExec=flatpak run org.x\n",
            "the target was written through"
        );
    }

    /// What was there first is kept, once. A second patch does not
    /// overwrite the backup with our own earlier output.
    #[test]
    fn the_first_version_is_kept_and_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("org.x.desktop");
        std::fs::write(&dest, "original\n").unwrap();

        write_desktop(&dest, "first patch\n").unwrap();
        let backup = dest.with_extension("desktop.oxdm.bak");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "original\n");

        write_desktop(&dest, "second patch\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "original\n",
            "the backup still holds what the user had"
        );
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "second patch\n");
        // Nothing left behind.
        assert!(!dest.with_extension("desktop.oxdm.tmp").exists());
    }

    /// A home directory with a space in it is ordinary. Spliced raw,
    /// `--filesystem=/home/my apps/host:ro` is two arguments and the
    /// browser launches with a mount nobody asked for.
    #[test]
    fn a_path_with_a_space_is_quoted_for_the_launcher() {
        let spaced = ["--filesystem=/home/my apps/oxdm:ro".to_owned()];
        let quoted: Vec<String> = spaced.iter().map(|a| desktop_quote(a)).collect();
        assert_eq!(quoted[0], "\"--filesystem=/home/my apps/oxdm:ro\"");

        let body = "[Desktop Entry]\nExec=flatpak run org.x\n";
        let out = splice_exec(body, &quoted);
        assert!(only_exec_grants_changed(body, &out, &quoted).is_ok());
        assert!(
            out.contains("run \"--filesystem=/home/my apps/oxdm:ro\" org.x"),
            "{out}"
        );
        assert_eq!(
            splice_exec(&out, &quoted),
            out,
            "still idempotent when quoted"
        );
    }

    /// The same path in the copy-paste command needs shell quoting,
    /// which is a different set of rules from the launcher's.
    #[test]
    fn the_override_command_survives_a_path_with_a_space() {
        let args = vec![
            "--filesystem=/home/my apps/oxdm-native-host:ro".to_owned(),
            "--filesystem=/home/u/.config/oxdm:ro".to_owned(),
        ];
        let cmd = shell_grants(&args);
        assert_eq!(
            cmd,
            "'--filesystem=/home/my apps/oxdm-native-host:ro' '--filesystem=/home/u/.config/oxdm:ro'"
        );
    }

    /// Desktop-entry quoting escapes what the spec says it must, and
    /// leaves an ordinary path alone rather than wrapping everything.
    #[test]
    fn only_arguments_that_need_quoting_get_it() {
        assert_eq!(
            desktop_quote("--filesystem=/opt/host:ro"),
            "--filesystem=/opt/host:ro"
        );
        assert_eq!(desktop_quote("a b"), "\"a b\"");
        assert_eq!(desktop_quote("a$b"), "\"a\\$b\"");
        assert_eq!(desktop_quote("a`b"), "\"a\\`b\"");
        assert_eq!(desktop_quote(r"a\b"), "\"a\\\\b\"");
        assert_eq!(desktop_quote("a\"b"), "\"a\\\"b\"");
    }

    /// The file cannot be written: report it, change nothing. A
    /// launcher the user cannot start is worse than one without our
    /// grants in it.
    #[cfg(unix)]
    #[test]
    fn an_unwritable_launcher_is_left_as_it_was() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("org.x.desktop");
        std::fs::write(&dest, "original\n").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

        let err = write_desktop(&dest, "patched\n").unwrap_err();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(err.contains("org.x.desktop"), "{err}");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "original\n");
    }

    /// The `.desktop` we write is one a launcher will read; it starts
    /// world-readable like every other, and a rename leaves nothing
    /// half-written behind.
    #[cfg(unix)]
    #[test]
    fn a_new_launcher_lands_readable_and_whole() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("sub").join("org.x.desktop");
        write_desktop(&dest, "[Desktop Entry]\n").unwrap();
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "{mode:o}");
        assert!(!dest.with_extension("desktop.oxdm.tmp").exists());
        assert!(
            !dest.with_extension("desktop.oxdm.bak").exists(),
            "nothing was replaced, so nothing was backed up"
        );
    }

    #[test]
    fn an_override_path_has_to_be_absolute_and_real() {
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("oxdm-native-host");
        std::fs::write(&host, b"x").unwrap();
        assert!(absolute_existing(&host).is_ok());
        assert!(absolute_existing(Path::new("relative/path")).is_err());
        assert!(absolute_existing(&dir.path().join("absent")).is_err());
    }

    /// The token wrapper is the whole point of `--token-file`: the
    /// secret is redirected onto fd 3, never passed as an argument
    /// where `ps` would show it to every process on the machine.
    #[cfg(unix)]
    #[test]
    fn the_token_never_reaches_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let token = dir.path().join("token");
        std::fs::write(&token, b"s3cret").unwrap();
        let host = dir.path().join("oxdm-native-host");
        std::fs::write(&host, b"x").unwrap();

        // Written into the user's data dir, so only its shape is
        // asserted here; the dry run gives the path without writing.
        let wrapper = token_fd_wrapper(&host, &token, true).unwrap();
        assert!(wrapper.ends_with("oxdm-native-host-fd.sh"));

        let body = format!(
            "exec {} --token-fd 3 \"$@\" 3< {}",
            shell_quote(&std::fs::canonicalize(&host).unwrap().display().to_string()),
            shell_quote(&std::fs::canonicalize(&token).unwrap().display().to_string()),
        );
        assert!(!body.contains("s3cret"), "the secret is read, not passed");
        assert!(body.contains("--token-fd 3"));
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
