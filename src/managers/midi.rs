//! MIDI input listener. Connects to a real device via midir, or drives a
//! synthetic clock at a configured tempo, mirroring the calibration or device
//! split the audio input already has.
//!
//! The device callback or synthetic clock writes the MIDI atomics on
//! `AppState`. The mapper samples them once per published frame, and on macOS
//! the frame writer reads them for every analysis snapshot. The synthetic clock
//! runs on this worker at a lower priority than the analyser. A real device
//! delivers bytes on midir's own backend thread, and this worker only holds
//! the connection open.
//!
//! Raw bytes are matched directly against the four MIDI Real-Time codes
//! phase4 cares about. Start, Stop, Continue, and a running 1/16 step
//! count derived from Clock ticks. Start resets the count. It advances once
//! per six ticks, including ticks received while stopped, and wraps on u32
//! overflow. Transport stores only the most recent event.

use crate::app::AppState;
use crate::ListFormat;
use anyhow::{Context, Result};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thread_priority::{set_current_thread_priority, ThreadPriority, ThreadPriorityValue};

/// A single enumerated MIDI input device, serialised as one entry in the
/// JSON array produced by `--midi-list-format json`.
#[derive(Debug, Serialize)]
pub(crate) struct MidiDeviceInfo {
    /// Zero-based position in the port enumeration.
    pub(crate) index: usize,

    /// Device name, or "Unknown Device" if the port name could not be read.
    pub(crate) name: String,
}

/// MIDI listener thread priority. Set lower than analyser.
const MIDI_THREAD_PRIORITY: u8 = 20;

/// Longest sleep between shutdown checks in the synthetic clock loop.
const MIDI_POLL_INTERVAL_MS: u64 = 10;

/// Raw MIDI clock ticks (0xF8 bytes) per 1/16 note step, phase4's fixed
/// resolution of 24 ticks per quarter note divided by four.
const MIDI_CLOCK_TICKS_PER_STEP: u8 = 6;

/// Raw MIDI Real-Time status bytes `record_byte` matches against.
const MIDI_STATUS_TIMING_CLOCK: u8 = 0xF8;
const MIDI_STATUS_START: u8 = 0xFA;
const MIDI_STATUS_CONTINUE: u8 = 0xFB;
const MIDI_STATUS_STOP: u8 = 0xFC;

/// Encoding of the last MIDI transport event seen, stored in an `AtomicU8`
/// on `AppState`. `NONE` means no transport event since the last read.
pub(crate) const MIDI_TRANSPORT_NONE: u8 = 0;
pub(crate) const MIDI_TRANSPORT_START: u8 = 1;
pub(crate) const MIDI_TRANSPORT_STOP: u8 = 2;
pub(crate) const MIDI_TRANSPORT_CONTINUE: u8 = 3;

#[cfg(test)]
type MidiStartPublishedObserver = Option<Box<dyn Fn(&AppState)>>;

