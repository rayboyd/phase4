//! A filter-bank analyser built from a bank
//! of fixed bandpass filters, one per band, each followed by an envelope
//! follower that tracks the band's amplitude over time. This is the classic analogue
//! vocoder architecture, filter then rectify then smooth, run per audio
//! sample rather than per FFT window.
//!
//! Band centres are spaced logarithmically rather than linearly, since pitch
//! perception is logarithmic, so a linear spacing would waste most of the
//! bands on the highest octave. Per-sample envelope updates avoid an FFT
//! window, but response time still depends on filter settling, attack and
//! release, buffering and output scheduling. The envelopes are not normalised
//! spectral magnitudes, and filter Q affects both bandwidth and gain.

use crate::config::{AppConfigError, VocoderConfig};
use crate::dsp::units::{Hertz, Milliseconds};
use crate::dsp::BAND_COUNT;
use biquad::{Biquad, Coefficients, DirectForm1, ToHertz, Type};

/// One-pole envelope follower with separate attack and release coefficients.
#[derive(Default)]
pub(crate) struct EnvelopeFollower {
    value: f32,
}

impl EnvelopeFollower {
    pub(crate) fn new() -> Self {
        Self { value: 0.0 }
    }

    /// Moves `value` a fraction of the way towards `rectified`, using
    /// `attack` while rising and `release` while falling.
    ///
    /// The standard one-pole follower is `value += coeff * (input -
    /// value)`. Using a smaller coefficient while falling than while rising
    /// (or vice versa) is what gives the follower its asymmetric attack and
    /// release shape, the same behaviour as a hardware envelope follower or
    /// compressor.
    pub(crate) fn process_sample(&mut self, rectified: f32, attack: f32, release: f32) -> f32 {
        let coeff = if rectified > self.value {
            attack
        } else {
            release
        };
        self.value += coeff * (rectified - self.value);
        self.value
    }

    pub(crate) fn reset(&mut self) {
        self.value = 0.0;
    }
}

/// Converts a time constant in milliseconds to a per-sample one-pole
/// coefficient, `coeff = 1 - exp(-1 / (tau * sample_rate))`.
///
/// This is the standard RC step-response formula. Feeding a one-pole
/// follower a constant target with this coefficient reaches roughly 63
/// percent of the target after `time` has elapsed, which is the usual
/// definition of a filter's time constant. Larger `time` gives a smaller
/// coefficient and therefore a slower-moving envelope.
pub(crate) fn envelope_coeff(time: Milliseconds, sample_rate: Hertz) -> f32 {
    let exponent = -1000.0 / (f64::from(time.0) * f64::from(sample_rate.0));
    -exponent.exp_m1() as f32
}

/// Builds the exact f32 coefficients used by the analyser and rejects poles
/// on or outside the unit circle before the pipeline starts.
pub(crate) fn bandpass_coefficients(
    sample_rate: u32,
    config: &VocoderConfig,
) -> Result<[Coefficients<f32>; BAND_COUNT], AppConfigError> {
    let log_low = config.freq_low.0.ln();
    let log_high = config.freq_high.0.ln();
    let mut coefficients = [Coefficients {
        a1: 0.0,
        a2: 0.0,
        b0: 0.0,
        b1: 0.0,
        b2: 0.0,
    }; BAND_COUNT];

    for (band, coefficients) in coefficients.iter_mut().enumerate() {
        let position = band as f32 / (BAND_COUNT as f32 - 1.0);
        let frequency_hz = (log_low + position * (log_high - log_low)).exp();
        let invalid_coefficients = || AppConfigError::InvalidVocoderBandCoefficients {
            band,
            frequency_hz,
            sample_rate,
        };
        let candidate = Coefficients::<f32>::from_params(
            Type::BandPass,
            sample_rate.hz(),
            frequency_hz.hz(),
            config.filter_q,
        )
        .map_err(|_| invalid_coefficients())?;

        if !coefficients_are_stable(&candidate) {
            return Err(invalid_coefficients());
        }
        *coefficients = candidate;
    }

    Ok(coefficients)
}

fn coefficients_are_stable(coefficients: &Coefficients<f32>) -> bool {
    let finite = [
        coefficients.a1,
        coefficients.a2,
        coefficients.b0,
        coefficients.b1,
        coefficients.b2,
    ]
    .into_iter()
    .all(f32::is_finite);

    // Jury stability conditions for z*z + a1*z + a2. Widen the stored f32
    // coefficients so cancellation cannot hide a pole at the boundary.
    let a1 = f64::from(coefficients.a1);
    let a2 = f64::from(coefficients.a2);
    finite && 1.0 + a1 + a2 > 0.0 && 1.0 - a1 + a2 > 0.0 && 1.0 - a2 > 0.0
}

