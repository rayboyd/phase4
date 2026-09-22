//! Message types and keys of the link contract.

use super::ffi::DictionaryRef;
use super::CONTRACT_VERSION;
use crate::config::{
    parse_file_config, resolve_with_outputs, AppConfig, AppConfigError, FileConfig,
    FileNetworkConfig, OutputConfig,
};
use crate::frames::FrameRegionSlot;
use crate::headless::EventCode;
use crate::{Args, CalibrationArgs, InputArgs, ListFormat, MidiArgs, NetworkArgs, VocoderArgs};
use std::ffi::CStr;

pub const KEY_VERSION: &CStr = c"v";
pub const KEY_TYPE: &CStr = c"type";
pub const KEY_OK: &CStr = c"ok";
pub const KEY_ERROR: &CStr = c"error";
pub const KEY_CODE: &CStr = c"code";
pub const KEY_MESSAGE: &CStr = c"message";
pub const KEY_ENGINE_VERSION: &CStr = c"engine_version";
pub const KEY_CONTRACT_VERSION: &CStr = c"contract_version";
pub const KEY_BAND_COUNT: &CStr = c"band_count";
pub const KEY_DEVICES: &CStr = c"devices";
pub const KEY_NAME: &CStr = c"name";
pub const KEY_SAMPLE_RATE: &CStr = c"sample_rate";
pub const KEY_CHANNELS: &CStr = c"channels";
pub const KEY_SUPPORTED: &CStr = c"supported";
pub const KEY_SOURCE: &CStr = c"source";
pub const KEY_KIND: &CStr = c"kind";
pub const KEY_DEVICE_NAME: &CStr = c"device_name";
pub const KEY_HZ: &CStr = c"hz";
pub const KEY_RATE_HZ: &CStr = c"rate_hz";
pub const KEY_MIDI: &CStr = c"midi";
pub const KEY_BPM: &CStr = c"bpm";
pub const KEY_CONFIG_YAML: &CStr = c"config_yaml";
pub const KEY_AUDIO_DEVICE: &CStr = c"audio_device";
pub const KEY_MIDI_DEVICE: &CStr = c"midi_device";
pub const KEY_FRAMES: &CStr = c"frames";
pub const KEY_FRAME_BYTES: &CStr = c"frame_bytes";
pub const KEY_REASON: &CStr = c"reason";

pub const TYPE_HELLO: &str = "hello";
pub const TYPE_LIST_AUDIO_DEVICES: &str = "list_audio_devices";
pub const TYPE_LIST_MIDI_DEVICES: &str = "list_midi_devices";
pub const TYPE_START: &str = "start";
pub const TYPE_STOP: &str = "stop";
pub const EVENT_ERROR: &str = "error";
pub const EVENT_STOPPED: &str = "stopped";

pub const SOURCE_DEVICE: &str = "device";
pub const SOURCE_TEST_TONE: &str = "test_tone";
pub const SOURCE_TEST_SWEEP: &str = "test_sweep";
pub const MIDI_DEVICE: &str = "device";
pub const MIDI_TEST_CLOCK: &str = "test_clock";

pub const REASON_REQUESTED: &str = "requested";
pub const REASON_FAILED: &str = "failed";

/// A parsed request.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Hello,
    ListAudioDevices,
    ListMidiDevices,
    Start(StartRequest),
    Stop,
}

/// The fields of a `start` request.
#[derive(Debug, Clone, PartialEq)]
pub struct StartRequest {
    pub source: Source,
    pub midi: Option<MidiRequest>,
    pub config_yaml: Option<String>,
}

/// What to analyse.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Device {
        name: String,
        channels: Option<Vec<u16>>,
    },
    TestTone {
        hz: f32,
    },
    TestSweep {
        rate_hz: f32,
    },
}

/// The MIDI input to attach.
#[derive(Debug, Clone, PartialEq)]
pub enum MidiRequest {
    Device { name: String },
    TestClock { bpm: f32 },
}