#[cfg(test)]
thread_local! {
    static MIDI_START_PUBLISHED_OBSERVER: std::cell::RefCell<MidiStartPublishedObserver> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn observe_published_midi_start(state: &AppState) {
    MIDI_START_PUBLISHED_OBSERVER.with(|observer| {
        if let Some(observer) = observer.borrow().as_ref() {
            observer(state);
        }
    });
}

/// Matches a single raw MIDI status byte against the four Real-Time codes
/// phase4 cares about. Start, Stop, and Continue update `AppState` directly.
/// Clock ticks accumulate privately in `ticks_since_step` and are only
/// published to `AppState` once every `MIDI_CLOCK_TICKS_PER_STEP` ticks, so
/// what phase4 exposes is a step count computed against the real MIDI clock.
/// This count is absolute since the latest Start event, and the mapper never
/// clears it. All other bytes are ignored.
fn record_byte(byte: u8, state: &AppState, ticks_since_step: &mut u8) {
    match byte {
        MIDI_STATUS_START => {
            *ticks_since_step = 0;
            state.midi_steps.store(0, Ordering::Release);
            record_transport(state, MIDI_TRANSPORT_START);
            #[cfg(test)]
            observe_published_midi_start(state);
        }
        MIDI_STATUS_STOP => record_transport(state, MIDI_TRANSPORT_STOP),
        MIDI_STATUS_CONTINUE => record_transport(state, MIDI_TRANSPORT_CONTINUE),
        MIDI_STATUS_TIMING_CLOCK => {
            *ticks_since_step += 1;
            if *ticks_since_step >= MIDI_CLOCK_TICKS_PER_STEP {
                *ticks_since_step = 0;
                state.midi_steps.fetch_add(1, Ordering::AcqRel);
            }
        }
        _ => {}
    }
}

/// Records one transport event. The mapper reads and clears
/// `midi_last_transport` for each broadcast frame, while the frame region
/// reads the running count and the latest code, which nothing clears.
fn record_transport(state: &AppState, code: u8) {
    state.midi_last_transport.store(code, Ordering::Release);
    state.midi_transport_latest.store(code, Ordering::Release);
    state.midi_transport_count.fetch_add(1, Ordering::AcqRel);
}

/// Typed MIDI device failures. Variant names are headless event codes.
///
/// Message texts are part of the user-facing, not machine-read, surface.
#[derive(Debug, thiserror::Error)]
pub enum MidiDeviceError {
    /// The MIDI backend could not be initialised.
    #[error("MIDI input could not be initialised: {message}")]
    MidiUnavailable { message: String },

    /// No MIDI input port matched the query, exactly or as a substring.
    #[error(
        "No MIDI input device matched \"{query}\". Run with --midi-list to see available devices."
    )]
    MidiNoMatch { query: String },

    /// A port matched the query but could not be opened.
    #[error("Failed to connect to MIDI device \"{device}\": {message}")]
    MidiConnectFailed { device: String, message: String },
}

/// Resolved MIDI input source, either a synthetic clock spec or an open real
/// device connection. Mirrors `InputSource` for audio.
pub(crate) enum MidiInputSource {
    /// Synthetic clock, driven at the given BPM instead of a real device.
    TestClock { bpm: f32, tick_interval: Duration },

    /// An open real MIDI input connection, established before worker startup.
    Hardware(midir::MidiInputConnection<u8>),
}

fn set_midi_thread_priority() {
    super::log_priority_result(set_current_thread_priority(ThreadPriority::Crossplatform(
        ThreadPriorityValue::try_from(MIDI_THREAD_PRIORITY).expect("valid priority"),
    )));
}

/// Listens for MIDI transport and clock events, real or synthetic.
pub struct MidiListener;

impl MidiListener {
    /// Queries the system for all available MIDI input devices and prints
    /// them in the requested format.
    ///
    /// # Errors
    ///
    /// Returns an error if MIDI input cannot be initialised, or if JSON
    /// encoding of the device list fails.
    pub fn list_devices(format: ListFormat) -> Result<()> {
        let entries = Self::enumerate_devices()?;
        match format {
            ListFormat::Text => {
                Self::list_devices_text(&entries);
                Ok(())
            }
            ListFormat::Json => Self::list_devices_json(&entries),
        }
    }

    pub(crate) fn enumerate_devices() -> Result<Vec<MidiDeviceInfo>> {
        let midi_in = midir::MidiInput::new("phase4").context("Failed to initialise MIDI input")?;
        let ports = midi_in.ports();

        Ok(ports
            .iter()
            .enumerate()
            .map(|(index, port)| MidiDeviceInfo {
                index,
                name: midi_in
                    .port_name(port)
                    .unwrap_or_else(|_| "Unknown Device".to_string()),
            })
            .collect())
    }

    fn list_devices_text(entries: &[MidiDeviceInfo]) {
        if entries.is_empty() {
            log::warn!("[*] No MIDI input devices detected.");
            return;
        }

        for entry in entries {
            log::info!("[{}] {}", entry.index, entry.name);
        }
    }

    fn list_devices_json(entries: &[MidiDeviceInfo]) -> Result<()> {
        let json =
            serde_json::to_string(entries).context("Failed to serialise MIDI device list")?;
        println!("{json}");
        Ok(())
    }

    /// Spawns the MIDI listener on a dedicated OS thread.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned.
    pub(crate) fn spawn(source: MidiInputSource, state: Arc<AppState>) -> JoinHandle<()> {
        thread::Builder::new()
            .name("midi-input".into())
            .spawn(move || {
                set_midi_thread_priority();
                match source {
                    MidiInputSource::TestClock { tick_interval, .. } => {
                        run_synthetic_clock(tick_interval, &state);
                    }
                    MidiInputSource::Hardware(connection) => run_real_device(connection, &state),
                }
            })
            .expect("failed to spawn midi-input thread")
    }
}

