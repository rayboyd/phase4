//! [`FrameWriter`] publishes every analysis snapshot into the frame region
//! the XPC service shares with its client.
//!
//! It reads the analyser's [`RawPayload`] watch channel directly, so each
//! snapshot reaches the region about every 10 ms, independent of the
//! mapper's 60 Hz display timer. The MIDI fields come from `AppState` values
//! that nothing clears, so a reader that skips frames loses no steps.

use crate::app::AppState;
use crate::dsp::RawPayload;
use crate::frames::{uptime_raw_ns, FrameMidi, FrameRegion};
use std::sync::{atomic::Ordering, Arc};
use std::thread::JoinHandle;
use tokio::sync::watch;

/// Copies each `RawPayload` snapshot and the MIDI state into a frame region.
pub struct FrameWriter;

impl FrameWriter {
    /// Spawns the frame writer on a dedicated thread through
    /// `spawn_async_worker`.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned or its runtime cannot be built.
    pub fn spawn(
        raw_rx: watch::Receiver<RawPayload>,
        region: Arc<FrameRegion>,
        state: Arc<AppState>,
    ) -> JoinHandle<()> {
        super::spawn_async_worker("frame-writer", Self::run(raw_rx, region, state))
    }

    async fn run(
        mut raw_rx: watch::Receiver<RawPayload>,
        region: Arc<FrameRegion>,
        state: Arc<AppState>,
    ) {
        let mut rejection_logged = false;

        while state.keep_running.load(Ordering::Acquire) {
            if raw_rx.changed().await.is_err() {
                log::info!("- Analyser channel closed, frame writer exiting");
                return;
            }

            let raw = raw_rx.borrow_and_update();
            if raw.channels.is_empty() {
                continue;
            }

            let midi = FrameMidi {
                steps: state.midi_steps.load(Ordering::Acquire),
                transport_count: state.midi_transport_count.load(Ordering::Acquire),
                transport_last: u32::from(state.midi_transport_latest.load(Ordering::Acquire)),
            };
            let published = region.publish(&raw.channels, midi, uptime_raw_ns());
            drop(raw);

            if !published && !rejection_logged {
                log::warn!("A frame with a non-finite value was not published to the frame region");
                rejection_logged = true;
            }
        }
    }
}
