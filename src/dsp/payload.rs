//! Two payload types flow through the DSP pipeline.
//!
//! [`RawChannelLevel`] / [`RawPayload`] carry the vocoder envelope levels.
//! These are internal-only and never serialised.
//!
//! [`DisplayChannelLevel`] / [`DisplayPayload`] carry the same 32 envelope
//! values in the serialisable output shape used by WebSocket and OSC.
//! [`DisplayPayload`] also carries an optional [`MidiSnapshot`] when MIDI
//! input is configured.

use crate::dsp::vocoder::VOCODER_BANDS;
use serde::{ser::SerializeStruct, Serialize, Serializer};

/// Number of analysis bins sent to clients for each channel.
pub const DISPLAY_BINS: usize = VOCODER_BANDS;

/// One channel's vocoder output for a single analysis frame.
/// Never serialised, `bins.len()` is always [`VOCODER_BANDS`].
#[derive(Debug, Clone)]
pub struct RawChannelLevel {
    /// Peak absolute sample value for this channel over the analysis chunk.
    pub peak: f32,

    /// One envelope-follower value per vocoder band, low to high frequency.
    pub bins: [f32; VOCODER_BANDS],
}

/// Vocoder output for every channel, published once per analysis frame.
/// Internal only, the mapper copies this into a `DisplayPayload` at 60 Hz.
#[derive(Debug, Clone, Default)]
pub struct RawPayload {
    /// One entry per audio channel, in hardware channel order.
    pub channels: Vec<RawChannelLevel>,
}

impl RawPayload {
    /// Allocates `channels` zeroed entries with 32 bins each.
    #[must_use]
    pub fn new(channels: usize) -> Self {
        Self {
            channels: (0..channels)
                .map(|_| RawChannelLevel {
                    peak: 0.0,
                    bins: [0.0; VOCODER_BANDS],
                })
                .collect(),
        }
    }
}

/// One channel's serialisable vocoder output. `bins.len()` is always
/// [`DISPLAY_BINS`], copied directly from a [`RawChannelLevel`].
#[derive(Debug, Clone)]
pub struct DisplayChannelLevel {
    /// Peak absolute sample value for this channel, copied through unchanged
    /// from the source `RawChannelLevel`.
    pub peak: f32,
    /// One envelope value per display bin, low to high frequency.
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

    /// Absolute count of MIDI 1/16 note steps since the most recent Start
    /// event. Monotonic across frames, reset only by Start.
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

/// Display vocoder output for every channel, published once per
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
        assert_eq!(RawPayload::new(0).channels.len(), 0);
        assert_eq!(RawPayload::new(1).channels.len(), 1);
        assert_eq!(RawPayload::new(2).channels.len(), 2);
    }

    // Each RawChannelLevel has the fixed bin count and all values are zero.
    #[test]
    #[allow(clippy::float_cmp)]
    fn raw_payload_bins_sized_and_zeroed() {
        let payload = RawPayload::new(2);
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
