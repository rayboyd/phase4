//! [`AppConfig`] holds the validated settings passed to [`crate::app::App::new`].
//! It is produced by [`TryFrom<&Args>`][AppConfig#impl-TryFrom<&Args>-for-AppConfig],
//! which resolves CLI arguments, optional `config.yaml` file settings, and hardcoded
//! defaults in that order of priority, then resolves a single input intent for
//! either calibration mode or hardware mode.

use crate::dsp::units::{Hertz, Milliseconds};
use crate::{Args, ControllerMode};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::Path;
use thiserror::Error;

pub const DEFAULT_MAX_CLIENTS: usize = 8;
const DEFAULT_BROADCAST_RATE_HZ: f32 = 60.0;
/// Default calibration tone frequency in Hz (concert pitch A4). Used only by
/// `AppConfig::default()`, which `resolve_config` always overwrites.
pub const DEFAULT_TEST_HZ: f32 = 440.0;

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
/// `--ws-addr` or `--osc-addr` (or their `config.yaml` equivalents).
///
/// Duplicate variants (e.g. two `WebSocket` entries) are representable and
/// not rejected here. The only consumer is the spawn loop in `App::new`,
/// which iterates the collection anyway and would simply spawn both.
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
    #[error("To start the app, run with: --device <ID>")]
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
    pub vocoder: FileVocoderConfig,
}

impl TryFrom<&Args> for AppConfig {
    type Error = AppConfigError;

    fn try_from(args: &Args) -> Result<Self, Self::Error> {
        let file_opt = load_file_config()?;
        if file_opt.is_some() {
            log::info!("Configuration loaded from config.yaml");
        }
        resolve_config(args, file_opt.unwrap_or_default())
    }
}

/// Attempts to load and deserialise `config.yaml` from the current working
/// directory.  Returns `Ok(None)` if the file does not exist.
fn load_file_config() -> Result<Option<FileConfig>, AppConfigError> {
    let path = Path::new("config.yaml");
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppConfigError::ConfigFileParseError(e.to_string()))?;
    let config: FileConfig = serde_yaml::from_str(&content)
        .map_err(|e| AppConfigError::ConfigFileParseError(e.to_string()))?;
    Ok(Some(config))
}

