//! Configuration: types, resolution, and validation.
//!
//! Submodules:
//! - [`types`]: config types and errors, [`AppConfig`], [`AppConfigError`],
//!   and the resolved and file-layer structs each field passes through.
//! - [`resolve`]: merges the CLI, file, and default layers into an
//!   [`AppConfig`].
//! - [`validate`]: standalone validation, each function takes
//!   already-resolved values and returns `Result<(), AppConfigError>`.

mod resolve;
mod types;
mod validate;

pub use types::{
    AppConfig, AppConfigError, ConfigInput, ConfigMidiInput, ConfigOutputs, FileAudioConfig,
    FileConfig, FileMidiConfig, FileNetworkConfig, FileVocoderConfig, OutputConfig, TestSignal,
    VocoderConfig, DEFAULT_MAX_CLIENTS,
};
pub(crate) use validate::validate_vocoder_sample_rate;
