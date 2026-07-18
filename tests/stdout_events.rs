//! End-to-end tests for `--stdout-events json`, spawning the real binary
//! with piped stdio, the way a wrapper process would.
//!
//! `--test-hz` puts phase4 in calibration mode, so these tests need no real
//! audio hardware and are safe to run in CI.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Generous per house style (see `tests/client_server.rs`): CI machines can be
/// slow, and a hang should surface as a fast, descriptive panic rather than
/// the test-binary-wide default timeout.
const WAIT_TIMEOUT: Duration = Duration::from_secs(5);

fn spawn_phase4(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_phase4"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Not asserted on; null avoids any risk of the child blocking on a
        // full stderr pipe while we only drain stdout.
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn phase4 binary")
}

/// Polls `child` for exit, killing it and panicking if `timeout` elapses.
/// `std::process::Command` has no built-in wait-with-timeout.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("failed to poll child status") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("phase4 did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Reads `stdout` line by line on a dedicated thread, forwarding each line
/// to the returned channel as it arrives. Lets the test block on the first
/// line with a deadline via `recv_timeout`, while the thread keeps reading
/// until the pipe closes (the child exits), so a later `try_recv` after
/// `JoinHandle::join` reliably observes "no further lines" with no race.
fn spawn_line_reader(stdout: std::process::ChildStdout) -> (Receiver<String>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    (rx, handle)
}

/// Asserts the line channel has nothing left once the reader thread has
/// finished (i.e. the child's stdout has hit EOF). Must be called after
/// the child has exited and `reader_handle` has been joined.
fn assert_no_further_lines(rx: &Receiver<String>) {
    assert!(
        rx.try_recv().is_err(),
        "expected no further stdout lines after the first event"
    );
}

#[test]
fn ready_event_carries_the_real_bound_port_and_precedes_a_clean_exit() {
    let mut child = spawn_phase4(&[
        "--stdout-events",
        "json",
        "--test-hz",
        "440",
        "--ws-addr",
        "127.0.0.1:0",
        "--controller-mode",
        "headless",
    ]);
    let child_pid = child.id();
    let stdout = child.stdout.take().expect("stdout should be piped");
    let (rx, reader_handle) = spawn_line_reader(stdout);

    let first_line = rx
        .recv_timeout(WAIT_TIMEOUT)
        .expect("expected a ready line on stdout");
    let event: Value = serde_json::from_str(&first_line).expect("stdout line should be valid JSON");

    assert_eq!(event["v"], 1);
    assert_eq!(event["event"], "ready");
    assert_eq!(event["pid"].as_u64(), Some(u64::from(child_pid)));
    assert_eq!(event["osc_addr"], Value::Null);

    let ws_addr: SocketAddr = event["ws_addr"]
        .as_str()
        .expect("ws_addr should be a string")
        .parse()
        .expect("ws_addr should be a valid socket address");
    assert_ne!(
        ws_addr.port(),
        0,
        "ready.ws_addr must carry the actually bound port, not the configured 0"
    );

    TcpStream::connect(ws_addr).unwrap_or_else(|e| {
        panic!("should be able to connect to the bound ws_addr {ws_addr}: {e}")
    });

    // Close stdin: the wrapper contract's stop signal in headless mode.
    drop(child.stdin.take());

    let status = wait_with_timeout(&mut child, WAIT_TIMEOUT);
    assert!(status.success(), "expected a clean exit, got {status:?}");

    reader_handle
        .join()
        .expect("stdout reader thread should not panic");
    assert_no_further_lines(&rx);
}

#[test]
fn fatal_event_reports_port_in_use_and_exits_non_zero() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind an occupying listener");
    let occupied_addr = listener.local_addr().unwrap();

    let mut child = spawn_phase4(&[
        "--stdout-events",
        "json",
        "--test-hz",
        "440",
        "--ws-addr",
        &occupied_addr.to_string(),
        "--controller-mode",
        "headless",
    ]);
    let stdout = child.stdout.take().expect("stdout should be piped");
    let (rx, reader_handle) = spawn_line_reader(stdout);

    let first_line = rx
        .recv_timeout(WAIT_TIMEOUT)
        .expect("expected a fatal line on stdout");
    let event: Value = serde_json::from_str(&first_line).expect("stdout line should be valid JSON");

    assert_eq!(event["v"], 1);
    assert_eq!(event["event"], "fatal");
    assert_eq!(event["reason"], "port_in_use");

    drop(child.stdin.take());
    let status = wait_with_timeout(&mut child, WAIT_TIMEOUT);
    assert!(
        !status.success(),
        "a fatal startup failure must exit non-zero"
    );

    reader_handle
        .join()
        .expect("stdout reader thread should not panic");
    assert_no_further_lines(&rx);

    drop(listener);
}

#[test]
fn fatal_event_reports_invalid_config_for_a_missing_config_file() {
    let mut child = spawn_phase4(&[
        "--stdout-events",
        "json",
        "--config",
        "a-path-no-real-machine-will-have.yaml",
    ]);
    let stdout = child.stdout.take().expect("stdout should be piped");
    let (rx, reader_handle) = spawn_line_reader(stdout);

    let first_line = rx
        .recv_timeout(WAIT_TIMEOUT)
        .expect("expected a fatal line on stdout");
    let event: Value = serde_json::from_str(&first_line).expect("stdout line should be valid JSON");

    assert_eq!(event["v"], 1);
    assert_eq!(event["event"], "fatal");
    assert_eq!(event["reason"], "invalid_config");

    drop(child.stdin.take());
    let status = wait_with_timeout(&mut child, WAIT_TIMEOUT);
    assert!(
        !status.success(),
        "a fatal startup failure must exit non-zero"
    );

    reader_handle
        .join()
        .expect("stdout reader thread should not panic");
    assert_no_further_lines(&rx);
}

#[test]
fn stdout_stays_silent_without_the_flag_on_a_clean_run() {
    let mut child = spawn_phase4(&[
        "--test-hz",
        "440",
        "--ws-addr",
        "127.0.0.1:0",
        "--controller-mode",
        "headless",
    ]);
    let mut stdout = child.stdout.take().expect("stdout should be piped");

    drop(child.stdin.take());
    let status = wait_with_timeout(&mut child, WAIT_TIMEOUT);
    assert!(status.success(), "expected a clean exit, got {status:?}");

    let mut buf = Vec::new();
    stdout.read_to_end(&mut buf).expect("failed to read stdout");
    assert!(
        buf.is_empty(),
        "stdout must be byte-for-byte silent without --stdout-events, got {buf:?}"
    );
}

#[test]
fn stdout_stays_silent_without_the_flag_on_a_fatal_startup_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind an occupying listener");
    let occupied_addr = listener.local_addr().unwrap();

    let mut child = spawn_phase4(&[
        "--test-hz",
        "440",
        "--ws-addr",
        &occupied_addr.to_string(),
        "--controller-mode",
        "headless",
    ]);
    let mut stdout = child.stdout.take().expect("stdout should be piped");

    let status = wait_with_timeout(&mut child, WAIT_TIMEOUT);
    assert!(
        !status.success(),
        "a fatal startup failure must exit non-zero"
    );

    let mut buf = Vec::new();
    stdout.read_to_end(&mut buf).expect("failed to read stdout");
    assert!(
        buf.is_empty(),
        "stdout must be byte-for-byte silent without --stdout-events, got {buf:?}"
    );

    drop(listener);
}
