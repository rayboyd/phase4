//! Integration tests for Phase4's fixed analysis and broadcast contract.

use clap::Parser;
use phase4::dsp::vocoder::VOCODER_BANDS;
use phase4::dsp::DISPLAY_BINS;
use phase4::Args;

const FIXED_BAND_COUNT: usize = 32;

#[test]
fn analysis_and_display_use_thirty_two_bands() {
    assert_eq!(VOCODER_BANDS, FIXED_BAND_COUNT);
    assert_eq!(DISPLAY_BINS, FIXED_BAND_COUNT);
}

#[test]
fn broadcast_rate_is_not_configurable() {
    let result = Args::try_parse_from([
        "phase4",
        "--test-hz",
        "440",
        "--ws-addr",
        "127.0.0.1:8889",
        "--broadcast-rate",
        "30",
    ]);

    assert!(result.is_err(), "--broadcast-rate must not be accepted");
}
