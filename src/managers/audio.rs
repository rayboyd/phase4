//! [`Input`] wraps a `cpal::Stream` and provides two methods.
//! [`Input::get_device`] queries the hardware configuration without
//! starting a stream, and [`Input::start_stream`] binds the device to
//! an SPSC ringbuf producer for the analyser.
//!
//! The stream callback pushes f32 frames into a pre-allocated ring buffer and
//! never waits for the analyser. When the buffer cannot take a whole frame,
//! the callback drops that frame. A partially written frame would rotate the
//! channel alignment of every frame the analyser reads after it, whereas a
//! dropped whole frame is a gap in the analysed signal and nothing more.
//!
//! [`Specs`] carries the stream channel count and sample rate for buffer
//! sizing. The analyser's specs use the selected channel count. The device's
//! default input configuration must report `F32`, which is the format cpal
//! hands to the callback, not the converter depth of the hardware.

use crate::app::AppState;
use crate::ListFormat;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use ringbuf::traits::{Producer, Split};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Typed device-resolution failures with user-facing messages.
///
/// Message texts are part of the user-facing (not machine-read) surface.
/// Each one names the fix and points at `--audio-list`.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// The device query was empty or whitespace-only.
    #[error("Device query must not be empty. Run with --audio-list to see available devices.")]
    EmptyQuery,

    /// No device matched the query, exactly or as a substring.
    #[error(
        "No input device matched \"{query}\". phase4 will not fall back to the \
         system default. Run with --audio-list to see available devices."
    )]
    NoMatch { query: String },

    /// The supplied input configuration does not report `F32`.
    #[error(
        "Device reports {format} sample format; phase4 requires f32 input. \
         Most professional audio interfaces deliver f32 natively. \
         Run with --audio-list to see available devices."
    )]
    UnsupportedFormat { format: String },
}

/// Stream channel count and sample rate used for buffer sizing.
#[derive(Clone, Copy)]
pub struct Specs {
    /// Number of interleaved channels at this pipeline stage.
    pub channels: u16,

    /// Hardware sample rate in Hz.
    pub sample_rate: u32,
}

impl Specs {
    /// Returns the number of samples needed to cover `ms` milliseconds,
    /// across all channels, at the configured stream sample rate.
    #[must_use]
    pub fn samples_for_ms(&self, ms: u32) -> usize {
        // Each as usize cast widens the u32/u16 inputs to 64-bit before the multiply.
        // Without that, 192000_u32 * 16_u32 * 3_600_000_u32 would overflow u32::MAX (4,294,967,295).
        (self.sample_rate as usize * self.channels as usize * ms as usize) / 1000
    }

    /// Like [`samples_for_ms`], rounded up to a whole frame multiple.
    ///
    /// The generator writes whole frames and the analyser drains whole
    /// frames, so both want a buffer whose length is a channel multiple.
    /// `samples_for_ms` alone can return an odd count. 22050 Hz stereo at
    /// 10 ms yields 441 samples.
    ///
    /// [`samples_for_ms`]: Specs::samples_for_ms
    #[must_use]
    pub fn frame_aligned_samples_for_ms(&self, ms: u32) -> usize {
        let channels = self.channels as usize;
        self.samples_for_ms(ms).div_ceil(channels) * channels
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
    /// Zero-based position in the host's input device enumeration.
    index: usize,

    /// Device name, or "Unknown Device" if the description could not be read.
    name: String,

    /// Hardware sample rate in Hz.
    sample_rate: Option<u32>,

    /// Number of hardware input channels.
    channels: Option<u16>,

    /// Sample format of the default input configuration, e.g. "F32".
    sample_format: Option<String>,

    /// Whether the device's default configuration is `f32`, phase4's required sample format.
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
    /// Forward every hardware channel via the `push_slice` fast path.
    All,

    /// Forward only the listed zero-based hardware channel indices, sorted and deduplicated.
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
    /// SPSC producer for the downstream consumer.
    pub tx: P,

    /// Which hardware channels to forward.
    pub mode: ChannelMode,
}

impl<P: Producer<Item = f32>> StreamSink<P> {
    /// Pushes audio data into the sink, applying the channel selection mode.
    ///
    /// `hw_channels` is the total interleaved channel count from cpal, used to
    /// stride across frames in both modes.
    ///
    /// Only whole frames are committed, so overflow never rotates the
    /// analyser's channel alignment. A frame is `hw_channels` samples in the
    /// `All` path and `indices.len()` samples in the `Selected` path.
    ///
    /// `All` checks space once and pushes the accepted slice in one call.
    /// `Selected` checks space per frame and pushes each selected sample on
    /// its own, so the analyser can read a partial frame mid-push. The
    /// analyser carries that partial frame until the rest arrives.
    ///
    /// Returns `true` if any frame was dropped.
    pub fn push(&mut self, data: &[f32], hw_channels: usize) -> bool {
        match &self.mode {
            ChannelMode::All => {
                // Truncate both the input (defensively, cpal delivers whole
                // frames) and the writable span to whole frame multiples.
                let len = data.len() / hw_channels * hw_channels;
                let writable = self.tx.vacant_len() / hw_channels * hw_channels;
                let n = len.min(writable);
                self.tx.push_slice(&data[..n]);
                n < len
            }
            ChannelMode::Selected(indices) => {
                let per_frame = indices.len();
                let mut dropped = false;
                for frame in data.chunks_exact(hw_channels) {
                    // All-or-nothing per frame. `vacant_len` is conservative
                    // on the producer side (the consumer can only grow it),
                    // so once it admits a frame the pushes cannot fail.
                    if self.tx.vacant_len() < per_frame {
                        dropped = true;
                        continue;
                    }
                    for &idx in indices {
                        let _ = self.tx.try_push(frame[idx as usize]);
                    }
                }
                dropped
            }
        }
    }
}

/// Owns the active input stream, if one has been started.
#[derive(Default)]
pub struct Input {
    /// The running `cpal` stream, kept alive for as long as capture should continue.
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
    /// The buffer is allocated once and never grows. Its capacity is the
    /// requested sample count rounded up to the next power of two, so it
    /// holds at least `buffer_ms` and usually more. 500 ms at 48 kHz stereo
    /// asks for 48,000 samples and gets 65,536, about 683 ms.
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
        let entries = Self::enumerate_devices()?;
        match format {
            ListFormat::Text => {
                Self::list_devices_text(&entries);
                Ok(())
            }
            ListFormat::Json => Self::list_devices_json(&entries),
        }
    }

