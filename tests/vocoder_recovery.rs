use phase4::config::VocoderConfig;
use phase4::dsp::VocoderAnalyser;

const SAMPLE_RATE: u32 = 44_100;
const RECOVERY_SAMPLE_COUNT: usize = 512;
const VALID_SAMPLE: f32 = 0.25;

#[test]
fn non_finite_samples_preserve_channel_state_and_alignment() {
    for invalid_sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut affected = VocoderAnalyser::new(SAMPLE_RATE, &VocoderConfig::default());
        let mut unaffected = VocoderAnalyser::new(SAMPLE_RATE, &VocoderConfig::default());
        let mut control = VocoderAnalyser::new(SAMPLE_RATE, &VocoderConfig::default());
        let finite = [VALID_SAMPLE; RECOVERY_SAMPLE_COUNT];
        affected.process_interleaved(&finite, 0, 1);
        control.process_interleaved(&finite, 0, 1);
        affected.process_interleaved(&[invalid_sample, VALID_SAMPLE], 0, 2);
        unaffected.process_interleaved(&[invalid_sample, VALID_SAMPLE], 1, 2);
        assert_eq!(affected.current_bins(), control.current_bins());
        affected.process_interleaved(&finite, 0, 1);
        control.process_interleaved(&finite, 0, 1);
        assert_eq!(affected.current_bins(), control.current_bins());
        assert!(unaffected.current_bins().iter().all(|bin| bin.is_finite()));
        assert!(unaffected.current_bins().iter().any(|bin| *bin > 0.0));
    }
}

#[test]
fn extreme_finite_samples_do_not_prevent_recovery() {
    let mut analyser = VocoderAnalyser::new(SAMPLE_RATE, &VocoderConfig::default());
    for _ in 0..RECOVERY_SAMPLE_COUNT {
        analyser.process_interleaved(&[f32::MAX, -f32::MAX], 0, 1);
        assert!(analyser.current_bins().iter().all(|bin| bin.is_finite()));
    }
    analyser.process_interleaved(&[VALID_SAMPLE; RECOVERY_SAMPLE_COUNT], 0, 1);
    assert!(analyser.current_bins().iter().all(|bin| bin.is_finite()));
    assert!(analyser.current_bins().iter().any(|bin| *bin > 0.0));
}
