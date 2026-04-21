//! Top-level application struct that owns and coordinates all subsystems.
//!
//! [`App`] is responsible for constructing the audio pipeline: it queries the
//! selected input device for its hardware capabilities, sizes the ringbufs
//! accordingly, spawns the recorder, analyser, WebSocket server, and (in
//! calibration mode) the synthetic generator threads, then hands control to
//! the [`Controller`] for interactive keyboard handling.
//!
//! Shared runtime state is carried by [`AppState`], which holds a set of
//! [`std::sync::atomic`] flags that the controller writes and the worker threads
//! observe. On drop, all threads are signalled to stop and joined in order,
//! ensuring that in-flight data is flushed before the process exits.

use crate::config::{AppConfig, AppConfigError};
use crate::controller::Controller;
use crate::dsp::{DisplayPayload, RawPayload};
use crate::managers::{Generator, Input, Mapper, Processor, Server, Specs, Writer};
use anyhow::Result;
use ringbuf::traits::Split;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::thread::JoinHandle;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Utf8Bytes;

/// Safety buffer for the record ringbuf, absorbs disk write jitter.
const RECORD_BUFFER_MS: u32 = 5000;

/// Safety buffer for the analyse ringbuf, headroom for analysis accumulation.
const ANALYSE_BUFFER_MS: u32 = 500;

/// Shared application state flags for cross-thread synchronisation.
pub struct AppState {
    pub record_ring_overflow_events: AtomicUsize,
    pub is_broadcasting: AtomicBool,
    pub is_recording: AtomicBool,
    pub is_analysing: AtomicBool,
    pub keep_running: AtomicBool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            record_ring_overflow_events: AtomicUsize::new(0),
            is_broadcasting: AtomicBool::new(false),
            is_recording: AtomicBool::new(false),
            is_analysing: AtomicBool::new(false),
            keep_running: AtomicBool::new(true),
        }
    }
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

pub struct App {
    // Kept alive until dropped. Dropping the stream stops audio capture,
    // and wraps the device in an Option so we can drop it on command.
    input_device: Option<Input>,

    /// Shared atomic flags for cross-thread coordination.
    state: Arc<AppState>,

    /// Background thread draining the record ringbuf to disk.
    recorder_thread: Option<JoinHandle<()>>,

    /// Background thread consuming the analyse ringbuf (visual/metering).
    analyser_thread: Option<JoinHandle<()>>,

    /// Background thread mapping raw vocoder bins to display-resolution bins.
    mapper_thread: Option<JoinHandle<()>>,

    /// Background thread running the WebSocket broadcast server.
    server_thread: Option<JoinHandle<()>>,

    /// Synthetic audio generator thread, active only in calibration mode.
    generator_thread: Option<JoinHandle<()>>,

    /// Keyboard input handler, drives all runtime state transitions.
    controller: Controller,
}

impl App {
    /// Constructs the audio pipeline from the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the audio device cannot be opened, the input stream
    /// cannot be started, or the WebSocket server cannot bind to its address.
    ///
    /// # Panics
    ///
    /// Panics if `device_index` is `None` when not in calibration mode. This is
    /// guarded by `AppConfig::TryFrom`, so it should never occur in practice.
    pub fn new(config: AppConfig) -> Result<Self> {
        let state = Arc::new(AppState::new());
        let mut input_device = Input::new();

        let addr = config.addr;
        let bit_depth = config.bit_depth;
        let filename_pattern = config.filename_pattern;
        let test_hz = config.test_hz;
        let test_sweep = config.test_sweep;
        let device_index = config.device_index;
        let vocoder_config = config.vocoder_config;
        let no_browser_origin = config.no_browser_origin;
        let broadcast_rate = config.broadcast_rate;

        let calibration_mode = test_hz.is_some() || test_sweep.is_some();

        // In calibration mode no hardware is needed. Use standard CD-quality defaults.
        // Otherwise query the device for its native sample rate and channel count.
        let (specs, device_handle) = if calibration_mode {
            (
                Specs {
                    sample_rate: 44100,
                    channels: 2,
                },
                None,
            )
        } else {
            // device_index is guaranteed Some when not in calibration mode by AppConfig::TryFrom.
            let idx = device_index.expect("device_index required in hardware mode");
            let (d, c, s) = input_device.get_device(idx)?;
            (s, Some((d, c)))
        };

        Self::validate_vocoder_sample_rate(vocoder_config.freq_high, specs.sample_rate)?;

        let (record_tx, record_rx) = Self::create_audio_channel(specs, RECORD_BUFFER_MS);
        let (analyse_tx, analyse_rx) = Self::create_audio_channel(specs, ANALYSE_BUFFER_MS);
        let channels = specs.channels as usize;
        let (raw_tx, raw_rx) = watch::channel(RawPayload::new(
            channels,
            crate::dsp::vocoder::VOCODER_BANDS,
        ));
        let initial_display = serde_json::to_string(&DisplayPayload::new(channels))
            .expect("failed to serialise initial display payload");
        let (display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display));

