//! Each submodule owns one DSP concern and exposes types re-exported here.
//!
//! [`payload`] holds the raw and display payload types carried through the
//! pipeline. [`units`] holds the zero-cost `Hertz` and `Milliseconds`
//! newtypes. [`vocoder`] holds the vocoder filter bank and envelope
//! followers.

pub mod payload;
pub mod units;
pub mod vocoder;

/// Number of logarithmically spaced frequency bands analysed and broadcast.
pub const BAND_COUNT: usize = 32;

pub use payload::{ChannelLevel, DisplayPayload, MidiSnapshot, RawPayload};
pub use units::{Hertz, Milliseconds};
pub use vocoder::VocoderAnalyser;
