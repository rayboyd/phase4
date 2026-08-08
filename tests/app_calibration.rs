//! Integration tests for App initialisation in calibration mode.
//!
//! These tests exercise the full `App::new()` path without requiring real audio
//! hardware. Calibration mode (via `ConfigInput::Calibration`) replaces the hardware device with
//! a synthetic sine wave generator, making it safe to run in CI.
//!
//! End-to-end, this covers that `AppConfig` is constructed correctly from
//! a test config, that all threads (analyser, mapper, generator) are
//! spawned, that the WebSocket server successfully binds to its address,
//! and that `Drop` cleanly signals threads to stop when `app` goes out of
//! scope.

use phase4::app::App;
use phase4::config::DEFAULT_MAX_CLIENTS;
use phase4::config::{AppConfig, ConfigInput, ConfigOutputs, OutputConfig, TestSignal};
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time::Duration;

const FAILED_STARTUP_SHUTDOWN_GRACE: Duration = Duration::from_millis(250);
const POST_FAILURE_OBSERVATION_TIMEOUT: Duration = Duration::from_millis(250);
const OSC_TEST_RECEIVE_BUFFER_BYTES: usize = 32 * 1024;

/// Builds a single-entry `ConfigOutputs` with a WebSocket transport listening
/// on `addr`, for tests that only care about exercising `App::new`.
fn ws_outputs(addr: SocketAddr) -> ConfigOutputs {
    ConfigOutputs::new(vec![OutputConfig::WebSocket {
        addr,
        max_clients: DEFAULT_MAX_CLIENTS,
        no_browser_origin: false,
    }])
    .expect("a single-element Vec is non-empty")
}

#[test]
fn app_new_returns_error_on_port_collision() {
    // Bind to a random free port to simulate another application using it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let occupied_addr = listener.local_addr().unwrap();

    // Attempt to construct the App using the occupied port.
    let config = AppConfig {
        input: ConfigInput::Calibration(TestSignal::FixedTone(440.0)),
        outputs: ws_outputs(occupied_addr),
        ..AppConfig::default()
    };

    let result = App::new(&config);

    // Verify it returns an error cleanly.
    assert!(
        result.is_err(),
        "App::new() should return an error if the WebSocket port is already in use"
    );

    // The listener is dropped here, freeing the port
}

#[test]
fn app_new_failure_stops_already_started_workers() {
    let osc_receiver = UdpSocket::bind("127.0.0.1:0").expect("failed to bind OSC receiver");
    let osc_address = osc_receiver
        .local_addr()
        .expect("failed to read OSC receiver address");

    let occupied_listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("failed to occupy WebSocket address");
    let occupied_address = occupied_listener
        .local_addr()
        .expect("failed to read occupied WebSocket address");

    let outputs = ConfigOutputs::new(vec![
        OutputConfig::Osc { addr: osc_address },
        OutputConfig::WebSocket {
            addr: occupied_address,
            max_clients: DEFAULT_MAX_CLIENTS,
            no_browser_origin: false,
        },
    ])
    .expect("the output set is non-empty");
    let config = AppConfig {
        input: ConfigInput::Calibration(TestSignal::FixedTone(440.0)),
        outputs,
        ..AppConfig::default()
    };

    let result = App::new(&config);
    assert!(result.is_err(), "the occupied WebSocket port should fail");

    thread::sleep(FAILED_STARTUP_SHUTDOWN_GRACE);

    osc_receiver
        .set_nonblocking(true)
        .expect("failed to make OSC receiver non-blocking");
    let mut buffer = vec![0u8; OSC_TEST_RECEIVE_BUFFER_BYTES];
    loop {
        match osc_receiver.recv_from(&mut buffer) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("failed while draining queued OSC datagrams: {error}"),
        }
    }

    osc_receiver
        .set_nonblocking(false)
        .expect("failed to restore blocking OSC receiver");
    osc_receiver
        .set_read_timeout(Some(POST_FAILURE_OBSERVATION_TIMEOUT))
        .expect("failed to set OSC receive timeout");

    let receive_result = osc_receiver.recv_from(&mut buffer);
    assert!(
        matches!(
            receive_result,
            Err(ref error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
        ),
        "an OSC worker kept transmitting after App::new returned an error: {receive_result:?}"
    );
}