/// Resolves and connects to a real MIDI input device synchronously, before
/// any worker thread is spawned. Mirrors the audio device resolution path, so
/// missing devices and connection failures are reported during construction.
///
/// # Errors
///
/// Returns an error if MIDI input cannot be initialised, no port matches the
/// given name, or the selected port cannot be opened.
pub(crate) fn connect_midi_device(
    name_query: &str,
    state: Arc<AppState>,
) -> Result<MidiInputSource> {
    let midi_in =
        midir::MidiInput::new("phase4").map_err(|error| MidiDeviceError::MidiUnavailable {
            message: error.to_string(),
        })?;

    let ports = midi_in.ports();
    let port = find_matching_midi_device(ports, name_query, |port| midi_in.port_name(port).ok())
        .ok_or_else(|| MidiDeviceError::MidiNoMatch {
            query: name_query.to_owned(),
        })?;

    let port_name = midi_in
        .port_name(&port)
        .unwrap_or_else(|_| name_query.to_string());
    let callback_state = state;
    let connection = midi_in
        .connect(
            &port,
            "phase4-midi-in",
            move |_timestamp_us, bytes, ticks_since_step: &mut u8| {
                for &byte in bytes {
                    record_byte(byte, &callback_state, ticks_since_step);
                }
            },
            0u8,
        )
        .map_err(|error| MidiDeviceError::MidiConnectFailed {
            device: port_name.clone(),
            message: error.to_string(),
        })?;

    log::info!("MIDI input connected: {port_name}");
    Ok(MidiInputSource::Hardware(connection))
}

fn find_matching_midi_device<T>(
    devices: impl IntoIterator<Item = T>,
    name_query: &str,
    mut device_name: impl FnMut(&T) -> Option<String>,
) -> Option<T> {
    let needle = name_query.to_lowercase();
    let mut substring_match = None;

    for device in devices {
        let Some(name) = device_name(&device) else {
            continue;
        };

        if name.eq_ignore_ascii_case(name_query) {
            return Some(device);
        }

        if substring_match.is_none() && name.to_lowercase().contains(&needle) {
            substring_match = Some(device);
        }
    }

    substring_match
}

fn run_synthetic_clock(tick_interval: Duration, state: &Arc<AppState>) {
    let poll_interval = Duration::from_millis(MIDI_POLL_INTERVAL_MS);
    let mut ticks_since_step: u8 = 0;

    // A synthetic clock is always running the instant it starts.
    record_byte(MIDI_STATUS_START, state, &mut ticks_since_step);

    let mut next_tick = Instant::now() + tick_interval;
    while state.keep_running.load(Ordering::Acquire) {
        let now = Instant::now();
        if now >= next_tick {
            record_byte(MIDI_STATUS_TIMING_CLOCK, state, &mut ticks_since_step);
            next_tick += tick_interval;
        }
        let sleep_for = next_tick
            .saturating_duration_since(Instant::now())
            .min(poll_interval);
        thread::sleep(sleep_for);
    }
}

