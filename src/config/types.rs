use crate::dsp::units::{Hertz, Milliseconds};
use crate::ControllerMode;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use thiserror::Error;

/// Fallback WebSocket client cap used when neither CLI nor `config.yaml` sets one.
pub const DEFAULT_MAX_CLIENTS: usize = 8;

/// Default calibration tone frequency in Hz (concert pitch A4). Used only by
/// `AppConfig::default()`, which `resolve_config` always overwrites.
pub const DEFAULT_TEST_HZ: f32 = 440.0;

/// Fallback broadcast rate in Hz used when neither CLI nor `config.yaml` sets one.
pub(super) const DEFAULT_BROADCAST_RATE_HZ: f32 = 60.0;

/// The synthetic calibration signal, a simple sine wave in one of two modes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TestSignal {
    /// Fixed tone at the given frequency in Hz.
    FixedTone(f32),

    /// Logarithmic frequency sweep driven by a sine LFO at the given rate in Hz.
    Sweep(f32),
}

/// The resolved input intent, built exactly once in `resolve_config`. Replaces
/// the loose `device_name_match`, `test_hz`, and `test_sweep` fields so that
/// hardware mode without a device name is unrepresentable.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigInput {
    /// Synthetic calibration signal, no hardware involved.
    Calibration(TestSignal),

    /// Hardware device resolved by name match (exact first, then substring).
    Device(String),
}

/// The resolved MIDI input intent, independent of the audio `ConfigInput`, both
/// may be active at once.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigMidiInput {
    /// Synthetic test clock at the given tempo, in beats per minute.
    TestClock(f32),

    /// Real MIDI input device resolved by name.
    Device(String),
}

/// One configured output transport, carrying everything that transport needs
/// to spawn. Built exactly once in `resolve_config` from the merged CLI and
/// file configuration.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputConfig {
    /// WebSocket JSON broadcast. `addr` is a listen address, phase4 binds it
    /// and clients connect in.
    WebSocket {
        addr: SocketAddr,
        max_clients: usize,
        no_browser_origin: bool,
    },

    /// OSC UDP messages. `addr` is a target address, phase4 sends to it.
    Osc { addr: SocketAddr },
}

/// The resolved output set, non-empty by construction. Phase4 is a consumer
/// tool, so an output exists only because the user named it, either
/// `--ws-addr` or `--osc-addr` (or their `config.yaml` equivalents), and
/// both may be configured together.
///
/// The spawn loop in `App::new` iterates the collection and spawns one
/// worker per entry, so a `WebSocket` entry and an `Osc` entry both present
/// results in both transports running side by side.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigOutputs(Vec<OutputConfig>);

impl ConfigOutputs {
    /// Builds a `ConfigOutputs` from the given descriptors, rejecting an
    /// empty collection.
    ///
    /// # Errors
    ///
    /// Returns [`AppConfigError::NoOutputConfigured`] if `outputs` is empty.
    pub fn new(outputs: Vec<OutputConfig>) -> Result<Self, AppConfigError> {
        if outputs.is_empty() {
            return Err(AppConfigError::NoOutputConfigured);
        }
        Ok(Self(outputs))
    }
}

impl std::ops::Deref for ConfigOutputs {
    type Target = [OutputConfig];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VocoderConfig {
    pub attack_ms: Milliseconds,
    pub release_ms: Milliseconds,
    pub freq_low: Hertz,
    pub freq_high: Hertz,
    pub filter_q: f32,
}

impl Default for VocoderConfig {
    fn default() -> Self {
        Self {
            attack_ms: Milliseconds(24.0),
            release_ms: Milliseconds(96.0),
            freq_low: Hertz(60.0),
            freq_high: Hertz(6_000.0),
            filter_q: 8.0,
        }
    }
}

#[derive(Error, Debug)]
pub enum AppConfigError {
    #[error("To start the app, run with: --audio-device <ID>")]
    MissingDevice,

