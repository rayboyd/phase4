//! Each submodule owns one DSP concern and exposes types re-exported here.
//!
//! Submodules:
//! - [`payload`]: raw and display payload types carried through the pipeline.
//! - [`units`]: zero-cost `Hertz` and `Milliseconds` newtypes.
//! - [`vocoder`]: the vocoder filter bank and envelope followers.

pub mod payload;
pub mod units;
pub mod vocoder;

pub use payload::{
    DisplayChannelLevel, DisplayPayload, MidiSnapshot, RawChannelLevel, RawPayload, DISPLAY_BINS,
};
pub use units::{Hertz, Milliseconds};
pub use vocoder::VocoderAnalyser;
