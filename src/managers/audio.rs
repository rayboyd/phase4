//! [`Input`] wraps a `cpal::Stream` and provides two methods:
//! [`Input::get_device`], which queries the hardware configuration without
//! starting a stream, and [`Input::start_stream`], which binds the device to
//! two SPSC ringbuf producers, one for the recorder and one for the analyser.
//!
//! The stream callback pushes f32 frames to both producers. If the record
//! producer cannot accept the full slice, one record overrun event is
//! counted so the controller can surface a warning to the console.
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
/// constructed once before stream start and moved into the closure. There are
/// no allocations or atomic ref-count touches at callback time.
pub enum ChannelMode {
    All,
    Selected(Box<[u16]>),
}

impl ChannelMode {
    /// Helper to update the effective specs and resolve the `ChannelMode`.
    pub fn resolve(selection: Option<Box<[u16]>>, specs: &mut Specs) -> Self {
        if let Some(indices) = selection {
            specs.channels = indices.len() as u16;
            Self::Selected(indices)
        } else {
            Self::All
        }
    }
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

impl<P: Producer<Item = f32>> StreamSink<P> {
    /// Pushes audio data into the sink, applying the channel selection mode.
    ///
    /// `hw_channels` is the total interleaved channel count from cpal. It is
    /// only used in the `Selected` path to stride across frames.
    ///
    /// Returns `true` if any sample could not be written to the ring buffer. In
    /// the `All` path this means the slice was only partially accepted. In the
    /// `Selected` path it means at least one `try_push` failed.
    pub fn push(&mut self, data: &[f32], hw_channels: usize) -> bool {
        match &self.mode {
            ChannelMode::All => self.tx.push_slice(data) < data.len(),
            ChannelMode::Selected(indices) => {
                let mut dropped = false;
                for frame in data.chunks_exact(hw_channels) {
                    for &idx in indices {
                        if self.tx.try_push(frame[idx as usize]).is_err() {
                            dropped = true;
                        }
                    }
                }
                dropped
            }
        }
    }
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

