//! End to end supervision of the headless binary.
//!
//! These tests drive the real process the way a host does. They spawn it with
//! no terminal, read the event stream from stdout, and signal it to stop.

use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

/// How long to wait for the child to exit after signalling it.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often to check whether the child has exited.
const EXIT_POLL: Duration = Duration::from_millis(25);

fn spawn(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_phase4"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn phase4")
}

fn read_event(reader: &mut BufReader<ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("failed to read an event line");
    assert!(!line.is_empty(), "the event stream ended unexpectedly");
    serde_json::from_str(&line)
        .unwrap_or_else(|error| panic!("invalid event line {line:?}: {error}"))
}

fn signal(child: &Child, name: &str) {
    let status = Command::new("kill")
        .args([name, &child.id().to_string()])
        .status()
        .expect("failed to run kill");
    assert!(status.success(), "kill {name} failed");
}

fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + EXIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("failed to poll the child") {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "phase4 did not exit within {EXIT_TIMEOUT:?}"
        );
        std::thread::sleep(EXIT_POLL);
    }
}

#[test]
fn a_headless_run_reports_ready_then_shuts_down_on_a_signal() {
    let mut child = spawn(&["--headless", "--test-hz", "440", "--ws-addr", "127.0.0.1:0"]);
    let mut reader = BufReader::new(child.stdout.take().expect("stdout must be piped"));

    let ready = read_event(&mut reader);
    assert_eq!(ready["v"], 1);
    assert_eq!(ready["event"], "ready");
    assert_eq!(
        ready["audio"]["device"],
        serde_json::Value::Null,
        "calibration mode resolves no hardware device"
    );
    assert!(
        ready["audio"]["sample_rate"].as_u64().unwrap_or(0) > 0,
        "ready must report the resolved sample rate, got: {ready}"
    );
    assert!(
        !ready["audio"]["channels"]
            .as_array()
            .expect("channels must be an array")
            .is_empty(),
        "ready must report at least one analysed channel"
    );

    let bound = ready["outputs"]["websocket"]
        .as_str()
        .expect("ready must report the bound WebSocket address")
        .to_owned();
    let port = bound
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .expect("the bound address must carry a port");
    assert_ne!(port, 0, "a zero port must resolve to the real one");
    TcpStream::connect(&bound).expect("the reported port must accept a connection");

    signal(&child, "-TERM");

    let shutdown = read_event(&mut reader);
    assert_eq!(shutdown["v"], 1);
    assert_eq!(shutdown["event"], "shutdown");
    assert_eq!(shutdown["reason"], "signal");

    let status = wait_for_exit(&mut child);
    assert!(status.success(), "a signalled shutdown must exit zero");

    let mut trailing = String::new();
    reader
        .read_to_string(&mut trailing)
        .expect("failed to drain stdout");
    assert!(
        trailing.trim().is_empty(),
        "stdout must carry the event stream alone, got: {trailing}"
    );
}

#[test]
fn an_interrupt_shuts_a_headless_run_down_the_same_way() {
    let mut child = spawn(&["--headless", "--test-hz", "440", "--ws-addr", "127.0.0.1:0"]);
    let mut reader = BufReader::new(child.stdout.take().expect("stdout must be piped"));

    assert_eq!(read_event(&mut reader)["event"], "ready");
    signal(&child, "-INT");
    assert_eq!(read_event(&mut reader)["event"], "shutdown");
    assert!(
        wait_for_exit(&mut child).success(),
        "an interrupted shutdown must exit zero"
    );
}

#[test]
fn a_headless_startup_failure_reports_a_typed_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_phase4"))
        .args(["--headless", "--test-hz", "440"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run phase4");

    assert!(
        !output.status.success(),
        "a run with no output configured must fail"
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be UTF-8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one event must be written, got: {stdout}"
    );

    let event: serde_json::Value =
        serde_json::from_str(lines[0]).expect("the failure must be a JSON event");
    assert_eq!(event["v"], 1);
    assert_eq!(event["event"], "error");
    assert_eq!(event["code"], "NoOutputConfigured");
    assert!(
        event["message"]
            .as_str()
            .expect("message must be a string")
            .contains("--ws-addr"),
        "the message must carry the original guidance, got: {event}"
    );
}

#[test]
fn headless_logs_stay_on_stderr_without_the_raw_mode_line_ending() {
    let mut child = spawn(&["--headless", "--test-hz", "440", "--ws-addr", "127.0.0.1:0"]);
    let mut reader = BufReader::new(child.stdout.take().expect("stdout must be piped"));
    assert_eq!(read_event(&mut reader)["event"], "ready");
    signal(&child, "-TERM");
    assert_eq!(read_event(&mut reader)["event"], "shutdown");
    wait_for_exit(&mut child);

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr must be piped")
        .read_to_string(&mut stderr)
        .expect("failed to read stderr");

    assert!(
        stderr.contains("Ready."),
        "the readiness log must reach stderr, got: {stderr}"
    );
    assert!(
        !stderr.contains('\r'),
        "headless logs must not carry the raw mode line ending, got: {stderr:?}"
    );
}