/// Per-channel vocoder analyser.
///
/// Splits the input signal into [`BAND_COUNT`] logarithmically spaced
/// frequency bands, each tracked by an envelope follower. Produces
/// [`BAND_COUNT`] envelope values, one per logarithmically spaced band.
pub struct VocoderAnalyser {
    /// One bandpass filter per band, coefficients from the Audio EQ Cookbook
    /// (Robert Bristow-Johnson) via the `biquad` crate. `DirectForm1` mirrors
    /// the previous hand-written state layout while delegating the
    /// coefficient maths.
    filters: [DirectForm1<f32>; BAND_COUNT],

    /// One envelope follower per band, tracking its filter's rectified output.
    envelopes: [EnvelopeFollower; BAND_COUNT],

    /// Latest envelope value per band, same order as `filters`.
    bins: [f32; BAND_COUNT],

    /// Shared one-pole attack coefficient, see [`envelope_coeff`].
    attack_coeff: f32,

    /// Shared one-pole release coefficient, see [`envelope_coeff`].
    release_coeff: f32,
}

impl VocoderAnalyser {
    /// Creates a new vocoder analyser for the given sample rate and configuration.
    ///
    /// # Panics
    ///
    /// Panics if the supplied configuration violates the validated assumptions
    /// required to construct finite, stable bandpass coefficients.
    #[must_use]
    pub fn new(sample_rate: u32, config: &VocoderConfig) -> Self {
        let sr = Hertz(sample_rate as f32);

        let filters = bandpass_coefficients(sample_rate, config)
            .expect("validated vocoder configuration should produce stable biquad coefficients")
            .map(DirectForm1::new);

        let envelopes = std::array::from_fn(|_| EnvelopeFollower::new());

        let attack_coeff = envelope_coeff(config.attack_ms, sr);
        let release_coeff = envelope_coeff(config.release_ms, sr);

        Self {
            filters,
            envelopes,
            bins: [0.0; BAND_COUNT],
            attack_coeff,
            release_coeff,
        }
    }

    /// Processes one channel from an interleaved audio buffer.
    ///
    /// Each band is updated sample by sample across the provided chunk. After
    /// this returns, [`current_bins`](Self::current_bins) exposes the latest
    /// envelope follower state for each band.
    pub fn process_interleaved(&mut self, buffer: &[f32], channel: usize, total_channels: usize) {
        let mut i = channel;
        while i < buffer.len() {
            let sample = buffer[i];
            for (band_idx, filter) in self.filters.iter_mut().enumerate() {
                let filtered = filter.run(sample);
                // Rectify (abs) so the envelope follower tracks the band's
                // amplitude rather than its raw, sign-alternating waveform.
                self.bins[band_idx] = self.envelopes[band_idx].process_sample(
                    filtered.abs(),
                    self.attack_coeff,
                    self.release_coeff,
                );
            }
            i += total_channels;
        }
    }

    /// Returns the current envelope value for each vocoder band.
    ///
    /// These values are the follower states after the most recent call to
    /// [`process_interleaved`](Self::process_interleaved). They describe the
    /// end-of-chunk envelope state, not an aggregate over the whole chunk.
    #[must_use]
    pub fn current_bins(&self) -> &[f32] {
        &self.bins
    }

    /// Clears filter and envelope state.
    ///
    /// Called when analysis resumes after being paused, so residual energy
    /// from before the pause does not leak into the first frames back.
    pub fn reset(&mut self) {
        for filter in &mut self.filters {
            filter.reset_state();
        }
        for env in &mut self.envelopes {
            env.reset();
        }
        self.bins.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORTED_TEST_SAMPLE_RATES: [u32; 5] = [22_050, 44_100, 48_000, 96_000, 192_000];

    #[test]
    fn default_filters_remain_stable_across_audio_sample_rates() {
        for sample_rate in SUPPORTED_TEST_SAMPLE_RATES {
            bandpass_coefficients(sample_rate, &VocoderConfig::default())
                .unwrap_or_else(|error| panic!("default filters failed at {sample_rate}: {error}"));
        }
    }

    #[test]
    fn stability_check_rejects_unit_circle_and_outside_poles() {
        for (a1, a2) in [(-2.0, 1.0), (2.0, 1.0), (0.0, -1.0), (-1.5, 0.4)] {
            let coefficients = Coefficients {
                a1,
                a2,
                b0: 1.0,
                b1: 0.0,
                b2: 0.0,
            };
            assert!(!coefficients_are_stable(&coefficients));
        }
    }

    #[test]
    fn stability_check_rejects_non_finite_feedforward_coefficients() {
        let coefficients = Coefficients {
            a1: 0.0,
            a2: 0.0,
            b0: f32::NAN,
            b1: 0.0,
            b2: 0.0,
        };
        assert!(!coefficients_are_stable(&coefficients));
    }

    #[test]
    fn envelope_coeff_remains_positive_for_the_largest_finite_time() {
        let coefficient = envelope_coeff(Milliseconds(f32::MAX), Hertz(44_100.0));

        assert!(coefficient.is_finite());
        assert!(
            coefficient > 0.0,
            "a finite time constant should not collapse to a zero coefficient"
        );
        assert!(coefficient <= 1.0);
    }
}
