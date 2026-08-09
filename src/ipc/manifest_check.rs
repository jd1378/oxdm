//! Best-effort native-messaging manifest tamper detection.
//!
//! Threat: an attacker with arbitrary write to the user's home (but
//! not full account compromise — e.g. a malicious package post-install
//! script run as the user) can rewrite the per-user manifest's `path`
//! field to a binary they control. The browser then launches *their*
//! binary the next time the extension talks to `connectNative`. The
//! manifest dir lives at known paths under `~/.config/<browser>/` (or
//! `~/.mozilla/native-messaging-hosts/` etc), all writable by the user
//! account; the OS gives us nothing automatic here.
//!
//! Mitigation: on daemon startup, scan the known manifest dirs for
//! files named `<host>.json`. Parse each, compare its `path` field
//! against the canonical `oxdm-native-host` adjacent to our own exe.
//! Mismatch → log a `warn!` and surface a desktop notification so the
//! user knows to re-run the install script.
//!
//! Limits:
//!   - Linux + macOS only; Windows uses HKCU registry, not files.
//!     The Windows installer writes the value with the user's ACLs,
//!     which is the same trust boundary.
//!   - Best-effort. We do not block startup or rewrite the manifest.
//!     Auto-rewriting would re-introduce the same race the attacker
//!     wins (last writer in `~/.config/<browser>/`). Detection +
//!     notification is the practical defence inside this threat
//!     model.

use std::path::{Path, PathBuf};

const HOST_NAME: &str = "io.github.jd1378.oxdm.host";

/// Run the scan in the background. Logs on its own. Never panics.
pub fn spawn() {
    tokio::spawn(async move {
        if let Err(e) = run().await {
            tracing::debug!(error = %e, "manifest check skipped");
        }
    });
}

async fn run() -> Result<(), String> {
    let expected = canonical_host_binary()?;
    let dirs = candidate_manifest_dirs();
    for dir in dirs {
        let path = dir.join(format!("{HOST_NAME}.json"));
        if !path.is_file() {
            continue;
        }
        match inspect(&path, &expected).await {
            Ok(()) => tracing::debug!(?path, "manifest check ok"),
            Err(reason) => {
                tracing::warn!(?path, %reason, "manifest mismatch");
                notify_user(&path, &reason);
            }
        }
    }
    Ok(())
}

async fn inspect(manifest: &Path, expected: &Path) -> Result<(), String> {
    let bytes = tokio::fs::read(manifest)
        .await
        .map_err(|e| format!("read: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))?;
    let raw = value
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing path field".to_string())?;
    let claimed = PathBuf::from(raw);
    // Canonicalize both sides so symlinks / `..` / trailing slashes
    // can't make a real mismatch look identical or vice-versa.
    let claimed_real = match std::fs::canonicalize(&claimed) {
        Ok(p) => p,
        Err(e) => {
            return Err(format!(
                "path '{}' does not resolve: {e}",
                claimed.display()
            ));
        }
    };
    if claimed_real != *expected {
        return Err(format!(
            "manifest points at {} but oxdm-native-host is at {}",
            claimed_real.display(),
            expected.display()
        ));
    }
    Ok(())
}

fn canonical_host_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "exe has no parent dir".to_string())?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let candidate = dir.join(format!("oxdm-native-host{suffix}"));
    std::fs::canonicalize(&candidate).map_err(|e| {
        format!(
            "expected oxdm-native-host next to oxdm at {}: {e}",
            candidate.display()
        )
    })
}

fn candidate_manifest_dirs() -> Vec<PathBuf> {
    // `mut` only on the platforms whose branches push to it; Windows
    // returns the bare list.
    #[allow(unused_mut)]
    let mut out = Vec::new();
    let Some(home) = dirs::home_dir() else {
        return out;
    };
    #[cfg(target_os = "macos")]
    {
        let app_sup = home.join("Library").join("Application Support");
        for sub in [
            "Google/Chrome",
            "Chromium",
            "Microsoft Edge",
            "BraveSoftware/Brave-Browser",
            "Vivaldi",
            "com.operasoftware.Opera",
        ] {
            out.push(app_sup.join(sub).join("NativeMessagingHosts"));
        }
        for sub in ["Mozilla", "zen", "LibreWolf"] {
            out.push(app_sup.join(sub).join("NativeMessagingHosts"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let config = home.join(".config");
        for sub in [
            "google-chrome",
            "chromium",
            "microsoft-edge",
            "BraveSoftware/Brave-Browser",
            "vivaldi",
            "opera",
        ] {
            out.push(config.join(sub).join("NativeMessagingHosts"));
        }
        for sub in [
            ".mozilla/native-messaging-hosts",
            ".zen/native-messaging-hosts",
            ".librewolf/native-messaging-hosts",
        ] {
            out.push(home.join(sub));
        }
    }
    #[cfg(windows)]
    {
        let _ = home;
        // Windows uses HKCU registry. A registry-key check would mean
        // adding the winreg crate; deferred — that path is covered by
        // the same-user ACL boundary.
    }
    out
}

fn notify_user(path: &Path, reason: &str) {
    let summary = "oxdm: native-messaging manifest may be tampered";
    let body = format!(
        "{}\nRe-run tools/install-native-host.sh to restore the canonical path.\n\nDetail: {reason}",
        path.display()
    );
    crate::platform::show_notification(summary.to_owned(), body);
}
