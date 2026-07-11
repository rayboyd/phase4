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
pub mod worker;

use clap::Parser;
use std::net::SocketAddr;

/// Synthetic signal generation for device calibration.
#[derive(clap::Args)]
#[command(next_help_heading = "Calibration")]
pub struct CalibrationArgs {
    /// Run in calibration mode with a synthetic sine wave at the given frequency
    /// (e.g. 440.0). Mutually exclusive with --test-sweep.
    #[arg(long)]
    pub test_hz: Option<f32>,

    /// Run a logarithmic sine wave sweep. The value is the LFO rate in Hz (e.g. 0.1
    /// for a 10 second cycle). Mutually exclusive with --test-hz.
    #[arg(long, conflicts_with = "test_hz")]
    pub test_sweep: Option<f32>,
}

/// MIDI input configuration.
#[derive(clap::Args, Debug, Clone)]
pub struct MidiArgs {
    /// Connect to a real MIDI input device matching this name (exact match
    /// first, then case-insensitive substring). Mutually exclusive with
    /// --midi-test-bpm.
    #[arg(long)]
    pub midi_device: Option<String>,

    /// Run against a synthetic MIDI clock at the given tempo (e.g. 120.0)
    /// instead of a real device. Mutually exclusive with --midi-device.
    #[arg(long, conflicts_with = "midi_device")]
    pub midi_test_bpm: Option<f32>,
}

/// Output format for `--list`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListFormat {
    /// Human-readable, one line per device. The default.
    #[default]
    Text,
    /// A single JSON array on stdout, one object per device. Intended for a
    /// wrapper process to parse programmatically, nothing else is written to
    /// stdout in this mode.
    Json,
}

/// Device selection and listing.
#[derive(clap::Args)]
#[command(next_help_heading = "Device")]
pub struct InputArgs {
    /// Input device name or partial name (exact match first, then substring).
    #[arg(short, long)]
    pub device: Option<String>,

    /// List available audio input devices and exit.
    #[arg(short, long)]
    pub list: bool,

    /// Output format for `--list`.
    #[arg(long, value_enum, default_value_t = ListFormat::Text)]
    pub list_format: ListFormat,

    /// Hardware channel indices to forward to the analyser, comma-separated (e.g. 0,1).
    /// Omit to forward all channels.
    #[arg(long, value_delimiter = ',')]
    pub analyse_channels: Option<Vec<u16>>,
}

/// Output transport settings. Both transports are opt-in, omitting an
/// address disables that transport entirely.
#[derive(clap::Args)]
#[command(next_help_heading = "Network")]
pub struct NetworkArgs {
    /// WebSocket JSON listen address (e.g. 127.0.0.1:8889). Phase4 binds this
    /// address and clients connect in. Omit to disable the WebSocket output.
    #[arg(long)]
    pub ws_addr: Option<SocketAddr>,

    /// Maximum number of concurrent WebSocket clients (default: 8).
    #[arg(long)]
    pub max_clients: Option<usize>,

    /// Target WebSocket broadcast rate in Hz, e.g. 30 or 60 (default: 60).
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

    /// OSC UDP target address (e.g. 127.0.0.1:7000). Phase4 sends to this
    /// address, it does not listen. Omit to disable the OSC output.
    ///
    /// When set, phase4 emits one OSC float message per bin per channel each broadcast
    /// frame to the given address.
    /// Addresses follow the scheme /phase4/ch/{n}/bin/{n} with a float argument in 0.0..=1.0.
    /// Map these to your VJ software parameters using its OSC shortcut editor.
    #[arg(long)]
    pub osc_addr: Option<SocketAddr>,
}

/// Vocoder filter bank tuning.
#[derive(clap::Args)]
#[command(next_help_heading = "Vocoder")]
pub struct VocoderArgs {
    /// Vocoder envelope attack time constant in milliseconds. Smaller is faster (default: 24).
    #[arg(long = "vocoder-attack-ms")]
    pub attack_ms: Option<f32>,

    /// Vocoder envelope release time constant in milliseconds. Smaller is faster (default: 96).
    #[arg(long = "vocoder-release-ms")]
    pub release_ms: Option<f32>,

    /// Vocoder lowest band centre frequency in Hz (default: 60).
    #[arg(long = "vocoder-freq-low")]
    pub freq_low: Option<f32>,

    /// Vocoder highest band centre frequency in Hz (default: 6000).
    #[arg(long = "vocoder-freq-high")]
    pub freq_high: Option<f32>,

    /// Vocoder bandpass filter Q factor. Higher is narrower (default: 8).
    #[arg(long = "vocoder-filter-q")]
    pub filter_q: Option<f32>,
}

/// Runtime controller selection.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ControllerMode {
    /// Interactive terminal control (raw mode, keyboard driven). The default,
    /// unchanged behaviour for running phase4 directly from a shell.
    #[default]
    Term,
    /// Headless mode: block until stdin closes, no keyboard handling. Wrapper
    /// processes should always pass this explicitly.
    Headless,
}

/// Runtime controller behaviour.
#[derive(clap::Args)]
#[command(next_help_heading = "Runtime")]
pub struct RuntimeArgs {
    /// How phase4 waits for a shutdown signal.
    #[arg(long, value_enum, default_value_t = ControllerMode::Term)]
    pub controller_mode: ControllerMode,
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
    about = "Phase4 is a fast, lightweight audio analysis tool built for real-time audio visualisation."
)]
pub struct Args {
    #[command(flatten)]
    pub calibration: CalibrationArgs,

    #[command(flatten)]
    pub midi: MidiArgs,

    #[command(flatten)]
    pub input: InputArgs,

    #[command(flatten)]
    pub network: NetworkArgs,

    #[command(flatten)]
    pub vocoder: VocoderArgs,

    #[command(flatten)]
    pub runtime: RuntimeArgs,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hz_and_test_sweep_conflict() {
        let result = Args::try_parse_from(["phase4", "--test-hz", "440", "--test-sweep", "0.1"]);
        assert!(
            result.is_err(),
            "passing both calibration flags must be a CLI error"
        );
    }

    #[test]
    fn midi_device_and_test_bpm_conflict() {
        let result = Args::try_parse_from([
            "phase4",
            "--device",
            "x",
            "--midi-device",
            "Loopback",
            "--midi-test-bpm",
            "120.0",
        ]);
        assert!(
            result.is_err(),
            "both MIDI input flags together must be a CLI error"
        );
    }
}
