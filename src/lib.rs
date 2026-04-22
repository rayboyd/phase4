//! Exposes the public API used by both the entry point (`main.rs`) and integration
//! tests. All subsystem modules are declared here so Cargo can build a library
//! target alongside the binary, enabling `tests/` to import from it.

// 64-bit only. usize arithmetic in the audio pipeline would overflow on 32-bit.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("phase4 requires a 64-bit target.");

pub mod app;
pub mod config;
pub mod controller;
pub mod dsp;
pub mod managers;

use clap::Parser;
use config::{BitDepth, DEFAULT_ADDR_PATTERN, DEFAULT_FILENAME_PATTERN, DEFAULT_MAX_CLIENTS};
use std::net::SocketAddr;

#[derive(Parser)]
#[command(
    author = "Ray Boyd <ray.boyd@pm.me>",
    version,
    long_version = concat!(
        env!("CARGO_PKG_VERSION"),
        " (",
        env!("BUILD_GIT_HASH"),
        ")"
    ),
    about = "A real-time audio capture and analysis tool."
)]
pub struct Args {
    /// WebSocket server bind address.
    #[arg(short, long, default_value = DEFAULT_ADDR_PATTERN)]
    pub addr: SocketAddr,

    /// Maximum number of concurrent WebSocket clients.
    #[arg(long, default_value_t = DEFAULT_MAX_CLIENTS)]
    pub max_clients: usize,

    /// Recording bit depth.
    #[arg(short, long, default_value = "24")]
    pub bit_depth: BitDepth,

    /// Input device index.
    #[arg(short, long)]
    pub device: Option<usize>,

    /// Output filename pattern. Supports the tokens {`timestamp`}, {`sample_rate`}, and
    /// {`bit_depth`}. Treated as a filename only and written inside `recordings/`.
    #[arg(long, default_value = DEFAULT_FILENAME_PATTERN)]
    pub filename_pattern: String,

    /// List available audio input devices and exit.
    #[arg(short, long)]
    pub list: bool,

    /// Run in calibration mode with a synthetic sine wave at the given frequency (e.g. 440.0).
    #[arg(long)]
    pub test_hz: Option<f32>,

    /// Run a logarithmic sine wave sweep. The value is the LFO rate in Hz (e.g. 0.1 for 10s).
    #[arg(long)]
    pub test_sweep: Option<f32>,

    /// Vocoder envelope attack time constant in milliseconds. Smaller is faster.
    #[arg(long, default_value_t = 37.8)]
    pub vocoder_attack_ms: f32,

    /// Vocoder envelope release time constant in milliseconds. Smaller is faster.
    #[arg(long, default_value_t = 56.7)]
    pub vocoder_release_ms: f32,

    /// Vocoder lowest band centre frequency in Hz.
    #[arg(long, default_value_t = 40.0)]
    pub vocoder_freq_low: f32,

    /// Vocoder highest band centre frequency in Hz.
    #[arg(long, default_value_t = 18_000.0)]
    pub vocoder_freq_high: f32,

    /// Vocoder bandpass filter Q factor. Higher is narrower.
    #[arg(long, default_value_t = 2.0)]
    pub vocoder_filter_q: f32,

    /// Reject WebSocket clients that send a browser Origin header.
    #[arg(long)]
    pub no_browser_origin: bool,

    /// Target WebSocket broadcast rate in Hz (e.g. 30 or 60). Omit for unlimited.
    #[arg(long)]
    pub broadcast_rate: Option<f32>,
}
