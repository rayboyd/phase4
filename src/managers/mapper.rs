//! [`Mapper`] sits between the [`crate::managers::analyser`] and the front-end
//! [`crate::managers::server`]. It receives the raw vocoder envelope bins and
//! maps them to a [`DISPLAY_BINS`]-bin representation for WebSocket broadcast.
//!
//! When the raw bin count exceeds [`DISPLAY_BINS`] the mapper averages adjacent
//! bins (downsampling). When it is lower the mapper spreads each raw bin across
//! multiple display slots (upsampling). An exact match is a direct copy.
//!
//! JSON serialisation happens here, once per frame, so the server tasks become
//! pure I/O forwarders. The watch channel carries a [`Utf8Bytes`] containing
//! the pre-serialised JSON rather than the typed payload.

use crate::app::AppState;
use crate::dsp::{DisplayChannelLevel, DisplayPayload, RawChannelLevel, RawPayload, DISPLAY_BINS};
use std::cmp::Ordering::{Equal, Greater, Less};
use std::sync::{atomic::Ordering, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Utf8Bytes;

pub struct Mapper;

impl Mapper {
    /// Spawns the mapper on a dedicated background thread.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned or if the single-threaded
    /// Tokio runtime cannot be built.
    pub fn spawn(
        raw_rx: watch::Receiver<RawPayload>,
        display_tx: watch::Sender<Utf8Bytes>,
        channels: usize,
        state: Arc<AppState>,
        broadcast_rate: Option<f32>,
    ) -> JoinHandle<()> {
        thread::Builder::new()
            .name("mapper".into())
            .spawn(move || {
                let broadcast_interval =
                    broadcast_rate.map(|hz| Duration::from_secs_f64(1.0 / f64::from(hz)));

                // Build the async runtime inside this dedicated OS thread.
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build tokio runtime for mapper")
                    .block_on(Self::run(
                        raw_rx,
                        display_tx,
                        channels,
                        state,
                        broadcast_interval,
                    ));
            })
            .expect("failed to spawn mapper thread")
    }

    async fn run(
        mut raw_rx: watch::Receiver<RawPayload>,
        display_tx: watch::Sender<Utf8Bytes>,
        channels: usize,
        state: Arc<AppState>,
        broadcast_interval: Option<Duration>,
    ) {
        // Pre-allocated buffer reused every frame to avoid per-frame heap allocation.
        let mut display_data = DisplayPayload::new(channels);
        let mut last_broadcast: Option<Instant> = None;

        while state.keep_running.load(Ordering::Acquire) {
            // Block efficiently until the Analyser publishes a new frame.
            if raw_rx.changed().await.is_err() {
                log::info!("Analyser channel closed, mapper exiting");
                break;
            }

            let raw = raw_rx.borrow_and_update();

            if raw.channels.is_empty() {
                continue;
            }

            for (ch_idx, channel) in raw.channels.iter().enumerate() {
                map_channel(channel, &mut display_data.channels[ch_idx]);
            }

            // Release the watch read-lock before sending.
            drop(raw);

            // When a broadcast rate is configured, skip serialisation and
            // transmission if the minimum interval has not yet elapsed. The
            // display buffer retains the latest mapped data, so the next
            // broadcast that does fire carries the most recent frame.
            if let Some(interval) = broadcast_interval {
                if let Some(last) = last_broadcast {
                    if last.elapsed() < interval {
                        continue;
                    }
                }
            }

            // Serialise once per frame. Every connected client receives a cheap
            // Utf8Bytes::clone rather than re-serialising or re-allocating independently.
            match serde_json::to_string(&display_data) {
                Ok(json) => {
                    display_tx.send_replace(Utf8Bytes::from(json));
                    last_broadcast = Some(Instant::now());
                }
                Err(e) => {
                    log::error!("Failed to serialise display payload: {e}");
                }
            }
        }
    }
}

/// Maps a single channel's raw vocoder envelope bins to display resolution.
///
/// Handles three cases based on the relationship between the raw bin count
/// and [`DISPLAY_BINS`]: downsampling (average), direct copy (equal), or
/// upsampling (spread).
fn map_channel(raw: &RawChannelLevel, out: &mut DisplayChannelLevel) {
    out.peak = raw.peak;

    let raw_len = raw.bins.len();

    match raw_len.cmp(&DISPLAY_BINS) {
        Equal => {
            out.bins.copy_from_slice(&raw.bins);
        }
        Greater => {
            // Downsample, average adjacent raw bins into each display slot.
            debug_assert_eq!(
                raw_len % DISPLAY_BINS,
                0,
                "raw bin count must be a multiple of DISPLAY_BINS when downsampling"
            );
            let chunk_size = raw_len / DISPLAY_BINS;
            for (i, chunk) in raw.bins.chunks_exact(chunk_size).enumerate() {
                let sum: f32 = chunk.iter().copied().sum();
                out.bins[i] = sum / chunk.len() as f32;
            }
        }
        Less => {
            // Upsample, spread each raw bin across multiple display slots.
            debug_assert_eq!(
                DISPLAY_BINS % raw_len,
                0,
                "DISPLAY_BINS must be a multiple of raw bin count when upsampling"
            );
            let spread = DISPLAY_BINS / raw_len;
            for (&val, chunk) in raw.bins.iter().zip(out.bins.chunks_exact_mut(spread)) {
                chunk.fill(val);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // When raw bin count equals DISPLAY_BINS the bins are copied exactly as is.
    #[test]
    #[allow(clippy::float_cmp)]
    fn map_channel_equal_copies_bins() {
        let raw = RawChannelLevel {
            peak: 0.0,
            bins: (0..DISPLAY_BINS).map(|i| i as f32 * 0.01).collect(),
        };
        let mut out = DisplayChannelLevel {
            peak: 0.0,
            bins: [0.0; DISPLAY_BINS],
        };

        map_channel(&raw, &mut out);

        for (i, &bin) in out.bins.iter().enumerate() {
            assert_eq!(bin, raw.bins[i], "bin {i} should be a direct copy");
        }
    }

    // When raw bin count exceeds DISPLAY_BINS, adjacent bins are averaged.
    // With the default feature (display-bins-32) and VOCODER_BANDS = 64,
    // each display slot is the mean of two raw bins.
    #[test]
    #[allow(clippy::float_cmp)]
    fn map_channel_downsample_averages_bins() {
        let raw_len = DISPLAY_BINS * 2;
        let raw = RawChannelLevel {
            peak: 0.0,
            bins: (0..raw_len).map(|i| i as f32).collect(),
        };
        let mut out = DisplayChannelLevel {
            peak: 0.0,
            bins: [0.0; DISPLAY_BINS],
        };

        map_channel(&raw, &mut out);

        for i in 0..DISPLAY_BINS {
            let expected = f32::midpoint(raw.bins[i * 2], raw.bins[i * 2 + 1]);
            assert_eq!(
                out.bins[i],
                expected,
                "display bin {i} should be the mean of raw bins {} and {}",
                i * 2,
                i * 2 + 1
            );
        }
    }

    // When raw bin count is less than DISPLAY_BINS, each raw bin is spread
    // across multiple display slots. With 16 raw bins and 32 display bins,
    // each raw value fills two consecutive slots.
    #[test]
    #[allow(clippy::float_cmp)]
    fn map_channel_upsample_spreads_bins() {
        let raw_len = DISPLAY_BINS / 2;
        let raw = RawChannelLevel {
            peak: 0.0,
            bins: (0..raw_len).map(|i| (i + 1) as f32 * 0.1).collect(),
        };
        let mut out = DisplayChannelLevel {
            peak: 0.0,
            bins: [0.0; DISPLAY_BINS],
        };

        map_channel(&raw, &mut out);

        let spread = DISPLAY_BINS / raw_len;
        for (raw_idx, &raw_val) in raw.bins.iter().enumerate() {
            for s in 0..spread {
                let display_idx = raw_idx * spread + s;
                assert_eq!(
                    out.bins[display_idx], raw_val,
                    "display bin {display_idx} should equal raw bin {raw_idx}"
                );
            }
        }
    }

    // The peak field is copied regardless of which mapping path is taken.
    #[test]
    #[allow(clippy::float_cmp)]
    fn map_channel_preserves_peak() {
        let cases: &[usize] = &[DISPLAY_BINS, DISPLAY_BINS * 2, DISPLAY_BINS / 2];

        for &raw_len in cases {
            let raw = RawChannelLevel {
                peak: 0.87,
                bins: vec![0.0; raw_len],
            };
            let mut out = DisplayChannelLevel {
                peak: 0.0,
                bins: [0.0; DISPLAY_BINS],
            };

            map_channel(&raw, &mut out);

            assert_eq!(
                out.peak, 0.87,
                "peak must pass through for raw_len={raw_len}"
            );
        }
    }

    // The serialised JSON must contain the expected top-level key, per-channel
    // keys, and the correct array lengths. This catches accidental schema drift
    // that would break clients parsing the wire format.
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
