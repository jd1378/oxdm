//! Build facts the About dialog reports: the pinned `odl` engine
//! version (Cargo.lock is the only place it is resolved), the source
//! commit, and the toolchain that produced the binary. Every one of
//! them degrades to "unknown" instead of failing the build, so a source
//! tarball without a `.git` or a `git`/`rustc` binary still compiles.
//!
//! Also the one build-time switch: `OXDM_NO_SELF_UPDATE`, for builds
//! whose files belong to a package manager rather than to oxdm.

use std::process::Command;

fn main() {
    // The script's output depends on these two files alone, so opt out
    // of cargo's default "rerun on any package change".
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=.git/HEAD");

    // The target triple this binary is being built for. The update
    // feed is published per target, so the app has to know which one
    // it is — and `TARGET` is only visible to build scripts.
    emit("OXDM_TARGET", std::env::var("TARGET").ok());
    emit_self_update();
    embed_windows_icon();
    emit("OXDM_ODL_VERSION", locked_version("odl"));
    emit("OXDM_GIT_COMMIT", git_short_commit());
    emit("OXDM_RUSTC", rustc_version());
}

/// Whether this build may replace its own files.
///
/// Set `OXDM_NO_SELF_UPDATE=1` for a build installed by something else
/// — a distro package, a Flatpak, a Homebrew formula. There the files
/// belong to a package manager: replacing them behind its back makes
/// its database wrong and the next system upgrade undoes the update.
/// Such a build never checks, never downloads, and never offers to
/// install; the packaging is what updates it.
///
/// Any value other than empty or `0` turns it off, so the usual
/// `OXDM_NO_SELF_UPDATE=1` and a bare `OXDM_NO_SELF_UPDATE=true` mean
/// the same thing. `rerun-if-env-changed` so flipping it rebuilds:
/// without it cargo would hand back a cached build that disagrees.
fn emit_self_update() {
    println!("cargo:rerun-if-env-changed=OXDM_NO_SELF_UPDATE");
    let off = std::env::var("OXDM_NO_SELF_UPDATE")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false);
    emit(
        "OXDM_SELF_UPDATE",
        Some(if off { "0" } else { "1" }.to_owned()),
    );
}

/// Put the app icon in the executable.
///
/// Windows reads a program's icon from a resource linked into the
/// binary — Explorer, the taskbar and Alt-Tab all end up there. Without
/// one they show the generic executable icon, which is what oxdm looked
/// like on Windows however the window itself was configured.
///
/// Never fatal. Cross-checking from Linux has no resource compiler, and
/// a missing icon is not a reason to fail a build that is otherwise
/// fine — it says so and carries on.
fn embed_windows_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=assets/oxdm.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/oxdm.ico");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon: {e}");
    }
}

fn emit(key: &str, value: Option<String>) {
    println!(
        "cargo:rustc-env={key}={}",
        value.as_deref().unwrap_or("unknown")
    );
}

/// Version `cargo` resolved for `name`, read straight out of Cargo.lock
/// — the manifest holds a requirement, not the built version.
fn locked_version(name: &str) -> Option<String> {
    let lock = std::fs::read_to_string("Cargo.lock").ok()?;
    let needle = format!("name = \"{name}\"");
    let mut lines = lock.lines().skip_while(|l| l.trim() != needle);
    lines.next()?;
    let version = lines.next()?.trim().strip_prefix("version = ")?;
    Some(version.trim_matches('"').to_owned())
}

fn git_short_commit() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn rustc_version() -> Option<String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc).arg("--version").output().ok()?;
    out.status.success().then(|| {
        // "rustc 1.88.0 (deadbeef 2025-06-23)" → "1.88.0"
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .nth(1)
            .unwrap_or("unknown")
            .to_owned()
    })
}
