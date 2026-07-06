//! Worker thread ownership and coordinated shutdown for the audio pipeline.
//!
//! [`WorkerThreads`] owns the [`JoinHandle`] for each pipeline stage plus a
//! dynamic list of output transport workers (WebSocket server, OSC sender,
//! and any future transport). Shutdown is driven by [`WorkerThreads::shutdown`],
//! which joins the fixed pipeline stages in order, then the output workers in
//! the order they were spawned, waiting a bounded time for each one before
//! detaching.
//!
//! Adding a new output transport requires only extending [`OutputWorker`] with
//! a new variant, giving it a [`WorkerSpec`] in [`OutputWorker::spec`], and
//! pushing its handle onto the `outputs` list passed to [`WorkerThreads::new`].
//! Nothing about the shutdown loop or [`WorkerThreads`] itself needs to change.

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

/// The fixed audio pipeline stages, present in every run regardless of which
/// output transports are configured.
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

/// Owns the [`JoinHandle`] for each fixed pipeline stage, plus one handle per
/// configured output transport worker.
#[derive(Default)]
pub(crate) struct WorkerThreads {
    pub(crate) pipeline: [Option<JoinHandle<()>>; PipelineWorker::COUNT],
    pub(crate) outputs: Vec<(OutputWorker, JoinHandle<()>)>,
}

impl WorkerThreads {
    /// Constructs a `WorkerThreads` from the fixed pipeline handles and a list
    /// of output transport handles, one entry per spawned output.
    ///
    /// Any pipeline handle that is `None` is simply skipped during shutdown.
    pub(crate) fn new(
        generator: Option<JoinHandle<()>>,
        analyser: Option<JoinHandle<()>>,
        mapper: Option<JoinHandle<()>>,
        outputs: Vec<(OutputWorker, JoinHandle<()>)>,
    ) -> Self {
        let mut pipeline = [None, None, None];
        pipeline[PipelineWorker::Generator as usize] = generator;
        pipeline[PipelineWorker::Analyser as usize] = analyser;
        pipeline[PipelineWorker::Mapper as usize] = mapper;
        Self { pipeline, outputs }
    }

    /// Signals all workers to stop by joining the fixed pipeline stages first,
    /// then every output transport worker, waiting a bounded time for each one.
    /// Workers that do not stop within their grace period are detached rather
    /// than blocking the main thread indefinitely.
    pub(crate) fn shutdown(&mut self) {
        for worker in PipelineWorker::ALL {
            let Some(handle) = self.pipeline[worker as usize].take() else {
                continue;
            };
            Self::join_and_log(worker.spec(), handle);
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
}