    #[error(
        "To start the app, configure at least one output: --ws-addr <ADDR> or --osc-addr <ADDR>"
    )]
    NoOutputConfigured,

    #[error("WebSocket server bind address must be loopback, got {0}")]
    NonLoopbackBindAddress(SocketAddr),

    #[error("Invalid vocoder configuration: attack time must be a finite value greater than 0 ms, got {value}")]
    InvalidAttackTime { value: f32 },

    #[error("Invalid vocoder configuration: release time must be a finite value greater than 0 ms, got {value}")]
    InvalidReleaseTime { value: f32 },

    #[error("Invalid vocoder configuration: low frequency must be greater than 0 Hz, got {value}")]
    InvalidFreqLow { value: f32 },

    #[error(
        "Invalid vocoder configuration: high frequency must be greater than 0 Hz, got {value}"
    )]
    InvalidFreqHigh { value: f32 },

    #[error(
        "Invalid vocoder configuration: high frequency ({freq_high}Hz) must be \
        greater than low frequency ({freq_low}Hz)"
    )]
    InvalidFreqRange { freq_low: f32, freq_high: f32 },

    #[error("Invalid vocoder configuration: filter Q must be greater than 0, got {value}")]
    InvalidFilterQ { value: f32 },

    #[error("Invalid broadcast rate: must be a finite value greater than 0 Hz, got {value}")]
    InvalidBroadcastRate { value: f32 },

    #[error("Invalid MIDI test tempo: must be a finite value greater than 0 bpm, got {value}")]
    InvalidMidiTempo { value: f32 },

    #[error("Invalid max clients: must be greater than 0")]
    InvalidMaxClients,

    #[error("Channel selection must not be empty")]
    EmptyChannelSelection,

    #[error("Selected audio channel index {idx} is unavailable on this {channels}-channel device")]
    ChannelIndexOutOfRange { idx: u16, channels: u16 },

    #[error("Failed to parse config.yaml: {0}")]
    ConfigFileParseError(String),

    #[error(
        "Invalid vocoder configuration: Sample rate {sample_rate}Hz: \
        high frequency must be below Nyquist ({nyquist_hz}Hz), got {freq_high}Hz"
    )]
    InvalidFreqAboveNyquist {
        sample_rate: u32,
        freq_high: f32,
        nyquist_hz: f32,
    },

    #[error(
        "Invalid vocoder configuration: Sample rate {sample_rate}Hz: high frequency \
        must be at or below 0.45 of the sample rate ({max_safe_hz}Hz), got {freq_high}Hz"
    )]
    InvalidFreqAboveSafetyCeiling {
        sample_rate: u32,
        freq_high: f32,
        max_safe_hz: f32,
    },
}

#[derive(Debug)]
pub struct AppConfig {
    /// The resolved, non-empty set of output transports.
    pub outputs: ConfigOutputs,

    /// The resolved input source intent, calibration signal or hardware device.
    pub input: ConfigInput,

    /// The resolved MIDI input intent for transport and clock forwarding.
    pub midi_input: Option<ConfigMidiInput>,

    /// Vocoder filter bank configuration.
    pub vocoder_config: VocoderConfig,

    /// Target WebSocket broadcast rate in Hz. None means no throttle (unlimited rate).
    ///
    /// In production this is always `Some` because `resolve_config` always wraps
    /// the resolved rate in `Some`. The `None` variant is only reachable through
    /// direct struct construction in tests.
    pub broadcast_rate: Option<f32>,

    /// Sorted, deduplicated hardware channel indices for the analyser.
    /// None means forward all channels.
    pub analyse_channels: Option<Box<[u16]>>,

    /// How phase4 waits for a shutdown signal.
    pub controller_mode: ControllerMode,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            // A loopback address with an OS-assigned port. There is no hardcoded
            // application default address any more, WebSocket output is opt-in,
            // so this exists only to give the `Default` trait a valid, constructible
            // value for tests that spread `..AppConfig::default()`.
            outputs: ConfigOutputs::new(vec![OutputConfig::WebSocket {
                addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                max_clients: DEFAULT_MAX_CLIENTS,
                no_browser_origin: false,
            }])
            .expect("a single-element Vec is non-empty"),
            input: ConfigInput::Calibration(TestSignal::FixedTone(DEFAULT_TEST_HZ)),
            midi_input: None,
            vocoder_config: VocoderConfig::default(),
            broadcast_rate: Some(DEFAULT_BROADCAST_RATE_HZ),
            analyse_channels: None,
            controller_mode: ControllerMode::Term,
        }
    }
}

/// Network-layer fields that may be set via `config.yaml`.
///
/// All fields are `Option<T>` so users can omit any subset; absent keys fall
/// back to the hardcoded application defaults.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileNetworkConfig {
    pub ws_addr: Option<SocketAddr>,
    pub max_clients: Option<usize>,
    pub broadcast_rate: Option<f32>,
    pub no_browser_origin: Option<bool>,
    pub osc_addr: Option<SocketAddr>,
}

/// Audio-layer fields that may be set via `config.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileAudioConfig {
    pub device_name_match: Option<String>,
    pub analyse_channels: Option<Vec<u16>>,
}

