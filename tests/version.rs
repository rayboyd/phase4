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
fn long_version_contains_git_hash() {
    let cmd = <Args as clap::CommandFactory>::command();
    let long = cmd.get_long_version().expect("long_version should be set");

    // Format: "0.1.0 (abc1234)" or "0.1.0 (abc1234-dirty)"
    assert!(
        long.contains('(') && long.contains(')'),
        "long_version should contain git hash in parens, got: {long}"
    );
    assert!(
        long.starts_with(env!("CARGO_PKG_VERSION")),
        "long_version should start with crate version, got: {long}"
    );
}
