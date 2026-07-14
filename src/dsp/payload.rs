//! Two payload types flow through the DSP pipeline:
//!
//! 1. **Raw**: [`RawChannelLevel`] / [`RawPayload`] carry the full-resolution
//!    vocoder envelope levels ([`VOCODER_BANDS`] bands). These are
//!    internal-only and never serialised.
//!
//! 2. **Display**: [`DisplayChannelLevel`] / [`DisplayPayload`] carry a
//!    [`DISPLAY_BINS`]-bin display mapping of those envelope levels and are
//!    serialised to JSON for WebSocket broadcast. [`DisplayPayload`] also
//!    carries an optional [`MidiSnapshot`] when MIDI input is configured.
//!
//! [`DISPLAY_BINS`] is set at compile time via a Cargo feature flag
//! (`display-bins-4`, `display-bins-8`, `display-bins-16`,
//! `display-bins-32`, `display-bins-64`, `display-bins-128`, or
//! `display-bins-256`). The mapper handles both downsampling and upsampling.
//! The vocoder's bin count must be an integer multiple of [`DISPLAY_BINS`],
//! or [`DISPLAY_BINS`] must be an integer multiple of the vocoder's bin count.

use crate::dsp::vocoder::VOCODER_BANDS;
use serde::{ser::SerializeStruct, Serialize, Serializer};

/// Display resolution (number of analysis bins) sent to frontend clients.
/// Selected at compile time via the `display-bins-*` feature flags.
#[cfg(feature = "display-bins-4")]
pub const DISPLAY_BINS: usize = 4;
#[cfg(feature = "display-bins-8")]
pub const DISPLAY_BINS: usize = 8;
#[cfg(feature = "display-bins-16")]
pub const DISPLAY_BINS: usize = 16;
#[cfg(feature = "display-bins-32")]
pub const DISPLAY_BINS: usize = 32;
#[cfg(feature = "display-bins-64")]
pub const DISPLAY_BINS: usize = 64;
#[cfg(feature = "display-bins-128")]
pub const DISPLAY_BINS: usize = 128;
#[cfg(feature = "display-bins-256")]
pub const DISPLAY_BINS: usize = 256;

#[cfg(not(any(
    feature = "display-bins-4",
    feature = "display-bins-8",
    feature = "display-bins-16",
    feature = "display-bins-32",
    feature = "display-bins-64",
    feature = "display-bins-128",
    feature = "display-bins-256",
)))]
compile_error!(
    "exactly one display-bins feature must be enabled: \
     display-bins-4, display-bins-8, display-bins-16, display-bins-32, \
     display-bins-64, display-bins-128, or display-bins-256"
);

#[cfg(any(
    all(feature = "display-bins-4", feature = "display-bins-8"),
    all(feature = "display-bins-4", feature = "display-bins-16"),
    all(feature = "display-bins-4", feature = "display-bins-32"),
    all(feature = "display-bins-4", feature = "display-bins-64"),
    all(feature = "display-bins-4", feature = "display-bins-128"),
    all(feature = "display-bins-4", feature = "display-bins-256"),
    all(feature = "display-bins-8", feature = "display-bins-16"),
    all(feature = "display-bins-8", feature = "display-bins-32"),
    all(feature = "display-bins-8", feature = "display-bins-64"),
    all(feature = "display-bins-8", feature = "display-bins-128"),
    all(feature = "display-bins-8", feature = "display-bins-256"),
    all(feature = "display-bins-16", feature = "display-bins-32"),
    all(feature = "display-bins-16", feature = "display-bins-64"),
    all(feature = "display-bins-16", feature = "display-bins-128"),
    all(feature = "display-bins-16", feature = "display-bins-256"),
    all(feature = "display-bins-32", feature = "display-bins-64"),
    all(feature = "display-bins-32", feature = "display-bins-128"),
    all(feature = "display-bins-32", feature = "display-bins-256"),
    all(feature = "display-bins-64", feature = "display-bins-128"),
    all(feature = "display-bins-64", feature = "display-bins-256"),
    all(feature = "display-bins-128", feature = "display-bins-256"),
))]
compile_error!(
    "display-bins features are mutually exclusive; enable exactly one of: \
    display-bins-4, display-bins-8, display-bins-16, display-bins-32, \
    display-bins-64, display-bins-128, or display-bins-256"
);