/// Merges three configuration layers (CLI > file > default) and validates the
/// result.  Separated from `TryFrom` so tests can inject a `FileConfig`
/// without touching the filesystem.
fn resolve_config(args: &Args, file: FileConfig) -> Result<AppConfig, AppConfigError> {
    let voc_def = VocoderConfig::default();

    // Network. Both transports are opt-in, an address arrives only from the
    // CLI or the file layer, there is no hardcoded fallback address.
    let ws_addr = args.network.ws_addr.or(file.network.ws_addr);

    let max_clients = args
        .network
        .max_clients
        .or(file.network.max_clients)
        .unwrap_or(DEFAULT_MAX_CLIENTS);

    let broadcast_rate = args
        .network
        .broadcast_rate
        .or(file.network.broadcast_rate)
        .unwrap_or(DEFAULT_BROADCAST_RATE_HZ);

    let no_browser_origin = if args.network.no_browser_origin {
        true
    } else {
        file.network.no_browser_origin.unwrap_or(false)
    };

    let osc_addr = args.network.osc_addr.or(file.network.osc_addr);

    // Audio
    let raw_device = args
        .input
        .device
        .clone()
        .or(file.audio.device_name_match)
        .filter(|name| !name.trim().is_empty());
    let raw_channels = args
        .input
        .analyse_channels
        .clone()
        .or(file.audio.analyse_channels);

    // Vocoder
    let attack_ms = args
        .vocoder
        .attack_ms
        .or(file.vocoder.attack_ms)
        .unwrap_or(voc_def.attack_ms.0);

    let release_ms = args
        .vocoder
        .release_ms
        .or(file.vocoder.release_ms)
        .unwrap_or(voc_def.release_ms.0);

    let freq_low = args
        .vocoder
        .freq_low
        .or(file.vocoder.freq_low)
        .unwrap_or(voc_def.freq_low.0);

    let freq_high = args
        .vocoder
        .freq_high
        .or(file.vocoder.freq_high)
        .unwrap_or(voc_def.freq_high.0);

    let filter_q = args
        .vocoder
        .filter_q
        .or(file.vocoder.filter_q)
        .unwrap_or(voc_def.filter_q);

    // Input resolution. Calibration flags take priority over a device name,
    // and clap guarantees at most one calibration flag is set.
    let input = if let Some(lfo_rate) = args.calibration.test_sweep {
        ConfigInput::Calibration(TestSignal::Sweep(lfo_rate))
    } else if let Some(hz) = args.calibration.test_hz {
        ConfigInput::Calibration(TestSignal::FixedTone(hz))
    } else {
        ConfigInput::Device(raw_device.ok_or(AppConfigError::MissingDevice)?)
    };

    // Validation
    validate_vocoder_fields(attack_ms, release_ms, freq_low, freq_high, filter_q)?;

    if !is_strictly_positive(broadcast_rate) {
        return Err(AppConfigError::InvalidBroadcastRate {
            value: broadcast_rate,
        });
    }

    // Build the output set. Each transport's settings are validated only when
    // that transport is actually configured, an unused --max-clients or
    // --no-browser-origin flag is meaningless without --ws-addr.
    let mut outputs = Vec::new();

    if let Some(addr) = ws_addr {
        validate_bind_addr(addr)?;

        if max_clients == 0 {
            return Err(AppConfigError::InvalidMaxClients);
        }

        outputs.push(OutputConfig::WebSocket {
            addr,
            max_clients,
            no_browser_origin,
        });
    }

    if let Some(addr) = osc_addr {
        outputs.push(OutputConfig::Osc { addr });
    }

    Ok(AppConfig {
        outputs: ConfigOutputs::new(outputs)?,
        input,
        vocoder_config: VocoderConfig {
            attack_ms: Milliseconds(attack_ms),
            release_ms: Milliseconds(release_ms),
            freq_low: Hertz(freq_low),
            freq_high: Hertz(freq_high),
            filter_q,
        },
        broadcast_rate: Some(broadcast_rate),
        analyse_channels: normalise_channel_selection(raw_channels.as_deref())?,
        controller_mode: args.runtime.controller_mode,
    })
}

/// Deduplicates and sorts a channel index slice, returning `None` when the
/// input is `None` and an error when the slice is present but empty.
fn normalise_channel_selection(
    indices: Option<&[u16]>,
) -> Result<Option<Box<[u16]>>, AppConfigError> {
    let Some(raw) = indices else {
        return Ok(None);
    };

    if raw.is_empty() {
        return Err(AppConfigError::EmptyChannelSelection);
    }

    let mut sorted = raw.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    Ok(Some(sorted.into_boxed_slice()))
}

fn is_strictly_positive(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

fn validate_bind_addr(addr: SocketAddr) -> Result<(), AppConfigError> {
    if addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(AppConfigError::NonLoopbackBindAddress(addr))
    }
}

fn validate_vocoder_fields(
    attack_ms: f32,
    release_ms: f32,
    freq_low: f32,
    freq_high: f32,
    filter_q: f32,
) -> Result<(), AppConfigError> {
    if !is_strictly_positive(attack_ms) {
        return Err(AppConfigError::InvalidAttackTime { value: attack_ms });
    }

    if !is_strictly_positive(release_ms) {
        return Err(AppConfigError::InvalidReleaseTime { value: release_ms });
    }

    if !is_strictly_positive(freq_low) {
        return Err(AppConfigError::InvalidFreqLow { value: freq_low });
    }

    if !is_strictly_positive(freq_high) {
        return Err(AppConfigError::InvalidFreqHigh { value: freq_high });
    }

    if freq_low >= freq_high {
        return Err(AppConfigError::InvalidFreqRange {
            freq_low,
            freq_high,
        });
    }

    if !is_strictly_positive(filter_q) {
        return Err(AppConfigError::InvalidFilterQ { value: filter_q });
    }

    Ok(())
}