/// Failures the link itself reports. Variant names are the contract's codes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("This engine speaks link contract version {supported}, and the request used version {requested}")]
    ContractMismatch { requested: i64, supported: i64 },

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("The engine is already running. Send stop before start.")]
    AlreadyRunning,
}

impl EventCode for LinkError {
    fn event_code(&self) -> &'static str {
        match self {
            Self::ContractMismatch { .. } => "ContractMismatch",
            Self::InvalidRequest(_) => "InvalidRequest",
            Self::AlreadyRunning => "AlreadyRunning",
        }
    }
}

/// An `InvalidRequest` naming the key that was missing or had another type.
fn invalid(key: &CStr) -> LinkError {
    LinkError::InvalidRequest(format!(
        "`{}` is missing or has the wrong type",
        key.to_string_lossy()
    ))
}

/// Parses a request message.
pub(crate) fn parse_request(message: DictionaryRef<'_>) -> Result<Request, LinkError> {
    let version = message
        .get_i64(KEY_VERSION)
        .ok_or_else(|| invalid(KEY_VERSION))?;
    if version != CONTRACT_VERSION {
        return Err(LinkError::ContractMismatch {
            requested: version,
            supported: CONTRACT_VERSION,
        });
    }

    let request_type = message
        .get_string(KEY_TYPE)
        .ok_or_else(|| invalid(KEY_TYPE))?;
    match request_type.as_str() {
        TYPE_HELLO => Ok(Request::Hello),
        TYPE_LIST_AUDIO_DEVICES => Ok(Request::ListAudioDevices),
        TYPE_LIST_MIDI_DEVICES => Ok(Request::ListMidiDevices),
        TYPE_START => parse_start(message).map(Request::Start),
        TYPE_STOP => Ok(Request::Stop),
        other => Err(LinkError::InvalidRequest(format!(
            "`{other}` is not a request type"
        ))),
    }
}

fn parse_start(message: DictionaryRef<'_>) -> Result<StartRequest, LinkError> {
    let source = message
        .get_dictionary(KEY_SOURCE)
        .ok_or_else(|| invalid(KEY_SOURCE))
        .and_then(parse_source)?;

    let midi = if message.contains(KEY_MIDI) {
        let midi = message
            .get_dictionary(KEY_MIDI)
            .ok_or_else(|| invalid(KEY_MIDI))?;
        Some(parse_midi(midi)?)
    } else {
        None
    };

    let config_yaml = if message.contains(KEY_CONFIG_YAML) {
        Some(
            message
                .get_string(KEY_CONFIG_YAML)
                .ok_or_else(|| invalid(KEY_CONFIG_YAML))?,
        )
    } else {
        None
    };

    Ok(StartRequest {
        source,
        midi,
        config_yaml,
    })
}

fn parse_source(source: DictionaryRef<'_>) -> Result<Source, LinkError> {
    let kind = source
        .get_string(KEY_KIND)
        .ok_or_else(|| invalid(KEY_KIND))?;
    match kind.as_str() {
        SOURCE_DEVICE => Ok(Source::Device {
            name: source
                .get_string(KEY_DEVICE_NAME)
                .ok_or_else(|| invalid(KEY_DEVICE_NAME))?,
            channels: parse_channels(source)?,
        }),
        SOURCE_TEST_TONE => Ok(Source::TestTone {
            hz: source.get_f64(KEY_HZ).ok_or_else(|| invalid(KEY_HZ))? as f32,
        }),
        SOURCE_TEST_SWEEP => Ok(Source::TestSweep {
            rate_hz: source
                .get_f64(KEY_RATE_HZ)
                .ok_or_else(|| invalid(KEY_RATE_HZ))? as f32,
        }),
        other => Err(LinkError::InvalidRequest(format!(
            "`{other}` is not a source kind"
        ))),
    }
}