    fn enumerate_devices() -> Result<Vec<DeviceInfo>> {
        let host = cpal::default_host();
        let devices = host
            .input_devices()
            .context("Failed to query input devices")?;

        Ok(devices
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
            .collect())
    }

    /// Human-readable device listing via `log`, one line per device.
    fn list_devices_text(entries: &[DeviceInfo]) {
        if entries.is_empty() {
            log::warn!("[*] No input devices detected. Check system permissions");
            return;
        }

        for entry in entries {
            if let (Some(sample_rate), Some(channels), Some(format)) = (
                entry.sample_rate,
                entry.channels,
                entry.sample_format.as_deref(),
            ) {
                let status = if entry.supported {
                    ""
                } else {
                    "* No hardware support (32-bit required)"
                };
                log::info!(
                    "[{}] {} ({}Hz, {}ch, {}) {}",
                    entry.index,
                    entry.name,
                    sample_rate,
                    channels,
                    format,
                    status
                );
            } else {
                log::warn!(
                    "[{}] {} (Configuration unavailable)",
                    entry.index,
                    entry.name
                );
            }
        }
    }

    /// Structured device listing as a single JSON array on stdout.
    /// Log output goes to stderr so scripts can parse stdout without filtering.
    fn list_devices_json(entries: &[DeviceInfo]) -> Result<()> {
        let json = serde_json::to_string(entries).context("Failed to serialise device list")?;
        println!("{json}");
        Ok(())
    }

    /// Retrieves a concrete handle to the device and its default configuration.
    ///
    /// Resolution attempts two matching strategies in order. The first
    /// device whose name matches `name_query` exactly wins. Failing that,
    /// the first device whose name contains `name_query` as a
    /// case-insensitive substring wins.
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
            return Err(DeviceError::EmptyQuery.into());
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

            // An exact name match wins immediately.
            if name == name_query {
                log::info!("Audio device resolved (exact match): {name}");
                return Self::build_device_specs(device);
            }

