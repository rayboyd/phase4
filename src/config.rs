//! [`AppConfig`] holds the validated settings passed to [`crate::app::App::new`].
//! It is produced by [`TryFrom<&Args>`][AppConfig#impl-TryFrom<&Args>-for-AppConfig],
//! which resolves CLI arguments, optional `config.yaml` file settings, and hardcoded
//! defaults in that order of priority, then enforces that a device name is present
//! whenever the application is not running in calibration mode.

use crate::dsp::units::{Hertz, Milliseconds};
use crate::{Args, ControllerMode};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::Path;
use thiserror::Error;

pub const DEFAULT_ADDR_PATTERN: &str = "127.0.0.1:8889";
pub const DEFAULT_MAX_CLIENTS: usize = 8;
const DEFAULT_BROADCAST_RATE_HZ: f32 = 30.0;

fn default_bind_addr() -> SocketAddr {
    DEFAULT_ADDR_PATTERN
        .parse()
        .expect("DEFAULT_ADDR_PATTERN is a valid socket address")
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
            attack_ms: Milliseconds(30.0),
            release_ms: Milliseconds(60.0),
            freq_low: Hertz(40.0),
            freq_high: Hertz(18_000.0),
            filter_q: 2.0,
        }
    }
}

#[derive(Error, Debug)]
pub enum AppConfigError {
    #[error("To start the app, run with: --device <ID>")]
    MissingDevice,

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
    /// WebSocket server bind address.
    pub addr: SocketAddr,

    /// Maximum number of concurrent WebSocket clients.
    pub max_clients: usize,

    /// Device name query for audio device resolution. None in calibration mode.
    pub device_name_match: Option<String>,

    /// A synthetic sine wave at the given frequency (e.g. 440.0).
    pub test_hz: Option<f32>,

    /// The value is the LFO rate in Hz (e.g. 0.1 for 10s).
    pub test_sweep: Option<f32>,

    /// Vocoder filter bank configuration.
    pub vocoder_config: VocoderConfig,

    /// When true, reject WebSocket clients that send a browser Origin header.
    pub no_browser_origin: bool,

    /// Target WebSocket broadcast rate in Hz. None means no throttle (unlimited rate).
    ///
    /// In production this is always `Some` because `resolve_config` always wraps
    /// the resolved rate in `Some`. The `None` variant is only reachable through
    /// direct struct construction in tests.
    pub broadcast_rate: Option<f32>,

    /// Sorted, deduplicated hardware channel indices for the analyser.
    /// None means forward all channels.
    pub analyse_channels: Option<Box<[u16]>>,

    /// OSC UDP output target address. None disables OSC output.
    pub osc_addr: Option<SocketAddr>,

    /// How phase4 waits for a shutdown signal.
    pub controller_mode: ControllerMode,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            addr: default_bind_addr(),
            max_clients: DEFAULT_MAX_CLIENTS,
            device_name_match: None,
            test_hz: None,
            test_sweep: None,
            vocoder_config: VocoderConfig::default(),
            no_browser_origin: false,
            broadcast_rate: Some(DEFAULT_BROADCAST_RATE_HZ),
            analyse_channels: None,
            osc_addr: None,
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
    pub addr: Option<SocketAddr>,
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
    let app_def = AppConfig::default();
    let voc_def = app_def.vocoder_config;

    // Network
    let addr = args
        .network
        .addr
        .or(file.network.addr)
        .unwrap_or(app_def.addr);

    let max_clients = args
        .network
        .max_clients
        .or(file.network.max_clients)
        .unwrap_or(app_def.max_clients);

    let broadcast_rate = args
        .network
        .broadcast_rate
        .or(file.network.broadcast_rate)
        .unwrap_or(DEFAULT_BROADCAST_RATE_HZ);

    let no_browser_origin = if args.network.no_browser_origin {
        true
    } else {
        file.network
            .no_browser_origin
            .unwrap_or(app_def.no_browser_origin)
    };

    let osc_addr = args.network.osc_addr.or(file.network.osc_addr);

    // Audio
    let raw_device = args.input.device.clone().or(file.audio.device_name_match);
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

    // Device requirement
    let in_calibration =
        args.calibration.test_hz.is_some() || args.calibration.test_sweep.is_some();
    let device_name_match = if in_calibration {
        raw_device
    } else {
        Some(raw_device.ok_or(AppConfigError::MissingDevice)?)
    };