fn run_real_device(_connection: midir::MidiInputConnection<u8>, state: &Arc<AppState>) {
    // midir delivers bytes on its own backend thread. Holding the connection
    // keeps that alive. This thread only waits for shutdown, so it parks
    // until the join loop unparks it.
    while state.keep_running.load(Ordering::Acquire) {
        thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_device_listing_preserves_device_indices_and_names() {
        testing_logger::setup();
        let entries = [
            MidiDeviceInfo {
                index: 0,
                name: "MIDI input".to_string(),
            },
            MidiDeviceInfo {
                index: 1,
                name: "Unknown Device".to_string(),
            },
        ];
        MidiListener::list_devices_text(&entries);

        testing_logger::validate(|logs| {
            assert_eq!(logs.len(), entries.len());
            assert_eq!(logs[0].body, "[0] MIDI input");
            assert_eq!(logs[1].body, "[1] Unknown Device");
            assert!(logs.iter().all(|entry| entry.level == log::Level::Info));
        });
    }

    #[test]
    fn empty_text_device_listing_reports_no_devices() {
        testing_logger::setup();
        MidiListener::list_devices_text(&[]);

        testing_logger::validate(|logs| {
            assert_eq!(logs.len(), 1);
            assert_eq!(logs[0].level, log::Level::Warn);
            assert_eq!(logs[0].body, "[*] No MIDI input devices detected.");
        });
    }

    #[test]
    fn connect_midi_device_reports_a_typed_error_for_an_unmatched_name() {
        let Err(error) = connect_midi_device(
            "a-name-no-real-device-will-ever-have",
            Arc::new(AppState::new()),
        ) else {
            panic!("an unmatched MIDI device name must fail");
        };
        // A machine without a MIDI backend cannot enumerate ports, which is also a typed failure.
        assert!(
            matches!(
                error.downcast_ref::<MidiDeviceError>(),
                Some(MidiDeviceError::MidiNoMatch { .. } | MidiDeviceError::MidiUnavailable { .. })
            ),
            "got: {error:#}"
        );
    }

    #[test]
    fn find_matching_midi_device_prefers_an_exact_match_over_an_earlier_substring() {
        const NAME_QUERY: &str = "Phase Control";
        const SUBSTRING_MATCH: &str = "Phase Control Extended";
        const EXACT_MATCH: &str = "Phase Control";

        let selected =
            find_matching_midi_device([SUBSTRING_MATCH, EXACT_MATCH], NAME_QUERY, |device_name| {
                Some((*device_name).to_owned())
            });

        assert_eq!(
            selected,
            Some(EXACT_MATCH),
            "an exact MIDI device name must take precedence over an earlier substring match"
        );
    }

    #[test]
    fn record_byte_sets_start() {
        let state = AppState::new();
        record_byte(0xFA, &state, &mut 0u8);
        assert_eq!(
            state.midi_last_transport.load(Ordering::Acquire),
            MIDI_TRANSPORT_START
        );
    }

    #[test]
    fn record_byte_publishes_start_with_reset_step_count() {
        const PREVIOUS_STEP_COUNT: u32 = 12;

        let state = AppState::new();
        state
            .midi_steps
            .store(PREVIOUS_STEP_COUNT, Ordering::Release);
        MIDI_START_PUBLISHED_OBSERVER.with(|observer| {
            *observer.borrow_mut() = Some(Box::new(|observed_state| {
                assert_eq!(
                    observed_state.midi_steps.load(Ordering::Acquire),
                    0,
                    "MIDI Start must not become observable before its step count has reset"
                );
            }));
        });

        record_byte(MIDI_STATUS_START, &state, &mut 0u8);

        MIDI_START_PUBLISHED_OBSERVER.with(|observer| {
            observer.borrow_mut().take();
        });
    }

    #[test]
    fn record_byte_sets_stop() {
        let state = AppState::new();
        record_byte(0xFC, &state, &mut 0u8);
        assert_eq!(
            state.midi_last_transport.load(Ordering::Acquire),
            MIDI_TRANSPORT_STOP
        );
    }

    #[test]
    fn record_byte_sets_continue() {
        let state = AppState::new();
        record_byte(0xFB, &state, &mut 0u8);
        assert_eq!(
            state.midi_last_transport.load(Ordering::Acquire),
            MIDI_TRANSPORT_CONTINUE
        );
    }

    #[test]
    fn each_transport_event_is_counted_and_kept_as_the_latest() {
        let state = AppState::new();

        for (byte, code) in [
            (MIDI_STATUS_START, MIDI_TRANSPORT_START),
            (MIDI_STATUS_STOP, MIDI_TRANSPORT_STOP),
            (MIDI_STATUS_CONTINUE, MIDI_TRANSPORT_CONTINUE),
        ] {
            let before = state.midi_transport_count.load(Ordering::Acquire);
            record_byte(byte, &state, &mut 0u8);
            assert_eq!(
                state.midi_transport_count.load(Ordering::Acquire),
                before + 1
            );
            assert_eq!(state.midi_transport_latest.load(Ordering::Acquire), code);
        }
    }

    #[test]
    fn timing_clock_bytes_leave_the_transport_count_and_latest_alone() {
        let state = AppState::new();
        let mut ticks_since_step = 0u8;

        for _ in 0..MIDI_CLOCK_TICKS_PER_STEP {
            record_byte(MIDI_STATUS_TIMING_CLOCK, &state, &mut ticks_since_step);
        }

        assert_eq!(state.midi_transport_count.load(Ordering::Acquire), 0);
        assert_eq!(
            state.midi_transport_latest.load(Ordering::Acquire),
            MIDI_TRANSPORT_NONE
        );
    }

    #[test]
    fn record_byte_does_not_publish_before_a_full_step() {
        let state = AppState::new();
        let mut ticks_since_step = 0u8;
        for _ in 0..(MIDI_CLOCK_TICKS_PER_STEP - 1) {
            record_byte(0xF8, &state, &mut ticks_since_step);
        }
        assert_eq!(state.midi_steps.load(Ordering::Acquire), 0);
    }

    #[test]
    fn record_byte_publishes_one_step_after_six_ticks() {
        let state = AppState::new();
        let mut ticks_since_step = 0u8;
        for _ in 0..MIDI_CLOCK_TICKS_PER_STEP {
            record_byte(0xF8, &state, &mut ticks_since_step);
        }
        assert_eq!(state.midi_steps.load(Ordering::Acquire), 1);
    }

    #[test]
    fn record_byte_accumulator_persists_across_calls() {
        let state = AppState::new();
        let mut ticks_since_step = 0u8;
        for _ in 0..4 {
            record_byte(0xF8, &state, &mut ticks_since_step);
        }
        assert_eq!(state.midi_steps.load(Ordering::Acquire), 0);
        for _ in 0..2 {
            record_byte(0xF8, &state, &mut ticks_since_step);
        }
        assert_eq!(
            state.midi_steps.load(Ordering::Acquire),
            1,
            "the four ticks from the first batch should carry over and complete a step with the next two, not reset between calls"
        );
    }

    #[test]
    fn record_byte_start_resets_the_step_accumulator() {
        let state = AppState::new();
        let mut ticks_since_step = 0u8;
        for _ in 0..3 {
            record_byte(0xF8, &state, &mut ticks_since_step);
        }
        record_byte(0xFA, &state, &mut ticks_since_step);
        for _ in 0..3 {
            record_byte(0xF8, &state, &mut ticks_since_step);
        }
        assert_eq!(
            state.midi_steps.load(Ordering::Acquire),
            0,
            "Start should reset the partial accumulator, three ticks before plus three after must not wrongly complete a step"
        );
    }

    #[test]
    fn record_byte_start_resets_the_absolute_step_count() {
        let state = AppState::new();
        let mut ticks_since_step = 0u8;

        // Complete two full steps first.
        for _ in 0..(MIDI_CLOCK_TICKS_PER_STEP * 2) {
            record_byte(0xF8, &state, &mut ticks_since_step);
        }
        assert_eq!(state.midi_steps.load(Ordering::Acquire), 2);

        // A fresh Start resets the published count itself, not just the
        // local tick accumulator.
        record_byte(0xFA, &state, &mut ticks_since_step);
        assert_eq!(state.midi_steps.load(Ordering::Acquire), 0);
    }

    #[test]
    fn record_byte_ignores_unrecognised_bytes() {
        let state = AppState::new();
        for byte in [0x00u8, 0xFE, 0xFF, 0x90] {
            record_byte(byte, &state, &mut 0u8);
        }
        assert_eq!(
            state.midi_last_transport.load(Ordering::Acquire),
            MIDI_TRANSPORT_NONE
        );
        assert_eq!(state.midi_steps.load(Ordering::Acquire), 0);
    }

    #[test]
    fn synthetic_clock_tick_interval_at_120_bpm_matches_hand_calculation() {
        // 60_000ms / (120 * 24) = 20.8333ms per tick.
        let interval = crate::config::midi_tick_interval(120.0)
            .expect("120 bpm should produce a valid tick interval");
        let expected_ms = 20.833_333_333_333_332;
        assert!((interval.as_secs_f64() * 1000.0 - expected_ms).abs() < 1e-6);
    }

    #[test]
    fn synthetic_clock_exits_promptly_when_keep_running_clears() {
        let state = Arc::new(AppState::new());
        let tick_interval = crate::config::midi_tick_interval(120.0)
            .expect("120 bpm should produce a valid tick interval");
        let handle = MidiListener::spawn(
            MidiInputSource::TestClock {
                bpm: 120.0,
                tick_interval,
            },
            Arc::clone(&state),
        );
        assert_eq!(handle.thread().name(), Some("midi-input"));

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
