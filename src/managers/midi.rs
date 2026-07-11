//! MIDI input listener. Connects to a real device via midir, or drives a
//! synthetic clock at a configured tempo, mirroring the calibration or device
//! split the audio input already has.
//!
//! Writes directly to two atomics on `AppState`, no channel. The mapper reads
//! them once per broadcast frame it actually sends. Runs at a lower thread
//! priority than the analyser, audio takes priority under contention.
//!
//! Deliberately minimal, raw bytes are matched directly against the four MIDI
//! Real-Time codes phase4 cares about, no parsed event type, no accumulated
//! step position, no subdivision. Start, Stop, Continue, and a running Clock
//! tick count, nothing else.

use crate::app::{AppState, MIDI_TRANSPORT_CONTINUE, MIDI_TRANSPORT_START, MIDI_TRANSPORT_STOP};
use crate::config::ConfigMidiInput;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thread_priority::{set_current_thread_priority, ThreadPriority, ThreadPriorityValue};

/// MIDI listener thread priority, same crate, same 0-99 cross-platform scale
/// as the analyser priority. Deliberately lower.
const MIDI_THREAD_PRIORITY: u8 = 20;

/// Poll cadence for both the synthetic clock tick scheduling and the real
/// device path shutdown check. `keep_running` is checked more often than any
/// realistic tick interval.
const MIDI_POLL_INTERVAL_MS: u64 = 10;

const MIDI_CLOCK_TICKS_PER_QUARTER_NOTE: f64 = 24.0;

/// Matches a single raw MIDI status byte against the four Real-Time codes
/// phase4 cares about, updating `AppState` directly. All other bytes are
/// ignored.
fn record_byte(byte: u8, state: &AppState) {
    match byte {
        0xFA => state
            .midi_last_transport
            .store(MIDI_TRANSPORT_START, Ordering::Release),
        0xFC => state
            .midi_last_transport
            .store(MIDI_TRANSPORT_STOP, Ordering::Release),
        0xFB => state
            .midi_last_transport
            .store(MIDI_TRANSPORT_CONTINUE, Ordering::Release),
        0xF8 => {
            state.midi_clock_ticks.fetch_add(1, Ordering::AcqRel);
        }
        _ => {}
    }
}

pub struct MidiListener;

impl MidiListener {
    /// Spawns the MIDI listener on a dedicated OS thread.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned.
    pub fn spawn(input: ConfigMidiInput, state: Arc<AppState>) -> JoinHandle<()> {
        thread::Builder::new()
            .name("midi-input".into())
            .spawn(move || {
                super::log_priority_result(set_current_thread_priority(
                    ThreadPriority::Crossplatform(
                        ThreadPriorityValue::try_from(MIDI_THREAD_PRIORITY)
                            .expect("valid priority"),
                    ),
                ));

                match input {
                    ConfigMidiInput::TestClock(bpm) => run_synthetic_clock(bpm, &state),
                    ConfigMidiInput::Device(name) => run_real_device(&name, &state),
                }
            })
            .expect("failed to spawn midi-input thread")
    }
}

fn run_synthetic_clock(bpm: f32, state: &Arc<AppState>) {
    let tick_interval =
        Duration::from_secs_f64(60.0 / (f64::from(bpm) * MIDI_CLOCK_TICKS_PER_QUARTER_NOTE));
    let poll_interval = Duration::from_millis(MIDI_POLL_INTERVAL_MS);

    // A synthetic clock is always running the instant it starts.
    record_byte(0xFA, state);

    let mut next_tick = Instant::now() + tick_interval;
    while state.keep_running.load(Ordering::Acquire) {
        let now = Instant::now();
        if now >= next_tick {
            record_byte(0xF8, state);
            next_tick += tick_interval;
        }
        let sleep_for = next_tick
            .saturating_duration_since(Instant::now())
            .min(poll_interval);
        thread::sleep(sleep_for);
    }
}