/// Reads `channels` as zero-based hardware channel indices. Absent means
/// every channel.
fn parse_channels(source: DictionaryRef<'_>) -> Result<Option<Vec<u16>>, LinkError> {
    if !source.contains(KEY_CHANNELS) {
        return Ok(None);
    }
    let channels = source
        .get_array(KEY_CHANNELS)
        .ok_or_else(|| invalid(KEY_CHANNELS))?;
    (0..channels.len())
        .map(|index| {
            channels
                .get_i64(index)
                .and_then(|channel| u16::try_from(channel).ok())
                .ok_or_else(|| invalid(KEY_CHANNELS))
        })
        .collect::<Result<Vec<u16>, LinkError>>()
        .map(Some)
}

fn parse_midi(midi: DictionaryRef<'_>) -> Result<MidiRequest, LinkError> {
    let kind = midi.get_string(KEY_KIND).ok_or_else(|| invalid(KEY_KIND))?;
    match kind.as_str() {
        MIDI_DEVICE => Ok(MidiRequest::Device {
            name: midi
                .get_string(KEY_DEVICE_NAME)
                .ok_or_else(|| invalid(KEY_DEVICE_NAME))?,
        }),
        MIDI_TEST_CLOCK => Ok(MidiRequest::TestClock {
            bpm: midi.get_f64(KEY_BPM).ok_or_else(|| invalid(KEY_BPM))? as f32,
        }),
        other => Err(LinkError::InvalidRequest(format!(
            "`{other}` is not a MIDI kind"
        ))),
    }
}

/// Resolves a `start` request into an `AppConfig` whose outputs are exactly
/// one `OutputConfig::FrameRegion` carrying `slot`.
///
/// The request's source and MIDI override the config text's audio and MIDI
/// sections, as the command line overrides a config file, and the config
/// text's network section is ignored.
pub(crate) fn to_app_config(
    start: &StartRequest,
    slot: &FrameRegionSlot,
) -> Result<AppConfig, AppConfigError> {
    let mut file = match &start.config_yaml {
        Some(text) => parse_file_config(text)?,
        None => FileConfig::default(),
    };
    file.network = FileNetworkConfig::default();

    let mut args = Args {
        config: None,
        headless: false,
        calibration: CalibrationArgs {
            test_hz: None,
            test_sweep: None,
            test_midi_clock: None,
        },
        input: InputArgs {
            audio_device: None,
            audio_list: false,
            audio_list_format: ListFormat::Text,
            audio_analyse_channels: None,
        },
        midi: MidiArgs {
            midi_device: None,
            midi_list: false,
            midi_list_format: ListFormat::Text,
        },
        network: NetworkArgs {
            ws_addr: None,
            max_clients: None,
            no_browser_origin: false,
            osc_addr: None,
        },
        vocoder: VocoderArgs {
            attack_ms: None,
            release_ms: None,
            freq_low: None,
            freq_high: None,
            filter_q: None,
        },
    };

    match &start.source {
        Source::Device { name, channels } => {
            args.input.audio_device = Some(name.clone());
            args.input.audio_analyse_channels.clone_from(channels);
        }
        Source::TestTone { hz } => args.calibration.test_hz = Some(*hz),
        Source::TestSweep { rate_hz } => args.calibration.test_sweep = Some(*rate_hz),
    }
    match &start.midi {
        Some(MidiRequest::Device { name }) => args.midi.midi_device = Some(name.clone()),
        Some(MidiRequest::TestClock { bpm }) => args.calibration.test_midi_clock = Some(*bpm),
        None => {}
    }

    resolve_with_outputs(&args, file, vec![OutputConfig::FrameRegion(slot.clone())])
}

#[cfg(test)]
mod tests {
    use super::super::ffi::{ArrayBuilder, DictionaryBuilder, Owned};
    use super::*;
    use crate::config::{ConfigInput, ConfigMidiInput, TestSignal};

    const TONE_HZ: f64 = 440.0;
    const SWEEP_RATE_HZ: f64 = 0.5;
    const CLOCK_BPM: f64 = 120.0;
    const DEVICE: &str = "Duet 3";
    const MIDI_PORT: &str = "Loopback";

