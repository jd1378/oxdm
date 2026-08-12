//! Build facts the About dialog reports: the pinned `odl` engine
//! version (Cargo.lock is the only place it is resolved), the source
//! commit, and the toolchain that produced the binary. Every one of
//! them degrades to "unknown" instead of failing the build, so a source
//! tarball without a `.git` or a `git`/`rustc` binary still compiles.

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
    emit("OXDM_ODL_VERSION", locked_version("odl"));
    emit("OXDM_GIT_COMMIT", git_short_commit());
    emit("OXDM_RUSTC", rustc_version());
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
