//! A cheap alternative to an FFT-based spectrum analyser, built from a bank
//! of fixed bandpass filters, one per band, each followed by an envelope
//! follower that tracks the band's amplitude over time. This is the classic analogue
//! vocoder architecture, filter then rectify then smooth, run per audio
//! sample rather than per FFT window.
//!
//! Band centres are spaced logarithmically rather than linearly, since pitch
//! perception is logarithmic, so a linear spacing would waste most of the
//! bands on the highest octave. Running the envelope followers per-sample
//! rather than per-window also gives lower latency and smoother animation
//! than a windowed transform.

use crate::config::VocoderConfig;
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
    /// required to construct the bandpass coefficients.
    #[must_use]
    pub fn new(sample_rate: u32, config: &VocoderConfig) -> Self {
        let sr = Hertz(sample_rate as f32);

        // Precompute the log-space bounds once. Each band centre below is
        // exp(log_low + t * (log_high - log_low)) for t stepping evenly
        // from 0 to 1, i.e. geometric (equal ratio) rather than equal Hz
        // spacing between bands.
        let log_low = config.freq_low.0.ln();
        let log_high = config.freq_high.0.ln();

        let filters = std::array::from_fn(|i| {
            let t = i as f32 / (BAND_COUNT as f32 - 1.0);
            let centre = (log_low + t * (log_high - log_low)).exp();

            // filter_q sets the bandpass width. Higher Q narrows the
            // band around centre, lower Q widens it.
            let coefficients = Coefficients::<f32>::from_params(
                Type::BandPass,
                sample_rate.hz(),
                centre.hz(),
                config.filter_q,
            )
            .expect("validated vocoder configuration should produce valid biquad coefficients");
            DirectForm1::new(coefficients)
        });

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
