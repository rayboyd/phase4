//! Resolves an [`AppConfig`] into running workers and shared state.
//!
//! [`bootstrap`] queries hardware, validates configuration, sizes the ring
//! buffers and starts the configured workers. [`App::new`](crate::app::App::new)
//! assembles the application from the returned state and worker handles.

use crate::app::AppState;
use crate::config::{
    midi_tick_interval, validate_app_config, validate_vocoder_sample_rate, AppConfig,
    AppConfigError, ConfigInput, ConfigMidiInput, ConfigOutputs, OutputConfig, TestSignal,
    CALIBRATION_SAMPLE_RATE_HZ,
};
use crate::dsp::vocoder::bandpass_coefficients;
use crate::dsp::{DisplayPayload, RawPayload};
#[cfg(target_os = "macos")]
use crate::frames::FrameRegion;
use crate::headless::ReadyAudio;
use crate::managers::audio::{ChannelMode, StreamSink};
#[cfg(target_os = "macos")]
use crate::managers::FrameWriter;
use crate::managers::{
    Generator, Input, Mapper, MidiInputSource, MidiListener, OscSender, Processor, Server, Specs,
};
use crate::worker::{WorkerKind, WorkerThreads};
#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::Result;
use cpal::traits::DeviceTrait;
use std::net::SocketAddr;
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::watch;

/// Safety buffer for the analyse ringbuf, headroom for analysis accumulation.
const ANALYSE_BUFFER_MS: u32 = 500;

/// Builds the calibration mode announcement for the given test signal.
fn calibration_announcement(signal: TestSignal) -> String {
    match signal {
        TestSignal::FixedTone(hz) => format!("Calibration mode: fixed tone at {hz} Hz"),
        TestSignal::Sweep(rate) => format!("Calibration mode: sweep at {rate} Hz LFO rate"),
    }
}

/// The input source for the audio pipeline. Either a real hardware device or a
/// synthetic calibration generator. Resolved once in `bootstrap` from `AppConfig::input`.
enum InputSource {
    Calibration(TestSignal),
    Hardware(cpal::Device, cpal::SupportedStreamConfig),
}

/// Everything `App::new` needs to finish construction once configuration has
/// been resolved and every worker thread spawned.
pub(crate) struct Bootstrapped {
    /// Owns the input stream. Dropping it stops audio capture.
    pub(crate) input_device: Input,

    /// Shared atomic flags for cross-thread coordination.
    pub(crate) state: Arc<AppState>,

    /// All worker threads owned by the application runtime.
    pub(crate) workers: WorkerThreads,

    /// The WebSocket listener's actually bound address, obtained from
    /// `local_addr()` rather than the configured one, so a `:0` port
    /// resolves to the real OS-assigned port. `None` when the WebSocket
    /// output is not configured.
    pub(crate) ws_bound_addr: Option<SocketAddr>,

    /// The resolved audio input, reported by the headless `ready` event.
    pub(crate) audio: ReadyAudio,
}