    fn message(request_type: &str) -> DictionaryBuilder {
        let mut builder = DictionaryBuilder::new();
        builder
            .set_i64(KEY_VERSION, CONTRACT_VERSION)
            .set_str(KEY_TYPE, request_type);
        builder
    }

    fn dictionary(entries: &[(&CStr, &str)]) -> Owned {
        let mut builder = DictionaryBuilder::new();
        for (key, value) in entries {
            builder.set_str(key, value);
        }
        builder.into_owned()
    }

    fn tone_source() -> Owned {
        let mut source = DictionaryBuilder::new();
        source
            .set_str(KEY_KIND, SOURCE_TEST_TONE)
            .set_f64(KEY_HZ, TONE_HZ);
        source.into_owned()
    }

    fn start_with(source: &Owned) -> DictionaryBuilder {
        let mut builder = message(TYPE_START);
        builder.set_value(KEY_SOURCE, source);
        builder
    }

    fn parse(builder: DictionaryBuilder) -> Result<Request, LinkError> {
        let owned = builder.into_owned();
        parse_request(owned.as_dictionary().expect("a dictionary"))
    }

    fn assert_invalid(builder: DictionaryBuilder) {
        assert!(
            matches!(parse(builder), Err(LinkError::InvalidRequest(_))),
            "the request must be rejected as invalid"
        );
    }

    fn start_request(source: Source) -> StartRequest {
        StartRequest {
            source,
            midi: None,
            config_yaml: None,
        }
    }

    #[test]
    fn requests_without_fields_parse() {
        assert_eq!(parse(message(TYPE_HELLO)), Ok(Request::Hello));
        assert_eq!(
            parse(message(TYPE_LIST_AUDIO_DEVICES)),
            Ok(Request::ListAudioDevices)
        );
        assert_eq!(
            parse(message(TYPE_LIST_MIDI_DEVICES)),
            Ok(Request::ListMidiDevices)
        );
        assert_eq!(parse(message(TYPE_STOP)), Ok(Request::Stop));
    }

    #[test]
    fn a_device_start_parses_with_its_channels_midi_and_config() {
        let mut channels = ArrayBuilder::new();
        channels.push_i64(3).push_i64(1);
        let mut source = DictionaryBuilder::new();
        source
            .set_str(KEY_KIND, SOURCE_DEVICE)
            .set_str(KEY_DEVICE_NAME, DEVICE)
            .set_value(KEY_CHANNELS, &channels.into_owned());
        let midi = dictionary(&[(KEY_KIND, MIDI_DEVICE), (KEY_DEVICE_NAME, MIDI_PORT)]);

        let mut builder = start_with(&source.into_owned());
        builder
            .set_value(KEY_MIDI, &midi)
            .set_str(KEY_CONFIG_YAML, "vocoder: {}");

        assert_eq!(
            parse(builder),
            Ok(Request::Start(StartRequest {
                source: Source::Device {
                    name: DEVICE.to_owned(),
                    channels: Some(vec![3, 1]),
                },
                midi: Some(MidiRequest::Device {
                    name: MIDI_PORT.to_owned()
                }),
                config_yaml: Some("vocoder: {}".to_owned()),
            }))
        );
    }

    #[test]
    fn a_device_start_without_channels_analyses_every_channel() {
        let source = dictionary(&[(KEY_KIND, SOURCE_DEVICE), (KEY_DEVICE_NAME, DEVICE)]);
        assert_eq!(
            parse(start_with(&source)),
            Ok(Request::Start(start_request(Source::Device {
                name: DEVICE.to_owned(),
                channels: None,
            })))
        );
    }

