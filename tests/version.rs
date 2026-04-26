//! Tests for version string output.

use phase4::Args;

#[test]
fn short_version_is_semver() {
    let cmd = <Args as clap::CommandFactory>::command();
    let version = cmd.get_version().expect("version should be set");
    assert!(
        version.starts_with(env!("CARGO_PKG_VERSION")),
        "short version should start with crate version, got: {version}"
    );
}

#[test]
fn long_version_contains_build_metadata() {
    let cmd = <Args as clap::CommandFactory>::command();
    let long = cmd.get_long_version().expect("long_version should be set");

    let metadata = long
        .strip_prefix(env!("CARGO_PKG_VERSION"))
        .and_then(|value| value.strip_prefix(" ("))
        .and_then(|value| value.strip_suffix(')'))
        .expect("long_version should be '<semver> (<build metadata>)'");

    let mut parts = metadata.split(", ");
    let git_metadata = parts.next().expect("git metadata should be present");
    let build_metadata = parts.next().expect("build timestamp should be present");
    let arch_metadata = parts.next().expect("target arch should be present");
    let bins_metadata = parts.next().expect("bin build metadata should be present");

    assert!(
        long.starts_with(env!("CARGO_PKG_VERSION")),
        "long_version should start with crate version, got: {long}"
    );
    assert!(
        !git_metadata.is_empty(),
        "git metadata should not be empty, got: {long}"
    );
    assert!(
        parts.next().is_none(),
        "long_version should contain exactly four metadata parts, got: {long}"
    );

    let build_timestamp = build_metadata
        .strip_prefix("built ")
        .expect("build metadata should start with 'built '");
    assert!(
        build_timestamp.contains('T') && build_timestamp.ends_with('Z'),
        "build timestamp should be RFC3339 UTC, got: {long}"
    );
    assert_eq!(
        arch_metadata,
        format!("arch {}", std::env::consts::ARCH),
        "long_version should include target arch, got: {long}"
    );
    let bin_count = bins_metadata
        .strip_suffix("-bin build")
        .expect("bins metadata should end with '-bin build'");
    assert!(
        bin_count.parse::<u32>().is_ok(),
        "bin count should be a positive integer, got: {long}"
    );
}
