//! Each submodule owns a distinct stage of the processing pipeline and exposes
//! a type that is re-exported here for use by [`crate::app`].
//!
//! Submodules:
//! - [`analyser`]: DSP analysis thread consuming the analyse ringbuf.
//! - [`audio`]: CPAL audio input device and stream management.
//! - [`generator`]: Synthetic signal generator for calibration mode.
//! - [`mapper`]: Display payload mapper reducing raw vocoder bins to [`crate::dsp::DISPLAY_BINS`] bins.
//! - [`midi`]: MIDI input listener writing transport and clock state atomics.
//! - [`osc`]: OSC UDP sender broadcasting bin values to a configured target address.
//! - [`server`]: WebSocket server broadcasting pre-serialised JSON to clients.

use std::future::Future;
use std::thread::{self, JoinHandle};

pub mod analyser;
pub mod audio;
pub mod generator;
pub mod mapper;
pub mod midi;
pub mod osc;
pub mod server;

pub use analyser::Processor;
pub use audio::{Input, Specs};
pub use generator::Generator;
pub use mapper::Mapper;
pub use midi::MidiListener;
pub(crate) use midi::{
    MidiInputSource, MIDI_TRANSPORT_CONTINUE, MIDI_TRANSPORT_NONE, MIDI_TRANSPORT_START,
    MIDI_TRANSPORT_STOP,
};
pub use osc::OscSender;
pub use server::Server;

/// Spawns a dedicated OS thread running a single-threaded Tokio runtime, then
/// blocks on `future` until it completes.
///
/// Centralises the runtime-per-worker pattern used by the mapper, server, and
/// OSC sender. Fallible setup that must happen before the thread starts (TCP
/// bind, UDP bind) stays in the caller, this only covers the uniform part.
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
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unknown");
        let linux_hint = if cfg!(target_os = "linux") {
            " On Linux, grant CAP_SYS_NICE to the binary or run under a user with rtprio limits set."
        } else {
            ""
        };
        log::warn!("Failed to set thread priority for '{name}': {e}.{linux_hint}");
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