/// MIDI-layer fields that may be set via `config.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileMidiConfig {
    pub device_name_match: Option<String>,
}

/// Vocoder filter-bank fields that may be set via `config.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileVocoderConfig {
    pub attack_ms: Option<f32>,
    pub release_ms: Option<f32>,
    pub freq_low: Option<f32>,
    pub freq_high: Option<f32>,
    pub filter_q: Option<f32>,
}

/// Mirror of the application's configurable surface, deserialised from
/// `config.yaml`.  Each sub-block is independently optional; a missing file
/// or missing key falls back to the hardcoded application default.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileConfig {
    #[serde(default)]
    pub network: FileNetworkConfig,

    #[serde(default)]
    pub audio: FileAudioConfig,

    #[serde(default)]
    pub midi: FileMidiConfig,

    #[serde(default)]
    pub vocoder: FileVocoderConfig,
}

#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    use crate::{Args, CalibrationArgs, InputArgs, MidiArgs, NetworkArgs};

    /// A fixed loopback address used across tests that need a valid
    /// `--ws-addr` value but do not bind a real socket.
    pub(in crate::config) fn test_ws_addr() -> SocketAddr {
        "127.0.0.1:9000".parse().expect("valid socket address")
    }

    pub(in crate::config) fn args_with_device(device: Option<&str>) -> Args {
        Args {
            input: InputArgs {
                audio_device: device.map(str::to_string),
                audio_list: false,
                audio_list_format: crate::ListFormat::Text,
                audio_analyse_channels: None,
            },
            network: NetworkArgs {
                ws_addr: Some(test_ws_addr()),
                max_clients: Some(DEFAULT_MAX_CLIENTS),
                broadcast_rate: Some(DEFAULT_BROADCAST_RATE_HZ),
                no_browser_origin: false,
                osc_addr: None,
            },
            vocoder: crate::VocoderArgs {
                attack_ms: Some(24.0),
                release_ms: Some(96.0),
                freq_low: Some(60.0),
                freq_high: Some(6_000.0),
                filter_q: Some(8.0),
            },
            calibration: CalibrationArgs {
                test_hz: None,
                test_sweep: None,
                test_midi_clock: None,
            },
            midi: MidiArgs {
                midi_device: None,
                midi_list: false,
                midi_list_format: crate::ListFormat::Text,
            },
            runtime: crate::RuntimeArgs {
                controller_mode: ControllerMode::Term,
            },
        }
    }

    /// Finds the single `WebSocket` output in a resolved config's output set.
    pub(in crate::config) fn websocket_output(config: &AppConfig) -> (SocketAddr, usize, bool) {
        config
            .outputs
            .iter()
            .find_map(|output| match output {
                OutputConfig::WebSocket {
                    addr,
                    max_clients,
                    no_browser_origin,
                } => Some((*addr, *max_clients, *no_browser_origin)),
                OutputConfig::Osc { .. } => None,
            })
            .expect("test config should configure a WebSocket output")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    // The default output set is a single WebSocket transport matching the
    // declared constants.
    #[test]
    fn default_config_matches_constants() {
        let config = AppConfig::default();
        let (_addr, max_clients, no_browser_origin) = websocket_output(&config);
        assert_eq!(max_clients, DEFAULT_MAX_CLIENTS);
        assert!(!no_browser_origin);
        assert_eq!(config.vocoder_config, VocoderConfig::default());
    }

    // The default config uses a 60 Hz broadcast rate.
    #[test]
    #[allow(clippy::float_cmp)]
    fn default_config_has_default_broadcast_rate() {
        let config = AppConfig::default();
        assert_eq!(config.broadcast_rate, Some(60.0));
    }

    // VocoderConfig::default produces the documented defaults.
    #[test]
    #[allow(clippy::float_cmp)]
    fn vocoder_config_default_values() {
        let config = VocoderConfig::default();
        assert_eq!(config.attack_ms, Milliseconds(24.0));
        assert_eq!(config.release_ms, Milliseconds(96.0));
        assert_eq!(config.freq_low, Hertz(60.0));
        assert_eq!(config.freq_high, Hertz(6_000.0));
        assert_eq!(config.filter_q, 8.0);
    }

    // The constructor is the single place emptiness is rejected, independent
    // of the CLI/file merge that normally calls it.
    #[test]
    fn config_outputs_new_rejects_empty_collection() {
        let result = ConfigOutputs::new(Vec::new());
        assert!(matches!(result, Err(AppConfigError::NoOutputConfigured)));
    }
}
