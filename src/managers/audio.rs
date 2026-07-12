//! [`Input`] wraps a `cpal::Stream` and provides two methods:
//! [`Input::get_device`], which queries the hardware configuration without
//! starting a stream, and [`Input::start_stream`], which binds the device to
//! an SPSC ringbuf producer for the analyser.
//!
//! The stream callback pushes f32 frames to the analyser producer. Dropped
//! analysis frames are intentionally tolerated as a missed frame is invisible
//! to the user.
//!
//! [`Specs`] carries the hardware's native channel count and sample rate and
//! is used throughout the pipeline for buffer sizing.

use crate::app::AppState;
use crate::ListFormat;
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

/// A single enumerated input device, serialised as one entry in the JSON
/// array produced by `--audio-list-format json`.
///
/// `sample_rate`, `channels`, and `sample_format` are `None` when the
/// device's hardware configuration could not be queried, the text-mode
/// equivalent is the "Configuration unavailable" warning line.
#[derive(serde::Serialize)]
struct DeviceInfo {
    index: usize,
    name: String,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    sample_format: Option<String>,
    supported: bool,
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

    /// Queries the system for all available audio input devices and prints them
    /// in the requested format.
    ///
    /// # Errors
    ///
    /// Returns an error if the host audio system cannot enumerate input devices,
    /// or if the JSON encoding of the device list fails.
    pub fn list_devices(format: ListFormat) -> Result<()> {
        match format {
            ListFormat::Text => Self::list_devices_text(),
            ListFormat::Json => Self::list_devices_json(),
        }
    }

