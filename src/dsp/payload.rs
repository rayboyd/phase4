//! Payload types carried between analysis, scheduling, and output transports.
//!
//! [`RawPayload`] carries each 32-band analysis snapshot to the mapper.
//! [`DisplayPayload`] carries the same channel data plus an optional
//! [`MidiSnapshot`] to WebSocket and OSC outputs.

use crate::dsp::BAND_COUNT;
use serde::Serialize;

/// One channel's peak and 32 logarithmically spaced frequency bands.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ChannelLevel {
    /// Peak absolute sample value over the latest analysis chunk, not clamped.
    /// Intermediate chunk peaks can be skipped before display publication.
    pub peak: f32,

    /// Unnormalised envelope values ordered from low to high frequency.
    /// Values include filter gain and can exceed one.
    pub bins: [f32; BAND_COUNT],
}

/// Analysis output for every channel, published once per analysis frame.
#[derive(Debug, Clone, Default)]
pub struct RawPayload {
    /// One entry per analysed channel, in selected-channel order.
    /// Entries are contiguous output positions, not hardware channel indices.
    pub channels: Vec<ChannelLevel>,
}

impl RawPayload {
    /// Allocates zeroed analysis data for `channels` audio channels.
    #[must_use]
    pub fn new(channels: usize) -> Self {
        Self {
            channels: vec![ChannelLevel::default(); channels],
        }
    }
}

/// One frame's MIDI transport and step state.
#[derive(Debug, Clone, Serialize)]
pub struct MidiSnapshot {
    /// Last "start", "stop", or "continue" event since the previous mapper
    /// publication. Omitted from JSON when none is pending. Events can be
    /// overwritten before publication or skipped by a downstream receiver.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<&'static str>,

    /// MIDI 1/16 note steps counted since Start or initialisation.
    /// Resets on Start and wraps on u32 overflow. Stop does not gate counting.
    pub steps: u32,
}

/// Output snapshots scheduled at 60 Hz for WebSocket and OSC.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DisplayPayload {
    /// One entry per analysed channel, in selected-channel order.
    /// Entries are contiguous output positions, not hardware channel indices.
    pub channels: Vec<ChannelLevel>,

    /// Absent when MIDI input is not configured or before the first
    /// mapper publication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub midi: Option<MidiSnapshot>,
}

impl DisplayPayload {
    /// Allocates zeroed output data for `channels` audio channels.
    #[must_use]
    pub fn new(channels: usize) -> Self {
        Self {
            channels: vec![ChannelLevel::default(); channels],
            midi: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn raw_payload_initialises_zeroed_channels() {
        let payload = RawPayload::new(2);

        assert_eq!(payload.channels.len(), 2);
        for channel in &payload.channels {
            assert_eq!(channel.peak, 0.0);
            assert!(channel.bins.iter().all(|&bin| bin == 0.0));
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn display_payload_initialises_zeroed_channels_without_midi() {
        let payload = DisplayPayload::new(2);

        assert_eq!(payload.channels.len(), 2);
        assert!(payload.midi.is_none());
        for channel in &payload.channels {
            assert_eq!(channel.peak, 0.0);
            assert!(channel.bins.iter().all(|&bin| bin == 0.0));
        }
    }

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

    #[test]
    fn serialisation_shape_matches_client_contract() {
        let payload = DisplayPayload::new(2);
        let json = serde_json::to_string(&payload).expect("serialisation should not fail");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("must be valid JSON");

        let channels = parsed["channels"]
            .as_array()
            .expect("top-level 'channels' must be an array");
        assert_eq!(channels.len(), 2, "channel count must match construction");

        for (index, channel) in channels.iter().enumerate() {
            assert!(
                channel.get("peak").is_some(),
                "channel {index} must contain 'peak'"
            );
            let bins = channel["bins"]
                .as_array()
                .unwrap_or_else(|| panic!("channel {index} must contain a 'bins' array"));
            assert_eq!(
                bins.len(),
                BAND_COUNT,
                "channel {index} must contain {BAND_COUNT} bins"
            );
        }
    }
}
