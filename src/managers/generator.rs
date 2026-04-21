//! [`Generator::spawn`] starts a background thread that produces a continuous
//! sine wave and pushes it to the same two ringbuf producers that the hardware
//! audio stream would use, making the rest of the pipeline fully operational
//! without audio hardware attached.
//!
//! Two signal modes are supported: a fixed-frequency tone controlled by
//! `test_hz`, and a logarithmic sine sweep driven by a sine LFO at
//! `test_sweep` Hz that scans from 20 Hz to just below the Nyquist frequency
//! (0.45 * sample rate) to avoid aliasing artefacts at the sweep ceiling.
//! Output level is fixed at `AMPLITUDE` (approximately -12 dBFS) to leave
//! headroom for the integer-format bit-depth converters in the recorder.

use crate::app::AppState;
use ringbuf::traits::Producer;
use std::f32::consts::PI;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct Generator;

/// Calibration signal level. -12 dBFS leaves plenty of headroom for the
/// integer bit-depth converters and keeps the visualiser at a comfortable level.
const AMPLITUDE: f32 = 0.25;

/// Fills `buffer` with a sine-wave signal and returns the updated oscillator
/// state as `(phase, lfo_phase)`.
///
/// This is the inner generation loop extracted as a pure function for
/// testability. No I/O, no timing, no threading.
fn fill_buffer(
    buffer: &mut [f32],
    mut phase: f32,
    mut lfo_phase: f32,
    test_hz: Option<f32>,
    test_sweep: Option<f32>,
    sample_rate: u32,
    channels: u16,
) -> (f32, f32) {
    let chunk_size = buffer.len();
    let sweep_ceiling = 0.45 * sample_rate as f32;

    for i in (0..chunk_size).step_by(channels as usize) {
        // Calculate the current frequency.
        let current_freq = if let Some(lfo_rate) = test_sweep {
            // A sine wave LFO that oscillates between 0.0 and 1.0.
            let lfo_val = (lfo_phase * 2.0 * PI).sin() * 0.5 + 0.5;
            lfo_phase = (lfo_phase + lfo_rate / sample_rate as f32) % 1.0;

            // Logarithmic sweep from 20 Hz up to just below Nyquist.
            20.0 * (sweep_ceiling / 20.0f32).powf(lfo_val)
        } else {
            test_hz.expect("calibration mode requires test_hz or test_sweep")
        };

        // Advance the primary audio sine wave.
        let phase_inc = 2.0 * PI * current_freq / (sample_rate as f32);
        let sample = phase.sin() * AMPLITUDE;
        phase = (phase + phase_inc) % (2.0 * PI);

        // Write to all channels.
        for ch in 0..channels as usize {
            buffer[i + ch] = sample;
        }
    }

    (phase, lfo_phase)
}