// Validate that the vocoder bin count is compatible with the chosen display
// resolution. Both downsample and upsample paths require an integer ratio.
const _: () = assert!(
    (VOCODER_BANDS >= DISPLAY_BINS && VOCODER_BANDS.is_multiple_of(DISPLAY_BINS))
        || (DISPLAY_BINS >= VOCODER_BANDS && DISPLAY_BINS.is_multiple_of(VOCODER_BANDS)),
    "VOCODER_BANDS and DISPLAY_BINS must be integer multiples of one another",
);

/// One channel's full-resolution vocoder output for a single analysis frame.
/// Never serialised, `bins.len()` is `VOCODER_BANDS` regardless of feature flags.
#[derive(Debug, Clone)]
pub struct RawChannelLevel {
    /// Peak absolute sample value for this channel over the analysis chunk.
    pub peak: f32,

    /// One envelope-follower value per vocoder band, low to high frequency.
    pub bins: Vec<f32>,
}

/// Vocoder output for every channel, published once per analysis frame.
/// Internal only, the mapper reduces this to a `DisplayPayload`.
#[derive(Debug, Clone, Default)]
pub struct RawPayload {
    /// One entry per audio channel, in hardware channel order.
    pub channels: Vec<RawChannelLevel>,
}

impl RawPayload {
    /// Allocates `channels` zeroed entries, each with `bin_count` bins.
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

/// One channel's display-resolution vocoder output. `bins.len()` is always `DISPLAY_BINS`,
/// written by the mapper's downsample or upsample path from a `RawChannelLevel`.
#[derive(Debug, Clone)]
pub struct DisplayChannelLevel {
    /// Peak absolute sample value for this channel, copied through unchanged
    /// from the source `RawChannelLevel`.
    pub peak: f32,
    /// One mapped envelope value per display bin, low to high frequency.
    pub bins: [f32; DISPLAY_BINS],
}

/// One frame's MIDI transport and step state, attached to `DisplayPayload`
/// only when MIDI input is configured.
#[derive(Debug, Clone, Serialize)]
pub struct MidiSnapshot {
    /// "start", "stop", or "continue" if a transport event happened since
    /// the previous broadcast frame. Omitted from JSON when nothing happened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<&'static str>,

    /// MIDI 1/16 note steps seen since the previous frame.
    pub steps: u32,
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

/// Display-resolution vocoder output for every channel, published once per
/// broadcast frame and serialised to JSON for WebSocket and OSC output.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DisplayPayload {
    /// One entry per audio channel, in hardware channel order.
    pub channels: Vec<DisplayChannelLevel>,

    /// Absent when MIDI input is not configured, so existing clients that
    /// only read channels see no schema change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi: Option<MidiSnapshot>,
}

impl DisplayPayload {
    /// Allocates `channels` zeroed entries with `midi` absent.
    #[must_use]
    pub fn new(channels: usize) -> Self {
        Self {
            channels: (0..channels)
                .map(|_| DisplayChannelLevel {
                    peak: 0.0,
                    bins: [0.0; DISPLAY_BINS],
                })
                .collect(),
            midi: None,
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

    // DisplayPayload carries no MIDI snapshot until MIDI input produces one.
    #[test]
    fn display_payload_midi_defaults_to_none() {
        assert!(DisplayPayload::new(1).midi.is_none());
    }

    // The transport key is omitted entirely, not sent as null, when nothing fired.
    #[test]
    fn midi_snapshot_omits_transport_when_none() {
        let snapshot = MidiSnapshot {
            transport: None,
            steps: 3,
        };
        let json = serde_json::to_string(&snapshot).expect("should serialise");
        assert!(!json.contains("transport"));
        assert!(json.contains("\"steps\":3"));
    }

    // Checks the full JSON shape, keys, channel count, bin length, so schema
    // drift that would break client parsers is caught here, not on the wire.
    #[test]
    fn serialisation_shape_matches_client_contract() {
        let payload = DisplayPayload::new(2);
        let json = serde_json::to_string(&payload).expect("serialisation should not fail");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("must be valid JSON");

        let channels = parsed["channels"]
            .as_array()
            .expect("top-level 'channels' must be an array");
        assert_eq!(channels.len(), 2, "channel count must match construction");

        for (i, channel) in channels.iter().enumerate() {
            assert!(
                channel.get("peak").is_some(),
                "channel {i} must contain 'peak'"
            );
            let bins = channel["bins"]
                .as_array()
                .unwrap_or_else(|| panic!("channel {i} must contain a 'bins' array"));
            assert_eq!(
                bins.len(),
                DISPLAY_BINS,
                "channel {i} bins array length must equal DISPLAY_BINS"
            );
        }
    }
}
