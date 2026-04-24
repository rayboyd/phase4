//! [`Input`] wraps a `cpal::Stream` and provides two methods:
//! [`Input::get_device`], which queries the hardware configuration without
//! starting a stream, and [`Input::start_stream`], which binds the device to
//! two SPSC ringbuf producers, one for the recorder and one for the analyser.
//!
//! The stream callback pushes f32 frames to both producers. If the record
//! producer cannot accept the full slice, one record ring overflow event is counted
//! so the controller can surface a warning to the operator.
//!
//! [`Specs`] carries the hardware's native channel count and sample rate and
//! is used throughout the pipeline for buffer sizing.

use crate::app::AppState;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use ringbuf::traits::{Producer, Split};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Captured hardware info used for buffer sizing logic.
#[derive(Clone, Copy)]
pub struct Specs {
    pub channels: u16,
    pub sample_rate: u32,
}

impl Specs {
    /// Returns the number of samples needed to cover `ms` milliseconds,
    /// across all channels, at the card's native sample rate.
    #[must_use]
    pub fn samples_for_ms(&self, ms: u32) -> usize {
        // Each as usize cast widens the u32/u16 inputs to 64-bit before the multiply.
        // Without that, 192000_u32 * 16_u32 * 3_600_000_u32 would overflow u32::MAX (4,294,967,295).
        (self.sample_rate as usize * self.channels as usize * ms as usize) / 1000
    }
}

/// Describes which channels to extract from the hardware interleaved stream.
///
/// `All` preserves the current `push_slice` fast path and is used when no
/// channel selection is specified at startup. `Selected` carries a sorted,
/// deduplicated list of zero-based hardware channel indices. Both variants are
/// constructed once before stream start and moved into the closure; there are
/// no allocations or atomic ref-count touches at callback time.
pub enum ChannelMode {
    All,
    Selected(Box<[u16]>),
}

/// Pairs a ring buffer producer with the channel selection for that sink.
///
/// Constructed once before stream start and moved into the audio callback
/// closure. `mode` determines whether all hardware channels are forwarded or
/// only a selected subset. `tx` is the SPSC producer for the downstream
/// consumer.
pub struct StreamSink<P> {
    pub tx: P,
    pub mode: ChannelMode,
}

#[derive(Default)]
pub struct Input {
    active_stream: Option<cpal::Stream>,
}

impl Input {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a producer and consumer pair sized for approximately `buffer_ms`
    /// milliseconds of interleaved audio at `specs`.
    ///
    /// The exact sample count is rounded up to the next power of two so the
    /// underlying ring buffer can use bitmask wrapping in the audio callback hot
    /// path. This means the actual capacity may be larger than the exact duration
    /// requested.
    ///
    /// # Panics
    ///
    /// Panics if `buffer_ms` is 0.
    #[must_use]
    pub fn create_audio_buffer_pair(
        specs: Specs,
        buffer_ms: u32,
    ) -> (ringbuf::HeapProd<f32>, ringbuf::HeapCons<f32>) {
        assert!(buffer_ms > 0);

        let samples_per_sec = specs.sample_rate as usize * specs.channels as usize;
        let capacity = (samples_per_sec * buffer_ms as usize) / 1000;

        // Power-of-two capacity enables bitmask wrapping (index & (len-1)) inside
        // the ringbuf, replacing modulo division on every push/pop. In the audio
        // callback (hot path) integer division has variable latency that risks
        // buffer underruns, where a single AND instruction is constant-time.
        ringbuf::HeapRb::<f32>::new(capacity.next_power_of_two()).split()
    }

    /// Retrieves a concrete handle to the device and its default info. This allows
    /// the App to size ringbufs before starting the stream callback (hotpath).
    ///
    /// # Errors
    ///
    /// Returns an error if the device index is out of range or the hardware
    /// configuration cannot be queried.
    pub fn get_device(
        &self,
        index: usize,
    ) -> Result<(cpal::Device, cpal::SupportedStreamConfig, Specs)> {
        let host = cpal::default_host();
        let device = host
            .input_devices()?
            .nth(index)
            .context("Device index not found")?;

        let config = device
            .default_input_config()
            .context("Failed to query hardware config")?;

        let specs = Specs {
            sample_rate: config.sample_rate(),
            channels: config.channels(),
        };

        Ok((device, config, specs))
    }

