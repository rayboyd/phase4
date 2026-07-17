use super::types::{
    AppConfig, AppConfigError, ConfigInput, ConfigMidiInput, ConfigOutputs, FileConfig,
    FileMidiConfig, OutputConfig, TestSignal, VocoderConfig, DEFAULT_BROADCAST_RATE_HZ,
    DEFAULT_MAX_CLIENTS,
};
use super::validate::{is_strictly_positive, validate_bind_addr, validate_vocoder_fields};
use crate::dsp::units::{Hertz, Milliseconds};
use crate::Args;
use std::path::Path;

impl TryFrom<&Args> for AppConfig {
    type Error = AppConfigError;

    fn try_from(args: &Args) -> Result<Self, Self::Error> {
        let file_opt = load_file_config(args.config.as_deref())?;
        resolve_config(args, file_opt.unwrap_or_default())
    }
}

/// Attempts to load and deserialise a configuration file.
///
/// With an explicit path (`--config`), the file must exist; a missing file is
/// an error, an explicitly requested configuration must never be silently
/// ignored. With no explicit path, the optional default `config.yaml` in the
/// current working directory is used, and `Ok(None)` is returned when it does
/// not exist.
fn load_file_config(explicit: Option<&Path>) -> Result<Option<FileConfig>, AppConfigError> {
    let path = explicit.unwrap_or_else(|| Path::new("config.yaml"));
    if !path.exists() {
        if explicit.is_some() {
            return Err(AppConfigError::ConfigFileNotFound(
                path.display().to_string(),
            ));
        }
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppConfigError::ConfigFileParseError(e.to_string()))?;
    let config: FileConfig = serde_yaml::from_str(&content)
        .map_err(|e| AppConfigError::ConfigFileParseError(e.to_string()))?;
    log::info!("Configuration loaded from {}", path.display());
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

    // CLI-only. A presence-style bool flag has no "explicitly false" form, so
    // offering it in config.yaml would break the CLI-overrides-file rule (a
    // file `true` could never be switched off from the command line).
    let no_browser_origin = args.network.no_browser_origin;

    let osc_addr = args.network.osc_addr.or(file.network.osc_addr);

    // Audio.
    let raw_device = args
        .input
        .audio_device
        .clone()
        .or(file.audio.device_name_match)
        .filter(|name| !name.trim().is_empty());
    let raw_channels = args
        .input
        .audio_analyse_channels
        .clone()
        .or(file.audio.analyse_channels);

    // Vocoder.
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

    let midi_input = resolve_midi_input(args, &file.midi)?;

    // Validation.
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
        midi_input,
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

fn resolve_midi_input(
    args: &Args,
    file_midi: &FileMidiConfig,
) -> Result<Option<ConfigMidiInput>, AppConfigError> {
    if let Some(bpm) = args.calibration.test_midi_clock {
        if !is_strictly_positive(bpm) {
            return Err(AppConfigError::InvalidMidiTempo { value: bpm });
        }
        return Ok(Some(ConfigMidiInput::TestClock(bpm)));
    }

    let raw_device = args
        .midi
        .midi_device
        .clone()
        .or(file_midi.device_name_match.clone())
        .filter(|name| !name.trim().is_empty());

    Ok(raw_device.map(ConfigMidiInput::Device))
}

#[cfg(test)]
mod tests {
    use super::super::types::test_support::*;
    use super::super::types::*;
    use super::*;

    #[test]
    fn try_from_requires_device_in_normal_mode() {
        let args = args_with_device(None);
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::MissingDevice)));
    }

    #[test]
    fn try_from_passes_device_index() {
        let args = args_with_device(Some("Focusrite 2i2"));
        let config = AppConfig::try_from(&args).unwrap();
        assert!(matches!(
            config.input,
            ConfigInput::Device(ref name) if name == "Focusrite 2i2"
        ));
    }

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

    #[test]
    fn try_from_resolves_midi_test_clock() {
        let mut args = args_with_device(Some("test"));
        args.calibration.test_midi_clock = Some(120.0);
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.midi_input, Some(ConfigMidiInput::TestClock(120.0)));
    }

    #[test]
    fn try_from_resolves_midi_device() {
        let mut args = args_with_device(Some("test"));
        args.midi.midi_device = Some("Loopback".to_string());
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(
            config.midi_input,
            Some(ConfigMidiInput::Device("Loopback".to_string()))
        );
    }

    #[test]
    fn try_from_rejects_empty_midi_device_string() {
        let mut args = args_with_device(Some("test"));
        args.midi.midi_device = Some(String::new());
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.midi_input, None);
    }

    #[test]
    fn try_from_leaves_midi_input_none_when_absent() {
        let args = args_with_device(Some("test"));
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.midi_input, None);
    }

    #[test]
    fn try_from_rejects_non_positive_midi_tempo() {
        let mut args = args_with_device(Some("test"));
        args.calibration.test_midi_clock = Some(0.0);
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::InvalidMidiTempo { value }) if value == 0.0));
    }

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

    #[test]
    fn try_from_default_vocoder_args_match_default_config() {
        let args = args_with_device(Some("test"));
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.vocoder_config, VocoderConfig::default());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn try_from_forwards_broadcast_rate() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = Some(45.0);
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.broadcast_rate, Some(45.0));
    }

    #[test]
    fn try_from_forwards_max_clients() {
        let mut args = args_with_device(Some("test"));
        args.network.max_clients = Some(16);
        let config = AppConfig::try_from(&args).unwrap();
        let (_addr, max_clients, _no_browser_origin) = websocket_output(&config);
        assert_eq!(max_clients, 16);
    }

    #[test]
    fn try_from_rejects_zero_max_clients() {
        let mut args = args_with_device(Some("test"));
        args.network.max_clients = Some(0);
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::InvalidMaxClients)));
    }

    #[test]
    fn try_from_rejects_zero_broadcast_rate() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = Some(0.0);
        let result = AppConfig::try_from(&args);
        assert!(matches!(
            result,
            Err(AppConfigError::InvalidBroadcastRate { .. })
        ));
    }

    #[test]
    fn try_from_rejects_negative_broadcast_rate() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = Some(-10.0);
        let result = AppConfig::try_from(&args);
        assert!(matches!(
            result,
            Err(AppConfigError::InvalidBroadcastRate { .. })
        ));
    }

    #[test]
    fn try_from_rejects_infinite_broadcast_rate() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = Some(f32::INFINITY);
        let result = AppConfig::try_from(&args);
        assert!(matches!(
            result,
            Err(AppConfigError::InvalidBroadcastRate { .. })
        ));
    }

    #[test]
    fn try_from_rejects_empty_channel_selection() {
        let mut args = args_with_device(Some("test"));
        args.input.audio_analyse_channels = Some(vec![]);
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::EmptyChannelSelection)));
    }

    #[test]
    fn try_from_normalises_channel_selection() {
        let mut args = args_with_device(Some("test"));
        args.input.audio_analyse_channels = Some(vec![3, 1, 1, 0]);
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(
            config.analyse_channels.as_deref(),
            Some([0u16, 1, 3].as_slice())
        );
    }

    #[test]
    fn try_from_defaults_channel_selection_to_none() {
        let args = args_with_device(Some("test"));
        let config = AppConfig::try_from(&args).unwrap();
        assert!(config.analyse_channels.is_none());
    }

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

    #[test]
    #[allow(clippy::float_cmp)]
    fn default_broadcast_rate_used_when_both_cli_and_file_absent() {
        let mut args = args_with_device(Some("test"));
        args.network.broadcast_rate = None;
        let config = resolve_config(&args, FileConfig::default()).unwrap();
        assert_eq!(config.broadcast_rate, Some(DEFAULT_BROADCAST_RATE_HZ));
    }

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
        assert!(matches!(result, Err(AppConfigError::InvalidMaxClients)));
    }

    #[test]
    fn cli_no_browser_origin_flag_is_forwarded() {
        let mut args = args_with_device(Some("test"));
        args.network.no_browser_origin = true;
        let config = resolve_config(&args, FileConfig::default()).unwrap();
        let (_addr, _max_clients, no_browser_origin) = websocket_output(&config);
        assert!(no_browser_origin);
    }

    #[test]
    fn try_from_rejects_empty_device_string() {
        let args = args_with_device(Some(""));
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::MissingDevice)));
    }

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

    #[test]
    fn file_config_midi_device_used_when_cli_absent() {
        let args = args_with_device(Some("test"));
        let file = FileConfig {
            midi: FileMidiConfig {
                device_name_match: Some("Loopback".to_string()),
            },
            ..Default::default()
        };
        let config = resolve_config(&args, file).unwrap();
        assert!(matches!(
            config.midi_input,
            Some(ConfigMidiInput::Device(ref name)) if name == "Loopback"
        ));
    }

    #[test]
    fn cli_midi_device_overrides_file_config() {
        let mut args = args_with_device(Some("test"));
        args.midi.midi_device = Some("Hardware Port".to_string());
        let file = FileConfig {
            midi: FileMidiConfig {
                device_name_match: Some("Loopback".to_string()),
            },
            ..Default::default()
        };
        let config = resolve_config(&args, file).unwrap();
        assert!(matches!(
            config.midi_input,
            Some(ConfigMidiInput::Device(ref name)) if name == "Hardware Port"
        ));
    }

    #[test]
    fn test_midi_clock_overrides_file_config_midi_device() {
        let mut args = args_with_device(Some("test"));
        args.calibration.test_midi_clock = Some(120.0);
        let file = FileConfig {
            midi: FileMidiConfig {
                device_name_match: Some("Loopback".to_string()),
            },
            ..Default::default()
        };
        let config = resolve_config(&args, file).unwrap();
        assert!(
            matches!(config.midi_input, Some(ConfigMidiInput::TestClock(bpm)) if (bpm - 120.0).abs() < f32::EPSILON),
            "the synthetic clock must win over a file-configured device, not error or silently prefer the device"
        );
    }

    #[test]
    fn empty_file_config_midi_device_resolves_to_none() {
        let args = args_with_device(Some("test"));
        let file = FileConfig {
            midi: FileMidiConfig {
                device_name_match: Some(String::new()),
            },
            ..Default::default()
        };
        let config = resolve_config(&args, file).unwrap();
        assert_eq!(config.midi_input, None);
    }

    /// Temp file that removes itself on drop, so a failing assertion cannot
    /// leak files between test runs.
    struct TempConfigFile(std::path::PathBuf);

    impl TempConfigFile {
        fn new(name: &str, content: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("phase4-test-{name}-{}.yaml", std::process::id()));
            std::fs::write(&path, content).expect("temp config file should be writable");
            Self(path)
        }
    }

    impl Drop for TempConfigFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn explicit_config_path_is_loaded() {
        let file = TempConfigFile::new("explicit-load", "network:\n  broadcast_rate: 45.0\n");
        let mut args = args_with_device(Some("test"));
        args.config = Some(file.0.clone());
        args.network.broadcast_rate = None;

        let config = AppConfig::try_from(&args).unwrap();

        assert_eq!(config.broadcast_rate, Some(45.0));
    }

    #[test]
    fn explicit_config_path_that_does_not_exist_is_an_error() {
        let mut args = args_with_device(Some("test"));
        args.config = Some(std::path::PathBuf::from(
            "a-path-no-real-machine-will-have.yaml",
        ));

        let result = AppConfig::try_from(&args);

        assert!(
            matches!(result, Err(AppConfigError::ConfigFileNotFound(_))),
            "an explicitly requested config file must never be silently ignored"
        );
    }

    #[test]
    fn explicit_config_path_with_invalid_yaml_is_an_error() {
        let file = TempConfigFile::new("invalid-yaml", "network: [not a mapping");
        let mut args = args_with_device(Some("test"));
        args.config = Some(file.0.clone());

        let result = AppConfig::try_from(&args);

        assert!(matches!(
            result,
            Err(AppConfigError::ConfigFileParseError(_))
        ));
    }

    #[test]
    fn try_from_rejects_when_no_output_configured() {
        let mut args = args_with_device(Some("test"));
        args.network.ws_addr = None;
        args.network.osc_addr = None;
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::NoOutputConfigured)));
    }

    #[test]
    fn try_from_builds_osc_only_output_when_ws_addr_absent() {
        let mut args = args_with_device(Some("test"));
        args.network.ws_addr = None;
        args.network.osc_addr = Some("127.0.0.1:7000".parse().unwrap());
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(config.outputs.len(), 1);
        assert!(matches!(config.outputs[0], OutputConfig::Osc { .. }));
    }

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
}
