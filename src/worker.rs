//! Worker thread ownership and coordinated shutdown for the audio pipeline.
//!
//! [`WorkerThreads`] owns the [`JoinHandle`] for each pipeline stage. Shutdown
//! is driven by [`WorkerThreads::shutdown`], which iterates every [`ShutdownWorker`]
//! variant in order and waits a bounded time for each one before detaching.
//!
//! Adding a new worker requires only extending [`ShutdownWorker`] with a new variant
//! and updating its three `match` arms. The shutdown loop and [`WorkerThreads`] array
//! size update automatically via [`ShutdownWorker::COUNT`] and [`ShutdownWorker::ALL`].

use crate::app::RECORD_BUFFER_MS;
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

/// Grace period for the recorder to drain the record ringbuf and finalise the WAV file.
const RECORDER_SHUTDOWN_TIMEOUT_MS: u64 = RECORD_BUFFER_MS as u64 + 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinOutcome {
    /// The thread finished and joined cleanly.
    Joined,
    /// The grace period elapsed before the thread finished; it has been detached.
    TimedOut,
    /// The thread finished but its closure panicked.
    Panicked,
}

/// Identifies a named worker thread and carries its shutdown timeout.
#[derive(Debug, Clone, Copy)]
enum ShutdownWorker {
    Generator = 0,
    Analyser = 1,
    Mapper = 2,
    Server = 3,
    Recorder = 4,
}

impl ShutdownWorker {
    /// Total number of variants. Keeps [`WorkerThreads`] array size in sync.
    const COUNT: usize = 5;

    /// Ordered list of all variants, used by the shutdown loop.
    const ALL: [Self; Self::COUNT] = [
        Self::Generator,
        Self::Analyser,
        Self::Mapper,
        Self::Server,
        Self::Recorder,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Generator => "generator",
            Self::Analyser => "analyser",
            Self::Mapper => "mapper",
            Self::Server => "server",
            Self::Recorder => "recorder",
        }
    }

    fn success_message(self) -> &'static str {
        match self {
            Self::Generator => "- Generator shutdown complete",
            Self::Analyser => "- Analyser shutdown complete",
            Self::Mapper => "- Mapper shutdown complete",
            Self::Server => "- Server shutdown complete",
            Self::Recorder => "- Recorder shutdown complete",
        }
    }

    fn timeout_ms(self) -> u64 {
        match self {
            Self::Generator => GENERATOR_SHUTDOWN_TIMEOUT_MS,
            Self::Analyser => ANALYSER_SHUTDOWN_TIMEOUT_MS,
            Self::Mapper => MAPPER_SHUTDOWN_TIMEOUT_MS,
            Self::Server => SERVER_SHUTDOWN_TIMEOUT_MS,
            Self::Recorder => RECORDER_SHUTDOWN_TIMEOUT_MS,
        }
    }

    fn timeout(self) -> Duration {
        Duration::from_millis(self.timeout_ms())
    }
}

/// Owns the [`JoinHandle`] for each pipeline stage, indexed by [`ShutdownWorker`] discriminant.
#[derive(Default)]
pub(crate) struct WorkerThreads(pub(crate) [Option<JoinHandle<()>>; ShutdownWorker::COUNT]);

impl WorkerThreads {
    /// Constructs a `WorkerThreads` from individual optional handles.
    ///
    /// Any handle that is `None` is simply skipped during shutdown.
    pub(crate) fn new(
        generator: Option<JoinHandle<()>>,
        analyser: Option<JoinHandle<()>>,
        mapper: Option<JoinHandle<()>>,
        server: Option<JoinHandle<()>>,
        recorder: Option<JoinHandle<()>>,
    ) -> Self {
        let mut handles = [None, None, None, None, None];
        handles[ShutdownWorker::Generator as usize] = generator;
        handles[ShutdownWorker::Analyser as usize] = analyser;
        handles[ShutdownWorker::Mapper as usize] = mapper;
        handles[ShutdownWorker::Server as usize] = server;
        handles[ShutdownWorker::Recorder as usize] = recorder;
        Self(handles)
    }

    /// Signals all workers to stop by iterating [`ShutdownWorker::ALL`] and waiting
    /// a bounded time for each one. Workers that do not stop within their grace period
    /// are detached rather than blocking the main thread indefinitely.
    pub(crate) fn shutdown(&mut self) {
        for worker in ShutdownWorker::ALL {
            Self::shutdown_worker(worker, &mut self.0[worker as usize]);
        }
    }

    fn shutdown_worker(worker: ShutdownWorker, handle: &mut Option<JoinHandle<()>>) {
        let Some(handle) = handle.take() else {
            return;
        };

        if Self::join_with_timeout(worker, handle) == JoinOutcome::Joined {
            log::info!("{}", worker.success_message());
        }
    }

    fn join_with_timeout(worker: ShutdownWorker, handle: JoinHandle<()>) -> JoinOutcome {
        let deadline = Instant::now() + worker.timeout();

        loop {
            if handle.is_finished() {
                return if let Ok(()) = handle.join() {
                    JoinOutcome::Joined
                } else {
                    log::error!("Worker thread '{}' panicked during shutdown", worker.name());
                    JoinOutcome::Panicked
                };
            }

            let now = Instant::now();
            if now >= deadline {
                log::error!(
                    "Worker thread '{}' did not stop within {} ms, detaching",
                    worker.name(),
                    worker.timeout_ms()
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
            WorkerThreads::join_with_timeout(ShutdownWorker::Generator, handle),
            JoinOutcome::Joined
        );
    }

    #[test]
    fn join_with_timeout_reports_panic() {
        let handle = thread::spawn(|| panic!("boom"));

        assert_eq!(
            WorkerThreads::join_with_timeout(ShutdownWorker::Analyser, handle),
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
            WorkerThreads::join_with_timeout(ShutdownWorker::Generator, handle),
            JoinOutcome::TimedOut
        );

        keep_running.store(false, Ordering::Release);
        rx.recv_timeout(Duration::from_millis(200))
            .expect("detached thread should still exit once signalled");
    }
}