    /// Binds the device to the record and analyse SPSC producers.
    ///
    /// # Errors
    ///
    /// Returns an error if the device does not support `f32` sample format,
    /// or if the input stream cannot be built or started.
    pub fn start_stream<P>(
        &mut self,
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
        mut record: StreamSink<P>,
        mut analyse: StreamSink<P>,
        state: Arc<AppState>,
    ) -> Result<()>
    where
        P: Producer<Item = f32> + Send + 'static,
    {
        if config.sample_format() != SampleFormat::F32 {
            anyhow::bail!(
                "Device reports {} sample format; phase4 requires f32 input. \
                 Most professional audio interfaces deliver f32 natively. \
                 Run with --list to see available devices.",
                config.sample_format()
            );
        }

        let error_state = state.clone();
        let stream_config = config.config();

        // Captured once at stream construction. Never touched again inside the callback.
        let hw_channels = stream_config.channels as usize;

        let stream = device.build_input_stream(
            &stream_config,
            // This callback runs on cpal's dedicated audio thread at hardware interrupt
            // rate. It must be lock-free, allocation-free, and non-blocking; any stall
            // here will cause a buffer underrun and an audible glitch.
            move |data: &[f32], _| {
                // Record path: lossless. One overflow event counted per callback
                // invocation that drops any sample, matching existing semantics.
                match &record.mode {
                    ChannelMode::All => {
                        if record.tx.push_slice(data) < data.len() {
                            state
                                .record_ring_overflow_events
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    ChannelMode::Selected(indices) => {
                        let mut overflowed = false;
                        for frame in data.chunks_exact(hw_channels) {
                            for &idx in indices {
                                if record.tx.try_push(frame[idx as usize]).is_err() {
                                    overflowed = true;
                                }
                            }
                        }
                        if overflowed {
                            state
                                .record_ring_overflow_events
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }

                // Analyse path is intentionally lossy. A dropped analysis frame is
                // invisible; a dropped recording frame is not.
                match &analyse.mode {
                    ChannelMode::All => {
                        let _ = analyse.tx.push_slice(data);
                    }
                    ChannelMode::Selected(indices) => {
                        for frame in data.chunks_exact(hw_channels) {
                            for &idx in indices {
                                let _ = analyse.tx.try_push(frame[idx as usize]);
                            }
                        }
                    }
                }
            },
            move |err| {
                log::error!("Hardware Stream Error: {err}");
                error_state.keep_running.store(false, Ordering::Release);
            },
            None,
        )?;

        stream.play()?;
        self.active_stream = Some(stream);

        Ok(())
    }

    /// Queries the system for all available audio input devices.
    ///
    /// # Errors
    ///
    /// Returns an error if the host audio system cannot enumerate input devices.
    pub fn list_devices() -> Result<()> {
        let host = cpal::default_host();
        let devices = host
            .input_devices()
            .context("Failed to query input devices")?;

        let mut devices_found = false;
        for (index, device) in devices.enumerate() {
            devices_found = true;
            let name = device
                .description()
                .map_or_else(|_| "Unknown Device".to_string(), |d| d.name().to_string());

            if let Ok(config) = device.default_input_config() {
                log::info!(
                    "[{}] {} ({}Hz, {}ch, {})",
                    index,
                    name,
                    config.sample_rate(),
                    config.channels(),
                    config.sample_format()
                );
            } else {
                log::warn!("[{index}] {name} (Configuration unavailable)");
            }
        }

        if !devices_found {
            log::warn!("[*] No input devices detected. Check system permissions");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::Observer;

    fn check_samples_for_ms(sample_rate: u32, channels: u16, ms: u32, expected: usize) {
        let specs = Specs {
            channels,
            sample_rate,
        };
        assert_eq!(
            specs.samples_for_ms(ms),
            expected,
            "arithmetic mismatch: sample_rate={sample_rate}, channels={channels}, ms={ms}",
        );
    }

    // Zero duration must always produce zero samples regardless of rate or channels.
    #[test]
    fn samples_for_ms_zero_duration_returns_zero() {
        check_samples_for_ms(48000, 2, 0, 0);
        check_samples_for_ms(192_000, 8, 0, 0);
    }

    // One full second at CD and DAT rates produces an exact round number.
    #[test]
    fn samples_for_ms_one_second_is_exact() {
        check_samples_for_ms(44100, 2, 1000, 88200);
        check_samples_for_ms(48000, 2, 1000, 96000);
    }

    // Mono at common sample rates, 10 ms chunk (the pipeline's CHUNK_SIZE_MS).
    #[test]
    fn samples_for_ms_mono() {
        check_samples_for_ms(48000, 1, 10, 480);
        check_samples_for_ms(44100, 1, 10, 441);
        check_samples_for_ms(96000, 1, 1, 96);
    }

    // Stereo at every rate the Duet 3 supports, 10 ms chunk.
    #[test]
    fn samples_for_ms_stereo_standard_rates() {
        check_samples_for_ms(44100, 2, 10, 882);
        check_samples_for_ms(48000, 2, 10, 960);
        check_samples_for_ms(88200, 2, 10, 1764);
        check_samples_for_ms(96000, 2, 10, 1920);
        check_samples_for_ms(176_400, 2, 10, 3528);
        check_samples_for_ms(192_000, 2, 10, 3840);
    }

    // Multi-channel layouts (5.1 and 7.1).
    #[test]
    fn samples_for_ms_multichannel() {
        check_samples_for_ms(96000, 8, 1, 768);
        check_samples_for_ms(48000, 6, 10, 2880);
    }

    // The .1 / .2 / .4 kHz rates do not divide evenly at 1 ms.
    // The result is truncated, not rounded.
    #[test]
    fn samples_for_ms_truncates_fractional_samples() {
        // 44100 * 2 * 1 / 1000 = 88.2 -> 88
        check_samples_for_ms(44100, 2, 1, 88);
        // 22050 * 1 * 3 / 1000 = 66.15 -> 66
        check_samples_for_ms(22050, 1, 3, 66);
        // 88200 * 2 * 1 / 1000 = 176.4 -> 176
        check_samples_for_ms(88200, 2, 1, 176);
        // 176400 * 2 * 1 / 1000 = 352.8 -> 352
        check_samples_for_ms(176_400, 2, 1, 352);
    }

    // 192 kHz, 16 channels, 1 hour. The intermediate product exceeds u32::MAX,
    // confirming the usize widening is necessary on 64-bit targets.
    #[test]
    fn samples_for_ms_large_duration_no_overflow() {
        check_samples_for_ms(192_000, 16, 3_600_000, 11_059_200_000);
    }

    #[test]
    fn create_audio_buffer_pair_keeps_exact_power_of_two_capacity() {
        let specs = Specs {
            sample_rate: 32_000,
            channels: 1,
        };

        let (p, c) = Input::create_audio_buffer_pair(specs, 1);

        assert_eq!(p.capacity().get(), 32);
        assert_eq!(p.vacant_len(), 32);
        assert_eq!(c.occupied_len(), 0);
    }

    #[test]
    fn create_audio_buffer_pair_rounds_up_to_next_power_of_two() {
        let specs = Specs {
            sample_rate: 48_000,
            channels: 2,
        };

        let (p, c) = Input::create_audio_buffer_pair(specs, 10);

        assert_eq!(p.capacity().get(), 1_024);
        assert_eq!(p.vacant_len(), 1_024);
        assert_eq!(c.occupied_len(), 0);
    }

    #[test]
    fn create_audio_buffer_pair_counts_all_channels_when_sizing() {
        let specs = Specs {
            sample_rate: 48_000,
            channels: 6,
        };

        let (p, _) = Input::create_audio_buffer_pair(specs, 10);

        assert_eq!(p.capacity().get(), 4_096);
    }
}
