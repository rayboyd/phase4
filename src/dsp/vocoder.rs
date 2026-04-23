//! Logarithmic band spacing matches human perception, producing visually balanced
//! output without the downstream mapper needing to compensate. Envelope followers
//! respond per-sample, giving lower latency and smoother animation than windowed
//! transforms.

use biquad::{Biquad, Coefficients, DirectForm1, ToHertz, Type};

use crate::config::VocoderConfig;

/// Number of frequency bands in the filter bank.
pub const VOCODER_BANDS: usize = 64;

/// One-pole envelope follower with separate attack and release coefficients.
pub(crate) struct EnvelopeFollower {
    value: f32,
}

impl EnvelopeFollower {
    pub(crate) fn new() -> Self {
        Self { value: 0.0 }
    }

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

pub(crate) fn envelope_coeff(milliseconds: f32, sample_rate: f32) -> f32 {
    let tau_seconds = milliseconds / 1000.0;
    1.0 - (-1.0 / (tau_seconds * sample_rate)).exp()
}

/// Per-channel vocoder analyser.
///
/// Splits the input signal into [`VOCODER_BANDS`] logarithmically spaced
/// frequency bands, each tracked by an envelope follower. Produces
/// [`VOCODER_BANDS`] envelope values, one per logarithmically spaced band.
pub struct VocoderAnalyser {
    // DirectForm1 mirrors the previous hand-written state layout while delegating the coefficient maths.
    filters: Vec<DirectForm1<f32>>,
    envelopes: Vec<EnvelopeFollower>,
    bins: Vec<f32>,
    attack_coeff: f32,
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
        let sr = sample_rate as f32;
        let log_low = config.freq_low.ln();
        let log_high = config.freq_high.ln();

        let filters = (0..VOCODER_BANDS)
            .map(|i| {
                let t = i as f32 / (VOCODER_BANDS as f32 - 1.0);
                let centre = (log_low + t * (log_high - log_low)).exp();
                let coefficients = Coefficients::<f32>::from_params(
                    Type::BandPass,
                    sample_rate.hz(),
                    centre.hz(),
                    config.filter_q,
                )
                .expect("validated vocoder configuration should produce valid biquad coefficients");
                DirectForm1::new(coefficients)
            })
            .collect();

        let envelopes = (0..VOCODER_BANDS)
            .map(|_| EnvelopeFollower::new())
            .collect();

        let attack_coeff = envelope_coeff(config.attack_ms, sr);
        let release_coeff = envelope_coeff(config.release_ms, sr);

        Self {
            filters,
            envelopes,
            bins: vec![0.0; VOCODER_BANDS],
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

    pub fn reset(&mut self) {
        for filter in &mut self.filters {
            filter.reset_state();
        }
        for env in &mut self.envelopes {
            env.reset();
        }
        self.bins.fill(0.0);
    }

    /// Returns the centre frequency of each vocoder band.
    ///
    /// Recomputed from the supplied frequency range rather than stored state,
    /// keeping the runtime struct free of test-only fields.
    #[cfg(test)]
    #[expect(dead_code, reason = "0.0.2 testing pass will add callers")]
    pub(crate) fn centre_frequencies(freq_low: f32, freq_high: f32) -> Vec<f32> {
        let log_low = freq_low.ln();
        let log_high = freq_high.ln();
        (0..VOCODER_BANDS)
            .map(|i| {
                let t = i as f32 / (VOCODER_BANDS as f32 - 1.0);
                (log_low + t * (log_high - log_low)).exp()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {}
