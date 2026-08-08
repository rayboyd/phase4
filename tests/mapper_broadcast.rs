//! Integration tests for the mapper's fixed 60 Hz broadcast cadence.

use phase4::app::AppState;
use phase4::dsp::{DisplayPayload, RawPayload};
use phase4::managers::Mapper;
use std::sync::{atomic::Ordering, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::watch;

const ANALYSIS_INTERVAL: Duration = Duration::from_millis(10);
const MAPPER_STARTUP_DELAY: Duration = Duration::from_millis(50);
const UPDATE_TIMEOUT: Duration = Duration::from_millis(200);

fn display_channel(
    channels: usize,
) -> (
    watch::Sender<DisplayPayload>,
    watch::Receiver<DisplayPayload>,
) {
    watch::channel(DisplayPayload::new(channels))
}

fn send_frame(raw_tx: &watch::Sender<RawPayload>, channels: usize, peak: f32) {
    let mut payload = RawPayload::new(channels);
    for channel in &mut payload.channels {
        channel.peak = peak;
    }
    raw_tx.send_replace(payload);
}

async fn stop_mapper(state: &AppState, handle: JoinHandle<()>) {
    state.keep_running.store(false, Ordering::Release);
    tokio::task::spawn_blocking(move || handle.join().expect("mapper thread panicked"))
        .await
        .expect("join task failed");
}

#[tokio::test]
async fn sixty_hz_broadcast_is_independent_of_hundred_hz_analysis() {
    const OBSERVATION_WINDOW: Duration = Duration::from_secs(2);
    const TARGET_BROADCASTS: usize = 120;
    const MINIMUM_BROADCASTS: usize = 115;
    const MAXIMUM_BROADCASTS: usize = 122;

    let channels = 1usize;
    let (raw_tx, raw_rx) = watch::channel(RawPayload::new(channels));
    let (display_tx, mut display_rx) = display_channel(channels);
    let state = Arc::new(AppState::new());
    let handle = Mapper::spawn(raw_rx, display_tx, channels, state.clone(), false);

    tokio::time::sleep(MAPPER_STARTUP_DELAY).await;

    let producer = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(ANALYSIS_INTERVAL);
        let mut frame = 0usize;
        loop {
            ticker.tick().await;
            frame += 1;
            send_frame(&raw_tx, channels, frame as f32);
        }
    });

    let started = tokio::time::Instant::now();
    let mut broadcasts = 0usize;
    while started.elapsed() < OBSERVATION_WINDOW {
        let remaining = OBSERVATION_WINDOW.saturating_sub(started.elapsed());
        if tokio::time::timeout(remaining, display_rx.changed())
            .await
            .is_ok_and(|result| result.is_ok())
        {
            broadcasts += 1;
        } else {
            break;
        }
    }

    assert!(
        (MINIMUM_BROADCASTS..=MAXIMUM_BROADCASTS).contains(&broadcasts),
        "expected {TARGET_BROADCASTS} broadcasts at 60 Hz, got {broadcasts}"
    );

    producer.abort();
    let _ = producer.await;
    stop_mapper(&state, handle).await;
}

#[tokio::test]
#[allow(clippy::float_cmp)]
async fn broadcast_reuses_the_latest_analysis_snapshot() {
    let channels = 1usize;
    let (raw_tx, raw_rx) = watch::channel(RawPayload::new(channels));
    let (display_tx, mut display_rx) = display_channel(channels);
    let state = Arc::new(AppState::new());
    let handle = Mapper::spawn(raw_rx, display_tx, channels, state.clone(), false);

    tokio::time::sleep(MAPPER_STARTUP_DELAY).await;
    send_frame(&raw_tx, channels, 0.42);

    tokio::time::timeout(UPDATE_TIMEOUT, display_rx.changed())
        .await
        .expect("first display frame timed out")
        .expect("display channel closed");
    assert_eq!(display_rx.borrow_and_update().channels[0].peak, 0.42);

    tokio::time::timeout(UPDATE_TIMEOUT, display_rx.changed())
        .await
        .expect("repeated display frame timed out")
        .expect("display channel closed");
    assert_eq!(display_rx.borrow_and_update().channels[0].peak, 0.42);

    stop_mapper(&state, handle).await;
}

#[tokio::test]
async fn midi_steps_are_sampled_on_every_broadcast_frame() {
    let channels = 1usize;
    let (raw_tx, raw_rx) = watch::channel(RawPayload::new(channels));
    let (display_tx, mut display_rx) = display_channel(channels);
    let state = Arc::new(AppState::new());
    let handle = Mapper::spawn(raw_rx, display_tx, channels, state.clone(), true);

    state.midi_steps.store(5, Ordering::Release);
    tokio::time::sleep(MAPPER_STARTUP_DELAY).await;
    send_frame(&raw_tx, channels, 0.1);

    tokio::time::timeout(UPDATE_TIMEOUT, display_rx.changed())
        .await
        .expect("first display frame timed out")
        .expect("display channel closed");
    assert_eq!(
        display_rx
            .borrow_and_update()
            .midi
            .as_ref()
            .map(|midi| midi.steps),
        Some(5)
    );

    state.midi_steps.store(7, Ordering::Release);
    tokio::time::timeout(UPDATE_TIMEOUT, display_rx.changed())
        .await
        .expect("second display frame timed out")
        .expect("display channel closed");
    assert_eq!(
        display_rx
            .borrow_and_update()
            .midi
            .as_ref()
            .map(|midi| midi.steps),
        Some(7)
    );

    stop_mapper(&state, handle).await;
}

#[tokio::test]
#[allow(clippy::float_cmp)]
async fn resume_waits_for_a_fresh_analysis_snapshot() {
    const PAUSE_SETTLE_TIME: Duration = Duration::from_millis(50);

    let channels = 1usize;
    let (raw_tx, raw_rx) = watch::channel(RawPayload::new(channels));
    let (display_tx, mut display_rx) = display_channel(channels);
    let state = Arc::new(AppState::new());
    let handle = Mapper::spawn(raw_rx, display_tx, channels, state.clone(), false);

    tokio::time::sleep(MAPPER_STARTUP_DELAY).await;
    send_frame(&raw_tx, channels, 0.1);
    tokio::time::timeout(UPDATE_TIMEOUT, display_rx.changed())
        .await
        .expect("initial display frame timed out")
        .expect("display channel closed");
    display_rx.borrow_and_update();

    state.is_active.store(false, Ordering::Release);
    send_frame(&raw_tx, channels, 0.2);
    tokio::time::sleep(PAUSE_SETTLE_TIME).await;
    while display_rx.has_changed().unwrap_or(false) {
        display_rx.borrow_and_update();
    }

    state.is_active.store(true, Ordering::Release);
    let stale_update = tokio::time::timeout(PAUSE_SETTLE_TIME, display_rx.changed()).await;
    assert!(
        stale_update.is_err(),
        "resume must not publish the pre-resume snapshot"
    );

    send_frame(&raw_tx, channels, 0.3);
    tokio::time::timeout(UPDATE_TIMEOUT, display_rx.changed())
        .await
        .expect("fresh post-resume frame timed out")
        .expect("display channel closed");
    assert_eq!(display_rx.borrow_and_update().channels[0].peak, 0.3);

    stop_mapper(&state, handle).await;
}