    /// Human-readable device listing via `log`, one line per device.
    fn list_devices_text() -> Result<()> {
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

    /// Structured device listing as a single JSON array on stdout.
    ///
    /// Nothing else is written to stdout in this mode, `log` output continues to
    /// go to stderr as normal, so a wrapper process can read stdout directly
    /// without filtering out anything else.
    fn list_devices_json() -> Result<()> {
        let host = cpal::default_host();
        let devices = host
            .input_devices()
            .context("Failed to query input devices")?;

        let entries: Vec<DeviceInfo> = devices
            .enumerate()
            .map(|(index, device)| {
                let name = device
                    .description()
                    .map_or_else(|_| "Unknown Device".to_string(), |d| d.name().to_string());

                let config = device.default_input_config().ok();

                DeviceInfo {
                    index,
                    name,
                    sample_rate: config
                        .as_ref()
                        .map(cpal::SupportedStreamConfig::sample_rate),
                    channels: config.as_ref().map(cpal::SupportedStreamConfig::channels),
                    sample_format: config.as_ref().map(|c| format!("{:?}", c.sample_format())),
                    supported: config
                        .as_ref()
                        .is_some_and(|c| c.sample_format() == SampleFormat::F32),
                }
            })
            .collect();

        let json = serde_json::to_string(&entries).context("Failed to serialise device list")?;
        println!("{json}");

        Ok(())
    }

    /// Retrieves a concrete handle to the device and its default configuration.
    ///
    /// Resolution attempts two matching strategies in order:
    /// 1. Exact match: picks the first device whose name matches `name_query` exactly.
    /// 2. Fuzzy match: if no exact match, picks the first device whose name contains
    ///    `name_query` as a case-insensitive substring.
    ///
    /// If neither strategy matches, an error is returned. There is no fallback to the
    /// system default input device, as in a professional audio setup it may be a live
    /// input, and silently capturing it would be unsafe. Device selection must be explicit.
    ///
    /// # Errors
    ///
    /// Returns an error if input devices cannot be enumerated, hardware configuration
    /// cannot be queried, if no device matches `name_query`, or if `name_query` is empty
    /// or whitespace-only.
    pub fn get_device(
        &self,
        name_query: &str,
    ) -> Result<(cpal::Device, cpal::SupportedStreamConfig, Specs)> {
        if name_query.trim().is_empty() {
            anyhow::bail!(
                "Device query must not be empty. Run with --audio-list to see available devices."
            );
        }

        let host = cpal::default_host();
        let query_lower = name_query.to_lowercase();
        let mut fuzzy_candidate: Option<cpal::Device> = None;

        for device in host
            .input_devices()
            .context("Failed to enumerate input devices")?
        {
            let name = device
                .description()
                .map_or_else(|_| "Unknown Device".to_string(), |d| d.name().to_string());

            // Tier 1: exact name match.
            if name == name_query {
                log::info!("Audio device resolved (exact match): {name}");
                return Self::build_device_specs(device);
            }

            // Tier 2: record the first case-insensitive substring match.
            if fuzzy_candidate.is_none() && name.to_lowercase().contains(&query_lower) {
                fuzzy_candidate = Some(device);
            }
        }

        // Tier 2: use the fuzzy candidate if one was found.
        if let Some(device) = fuzzy_candidate {
            let name = device
                .description()
                .map_or_else(|_| "Unknown Device".to_string(), |d| d.name().to_string());
            log::info!("Audio device resolved (fuzzy match): {name}");
            return Self::build_device_specs(device);
        }

        // No default fallback: in a professional audio setup the system default may be
        // a live input, so silently capturing it is unsafe. Device selection must be explicit.
        anyhow::bail!(
            "No input device matched \"{name_query}\". phase4 will not fall back to the \
             system default. Run with --audio-list to see available devices."
        );
    }

    /// Queries the default input configuration for `device` and assembles a `Specs` block.
    ///
    /// # Errors
    ///
    /// Returns an error if the hardware configuration cannot be queried.
    fn build_device_specs(
        device: cpal::Device,
    ) -> Result<(cpal::Device, cpal::SupportedStreamConfig, Specs)> {
        let config = device
            .default_input_config()
            .context("Failed to query hardware config")?;

        let specs = Specs {
            sample_rate: config.sample_rate(),
            channels: config.channels(),
        };

        Ok((device, config, specs))
    }

    /// Binds the device to the analyse SPSC producer.
    ///
    /// # Errors
    ///
    /// Returns an error if the device does not support `f32` sample format,
    /// or if the input stream cannot be built or started.
    pub fn start_stream<P>(
        &mut self,
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
        mut analyse: StreamSink<P>,
        state: &Arc<AppState>,
    ) -> Result<()>
    where
        P: Producer<Item = f32> + Send + 'static,
    {
        if config.sample_format() != SampleFormat::F32 {
            anyhow::bail!(
                "Device reports {} sample format; phase4 requires f32 input. \
                 Most professional audio interfaces deliver f32 natively. \
                 Run with --audio-list to see available devices.",
                config.sample_format()
            );
        }

        let error_state = Arc::clone(state);
        let stream_config = config.config();

        // Captured once at stream construction. Never touched again inside the callback.
        let hw_channels = stream_config.channels as usize;

        let stream = device.build_input_stream(
            stream_config,
            // This callback runs on cpal's dedicated audio thread at hardware interrupt
            // rate. It must be lock-free, allocation-free, and non-blocking. Any stall
            // here will cause a buffer underrun and an audible glitch.
            move |data: &[f32], _| {
                // Analyse path is intentionally lossy. A dropped analysis frame is
                // invisible to the user.
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

    #[test]
    fn device_info_serialises_expected_shape() {
        let entry = DeviceInfo {
            index: 0,
            name: "Focusrite 2i2".to_string(),
            sample_rate: Some(48_000),
            channels: Some(2),
            sample_format: Some("F32".to_string()),
            supported: true,
        };

        let json = serde_json::to_string(&entry).expect("DeviceInfo should serialise");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("output should be valid JSON");

        assert_eq!(parsed["index"], 0);
        assert_eq!(parsed["name"], "Focusrite 2i2");
        assert_eq!(parsed["sample_rate"], 48_000);
        assert_eq!(parsed["channels"], 2);
        assert_eq!(parsed["sample_format"], "F32");
        assert_eq!(parsed["supported"], true);
    }

    #[test]
    fn device_info_serialises_unavailable_config_as_null() {
        let entry = DeviceInfo {
            index: 1,
            name: "Unknown Device".to_string(),
            sample_rate: None,
            channels: None,
            sample_format: None,
            supported: false,
        };

        let json = serde_json::to_string(&entry).expect("DeviceInfo should serialise");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("output should be valid JSON");

        assert_eq!(parsed["sample_rate"], serde_json::Value::Null);
        assert_eq!(parsed["channels"], serde_json::Value::Null);
        assert_eq!(parsed["sample_format"], serde_json::Value::Null);
        assert_eq!(parsed["supported"], false);
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

    #[test]
    fn get_device_rejects_empty_query() {
        let input = Input::new();
        let result = input.get_device("");
        assert!(
            result.is_err(),
            "an empty device query must not match anything"
        );
    }

    #[test]
    fn test_ringbuf_power_of_two() {
        let specs = Specs {
            sample_rate: 48000,
            channels: 2,
        };
        let buffer_ms = 5000;

        let (prod, _cons) = Input::create_audio_buffer_pair(specs, buffer_ms);

        // Extract the raw usize from NonZero<usize>
        let usable_capacity: usize = prod.capacity().get();
        println!("Usable Capacity: {usable_capacity}");

        let is_pow2 = usable_capacity.is_power_of_two();

        assert!(
            is_pow2,
            "Usable capacity {usable_capacity} is NOT a power of two!"
        );
    }
}
