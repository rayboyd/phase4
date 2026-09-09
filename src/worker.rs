//! Worker thread ownership and coordinated shutdown for the audio pipeline.
//!
//! `WorkerThreads` owns the [`JoinHandle`] for each pipeline stage, MIDI input
//! and configured output transport. Shutdown joins the generator, analyser,
//! mapper and MIDI input, then the output workers in registration order.
//! Each join has a grace period, after which an unfinished worker is detached.
//!
//! Registering a new output worker for shutdown requires extending `OutputWorker` with
//! a new variant, giving it a `WorkerSpec` in `OutputWorker::spec`, and
//! pushing its handle onto the `outputs` list passed to `WorkerThreads::new`.
//! Nothing about the shutdown loop or `WorkerThreads` itself needs to change.

use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Poll interval while waiting for a worker thread to finish.
const SHUTDOWN_POLL_MS: u64 = 10;

/// Grace period for the generator thread, which wakes on a 10 ms cadence.
const GENERATOR_SHUTDOWN_TIMEOUT_MS: u64 = 250;

/// Grace period for the analyser thread to drain and release the mapper input.
const ANALYSER_SHUTDOWN_TIMEOUT_MS: u64 = 1_000;

/// Grace period for the mapper thread to observe analyser channel closure.
const MAPPER_SHUTDOWN_TIMEOUT_MS: u64 = 1_000;

/// Grace period for the MIDI input thread. The join loop unparks a real
/// device holder so it can observe shutdown and release its connection.
const MIDI_INPUT_SHUTDOWN_TIMEOUT_MS: u64 = 250;

/// Grace period for the server thread to finish its bounded accept and client shutdown.
const SERVER_SHUTDOWN_TIMEOUT_MS: u64 = 1_500;

/// Grace period for the OSC sender to observe display channel closure after the mapper exits.
const OSC_SENDER_SHUTDOWN_TIMEOUT_MS: u64 = 1_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinOutcome {
    /// The thread finished and joined cleanly.
    Joined,

    /// The grace period elapsed before the thread finished; it has been detached.
    TimedOut,

    /// The thread finished but its closure panicked.
    Panicked,
}

/// A worker thread's display name, shutdown grace period, and success log line.
/// Shared between the fixed pipeline stages and the dynamic output transports
/// so both are joined through the same code path.
#[derive(Debug, Clone, Copy)]
struct WorkerSpec {
    name: &'static str,
    success_message: &'static str,
    timeout_ms: u64,
}