    #[test]
    fn calibration_sources_and_the_test_clock_parse() {
        let mut clock = DictionaryBuilder::new();
        clock
            .set_str(KEY_KIND, MIDI_TEST_CLOCK)
            .set_f64(KEY_BPM, CLOCK_BPM);
        let mut builder = start_with(&tone_source());
        builder.set_value(KEY_MIDI, &clock.into_owned());
        assert_eq!(
            parse(builder),
            Ok(Request::Start(StartRequest {
                source: Source::TestTone { hz: TONE_HZ as f32 },
                midi: Some(MidiRequest::TestClock {
                    bpm: CLOCK_BPM as f32
                }),
                config_yaml: None,
            }))
        );

        let mut sweep = DictionaryBuilder::new();
        sweep
            .set_str(KEY_KIND, SOURCE_TEST_SWEEP)
            .set_f64(KEY_RATE_HZ, SWEEP_RATE_HZ);
        assert_eq!(
            parse(start_with(&sweep.into_owned())),
            Ok(Request::Start(start_request(Source::TestSweep {
                rate_hz: SWEEP_RATE_HZ as f32
            })))
        );
    }

    #[test]
    fn another_contract_version_is_a_mismatch_for_every_request() {
        for request_type in [TYPE_HELLO, TYPE_START] {
            let mut builder = message(request_type);
            builder.set_i64(KEY_VERSION, CONTRACT_VERSION + 1);
            assert_eq!(
                parse(builder),
                Err(LinkError::ContractMismatch {
                    requested: CONTRACT_VERSION + 1,
                    supported: CONTRACT_VERSION,
                })
            );
        }
    }

    #[test]
    fn a_missing_or_mistyped_version_or_type_is_invalid() {
        let mut missing_version = DictionaryBuilder::new();
        missing_version.set_str(KEY_TYPE, TYPE_HELLO);
        assert_invalid(missing_version);

        let mut string_version = message(TYPE_HELLO);
        string_version.set_str(KEY_VERSION, "1");
        assert_invalid(string_version);

        let mut missing_type = DictionaryBuilder::new();
        missing_type.set_i64(KEY_VERSION, CONTRACT_VERSION);
        assert_invalid(missing_type);

        assert_invalid(message("restart"));
    }

    #[test]
    fn a_malformed_source_is_invalid() {
        assert_invalid(message(TYPE_START));

        let mut string_source = message(TYPE_START);
        string_source.set_str(KEY_SOURCE, SOURCE_DEVICE);
        assert_invalid(string_source);

        assert_invalid(start_with(&dictionary(&[(KEY_KIND, "microphone")])));
        assert_invalid(start_with(&dictionary(&[(KEY_KIND, SOURCE_DEVICE)])));
        assert_invalid(start_with(&dictionary(&[
            (KEY_KIND, SOURCE_TEST_TONE),
            (KEY_HZ, "440"),
        ])));
    }

    #[test]
    fn channels_must_be_an_array_of_valid_indices() {
        for bad in [-1_i64, i64::from(u16::MAX) + 1] {
            let mut channels = ArrayBuilder::new();
            channels.push_i64(bad);
            let mut source = DictionaryBuilder::new();
            source
                .set_str(KEY_KIND, SOURCE_DEVICE)
                .set_str(KEY_DEVICE_NAME, DEVICE)
                .set_value(KEY_CHANNELS, &channels.into_owned());
            assert_invalid(start_with(&source.into_owned()));
        }

        let mut strings = ArrayBuilder::new();
        strings.push(&dictionary(&[(KEY_NAME, "left")]));
        let mut source = DictionaryBuilder::new();
        source
            .set_str(KEY_KIND, SOURCE_DEVICE)
            .set_str(KEY_DEVICE_NAME, DEVICE)
            .set_value(KEY_CHANNELS, &strings.into_owned());
        assert_invalid(start_with(&source.into_owned()));

        let mut not_an_array = DictionaryBuilder::new();
        not_an_array
            .set_str(KEY_KIND, SOURCE_DEVICE)
            .set_str(KEY_DEVICE_NAME, DEVICE)
            .set_i64(KEY_CHANNELS, 0);
        assert_invalid(start_with(&not_an_array.into_owned()));
    }