impl Generator {
    /// Spawns the signal generator on a background thread.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned.
    pub fn spawn<P>(
        test_hz: Option<f32>,
        test_sweep: Option<f32>,
        sample_rate: u32,
        channels: u16,
        mut record_tx: P,
        mut analyse_tx: P,
        state: Arc<AppState>,
    ) -> JoinHandle<()>
    where
        P: Producer<Item = f32> + Send + 'static,
    {
        thread::Builder::new()
            .name("generator".into())
            .spawn(move || {
                let mut phase = 0.0f32;
                let mut lfo_phase = 0.0f32;

                let chunk_duration = Duration::from_millis(10);
                let chunk_size = (sample_rate as usize * channels as usize * 10) / 1000;
                let mut buffer = vec![0.0f32; chunk_size];
                let mut deadline = Instant::now() + chunk_duration;

                while state.keep_running.load(Ordering::Acquire) {
                    (phase, lfo_phase) = fill_buffer(
                        &mut buffer,
                        phase,
                        lfo_phase,
                        test_hz,
                        test_sweep,
                        sample_rate,
                        channels,
                    );

                    // These are both intentionally lossy. This is just a test signal, we don't
                    // care about dropped frames or blips in audio (we might want that for soak testing).
                    let _ = record_tx.push_slice(&buffer);
                    let _ = analyse_tx.push_slice(&buffer);

                    // Sleep only the remaining time until the next deadline to absorb
                    // scheduling jitter and keep the long-term sample rate accurate.
                    let now = Instant::now();
                    if let Some(remaining) = deadline.checked_duration_since(now) {
                        thread::sleep(remaining);
                    }
                    deadline += chunk_duration;
                }
            })
            .expect("failed to spawn generator thread")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generates several chunks of a 1000 Hz tone at 48 kHz and verifies the
    /// frequency by counting zero crossings. A sine wave at frequency f has
    /// 2*f zero crossings per second. We allow 0.5% tolerance.
    #[test]
    fn tone_frequency_accuracy() {
        let sample_rate: u32 = 48_000;
        let channels: u16 = 1;
        let test_hz = 1000.0f32;
        let chunk_size = (sample_rate as usize * channels as usize * 10) / 1000;
        let num_chunks = 100;

        let mut buffer = vec![0.0f32; chunk_size];
        let mut phase = 0.0f32;
        let lfo_phase = 0.0f32;

        let mut total_crossings: usize = 0;
        let mut total_samples: usize = 0;
        let mut prev_sample = 0.0f32;

        for chunk_idx in 0..num_chunks {
            let (new_phase, _) = fill_buffer(
                &mut buffer,
                phase,
                lfo_phase,
                Some(test_hz),
                None,
                sample_rate,
                channels,
            );
            phase = new_phase;

            for (i, &sample) in buffer.iter().enumerate() {
                // Skip the very first sample of the very first chunk (no previous value).
                if chunk_idx == 0 && i == 0 {
                    prev_sample = sample;
                    continue;
                }
                if (prev_sample < 0.0 && sample >= 0.0) || (prev_sample >= 0.0 && sample < 0.0) {
                    total_crossings += 1;
                }
                prev_sample = sample;
            }

            total_samples += chunk_size;
        }

        // A sine at f Hz has 2*f crossings per second.
        let duration_secs = total_samples as f64 / f64::from(sample_rate);
        let measured_freq = total_crossings as f64 / (2.0 * duration_secs);
        let tolerance = 0.005 * f64::from(test_hz);

        assert!(
            (measured_freq - f64::from(test_hz)).abs() < tolerance,
            "expected {test_hz} Hz, measured {measured_freq:.2} Hz (tolerance {tolerance:.2} Hz)",
        );
    }

    /// Generates many chunks of a fixed-frequency tone and asserts that no
    /// sample exceeds `AMPLITUDE` in absolute value. A breach would indicate
    /// the generator is producing a signal that could clip downstream
    /// integer-format converters.
    #[test]
    fn amplitude_ceiling() {
        let sample_rate: u32 = 48_000;
        let channels: u16 = 2;
        let test_hz = 1000.0f32;
        let chunk_size = (sample_rate as usize * channels as usize * 10) / 1000;
        let num_chunks = 200;

        let mut buffer = vec![0.0f32; chunk_size];
        let mut phase = 0.0f32;
        let lfo_phase = 0.0f32;
        let mut peak = 0.0f32;

        for _ in 0..num_chunks {
            let (new_phase, _) = fill_buffer(
                &mut buffer,
                phase,
                lfo_phase,
                Some(test_hz),
                None,
                sample_rate,
                channels,
            );
            phase = new_phase;

            for &sample in &buffer {
                let abs = sample.abs();
                if abs > peak {
                    peak = abs;
                }
            }
        }

        assert!(
            peak <= AMPLITUDE,
            "peak {peak} exceeds AMPLITUDE {AMPLITUDE}",
        );
    }

    /// Runs the sweep generator for one full LFO period, estimates
    /// instantaneous frequency from positive-going zero-crossing intervals,
    /// and asserts every measured frequency lies within [20 Hz, 0.45 * `sample_rate`].
    #[test]
    fn sweep_bounds() {
        let sample_rate: u32 = 48_000;
        let channels: u16 = 1;
        let lfo_rate = 0.5f32;
        let sweep_floor = 20.0f64;
        let sweep_ceiling = f64::from(sample_rate) * 0.45;

        // One full LFO period at 0.5 Hz is 2 seconds (200 chunks at 10 ms).
        let chunk_size = (sample_rate as usize * channels as usize * 10) / 1000;
        let num_chunks = 200;

        let mut buffer = vec![0.0f32; chunk_size];
        let mut phase = 0.0f32;
        let mut lfo_phase = 0.0f32;

        // Track positive-going zero crossings across all chunks.
        let mut prev_sample = 0.0f32;
        let mut prev_crossing_pos: Option<usize> = None;
        let mut min_freq = f64::MAX;
        let mut max_freq = 0.0f64;
        let mut global_sample_idx: usize = 0;
        let mut crossing_count: usize = 0;

        for chunk_idx in 0..num_chunks {
            let (new_phase, new_lfo) = fill_buffer(
                &mut buffer,
                phase,
                lfo_phase,
                None,
                Some(lfo_rate),
                sample_rate,
                channels,
            );
            phase = new_phase;
            lfo_phase = new_lfo;

            for (i, &sample) in buffer.iter().enumerate() {
                if chunk_idx == 0 && i == 0 {
                    prev_sample = sample;
                    global_sample_idx += 1;
                    continue;
                }

                // Detect positive-going zero crossing.
                if prev_sample < 0.0 && sample >= 0.0 {
                    if let Some(prev_pos) = prev_crossing_pos {
                        let period_samples = global_sample_idx - prev_pos;
                        let freq = f64::from(sample_rate) / period_samples as f64;
                        if freq < min_freq {
                            min_freq = freq;
                        }
                        if freq > max_freq {
                            max_freq = freq;
                        }
                        crossing_count += 1;
                    }
                    prev_crossing_pos = Some(global_sample_idx);
                }

                prev_sample = sample;
                global_sample_idx += 1;
            }
        }

        assert!(
            crossing_count > 0,
            "no zero-crossing intervals were measured",
        );

        // The zero-crossing method quantises the period to integer samples.
        // At the sweep ceiling the true period is sample_rate / sweep_ceiling
        // (~2.22 samples at 48 kHz), but the method rounds down to the nearest
        // integer, inflating the measured frequency. The tightest correct upper
        // bound is sample_rate / floor(true_period).
        let quantised_ceiling =
            f64::from(sample_rate) / (f64::from(sample_rate) / sweep_ceiling).floor();
        let floor_tolerance = sweep_floor * 0.05;

        assert!(
            min_freq >= sweep_floor - floor_tolerance,
            "minimum measured frequency {min_freq:.1} Hz is below the 20 Hz floor",
        );
        assert!(
            max_freq <= quantised_ceiling,
            "maximum measured frequency {max_freq:.1} Hz exceeds \
             the quantised ceiling {quantised_ceiling:.0} Hz",
        );
    }
}
