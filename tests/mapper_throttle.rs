//! Integration tests for the mapper broadcast rate throttle.
//!
//! These tests drive [`Mapper::spawn`] directly via watch channels, with no
//! server or audio hardware, to verify that the configurable broadcast rate
//! correctly suppresses, passes, or immediately fires frames.

use phase4::app::AppState;
use phase4::dsp::vocoder::VOCODER_BANDS;
use phase4::dsp::{DisplayPayload, RawPayload};
use phase4::managers::Mapper;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::Utf8Bytes;

/// Build the initial display JSON and watch channel pair used by all tests.
fn display_channel(channels: usize) -> (watch::Sender<Utf8Bytes>, watch::Receiver<Utf8Bytes>) {
    let initial = serde_json::to_string(&DisplayPayload::new(channels))
        .expect("failed to serialise initial display payload");
    watch::channel(Utf8Bytes::from(initial))
}

/// Send a raw frame with a unique peak value so each frame is distinguishable.
fn send_frame(raw_tx: &watch::Sender<RawPayload>, channels: usize, peak: f32) {
    let mut payload = RawPayload::new(channels, VOCODER_BANDS);
    for channel in &mut payload.channels {
        channel.peak = peak;
    }
    raw_tx.send_replace(payload);
}

/// Drain all pending updates from the display receiver.
///
/// Returns the number of distinct watch notifications observed. Because
/// `watch` coalesces rapid writes, calling `changed()` once may cover
/// multiple `send_replace` calls. This function counts the number of
/// successful `changed()` returns before the deadline expires.
async fn drain_display_updates(
    display_rx: &mut watch::Receiver<Utf8Bytes>,
    deadline: Duration,
) -> usize {
    let mut count = 0usize;
    let start = tokio::time::Instant::now();
    while start.elapsed() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), display_rx.changed()).await {
            Ok(Ok(())) => count += 1,
            _ => break,
        }
    }
    count
}

// With no broadcast rate limit every frame that the mapper processes is
// forwarded to the display channel.
#[tokio::test]
async fn unlimited_rate_forwards_every_frame() {
    let channels = 1usize;
    let (raw_tx, raw_rx) = watch::channel(RawPayload::new(channels, VOCODER_BANDS));
    let (display_tx, mut display_rx) = display_channel(channels);
    let state = Arc::new(AppState::new());

    let handle = Mapper::spawn(raw_rx, display_tx, channels, state.clone(), None);

    // Allow the mapper thread to start and block on raw_rx.changed().
    sleep(Duration::from_millis(50)).await;

    let frame_count = 5usize;
    for i in 0..frame_count {
        send_frame(&raw_tx, channels, (i + 1) as f32 * 0.1);

        // Consume the display update before sending the next frame.
        // This prevents watch coalescing from merging multiple updates.
        let result = tokio::time::timeout(Duration::from_millis(200), display_rx.changed()).await;
        assert!(
            matches!(result, Ok(Ok(()))),
            "frame {i} should produce a display update"
        );
    }

    state.keep_running.store(false, Ordering::Release);
    drop(raw_tx);
    tokio::task::spawn_blocking(move || handle.join().expect("mapper thread panicked"))
        .await
        .expect("join task failed");
}

// The very first frame must be broadcast immediately regardless of the
// configured rate, so clients get an initial snapshot without waiting for
// the first tick interval to elapse.
#[tokio::test]
async fn first_frame_is_broadcast_immediately() {
    let channels = 1usize;
    let (raw_tx, raw_rx) = watch::channel(RawPayload::new(channels, VOCODER_BANDS));
    let (display_tx, mut display_rx) = display_channel(channels);
    let state = Arc::new(AppState::new());

    // A very low rate (2 Hz, 500 ms interval) to prove the first frame does
    // not wait for the interval to elapse.
    let handle = Mapper::spawn(raw_rx, display_tx, channels, state.clone(), Some(2.0));

    sleep(Duration::from_millis(50)).await;
    send_frame(&raw_tx, channels, 0.42);

    // The update should arrive well within 200 ms, far before the 500 ms tick.
    let result = tokio::time::timeout(Duration::from_millis(200), display_rx.changed()).await;

    assert!(
        result.is_ok(),
        "first frame should be broadcast without waiting for the tick interval"
    );

    state.keep_running.store(false, Ordering::Release);
    drop(raw_tx);
    tokio::task::spawn_blocking(move || handle.join().expect("mapper thread panicked"))
        .await
        .expect("join task failed");
}

// With a low broadcast rate, rapid raw frames are coalesced and fewer display
// updates reach the receiver than frames sent.
#[tokio::test]
async fn throttle_suppresses_intermediate_frames() {
    let channels = 1usize;
    let (raw_tx, raw_rx) = watch::channel(RawPayload::new(channels, VOCODER_BANDS));
    let (display_tx, mut display_rx) = display_channel(channels);
    let state = Arc::new(AppState::new());

    // 5 Hz means one broadcast per 200 ms.
    let handle = Mapper::spawn(raw_rx, display_tx, channels, state.clone(), Some(5.0));

    sleep(Duration::from_millis(50)).await;

    // Blast 50 frames over ~500 ms (one every 10 ms).
    let total_frames = 50usize;
    for i in 0..total_frames {
        send_frame(&raw_tx, channels, (i + 1) as f32 * 0.01);
        sleep(Duration::from_millis(10)).await;
    }

    // Drain with a generous window.
    let updates = drain_display_updates(&mut display_rx, Duration::from_millis(400)).await;

    // At 5 Hz over ~500 ms we expect roughly 2 to 4 broadcasts (first fires
    // immediately, then one per 200 ms tick). Allow a wide band so the test
    // is not timing-sensitive.
    assert!(
        (1..=10).contains(&updates),
        "expected a small number of throttled broadcasts, got {updates}"
    );
    assert!(
        updates < total_frames,
        "throttle should suppress intermediate frames: got {updates} updates from {total_frames} frames"
    );

    state.keep_running.store(false, Ordering::Release);
    drop(raw_tx);
    tokio::task::spawn_blocking(move || handle.join().expect("mapper thread panicked"))
        .await
        .expect("join task failed");
}
