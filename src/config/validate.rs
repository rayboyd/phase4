use super::types::{
    AppConfig, AppConfigError, ConfigInput, ConfigMidiInput, OutputConfig, TestSignal,
    CALIBRATION_MAX_FREQUENCY_HZ,
};
use crate::dsp::units::Hertz;
use std::time::{Duration, Instant};

/// MIDI clock pulses emitted per quarter note by the standard timing clock.
const MIDI_CLOCK_TICKS_PER_QUARTER_NOTE: f64 = 24.0;

pub(super) fn is_strictly_positive(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

fn non_zero_duration_from_secs(seconds: f64) -> Option<Duration> {
    Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|duration| !duration.is_zero())
}

pub(crate) fn broadcast_interval(rate_hz: f32) -> Result<Duration, AppConfigError> {
    if !is_strictly_positive(rate_hz) {
        return Err(AppConfigError::InvalidBroadcastRate { value: rate_hz });
    }

    non_zero_duration_from_secs(1.0 / f64::from(rate_hz))
        .ok_or(AppConfigError::InvalidBroadcastRate { value: rate_hz })
}

pub(crate) fn midi_tick_interval(bpm: f32) -> Result<Duration, AppConfigError> {
    if !is_strictly_positive(bpm) {
        return Err(AppConfigError::InvalidMidiTempo { value: bpm });
    }

    let seconds = 60.0 / (f64::from(bpm) * MIDI_CLOCK_TICKS_PER_QUARTER_NOTE);
    let interval = non_zero_duration_from_secs(seconds)
        .ok_or(AppConfigError::InvalidMidiTempo { value: bpm })?;
    if Instant::now().checked_add(interval).is_none() {
        return Err(AppConfigError::InvalidMidiTempo { value: bpm });
    }

    Ok(interval)
}

pub(super) fn validate_test_signal(signal: TestSignal) -> Result<(), AppConfigError> {
    match signal {
        TestSignal::FixedTone(value)
            if !is_strictly_positive(value) || value > CALIBRATION_MAX_FREQUENCY_HZ =>
        {
            Err(AppConfigError::InvalidTestFrequency { value })
        }
        TestSignal::Sweep(value)
            if !is_strictly_positive(value) || value > CALIBRATION_MAX_FREQUENCY_HZ =>
        {
            Err(AppConfigError::InvalidTestSweepRate { value })
        }
        TestSignal::FixedTone(_) | TestSignal::Sweep(_) => Ok(()),
    }
}

pub(super) fn validate_max_clients(max_clients: usize) -> Result<(), AppConfigError> {
    let maximum = tokio::sync::Semaphore::MAX_PERMITS;
    if max_clients == 0 || max_clients > maximum {
        return Err(AppConfigError::InvalidMaxClients);
    }
    Ok(())
}

pub(crate) fn validate_app_config(config: &AppConfig) -> Result<(), AppConfigError> {
    validate_vocoder_fields(
        config.vocoder_config.attack_ms.0,
        config.vocoder_config.release_ms.0,
        config.vocoder_config.freq_low.0,
        config.vocoder_config.freq_high.0,
        config.vocoder_config.filter_q,
    )?;

    if let Some(rate_hz) = config.broadcast_rate {
        broadcast_interval(rate_hz)?;
    }

    if let ConfigInput::Calibration(signal) = &config.input {
        validate_test_signal(*signal)?;
    }

    if let Some(ConfigMidiInput::TestClock(bpm)) = &config.midi_input {
        midi_tick_interval(*bpm)?;
    }

    for output in config.outputs.iter() {
        if let OutputConfig::WebSocket { max_clients, .. } = output {
            validate_max_clients(*max_clients)?;
        }
    }

    Ok(())
}

pub(super) fn validate_bind_addr(addr: std::net::SocketAddr) -> Result<(), AppConfigError> {
    if addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(AppConfigError::NonLoopbackBindAddress(addr))
    }
}

pub(super) fn validate_vocoder_fields(
    attack_ms: f32,
    release_ms: f32,
    freq_low: f32,
    freq_high: f32,
    filter_q: f32,
) -> Result<(), AppConfigError> {
    if !is_strictly_positive(attack_ms) {
        return Err(AppConfigError::InvalidAttackTime { value: attack_ms });
    }

    if !is_strictly_positive(release_ms) {
        return Err(AppConfigError::InvalidReleaseTime { value: release_ms });
    }

    if !is_strictly_positive(freq_low) {
        return Err(AppConfigError::InvalidFreqLow { value: freq_low });
    }

    if !is_strictly_positive(freq_high) {
        return Err(AppConfigError::InvalidFreqHigh { value: freq_high });
    }

    if freq_low >= freq_high {
        return Err(AppConfigError::InvalidFreqRange {
            freq_low,
            freq_high,
        });
    }

    let maximum_biquad_alpha = 1.0 / (2.0 * filter_q);
    if !is_strictly_positive(filter_q) || !maximum_biquad_alpha.is_finite() {
        return Err(AppConfigError::InvalidFilterQ { value: filter_q });
    }

    Ok(())
}

