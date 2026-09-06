//! Configuration types, resolution, and validation.
//!
//! The internal `types` module holds the config types and errors, [`AppConfig`],
//! [`AppConfigError`], and the resolved and file-layer structs each field
//! passes through. `resolve` merges the CLI, file, and default layers
//! into an [`AppConfig`]. `validate` checks resolved values and derives the
//! validated MIDI tick interval. Failures use [`AppConfigError`].

mod resolve;
mod types;
mod validate;

pub use types::{
    AppConfig, AppConfigError, ConfigInput, ConfigMidiInput, ConfigOutputs, FileAudioConfig,
    FileConfig, FileMidiConfig, FileNetworkConfig, FileVocoderConfig, OutputConfig, TestSignal,
    VocoderConfig, DEFAULT_MAX_CLIENTS,
};
pub(crate) use types::{CALIBRATION_FREQUENCY_CEILING_RATIO, CALIBRATION_SAMPLE_RATE_HZ};
pub(crate) use validate::{midi_tick_interval, validate_app_config, validate_vocoder_sample_rate};
