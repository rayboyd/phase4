use std::process::{Command, Stdio};

#[test]
fn serving_requires_an_interactive_terminal() {
    let output = Command::new(env!("CARGO_BIN_EXE_phase4"))
        .args(["--test-hz", "440", "--ws-addr", "127.0.0.1:0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run phase4");

    assert!(
        !output.status.success(),
        "a serving invocation without a TTY must fail"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");

    assert!(
        stderr.contains("Phase4 requires an interactive terminal"),
        "stderr must explain the interactive terminal requirement, got: {stderr}"
    );
    assert!(
        !stderr.to_lowercase().contains("headless"),
        "stderr must not offer the removed headless mode, got: {stderr}"
    );
}