// App::new() should succeed in calibration mode. No audio hardware required.
#[test]
fn app_new_succeeds_in_calibration_mode() {
    let config = AppConfig {
        input: ConfigInput::Calibration(TestSignal::FixedTone(440.0)),
        // Port 0 asks the OS to assign a random free port, avoiding conflicts
        // with the real app or other tests running in parallel.
        outputs: ws_outputs("127.0.0.1:0".parse::<SocketAddr>().unwrap()),
        ..AppConfig::default()
    };

    let result = App::new(&config);
    assert!(result.is_ok(), "App::new() failed: {:?}", result.err());

    // When `app` drops here, the `Drop` impl signals all threads to stop
    // and joins them. If anything panics or deadlocks, the test will fail.
}

#[test]
fn app_new_rejects_direct_numeric_config_that_would_panic_a_worker() {
    let outputs = ConfigOutputs::new(vec![OutputConfig::WebSocket {
        addr: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        max_clients: tokio::sync::Semaphore::MAX_PERMITS + 1,
        no_browser_origin: false,
    }])
    .expect("a single-element Vec is non-empty");
    let config = AppConfig {
        outputs,
        ..AppConfig::default()
    };

    let result = App::new(&config);

    assert!(
        result.is_err(),
        "App::new() should reject numeric config that exceeds a worker's runtime limit"
    );
}

// App::ws_bound_addr() must report the real OS-assigned port for `:0`, not
// the configured placeholder, since it feeds the ready event's ws_addr.
#[test]
fn app_exposes_the_actually_bound_ws_port_not_the_configured_zero() {
    let config = AppConfig {
        input: ConfigInput::Calibration(TestSignal::FixedTone(440.0)),
        outputs: ws_outputs("127.0.0.1:0".parse::<SocketAddr>().unwrap()),
        ..AppConfig::default()
    };

    let app = App::new(&config).expect("App::new() failed in calibration mode");
    let bound_addr = app.ws_bound_addr().expect("WebSocket output is configured");

    assert_ne!(bound_addr.port(), 0, "expected the real bound port");
}

// Calibration mode with a sweep should also initialise without error.
#[test]
fn app_new_succeeds_with_sweep() {
    let config = AppConfig {
        input: ConfigInput::Calibration(TestSignal::Sweep(0.1)), // 0.1 Hz LFO, 10 second sweep cycle
        outputs: ws_outputs("127.0.0.1:0".parse::<SocketAddr>().unwrap()),
        ..AppConfig::default()
    };

    let result = App::new(&config);
    assert!(result.is_ok(), "App::new() failed: {:?}", result.err());
}

// Shutdown must complete within a deadline. A deadlock in any thread join
// would otherwise hang the entire test binary with no diagnostic output.
// The App is constructed and dropped inside `spawn_blocking` because
// `cpal::Stream` is `!Send` on some platforms, making `App` structurally
// `!Send` even when no stream is active.
#[tokio::test]
async fn drop_joins_all_threads_within_deadline() {
    let deadline = std::time::Duration::from_secs(2);

    let result = tokio::time::timeout(
        deadline,
        tokio::task::spawn_blocking(|| {
            let config = AppConfig {
                input: ConfigInput::Calibration(TestSignal::FixedTone(440.0)),
                outputs: ws_outputs("127.0.0.1:0".parse::<SocketAddr>().unwrap()),
                ..AppConfig::default()
            };

            let app = App::new(&config).expect("App::new() failed in calibration mode");
            drop(app);
        }),
    )
    .await;

    match result {
        Ok(Ok(())) => {} // clean shutdown within deadline
        Ok(Err(join_err)) => panic!("spawn_blocking task panicked: {join_err}"),
        Err(elapsed) => panic!("App::drop did not complete within {deadline:?}: {elapsed}"),
    }
}
