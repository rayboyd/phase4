//! Integration tests for App initialisation in calibration mode.
//!
//! These tests exercise the full `App::new()` path without requiring real audio
//! hardware. Calibration mode (via `test_hz`) replaces the hardware device with
//! a synthetic sine wave generator, making it safe to run in CI.
//!
//! Key things this covers end-to-end:
//!   - `AppConfig` is constructed correctly from a test config
//!   - All threads (recorder, analyser, mapper, generator) are spawned
//!   - The WebSocket server successfully binds to its address
//!   - `Drop` cleanly signals threads to stop when `app` goes out of scope

use phase4::app::App;
use phase4::config::AppConfig;
use std::net::SocketAddr;

// App::new() should succeed in calibration mode. No audio hardware required.
#[test]
fn app_new_succeeds_in_calibration_mode() {
    let config = AppConfig {
        test_hz: Some(440.0),
        // Port 0 asks the OS to assign a random free port, avoiding conflicts
        // with the real app (8889) or other tests running in parallel.
        addr: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..AppConfig::default()
    };

    let result = App::new(config);
    assert!(result.is_ok(), "App::new() failed: {:?}", result.err());

    // When `app` drops here, the `Drop` impl signals all threads to stop
    // and joins them. If anything panics or deadlocks, the test will fail.
}

// Calibration mode with a sweep should also initialise without error.
#[test]
fn app_new_succeeds_with_sweep() {
    let config = AppConfig {
        test_sweep: Some(0.1), // 0.1 Hz LFO, 10 second sweep cycle
        addr: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..AppConfig::default()
    };

    let result = App::new(config);
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
                test_hz: Some(440.0),
                addr: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                ..AppConfig::default()
            };

            let app = App::new(config).expect("App::new() failed in calibration mode");
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
