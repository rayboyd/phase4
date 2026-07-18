//! Structured machine-readable events emitted on stdout when
//! `--stdout-events json` is passed. See docs/tutorials/wrapper.md for the
//! full contract; this module is the implementation of that contract.
//!
//! [`Emitter`] is a no-op unless constructed with the flag set, so every
//! call site can call it unconditionally and stdout stays silent by default.
//! [`map_config_error`] and [`map_startup_error`] classify the two places
//! startup can fail into the closed [`FatalReason`] enum.

use crate::config::AppConfigError;
use crate::EventFormat;
use serde::Serialize;
use std::io::Write;
use std::net::SocketAddr;

/// Schema version for the event envelope. Additive evolution only, readers
/// ignore unknown fields.
const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Emitted exactly once, after all outputs are bound/started and before
    /// the controller blocks.
    Ready {
        pid: u32,
        ws_addr: Option<SocketAddr>,
        osc_addr: Option<SocketAddr>,
    },

    /// Emitted at most once on startup failure, always followed by a
    /// non-zero exit. Never emitted alongside `Ready`.
    Fatal { reason: FatalReason, detail: String },
}

/// Closed enum of startup failure classes. A reason earns a slot only if the
/// user's fix differs from every other reason; unrecognised failures fall
/// back to `StartupFailed`, which wrapper authors must treat as the default
/// case for any reason they don't otherwise handle.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum FatalReason {
    PortInUse,
    DeviceNotFound,
    DeviceUnsupported,
    InvalidConfig,
    NoOutputConfigured,
    StartupFailed,
}

/// Wraps an [`Event`] with the schema version field for serialisation.
#[derive(Serialize)]
struct Envelope<'a> {
    v: u32,
    #[serde(flatten)]
    event: &'a Event,
}

/// Emits [`Event`]s as NDJSON on stdout, or does nothing when disabled.
///
/// Every call site constructs and calls this unconditionally; the `enabled`
/// check keeps stdout byte-for-byte silent without `--stdout-events`.
pub struct Emitter {
    enabled: bool,
}

impl Emitter {
    /// Constructs an emitter from the CLI flag's value: enabled when a
    /// format was requested, a no-op otherwise.
    #[must_use]
    pub fn new(format: Option<EventFormat>) -> Self {
        Self {
            enabled: format.is_some(),
        }
    }

    /// Serialises `event`, writes it as one line to stdout, and flushes.
    ///
    /// # Panics
    ///
    /// Panics if serialisation or the stdout write fails. Serialisation of
    /// these types cannot fail in practice (no maps, no non-finite floats),
    /// and a stdout write failure here is treated the same as the codebase's
    /// existing `println!` call sites: a programmer/environment error, not a
    /// recoverable condition.
    pub fn emit(&self, event: &Event) {
        if !self.enabled {
            return;
        }

        let envelope = Envelope {
            v: SCHEMA_VERSION,
            event,
        };
        let json = serde_json::to_string(&envelope).expect("event serialisation cannot fail");

        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{json}").expect("failed writing event to stdout");
        stdout.flush().expect("failed flushing stdout");
    }
}

/// Maps every [`AppConfigError`] variant to a [`FatalReason`].
///
/// Exhaustive on purpose (no `_` arm): a new `AppConfigError` variant forces
/// a mapping decision here rather than silently falling back to
/// `startup_failed`.
#[must_use]
pub fn map_config_error(e: &AppConfigError) -> FatalReason {
    match e {
        AppConfigError::MissingDevice => FatalReason::DeviceNotFound,

        AppConfigError::NoOutputConfigured => FatalReason::NoOutputConfigured,

        AppConfigError::NonLoopbackBindAddress(_)
        | AppConfigError::InvalidAttackTime { .. }
        | AppConfigError::InvalidReleaseTime { .. }
        | AppConfigError::InvalidFreqLow { .. }
        | AppConfigError::InvalidFreqHigh { .. }
        | AppConfigError::InvalidFreqRange { .. }
        | AppConfigError::InvalidFilterQ { .. }
        | AppConfigError::InvalidBroadcastRate { .. }
        | AppConfigError::InvalidMidiTempo { .. }
        | AppConfigError::InvalidMaxClients
        | AppConfigError::EmptyChannelSelection
        | AppConfigError::ChannelIndexOutOfRange { .. }
        | AppConfigError::ConfigFileParseError(_)
        | AppConfigError::ConfigFileNotFound(_)
        | AppConfigError::InvalidFreqAboveNyquist { .. }
        | AppConfigError::InvalidFreqAboveSafetyCeiling { .. } => FatalReason::InvalidConfig,
    }
}

