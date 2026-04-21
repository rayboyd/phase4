//! Cargo build script that embeds Git metadata at compile time.
//!
//! Resolves the current short commit hash and appends a `-dirty` suffix when
//! the working tree has uncommitted changes. The result is exposed to the
//! crate as the `BUILD_GIT_HASH` environment variable, which [`clap`] picks
//! up via `env!()` for the `--version` long output.

use std::process::Command;

fn main() {
    // Abbreviated commit hash, e.g. "a3f8c12".
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "unknown".into(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        );

    // `git diff --quiet` exits non-zero when there are uncommitted changes.
    let dirty = Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .status()
        .map_or("", |s| if s.success() { "" } else { "-dirty" });

    // Only re-run when HEAD moves (new commit, branch switch, rebase, etc.).
    println!("cargo::rerun-if-changed=.git/HEAD");
    println!("cargo::rustc-env=BUILD_GIT_HASH={hash}{dirty}");
}
