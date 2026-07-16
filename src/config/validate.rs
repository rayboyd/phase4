use super::types::AppConfigError;
use crate::dsp::units::Hertz;

pub(super) fn is_strictly_positive(value: f32) -> bool {
    value.is_finite() && value > 0.0
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

    if !is_strictly_positive(filter_q) {
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

    // Review regression: attack times must remain strictly positive.
    #[test]
    fn try_from_rejects_negative_vocoder_attack_ms() {
        let mut args = args_with_device(Some("test"));
        args.vocoder.attack_ms = Some(-0.1);
        let result = AppConfig::try_from(&args);
        assert!(result.is_err(), "negative attack times should be rejected");
    }

    // Review regression: release times must remain finite.
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

    // Review regression: logarithmic band spacing requires strictly positive bounds.
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

    // Review regression: the high bound must remain above the low bound.
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

    // Review regression: the filter Q must be strictly positive.
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
