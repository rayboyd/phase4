//! [`Writer`] spawns a high-priority background thread that drains the record
//! ringbuf and writes interleaved `f32` samples to a WAV file. Recording is
//! started and stopped at runtime via the `is_recording` flag on [`AppState`].
//!
//! Each recording session opens a new file under `recordings/`, whose name is
//! derived from the configured filename pattern with the UTC Unix timestamp,
//! sample rate, and bit depth substituted in. Samples are converted from the
//! internal `f32` representation to the target bit depth (32-bit float, 24-bit
//! integer, or 16-bit integer) before being passed to [`hound`]. The writer is
//! finalised and flushed when recording is stopped or the thread exits.

use super::audio::Specs;
use crate::app::AppState;
use crate::config::{BitDepth, RECORDINGS_DIR};
use ringbuf::traits::Consumer;
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thread_priority::{set_current_thread_priority, ThreadPriority, ThreadPriorityValue};

/// The maximum chunk of audio processed in a single loop iteration.
const CHUNK_SIZE_MS: u32 = 10;

/// Thread sleep duration (ms) when the ringbuf is empty, avoids busy-waiting.
const IDLE_SLEEP_MS: u64 = 10;

/// Writer thread priority in the crate's cross-platform 0-99 scale.
///
/// Kept above the analyser thread so disk writes outrank DSP work, but still
/// within the normal-policy range observed on macOS.
const RECORDER_THREAD_PRIORITY: u8 = 45;

/// Internal state machine for the recorder's lifecycle.
///
/// Transitions:
/// - `Idle` -> `Recording`: user toggles on
/// - `Recording` -> `Draining`: user toggles off (ringbuf may still have data)
/// - `Draining` -> `Idle`: ringbuf empty, writer finalised
/// - `Draining` -> `Recording`: user toggles on again before drain completes
///   (close current file immediately, open a new one)
/// - `Recording` -> `Idle`: fatal write error (via `handle_fatal_write_error`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingPhase {
    Idle,
    Recording,
    Draining,
}

/// Internal state machine for a single recording session (open/write/close).
struct State {
    bit_depth: BitDepth,
    filename_pattern: String,
    phase: RecordingPhase,
    recordings_dir: PathBuf,
    transfer_buffer: Vec<f32>,
    wav_spec: hound::WavSpec,
    writer: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
}

impl State {
    #[must_use]
    fn new(specs: Specs, bit_depth: BitDepth, filename_pattern: String) -> Self {
        Self::new_with_recordings_dir(
            specs,
            bit_depth,
            filename_pattern,
            PathBuf::from(RECORDINGS_DIR),
        )
    }

    #[cfg(test)]
    #[must_use]
    fn new_in_recordings_dir(
        specs: Specs,
        bit_depth: BitDepth,
        filename_pattern: String,
        recordings_dir: PathBuf,
    ) -> Self {
        Self::new_with_recordings_dir(specs, bit_depth, filename_pattern, recordings_dir)
    }

    #[must_use]
    fn new_with_recordings_dir(
        specs: Specs,
        bit_depth: BitDepth,
        filename_pattern: String,
        recordings_dir: PathBuf,
    ) -> Self {
        let wav_spec = match bit_depth {
            BitDepth::Float32 => hound::WavSpec {
                bits_per_sample: 32,
                channels: specs.channels,
                sample_format: hound::SampleFormat::Float,
                sample_rate: specs.sample_rate,
            },
            BitDepth::Int24 => hound::WavSpec {
                bits_per_sample: 24,
                channels: specs.channels,
                sample_format: hound::SampleFormat::Int,
                sample_rate: specs.sample_rate,
            },
            BitDepth::Int16 => hound::WavSpec {
                bits_per_sample: 16,
                channels: specs.channels,
                sample_format: hound::SampleFormat::Int,
                sample_rate: specs.sample_rate,
            },
        };

        Self {
            bit_depth,
            filename_pattern,
            phase: RecordingPhase::Idle,
            recordings_dir,
            transfer_buffer: vec![0.0f32; specs.samples_for_ms(CHUNK_SIZE_MS)],
            wav_spec,
            writer: None,
        }
    }

    fn recording_filename(&self, ts: u128) -> String {
        self.filename_pattern
            .replace("{timestamp}", &ts.to_string())
            .replace("{sample_rate}", &self.wav_spec.sample_rate.to_string())
            .replace("{bit_depth}", &self.wav_spec.bits_per_sample.to_string())
    }

