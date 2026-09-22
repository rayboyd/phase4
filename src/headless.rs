//! Headless supervision contract.
//!
//! Under `--headless` Phase4 runs without a terminal and writes a
//! newline-delimited JSON event stream to stdout, so a supervising host process
//! can tell when the engine is ready, what it resolved, and why it stopped.
//! Logs stay on stderr and stdout carries the event stream alone.
//!
//! A headless run also watches stdin and stops when it reaches end of file, so
//! the engine cannot outlive its host. Bytes arriving on stdin are discarded.
//!
//! This is a control plane. The WebSocket and OSC data planes are unaffected.

use crate::config::AppConfigError;
use crate::managers::audio::DeviceError;
use crate::managers::midi::MidiDeviceError;
use anyhow::Result;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::io::{ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::JoinHandle;

/// Schema version carried by every event line. A host reads this to establish
/// which contract it is supervising against.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// The event code reported when the process panics.
pub const PANIC_EVENT_CODE: &str = "Panic";

/// Name of the thread that watches stdin for end of file.
pub const STDIN_WATCHER_THREAD_NAME: &str = "headless-stdin";

/// Size of the buffer the stdin watcher reads into. Every byte read is discarded.
const STDIN_WATCH_BUFFER_BYTES: usize = 1024;

/// The audio input the engine actually resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadyAudio {
    /// Resolved hardware device name. `None` in calibration mode.
    pub device: Option<String>,

    /// Hardware sample rate in Hz.
    pub sample_rate: u32,

    /// Analysed channel indices in output order. Calibration and an unfiltered
    /// device both report the full set rather than an empty one.
    pub channels: Vec<u16>,
}

/// The transports the engine actually bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadyOutputs {
    /// The WebSocket listener's bound address, resolving a `:0` port to the
    /// real OS-assigned one. `None` when the WebSocket output is not configured.
    pub websocket: Option<SocketAddr>,

    /// The configured OSC target address. `None` when OSC is not configured.
    pub osc: Option<SocketAddr>,
}

/// Facts a host cannot know until the engine has started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadyReport {
    /// The resolved audio input.
    pub audio: ReadyAudio,

    /// Resolved MIDI device name when MIDI input is enabled by device name.
    /// `None` when MIDI is disabled or driven by the synthetic test clock.
    pub midi_device: Option<String>,

    /// The bound and configured output transports.
    pub outputs: ReadyOutputs,
}

/// Why the engine stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    /// SIGINT or SIGTERM was received.
    Signal,

    /// stdin reached end of file, because the host closed it or exited.
    StdinClosed,
}

/// What ended a headless run, decided once per poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessStop {
    /// A shutdown was requested and the run drains cleanly.
    Requested(ShutdownReason),

    /// The engine cleared `keep_running` itself, so the run failed.
    Engine,
}

/// One line of the headless event stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The engine started and is serving its configured transports.
    Ready(ReadyReport),

    /// Startup or the run failed. The code is a stable identifier and the
    /// message is the error's own text.
    Error {
        /// Stable identifier taken from the originating error variant.
        code: String,
        /// The originating error's display text.
        message: String,
    },

    /// The engine drained its workers and is exiting cleanly.
    Shutdown {
        /// What requested the shutdown.
        reason: ShutdownReason,
    },
}

impl Serialize for Event {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Ready(report) => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("v", &EVENT_SCHEMA_VERSION)?;
                map.serialize_entry("event", "ready")?;
                map.serialize_entry("audio", &report.audio)?;
                map.serialize_entry("midi_device", &report.midi_device)?;
                map.serialize_entry("outputs", &report.outputs)?;
                map.end()
            }
            Self::Error { code, message } => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("v", &EVENT_SCHEMA_VERSION)?;
                map.serialize_entry("event", "error")?;
                map.serialize_entry("code", code)?;
                map.serialize_entry("message", message)?;
                map.end()
            }
            Self::Shutdown { reason } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("v", &EVENT_SCHEMA_VERSION)?;
                map.serialize_entry("event", "shutdown")?;
                map.serialize_entry("reason", reason)?;
                map.end()
            }
        }
    }
}

impl Event {
    /// Builds an error event from any error that carries a stable code.
    #[must_use]
    pub fn from_error(error: &(impl EventCode + std::fmt::Display)) -> Self {
        Self::Error {
            code: error.event_code().to_owned(),
            message: error.to_string(),
        }
    }

    /// Builds an error event from an [`anyhow::Error`], recovering the stable
    /// code from the typed error in its chain when one is present. An error
    /// with no typed source reports the code `Unknown`.
    #[must_use]
    pub fn from_anyhow(error: &anyhow::Error) -> Self {
        Self::Error {
            code: error_code(error).to_owned(),
            message: format!("{error:#}"),
        }
    }
}

/// The code reported for an error with no typed source.
pub const UNKNOWN_EVENT_CODE: &str = "Unknown";

/// The stable code of the typed error in `error`'s chain, or `Unknown`.
#[must_use]
pub fn error_code(error: &anyhow::Error) -> &'static str {
    error
        .downcast_ref::<AppConfigError>()
        .map(EventCode::event_code)
        .or_else(|| {
            error
                .downcast_ref::<DeviceError>()
                .map(EventCode::event_code)
        })
        .or_else(|| {
            error
                .downcast_ref::<MidiDeviceError>()
                .map(EventCode::event_code)
        })
        .unwrap_or(UNKNOWN_EVENT_CODE)
}

