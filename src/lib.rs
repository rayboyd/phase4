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

/// Synthetic signal generation for device calibration.
#[derive(clap::Args)]
#[command(next_help_heading = "Calibration")]
pub struct CalibrationArgs {
    /// Run in calibration mode with a synthetic sine wave at the given frequency (e.g. 440.0).
    #[arg(long)]
    pub test_hz: Option<f32>,

    /// Run a logarithmic sine wave sweep. The value is the LFO rate in Hz (e.g. 0.1 for 10s).
    #[arg(long)]
    pub test_sweep: Option<f32>,
}

/// Device selection and listing.
#[derive(clap::Args)]
#[command(next_help_heading = "Device")]
pub struct InputArgs {
    /// Input device index.
    #[arg(short, long)]
    pub device: Option<usize>,

    /// List available audio input devices and exit.
    #[arg(short, long)]
    pub list: bool,
}

/// WebSocket server and broadcast settings.
#[derive(clap::Args)]
#[command(next_help_heading = "Network")]
pub struct NetworkArgs {
    /// WebSocket server bind address.
    #[arg(short, long, default_value = DEFAULT_ADDR_PATTERN)]
    pub addr: SocketAddr,

    /// Maximum number of concurrent WebSocket clients.
    #[arg(long, default_value_t = DEFAULT_MAX_CLIENTS)]
    pub max_clients: usize,

    /// Target WebSocket broadcast rate in Hz (e.g. 30 or 60). Omit for unlimited.
    #[arg(long)]
    pub broadcast_rate: Option<f32>,

    /// Reject WebSocket clients whose handshake includes an Origin header.
    ///
    /// Only browsers are required by the Fetch spec to send Origin, so this flag
    /// blocks browser-originated connections while still allowing native clients
    /// that omit the header. It is not an authentication mechanism. Proper
    /// client authentication is planned for a later release.
    #[arg(long)]
    pub no_browser_origin: bool,
}

/// Audio recording settings.
#[derive(clap::Args)]
#[command(next_help_heading = "Recording")]
pub struct RecordingArgs {
    /// Recording bit depth.
    #[arg(short, long, default_value = "24")]
    pub bit_depth: BitDepth,

    /// Output filename pattern. Supports the tokens {`timestamp`}, {`sample_rate`}, and
    /// {`bit_depth`}. Treated as a filename only and written inside `recordings/`.
    #[arg(long, default_value = DEFAULT_FILENAME_PATTERN)]
    pub filename_pattern: String,

    /// Hardware channel indices to forward to the analyser, comma-separated (e.g. 0,1).
    /// Omit to forward all channels.
    #[arg(long, value_delimiter = ',')]
    pub analyse_channels: Option<Vec<u16>>,

    /// Hardware channel indices to record, comma-separated (e.g. 0,1).
    /// Omit to record all channels.
    #[arg(long, value_delimiter = ',')]
    pub record_channels: Option<Vec<u16>>,
}

/// Vocoder filter bank tuning.
#[derive(clap::Args)]
#[command(next_help_heading = "Vocoder")]
pub struct VocoderArgs {
    /// Vocoder envelope attack time constant in milliseconds. Smaller is faster.
    #[arg(long = "vocoder-attack-ms", default_value_t = 30.0)]
    pub attack_ms: f32,

    /// Vocoder envelope release time constant in milliseconds. Smaller is faster.
    #[arg(long = "vocoder-release-ms", default_value_t = 60.0)]
    pub release_ms: f32,

    /// Vocoder lowest band centre frequency in Hz.
    #[arg(long = "vocoder-freq-low", default_value_t = 40.0)]
    pub freq_low: f32,

    /// Vocoder highest band centre frequency in Hz.
    #[arg(long = "vocoder-freq-high", default_value_t = 18_000.0)]
    pub freq_high: f32,

    /// Vocoder bandpass filter Q factor. Higher is narrower.
    #[arg(long = "vocoder-filter-q", default_value_t = 2.0)]
    pub filter_q: f32,
}

#[derive(Parser)]
#[command(
    author = "Ray Boyd <ray.boyd@pm.me>",
    version,
    long_version = concat!(
        env!("CARGO_PKG_VERSION"),
        " (",
        env!("BUILD_GIT_HASH"),
        ", ",
        env!("BUILD_DISPLAY_BINS"),
        "-bin build)"
    ),
    about = "Phase4 is a fast, lightweight audio analysis tool built for \
            real-time audio visualization."
)]
pub struct Args {
    #[command(flatten)]
    pub calibration: CalibrationArgs,

    #[command(flatten)]
    pub input: InputArgs,

    #[command(flatten)]
    pub network: NetworkArgs,

    #[command(flatten)]
    pub recording: RecordingArgs,

    #[command(flatten)]
    pub vocoder: VocoderArgs,
}