    fn recording_path(&self, ts: u128) -> PathBuf {
        self.recordings_dir.join(self.recording_filename(ts))
    }

    fn update_recording_status(&mut self, is_recording: &AtomicBool, buffer_empty: bool) {
        let is_recording_now = is_recording.load(Ordering::Acquire);

        match self.phase {
            RecordingPhase::Idle if is_recording_now => {
                self.open_writer(is_recording);
            }
            RecordingPhase::Recording if !is_recording_now => {
                self.phase = RecordingPhase::Draining;
            }
            RecordingPhase::Draining if is_recording_now => {
                // User toggled back on before the drain finished. Close the
                // current file immediately and start a new one.
                self.close_writer();
                self.open_writer(is_recording);
            }
            RecordingPhase::Draining if buffer_empty => {
                self.close_writer();
                self.phase = RecordingPhase::Idle;
            }
            _ => {}
        }
    }

    fn open_writer(&mut self, is_recording: &AtomicBool) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();
        let path = self.recording_path(ts);

        if let Err(e) = std::fs::create_dir_all(&self.recordings_dir) {
            log::error!(
                "Failed to create recordings directory '{}': {}",
                self.recordings_dir.display(),
                e
            );
            is_recording.store(false, Ordering::Release);
            return;
        }
        match hound::WavWriter::create(&path, self.wav_spec) {
            Ok(writer) => {
                self.writer = Some(writer);
                self.phase = RecordingPhase::Recording;
            }
            Err(e) => {
                log::error!("Failed to create recording file '{}': {e}", path.display());
                is_recording.store(false, Ordering::Release);
            }
        }
    }

    fn write_samples(&mut self, count: usize, is_recording: &AtomicBool) {
        let Some(ref mut writer) = self.writer else {
            return;
        };

        // Bit depth is invariant for the lifetime of a recording session, so we
        // branch once per chunk rather than once per sample. This gives the
        // compiler a single-type tight loop per arm that it can vectorise.
        //
        // For the integer paths: f32 audio is normalised to [-1.0, 1.0]. We
        // clamp before scaling to prevent over-driven floats (e.g. 1.5) from
        // wrapping to nonsense values after the cast. The i24 result is stored
        // as i32 because Rust has no native i24 type. Hound packs it to 3 bytes
        // on disk when bits_per_sample = 24.
        let samples = &self.transfer_buffer[..count];
        let result = match self.bit_depth {
            BitDepth::Float32 => Self::write_f32(writer, samples),
            BitDepth::Int24 => Self::write_int24(writer, samples),
            BitDepth::Int16 => Self::write_int16(writer, samples),
        };

        if let Err(error) = result.as_ref() {
            self.handle_fatal_write_error(error, is_recording);
        }
    }

    fn handle_fatal_write_error(&mut self, error: &hound::Error, is_recording: &AtomicBool) {
        log::error!("Disk write failed, stopping recording: {error}");
        self.close_writer();
        is_recording.store(false, Ordering::Release);
        self.phase = RecordingPhase::Idle;
    }

    fn write_f32(
        writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
        samples: &[f32],
    ) -> Result<(), hound::Error> {
        for &sample in samples {
            writer.write_sample(sample)?;
        }
        Ok(())
    }

    fn write_int24(
        writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
        samples: &[f32],
    ) -> Result<(), hound::Error> {
        for &sample in samples {
            writer.write_sample((sample.clamp(-1.0, 1.0) * BitDepth::INT24_MAX) as i32)?;
        }
        Ok(())
    }

    fn write_int16(
        writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
        samples: &[f32],
    ) -> Result<(), hound::Error> {
        for &sample in samples {
            writer.write_sample((sample.clamp(-1.0, 1.0) * BitDepth::INT16_MAX) as i16)?;
        }
        Ok(())
    }

    fn close_writer(&mut self) {
        if let Some(writer) = self.writer.take() {
            if let Err(e) = writer.finalize() {
                log::error!("Failed to finalise WAV file: {e}");
            }
        }
    }
}

/// Owns the recorder thread spawns a loop that drains audio samples to a wav file.
pub struct Writer {
    filename_pattern: String,
}

impl Writer {
    #[must_use]
    pub fn new(filename_pattern: String) -> Self {
        Self { filename_pattern }
    }