/// Resolves the given configuration into hardware handles, shared state, and
/// running worker threads.
///
/// # Errors
///
/// Returns an error if an audio or MIDI device cannot be opened, the audio
/// input stream cannot be started, or a configured output transport cannot
/// bind to its given address.
pub(crate) fn bootstrap(config: &AppConfig) -> Result<Bootstrapped> {
    validate_app_config(config)?;

    let state = Arc::new(AppState::new());
    let stream_state = Arc::clone(&state);
    let analyser_state = Arc::clone(&state);
    let mapper_state = Arc::clone(&state);
    let generator_state = Arc::clone(&state);
    let mut input_device = Input::new();

    // `analyse_channels` only exists on the hardware input variant, so
    // calibration mode structurally cannot carry a channel selection into
    // the analyser (the generator always writes every hardware channel).
    let resolved = resolve_audio_hardware(config, &mut input_device)?;
    let (hw_specs, input_source) = (resolved.hw_specs, resolved.source);
    let audio = ReadyAudio {
        device: match &input_source {
            InputSource::Hardware(device, _) => {
                device.description().ok().map(|d| d.name().to_string())
            }
            InputSource::Calibration(_) => None,
        },
        sample_rate: hw_specs.sample_rate,
        channels: resolved
            .analyse_channels
            .as_deref()
            .map_or_else(|| (0..hw_specs.channels).collect(), <[u16]>::to_vec),
    };
    let midi_source = resolve_midi_hardware(config, &state)?;
    let midi_enabled = midi_source.is_some();

    // Validate. Must happen before ChannelMode::resolve below, which
    // takes the channel selection by value.
    validate_vocoder_sample_rate(config.vocoder_config.freq_high, hw_specs.sample_rate)?;
    bandpass_coefficients(hw_specs.sample_rate, &config.vocoder_config)?;
    validate_channel_selection(resolved.analyse_channels.as_deref(), hw_specs)?;

    let mut analyser_specs = hw_specs;
    let analyse_mode = ChannelMode::resolve(resolved.analyse_channels, &mut analyser_specs);

    let (analyse_tx, analyse_rx) =
        Input::create_audio_buffer_pair(analyser_specs, ANALYSE_BUFFER_MS);
    let display_channels = analyser_specs.channels as usize;
    let (raw_tx, raw_rx) = watch::channel(RawPayload::new(display_channels));
    let (display_tx, display_rx) = watch::channel(DisplayPayload::new(display_channels));

    let mut workers = WorkerThreads::default();
    let generator_thread = spawn_audio_input(
        input_source,
        hw_specs,
        analyse_mode,
        analyse_tx,
        generator_state,
        &stream_state,
        &mut input_device,
    )?;

    if let Some(handle) = generator_thread {
        workers.register(WorkerKind::Generator, handle);
    }

    let analyser = Processor::new(config.vocoder_config);
    workers.register(
        WorkerKind::Analyser,
        analyser.spawn(analyse_rx, raw_tx, analyser_specs, analyser_state),
    );

    // The frame writer reads analysis snapshots directly, so it takes its own
    // receiver before the mapper consumes this one.
    #[cfg(target_os = "macos")]
    let frame_raw_rx = raw_rx.clone();

    workers.register(
        WorkerKind::Mapper,
        Mapper::spawn(
            raw_rx,
            display_tx,
            display_channels,
            mapper_state,
            midi_enabled,
        ),
    );

    if let Some(source) = midi_source {
        workers.register(
            WorkerKind::MidiInput,
            spawn_midi_input(source, state.clone()),
        );
    }

    // Retain each output handle as it starts so a later output failure can
    // shut down every worker through the normal bounded join path.
    let sources = OutputSources {
        display_rx: &display_rx,
        #[cfg(target_os = "macos")]
        raw_rx: &frame_raw_rx,
        display_channels,
        #[cfg(target_os = "macos")]
        sample_rate: analyser_specs.sample_rate,
        state: &state,
        midi_enabled,
    };
    let ws_bound_addr = match spawn_outputs(&config.outputs, &sources, &mut workers) {
        Ok(bound_addr) => bound_addr,
        Err(error) => {
            drop(input_device);
            state.keep_running.store(false, Ordering::Release);
            workers.shutdown();
            return Err(error);
        }
    };

    Ok(Bootstrapped {
        input_device,
        state,
        workers,
        ws_bound_addr,
        audio,
    })
}

/// The pipeline handles and resolved facts every output transport spawns from.
struct OutputSources<'a> {
    /// The mapper's 60 Hz display snapshots, for the network outputs.
    display_rx: &'a watch::Receiver<DisplayPayload>,

    /// The analyser's snapshots, for the frame region.
    #[cfg(target_os = "macos")]
    raw_rx: &'a watch::Receiver<RawPayload>,

    /// Analysed channels, which every snapshot carries.
    display_channels: usize,

    /// The resolved sample rate in Hz, written into the frame region header.
    #[cfg(target_os = "macos")]
    sample_rate: u32,

    /// Shared runtime state.
    state: &'a Arc<AppState>,

    /// Whether MIDI input is configured.
    midi_enabled: bool,
}

/// Spawns one worker thread per configured output transport, matching each
/// [`OutputConfig`] descriptor to its spawn call.
///
/// Returns the WebSocket listener's actually bound address (`None` if no
/// WebSocket output is configured). Each started output is registered with
/// `workers` immediately so the caller retains ownership if a later output
/// fails to start.
///
/// # Errors
///
/// Returns an error if a transport fails to bind (WebSocket listener), fails
/// to acquire its local socket (OSC sender), or cannot map or share its frame
/// region.
fn spawn_outputs(
    outputs: &ConfigOutputs,
    sources: &OutputSources<'_>,
    workers: &mut WorkerThreads,
) -> Result<Option<SocketAddr>> {
    let mut ws_bound_addr = None;

    for output in outputs.iter() {
        match output {
            OutputConfig::WebSocket {
                addr,
                max_clients,
                no_browser_origin,
            } => {
                let server = Server::new(*addr, *no_browser_origin, *max_clients);
                let (bound_addr, handle) =
                    server.spawn(sources.display_rx.clone(), Arc::clone(sources.state))?;
                log::info!("WebSocket server listening on ws://{bound_addr}");
                ws_bound_addr = Some(bound_addr);
                workers.register(WorkerKind::WebSocket, handle);
            }
            OutputConfig::Osc { addr } => {
                let sender = OscSender::new(*addr);
                let handle = sender.spawn(
                    sources.display_rx.clone(),
                    sources.display_channels,
                    Arc::clone(sources.state),
                    sources.midi_enabled,
                )?;
                log::info!("OSC sender transmitting to udp://{addr}");
                workers.register(WorkerKind::Osc, handle);
            }
            #[cfg(target_os = "macos")]
            OutputConfig::FrameRegion(slot) => {
                let region = Arc::new(
                    FrameRegion::new(
                        sources.display_channels,
                        sources.sample_rate,
                        sources.midi_enabled,
                    )
                    .context("Failed to map the frame region")?,
                );
                if !slot.fill(Arc::clone(&region)) {
                    anyhow::bail!("The frame region slot was already filled");
                }
                let handle =
                    FrameWriter::spawn(sources.raw_rx.clone(), region, Arc::clone(sources.state));
                log::info!(
                    "Frame region publishing {} channels",
                    sources.display_channels
                );
                workers.register(WorkerKind::FrameWriter, handle);
            }
        }
    }

    Ok(ws_bound_addr)
}

