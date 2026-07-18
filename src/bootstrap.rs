//! Resolves an [`AppConfig`] into running workers and shared state.
//!
//! [`bootstrap`] is the first half of what `App::new` used to do in one
//! function: query hardware, validate it, size the ringbufs, and spawn every
//! worker thread. `App::new` calls it once, then assembles the [`App`](crate::app::App)
//! value from the result.

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
use crate::ControllerMode;
use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
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

    /// How the controller should wait for a shutdown signal, threaded through
    /// from `config.controller_mode`.
    pub(crate) controller_mode: ControllerMode,

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
pub(crate) fn bootstrap(config: AppConfig) -> Result<Bootstrapped> {
    let state = Arc::new(AppState::new());
    let stream_state = Arc::clone(&state);
    let analyser_state = Arc::clone(&state);
    let mapper_state = Arc::clone(&state);
    let generator_state = Arc::clone(&state);
    let mut input_device = Input::new();

    let (hw_specs, input_source) = resolve_audio_hardware(&config, &mut input_device)?;
    let midi_source = resolve_midi_hardware(&config)?;
    let midi_enabled = midi_source.is_some();

    // Validate. Must happen before ChannelMode::resolve below, which
    // takes config.analyse_channels by value.
    validate_vocoder_sample_rate(config.vocoder_config.freq_high, hw_specs.sample_rate)?;
    validate_audio_hardware(&config, hw_specs, &input_source)?;

    let mut analyser_specs = hw_specs;
    let analyse_mode = ChannelMode::resolve(config.analyse_channels, &mut analyser_specs);

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

    // Spawns one worker per configured output transport. Each descriptor in
    // config.outputs matches to exactly one spawn arm in spawn_outputs,
    // adding a new transport means adding one variant and one arm there.
    let (ws_bound_addr, output_threads) = spawn_outputs(
        &config.outputs,
        &display_rx,
        display_channels,
        &state,
        midi_enabled,
    )?;

    Ok(Bootstrapped {
        input_device,
        state,
        workers: WorkerThreads::new(
            generator_thread,
            analyser_thread,
            mapper_thread,
            midi_thread,
            output_threads,
        ),
        controller_mode: config.controller_mode,
        ws_bound_addr,
    })
}

/// Spawns one worker thread per configured output transport, matching each
/// [`OutputConfig`] descriptor to its spawn call.
///
/// Returns the WebSocket listener's actually bound address (`None` if no
/// WebSocket output is configured) alongside the spawned thread handles.
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
) -> Result<(Option<SocketAddr>, OutputThreads)> {
    let mut output_threads = Vec::new();
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

    Ok((ws_bound_addr, output_threads))
}

/// Validates that all channel indices are within the hardware's capacity.
/// Does not apply in calibration mode, where no real device is involved.
///
/// # Errors
///
/// Returns an error if a requested channel index is at or beyond the
/// resolved hardware's channel count.
fn validate_audio_hardware(
    config: &AppConfig,
    hw_specs: Specs,
    input_source: &InputSource,
) -> Result<()> {
    match input_source {
        InputSource::Calibration(_) => Ok(()),
        InputSource::Hardware(..) => {
            if let Some(&idx) = config
                .analyse_channels
                .as_deref()
                .map(<[u16]>::iter)
                .and_then(Iterator::max)
            {
                if idx >= hw_specs.channels {
                    anyhow::bail!(AppConfigError::ChannelIndexOutOfRange {
                        idx,
                        channels: hw_specs.channels,
                    });
                }
            }
            Ok(())
        }
    }
}

/// Spawns the audio producer side of the pipeline: either a synthetic
/// [`Generator`] thread in calibration mode, or a real hardware input
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

/// Returns hardware specs and a resolved [`InputSource`], either calibration-mode
/// defaults or a real device handle.
///
/// # Errors
///
/// Returns an error if the device cannot be resolved or queried.
fn resolve_audio_hardware(config: &AppConfig, input: &mut Input) -> Result<(Specs, InputSource)> {
    match &config.input {
        ConfigInput::Calibration(signal) => Ok((
            Specs {
                sample_rate: 44100,
                channels: 2,
            },
            InputSource::Calibration(*signal),
        )),
        ConfigInput::Device(name_query) => {
            let (device, stream_config, specs) = input.get_device(name_query)?;
            Ok((specs, InputSource::Hardware(device, stream_config)))
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

        let (specs, input_source) = resolve_audio_hardware(&config, &mut input)
            .expect("resolve_audio_hardware should succeed in calibration mode");

        assert_eq!(specs.sample_rate, 44100);
        assert_eq!(specs.channels, 2);
        assert!(matches!(
            input_source,
            InputSource::Calibration(TestSignal::FixedTone(hz)) if (hz - 440.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn validate_audio_hardware_skips_the_check_in_calibration_mode() {
        let config = AppConfig {
            analyse_channels: Some(vec![99].into_boxed_slice()),
            ..AppConfig::default()
        };
        let hw_specs = Specs {
            sample_rate: 44100,
            channels: 2,
        };
        let input_source = InputSource::Calibration(TestSignal::FixedTone(440.0));

        assert!(validate_audio_hardware(&config, hw_specs, &input_source).is_ok());
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
