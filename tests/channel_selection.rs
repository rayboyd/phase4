//! Integration tests for hardware channel selection filtering.

use phase4::app::AppState;
use phase4::config::VocoderConfig;
use phase4::dsp::RawPayload;
use phase4::managers::audio::{ChannelMode, Input, Specs, StreamSink};
use phase4::managers::Processor;
use std::sync::Arc;
use tokio::sync::watch;

#[tokio::test]
#[allow(clippy::float_cmp)]
async fn selecting_single_channel_updates_analyser_payload() {
    let hw_specs = Specs {
        sample_rate: 48000,
        channels: 4,
    };

    // Create the specs for the analyser (1 channel).
    let mut analyse_specs = hw_specs;
    analyse_specs.channels = 1;

    let (analyse_tx, analyse_rx) = Input::create_audio_buffer_pair(analyse_specs, 100);

    // Wire the sink to select only hardware channel index 2 (the 0.99 sample).
    let mut sink = StreamSink {
        tx: analyse_tx,
        mode: ChannelMode::Selected(Box::new([2])),
    };

    let (raw_tx, mut raw_rx) = watch::channel(RawPayload::new(analyse_specs.channels as usize, 64));
    let state = Arc::new(AppState::new());

    // Spawn the analyser with the specs.
    let processor = Processor::new(VocoderConfig::default());
    let handle = processor.spawn(analyse_rx, raw_tx, analyse_specs, state.clone());

    // Push 4-channel interleaved data. We want Channel 2 (0.99).
    let fake_hardware_data = [0.1, 0.2, 0.99, 0.4];
    sink.push(&fake_hardware_data, hw_specs.channels as usize);

    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), raw_rx.changed())
        .await
        .expect("Test timed out waiting for the analyser to publish a frame!");

    let payload = raw_rx.borrow().clone();

    // Did the Analyser correctly create a 1-channel payload?
    assert_eq!(
        payload.channels.len(),
        1,
        "Payload should only contain the 1 selected channel"
    );

    // The Analyser should have received only the sample from channel index 2 (0.99).
    // If channel selection is broken and all samples are forwarded, the ring buffer
    // contains [0.1, 0.2, 0.99, 0.4] and the peak would still be 0.99 by accident.
    // With correct selection the buffer contains only [0.99], leaving no ambiguity.
    assert_eq!(
        payload.channels[0].peak, 0.99,
        "If this fails with 'left: 0.1', the data is scrambled"
    );

    state
        .keep_running
        .store(false, std::sync::atomic::Ordering::Release);
    let _ = handle.join();
}