impl WorkerSpec {
    fn timeout(self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

/// Fixed pipeline slots. The generator slot is populated only in calibration mode.
#[derive(Debug, Clone, Copy)]
enum PipelineWorker {
    Generator = 0,
    Analyser = 1,
    Mapper = 2,
}

impl PipelineWorker {
    /// Total number of variants. Keeps the `WorkerThreads` pipeline array size in sync.
    const COUNT: usize = 3;

    /// Ordered list of all variants, used by the shutdown loop.
    const ALL: [Self; Self::COUNT] = [Self::Generator, Self::Analyser, Self::Mapper];

    fn spec(self) -> WorkerSpec {
        match self {
            Self::Generator => WorkerSpec {
                name: "generator",
                success_message: "- Generator shutdown complete",
                timeout_ms: GENERATOR_SHUTDOWN_TIMEOUT_MS,
            },
            Self::Analyser => WorkerSpec {
                name: "analyser",
                success_message: "- Analyser shutdown complete",
                timeout_ms: ANALYSER_SHUTDOWN_TIMEOUT_MS,
            },
            Self::Mapper => WorkerSpec {
                name: "mapper",
                success_message: "- Mapper shutdown complete",
                timeout_ms: MAPPER_SHUTDOWN_TIMEOUT_MS,
            },
        }
    }
}

/// Identifies which output transport an entry in `WorkerThreads::outputs`
/// belongs to. One variant per [`crate::config::OutputConfig`] variant.
#[derive(Debug, Clone, Copy)]
pub(crate) enum OutputWorker {
    WebSocket,
    Osc,
}

impl OutputWorker {
    fn spec(self) -> WorkerSpec {
        match self {
            Self::WebSocket => WorkerSpec {
                name: "websocket-server",
                success_message: "- WebSocket server shutdown complete",
                timeout_ms: SERVER_SHUTDOWN_TIMEOUT_MS,
            },
            Self::Osc => WorkerSpec {
                name: "osc-sender",
                success_message: "- OSC sender shutdown complete",
                timeout_ms: OSC_SENDER_SHUTDOWN_TIMEOUT_MS,
            },
        }
    }
}

fn midi_input_spec() -> WorkerSpec {
    WorkerSpec {
        name: "midi-input",
        success_message: "- MIDI input shutdown complete",
        timeout_ms: MIDI_INPUT_SHUTDOWN_TIMEOUT_MS,
    }
}

/// Owns the [`JoinHandle`] for each fixed pipeline stage, plus one handle per
/// configured output transport worker.
#[derive(Default)]
pub(crate) struct WorkerThreads {
    pub(crate) pipeline: [Option<JoinHandle<()>>; PipelineWorker::COUNT],
    pub(crate) midi_input: Option<JoinHandle<()>>,
    pub(crate) outputs: Vec<(OutputWorker, JoinHandle<()>)>,
}

impl WorkerThreads {
    /// Constructs a `WorkerThreads` from the fixed pipeline handles and a list
    /// of output transport handles, one entry per spawned output.
    ///
    /// Any pipeline handle that is `None` is skipped during shutdown.
    pub(crate) fn new(
        generator: Option<JoinHandle<()>>,
        analyser: Option<JoinHandle<()>>,
        mapper: Option<JoinHandle<()>>,
        midi_input: Option<JoinHandle<()>>,
        outputs: Vec<(OutputWorker, JoinHandle<()>)>,
    ) -> Self {
        let mut pipeline = [None, None, None];
        pipeline[PipelineWorker::Generator as usize] = generator;
        pipeline[PipelineWorker::Analyser as usize] = analyser;
        pipeline[PipelineWorker::Mapper as usize] = mapper;
        Self {
            pipeline,
            midi_input,
            outputs,
        }
    }

    /// Joins the fixed pipeline stages, MIDI input, then output workers,
    /// waiting a bounded time for each one. The caller must first clear
    /// `keep_running`. Joining does not set the shutdown flag.
    /// Workers that do not stop within their grace period are detached rather
    /// than blocking the main thread indefinitely.
    pub(crate) fn shutdown(&mut self) {
        for worker in PipelineWorker::ALL {
            let Some(handle) = self.pipeline[worker as usize].take() else {
                continue;
            };
            Self::join_and_log(worker.spec(), handle);
        }

        if let Some(handle) = self.midi_input.take() {
            Self::join_and_log(midi_input_spec(), handle);
        }

        for (worker, handle) in self.outputs.drain(..) {
            Self::join_and_log(worker.spec(), handle);
        }
    }

    fn join_and_log(spec: WorkerSpec, handle: JoinHandle<()>) {
        if Self::join_with_timeout(spec, handle) == JoinOutcome::Joined {
            log::info!("{}", spec.success_message);
        }
    }

    fn join_with_timeout(spec: WorkerSpec, handle: JoinHandle<()>) -> JoinOutcome {
        let deadline = Instant::now() + spec.timeout();

        loop {
            if handle.is_finished() {
                return if let Ok(()) = handle.join() {
                    JoinOutcome::Joined
                } else {
                    log::error!("Worker thread '{}' panicked during shutdown", spec.name);
                    JoinOutcome::Panicked
                };
            }

            let now = Instant::now();
            if now >= deadline {
                log::error!(
                    "Worker thread '{}' did not stop within {} ms, detaching",
                    spec.name,
                    spec.timeout_ms
                );
                return JoinOutcome::TimedOut;
            }

            // Continually wake to prevent a race condition where the thread
            // enters a park state after the initial shutdown signal is posted.
            handle.thread().unpark();

            let remaining = deadline.saturating_duration_since(now);
            thread::sleep(remaining.min(Duration::from_millis(SHUTDOWN_POLL_MS)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };

    #[test]
    fn join_with_timeout_joins_completed_thread() {
        let handle = thread::spawn(|| {});

        assert_eq!(
            WorkerThreads::join_with_timeout(PipelineWorker::Generator.spec(), handle),
            JoinOutcome::Joined
        );
    }

    #[test]
    fn join_with_timeout_reports_panic() {
        let handle = thread::spawn(|| panic!("boom"));

        assert_eq!(
            WorkerThreads::join_with_timeout(PipelineWorker::Analyser.spec(), handle),
            JoinOutcome::Panicked
        );
    }

    #[test]
    fn join_with_timeout_times_out_without_blocking_forever() {
        let keep_running = Arc::new(AtomicBool::new(true));
        let thread_state = keep_running.clone();
        let (tx, rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            while thread_state.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            tx.send(()).expect("thread exit signal should be delivered");
        });

        assert_eq!(
            WorkerThreads::join_with_timeout(PipelineWorker::Generator.spec(), handle),
            JoinOutcome::TimedOut
        );

        keep_running.store(false, Ordering::Release);
        rx.recv_timeout(Duration::from_millis(200))
            .expect("detached thread should still exit once signalled");
    }

    #[test]
    fn shutdown_unparks_and_joins_midi_input() {
        const READY_TIMEOUT: Duration = Duration::from_secs(1);

        testing_logger::setup();
        let keep_running = Arc::new(AtomicBool::new(true));
        let exited = Arc::new(AtomicBool::new(false));
        let thread_keep_running = Arc::clone(&keep_running);
        let thread_exited = Arc::clone(&exited);
        let (ready_tx, ready_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            ready_tx.send(()).expect("ready signal should be delivered");
            loop {
                thread::park();
                if !thread_keep_running.load(Ordering::Acquire) {
                    break;
                }
            }
            thread_exited.store(true, Ordering::Release);
        });
        ready_rx
            .recv_timeout(READY_TIMEOUT)
            .expect("worker should start");

        let mut workers = WorkerThreads {
            midi_input: Some(handle),
            ..WorkerThreads::default()
        };
        keep_running.store(false, Ordering::Release);
        workers.shutdown();

        assert!(
            workers.midi_input.is_none(),
            "shutdown must consume the handle"
        );
        assert!(
            exited.load(Ordering::Acquire),
            "worker must finish before shutdown returns"
        );
        testing_logger::validate(|logs| {
            assert_eq!(logs.len(), 1);
            assert_eq!(logs[0].level, log::Level::Info);
            assert_eq!(logs[0].body, "- MIDI input shutdown complete");
        });
    }

    #[test]
    fn shutdown_preserves_join_order_and_consumes_handles_once() {
        for outputs in [
            [OutputWorker::WebSocket, OutputWorker::Osc],
            [OutputWorker::Osc, OutputWorker::WebSocket],
        ] {
            testing_logger::setup();
            let mut workers = WorkerThreads::new(
                Some(thread::spawn(|| {})),
                Some(thread::spawn(|| {})),
                Some(thread::spawn(|| {})),
                Some(thread::spawn(|| {})),
                outputs
                    .into_iter()
                    .map(|worker| (worker, thread::spawn(|| {})))
                    .collect(),
            );
            let expected_outputs = match outputs[0] {
                OutputWorker::WebSocket => [
                    "- WebSocket server shutdown complete",
                    "- OSC sender shutdown complete",
                ],
                OutputWorker::Osc => [
                    "- OSC sender shutdown complete",
                    "- WebSocket server shutdown complete",
                ],
            };
            let expected_messages = [
                "- Generator shutdown complete",
                "- Analyser shutdown complete",
                "- Mapper shutdown complete",
                "- MIDI input shutdown complete",
                expected_outputs[0],
                expected_outputs[1],
            ];

            workers.shutdown();
            assert!(workers.pipeline.iter().all(Option::is_none));
            assert!(workers.midi_input.is_none());
            assert!(workers.outputs.is_empty());
            workers.shutdown();

            testing_logger::validate(|logs| {
                let messages: Vec<_> = logs.iter().map(|entry| entry.body.as_str()).collect();
                assert_eq!(messages, expected_messages);
                assert!(logs.iter().all(|entry| entry.level == log::Level::Info));
            });
        }
    }

    #[test]
    fn shutdown_skips_absent_workers() {
        testing_logger::setup();
        let mut workers = WorkerThreads::new(
            None,
            Some(thread::spawn(|| {})),
            Some(thread::spawn(|| {})),
            None,
            Vec::new(),
        );
        workers.shutdown();
        assert!(workers.pipeline.iter().all(Option::is_none));
        assert!(workers.midi_input.is_none());
        assert!(workers.outputs.is_empty());

        testing_logger::validate(|logs| {
            let messages: Vec<_> = logs.iter().map(|entry| entry.body.as_str()).collect();
            assert_eq!(
                messages,
                ["- Analyser shutdown complete", "- Mapper shutdown complete"]
            );
            assert!(logs.iter().all(|entry| entry.level == log::Level::Info));
        });
    }
}