fn run_real_device(name_query: &str, state: &Arc<AppState>) {
    let midi_in = match midir::MidiInput::new("phase4") {
        Ok(input) => input,
        Err(e) => {
            log::error!("Failed to initialise MIDI input: {e}");
            return;
        }
    };

    let needle = name_query.to_lowercase();
    let ports = midi_in.ports();
    let port = ports.iter().find(|p| {
        midi_in.port_name(p).is_ok_and(|name| {
            name.eq_ignore_ascii_case(name_query) || name.to_lowercase().contains(&needle)
        })
    });

    let Some(port) = port else {
        log::error!("No MIDI input device matching '{name_query}' found");
        return;
    };

    let port_name = midi_in
        .port_name(port)
        .unwrap_or_else(|_| name_query.to_string());
    let thread_state = Arc::clone(state);

    let connection = midi_in.connect(
        port,
        "phase4-midi-in",
        move |_timestamp_us, bytes, _data: &mut ()| {
            for &byte in bytes {
                record_byte(byte, &thread_state);
            }
        },
        (),
    );

    let Ok(_connection) = connection else {
        log::error!("Failed to connect to MIDI device '{port_name}'");
        return;
    };

    log::info!("MIDI input connected: {port_name}");

    // midir delivers bytes on its own backend thread. Holding _connection keeps
    // that alive, this thread only needs to wait for shutdown.
    while state.keep_running.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(MIDI_POLL_INTERVAL_MS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_byte_sets_start() {
        let state = AppState::new();
        record_byte(0xFA, &state);
        assert_eq!(
            state.midi_last_transport.load(Ordering::Acquire),
            MIDI_TRANSPORT_START
        );
    }

    #[test]
    fn record_byte_sets_stop() {
        let state = AppState::new();
        record_byte(0xFC, &state);
        assert_eq!(
            state.midi_last_transport.load(Ordering::Acquire),
            MIDI_TRANSPORT_STOP
        );
    }

    #[test]
    fn record_byte_sets_continue() {
        let state = AppState::new();
        record_byte(0xFB, &state);
        assert_eq!(
            state.midi_last_transport.load(Ordering::Acquire),
            MIDI_TRANSPORT_CONTINUE
        );
    }

    #[test]
    fn record_byte_increments_clock_ticks() {
        let state = AppState::new();
        record_byte(0xF8, &state);
        record_byte(0xF8, &state);
        record_byte(0xF8, &state);
        assert_eq!(state.midi_clock_ticks.load(Ordering::Acquire), 3);
    }

    #[test]
    fn record_byte_ignores_unrecognised_bytes() {
        let state = AppState::new();
        for byte in [0x00u8, 0xFE, 0xFF, 0x90] {
            record_byte(byte, &state);
        }
        assert_eq!(
            state.midi_last_transport.load(Ordering::Acquire),
            crate::app::MIDI_TRANSPORT_NONE
        );
        assert_eq!(state.midi_clock_ticks.load(Ordering::Acquire), 0);
    }

    #[test]
    fn synthetic_clock_tick_interval_at_120_bpm_matches_hand_calculation() {
        // 60_000ms / (120 * 24) = 20.8333ms per tick.
        let interval = Duration::from_secs_f64(60.0 / (120.0 * MIDI_CLOCK_TICKS_PER_QUARTER_NOTE));
        let expected_ms = 20.833_333_333_333_332;
        assert!((interval.as_secs_f64() * 1000.0 - expected_ms).abs() < 1e-6);
    }

    #[test]
    fn synthetic_clock_exits_promptly_when_keep_running_clears() {
        let state = Arc::new(AppState::new());
        let thread_state = state.clone();
        let handle = thread::spawn(move || run_synthetic_clock(120.0, &thread_state));

        thread::sleep(Duration::from_millis(20));
        let start = Instant::now();
        state.keep_running.store(false, Ordering::Release);
        handle.join().expect("thread should not panic");

        assert!(
            start.elapsed() < Duration::from_millis(100),
            "expected shutdown within roughly one poll interval, took {:?}",
            start.elapsed()
        );
    }
}