    /// Returns a list of available audio input devices as structured data.
    ///
    /// Each entry is `(index, name, sample_rate, channels)`. Devices whose
    /// hardware configuration cannot be queried are omitted.
    ///
    /// # Errors
    ///
    /// Returns an error if the host audio system cannot enumerate input devices.
    pub fn enumerate_devices() -> Result<Vec<(usize, String, u32, u16)>> {
        let host = cpal::default_host();
        let devices = host
            .input_devices()
            .context("Failed to query input devices")?;

        let mut result = Vec::new();
        for (index, device) in devices.enumerate() {
            let name = device
                .description()
                .map_or_else(|_| "Unknown Device".to_string(), |d| d.name().to_string());

            if let Ok(config) = device.default_input_config() {
                if config.sample_format() == cpal::SampleFormat::F32 {
                    result.push((index, name, config.sample_rate(), config.channels()));
                }
            }
        }

        Ok(result)
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
                let format = config.sample_format();
                let is_f32 = format == cpal::SampleFormat::F32;
                let status = if is_f32 {
                    ""
                } else {
                    "* No hardware support (32-bit required)"
                };

                log::info!(
                    "[{}] {} ({}Hz, {}ch, {:?}) {}",
                    index,
                    name,
                    config.sample_rate(),
                    config.channels(),
                    format,
                    status
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
            // rate. It must be lock-free, allocation-free, and non-blocking. Any stall
            // here will cause a buffer underrun and an audible glitch.
            move |data: &[f32], _| {
                // Record path: lossless. One overflow event counted per callback
                // invocation that drops any sample, matching existing semantics.
                if record.push(data, hw_channels) {
                    state
                        .record_ring_overflow_events
                        .fetch_add(1, Ordering::Relaxed);
                }

                // Analyse path is intentionally lossy. A dropped analysis frame is
                // invisible; a dropped recording frame is not.
                let _ = analyse.push(data, hw_channels);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::{Consumer, Observer};

    fn make_ring(capacity: usize) -> (ringbuf::HeapProd<f32>, ringbuf::HeapCons<f32>) {
        ringbuf::HeapRb::<f32>::new(capacity).split()
    }

    fn drain(mut c: ringbuf::HeapCons<f32>) -> Vec<f32> {
        let mut out = Vec::new();
        while let Some(s) = c.try_pop() {
            out.push(s);
        }
        out
    }

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

    fn make_sink(capacity: usize, mode: ChannelMode) -> StreamSink<ringbuf::HeapProd<f32>> {
        let (tx, _) = make_ring(capacity);
        StreamSink { tx, mode }
    }

    fn make_sink_with_consumer(
        capacity: usize,
        mode: ChannelMode,
    ) -> (StreamSink<ringbuf::HeapProd<f32>>, ringbuf::HeapCons<f32>) {
        let (tx, rx) = make_ring(capacity);
        (StreamSink { tx, mode }, rx)
    }

    // All mode forwards every sample unchanged and reports no overflow.
    #[test]
    fn push_all_forwards_all_samples() {
        let (mut sink, c) = make_sink_with_consumer(8, ChannelMode::All);
        let data = [1.0_f32, 2.0, 3.0, 4.0];
        let dropped = sink.push(&data, 2);
        assert!(!dropped);
        assert_eq!(drain(c), &[1.0, 2.0, 3.0, 4.0]);
    }

    // All mode returns true when the ring cannot accept the full slice.
    #[test]
    fn push_all_reports_overflow_when_full() {
        let mut sink = make_sink(2, ChannelMode::All);
        sink.push(&[1.0, 2.0], 2);
        let dropped = sink.push(&[3.0, 4.0], 2);
        assert!(dropped);
    }

    // Selected([0]) extracts only channel 0 from each frame of a 4-channel stream.
    #[test]
    fn push_selected_extracts_first_channel() {
        // 2 frames * 4 channels: [ch0, ch1, ch2, ch3, ch0, ch1, ch2, ch3]
        let data = [1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let (mut sink, c) = make_sink_with_consumer(16, ChannelMode::Selected(Box::new([0])));
        let dropped = sink.push(&data, 4);
        assert!(!dropped);
        assert_eq!(drain(c), &[1.0, 5.0]);
    }

    // Selected([1, 3]) extracts both channels in frame order.
    #[test]
    fn push_selected_extracts_two_channels_in_order() {
        let data = [1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let (mut sink, c) = make_sink_with_consumer(16, ChannelMode::Selected(Box::new([1, 3])));
        let dropped = sink.push(&data, 4);
        assert!(!dropped);
        // frame 0: ch1=2.0, ch3=4.0 | frame 1: ch1=6.0, ch3=8.0
        assert_eq!(drain(c), &[2.0, 4.0, 6.0, 8.0]);
    }

    // Selected on a 1-channel stream produces the same output as All.
    #[test]
    fn push_selected_single_channel_matches_all() {
        let data = [0.1_f32, 0.2, 0.3];
        let (mut sink_all, c_all) = make_sink_with_consumer(16, ChannelMode::All);
        let (mut sink_sel, c_sel) =
            make_sink_with_consumer(16, ChannelMode::Selected(Box::new([0])));
        sink_all.push(&data, 1);
        sink_sel.push(&data, 1);
        assert_eq!(drain(c_all), drain(c_sel));
    }

    // Selected returns true when the ring fills mid-callback.
    #[test]
    fn push_selected_reports_overflow_when_full() {
        // 2 frames of 2 channels, selecting both = 4 samples, ring holds 2.
        let data = [1.0_f32, 2.0, 3.0, 4.0];
        let mut sink = make_sink(2, ChannelMode::Selected(Box::new([0, 1])));
        let dropped = sink.push(&data, 2);
        assert!(dropped);
    }

    // Remainder samples (data.len() not a multiple of hw_channels) are silently ignored.
    // This is cpal's contract: the callback always delivers complete frames.
    #[test]
    fn push_selected_ignores_partial_trailing_frame() {
        // 9 samples with hw_channels=4: 2 full frames + 1 orphan sample.
        let data = [1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let (mut sink, c) = make_sink_with_consumer(16, ChannelMode::Selected(Box::new([0])));
        sink.push(&data, 4);
        assert_eq!(drain(c), &[1.0, 5.0]);
    }

    // Empty slice produces no pushes and no overflow for either mode.
    #[test]
    fn push_empty_data_produces_nothing() {
        let (mut sink_all, c_all) = make_sink_with_consumer(8, ChannelMode::All);
        let (mut sink_sel, c_sel) =
            make_sink_with_consumer(8, ChannelMode::Selected(Box::new([0])));
        let dropped_all = sink_all.push(&[], 2);
        let dropped_sel = sink_sel.push(&[], 2);
        assert!(!dropped_all);
        assert!(!dropped_sel);
        assert_eq!(drain(c_all), Vec::<f32>::new());
        assert_eq!(drain(c_sel), Vec::<f32>::new());
    }
}