/// Validates that all channel indices are within the hardware's capacity.
/// Calibration mode never has a selection (`ConfigInput::Calibration`
/// cannot carry one), so `None` passes trivially.
///
/// # Errors
///
/// Returns an error if a requested channel index is at or beyond the
/// resolved hardware's channel count.
fn validate_channel_selection(selection: Option<&[u16]>, hw_specs: Specs) -> Result<()> {
    if let Some(&idx) = selection.map(<[u16]>::iter).and_then(Iterator::max) {
        if idx >= hw_specs.channels {
            anyhow::bail!(AppConfigError::ChannelIndexOutOfRange {
                idx,
                channels: hw_specs.channels,
            });
        }
    }
    Ok(())
}

/// Spawns the audio producer side of the pipeline, either a synthetic
/// [`Generator`] thread in calibration mode or a real hardware input
/// stream started in place on `input_device`.
///
/// # Errors
///
/// Returns an error if the hardware input stream cannot be started.
fn spawn_audio_input(
    input_source: InputSource,
    hw_specs: Specs,
    analyse_mode: ChannelMode,
    analyse_tx: ringbuf::HeapProd<f32>,
    generator_state: Arc<AppState>,
    stream_state: &Arc<AppState>,
    input_device: &mut Input,
) -> Result<Option<std::thread::JoinHandle<()>>> {
    match input_source {
        InputSource::Calibration(signal) => {
            log::info!("{}", calibration_announcement(signal));
            Ok(Some(Generator::spawn(
                signal,
                hw_specs,
                analyse_tx,
                generator_state,
            )))
        }
        InputSource::Hardware(device, stream_config) => {
            input_device.start_stream(
                &device,
                &stream_config,
                StreamSink {
                    tx: analyse_tx,
                    mode: analyse_mode,
                },
                stream_state,
            )?;
            Ok(None)
        }
    }
}

/// The fully resolved audio input, carrying hardware specs, the input
/// source, and the analyser channel selection. `analyse_channels` is `None` in calibration
/// mode by construction, [`ConfigInput::Calibration`] has no field to carry
/// one.
struct ResolvedInput {
    hw_specs: Specs,
    source: InputSource,
    analyse_channels: Option<Box<[u16]>>,
}

/// Returns the resolved audio input, either calibration-mode defaults or a
/// real device handle.
///
/// # Errors
///
/// Returns an error if the device cannot be resolved or queried.
fn resolve_audio_hardware(config: &AppConfig, input: &mut Input) -> Result<ResolvedInput> {
    match &config.input {
        ConfigInput::Calibration(signal) => Ok(ResolvedInput {
            hw_specs: Specs {
                sample_rate: CALIBRATION_SAMPLE_RATE_HZ,
                channels: 2,
            },
            source: InputSource::Calibration(*signal),
            analyse_channels: None,
        }),
        ConfigInput::Device {
            name,
            analyse_channels,
        } => {
            let (device, stream_config, specs) = input.get_device(name)?;
            Ok(ResolvedInput {
                hw_specs: specs,
                source: InputSource::Hardware(device, stream_config),
                analyse_channels: analyse_channels.clone(),
            })
        }
    }
}

/// Returns a resolved MIDI input source, if MIDI input is configured. Real
/// devices are connected here before any worker thread is spawned.
///
/// # Errors
///
/// Returns an error if a configured MIDI device does not match an available
/// port or the selected port cannot be opened.
fn resolve_midi_hardware(
    config: &AppConfig,
    state: &Arc<AppState>,
) -> Result<Option<MidiInputSource>> {
    resolve_midi_hardware_with(config, state, crate::managers::midi::connect_midi_device)
}

