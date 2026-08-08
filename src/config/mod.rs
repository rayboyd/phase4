//! Configuration types, resolution, and validation.
//!
//! [`types`] holds the config types and errors, [`AppConfig`],
//! [`AppConfigError`], and the resolved and file-layer structs each field
//! passes through. [`resolve`] merges the CLI, file, and default layers
//! into an [`AppConfig`]. [`validate`] holds standalone validation, where
//! each function takes already-resolved values and returns
//! `Result<(), AppConfigError>`.

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
