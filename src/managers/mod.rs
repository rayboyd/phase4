//! Each submodule owns a distinct stage of the processing pipeline and exposes
//! a type that is re-exported here for use by [`crate::app`].
//!
//! Submodules:
//! - [`analyser`]: DSP analysis thread consuming the analyse ringbuf.
//! - [`audio`]: CPAL audio input device and stream management.
//! - [`generator`]: Synthetic signal generator for calibration mode.
//! - [`mapper`]: Display payload mapper reducing raw vocoder bins to [`crate::dsp::DISPLAY_BINS`] bins.
//! - [`osc`]: OSC UDP sender broadcasting bin values to a configured target address.
//! - [`server`]: WebSocket server broadcasting pre-serialised JSON to clients.

use std::future::Future;
use std::thread::{self, JoinHandle};

pub mod analyser;
pub mod audio;
pub mod generator;
pub mod mapper;
pub mod osc;
pub mod server;

pub use analyser::Processor;
pub use audio::{Input, Specs};
pub use generator::Generator;
pub use mapper::Mapper;
pub use osc::OscSender;
pub use server::Server;

/// Spawns a dedicated OS thread running a single-threaded Tokio runtime, then
/// blocks on `future` until it completes.
///
/// Centralises the runtime-per-worker pattern used by the mapper, server, and
/// OSC sender. Fallible setup that must happen before the thread starts (TCP
/// bind, UDP connect) stays in the caller, this only covers the uniform part.
///
/// # Panics
///
/// Panics if the OS thread cannot be spawned or the Tokio runtime cannot be
/// built.
fn spawn_async_worker<F>(name: &'static str, future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap_or_else(|e| panic!("failed to build tokio runtime for {name}: {e}"))
                .block_on(future);
        })
        .unwrap_or_else(|e| panic!("failed to spawn {name} thread: {e}"))
}

/// Log a warning when setting thread priority fails, rather than silently
/// discarding the error. On Linux without `CAP_SYS_NICE` the call always
/// fails and this surfaces the reason without panicking.
pub(crate) fn log_priority_result(result: Result<(), thread_priority::Error>) {
    if let Err(e) = result {
        if cfg!(target_os = "linux") {
            log::warn!(
                "Failed to set thread priority for '{}': {}. \
                 On Linux, grant CAP_SYS_NICE to the binary or run under a \
                 user with rtprio limits set.",
                std::thread::current().name().unwrap_or("unknown"),
                e,
            );
        } else {
            log::warn!(
                "Failed to set thread priority for '{}': {}",
                std::thread::current().name().unwrap_or("unknown"),
                e,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_priority_result_logs_on_failure() {
        testing_logger::setup();

        log_priority_result(Err(thread_priority::Error::OS(1)));

        testing_logger::validate(|captured_logs| {
            assert_eq!(captured_logs.len(), 1);
            assert!(captured_logs[0]
                .body
                .contains("Failed to set thread priority"));
            assert_eq!(captured_logs[0].level, log::Level::Warn);
        });
    }

    #[test]
    fn log_priority_result_silent_on_success() {
        testing_logger::setup();

        log_priority_result(Ok(()));

        testing_logger::validate(|captured_logs| {
            assert!(captured_logs.is_empty());
        });
    }
}