    #[test]
    fn malformed_midi_and_config_are_invalid() {
        let mut string_midi = start_with(&tone_source());
        string_midi.set_str(KEY_MIDI, MIDI_DEVICE);
        assert_invalid(string_midi);

        let mut unknown_midi = start_with(&tone_source());
        unknown_midi.set_value(KEY_MIDI, &dictionary(&[(KEY_KIND, "network")]));
        assert_invalid(unknown_midi);

        let mut number_config = start_with(&tone_source());
        number_config.set_i64(KEY_CONFIG_YAML, 1);
        assert_invalid(number_config);
    }

    #[test]
    fn link_error_codes_are_the_variant_names() {
        assert_eq!(
            LinkError::ContractMismatch {
                requested: 2,
                supported: 1
            }
            .event_code(),
            "ContractMismatch"
        );
        assert_eq!(
            LinkError::InvalidRequest(String::new()).event_code(),
            "InvalidRequest"
        );
        assert_eq!(LinkError::AlreadyRunning.event_code(), "AlreadyRunning");
    }

    #[test]
    fn a_tone_resolves_to_calibration_with_only_the_frame_region() {
        let slot = FrameRegionSlot::new();
        let config = to_app_config(
            &start_request(Source::TestTone { hz: TONE_HZ as f32 }),
            &slot,
        )
        .unwrap();

        assert_eq!(
            config.input,
            ConfigInput::Calibration(TestSignal::FixedTone(TONE_HZ as f32))
        );
        assert_eq!(config.midi_input, None);
        assert_eq!(config.outputs.len(), 1);
        assert_eq!(config.outputs[0], OutputConfig::FrameRegion(slot));
    }

    #[test]
    fn a_device_resolves_with_sorted_channels() {
        let config = to_app_config(
            &start_request(Source::Device {
                name: DEVICE.to_owned(),
                channels: Some(vec![3, 1, 3]),
            }),
            &FrameRegionSlot::new(),
        )
        .unwrap();

        assert_eq!(
            config.input,
            ConfigInput::Device {
                name: DEVICE.to_owned(),
                analyse_channels: Some(vec![1, 3].into_boxed_slice()),
            }
        );
    }

    #[test]
    fn the_request_overrides_the_config_audio_and_midi() {
        let config = to_app_config(
            &StartRequest {
                source: Source::Device {
                    name: DEVICE.to_owned(),
                    channels: None,
                },
                midi: Some(MidiRequest::TestClock {
                    bpm: CLOCK_BPM as f32,
                }),
                config_yaml: Some(
                    "audio:\n  device_name_match: \"Other\"\nmidi:\n  device_name_match: \"Other\"\n"
                        .to_owned(),
                ),
            },
            &FrameRegionSlot::new(),
        )
        .unwrap();

        assert!(matches!(
            config.input,
            ConfigInput::Device { ref name, .. } if name == DEVICE
        ));
        assert_eq!(
            config.midi_input,
            Some(ConfigMidiInput::TestClock(CLOCK_BPM as f32))
        );
    }

    #[test]
    fn the_config_network_section_starts_no_transport() {
        let config = to_app_config(
            &StartRequest {
                source: Source::TestTone { hz: TONE_HZ as f32 },
                midi: None,
                config_yaml: Some(
                    "network:\n  ws_addr: \"127.0.0.1:8889\"\n  osc_addr: \"127.0.0.1:7000\"\n"
                        .to_owned(),
                ),
            },
            &FrameRegionSlot::new(),
        )
        .unwrap();

        assert_eq!(config.outputs.len(), 1);
        assert!(matches!(config.outputs[0], OutputConfig::FrameRegion(_)));
    }

    #[test]
    fn the_config_text_is_validated() {
        let with_config = |text: &str| {
            to_app_config(
                &StartRequest {
                    source: Source::TestTone { hz: TONE_HZ as f32 },
                    midi: None,
                    config_yaml: Some(text.to_owned()),
                },
                &FrameRegionSlot::new(),
            )
        };

        assert!(matches!(
            with_config("vocoder: ["),
            Err(AppConfigError::ConfigFileParseError(_))
        ));
        assert!(matches!(
            with_config("vocoder:\n  attack_ms: 0\n"),
            Err(AppConfigError::InvalidAttackTime { .. })
        ));
    }
}