    /// Spawns the recorder on a dedicated background thread.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned.
    pub fn spawn<C>(
        self,
        mut consumer: C,
        bit_depth: BitDepth,
        specs: Specs,
        state: Arc<AppState>,
    ) -> JoinHandle<()>
    where
        C: Consumer<Item = f32> + Send + 'static,
    {
        thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                // Priority mapping is policy-dependent on Unix. Failures are logged,
                // but the recorder continues to run at the OS default priority.
                super::log_priority_result(set_current_thread_priority(
                    ThreadPriority::Crossplatform(
                        ThreadPriorityValue::try_from(RECORDER_THREAD_PRIORITY)
                            .expect("valid priority"),
                    ),
                ));

                let mut rec_state = State::new(specs, bit_depth, self.filename_pattern);

                while state.keep_running.load(Ordering::Acquire) || !consumer.is_empty() {
                    // Pass the flag directly. If the OS has a write error, this will
                    // get changed and stop any app thrashing the logs with write errors.
                    rec_state.update_recording_status(&state.is_recording, consumer.is_empty());

                    // Grab the samples or idle so we don't thrash the disk waiting.
                    let samples = consumer.pop_slice(&mut rec_state.transfer_buffer);
                    if samples > 0 {
                        rec_state.write_samples(samples, &state.is_recording);
                    } else if state.keep_running.load(Ordering::Acquire) {
                        // Idle backoff only, not a timing-critical path. Sample throughput is
                        // governed by the producer (CPAL callback or generator), not by this
                        // sleep. Drift here just adds up to 10 ms of wake-up latency, which
                        // the ringbuf absorbs without data loss.
                        thread::sleep(Duration::from_millis(IDLE_SLEEP_MS));
                    }
                }

                rec_state.close_writer();
            })
            .expect("failed to spawn recorder thread")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_specs() -> Specs {
        Specs {
            sample_rate: 48_000,
            channels: 2,
        }
    }

    fn mono_specs() -> Specs {
        Specs {
            sample_rate: 48_000,
            channels: 1,
        }
    }

    // Float32 stores IEEE floats directly. Over-driven values (above 1.0) pass
    // through unchanged because clamping Float32 would permanently destroy
    // recoverable headroom. See docs/system/BIT_DEPTH.md.
    #[test]
    fn bit_depth_boundary_values_float32() {
        let dir = std::env::temp_dir().join("phase4_test_bitdepth_f32");
        let _ = std::fs::remove_dir_all(&dir);

        let pattern = "rec_{timestamp}_{sample_rate}hz_{bit_depth}bit.wav".to_string();
        let mut state =
            State::new_in_recordings_dir(mono_specs(), BitDepth::Float32, pattern, dir.clone());
        let is_recording = AtomicBool::new(true);

        state.update_recording_status(&is_recording, true);
        assert!(state.writer.is_some());

        let test_samples: [f32; 5] = [1.0, -1.0, 1.5, -1.5, 0.0];
        state.transfer_buffer[..5].copy_from_slice(&test_samples);
        state.write_samples(5, &is_recording);
        state.close_writer();

        let wav_path = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.path().extension().is_some_and(|ext| ext == "wav"))
            .expect("expected one WAV file")
            .path();

        let reader = hound::WavReader::open(&wav_path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 32);
        assert_eq!(spec.sample_format, hound::SampleFormat::Float);

        let samples: Vec<f32> = reader.into_samples::<f32>().map(Result::unwrap).collect();
        assert_eq!(
            samples,
            vec![1.0, -1.0, 1.5, -1.5, 0.0],
            "Float32 must preserve over-driven values without clamping"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Int24 clamps to [-1.0, 1.0] then scales by INT24_MAX (8,388,607).
    // Symmetric scaling: +1.0 and -1.0 produce equal magnitudes. The true
    // type minimum (-8,388,608) is intentionally unused.
    #[test]
    fn bit_depth_boundary_values_int24() {
        let dir = std::env::temp_dir().join("phase4_test_bitdepth_i24");
        let _ = std::fs::remove_dir_all(&dir);

        let pattern = "rec_{timestamp}_{sample_rate}hz_{bit_depth}bit.wav".to_string();
        let mut state =
            State::new_in_recordings_dir(mono_specs(), BitDepth::Int24, pattern, dir.clone());
        let is_recording = AtomicBool::new(true);

        state.update_recording_status(&is_recording, true);
        assert!(state.writer.is_some());

        let test_samples: [f32; 5] = [1.0, -1.0, 1.5, -1.5, 0.0];
        state.transfer_buffer[..5].copy_from_slice(&test_samples);
        state.write_samples(5, &is_recording);
        state.close_writer();

        let wav_path = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.path().extension().is_some_and(|ext| ext == "wav"))
            .expect("expected one WAV file")
            .path();

        let reader = hound::WavReader::open(&wav_path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 24);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);

        let samples: Vec<i32> = reader.into_samples::<i32>().map(Result::unwrap).collect();
        assert_eq!(
            samples,
            vec![8_388_607, -8_388_607, 8_388_607, -8_388_607, 0],
            "Int24 must clamp over-driven values and use symmetric scaling"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Int16 clamps to [-1.0, 1.0] then scales by INT16_MAX (32,767).
    // Same symmetric convention as Int24.
    #[test]
    fn bit_depth_boundary_values_int16() {
        let dir = std::env::temp_dir().join("phase4_test_bitdepth_i16");
        let _ = std::fs::remove_dir_all(&dir);

        let pattern = "rec_{timestamp}_{sample_rate}hz_{bit_depth}bit.wav".to_string();
        let mut state =
            State::new_in_recordings_dir(mono_specs(), BitDepth::Int16, pattern, dir.clone());
        let is_recording = AtomicBool::new(true);

        state.update_recording_status(&is_recording, true);
        assert!(state.writer.is_some());

        let test_samples: [f32; 5] = [1.0, -1.0, 1.5, -1.5, 0.0];
        state.transfer_buffer[..5].copy_from_slice(&test_samples);
        state.write_samples(5, &is_recording);
        state.close_writer();

        let wav_path = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.path().extension().is_some_and(|ext| ext == "wav"))
            .expect("expected one WAV file")
            .path();

        let reader = hound::WavReader::open(&wav_path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);

        let samples: Vec<i16> = reader.into_samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(
            samples,
            vec![32_767, -32_767, 32_767, -32_767, 0],
            "Int16 must clamp over-driven values and use symmetric scaling"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Review regression: a fatal write error should clear the public recording flag.
    #[test]
    fn fatal_write_error_clears_recording_flag() {
        let mut state = State::new(
            test_specs(),
            BitDepth::Int16,
            crate::config::DEFAULT_FILENAME_PATTERN.to_string(),
        );
        state.phase = RecordingPhase::Recording;

        let is_recording = AtomicBool::new(true);
        let error = hound::Error::IoError(std::io::Error::other("simulated write failure"));
        state.handle_fatal_write_error(&error, &is_recording);

        assert_eq!(state.phase, RecordingPhase::Idle);
        assert!(
            !is_recording.load(Ordering::Acquire),
            "fatal write errors should clear the public recording flag"
        );
    }

    // Review regression: after a fatal write error, the recorder must not
    // reopen a new file on the next loop iteration even if the user's flag
    // somehow remains set. The cleared public flag is the primary defence,
    // this guards the combined state-machine behaviour.
    #[test]
    fn fatal_write_error_does_not_reopen_on_next_update() {
        let mut state = State::new(
            test_specs(),
            BitDepth::Int16,
            crate::config::DEFAULT_FILENAME_PATTERN.to_string(),
        );
        state.phase = RecordingPhase::Recording;

        let is_recording = AtomicBool::new(true);
        let error = hound::Error::IoError(std::io::Error::other("simulated write failure"));
        state.handle_fatal_write_error(&error, &is_recording);

        // Sanity: writer has been closed and the public flag cleared.
        assert!(state.writer.is_none());
        assert!(!is_recording.load(Ordering::Acquire));

        // Next loop iteration sees a consistent off state, no new file opens.
        state.update_recording_status(&is_recording, true);

        assert!(
            state.writer.is_none(),
            "the recorder must not reopen a file after a fatal write error"
        );
        assert_eq!(state.phase, RecordingPhase::Idle);
    }

    // Toggling recording off then on before the ringbuf has fully drained
    // must close the first file and open a second one immediately.
    #[test]
    fn toggle_off_on_while_draining_opens_new_file() {
        let dir = std::env::temp_dir().join("phase4_test_drain");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let pattern = "rec_{timestamp}_{sample_rate}hz_{bit_depth}bit.wav".to_string();
        let mut state =
            State::new_in_recordings_dir(test_specs(), BitDepth::Int16, pattern, dir.clone());
        let is_recording = AtomicBool::new(true);

        // Start recording (Idle -> Recording).
        state.update_recording_status(&is_recording, true);
        assert_eq!(state.phase, RecordingPhase::Recording);
        assert!(state.writer.is_some());

        // User toggles off (Recording -> Draining). Buffer is not empty yet.
        is_recording.store(false, Ordering::Release);
        state.update_recording_status(&is_recording, false);
        assert_eq!(state.phase, RecordingPhase::Draining);
        assert!(state.writer.is_some(), "writer stays open while draining");

        // User toggles on again before drain completes (Draining -> Recording).
        // Must close the first file and open a second one.
        is_recording.store(true, Ordering::Release);

        state.update_recording_status(&is_recording, false);
        assert_eq!(state.phase, RecordingPhase::Recording);
        assert!(state.writer.is_some(), "a new writer should be open");

        // Clean up: close the writer so hound finalises the file.
        state.close_writer();

        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "wav"))
            .collect();
        assert_eq!(files.len(), 2, "expected two WAV files, got: {files:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // Recordings are always written under the recordings directory, regardless of pattern.
    #[test]
    fn recording_path_is_nested_under_recordings_dir() {
        let state = State::new(
            test_specs(),
            BitDepth::Int16,
            crate::config::DEFAULT_FILENAME_PATTERN.to_string(),
        );

        let path = state.recording_path(123);

        assert_eq!(
            path,
            std::path::Path::new(crate::config::RECORDINGS_DIR).join("rec_123_48000hz_16bit.wav")
        );
    }

    // Every documented token in the filename pattern resolves to the correct
    // value for each bit depth and sample rate combination.
    #[test]
    fn filename_pattern_substitution() {
        let cases: &[(Specs, BitDepth, &str)] = &[
            (test_specs(), BitDepth::Float32, "rec_100_48000hz_32bit.wav"),
            (test_specs(), BitDepth::Int24, "rec_100_48000hz_24bit.wav"),
            (test_specs(), BitDepth::Int16, "rec_100_48000hz_16bit.wav"),
            (
                Specs {
                    sample_rate: 44_100,
                    channels: 1,
                },
                BitDepth::Int24,
                "rec_100_44100hz_24bit.wav",
            ),
        ];

        let pattern = "rec_{timestamp}_{sample_rate}hz_{bit_depth}bit.wav".to_string();
        let timestamp: u128 = 100;

        for (specs, bit_depth, expected) in cases {
            let state = State::new_in_recordings_dir(
                *specs,
                *bit_depth,
                pattern.clone(),
                PathBuf::from("unused"),
            );
            assert_eq!(
                state.recording_filename(timestamp),
                *expected,
                "pattern mismatch for {bit_depth:?} at {}Hz",
                specs.sample_rate,
            );
        }
    }

    // The recordings directory is created automatically on first open_writer
    // call. A non-existent path must be created, and a WAV file must appear
    // inside it.
    #[test]
    fn recordings_directory_created_on_first_open() {
        let base = std::env::temp_dir().join("phase4_test_mkdir");
        let dir = base.join("nested");
        let _ = std::fs::remove_dir_all(&base);

        assert!(!dir.exists(), "directory must not exist before the test");

        let pattern = "rec_{timestamp}.wav".to_string();
        let mut state =
            State::new_in_recordings_dir(mono_specs(), BitDepth::Int16, pattern, dir.clone());
        let is_recording = AtomicBool::new(true);

        state.open_writer(&is_recording);

        assert!(
            dir.exists(),
            "open_writer must create the recordings directory"
        );
        assert!(
            state.writer.is_some(),
            "writer must be open after a successful create"
        );
        assert!(
            is_recording.load(Ordering::Acquire),
            "is_recording must remain true on success"
        );

        state.close_writer();
        let _ = std::fs::remove_dir_all(&base);
    }

    // Two back-to-back open_writer calls must produce distinct filenames.
    // The microsecond timestamp is the uniqueness mechanism.
    #[test]
    fn rapid_toggle_produces_distinct_filenames() {
        let dir = std::env::temp_dir().join("phase4_test_rapid_toggle");
        let _ = std::fs::remove_dir_all(&dir);

        let pattern = "rec_{timestamp}_{sample_rate}hz_{bit_depth}bit.wav".to_string();
        let mut state =
            State::new_in_recordings_dir(mono_specs(), BitDepth::Int16, pattern, dir.clone());
        let is_recording = AtomicBool::new(true);

        state.open_writer(&is_recording);
        assert!(state.writer.is_some(), "first writer must open");
        state.close_writer();

        state.open_writer(&is_recording);
        assert!(state.writer.is_some(), "second writer must open");
        state.close_writer();

        let files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "wav"))
            .collect();

        assert_eq!(
            files.len(),
            2,
            "expected two distinct WAV files, got: {files:?}"
        );
        assert_ne!(files[0], files[1], "filenames must differ");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