pub(crate) fn validate_vocoder_sample_rate(
    freq_high: Hertz,
    sample_rate: u32,
) -> Result<(), AppConfigError> {
    let sample_rate_hz = sample_rate as f32;
    let nyquist_hz = sample_rate_hz / 2.0;
    if freq_high.0 >= nyquist_hz {
        return Err(AppConfigError::InvalidFreqAboveNyquist {
            sample_rate,
            freq_high: freq_high.0,
            nyquist_hz,
        });
    }

    let max_safe_hz = sample_rate_hz * 0.45;
    if freq_high.0 > max_safe_hz {
        return Err(AppConfigError::InvalidFreqAboveSafetyCeiling {
            sample_rate,
            freq_high: freq_high.0,
            max_safe_hz,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::types::test_support::args_with_device;
    use super::super::types::AppConfig;
    use super::*;

    // Attack times must remain strictly positive (review regression).
    #[test]
    fn try_from_rejects_negative_vocoder_attack_ms() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.attack_ms = Some(-0.1);
        let result = AppConfig::try_from(&args);
        assert!(result.is_err(), "negative attack times should be rejected");
    }

    // Release times must remain finite (review regression).
    #[test]
    fn try_from_rejects_non_finite_vocoder_release_ms() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.release_ms = Some(f32::INFINITY);
        let result = AppConfig::try_from(&args);
        assert!(
            result.is_err(),
            "non-finite release times should be rejected"
        );
    }

    // Logarithmic band spacing requires strictly positive bounds (review regression).
    #[test]
    fn try_from_rejects_non_positive_vocoder_low_frequency() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.freq_low = Some(0.0);
        let result = AppConfig::try_from(&args);
        assert!(
            result.is_err(),
            "non-positive low frequencies should be rejected"
        );
    }

    // The high bound must remain above the low bound (review regression).
    #[test]
    fn try_from_rejects_vocoder_high_frequency_below_low_frequency() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.freq_low = Some(2_000.0);
        args.vocoder.freq_high = Some(1_000.0);
        let result = AppConfig::try_from(&args);
        assert!(
            result.is_err(),
            "high frequencies below the low bound should be rejected"
        );
    }

    // The filter Q must be strictly positive (review regression).
    #[test]
    fn try_from_rejects_non_positive_vocoder_filter_q() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.filter_q = Some(0.0);
        let result = AppConfig::try_from(&args);
        assert!(
            result.is_err(),
            "non-positive filter Q values should be rejected"
        );
    }

    // The WebSocket server is intentionally loopback-only unless a later change makes this explicit.
    #[test]
    fn try_from_rejects_non_loopback_bind_address() {
        let mut args = args_with_device(Some("test"));
        args.network.ws_addr = Some("0.0.0.0:8889".parse().unwrap());
        let result = AppConfig::try_from(&args);
        assert!(
            matches!(
                result,
                Err(super::super::types::AppConfigError::NonLoopbackBindAddress(
                    _
                ))
            ),
            "non-loopback bind addresses should be rejected"
        );
    }

    // 48 kHz sample rate means Nyquist is 24 kHz
    #[test]
    fn validate_vocoder_sample_rate_rejects_freq_above_nyquist() {
        let result = validate_vocoder_sample_rate(Hertz(25_000.0), 48_000);
        assert!(
            matches!(result, Err(AppConfigError::InvalidFreqAboveNyquist { .. })),
            "frequencies above Nyquist should be rejected"
        );
    }

    // 48 kHz sample rate means the 45 percent safety ceiling is 21.6 kHz
    // 22 kHz is below Nyquist (24 kHz) but above the safety ceiling
    #[test]
    fn validate_vocoder_sample_rate_rejects_freq_above_safety_ceiling() {
        let result = validate_vocoder_sample_rate(Hertz(22_000.0), 48_000);
        assert!(
            matches!(
                result,
                Err(AppConfigError::InvalidFreqAboveSafetyCeiling { .. })
            ),
            "frequencies above the 45 percent safety ceiling should be rejected"
        );
    }

    // 18 kHz is well below the 21.6 kHz safety ceiling for 48 kHz
    #[test]
    fn validate_vocoder_sample_rate_accepts_valid_frequencies() {
        let result = validate_vocoder_sample_rate(Hertz(18_000.0), 48_000);
        assert!(result.is_ok(), "valid frequencies should be accepted");
    }
}
