//! [`Processor`] spawns a background thread that drains the analyse ringbuf in
//! chunks of `CHUNK_SIZE_MS` milliseconds, runs per-channel peak measurement
//! and vocoder envelope analysis, then publishes the resulting
//! [`crate::dsp::RawPayload`] via a [`tokio::sync::watch`] channel.
//!
//! The thread runs at elevated scheduling priority (`ANALYSER_THREAD_PRIORITY`)
//! to avoid starvation, but is ranked below the recorder thread on the
//! basis that a delayed analysis frame is acceptable whereas a missed disk
//! write is not. Denormal floating-point values are suppressed via
//! [`no_denormals()`] to prevent CPU performance degradation under silence or
//! very low signal levels.

use super::audio::Specs;
use crate::app::AppState;
use crate::config::VocoderConfig;
use crate::dsp::{RawPayload, VocoderAnalyser};
use no_denormals::no_denormals;
use ringbuf::traits::Consumer;
use std::sync::{atomic::Ordering, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thread_priority::{set_current_thread_priority, ThreadPriority, ThreadPriorityValue};
use tokio::sync::watch;

/// The maximum chunk of audio processed in a single loop iteration.
const CHUNK_SIZE_MS: u32 = 10;

/// Thread sleep duration (ms) when the ringbuf is empty, avoids busy-waiting.
const IDLE_SLEEP_MS: u64 = 10;

/// Processor thread priority in the crate's cross-platform 0-99 scale.
/// Lower than the recorder thread. Analysis can safely lag a frame. A missed
/// write to disk cannot.
const ANALYSER_THREAD_PRIORITY: u8 = 40;

/// Returns the peak absolute sample value for a single channel within an
/// interleaved audio buffer.
///
/// `channel` is the zero-based channel index and `channels` is the total
/// number of interleaved channels. The function strides through the buffer
/// at `channels` intervals starting from `channel`.
fn interleaved_peak(buffer: &[f32], channel: usize, channels: usize) -> f32 {
    let mut peak = 0.0f32;
    let mut i = channel;
    while i < buffer.len() {
        let abs_s = buffer[i].abs();
        if abs_s > peak {
            peak = abs_s;
        }
        i += channels;
    }
    peak
}

/// Internal state for the analyser thread, owns the transfer buffer and DSP state.
struct State {
    channels: usize,
    transfer_buffer: Vec<f32>,
    /// Pre-allocated payload buffer, reused every frame to avoid per-call heap allocation.
    /// Published via swap into the watch channel gives us no clones or allocations.
    frame_data: RawPayload,
    raw_tx: watch::Sender<RawPayload>,
    app: Arc<AppState>,
    analysers: Vec<VocoderAnalyser>,
}

impl State {
    fn new(
        specs: Specs,
        raw_tx: watch::Sender<RawPayload>,
        state: Arc<AppState>,
        vocoder_config: &VocoderConfig,
    ) -> Self {
        let channels = specs.channels as usize;
        let analysers: Vec<VocoderAnalyser> = (0..channels)
            .map(|_| VocoderAnalyser::new(specs.sample_rate, vocoder_config))
            .collect();
        let bin_count = crate::dsp::vocoder::VOCODER_BANDS;

        let frame_data = RawPayload::new(channels, bin_count);

        Self {
            channels,
            transfer_buffer: vec![0.0f32; specs.samples_for_ms(CHUNK_SIZE_MS)],
            frame_data,
            raw_tx,
            app: state,
            analysers,
        }
    }

    fn process(&mut self, count: usize) {
        let active_buffer = &self.transfer_buffer[..count];

        for ch in 0..self.channels {
            let peak = interleaved_peak(active_buffer, ch, self.channels);

            // Run the vocoder analysis for this channel.
            self.analysers[ch].process_interleaved(active_buffer, ch, self.channels);

            // Write directly into the pre-allocated payload.
            let out = &mut self.frame_data.channels[ch];
            out.peak = peak;
            out.bins.copy_from_slice(self.analysers[ch].current_bins());
        }

        if self.app.is_broadcasting_websocket.load(Ordering::Acquire) {
            if self.raw_tx.is_closed() {
                log::warn!("Mapper receiver has dropped, analysis frames will be discarded");
                return;
            }

            // Swap the frame into the watch channel in-place. Keep std::mem::take
            // and send_replace adjacent, adding an early return between them would
            // drop the reusable buffer and force a fresh allocation next frame.
            // frame_data retains the previous frame's buffer. This preserves Vec
            // capacity for reuse on the next iteration.
            self.frame_data = self
                .raw_tx
                .send_replace(std::mem::take(&mut self.frame_data));
        }
    }
}

/// Owns the analyser thread, spawning a loop that drains audio samples and
/// publishing DSP results over the watch channel.
pub struct Processor {
    vocoder_config: VocoderConfig,
}

impl Processor {
    /// Creates a new `Processor`. Use [`spawn`] to start the background thread.
    ///
    /// [`spawn`]: Processor::spawn
    #[must_use]
    pub fn new(vocoder_config: VocoderConfig) -> Self {
        Self { vocoder_config }
    }

    /// Spawns the analyser background thread.
    ///
    /// The thread drains `consumer`, runs per-channel peak and vocoder envelope
    /// analysis on each `CHUNK_SIZE_MS` block, and sends the result to `watch_tx` when
    /// `is_broadcasting_websocket` is set. The thread exits when `keep_running` is cleared
    /// and the ringbuf is empty.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned.
    pub fn spawn<C>(
        self,
        mut consumer: C,
        raw_tx: watch::Sender<RawPayload>,
        specs: Specs,
        state: Arc<AppState>,
    ) -> JoinHandle<()>
    where
        C: Consumer<Item = f32> + Send + 'static,
    {
        thread::Builder::new()
            .name("analyser".into())
            .spawn(move || {
                // Runs at a lower priority than the recorder thread. A delayed
                // analysis frame is acceptable. A missed disk write is not.
                // Priority mapping is policy-dependent on Unix. Failures are logged,
                // but analysis continues to run at the OS default priority.
                super::log_priority_result(set_current_thread_priority(
                    ThreadPriority::Crossplatform(
                        ThreadPriorityValue::try_from(ANALYSER_THREAD_PRIORITY)
                            .expect("valid priority"),
                    ),
                ));

                let mut dsp_state = State::new(specs, raw_tx, state.clone(), &self.vocoder_config);
                let mut was_analysing = false;

                // Set the CPU's FTZ (Flush-to-Zero) and DAZ (Denormals-Are-Zero) flags here.
                // Prevents CPU spikes when processing near-silent audio signals (subnormal numbers).
                // No other code on this thread relies on denormal behaviour, or will, so this is ok.
                unsafe {
                    no_denormals(|| {
                        while state.keep_running.load(Ordering::Acquire) || !consumer.is_empty() {
                            // Check for any state transitions.
                            let is_analysing_active = state.is_analysing.load(Ordering::Acquire);

                            // If we just turned analysis back ON, flush the old analysis history.
                            if is_analysing_active && !was_analysing {
                                for analyser in &mut dsp_state.analysers {
                                    analyser.reset();
                                }
                            }
                            was_analysing = is_analysing_active;

                            // Drain the ringbuf, or sleep briefly when empty to avoid
                            // spinning the CPU with nothing to process.
                            let samples = consumer.pop_slice(&mut dsp_state.transfer_buffer);
                            if samples > 0 {
                                if is_analysing_active {
                                    dsp_state.process(samples);
                                }
                            } else if state.keep_running.load(Ordering::Acquire) {
                                // Idle backoff only, not a timing-critical path. Sample throughput is
                                // governed by the producer (CPAL callback or generator), not by this
                                // sleep. Drift here just adds up to 10 ms of wake-up latency, which
                                // the ringbuf absorbs without data loss.
                                thread::sleep(Duration::from_millis(IDLE_SLEEP_MS));
                            }
                        }
                    });
                }
            })
            .expect("failed to spawn analyser thread")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mono buffer: every sample belongs to channel 0.
    #[test]
    #[allow(clippy::float_cmp)]
    fn peak_mono() {
        let buffer = [0.1, -0.5, 0.3, -0.2];
        assert_eq!(interleaved_peak(&buffer, 0, 1), 0.5);
    }

    // Stereo buffer: channels are interleaved [L, R, L, R, ...].
    #[test]
    #[allow(clippy::float_cmp)]
    fn peak_stereo_channel_isolation() {
        //          L     R     L     R     L     R
        let buf = [0.1, -0.9, 0.5, -0.2, 0.3, 0.8];
        assert_eq!(interleaved_peak(&buf, 0, 2), 0.5); // L peak
        assert_eq!(interleaved_peak(&buf, 1, 2), 0.9); // R peak
    }

    // Negative values are measured by absolute value.
    #[test]
    #[allow(clippy::float_cmp)]
    fn peak_negative_values() {
        let buffer = [-0.7, 0.1, -0.3, 0.6];
        assert_eq!(interleaved_peak(&buffer, 0, 2), 0.7);
        assert_eq!(interleaved_peak(&buffer, 1, 2), 0.6);
    }

    // All-zero buffer returns zero peak.
    #[test]
    #[allow(clippy::float_cmp)]
    fn peak_silence() {
        let buffer = [0.0; 8];
        assert_eq!(interleaved_peak(&buffer, 0, 2), 0.0);
        assert_eq!(interleaved_peak(&buffer, 1, 2), 0.0);
    }

    // Empty buffer returns zero peak.
    #[test]
    #[allow(clippy::float_cmp)]
    fn peak_empty_buffer() {
        let buffer: [f32; 0] = [];
        assert_eq!(interleaved_peak(&buffer, 0, 1), 0.0);
    }

    // Three channels: stride of 3 reaches only the correct samples.
    #[test]
    #[allow(clippy::float_cmp)]
    fn peak_three_channels() {
        //          0     1     2     0     1     2
        let buf = [0.1, 0.2, 0.9, 0.8, 0.3, 0.4];
        assert_eq!(interleaved_peak(&buf, 0, 3), 0.8);
        assert_eq!(interleaved_peak(&buf, 1, 3), 0.3);
        assert_eq!(interleaved_peak(&buf, 2, 3), 0.9);
    }

    // Single sample per channel (one frame, two channels).
    #[test]
    #[allow(clippy::float_cmp)]
    fn peak_single_frame() {
        let buf = [0.42, -0.73];
        assert_eq!(interleaved_peak(&buf, 0, 2), 0.42);
        assert_eq!(interleaved_peak(&buf, 1, 2), 0.73);
    }
}
