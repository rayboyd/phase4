//! Zero-cost unit newtypes for audio domain values.
//!
//! Wrapping raw `f32` values in these types lets the compiler enforce unit
//! correctness at boundaries such as [`crate::config::VocoderConfig`] and
//! [`crate::dsp::vocoder::envelope_coeff`]. At runtime the wrapper is erased
//! entirely; each newtype compiles to the same machine code as a bare `f32`.

/// A duration expressed in milliseconds.
///
/// # Type safety
///
/// Passing a [`Hertz`] value where [`Milliseconds`] is expected is a compile
/// error, even though both wrap an `f32`:
///
/// ```compile_fail
/// use phase4::dsp::units::{Hertz, Milliseconds};
/// fn takes_time(_: Milliseconds) {}
/// takes_time(Hertz(440.0));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Milliseconds(pub f32);

/// A frequency expressed in Hertz.
///
/// # Type safety
///
/// Passing a [`Milliseconds`] value where [`Hertz`] is expected is a compile
/// error, even though both wrap an `f32`:
///
/// ```compile_fail
/// use phase4::dsp::units::{Hertz, Milliseconds};
/// fn takes_freq(_: Hertz) {}
/// takes_freq(Milliseconds(30.0));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Hertz(pub f32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn milliseconds_inner_value_accessible() {
        assert_eq!(Milliseconds(30.0).0, 30.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn hertz_inner_value_accessible() {
        assert_eq!(Hertz(440.0).0, 440.0);
    }
}
