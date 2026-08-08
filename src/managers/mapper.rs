//! [`Mapper`] sits between the [`crate::managers::analyser`] and the front-end
//! [`crate::managers::server`] and [`crate::managers::osc`]. It copies each
//! 32-band analysis snapshot into the serialisable [`DisplayPayload`] shape
//! and publishes the latest snapshot at a fixed 60 Hz.
//!
//! The output timer is independent of analyser updates. A slower or irregular
//! analyser therefore changes data freshness without changing the public
//! broadcast cadence. The latest snapshot is reused when no newer analysis
//! frame is available.

use crate::app::AppState;
use crate::dsp::{DisplayChannelLevel, DisplayPayload, MidiSnapshot, RawChannelLevel, RawPayload};
use crate::managers::{
    MIDI_TRANSPORT_CONTINUE, MIDI_TRANSPORT_NONE, MIDI_TRANSPORT_START, MIDI_TRANSPORT_STOP,
};
use std::sync::{atomic::Ordering, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};

/// Fixed number of display frames published per second.
const BROADCAST_RATE_HZ: u64 = 60;

/// Nanoseconds in one second, used to derive the fixed broadcast interval.
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;

/// Fixed interval between display frames.
const BROADCAST_INTERVAL: Duration =
    Duration::from_nanos(NANOSECONDS_PER_SECOND / BROADCAST_RATE_HZ);

/// Copies analysis snapshots into the display payload and broadcasts them.
pub struct Mapper;

impl Mapper {
    /// Spawns the mapper on a dedicated background thread.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned or if the single-threaded
    /// Tokio runtime cannot be built.
    pub fn spawn(
        raw_rx: watch::Receiver<RawPayload>,
        display_tx: watch::Sender<DisplayPayload>,
        channels: usize,
        state: Arc<AppState>,
        midi_enabled: bool,
    ) -> JoinHandle<()> {
        super::spawn_async_worker(
            "mapper",
            Self::run(raw_rx, display_tx, channels, state, midi_enabled),
        )
    }

    async fn run(
        mut raw_rx: watch::Receiver<RawPayload>,
        display_tx: watch::Sender<DisplayPayload>,
        channels: usize,
        state: Arc<AppState>,
        midi_enabled: bool,
    ) {
        let mut display_data = DisplayPayload::new(channels);

        while state.keep_running.load(Ordering::Acquire) {
            if raw_rx.changed().await.is_err() {
                log::info!("- Analyser channel closed, mapper exiting");
                return;
            }
            if !raw_rx.borrow_and_update().channels.is_empty() {
                break;
            }
        }

        let first_deadline = Instant::now() + BROADCAST_INTERVAL;
        let mut broadcast_timer = tokio::time::interval_at(first_deadline, BROADCAST_INTERVAL);
        broadcast_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut latest_frame_available = true;

        while state.keep_running.load(Ordering::Acquire) {
            tokio::select! {
                raw_result = raw_rx.changed() => {
                    if raw_result.is_err() {
                        log::info!("- Analyser channel closed, mapper exiting");
                        break;
                    }
                    latest_frame_available = !raw_rx.borrow_and_update().channels.is_empty();
                }
                _ = broadcast_timer.tick() => {
                    if !state.is_active.load(Ordering::Acquire) {
                        latest_frame_available = false;
                        continue;
                    }
                    if !latest_frame_available {
                        continue;
                    }

                    let raw = raw_rx.borrow_and_update();
                    debug_assert_eq!(raw.channels.len(), display_data.channels.len());
                    for (source, target) in raw.channels.iter().zip(&mut display_data.channels) {
                        map_channel(source, target);
                    }
                    drop(raw);

                    display_data.midi = read_midi_snapshot(&state, midi_enabled);

                    let payload = std::mem::take(&mut display_data);
                    display_data = display_tx.send_replace(payload);
                }
            }
        }
    }
}

/// Copies one channel's fixed 32-band analysis snapshot into its display slot.
fn map_channel(raw: &RawChannelLevel, out: &mut DisplayChannelLevel) {
    out.peak = raw.peak;
    out.bins.copy_from_slice(&raw.bins);
}

/// Reads and clears MIDI transport, and snapshots MIDI steps, once per
/// broadcast frame.
fn read_midi_snapshot(state: &AppState, midi_enabled: bool) -> Option<MidiSnapshot> {
    if !midi_enabled {
        return None;
    }
    let transport_code = state
        .midi_last_transport
        .swap(MIDI_TRANSPORT_NONE, Ordering::AcqRel);
    let steps = state.midi_steps.load(Ordering::Acquire);
    Some(MidiSnapshot {
        transport: transport_code_to_str(transport_code),
        steps,
    })
}

fn transport_code_to_str(code: u8) -> Option<&'static str> {
    match code {
        MIDI_TRANSPORT_START => Some("start"),
        MIDI_TRANSPORT_STOP => Some("stop"),
        MIDI_TRANSPORT_CONTINUE => Some("continue"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::vocoder::VOCODER_BANDS;

    #[test]
    #[allow(clippy::float_cmp)]
    fn map_channel_copies_peak_and_bins() {
        let raw = RawChannelLevel {
            peak: 0.87,
            bins: std::array::from_fn(|index| index as f32 * 0.01),
        };
        let mut out = DisplayChannelLevel {
            peak: 0.0,
            bins: [0.0; VOCODER_BANDS],
        };

        map_channel(&raw, &mut out);

        assert_eq!(out.peak, raw.peak);
        assert_eq!(out.bins, raw.bins);
    }
}