        let mut generator_thread = None;

        if calibration_mode {
            generator_thread = Some(Generator::spawn(
                test_hz,
                test_sweep,
                specs.sample_rate,
                specs.channels,
                record_tx,
                analyse_tx,
                state.clone(),
            ));
        } else {
            // device_handle is guaranteed Some when not in calibration mode.
            let (device, stream_config) = device_handle.expect("device present in hardware mode");
            input_device.start_stream(
                &device,
                &stream_config,
                record_tx,
                analyse_tx,
                state.clone(),
            )?;
        }

        // Spin up the recorder thread to drain the record ringbuf to disk.
        let recorder = Writer::new(filename_pattern);
        let recorder_thread = Some(recorder.spawn(record_rx, bit_depth, specs, state.clone()));

        // Spin up the analyser thread to drain the analyse ringbuf and publish DSP results.
        let analyser = Processor::new(vocoder_config);
        let analyser_thread = Some(analyser.spawn(analyse_rx, raw_tx, specs, state.clone()));

        // Spin up the mapper thread to map raw vocoder bins to display resolution.
        let mapper_thread = Some(Mapper::spawn(
            raw_rx,
            display_tx,
            channels,
            state.clone(),
            broadcast_rate,
        ));

        // Spin up the WebSocket server thread.
        let server = Server::new(addr, no_browser_origin);
        let server_thread = Some(server.spawn(display_rx, state.clone())?);

        let controller = Controller::new(state.clone());

        Ok(Self {
            input_device: Some(input_device),
            state,
            recorder_thread,
            analyser_thread,
            mapper_thread,
            server_thread,
            generator_thread,
            controller,
        })
    }

    /// Hands control to the interactive controller, blocking until shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if the controller encounters a terminal or I/O failure.
    pub fn run(&self) -> Result<()> {
        self.controller.run()
    }

    fn create_audio_channel(
        specs: Specs,
        ms_safety: u32,
    ) -> (ringbuf::HeapProd<f32>, ringbuf::HeapCons<f32>) {
        let samples_per_sec = specs.sample_rate as usize * specs.channels as usize;
        let capacity = (samples_per_sec * ms_safety as usize) / 1000;

        // Power-of-two capacity enables bitmask wrapping (index & (len-1)) inside
        // the ringbuf, replacing modulo division on every push/pop. In the audio
        // callback (hot path) integer division has variable latency that risks
        // buffer underruns, where a single AND instruction is constant-time.
        ringbuf::HeapRb::<f32>::new(capacity.next_power_of_two()).split()
    }

    fn validate_vocoder_sample_rate(
        freq_high: f32,
        sample_rate: u32,
    ) -> Result<(), AppConfigError> {
        let sample_rate_hz = sample_rate as f32;
        let nyquist_hz = sample_rate_hz / 2.0;
        if freq_high >= nyquist_hz {
            return Err(AppConfigError::VocoderHighFrequencyAboveNyquist {
                sample_rate,
                freq_high,
                nyquist_hz,
            });
        }

        let max_safe_hz = sample_rate_hz * 0.45;
        if freq_high > max_safe_hz {
            return Err(AppConfigError::VocoderHighFrequencyAboveSafetyCeiling {
                sample_rate,
                freq_high,
                max_safe_hz,
            });
        }

        Ok(())
    }
}

impl Drop for App {
    // Before we signal all threads to stop looping we should drop the
    // input stream. This means no new samples will enter the ringbufs,
    // and the recorder can close the file without potentially missing
    // data being pushed, or corrupting the file.
    fn drop(&mut self) {
        log::info!("Shutdown started");

        self.input_device.take();
        log::info!("> Device shutdown complete");

        // Signal all threads to stop before joining any of them. This must
        // happen before joining the generator thread, which loops on this flag.
        self.state.keep_running.store(false, Ordering::Release);

        if let Some(handle) = self.generator_thread.take() {
            let _ = handle.join();
            log::info!("> Generator shutdown complete");
        }

        if let Some(handle) = self.recorder_thread.take() {
            let _ = handle.join();
            log::info!(">> Recorder shutdown complete");
        }

        if let Some(handle) = self.analyser_thread.take() {
            let _ = handle.join();
            log::info!(">> Analyser shutdown complete");
        }

        if let Some(handle) = self.mapper_thread.take() {
            let _ = handle.join();
            log::info!(">> Mapper shutdown complete");
        }

        if let Some(handle) = self.server_thread.take() {
            let _ = handle.join();
            log::info!(">> Server shutdown complete");
        }

        log::info!("Shutdown complete");
    }
}