    // Validation
    validate_bind_addr(addr)?;
    validate_vocoder_fields(attack_ms, release_ms, freq_low, freq_high, filter_q)?;

    if !is_strictly_positive(broadcast_rate) {
        return Err(AppConfigError::InvalidBroadcastRate {
            value: broadcast_rate,
        });
    }

    if max_clients == 0 {
        return Err(AppConfigError::InvalidMaxClients);
    }

    Ok(AppConfig {
        addr,
        max_clients,
        device_name_match,
        test_hz: args.calibration.test_hz,
        test_sweep: args.calibration.test_sweep,
        vocoder_config: VocoderConfig {
            attack_ms: Milliseconds(attack_ms),
            release_ms: Milliseconds(release_ms),
            freq_low: Hertz(freq_low),
            freq_high: Hertz(freq_high),
            filter_q,
        },
        no_browser_origin,
        broadcast_rate: Some(broadcast_rate),
        analyse_channels: normalise_channel_selection(raw_channels.as_deref())?,
        osc_addr,
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

    fn args_with_device(device: Option<&str>) -> Args {
        Args {
            input: InputArgs {
                device: device.map(str::to_string),
                list: false,
                list_format: crate::ListFormat::Text,
                analyse_channels: None,
            },
            network: NetworkArgs {
                addr: Some(default_bind_addr()),
                max_clients: Some(DEFAULT_MAX_CLIENTS),
                broadcast_rate: Some(DEFAULT_BROADCAST_RATE_HZ),
                no_browser_origin: false,
                osc_addr: None,
            },
            vocoder: crate::VocoderArgs {
                attack_ms: Some(30.0),
                release_ms: Some(60.0),
                freq_low: Some(40.0),
                freq_high: Some(18_000.0),
                filter_q: Some(2.0),
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
        assert_eq!(config.device_name_match.as_deref(), Some("Focusrite 2i2"));
    }

    // In calibration mode no device is required.
    #[test]
    fn try_from_allows_no_device_in_calibration_mode() {
        let mut args = args_with_device(None);
        args.calibration.test_hz = Some(440.0);
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.device_name_match, None);
        assert_eq!(config.test_hz, Some(440.0));
    }

    // The default addr matches the declared constants.
    #[test]
    fn default_config_matches_constants() {
        let config = AppConfig::default();
        assert_eq!(config.addr, default_bind_addr());
        assert_eq!(config.max_clients, DEFAULT_MAX_CLIENTS);
        assert_eq!(config.vocoder_config, VocoderConfig::default());
    }

    // VocoderConfig::default produces the documented defaults.
    #[test]
    #[allow(clippy::float_cmp)]
    fn vocoder_config_default_values() {
        let config = VocoderConfig::default();
        assert_eq!(config.attack_ms, Milliseconds(30.0));
        assert_eq!(config.release_ms, Milliseconds(60.0));
        assert_eq!(config.freq_low, Hertz(40.0));
        assert_eq!(config.freq_high, Hertz(18_000.0));
        assert_eq!(config.filter_q, 2.0);
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
        args.network.addr = Some("0.0.0.0:8889".parse().unwrap());

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
        args.network.broadcast_rate = Some(60.0);

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(config.broadcast_rate, Some(60.0));
    }

    // The default config uses a 30 Hz broadcast rate.
    #[test]
    #[allow(clippy::float_cmp)]
    fn default_config_has_default_broadcast_rate() {
        let config = AppConfig::default();
        assert_eq!(config.broadcast_rate, Some(30.0));
    }

    // A valid max client count is forwarded into the config.
    #[test]
    fn try_from_forwards_max_clients() {
        let mut args = args_with_device(Some("test"));
        args.network.max_clients = Some(16);

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(config.max_clients, 16);
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
        assert_eq!(config.max_clients, 4);
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
        let mut args = args_with_device(None);
        args.calibration.test_hz = Some(440.0); // calibration mode to skip MissingDevice error

        let file = FileConfig {
            audio: FileAudioConfig {
                device_name_match: Some("Focusrite 2i2".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = resolve_config(&args, file).unwrap();
        assert_eq!(config.device_name_match.as_deref(), Some("Focusrite 2i2"));
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
        assert!(config.no_browser_origin);
    }
}