pub(crate) fn validate_vocoder_sample_rate(
    freq_high: Hertz,
    sample_rate: u32,
) -> Result<(), AppConfigError> {
    let sample_rate_hz = sample_rate as f32;
    let nyquist_hz = sample_rate_hz / 2.0;
    if freq_high.0 >= nyquist_hz {
        return Err(AppConfigError::InvalidFreqAboveNyquist {
            sample_rate,
            freq_high: freq_high.0,
            nyquist_hz,
        });
    }

    let max_safe_hz = sample_rate_hz * 0.45;
    if freq_high.0 > max_safe_hz {
        return Err(AppConfigError::InvalidFreqAboveSafetyCeiling {
            sample_rate,
            freq_high: freq_high.0,
            max_safe_hz,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CalibrationArgs, InputArgs, NetworkArgs};

    /// A fixed loopback address used across tests that need a valid
    /// `--ws-addr` value but do not bind a real socket.
    fn test_ws_addr() -> SocketAddr {
        "127.0.0.1:9000".parse().expect("valid socket address")
    }

    fn args_with_device(device: Option<&str>) -> Args {
        Args {
            input: InputArgs {
                device: device.map(str::to_string),
                list: false,
                list_format: crate::ListFormat::Text,
                analyse_channels: None,
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
            },
            runtime: crate::RuntimeArgs {
                controller_mode: ControllerMode::Term,
            },
        }
    }

    /// Finds the single `WebSocket` output in a resolved config's output set.
    fn websocket_output(config: &AppConfig) -> (SocketAddr, usize, bool) {
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

    // AppConfig::try_from returns an error when no device is supplied and
    // the app is not in calibration mode.
    #[test]
    fn try_from_requires_device_in_normal_mode() {
        let args = args_with_device(None);
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::MissingDevice)));
    }

    // A device name is accepted and forwarded without modification.
    #[test]
    fn try_from_passes_device_index() {
        let args = args_with_device(Some("Focusrite 2i2"));
        let config = AppConfig::try_from(&args).unwrap();
        assert!(matches!(
            config.input,
            ConfigInput::Device(ref name) if name == "Focusrite 2i2"
        ));
    }

    // In calibration mode no device is required.
    #[test]
    fn try_from_allows_no_device_in_calibration_mode() {
        let mut args = args_with_device(None);
        args.calibration.test_hz = Some(440.0);
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(
            config.input,
            ConfigInput::Calibration(TestSignal::FixedTone(440.0))
        );
    }

    #[test]
    fn try_from_resolves_test_sweep_to_calibration_input() {
        let mut args = args_with_device(None);
        args.calibration.test_sweep = Some(0.1);

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(
            config.input,
            ConfigInput::Calibration(TestSignal::Sweep(0.1))
        );
    }

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

    // Custom vocoder CLI args are forwarded into VocoderConfig.
    #[test]
    #[allow(clippy::float_cmp)]
    fn try_from_forwards_vocoder_args() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.attack_ms = Some(12.0);
        args.vocoder.release_ms = Some(80.0);
        args.vocoder.freq_low = Some(40.0);
        args.vocoder.freq_high = Some(16_000.0);
        args.vocoder.filter_q = Some(4.0);

        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.vocoder_config.attack_ms, Milliseconds(12.0));
        assert_eq!(config.vocoder_config.release_ms, Milliseconds(80.0));
        assert_eq!(config.vocoder_config.freq_low, Hertz(40.0));
        assert_eq!(config.vocoder_config.freq_high, Hertz(16_000.0));
        assert_eq!(config.vocoder_config.filter_q, 4.0);
    }

    // Default vocoder args produce the default VocoderConfig.
    #[test]
    fn try_from_default_vocoder_args_match_default_config() {
        let args = args_with_device(Some("test"));
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.vocoder_config, VocoderConfig::default());
    }

    // Review regression: attack times must remain strictly positive.
    #[test]
    fn try_from_rejects_negative_vocoder_attack_ms() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.attack_ms = Some(-0.1);

        let result = AppConfig::try_from(&args);

        assert!(result.is_err(), "negative attack times should be rejected");
    }

    // Review regression: release times must remain finite.
    #[test]
    fn try_from_rejects_non_finite_vocoder_release_ms() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.release_ms = Some(f32::INFINITY);

        let result = AppConfig::try_from(&args);

        assert!(
            result.is_err(),
            "non-finite release times should be rejected"
        );
    }

    // Review regression: logarithmic band spacing requires strictly positive bounds.
    #[test]
    fn try_from_rejects_non_positive_vocoder_low_frequency() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.freq_low = Some(0.0);

        let result = AppConfig::try_from(&args);

        assert!(
            result.is_err(),
            "non-positive low frequencies should be rejected"
        );
    }

    // Review regression: the high bound must remain above the low bound.
    #[test]
    fn try_from_rejects_vocoder_high_frequency_below_low_frequency() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.freq_low = Some(2_000.0);
        args.vocoder.freq_high = Some(1_000.0);

        let result = AppConfig::try_from(&args);

        assert!(
            result.is_err(),
            "high frequencies below the low bound should be rejected"
        );
    }

    // Review regression: the filter Q must be strictly positive.
    #[test]
    fn try_from_rejects_non_positive_vocoder_filter_q() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.filter_q = Some(0.0);

        let result = AppConfig::try_from(&args);

        assert!(
            result.is_err(),
            "non-positive filter Q values should be rejected"
        );
    }

    // The WebSocket server is intentionally loopback-only unless a later change makes this explicit.
    #[test]
    fn try_from_rejects_non_loopback_bind_address() {
        let mut args = args_with_device(Some("test"));
        args.network.ws_addr = Some("0.0.0.0:8889".parse().unwrap());

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::NonLoopbackBindAddress(_))),
            "non-loopback bind addresses should be rejected"
        );
    }

    // A valid broadcast rate is forwarded into the config.
    #[test]
    #[allow(clippy::float_cmp)]
    fn try_from_forwards_broadcast_rate() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = Some(45.0);

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(config.broadcast_rate, Some(45.0));
    }

    // The default config uses a 60 Hz broadcast rate.
    #[test]
    #[allow(clippy::float_cmp)]
    fn default_config_has_default_broadcast_rate() {
        let config = AppConfig::default();
        assert_eq!(config.broadcast_rate, Some(60.0));
    }

    // A valid max client count is forwarded into the config.
    #[test]
    fn try_from_forwards_max_clients() {
        let mut args = args_with_device(Some("test"));
        args.network.max_clients = Some(16);

        let config = AppConfig::try_from(&args).unwrap();
        let (_addr, max_clients, _no_browser_origin) = websocket_output(&config);

        assert_eq!(max_clients, 16);
    }

    // Zero is not a valid client limit.
    #[test]
    fn try_from_rejects_zero_max_clients() {
        let mut args = args_with_device(Some("test"));
        args.network.max_clients = Some(0);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidMaxClients)),
            "zero max clients should be rejected"
        );
    }

    // Zero is not a valid broadcast rate.
    #[test]
    fn try_from_rejects_zero_broadcast_rate() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = Some(0.0);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidBroadcastRate { .. })),
            "zero broadcast rate should be rejected"
        );
    }

    // Negative values are not valid broadcast rates.
    #[test]
    fn try_from_rejects_negative_broadcast_rate() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = Some(-10.0);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidBroadcastRate { .. })),
            "negative broadcast rates should be rejected"
        );
    }

    // Non-finite values are not valid broadcast rates.
    #[test]
    fn try_from_rejects_infinite_broadcast_rate() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = Some(f32::INFINITY);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidBroadcastRate { .. })),
            "non-finite broadcast rates should be rejected"
        );
    }

    // An empty channel list is rejected because it would produce a silent stream.
    #[test]
    fn try_from_rejects_empty_channel_selection() {
        let mut args = args_with_device(Some("test"));
        args.input.analyse_channels = Some(vec![]);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::EmptyChannelSelection)),
            "empty analyse_channels should be rejected"
        );
    }

    // Duplicate and unsorted indices are normalised to a sorted, deduplicated slice.
    #[test]
    fn try_from_normalises_channel_selection() {
        let mut args = args_with_device(Some("test"));
        args.input.analyse_channels = Some(vec![3, 1, 1, 0]);

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(
            config.analyse_channels.as_deref(),
            Some([0u16, 1, 3].as_slice())
        );
    }

    // Omitting channel flags results in None, preserving the all-channels fast path.
    #[test]
    fn try_from_defaults_channel_selection_to_none() {
        let args = args_with_device(Some("test"));
        let config = AppConfig::try_from(&args).unwrap();
        assert!(config.analyse_channels.is_none());
    }

    // 48 kHz sample rate means Nyquist is 24 kHz
    #[test]
    fn validate_vocoder_sample_rate_rejects_freq_above_nyquist() {
        let result = validate_vocoder_sample_rate(Hertz(25_000.0), 48_000);
        assert!(
            matches!(result, Err(AppConfigError::InvalidFreqAboveNyquist { .. })),
            "frequencies above Nyquist should be rejected"
        );
    }

    // 48 kHz sample rate means the 45 percent safety ceiling is 21.6 kHz
    // 22 kHz is below Nyquist (24 kHz) but above the safety ceiling
    #[test]
    fn validate_vocoder_sample_rate_rejects_freq_above_safety_ceiling() {
        let result = validate_vocoder_sample_rate(Hertz(22_000.0), 48_000);
        assert!(
            matches!(
                result,
                Err(AppConfigError::InvalidFreqAboveSafetyCeiling { .. })
            ),
            "frequencies above the 45 percent safety ceiling should be rejected"
        );
    }

    // 18 kHz is well below the 21.6 kHz safety ceiling for 48 kHz
    #[test]
    fn validate_vocoder_sample_rate_accepts_valid_frequencies() {
        let result = validate_vocoder_sample_rate(Hertz(18_000.0), 48_000);
        assert!(result.is_ok(), "valid frequencies should be accepted");
    }

    // --- Three-tier merge tests (CLI > file > default) ---

    // When the CLI supplies no value for a field the file config value is used.
    #[test]
    #[allow(clippy::float_cmp)]
    fn file_config_broadcast_rate_overrides_default_when_cli_absent() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = None;

        let file = FileConfig {
            network: FileNetworkConfig {
                broadcast_rate: Some(45.0),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = resolve_config(&args, file).unwrap();
        assert_eq!(config.broadcast_rate, Some(45.0));
    }

    // A CLI value takes priority over a file config value.
    #[test]
    #[allow(clippy::float_cmp)]
    fn cli_broadcast_rate_overrides_file_config() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = Some(60.0);

        let file = FileConfig {
            network: FileNetworkConfig {
                broadcast_rate: Some(10.0),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = resolve_config(&args, file).unwrap();
        assert_eq!(config.broadcast_rate, Some(60.0));
    }

    // When neither CLI nor file supplies a value the hardcoded default is used.
    #[test]
    #[allow(clippy::float_cmp)]
    fn default_broadcast_rate_used_when_both_cli_and_file_absent() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = None;

        let config = resolve_config(&args, FileConfig::default()).unwrap();
        assert_eq!(config.broadcast_rate, Some(DEFAULT_BROADCAST_RATE_HZ));
    }

    // File config max_clients is used when the CLI supplies None.
    #[test]
    fn file_config_max_clients_overrides_default_when_cli_absent() {
        let mut args = args_with_device(Some("test"));
        args.network.max_clients = None;

        let file = FileConfig {
            network: FileNetworkConfig {
                max_clients: Some(4),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = resolve_config(&args, file).unwrap();
        let (_addr, max_clients, _no_browser_origin) = websocket_output(&config);
        assert_eq!(max_clients, 4);
    }

    // File config vocoder attack_ms is respected when CLI is absent.
    #[test]
    #[allow(clippy::float_cmp)]
    fn file_config_vocoder_attack_ms_overrides_default_when_cli_absent() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.attack_ms = None;

        let file = FileConfig {
            vocoder: FileVocoderConfig {
                attack_ms: Some(15.0),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = resolve_config(&args, file).unwrap();
        assert_eq!(config.vocoder_config.attack_ms, Milliseconds(15.0));
    }

    // File config device_name_match is used when CLI device is absent.
    #[test]
    fn file_config_device_overrides_none_when_cli_absent() {
        let args = args_with_device(None);

        let file = FileConfig {
            audio: FileAudioConfig {
                device_name_match: Some("Focusrite 2i2".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = resolve_config(&args, file).unwrap();
        assert!(matches!(
            config.input,
            ConfigInput::Device(ref name) if name == "Focusrite 2i2"
        ));
    }

    // An invalid file config value (zero max_clients) is rejected by validation
    // even when it originates from the file layer.
    #[test]
    fn file_config_invalid_max_clients_is_rejected() {
        let mut args = args_with_device(Some("test"));
        args.network.max_clients = None;

        let file = FileConfig {
            network: FileNetworkConfig {
                max_clients: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = resolve_config(&args, file);
        assert!(
            matches!(result, Err(AppConfigError::InvalidMaxClients)),
            "zero max clients from file config should be rejected"
        );
    }

    // no_browser_origin from file config is respected when CLI flag is not set.
    #[test]
    fn file_config_no_browser_origin_overrides_default() {
        let args = args_with_device(Some("test")); // no_browser_origin = false

        let file = FileConfig {
            network: FileNetworkConfig {
                no_browser_origin: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = resolve_config(&args, file).unwrap();
        let (_addr, _max_clients, no_browser_origin) = websocket_output(&config);
        assert!(no_browser_origin);
    }

    // An empty device string is rejected, not treated as a valid query.
    #[test]
    fn try_from_rejects_empty_device_string() {
        let args = args_with_device(Some(""));
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::MissingDevice)));
    }

    // An empty device_name_match from the file layer is rejected identically.
    #[test]
    fn file_config_rejects_empty_device_string() {
        let args = args_with_device(None);
        let file = FileConfig {
            audio: FileAudioConfig {
                device_name_match: Some(String::new()),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = resolve_config(&args, file);
        assert!(matches!(result, Err(AppConfigError::MissingDevice)));
    }

    // --- Output transport resolution ---

    // Neither transport configured is a startup error, not a silent no-op.
    #[test]
    fn try_from_rejects_when_no_output_configured() {
        let mut args = args_with_device(Some("test"));
        args.network.ws_addr = None;
        args.network.osc_addr = None;

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::NoOutputConfigured)),
            "configuring no output should be rejected"
        );
    }

    // OSC alone is a valid output set, WebSocket is no longer mandatory.
    #[test]
    fn try_from_builds_osc_only_output_when_ws_addr_absent() {
        let mut args = args_with_device(Some("test"));
        args.network.ws_addr = None;
        args.network.osc_addr = Some("127.0.0.1:7000".parse().unwrap());

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(config.outputs.len(), 1);
        assert!(matches!(config.outputs[0], OutputConfig::Osc { .. }));
    }

    // Both transports may be configured together.
    #[test]
    fn try_from_builds_both_outputs_when_both_addrs_present() {
        let mut args = args_with_device(Some("test"));
        args.network.osc_addr = Some("127.0.0.1:7000".parse().unwrap());

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(config.outputs.len(), 2);
        assert!(config
            .outputs
            .iter()
            .any(|o| matches!(o, OutputConfig::WebSocket { .. })));
        assert!(config
            .outputs
            .iter()
            .any(|o| matches!(o, OutputConfig::Osc { .. })));
    }

    // The constructor is the single place emptiness is rejected, independent
    // of the CLI/file merge that normally calls it.
    #[test]
    fn config_outputs_new_rejects_empty_collection() {
        let result = ConfigOutputs::new(Vec::new());
        assert!(matches!(result, Err(AppConfigError::NoOutputConfigured)));
    }
}