            // Record the first case-insensitive substring match as a fallback.
            if fuzzy_candidate.is_none() && name.to_lowercase().contains(&query_lower) {
                fuzzy_candidate = Some(device);
            }
        }

        // Fall back to the recorded substring match if one was found.
        if let Some(device) = fuzzy_candidate {
            let name = device
                .description()
                .map_or_else(|_| "Unknown Device".to_string(), |d| d.name().to_string());
            log::info!("Audio device resolved (fuzzy match): {name}");
            return Self::build_device_specs(device);
        }

        // There is deliberately no fallback to the system default input. In a
        // professional audio setup it may be a live input, and silently capturing
        // it would be unsafe. Device selection must be explicit.
        Err(DeviceError::NoMatch {
            query: name_query.to_string(),
        }
        .into())
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
    /// Returns an error if the supplied configuration is not `F32`,
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
            return Err(DeviceError::UnsupportedFormat {
                format: config.sample_format().to_string(),
            }
            .into());
        }

        let error_state = Arc::clone(state);
        let stream_config = config.config();

        // Captured once at stream construction. Never touched again inside the callback.
        let hw_channels = stream_config.channels as usize;

        let stream = device.build_input_stream(
            stream_config,
            // cpal calls this on its audio thread at hardware interrupt rate.
            // It must stay allocation-free, lock-free and non-blocking, so a
            // full buffer drops the frame rather than waiting. A dropped
            // analysis frame is invisible to the user, hence the ignored flag.
            move |data: &[f32], _| {
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
    fn text_device_listing_preserves_supported_unsupported_and_unavailable_output() {
        testing_logger::setup();
        let entries = [
            DeviceInfo {
                index: 0,
                name: "Float device".to_string(),
                sample_rate: Some(48_000),
                channels: Some(2),
                sample_format: Some("F32".to_string()),
                supported: true,
            },
            DeviceInfo {
                index: 1,
                name: "Integer device".to_string(),
                sample_rate: Some(44_100),
                channels: Some(2),
                sample_format: Some("I16".to_string()),
                supported: false,
            },
            DeviceInfo {
                index: 2,
                name: "Unknown Device".to_string(),
                sample_rate: None,
                channels: None,
                sample_format: None,
                supported: false,
            },
        ];
        Input::list_devices_text(&entries);

        testing_logger::validate(|logs| {
            assert_eq!(logs.len(), entries.len());
            assert_eq!(logs[0].level, log::Level::Info);
            assert_eq!(logs[0].body, "[0] Float device (48000Hz, 2ch, F32) ");
            assert_eq!(logs[1].level, log::Level::Info);
            assert_eq!(
                logs[1].body,
                "[1] Integer device (44100Hz, 2ch, I16) * No hardware support (32-bit required)"
            );
            assert_eq!(logs[2].level, log::Level::Warn);
            assert_eq!(
                logs[2].body,
                "[2] Unknown Device (Configuration unavailable)"
            );
        });
    }

    #[test]
    fn empty_text_device_listing_reports_no_devices() {
        testing_logger::setup();
        Input::list_devices_text(&[]);

        testing_logger::validate(|logs| {
            assert_eq!(logs.len(), 1);
            assert_eq!(logs[0].level, log::Level::Warn);
            assert_eq!(
                logs[0].body,
                "[*] No input devices detected. Check system permissions"
            );
        });
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

    // Rates that divide evenly are unchanged; rates that land mid-frame
    // round up to the next whole frame.
    #[test]
    fn frame_aligned_samples_for_ms_rounds_up_to_whole_frames() {
        let aligned = |sample_rate, channels, ms| {
            Specs {
                channels,
                sample_rate,
            }
            .frame_aligned_samples_for_ms(ms)
        };

        // Counts already on a frame boundary pass through unchanged.
        assert_eq!(aligned(48_000, 2, 10), 960);
        assert_eq!(aligned(44_100, 2, 10), 882);
        assert_eq!(aligned(44_100, 6, 10), 2_646);

        // 22050 Hz stereo at 10 ms is 441 samples, mid-frame, and rounds up to 442.
        assert_eq!(aligned(22_050, 2, 10), 442);
        // 22050 Hz 6ch at 10 ms is 1323 samples and rounds up to 1326.
        assert_eq!(aligned(22_050, 6, 10), 1_326);
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

    // All mode never commits a partial frame. With room for 3 samples but
    // 2-channel frames, only one whole frame goes in. A torn frame here would
    // rotate the analyser's channel alignment for every later frame.
    #[test]
    fn push_all_commits_whole_frames_only() {
        let (mut sink, c) = make_sink_with_consumer(3, ChannelMode::All);
        let dropped = sink.push(&[1.0, 2.0, 3.0, 4.0], 2);
        assert!(dropped);
        assert_eq!(drain(c), &[1.0, 2.0]);
    }

    // Selected mode drops a frame entirely when the ring cannot hold all of
    // that frame's selected samples, rather than committing part of it.
    #[test]
    fn push_selected_drops_whole_frames_when_short_of_space() {
        // With room for 3 samples and 2 selected samples per frame, frame 1
        // fits and frame 2 must be dropped in full, not split.
        let (mut sink, c) = make_sink_with_consumer(3, ChannelMode::Selected(Box::new([0, 1])));
        let dropped = sink.push(&[1.0, 2.0, 3.0, 4.0], 2);
        assert!(dropped);
        assert_eq!(drain(c), &[1.0, 2.0]);
    }

    // Selected([0]) extracts only channel 0 from each frame of a 4-channel stream.
    #[test]
    fn push_selected_extracts_first_channel() {
        // 2 frames * 4 channels, interleaved as [ch0, ch1, ch2, ch3, ch0, ch1, ch2, ch3].
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
    // cpal's contract guarantees the callback always delivers complete frames.
    #[test]
    fn push_selected_ignores_partial_trailing_frame() {
        // 9 samples with hw_channels=4 is 2 full frames plus 1 orphan sample.
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
}
