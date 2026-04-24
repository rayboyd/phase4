//! Two payload types flow through the DSP pipeline:
//!
//! 1. **Raw**: [`RawChannelLevel`] / [`RawPayload`] carry the full-resolution
//!    vocoder envelope levels (64 bands). These are
//!    internal-only and never serialised.
//!
//! 2. **Display**: [`DisplayChannelLevel`] / [`DisplayPayload`] carry a
//!    [`DISPLAY_BINS`]-bin display mapping of those envelope levels and are
//!    serialised to JSON for WebSocket broadcast.
//!
//! [`DISPLAY_BINS`] is set at compile time via a Cargo feature flag
//! (`display-bins-32`, `display-bins-64`, `display-bins-128`, or
//! `display-bins-256`). The mapper handles both downsampling and upsampling.
//! The vocoder's bin count must be an integer multiple of [`DISPLAY_BINS`],
//! or [`DISPLAY_BINS`] must be an integer multiple of the vocoder's bin count.

use crate::dsp::vocoder::VOCODER_BANDS;
use serde::{ser::SerializeStruct, Serialize, Serializer};

/// Display resolution (number of analysis bins) sent to frontend clients.
/// Selected at compile time via the `display-bins-*` feature flags.
#[cfg(feature = "display-bins-32")]
pub const DISPLAY_BINS: usize = 32;
#[cfg(feature = "display-bins-64")]
pub const DISPLAY_BINS: usize = 64;
#[cfg(feature = "display-bins-128")]
pub const DISPLAY_BINS: usize = 128;
#[cfg(feature = "display-bins-256")]
pub const DISPLAY_BINS: usize = 256;

#[cfg(not(any(
    feature = "display-bins-32",
    feature = "display-bins-64",
    feature = "display-bins-128",
    feature = "display-bins-256",
)))]
compile_error!(
    "exactly one display-bins feature must be enabled: \
     display-bins-32, display-bins-64, display-bins-128, or display-bins-256"
);

#[cfg(any(
    all(feature = "display-bins-32", feature = "display-bins-64"),
    all(feature = "display-bins-32", feature = "display-bins-128"),
    all(feature = "display-bins-32", feature = "display-bins-256"),
    all(feature = "display-bins-64", feature = "display-bins-128"),
    all(feature = "display-bins-64", feature = "display-bins-256"),
    all(feature = "display-bins-128", feature = "display-bins-256"),
))]
compile_error!(
    "display-bins features are mutually exclusive; enable exactly one of: \
     display-bins-32, display-bins-64, display-bins-128, or display-bins-256"
);

// Validate that the vocoder bin count is compatible with the chosen display
// resolution. Both downsample and upsample paths require an integer ratio.
const _: () = assert!(
    (VOCODER_BANDS >= DISPLAY_BINS && VOCODER_BANDS.is_multiple_of(DISPLAY_BINS))
        || (DISPLAY_BINS >= VOCODER_BANDS && DISPLAY_BINS.is_multiple_of(VOCODER_BANDS)),
    "VOCODER_BANDS and DISPLAY_BINS must be integer multiples of one another",
);

//
// Raw Payload (Internal)
// Produced by the Analyser thread. Full resolution, not serialised.

#[derive(Debug, Clone)]
pub struct RawChannelLevel {
    pub peak: f32,
    pub bins: Vec<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct RawPayload {
    pub channels: Vec<RawChannelLevel>,
}

impl RawPayload {
    #[must_use]
    pub fn new(channels: usize, bin_count: usize) -> Self {
        Self {
            channels: (0..channels)
                .map(|_| RawChannelLevel {
                    peak: 0.0,
                    bins: vec![0.0; bin_count],
                })
                .collect(),
        }
    }
}

//
// Display Payload (Broadcast)
// Produced by the Mapper thread. Reduced resolution, serialised for the WebSocket.

#[derive(Debug, Clone)]
pub struct DisplayChannelLevel {
    pub peak: f32,
    pub bins: [f32; DISPLAY_BINS],
}

impl Serialize for DisplayChannelLevel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("DisplayChannelLevel", 2)?;
        s.serialize_field("peak", &self.peak)?;
        // Slice the array so Serde sees &[f32] rather than [f32; N].
        s.serialize_field("bins", &self.bins[..])?;
        s.end()
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DisplayPayload {
    pub channels: Vec<DisplayChannelLevel>,
}

impl DisplayPayload {
    #[must_use]
    pub fn new(channels: usize) -> Self {
        Self {
            channels: (0..channels)
                .map(|_| DisplayChannelLevel {
                    peak: 0.0,
                    bins: [0.0; DISPLAY_BINS],
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // RawPayload::new allocates the correct number of channels.
    #[test]
    fn raw_payload_channel_count() {
        assert_eq!(RawPayload::new(0, VOCODER_BANDS).channels.len(), 0);
        assert_eq!(RawPayload::new(1, VOCODER_BANDS).channels.len(), 1);
        assert_eq!(RawPayload::new(2, VOCODER_BANDS).channels.len(), 2);
    }

    // Each RawChannelLevel has the requested bin count and all values are zero.
    #[test]
    #[allow(clippy::float_cmp)]
    fn raw_payload_bins_sized_and_zeroed() {
        let payload = RawPayload::new(2, VOCODER_BANDS);
        for ch in &payload.channels {
            assert_eq!(ch.bins.len(), VOCODER_BANDS);
            assert_eq!(ch.peak, 0.0);
            assert!(ch.bins.iter().all(|&b| b == 0.0));
        }
    }

    // DisplayPayload::new allocates the correct number of channels.
    #[test]
    fn display_payload_channel_count() {
        assert_eq!(DisplayPayload::new(0).channels.len(), 0);
        assert_eq!(DisplayPayload::new(1).channels.len(), 1);
        assert_eq!(DisplayPayload::new(2).channels.len(), 2);
    }

    // Each DisplayChannelLevel has DISPLAY_BINS bins and all values are zero.
    #[test]
    #[allow(clippy::float_cmp)]
    fn display_payload_bins_sized_and_zeroed() {
        let payload = DisplayPayload::new(2);
        for ch in &payload.channels {
            assert_eq!(ch.bins.len(), DISPLAY_BINS);
            assert_eq!(ch.peak, 0.0);
            assert!(ch.bins.iter().all(|&b| b == 0.0));
        }
    }
}
