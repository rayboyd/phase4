use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Safety buffer for the record ringbuf, absorbs disk write jitter.
pub(crate) const RECORD_BUFFER_MS: u32 = 5000;

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
    Joined,
    TimedOut,
    Panicked,
}

#[derive(Debug, Clone, Copy)]
struct ShutdownWorker {
    name: &'static str,
    success_message: &'static str,
    timeout_ms: u64,
}

impl ShutdownWorker {
    const GENERATOR: Self = Self {
        name: "generator",
        success_message: "- Generator shutdown complete",
        timeout_ms: GENERATOR_SHUTDOWN_TIMEOUT_MS,
    };

    const ANALYSER: Self = Self {
        name: "analyser",
        success_message: "- Analyser shutdown complete",
        timeout_ms: ANALYSER_SHUTDOWN_TIMEOUT_MS,
    };

    const MAPPER: Self = Self {
        name: "mapper",
        success_message: "- Mapper shutdown complete",
        timeout_ms: MAPPER_SHUTDOWN_TIMEOUT_MS,
    };

    const SERVER: Self = Self {
        name: "server",
        success_message: "- Server shutdown complete",
        timeout_ms: SERVER_SHUTDOWN_TIMEOUT_MS,
    };

    const RECORDER: Self = Self {
        name: "recorder",
        success_message: "- Recorder shutdown complete",
        timeout_ms: RECORDER_SHUTDOWN_TIMEOUT_MS,
    };

    fn timeout(self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

#[derive(Default)]
pub(crate) struct WorkerThreads {
    pub(crate) recorder: Option<JoinHandle<()>>,
    pub(crate) analyser: Option<JoinHandle<()>>,
    pub(crate) mapper: Option<JoinHandle<()>>,
    pub(crate) server: Option<JoinHandle<()>>,
    pub(crate) generator: Option<JoinHandle<()>>,
}

impl WorkerThreads {
    pub(crate) fn shutdown(&mut self) {
        Self::shutdown_worker(ShutdownWorker::GENERATOR, &mut self.generator);
        Self::shutdown_worker(ShutdownWorker::ANALYSER, &mut self.analyser);
        Self::shutdown_worker(ShutdownWorker::MAPPER, &mut self.mapper);
        Self::shutdown_worker(ShutdownWorker::SERVER, &mut self.server);
        Self::shutdown_worker(ShutdownWorker::RECORDER, &mut self.recorder);
    }

    fn shutdown_worker(worker: ShutdownWorker, handle: &mut Option<JoinHandle<()>>) {
        let Some(handle) = handle.take() else {
            return;
        };

        if Self::join_with_timeout(worker, handle) == JoinOutcome::Joined {
            log::info!("{}", worker.success_message);
        }
    }

    fn join_with_timeout(worker: ShutdownWorker, handle: JoinHandle<()>) -> JoinOutcome {
        let deadline = Instant::now() + worker.timeout();

        loop {
            if handle.is_finished() {
                return if let Ok(()) = handle.join() {
                    JoinOutcome::Joined
                } else {
                    log::error!("Worker thread '{}' panicked during shutdown", worker.name);
                    JoinOutcome::Panicked
                };
            }

            let now = Instant::now();
            if now >= deadline {
                log::error!(
                    "Worker thread '{}' did not stop within {} ms, detaching",
                    worker.name,
                    worker.timeout_ms
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
            WorkerThreads::join_with_timeout(ShutdownWorker::GENERATOR, handle),
            JoinOutcome::Joined
        );
    }

    #[test]
    fn join_with_timeout_reports_panic() {
        let handle = thread::spawn(|| panic!("boom"));

        assert_eq!(
            WorkerThreads::join_with_timeout(ShutdownWorker::ANALYSER, handle),
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
            WorkerThreads::join_with_timeout(
                ShutdownWorker {
                    name: "blocking",
                    success_message: "",
                    timeout_ms: 20,
                },
                handle,
            ),
            JoinOutcome::TimedOut
        );

        keep_running.store(false, Ordering::Release);
        rx.recv_timeout(Duration::from_millis(200))
            .expect("detached thread should still exit once signalled");
    }
}
