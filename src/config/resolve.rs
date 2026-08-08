use super::types::{
    AppConfig, AppConfigError, ConfigInput, ConfigMidiInput, ConfigOutputs, FileConfig,
    FileMidiConfig, OutputConfig, TestSignal, VocoderConfig, DEFAULT_MAX_CLIENTS,
};
use super::validate::{
    midi_tick_interval, validate_bind_addr, validate_max_clients, validate_test_signal,
    validate_vocoder_fields,
};
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

    let input = resolve_input(args, raw_device, raw_channels.as_deref())?;
    let midi_input = resolve_midi_input(args, &file.midi)?;

    // Validation.
    validate_vocoder_fields(attack_ms, release_ms, freq_low, freq_high, filter_q)?;

    // Build the output set. Each transport's settings are validated only when
    // that transport is actually configured, an unused --max-clients or
    // --no-browser-origin flag is meaningless without --ws-addr.
    let mut outputs = Vec::new();

    if let Some(addr) = ws_addr {
        validate_bind_addr(addr)?;

        validate_max_clients(max_clients)?;

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
    })
}

/// Resolves the audio input intent. Calibration flags take priority over a
/// device name, and clap guarantees at most one calibration flag is set.
///
/// The channel selection is validated eagerly in both branches (an empty
/// selection is always a config error), but only the hardware variant
/// carries it. The calibration generator writes a fixed stereo signal, so a
/// selection has nothing to select from and is deliberately dropped with a
/// note rather than silently mis-striding the analyser.
fn resolve_input(
    args: &Args,
    raw_device: Option<String>,
    raw_channels: Option<&[u16]>,
) -> Result<ConfigInput, AppConfigError> {
    let analyse_channels = normalise_channel_selection(raw_channels)?;

    let calibration_signal = if let Some(lfo_rate) = args.calibration.test_sweep {
        let signal = TestSignal::Sweep(lfo_rate);
        validate_test_signal(signal)?;
        Some(signal)
    } else if let Some(frequency) = args.calibration.test_hz {
        let signal = TestSignal::FixedTone(frequency);
        validate_test_signal(signal)?;
        Some(signal)
    } else {
        None
    };

    match calibration_signal {
        Some(signal) => {
            if analyse_channels.is_some() {
                log::warn!(
                    "Ignoring the analyse channel selection. Calibration mode generates \
                     its own signal, all generated channels are analysed"
                );
            }
            Ok(ConfigInput::Calibration(signal))
        }
        None => Ok(ConfigInput::Device {
            name: raw_device.ok_or(AppConfigError::MissingDevice)?,
            analyse_channels,
        }),
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

fn resolve_midi_input(
    args: &Args,
    file_midi: &FileMidiConfig,
) -> Result<Option<ConfigMidiInput>, AppConfigError> {
    if let Some(bpm) = args.calibration.test_midi_clock {
        midi_tick_interval(bpm)?;
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
            ConfigInput::Device { ref name, .. } if name == "Focusrite 2i2"
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
    fn try_from_rejects_non_finite_calibration_values() {
        for non_finite_value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut fixed_tone_args = args_with_device(None);
            fixed_tone_args.calibration.test_hz = Some(non_finite_value);
            assert!(
                AppConfig::try_from(&fixed_tone_args).is_err(),
                "--test-hz should reject {non_finite_value}"
            );

            let mut sweep_args = args_with_device(None);
            sweep_args.calibration.test_sweep = Some(non_finite_value);
            assert!(
                AppConfig::try_from(&sweep_args).is_err(),
                "--test-sweep should reject {non_finite_value}"
            );
        }
    }

    #[test]
    fn try_from_rejects_calibration_values_outside_the_generator_range() {
        for invalid_frequency in [0.0, -1.0, f32::MAX] {
            let mut fixed_tone_args = args_with_device(None);
            fixed_tone_args.calibration.test_hz = Some(invalid_frequency);
            assert!(
                AppConfig::try_from(&fixed_tone_args).is_err(),
                "--test-hz should reject {invalid_frequency}"
            );

            let mut sweep_args = args_with_device(None);
            sweep_args.calibration.test_sweep = Some(invalid_frequency);
            assert!(
                AppConfig::try_from(&sweep_args).is_err(),
                "--test-sweep should reject {invalid_frequency}"
            );
        }
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
    fn try_from_rejects_midi_tempos_with_unrepresentable_tick_intervals() {
        for invalid_tempo in [f32::MIN_POSITIVE, f32::MAX] {
            let mut args = args_with_device(Some("test"));
            args.calibration.test_midi_clock = Some(invalid_tempo);
            assert!(
                AppConfig::try_from(&args).is_err(),
                "--test-midi-clock should reject {invalid_tempo}"
            );
        }
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
    fn try_from_rejects_max_clients_above_the_runtime_limit() {
        let mut args = args_with_device(Some("test"));
        args.network.max_clients = Some(tokio::sync::Semaphore::MAX_PERMITS + 1);
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::InvalidMaxClients)));
    }

    #[test]
    fn try_from_rejects_filter_q_that_produces_non_finite_coefficients() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.filter_q = Some(f32::from_bits(1));
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::InvalidFilterQ { .. })));
    }

    #[test]
    fn try_from_rejects_empty_channel_selection() {
        let mut args = args_with_device(Some("test"));
        args.input.audio_analyse_channels = Some(vec![]);
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::EmptyChannelSelection)));
    }

    /// Extracts the channel selection from a hardware input, panicking on a
    /// calibration input. Tests asserting on the selection only make sense
    /// against the `Device` variant, which is the only place it can live.
    fn device_channels(config: &AppConfig) -> Option<&[u16]> {
        match &config.input {
            ConfigInput::Device {
                analyse_channels, ..
            } => analyse_channels.as_deref(),
            ConfigInput::Calibration(_) => panic!("expected a hardware input"),
        }
    }

    #[test]
    fn try_from_normalises_channel_selection() {
        let mut args = args_with_device(Some("test"));
        args.input.audio_analyse_channels = Some(vec![3, 1, 1, 0]);
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(device_channels(&config), Some([0u16, 1, 3].as_slice()));
    }

    #[test]
    fn try_from_defaults_channel_selection_to_none() {
        let args = args_with_device(Some("test"));
        let config = AppConfig::try_from(&args).unwrap();
        assert!(device_channels(&config).is_none());
    }

    // Calibration mode generates its own signal, so a channel selection has
    // nothing to select from. It is dropped (with a logged note), and the
    // resolved input is a plain calibration variant with no way to carry it.
    #[test]
    fn calibration_mode_drops_channel_selection() {
        let mut args = args_with_device(None);
        args.calibration.test_hz = Some(440.0);
        args.input.audio_analyse_channels = Some(vec![0, 3]);
        let config = AppConfig::try_from(&args).unwrap();
        assert_eq!(
            config.input,
            ConfigInput::Calibration(TestSignal::FixedTone(440.0))
        );
    }

    // An empty selection is a config error in every mode. It is validated
    // eagerly, before the calibration branch discards the selection.
    #[test]
    fn calibration_mode_still_rejects_empty_channel_selection() {
        let mut args = args_with_device(None);
        args.calibration.test_hz = Some(440.0);
        args.input.audio_analyse_channels = Some(vec![]);
        let result = AppConfig::try_from(&args);
        assert!(matches!(result, Err(AppConfigError::EmptyChannelSelection)));
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
            ConfigInput::Device { ref name, .. } if name == "Focusrite 2i2"
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
    fn explicit_config_path_is_loaded() {
        let file = TempConfigFile::new("explicit-load", "network:\n  ws_addr: 127.0.0.1:9001\n");
        let mut args = args_with_device(Some("test"));
        args.config = Some(file.0.clone());
        args.network.ws_addr = None;

        let config = AppConfig::try_from(&args).unwrap();
        let (address, _max_clients, _no_browser_origin) = websocket_output(&config);

        assert_eq!(address, "127.0.0.1:9001".parse().unwrap());
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
    fn explicit_config_path_with_unknown_top_level_key_is_an_error() {
        let file = TempConfigFile::new(
            "unknown-top-level-key",
            "netwrok:\n  ws_addr: 127.0.0.1:8889\n",
        );

        let result = load_file_config(Some(&file.0));

        match result {
            Err(AppConfigError::ConfigFileParseError(message)) => {
                assert!(message.contains("unknown field `netwrok`"));
            }
            other => panic!("unknown top-level key should be rejected, got {other:?}"),
        }
    }

    #[test]
    fn explicit_config_path_with_unknown_nested_key_is_an_error() {
        for (section, unknown_key) in [
            ("network", "ws_adrr"),
            ("audio", "device_name_macth"),
            ("midi", "device_name_macth"),
            ("vocoder", "attack_sm"),
        ] {
            let content = format!("{section}:\n  {unknown_key}: true\n");
            let file = TempConfigFile::new(&format!("unknown-{section}-key"), &content);

            let result = load_file_config(Some(&file.0));

            match result {
                Err(AppConfigError::ConfigFileParseError(message)) => {
                    assert!(
                        message.contains(&format!("unknown field `{unknown_key}`")),
                        "unexpected parse error for {section}: {message}"
                    );
                }
                other => panic!("unknown key in {section} should be rejected, got {other:?}"),
            }
        }
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