/// Distinctive substring shared by every device-resolution failure message
/// in `Input::get_device` (src/managers/audio.rs): the empty-query check,
/// the no-match error, and the non-f32 sample format error. None of these
/// errors are typed, so this is the only handle available to classify them
/// without over-fitting a distinction (not-found vs unsupported) the error
/// type doesn't actually carry.
const DEVICE_RESOLUTION_MARKER: &str = "--audio-list";

/// Classifies an `App::new` startup failure into a [`FatalReason`] by
/// walking its error chain.
///
/// Checks, in order: an `io::Error` with `AddrInUse` anywhere in the chain
/// (a WebSocket bind failure), an `AppConfigError` anywhere in the chain
/// (bootstrap re-validation, e.g. channel index or Nyquist checks), then a
/// device-resolution failure by message marker. Anything else falls back to
/// `startup_failed`, the closed-enum contract's deliberate escape hatch.
#[must_use]
pub fn map_startup_error(e: &anyhow::Error) -> FatalReason {
    for cause in e.chain() {
        if let Some(io_err) = cause.downcast_ref::<std::io::Error>() {
            if io_err.kind() == std::io::ErrorKind::AddrInUse {
                return FatalReason::PortInUse;
            }
        }

        if let Some(config_err) = cause.downcast_ref::<AppConfigError>() {
            return map_config_error(config_err);
        }

        if cause.to_string().contains(DEVICE_RESOLUTION_MARKER) {
            return FatalReason::DeviceNotFound;
        }
    }

    FatalReason::StartupFailed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_serialises_with_both_addresses() {
        let event = Event::Ready {
            pid: 34112,
            ws_addr: Some("127.0.0.1:8889".parse().unwrap()),
            osc_addr: Some("127.0.0.1:7000".parse().unwrap()),
        };
        let envelope = Envelope {
            v: SCHEMA_VERSION,
            event: &event,
        };

        let json = serde_json::to_string(&envelope).unwrap();

        assert_eq!(
            json,
            r#"{"v":1,"event":"ready","pid":34112,"ws_addr":"127.0.0.1:8889","osc_addr":"127.0.0.1:7000"}"#
        );
    }

    #[test]
    fn ready_serialises_with_null_ws_addr_when_osc_only() {
        let event = Event::Ready {
            pid: 1,
            ws_addr: None,
            osc_addr: Some("127.0.0.1:7000".parse().unwrap()),
        };
        let envelope = Envelope {
            v: SCHEMA_VERSION,
            event: &event,
        };

        let json = serde_json::to_string(&envelope).unwrap();

        assert_eq!(
            json,
            r#"{"v":1,"event":"ready","pid":1,"ws_addr":null,"osc_addr":"127.0.0.1:7000"}"#
        );
    }

    #[test]
    fn fatal_serialises_reason_and_detail() {
        let event = Event::Fatal {
            reason: FatalReason::PortInUse,
            detail: "Failed to bind WebSocket server to 127.0.0.1:8889".to_string(),
        };
        let envelope = Envelope {
            v: SCHEMA_VERSION,
            event: &event,
        };

        let json = serde_json::to_string(&envelope).unwrap();

        assert_eq!(
            json,
            r#"{"v":1,"event":"fatal","reason":"port_in_use","detail":"Failed to bind WebSocket server to 127.0.0.1:8889"}"#
        );
    }

    #[test]
    fn emitter_is_silent_when_disabled() {
        // Disabled emit() must not touch stdout at all; there is nothing to
        // assert on stdout directly in a unit test, so this pins the
        // constructor's mapping from `None` to `enabled: false` instead,
        // which is what `emit` branches on.
        let emitter = Emitter::new(None);
        assert!(!emitter.enabled);
    }

    #[test]
    fn emitter_is_enabled_for_json_format() {
        let emitter = Emitter::new(Some(EventFormat::Json));
        assert!(emitter.enabled);
    }

    #[test]
    fn map_config_error_covers_every_variant() {
        let cases: Vec<(AppConfigError, FatalReason)> = vec![
            (AppConfigError::MissingDevice, FatalReason::DeviceNotFound),
            (
                AppConfigError::NoOutputConfigured,
                FatalReason::NoOutputConfigured,
            ),
            (
                AppConfigError::NonLoopbackBindAddress("0.0.0.0:1".parse().unwrap()),
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidAttackTime { value: 0.0 },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidReleaseTime { value: 0.0 },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidFreqLow { value: 0.0 },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidFreqHigh { value: 0.0 },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidFreqRange {
                    freq_low: 1.0,
                    freq_high: 1.0,
                },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidFilterQ { value: 0.0 },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidBroadcastRate { value: 0.0 },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidMidiTempo { value: 0.0 },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidMaxClients,
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::EmptyChannelSelection,
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::ChannelIndexOutOfRange {
                    idx: 5,
                    channels: 2,
                },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::ConfigFileParseError(String::new()),
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::ConfigFileNotFound(String::new()),
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidFreqAboveNyquist {
                    sample_rate: 48_000,
                    freq_high: 25_000.0,
                    nyquist_hz: 24_000.0,
                },
                FatalReason::InvalidConfig,
            ),
            (
                AppConfigError::InvalidFreqAboveSafetyCeiling {
                    sample_rate: 48_000,
                    freq_high: 22_000.0,
                    max_safe_hz: 21_600.0,
                },
                FatalReason::InvalidConfig,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(
                map_config_error(&error),
                expected,
                "unexpected mapping for {error:?}"
            );
        }
    }

    #[test]
    fn map_startup_error_detects_addr_in_use_through_anyhow_context() {
        let io_err = std::io::Error::from(std::io::ErrorKind::AddrInUse);
        let err =
            anyhow::Error::new(io_err).context("Failed to bind WebSocket server to 127.0.0.1:8889");

        assert_eq!(map_startup_error(&err), FatalReason::PortInUse);
    }

    #[test]
    fn map_startup_error_ignores_non_addr_in_use_io_errors() {
        let io_err = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let err = anyhow::Error::new(io_err).context("Failed to bind WebSocket server");

        assert_eq!(map_startup_error(&err), FatalReason::StartupFailed);
    }

    #[test]
    fn map_startup_error_reuses_config_error_mapping_through_the_chain() {
        let err = anyhow::Error::from(AppConfigError::ChannelIndexOutOfRange {
            idx: 5,
            channels: 2,
        });

        assert_eq!(map_startup_error(&err), FatalReason::InvalidConfig);
    }

    #[test]
    fn map_startup_error_detects_device_resolution_failures() {
        let err = anyhow::anyhow!(
            "No input device matched \"Duet 3\". phase4 will not fall back to the \
             system default. Run with --audio-list to see available devices."
        );

        assert_eq!(map_startup_error(&err), FatalReason::DeviceNotFound);
    }

    #[test]
    fn map_startup_error_falls_back_to_startup_failed() {
        let err = anyhow::anyhow!("some unexpected failure with no known marker");

        assert_eq!(map_startup_error(&err), FatalReason::StartupFailed);
    }
}
