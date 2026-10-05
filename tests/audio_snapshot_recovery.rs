use phase4::app::AppState;
use phase4::config::VocoderConfig;
use phase4::dsp::{DisplayPayload, RawPayload};
use phase4::managers::audio::{ChannelMode, Input, Specs, StreamSink};
use phase4::managers::{Mapper, Processor};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use tokio::sync::watch;

const SAMPLE_RATE: u32 = 44_100;
const HARDWARE_CHANNELS: u16 = 2;
const BUFFER_DURATION_MS: u32 = 100;
const UPDATE_TIMEOUT: Duration = Duration::from_secs(2);
const RECOVERED_MIDI_STEPS: u32 = 7;
const VALID_SAMPLE: f32 = 0.25;
const NEIGHBOUR_SAMPLE: f32 = 0.5;

#[tokio::test]
async fn finite_audio_restores_complete_snapshots_in_both_capture_modes() {
    for selected in [false, true] {
        for invalid_sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let channels = if selected { 1 } else { HARDWARE_CHANNELS };
            let specs = Specs {
                sample_rate: SAMPLE_RATE,
                channels,
            };
            let (audio_tx, audio_rx) = Input::create_audio_buffer_pair(specs, BUFFER_DURATION_MS);
            let mode = if selected {
                ChannelMode::Selected(Box::new([0]))
            } else {
                ChannelMode::All
            };
            let mut sink = StreamSink { tx: audio_tx, mode };
            let (raw_tx, mut raw_rx) = watch::channel(RawPayload::new(usize::from(channels)));
            let (display_tx, mut display_rx) =
                watch::channel(DisplayPayload::new(usize::from(channels)));
            let state = Arc::new(AppState::new());
            let processor = Processor::new(VocoderConfig::default()).spawn(
                audio_rx,
                raw_tx,
                specs,
                state.clone(),
            );
            let mapper = Mapper::spawn(
                raw_rx.clone(),
                display_tx,
                usize::from(channels),
                state.clone(),
                true,
            );
            assert!(!sink.push(
                &[invalid_sample, NEIGHBOUR_SAMPLE],
                usize::from(HARDWARE_CHANNELS)
            ));
            tokio::time::timeout(UPDATE_TIMEOUT, raw_rx.changed())
                .await
                .unwrap()
                .unwrap();
            raw_rx.borrow_and_update();
            state
                .midi_steps
                .store(RECOVERED_MIDI_STEPS, Ordering::Release);
            assert!(!sink.push(
                &[VALID_SAMPLE, NEIGHBOUR_SAMPLE],
                usize::from(HARDWARE_CHANNELS)
            ));
            tokio::time::timeout(UPDATE_TIMEOUT, raw_rx.changed())
                .await
                .unwrap()
                .unwrap();
            let recovered = tokio::time::timeout(UPDATE_TIMEOUT, async {
                loop {
                    display_rx.changed().await.unwrap();
                    let snapshot = display_rx.borrow_and_update().clone();
                    if snapshot.channels[0].peak == VALID_SAMPLE
                        && snapshot
                            .midi
                            .as_ref()
                            .is_some_and(|midi| midi.steps == RECOVERED_MIDI_STEPS)
                    {
                        break snapshot;
                    }
                }
            })
            .await;
            state.keep_running.store(false, Ordering::Release);
            processor.join().unwrap();
            mapper.join().unwrap();
            let snapshot = recovered.expect("finite audio did not restore a complete snapshot");
            assert!(snapshot
                .channels
                .iter()
                .all(|channel| channel.peak.is_finite()
                    && channel.bins.iter().all(|bin| bin.is_finite())));
            assert!(snapshot
                .channels
                .iter()
                .all(|channel| channel.bins.iter().any(|bin| *bin > 0.0)));
            if !selected {
                assert!((snapshot.channels[1].peak - NEIGHBOUR_SAMPLE).abs() < f32::EPSILON);
            }
            #[cfg(target_os = "macos")]
            {
                use phase4::frames::{FrameMidi, FrameRegion};
                let region = FrameRegion::new(usize::from(channels), SAMPLE_RATE, true).unwrap();
                assert!(region.publish(
                    &snapshot.channels,
                    FrameMidi {
                        steps: RECOVERED_MIDI_STEPS,
                        ..FrameMidi::default()
                    },
                    0
                ));
            }
        }
    }
}
