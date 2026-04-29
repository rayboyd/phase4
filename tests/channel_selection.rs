//! Integration tests for hardware channel selection filtering.

use phase4::app::AppState;
use phase4::config::VocoderConfig;
use phase4::dsp::RawPayload;
use phase4::managers::audio::{ChannelMode, Input, Specs, StreamSink};
use phase4::managers::Processor;
use std::sync::Arc;
use tokio::sync::watch;

#[tokio::test]
async fn selecting_single_channel_updates_analyser_payload() {
    // A 4-channel hardware device.
    let hw_specs = Specs {
        sample_rate: 48000,
        channels: 4,
    };

    // Only want to analyse Channel 2 (the 3rd channel).
    let mode = ChannelMode::Selected(Box::new([2]));

    let (analyse_tx, analyse_rx) = Input::create_audio_buffer_pair(hw_specs, 100);
    let mut sink = StreamSink {
        tx: analyse_tx,
        mode,
    };

    let (raw_tx, mut raw_rx) = watch::channel(RawPayload::new(hw_specs.channels as usize, 64));
    let state = Arc::new(AppState::new());
    state
        .is_analysing
        .store(true, std::sync::atomic::Ordering::Release);

    // Spawn the analyser.
    let processor = Processor::new(VocoderConfig::default());
    let handle = processor.spawn(analyse_rx, raw_tx, hw_specs, state.clone());

    // Push exactly ONE frame of 4-channel data: [ch0, ch1, ch2, ch3] make Channel 2 distinct (0.99)
    let fake_hardware_data = [0.1, 0.2, 0.99, 0.4];

    // Using StreamSink's internal push logic that the CPAL callback uses.
    // Temporarily made `push` pub in StreamSink for this test.
    sink.push(&fake_hardware_data, hw_specs.channels as usize);

    // Wait for the analyser to process.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), raw_rx.changed()).await;
    let payload = raw_rx.borrow().clone();

    // WILL FAIL
    // If we only selected 1 channel, the resulting JSON payload should only have 1 channel.
    assert_eq!(
        payload.channels.len(),
        1,
        "Payload should only contain the 1 selected channel, but it has {}",
        payload.channels.len()
    );

    // WILL FAIL
    // That 1 channel should contain the peak from Channel 2.
    assert!(
        (payload.channels[0].peak - 0.99).abs() < f32::EPSILON,
        "Expected 0.99, got {}",
        payload.channels[0].peak
    );

    // Cleanup
    state
        .keep_running
        .store(false, std::sync::atomic::Ordering::Release);
    let _ = handle.join();
}
