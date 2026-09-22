//! Worker thread ownership and coordinated shutdown for the audio pipeline.
//!
//! `WorkerThreads` owns the [`JoinHandle`] for every worker. Bootstrap
//! registers each worker as it starts, in the order shutdown should join
//! them, so a later startup failure can still stop the ones already running.
//! Each join has a grace period, after which the worker is detached.
//!
//! Adding a worker means adding a `WorkerKind` variant and registering the
//! handle where the worker is spawned.

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

/// Grace period for the MIDI input thread. The join loop unparks it so a
/// parked device holder sees the shutdown flag.
const MIDI_INPUT_SHUTDOWN_TIMEOUT_MS: u64 = 250;

/// Grace period for the server thread to finish its bounded accept and client shutdown.
const SERVER_SHUTDOWN_TIMEOUT_MS: u64 = 1_500;

/// Grace period for the OSC sender to observe display channel closure after the mapper exits.
const OSC_SENDER_SHUTDOWN_TIMEOUT_MS: u64 = 1_500;

/// Grace period for the frame writer to observe analyser channel closure.
#[cfg(target_os = "macos")]
const FRAME_WRITER_SHUTDOWN_TIMEOUT_MS: u64 = 1_000;

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
/// Stored with each registered handle and used by the shared join path.
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

/// Identifies a worker's shutdown metadata. Registration determines join order.
#[derive(Debug, Clone, Copy)]
pub(crate) enum WorkerKind {
    Generator,
    Analyser,
    Mapper,
    MidiInput,
    WebSocket,
    Osc,
    #[cfg(target_os = "macos")]
    FrameWriter,
}

impl WorkerKind {
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
            Self::MidiInput => WorkerSpec {
                name: "midi-input",
                success_message: "- MIDI input shutdown complete",
                timeout_ms: MIDI_INPUT_SHUTDOWN_TIMEOUT_MS,
            },
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
            #[cfg(target_os = "macos")]
            Self::FrameWriter => WorkerSpec {
                name: "frame-writer",
                success_message: "- Frame writer shutdown complete",
                timeout_ms: FRAME_WRITER_SHUTDOWN_TIMEOUT_MS,
            },
        }
    }
}

/// Owns worker handles and shutdown metadata in registration order.
#[derive(Default)]
pub(crate) struct WorkerThreads {
    registered_workers: Vec<(WorkerSpec, JoinHandle<()>)>,
}

impl WorkerThreads {
    /// Registers a started worker at the end of the shutdown sequence.
    /// Callers register the generator, analyser, mapper and MIDI input first,
    /// followed by output transports in their configured order.
    pub(crate) fn register(&mut self, kind: WorkerKind, handle: JoinHandle<()>) {
        self.registered_workers.push((kind.spec(), handle));
    }

    /// Joins workers in registration order, waiting a bounded time for each.
    /// The caller clears `keep_running` first. Workers that exceed their grace
    /// period are detached. The collection is drained, so a second call is a
    /// no-op.
    pub(crate) fn shutdown(&mut self) {
        for (spec, handle) in self.registered_workers.drain(..) {
            Self::join_and_log(spec, handle);
        }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.registered_workers.is_empty()
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
            WorkerThreads::join_with_timeout(WorkerKind::Generator.spec(), handle),
            JoinOutcome::Joined
        );
    }

    #[test]
    fn join_with_timeout_reports_panic() {
        let handle = thread::spawn(|| panic!("boom"));

        assert_eq!(
            WorkerThreads::join_with_timeout(WorkerKind::Analyser.spec(), handle),
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
            WorkerThreads::join_with_timeout(WorkerKind::Generator.spec(), handle),
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

        let mut workers = WorkerThreads::default();
        workers.register(WorkerKind::MidiInput, handle);
        keep_running.store(false, Ordering::Release);
        workers.shutdown();

        assert!(workers.is_empty(), "shutdown must consume the handle");
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
        const WEBSOCKET_OUTPUT: (WorkerKind, &str) = (
            WorkerKind::WebSocket,
            "- WebSocket server shutdown complete",
        );
        const OSC_OUTPUT: (WorkerKind, &str) = (WorkerKind::Osc, "- OSC sender shutdown complete");

        for outputs in [
            [WEBSOCKET_OUTPUT, OSC_OUTPUT],
            [OSC_OUTPUT, WEBSOCKET_OUTPUT],
        ] {
            testing_logger::setup();
            let mut workers = WorkerThreads::default();
            for kind in [
                WorkerKind::Generator,
                WorkerKind::Analyser,
                WorkerKind::Mapper,
                WorkerKind::MidiInput,
            ] {
                workers.register(kind, thread::spawn(|| {}));
            }
            for (kind, _) in outputs {
                workers.register(kind, thread::spawn(|| {}));
            }
            let expected_messages = [
                "- Generator shutdown complete",
                "- Analyser shutdown complete",
                "- Mapper shutdown complete",
                "- MIDI input shutdown complete",
                outputs[0].1,
                outputs[1].1,
            ];

            workers.shutdown();
            assert!(workers.is_empty());
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
        let mut workers = WorkerThreads::default();
        workers.register(WorkerKind::Analyser, thread::spawn(|| {}));
        workers.register(WorkerKind::Mapper, thread::spawn(|| {}));
        workers.shutdown();
        assert!(workers.is_empty());

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
