//! Cargo build script that embeds build metadata at compile time.
//!
//! Resolves the current short commit hash, appends a `-dirty` suffix when the
//! working tree has uncommitted package changes, then includes the build
//! timestamp and target architecture. The result is exposed to the crate as
//! the `BUILD_GIT_HASH` environment variable, which [`clap`] picks up via
//! `env!()` for the `--version` long output.

use std::{env, process::Command};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

fn main() {
    emit_rerun_instructions();

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

    let dirty = git_dirty_suffix();
    let build_timestamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".into());
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "unknown".into());

    let display_bins = resolve_display_bins();

    println!(
        "cargo::rustc-env=BUILD_GIT_HASH={hash}{dirty}, built {build_timestamp}, arch {target_arch}"
    );
    println!("cargo::rustc-env=BUILD_DISPLAY_BINS={display_bins}");
}

fn emit_rerun_instructions() {
    for path in ["build.rs", "Cargo.toml", "src", "tests"] {
        println!("cargo::rerun-if-changed={path}");
    }

    for path in ["HEAD", "index", "packed-refs"] {
        if let Some(git_path) = git_path(path) {
            println!("cargo::rerun-if-changed={git_path}");
        }
    }

    if let Some(head_ref) =
        git_stdout(["symbolic-ref", "-q", "HEAD"]).and_then(|head_ref| git_path(head_ref.trim()))
    {
        println!("cargo::rerun-if-changed={head_ref}");
    }

    println!("cargo::rerun-if-env-changed=CARGO_CFG_TARGET_ARCH");
}

fn git_dirty_suffix() -> &'static str {
    // `git diff --quiet HEAD` exits with status 1 when tracked files are dirty.
    match Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .status()
    {
        Ok(status) if status.code() == Some(1) => "-dirty",
        _ => "",
    }
}

fn git_path(path: &str) -> Option<String> {
    git_stdout(["rev-parse", "--git-path", path])
}

fn git_stdout<const N: usize>(args: [&str; N]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Resolves the active `display-bins-*` Cargo feature to a plain bin count
/// string. Cargo sets `CARGO_FEATURE_<FEATURE>` for every enabled feature,
/// with hyphens normalised to underscores and names uppercased. Falls back to
/// `"64"` when no feature env var is set, which matches the default feature.
fn resolve_display_bins() -> &'static str {
    if env::var("CARGO_FEATURE_DISPLAY_BINS_32").is_ok() {
        "32"
    } else if env::var("CARGO_FEATURE_DISPLAY_BINS_128").is_ok() {
        "128"
    } else if env::var("CARGO_FEATURE_DISPLAY_BINS_256").is_ok() {
        "256"
    } else {
        "64"
    }
}
