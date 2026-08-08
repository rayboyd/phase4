//! Resolves an [`AppConfig`] into running workers and shared state.
//!
//! [`bootstrap`] is the first half of what `App::new` used to do in one
//! function. It queries hardware, validates it, sizes the ringbufs, and
//! spawns every worker thread. `App::new` calls it once, then assembles the
//! [`App`](crate::app::App) value from the result.

use crate::app::AppState;
use crate::config::{
    validate_vocoder_sample_rate, AppConfig, AppConfigError, ConfigInput, ConfigMidiInput,
    ConfigOutputs, OutputConfig, TestSignal,
};
use crate::dsp::{vocoder::VOCODER_BANDS, DisplayPayload, RawPayload};
use crate::managers::audio::{ChannelMode, StreamSink};
use crate::managers::{
    Generator, Input, Mapper, MidiInputSource, MidiListener, OscSender, Processor, Server, Specs,
};
use crate::worker::{OutputWorker, WorkerThreads};
use anyhow::Result;
use std::net::SocketAddr;
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::watch;

/// Safety buffer for the analyse ringbuf, headroom for analysis accumulation.
const ANALYSE_BUFFER_MS: u32 = 500;

/// One spawned output transport worker's identity and thread handle, one
/// entry per configured [`OutputConfig`].
type OutputThreads = Vec<(OutputWorker, std::thread::JoinHandle<()>)>;

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
    /// Kept alive until dropped. Dropping the stream stops audio capture,
    /// and wraps the device in an Option so the caller can drop it on command.
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
}

/// Resolves the given configuration into hardware handles, shared state, and
/// running worker threads.
///
/// # Errors
///
/// Returns an error if the audio device cannot be opened, the input stream
/// cannot be started, or a configured output transport cannot bind to its
/// given address.
pub(crate) fn bootstrap(config: &AppConfig) -> Result<Bootstrapped> {
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
    let midi_source = resolve_midi_hardware(config)?;
    let midi_enabled = midi_source.is_some();

    // Validate. Must happen before ChannelMode::resolve below, which
    // takes the channel selection by value.
    validate_vocoder_sample_rate(config.vocoder_config.freq_high, hw_specs.sample_rate)?;
    validate_channel_selection(resolved.analyse_channels.as_deref(), hw_specs)?;

    let mut analyser_specs = hw_specs;
    let analyse_mode = ChannelMode::resolve(resolved.analyse_channels, &mut analyser_specs);

    let (analyse_tx, analyse_rx) =
        Input::create_audio_buffer_pair(analyser_specs, ANALYSE_BUFFER_MS);
    let display_channels = analyser_specs.channels as usize;
    let (raw_tx, raw_rx) = watch::channel(RawPayload::new(display_channels, VOCODER_BANDS));
    let (display_tx, display_rx) = watch::channel(DisplayPayload::new(display_channels));

    let generator_thread = spawn_audio_input(
        input_source,
        hw_specs,
        analyse_mode,
        analyse_tx,
        generator_state,
        &stream_state,
        &mut input_device,
    )?;

    let analyser = Processor::new(config.vocoder_config);
    let analyser_thread = Some(analyser.spawn(analyse_rx, raw_tx, analyser_specs, analyser_state));

    let mapper_thread = Some(Mapper::spawn(
        raw_rx,
        display_tx,
        display_channels,
        mapper_state,
        config.broadcast_rate,
        midi_enabled,
    ));

    let midi_thread = midi_source.map(|source| spawn_midi_input(source, state.clone()));

    let mut workers = WorkerThreads::new(
        generator_thread,
        analyser_thread,
        mapper_thread,
        midi_thread,
        Vec::new(),
    );

    // Retain each output handle as it starts so a later output failure can
    // shut down every worker through the normal bounded join path.
    let ws_bound_addr = match spawn_outputs(
        &config.outputs,
        &display_rx,
        display_channels,
        &state,
        midi_enabled,
        &mut workers.outputs,
    ) {
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
    })
}

/// Spawns one worker thread per configured output transport, matching each
/// [`OutputConfig`] descriptor to its spawn call.
///
/// Returns the WebSocket listener's actually bound address (`None` if no
/// WebSocket output is configured). Each spawned thread handle is appended
/// to `output_threads` immediately so the caller retains ownership if a later
/// output fails to start.
///
/// # Errors
///
/// Returns an error if a transport fails to bind (WebSocket listener) or
/// fails to acquire its local socket (OSC sender).
fn spawn_outputs(
    outputs: &ConfigOutputs,
    display_rx: &watch::Receiver<DisplayPayload>,
    display_channels: usize,
    state: &Arc<AppState>,
    midi_enabled: bool,
    output_threads: &mut OutputThreads,
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
                let (bound_addr, handle) = server.spawn(display_rx.clone(), Arc::clone(state))?;
                log::info!("WebSocket server listening on ws://{bound_addr}");
                ws_bound_addr = Some(bound_addr);
                output_threads.push((OutputWorker::WebSocket, handle));
            }
            OutputConfig::Osc { addr } => {
                let sender = OscSender::new(*addr);
                let handle = sender.spawn(
                    display_rx.clone(),
                    display_channels,
                    Arc::clone(state),
                    midi_enabled,
                )?;
                log::info!("OSC sender transmitting to udp://{addr}");
                output_threads.push((OutputWorker::Osc, handle));
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
                sample_rate: 44100,
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

/// Returns a resolved MIDI input source, if MIDI input is configured.
/// Mirrors `resolve_audio_hardware`: a missing device is reported
/// here, before any thread is spawned, rather than discovered later
/// inside a running thread.
///
/// # Errors
///
/// Returns an error if a configured MIDI device does not match any
/// available port.
fn resolve_midi_hardware(config: &AppConfig) -> Result<Option<MidiInputSource>> {
    match &config.midi_input {
        None => Ok(None),
        Some(ConfigMidiInput::TestClock(bpm)) => Ok(Some(MidiInputSource::TestClock(*bpm))),
        Some(ConfigMidiInput::Device(name)) => {
            let (midi_in, port, port_name) = crate::managers::midi::resolve_midi_device(name)?;
            Ok(Some(MidiInputSource::Hardware(midi_in, port, port_name)))
        }
    }
}

/// Spawns the MIDI listener thread for an already-resolved source,
/// announcing calibration mode synchronously first, matching
/// `spawn_audio_input`'s calibration announcement.
fn spawn_midi_input(source: MidiInputSource, state: Arc<AppState>) -> std::thread::JoinHandle<()> {
    if let MidiInputSource::TestClock(bpm) = &source {
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

        assert_eq!(resolved.hw_specs.sample_rate, 44100);
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
        let result = resolve_midi_hardware(&config).expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_midi_hardware_resolves_test_clock() {
        let config = AppConfig {
            midi_input: Some(ConfigMidiInput::TestClock(120.0)),
            ..AppConfig::default()
        };
        let result = resolve_midi_hardware(&config)
            .expect("should not error")
            .expect("should resolve to Some");
        assert!(
            matches!(result, MidiInputSource::TestClock(bpm) if (bpm - 120.0).abs() < f32::EPSILON)
        );
    }
}