/// A stable identifier for the `error` event's `code` field.
///
/// Codes are variant names. Adding a variant adds a code. Renaming a variant
/// breaks the headless contract and must be treated as a breaking change.
pub trait EventCode {
    /// The stable code for this value.
    fn event_code(&self) -> &'static str;
}

impl EventCode for AppConfigError {
    fn event_code(&self) -> &'static str {
        match self {
            Self::MissingDevice => "MissingDevice",
            Self::NoOutputConfigured => "NoOutputConfigured",
            Self::NonLoopbackBindAddress(_) => "NonLoopbackBindAddress",
            Self::InvalidAttackTime { .. } => "InvalidAttackTime",
            Self::InvalidReleaseTime { .. } => "InvalidReleaseTime",
            Self::InvalidFreqLow { .. } => "InvalidFreqLow",
            Self::InvalidFreqHigh { .. } => "InvalidFreqHigh",
            Self::InvalidFreqRange { .. } => "InvalidFreqRange",
            Self::InvalidFilterQ { .. } => "InvalidFilterQ",
            Self::InvalidVocoderBandCoefficients { .. } => "InvalidVocoderBandCoefficients",
            Self::InvalidMidiTempo { .. } => "InvalidMidiTempo",
            Self::InvalidTestFrequency { .. } => "InvalidTestFrequency",
            Self::InvalidTestSweepRate { .. } => "InvalidTestSweepRate",
            Self::InvalidMaxClients => "InvalidMaxClients",
            Self::EmptyChannelSelection => "EmptyChannelSelection",
            Self::InvalidMidiDeviceName => "InvalidMidiDeviceName",
            Self::DuplicateOutputTransport { .. } => "DuplicateOutputTransport",
            Self::ChannelIndexOutOfRange { .. } => "ChannelIndexOutOfRange",
            Self::ConfigFileParseError(_) => "ConfigFileParseError",
            Self::ConfigFileNotFound(_) => "ConfigFileNotFound",
            Self::InvalidFreqAboveNyquist { .. } => "InvalidFreqAboveNyquist",
            Self::InvalidFreqAboveSafetyCeiling { .. } => "InvalidFreqAboveSafetyCeiling",
        }
    }
}

impl EventCode for DeviceError {
    fn event_code(&self) -> &'static str {
        match self {
            Self::EmptyQuery => "EmptyQuery",
            Self::NoMatch { .. } => "NoMatch",
            Self::UnsupportedFormat { .. } => "UnsupportedFormat",
            Self::HardwareStreamError { .. } => "HardwareStreamError",
        }
    }
}

impl EventCode for MidiDeviceError {
    fn event_code(&self) -> &'static str {
        match self {
            Self::MidiUnavailable { .. } => "MidiUnavailable",
            Self::MidiNoMatch { .. } => "MidiNoMatch",
            Self::MidiConnectFailed { .. } => "MidiConnectFailed",
        }
    }
}

/// Decides whether a headless run stops. A signal takes precedence over a
/// closed stdin, and both take precedence over a stop raised by the engine.
#[must_use]
pub fn headless_stop(
    signal_received: bool,
    stdin_closed: bool,
    keep_running: bool,
) -> Option<HeadlessStop> {
    if signal_received {
        Some(HeadlessStop::Requested(ShutdownReason::Signal))
    } else if stdin_closed {
        Some(HeadlessStop::Requested(ShutdownReason::StdinClosed))
    } else if keep_running {
        None
    } else {
        Some(HeadlessStop::Engine)
    }
}

/// Spawns a detached thread that reads `reader` until end of file or a read
/// error, discarding every byte, then sets `closed`. An interrupted read is
/// retried.
///
/// A blocking read on stdin cannot be interrupted portably, so the thread is
/// not registered with the worker registry and ends with the process.
///
/// # Errors
///
/// Returns an error if the thread cannot be spawned.
pub fn watch_stdin(
    mut reader: impl Read + Send + 'static,
    closed: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(STDIN_WATCHER_THREAD_NAME.to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; STDIN_WATCH_BUFFER_BYTES];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => {
                        log::warn!("Treating stdin as closed after a read error: {error}");
                        break;
                    }
                }
            }
            closed.store(true, Ordering::Release);
        })
}

/// Reports whether an event write failed because the reader of stdout has gone.
#[must_use]
pub fn is_broken_pipe(error: &anyhow::Error) -> bool {
    let kind = error
        .downcast_ref::<std::io::Error>()
        .map(std::io::Error::kind)
        .or_else(|| {
            error
                .downcast_ref::<serde_json::Error>()
                .and_then(serde_json::Error::io_error_kind)
        });
    kind == Some(ErrorKind::BrokenPipe)
}

/// Writes one event as a single JSON line, then flushes.
///
/// # Errors
///
/// Returns an error if serialisation or the write fails.
pub fn write_event(writer: &mut impl Write, event: &Event) -> Result<()> {
    serde_json::to_writer(&mut *writer, event)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

/// Installs a process-wide panic hook for headless runs.
///
/// There is no terminal to restore, so this writes one `error` event with the
/// code `Panic` before deferring to the previous hook. A failed write is
/// ignored, since the process is already aborting. The previous hook, which
/// prints the panic message and backtrace, is preserved and still runs.
pub fn install_panic_hook() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let event = Event::Error {
            code: PANIC_EVENT_CODE.to_owned(),
            message: info.to_string(),
        };
        let _ = write_event(&mut std::io::stdout().lock(), &event);
        log::error!("{info}");
        previous_hook(info);
    }));
}
