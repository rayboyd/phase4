//! Behavioural regression coverage for the accepted `no_denormals` limitation.
//! These tests exercise the compiled build. They do not prove compiler safety.

use no_denormals::no_denormals;
use phase4::config::VocoderConfig;
use phase4::dsp::{VocoderAnalyser, BAND_COUNT};
use std::hint::black_box;

const SIGN_BIT: u32 = 0x8000_0000;
const EXPONENT_MASK: u32 = 0x7f80_0000;
const MAGNITUDE_MASK: u32 = 0x7fff_ffff;
const MIN_NORMAL_BITS: u32 = 0x0080_0000;
const HALF_MIN_NORMAL_BITS: u32 = 0x0040_0000;
const MAX_SUBNORMAL_BITS: u32 = 0x007f_ffff;
const HALF_BITS: u32 = 0x3f00_0000;
const OPERAND_SCALE_BITS: u32 = 0x4b00_0000;
const SCALED_OPERAND_BITS: u32 = 0x0b80_0000;
const SAMPLE_RATE: u32 = 48_000;
const CHUNK_SAMPLES: usize = 480;
const SIGNAL_CHUNKS: usize = 100;
const SILENCE_CHUNKS: usize = 2_000;
const SIGNAL_BAND: usize = 16;
const SIGNAL_AMPLITUDE: f32 = 0.25;
const MIN_SIGNAL_RESPONSE: f32 = 0.1;
const MAX_DECAY_RESIDUE: f32 = 1.0e-25;

fn multiply_bits(left: u32, right: u32) -> u32 {
    let left = f32::from_bits(black_box(left));
    let right = f32::from_bits(black_box(right));
    black_box(left * right).to_bits()
}

fn assert_normal_or_zero_bins(bins: &[f32]) {
    for (band, value) in bins.iter().enumerate() {
        let representation = value.to_bits();
        let exponent = representation & EXPONENT_MASK;
        assert_ne!(
            exponent, EXPONENT_MASK,
            "non-finite band {band}: {representation:#x}"
        );
        assert!(
            exponent != 0 || representation & MAGNITUDE_MASK == 0,
            "subnormal band {band}: {representation:#x}"
        );
        assert_eq!(
            representation & SIGN_BIT,
            0,
            "negative band {band}: {representation:#x}"
        );
    }
}

#[test]
fn guard_suppresses_signed_operands_and_results_then_restores_behaviour() {
    println!(
        "Denormal regression target: {} / {}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    for sign in [0, SIGN_BIT] {
        let result_probe = || multiply_bits(MIN_NORMAL_BITS | sign, HALF_BITS);
        // Scaling makes the unguarded result normal, so this checks input
        // suppression independently of flushing subnormal results.
        let operand_probe = || multiply_bits(HALF_MIN_NORMAL_BITS | sign, OPERAND_SCALE_BITS);

        assert_eq!(result_probe(), HALF_MIN_NORMAL_BITS | sign);
        assert_eq!(operand_probe(), SCALED_OPERAND_BITS | sign);
        // Deliberately exercise the same accepted compiler limitation as the analyser.
        let (result, operand) = unsafe { no_denormals(|| (result_probe(), operand_probe())) };
        assert_eq!(result, sign);
        assert_eq!(operand, sign);
        assert_eq!(result_probe(), HALF_MIN_NORMAL_BITS | sign);
        assert_eq!(operand_probe(), SCALED_OPERAND_BITS | sign);
    }
}

#[test]
fn nested_guard_preserves_the_outer_guard() {
    let probe = || multiply_bits(MIN_NORMAL_BITS, HALF_BITS);
    assert_eq!(probe(), HALF_MIN_NORMAL_BITS);
    let results = unsafe {
        no_denormals(|| {
            let before_inner = probe();
            let inside_inner = no_denormals(probe);
            let after_inner = probe();
            [before_inner, inside_inner, after_inner]
        })
    };
    assert_eq!(results, [0; 3]);
    assert_eq!(probe(), HALF_MIN_NORMAL_BITS);
}

#[test]
fn guard_restores_behaviour_after_unwinding() {
    let probe = || multiply_bits(MIN_NORMAL_BITS, HALF_BITS);
    assert_eq!(probe(), HALF_MIN_NORMAL_BITS);
    let panic_result = std::panic::catch_unwind(|| unsafe {
        no_denormals(|| panic!("exercise denormal guard unwinding"));
    });
    assert!(panic_result.is_err());
    assert_eq!(probe(), HALF_MIN_NORMAL_BITS);
}

#[test]
fn guarded_filter_bank_suppresses_subnormal_input_and_still_responds_to_signal() {
    let mut analyser = VocoderAnalyser::new(SAMPLE_RATE, &VocoderConfig::default());
    let subnormal_input = [
        f32::from_bits(MAX_SUBNORMAL_BITS),
        f32::from_bits(MAX_SUBNORMAL_BITS | SIGN_BIT),
        f32::from_bits(1),
        f32::from_bits(1 | SIGN_BIT),
    ];
    unsafe {
        no_denormals(|| {
            analyser.process_interleaved(black_box(&subnormal_input), 0, 1);
        });
    }
    assert!(analyser
        .current_bins()
        .iter()
        .all(|value| value.to_bits() == 0));

    unsafe {
        no_denormals(|| analyser.process_interleaved(black_box(&[SIGNAL_AMPLITUDE]), 0, 1));
    }
    assert_normal_or_zero_bins(analyser.current_bins());
    assert!(analyser.current_bins().iter().any(|value| *value > 0.0));
}

#[test]
fn guarded_filter_bank_tracks_a_band_centre_and_decays_through_long_silence() {
    let config = VocoderConfig::default();
    let mut analyser = VocoderAnalyser::new(SAMPLE_RATE, &config);
    let position = SIGNAL_BAND as f32 / (BAND_COUNT - 1) as f32;
    let frequency = (config.freq_low.0.ln()
        + position * (config.freq_high.0.ln() - config.freq_low.0.ln()))
    .exp();
    let signal: Vec<f32> = (0..CHUNK_SAMPLES * SIGNAL_CHUNKS)
        .map(|sample| {
            let phase = std::f64::consts::TAU * f64::from(frequency) * sample as f64
                / f64::from(SAMPLE_RATE);
            SIGNAL_AMPLITUDE * phase.sin() as f32
        })
        .collect();
    let silence = [0.0; CHUNK_SAMPLES];

    unsafe {
        no_denormals(|| {
            for chunk in signal.chunks(CHUNK_SAMPLES) {
                analyser.process_interleaved(chunk, 0, 1);
                assert_normal_or_zero_bins(analyser.current_bins());
            }
            let strongest_band = analyser
                .current_bins()
                .iter()
                .enumerate()
                .max_by(|(_, left), (_, right)| left.total_cmp(right))
                .map(|(band, _)| band)
                .expect("the filter bank contains bands");
            assert_eq!(strongest_band, SIGNAL_BAND);
            assert!(analyser.current_bins()[SIGNAL_BAND] > MIN_SIGNAL_RESPONSE);

            for _ in 0..SILENCE_CHUNKS {
                analyser.process_interleaved(&silence, 0, 1);
                assert_normal_or_zero_bins(analyser.current_bins());
            }
        });
    }
    assert!(analyser
        .current_bins()
        .iter()
        .all(|value| *value < MAX_DECAY_RESIDUE));
    analyser.reset();
    assert!(analyser
        .current_bins()
        .iter()
        .all(|value| value.to_bits() == 0));
}
