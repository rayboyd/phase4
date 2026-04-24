//! [`AppConfig`] holds the validated settings passed to [`crate::app::App::new`].
//! It is produced by [`TryFrom<&Args>`][AppConfig#impl-TryFrom<&Args>-for-AppConfig],
//! which resolves the CLI arguments and enforces that a device index is present
//! whenever the application is not running in calibration mode.

use crate::{Args, VocoderArgs};
use std::net::SocketAddr;
use std::path::{Component, Path};
use thiserror::Error;

pub const DEFAULT_ADDR_PATTERN: &str = "127.0.0.1:8889";
pub const DEFAULT_FILENAME_PATTERN: &str = "rec_{timestamp}_{sample_rate}hz_{bit_depth}bit.wav";
pub const DEFAULT_MAX_CLIENTS: usize = 8;
pub const RECORDINGS_DIR: &str = "recordings";

fn default_bind_addr() -> SocketAddr {
    DEFAULT_ADDR_PATTERN
        .parse()
        .expect("DEFAULT_ADDR_PATTERN is a valid socket address")
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VocoderConfig {
    pub attack_ms: f32,
    pub release_ms: f32,
    pub freq_low: f32,
    pub freq_high: f32,
    pub filter_q: f32,
}

impl Default for VocoderConfig {
    fn default() -> Self {
        Self {
            attack_ms: 37.8,
            release_ms: 56.7,
            freq_low: 40.0,
            freq_high: 18_000.0,
            filter_q: 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum BitDepth {
    #[value(name = "32")]
    Float32,
    #[default]
    #[value(name = "24")]
    Int24,
    #[value(name = "16")]
    Int16,
}

impl BitDepth {
    // Scaling constants for converting normalised f32 audio ([-1.0, 1.0]) to
    // fixed-point integer samples. Both use the positive maximum of the signed
    // type, which leaves one quantisation level unused at the negative end
    // (e.g. -8_388_607 rather than -8_388_608 for i24). This is the symmetric
    // scaling convention adopted by Core Audio, libsndfile, and most DAWs. It
    // guarantees that +1.0 and -1.0 produce equal magnitudes, keeping a centred
    // sine wave symmetric after quantisation. The lost code sits at roughly
    // -140 dBFS for 24-bit, well below any real converter's noise floor.
    //
    // Rust has no native i24 type, so the 24-bit result is stored as i32 and
    // hound packs it to three bytes on disk when bits_per_sample = 24.
    //
    // https://en.wikipedia.org/wiki/Audio_bit_depth#Fixed-point_numbers
    pub const INT24_MAX: f32 = (1i32 << 23) as f32 - 1.0;
    pub const INT16_MAX: f32 = i16::MAX as f32;
}

impl std::fmt::Display for BitDepth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BitDepth::Float32 => write!(f, "32"),
            BitDepth::Int24 => write!(f, "24"),
            BitDepth::Int16 => write!(f, "16"),
        }
    }
}

#[derive(Error, Debug)]
pub enum AppConfigError {
    #[error("To start the app, run with: --device <ID>")]
    MissingDevice,

    #[error("WebSocket server bind address must be loopback, got {0}")]
    NonLoopbackBindAddress(SocketAddr),

    #[error("Invalid filename pattern: {0}")]
    InvalidFilenamePattern(&'static str),

    #[error("Invalid vocoder configuration: {0}")]
    InvalidVocoderConfig(&'static str),

    #[error("Invalid broadcast rate: {0}")]
    InvalidBroadcastRate(&'static str),

    #[error("Invalid max clients: {0}")]
    InvalidMaxClients(&'static str),

    #[error("Channel selection must not be empty")]
    EmptyChannelSelection,

    #[error("Invalid vocoder configuration: Sample rate {sample_rate} Hz: high frequency must be below Nyquist ({nyquist_hz} Hz), got {freq_high} Hz")]
    InvalidFreqAboveNyquist {
        sample_rate: u32,
        freq_high: f32,
        nyquist_hz: f32,
    },

    #[error("Invalid vocoder configuration: Sample rate {sample_rate} Hz: high frequency must be at or below 0.45 of the sample rate ({max_safe_hz} Hz), got {freq_high} Hz")]
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

    /// Recording bit depth.
    pub bit_depth: BitDepth,

    /// Target device index for the audio stream. None in calibration mode.
    pub device_index: Option<usize>,

    /// Output filename pattern, written inside the fixed recordings directory.
    pub filename_pattern: String,

    /// A synthetic sine wave at the given frequency (e.g. 440.0).
    pub test_hz: Option<f32>,

    /// The value is the LFO rate in Hz (e.g. 0.1 for 10s).
    pub test_sweep: Option<f32>,

    /// Vocoder filter bank configuration.
    pub vocoder_config: VocoderConfig,

    /// When true, reject WebSocket clients that send a browser Origin header.
    pub no_browser_origin: bool,

    /// Target WebSocket broadcast rate in Hz. None means no throttle.
    pub broadcast_rate: Option<f32>,

    /// Sorted, deduplicated hardware channel indices for the analyser.
    /// None means forward all channels.
    pub analyse_channels: Option<Box<[u16]>>,

    /// Sorted, deduplicated hardware channel indices for the recorder.
    /// None means record all channels.
    pub record_channels: Option<Box<[u16]>>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            addr: default_bind_addr(),
            max_clients: DEFAULT_MAX_CLIENTS,
            bit_depth: BitDepth::Int24,
            device_index: None,
            filename_pattern: DEFAULT_FILENAME_PATTERN.to_string(),
            test_hz: None,
            test_sweep: None,
            vocoder_config: VocoderConfig::default(),
            no_browser_origin: false,
            broadcast_rate: None,
            analyse_channels: None,
            record_channels: None,
        }
    }
}

impl TryFrom<&Args> for AppConfig {
    type Error = AppConfigError;

    fn try_from(args: &Args) -> Result<Self, Self::Error> {
        let in_calibration =
            args.calibration.test_hz.is_some() || args.calibration.test_sweep.is_some();
        let device_index = if in_calibration {
            args.input.device
        } else {
            Some(args.input.device.ok_or(AppConfigError::MissingDevice)?)
        };

        validate_bind_addr(args.network.addr)?;
        validate_filename_pattern(&args.recording.filename_pattern)?;
        validate_vocoder_args(&args.vocoder)?;

        if let Some(rate) = args.network.broadcast_rate {
            if !is_strictly_positive(rate) {
                return Err(AppConfigError::InvalidBroadcastRate(
                    "broadcast rate must be a finite value greater than 0 Hz",
                ));
            }
        }

        if args.network.max_clients == 0 {
            return Err(AppConfigError::InvalidMaxClients(
                "max clients must be greater than 0",
            ));
        }

        Ok(Self {
            addr: args.network.addr,
            max_clients: args.network.max_clients,
            bit_depth: args.recording.bit_depth,
            device_index,
            filename_pattern: args.recording.filename_pattern.clone(),
            test_hz: args.calibration.test_hz,
            test_sweep: args.calibration.test_sweep,
            vocoder_config: VocoderConfig {
                attack_ms: args.vocoder.attack_ms,
                release_ms: args.vocoder.release_ms,
                freq_low: args.vocoder.freq_low,
                freq_high: args.vocoder.freq_high,
                filter_q: args.vocoder.filter_q,
            },
            no_browser_origin: args.network.no_browser_origin,
            broadcast_rate: args.network.broadcast_rate,
            analyse_channels: normalise_channel_selection(
                args.recording.analyse_channels.as_deref(),
            )?,
            record_channels: normalise_channel_selection(
                args.recording.record_channels.as_deref(),
            )?,
        })
    }
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

fn validate_filename_pattern(pattern: &str) -> Result<(), AppConfigError> {
    if pattern.is_empty() {
        return Err(AppConfigError::InvalidFilenamePattern(
            "filename pattern must not be empty",
        ));
    }

    if pattern.chars().any(std::path::is_separator) {
        return Err(AppConfigError::InvalidFilenamePattern(
            "filename pattern must be a file name only, not a path",
        ));
    }

    let mut components = Path::new(pattern).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err(AppConfigError::InvalidFilenamePattern(
            "filename pattern must be a file name only, not a path",
        )),
    }
}

fn validate_vocoder_args(vocoder: &VocoderArgs) -> Result<(), AppConfigError> {
    if !is_strictly_positive(vocoder.attack_ms) {
        return Err(AppConfigError::InvalidVocoderConfig(
            "attack time must be a finite value greater than 0 ms",
        ));
    }

    if !is_strictly_positive(vocoder.release_ms) {
        return Err(AppConfigError::InvalidVocoderConfig(
            "release time must be a finite value greater than 0 ms",
        ));
    }

    if !is_strictly_positive(vocoder.freq_low) {
        return Err(AppConfigError::InvalidVocoderConfig(
            "low frequency must be greater than 0 Hz",
        ));
    }

    if !is_strictly_positive(vocoder.freq_high) {
        return Err(AppConfigError::InvalidVocoderConfig(
            "high frequency must be greater than 0 Hz",
        ));
    }

    if vocoder.freq_low >= vocoder.freq_high {
        return Err(AppConfigError::InvalidVocoderConfig(
            "high frequency must be greater than low frequency",
        ));
    }

    if !is_strictly_positive(vocoder.filter_q) {
        return Err(AppConfigError::InvalidVocoderConfig(
            "filter Q must be greater than 0",
        ));
    }

    Ok(())
}

pub(crate) fn validate_vocoder_sample_rate(
    freq_high: f32,
    sample_rate: u32,
) -> Result<(), AppConfigError> {
    let sample_rate_hz = sample_rate as f32;
    let nyquist_hz = sample_rate_hz / 2.0;
    if freq_high >= nyquist_hz {
        return Err(AppConfigError::InvalidFreqAboveNyquist {
            sample_rate,
            freq_high,
            nyquist_hz,
        });
    }

    let max_safe_hz = sample_rate_hz * 0.45;
    if freq_high > max_safe_hz {
        return Err(AppConfigError::InvalidFreqAboveSafetyCeiling {
            sample_rate,
            freq_high,
            max_safe_hz,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CalibrationArgs, InputArgs, NetworkArgs, RecordingArgs};

    fn args_with_device(device: Option<usize>) -> Args {
        Args {
            input: InputArgs {
                device,
                list: false,
            },
            network: NetworkArgs {
                addr: default_bind_addr(),
                max_clients: DEFAULT_MAX_CLIENTS,
                broadcast_rate: None,
                no_browser_origin: false,
            },
            recording: RecordingArgs {
                bit_depth: BitDepth::Int24,
                filename_pattern: DEFAULT_FILENAME_PATTERN.to_string(),
                analyse_channels: None,
                record_channels: None,
            },
            vocoder: VocoderArgs {
                attack_ms: 37.8,
                release_ms: 56.7,
                freq_low: 40.0,
                freq_high: 18_000.0,
                filter_q: 2.0,
            },
            calibration: CalibrationArgs {
                test_hz: None,
                test_sweep: None,
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

    // A device index is accepted and forwarded without modification.
    #[test]
    fn try_from_passes_device_index() {
        let args = args_with_device(Some(2));
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.device_index, Some(2));
    }

    // In calibration mode no device is required.
    #[test]
    fn try_from_allows_no_device_in_calibration_mode() {
        let mut args = args_with_device(None);
        args.calibration.test_hz = Some(440.0);
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.device_index, None);
        assert_eq!(config.test_hz, Some(440.0));
    }

    // The default addr and filename pattern match the declared constants.
    #[test]
    fn default_config_matches_constants() {
        let config = AppConfig::default();
        assert_eq!(config.addr, default_bind_addr());
        assert_eq!(config.max_clients, DEFAULT_MAX_CLIENTS);
        assert_eq!(config.filename_pattern, DEFAULT_FILENAME_PATTERN);
        assert_eq!(config.vocoder_config, VocoderConfig::default());
    }

    // BitDepth::Display produces the correct numeric string for each variant.
    #[test]
    fn bit_depth_display() {
        assert_eq!(BitDepth::Float32.to_string(), "32");
        assert_eq!(BitDepth::Int24.to_string(), "24");
        assert_eq!(BitDepth::Int16.to_string(), "16");
    }

    // VocoderConfig::default produces the documented defaults.
    #[test]
    #[allow(clippy::float_cmp)]
    fn vocoder_config_default_values() {
        let config = VocoderConfig::default();
        assert_eq!(config.attack_ms, 37.8);
        assert_eq!(config.release_ms, 56.7);
        assert_eq!(config.freq_low, 40.0);
        assert_eq!(config.freq_high, 18_000.0);
        assert_eq!(config.filter_q, 2.0);
    }

    // Custom vocoder CLI args are forwarded into VocoderConfig.
    #[test]
    #[allow(clippy::float_cmp)]
    fn try_from_forwards_vocoder_args() {
        let mut args = args_with_device(Some(0));
        args.vocoder.attack_ms = 12.0;
        args.vocoder.release_ms = 80.0;
        args.vocoder.freq_low = 40.0;
        args.vocoder.freq_high = 16_000.0;
        args.vocoder.filter_q = 4.0;

        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.vocoder_config.attack_ms, 12.0);
        assert_eq!(config.vocoder_config.release_ms, 80.0);
        assert_eq!(config.vocoder_config.freq_low, 40.0);
        assert_eq!(config.vocoder_config.freq_high, 16_000.0);
        assert_eq!(config.vocoder_config.filter_q, 4.0);
    }

    // Default vocoder args produce the default VocoderConfig.
    #[test]
    fn try_from_default_vocoder_args_match_default_config() {
        let args = args_with_device(Some(0));
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.vocoder_config, VocoderConfig::default());
    }

    // Review regression: attack times must remain strictly positive.
    #[test]
    fn try_from_rejects_negative_vocoder_attack_ms() {
        let mut args = args_with_device(Some(0));
        args.vocoder.attack_ms = -0.1;

        let result = AppConfig::try_from(&args);

        assert!(result.is_err(), "negative attack times should be rejected");
    }

    // Review regression: release times must remain finite.
    #[test]
    fn try_from_rejects_non_finite_vocoder_release_ms() {
        let mut args = args_with_device(Some(0));
        args.vocoder.release_ms = f32::INFINITY;

        let result = AppConfig::try_from(&args);

        assert!(
            result.is_err(),
            "non-finite release times should be rejected"
        );
    }

    // Review regression: logarithmic band spacing requires strictly positive bounds.
    #[test]
    fn try_from_rejects_non_positive_vocoder_low_frequency() {
        let mut args = args_with_device(Some(0));
        args.vocoder.freq_low = 0.0;

        let result = AppConfig::try_from(&args);

        assert!(
            result.is_err(),
            "non-positive low frequencies should be rejected"
        );
    }

    // Review regression: the high bound must remain above the low bound.
    #[test]
    fn try_from_rejects_vocoder_high_frequency_below_low_frequency() {
        let mut args = args_with_device(Some(0));
        args.vocoder.freq_low = 2_000.0;
        args.vocoder.freq_high = 1_000.0;

        let result = AppConfig::try_from(&args);

        assert!(
            result.is_err(),
            "high frequencies below the low bound should be rejected"
        );
    }

    // Review regression: the filter Q must be strictly positive.
    #[test]
    fn try_from_rejects_non_positive_vocoder_filter_q() {
        let mut args = args_with_device(Some(0));
        args.vocoder.filter_q = 0.0;

        let result = AppConfig::try_from(&args);

        assert!(
            result.is_err(),
            "non-positive filter Q values should be rejected"
        );
    }

    // Filename patterns are filename-only. Paths belong to the fixed recordings directory.
    #[test]
    fn try_from_rejects_filename_pattern_with_path_separator() {
        let mut args = args_with_device(Some(0));
        args.recording.filename_pattern = "nested/rec_{timestamp}.wav".to_string();

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidFilenamePattern(_))),
            "filename patterns with path separators should be rejected"
        );
    }

    // Parent traversal must not be accepted via the filename pattern.
    #[test]
    fn try_from_rejects_parent_relative_filename_pattern() {
        let mut args = args_with_device(Some(0));
        args.recording.filename_pattern = "../rec_{timestamp}.wav".to_string();

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidFilenamePattern(_))),
            "parent-relative filename patterns should be rejected"
        );
    }

    // The WebSocket server is intentionally loopback-only unless a later change makes this explicit.
    #[test]
    fn try_from_rejects_non_loopback_bind_address() {
        let mut args = args_with_device(Some(0));
        args.network.addr = "0.0.0.0:8889".parse().unwrap();

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
        let mut args = args_with_device(Some(0));
        args.network.broadcast_rate = Some(30.0);

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(config.broadcast_rate, Some(30.0));
    }

    // The default config has no broadcast rate limit.
    #[test]
    fn default_config_has_no_broadcast_rate() {
        let config = AppConfig::default();
        assert!(config.broadcast_rate.is_none());
    }

    // A valid max client count is forwarded into the config.
    #[test]
    fn try_from_forwards_max_clients() {
        let mut args = args_with_device(Some(0));
        args.network.max_clients = 16;

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(config.max_clients, 16);
    }

    // Zero is not a valid client limit.
    #[test]
    fn try_from_rejects_zero_max_clients() {
        let mut args = args_with_device(Some(0));
        args.network.max_clients = 0;

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidMaxClients(_))),
            "zero max clients should be rejected"
        );
    }

    // Zero is not a valid broadcast rate.
    #[test]
    fn try_from_rejects_zero_broadcast_rate() {
        let mut args = args_with_device(Some(0));
        args.network.broadcast_rate = Some(0.0);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidBroadcastRate(_))),
            "zero broadcast rate should be rejected"
        );
    }

    // Negative values are not valid broadcast rates.
    #[test]
    fn try_from_rejects_negative_broadcast_rate() {
        let mut args = args_with_device(Some(0));
        args.network.broadcast_rate = Some(-10.0);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidBroadcastRate(_))),
            "negative broadcast rates should be rejected"
        );
    }

    // Non-finite values are not valid broadcast rates.
    #[test]
    fn try_from_rejects_infinite_broadcast_rate() {
        let mut args = args_with_device(Some(0));
        args.network.broadcast_rate = Some(f32::INFINITY);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::InvalidBroadcastRate(_))),
            "non-finite broadcast rates should be rejected"
        );
    }

    // An empty channel list is rejected because it would produce a silent stream.
    #[test]
    fn try_from_rejects_empty_channel_selection() {
        let mut args = args_with_device(Some(0));
        args.recording.analyse_channels = Some(vec![]);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::EmptyChannelSelection)),
            "empty analyse_channels should be rejected"
        );

        let mut args = args_with_device(Some(0));
        args.recording.record_channels = Some(vec![]);

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::EmptyChannelSelection)),
            "empty record_channels should be rejected"
        );
    }

    // Duplicate and unsorted indices are normalised to a sorted, deduplicated slice.
    #[test]
    fn try_from_normalises_channel_selection() {
        let mut args = args_with_device(Some(0));
        args.recording.analyse_channels = Some(vec![3, 1, 1, 0]);
        args.recording.record_channels = Some(vec![2, 0, 2]);

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(
            config.analyse_channels.as_deref(),
            Some([0u16, 1, 3].as_slice())
        );
        assert_eq!(
            config.record_channels.as_deref(),
            Some([0u16, 2].as_slice())
        );
    }

    // Omitting channel flags results in None, preserving the all-channels fast path.
    #[test]
    fn try_from_defaults_channel_selection_to_none() {
        let args = args_with_device(Some(0));
        let config = AppConfig::try_from(&args).unwrap();
        assert!(config.analyse_channels.is_none());
        assert!(config.record_channels.is_none());
    }
}