fn resolve_midi_hardware_with(
    config: &AppConfig,
    state: &Arc<AppState>,
    connect_hardware: impl FnOnce(&str, Arc<AppState>) -> Result<MidiInputSource>,
) -> Result<Option<MidiInputSource>> {
    match &config.midi_input {
        None => Ok(None),
        Some(ConfigMidiInput::TestClock(bpm)) => Ok(Some(MidiInputSource::TestClock {
            bpm: *bpm,
            tick_interval: midi_tick_interval(*bpm)?,
        })),
        Some(ConfigMidiInput::Device(name)) => connect_hardware(name, Arc::clone(state)).map(Some),
    }
}

/// Spawns the MIDI listener thread for an already-resolved source,
/// announcing calibration mode synchronously first, matching
/// `spawn_audio_input`'s calibration announcement.
fn spawn_midi_input(source: MidiInputSource, state: Arc<AppState>) -> std::thread::JoinHandle<()> {
    if let MidiInputSource::TestClock { bpm, .. } = &source {
        log::info!("Calibration mode: MIDI test clock at {bpm} bpm");
    }
    MidiListener::spawn(source, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigInput, ConfigMidiInput, TestSignal};

    #[test]
    fn calibration_announcement_describes_fixed_tone() {
        assert_eq!(
            calibration_announcement(TestSignal::FixedTone(440.0)),
            "Calibration mode: fixed tone at 440 Hz"
        );
    }

    #[test]
    fn calibration_announcement_describes_sweep() {
        assert_eq!(
            calibration_announcement(TestSignal::Sweep(0.1)),
            "Calibration mode: sweep at 0.1 Hz LFO rate"
        );
    }

    #[test]
    fn resolve_audio_hardware_in_calibration_mode_returns_defaults() {
        let config = AppConfig {
            input: ConfigInput::Calibration(TestSignal::FixedTone(440.0)),
            ..AppConfig::default()
        };
        let mut input = Input::new();

        let resolved = resolve_audio_hardware(&config, &mut input)
            .expect("resolve_audio_hardware should succeed in calibration mode");

        assert_eq!(resolved.hw_specs.sample_rate, CALIBRATION_SAMPLE_RATE_HZ);
        assert_eq!(resolved.hw_specs.channels, 2);
        assert!(matches!(
            resolved.source,
            InputSource::Calibration(TestSignal::FixedTone(hz)) if (hz - 440.0).abs() < f32::EPSILON
        ));
        assert!(
            resolved.analyse_channels.is_none(),
            "calibration mode can never carry a channel selection"
        );
    }

    #[test]
    fn validate_channel_selection_accepts_none() {
        let hw_specs = Specs {
            sample_rate: 44100,
            channels: 2,
        };

        assert!(validate_channel_selection(None, hw_specs).is_ok());
    }

    #[test]
    fn validate_channel_selection_rejects_out_of_range_index() {
        let hw_specs = Specs {
            sample_rate: 44100,
            channels: 2,
        };

        let result = validate_channel_selection(Some(&[0, 2]), hw_specs);
        assert!(
            result.is_err(),
            "index 2 must be rejected on 2-channel hardware"
        );
    }

    #[test]
    fn validate_channel_selection_accepts_in_range_indices() {
        let hw_specs = Specs {
            sample_rate: 44100,
            channels: 6,
        };

        assert!(validate_channel_selection(Some(&[0, 3, 5]), hw_specs).is_ok());
    }

    #[test]
    fn resolve_midi_hardware_returns_none_when_not_configured() {
        let config = AppConfig::default();
        let state = Arc::new(AppState::new());
        let result = resolve_midi_hardware(&config, &state).expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_midi_hardware_resolves_test_clock() {
        let config = AppConfig {
            midi_input: Some(ConfigMidiInput::TestClock(120.0)),
            ..AppConfig::default()
        };
        let state = Arc::new(AppState::new());
        let result = resolve_midi_hardware(&config, &state)
            .expect("should not error")
            .expect("should resolve to Some");
        assert!(matches!(
            result,
            MidiInputSource::TestClock { bpm, tick_interval }
                if (bpm - 120.0).abs() < f32::EPSILON
                    && tick_interval == midi_tick_interval(120.0).unwrap()
        ));
    }

    #[test]
    fn resolve_midi_hardware_propagates_connection_failure() {
        const DEVICE_NAME: &str = "Disconnected MIDI Device";

        let config = AppConfig {
            midi_input: Some(ConfigMidiInput::Device(DEVICE_NAME.to_owned())),
            ..AppConfig::default()
        };
        let state = Arc::new(AppState::new());

        let result = resolve_midi_hardware_with(&config, &state, |name, _state| {
            assert_eq!(name, DEVICE_NAME);
            anyhow::bail!("injected MIDI connection failure")
        });

        let Err(error) = result else {
            panic!("MIDI connection failure should stop hardware resolution");
        };
        assert!(error
            .to_string()
            .contains("injected MIDI connection failure"));
    }
}
