use massiveeq_core::{Filter, FilterKind};
use rustfft::num_complex::Complex64;
use std::f64::consts::PI;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BiquadError {
    #[error("filter frequency {frequency} Hz is not below Nyquist ({nyquist} Hz)")]
    FrequencyAboveNyquist { frequency: f64, nyquist: f64 },
    #[error("filter contains a non-finite parameter")]
    NonFinite,
    #[error("filter coefficients are unstable or non-finite")]
    Unstable,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoefficients {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl BiquadCoefficients {
    pub fn from_filter(filter: &Filter, sample_rate: f64) -> Result<Self, BiquadError> {
        if !sample_rate.is_finite()
            || !filter.frequency.is_finite()
            || !filter.gain_db.is_finite()
            || !filter.q.is_finite()
        {
            return Err(BiquadError::NonFinite);
        }
        let nyquist = sample_rate * 0.5;
        if filter.frequency >= nyquist {
            return Err(BiquadError::FrequencyAboveNyquist {
                frequency: filter.frequency,
                nyquist,
            });
        }

        let omega = 2.0 * PI * filter.frequency / sample_rate;
        let cos = omega.cos();
        let sin = omega.sin();
        let alpha = sin / (2.0 * filter.q);
        let amp = 10.0_f64.powf(filter.gain_db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match filter.kind {
            FilterKind::Peaking => (
                1.0 + alpha * amp,
                -2.0 * cos,
                1.0 - alpha * amp,
                1.0 + alpha / amp,
                -2.0 * cos,
                1.0 - alpha / amp,
            ),
            FilterKind::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterKind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterKind::AllPass => (
                1.0 - alpha,
                -2.0 * cos,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::LowShelf => {
                let beta = 2.0 * amp.sqrt() * alpha;
                (
                    amp * ((amp + 1.0) - (amp - 1.0) * cos + beta),
                    2.0 * amp * ((amp - 1.0) - (amp + 1.0) * cos),
                    amp * ((amp + 1.0) - (amp - 1.0) * cos - beta),
                    (amp + 1.0) + (amp - 1.0) * cos + beta,
                    -2.0 * ((amp - 1.0) + (amp + 1.0) * cos),
                    (amp + 1.0) + (amp - 1.0) * cos - beta,
                )
            }
            FilterKind::HighShelf => {
                let beta = 2.0 * amp.sqrt() * alpha;
                (
                    amp * ((amp + 1.0) + (amp - 1.0) * cos + beta),
                    -2.0 * amp * ((amp - 1.0) + (amp + 1.0) * cos),
                    amp * ((amp + 1.0) + (amp - 1.0) * cos - beta),
                    (amp + 1.0) - (amp - 1.0) * cos + beta,
                    2.0 * ((amp - 1.0) - (amp + 1.0) * cos),
                    (amp + 1.0) - (amp - 1.0) * cos - beta,
                )
            }
        };
        let coefficients = Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        };
        if !coefficients.is_finite() || !coefficients.is_stable() {
            return Err(BiquadError::Unstable);
        }
        Ok(coefficients)
    }

    pub fn transfer(self, frequency: f64, sample_rate: f64) -> Complex64 {
        let omega = 2.0 * PI * frequency / sample_rate;
        let z1 = Complex64::from_polar(1.0, -omega);
        let z2 = z1 * z1;
        (self.b0 + self.b1 * z1 + self.b2 * z2) / (1.0 + self.a1 * z1 + self.a2 * z2)
    }

    fn is_finite(self) -> bool {
        [self.b0, self.b1, self.b2, self.a1, self.a2]
            .into_iter()
            .all(f64::is_finite)
    }

    fn is_stable(self) -> bool {
        let discriminant = Complex64::new(self.a1 * self.a1 - 4.0 * self.a2, 0.0).sqrt();
        let p1 = (-self.a1 + discriminant) * 0.5;
        let p2 = (-self.a1 - discriminant) * 0.5;
        p1.norm() < 1.0 && p2.norm() < 1.0
    }
}

#[derive(Debug, Clone)]
pub struct Biquad {
    coefficients: BiquadCoefficients,
    s1: f64,
    s2: f64,
}

impl Biquad {
    pub fn new(coefficients: BiquadCoefficients) -> Self {
        Self {
            coefficients,
            s1: 0.0,
            s2: 0.0,
        }
    }

    #[inline]
    pub fn process(&mut self, sample: f32) -> f32 {
        let input = sample as f64;
        let output = self.coefficients.b0 * input + self.s1;
        self.s1 = self.coefficients.b1 * input - self.coefficients.a1 * output + self.s2;
        self.s2 = self.coefficients.b2 * input - self.coefficients.a2 * output;
        output as f32
    }

    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use massiveeq_core::parse_text;

    #[test]
    fn peaking_filter_hits_center_gain_at_all_rates() {
        let profile = parse_text("test", "Filter: ON PK Fc 1000 Hz Gain 6 dB Q 1");
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let coefficients = BiquadCoefficients::from_filter(&profile.filters[0], rate).unwrap();
            let gain = 20.0 * coefficients.transfer(1000.0, rate).norm().log10();
            assert!((gain - 6.0).abs() < 1e-9, "{rate}: {gain}");
        }
    }

    #[test]
    fn rejects_frequency_at_nyquist() {
        let profile = parse_text("test", "Filter: ON PK Fc 24000 Hz Gain 6 dB Q 1");
        assert!(BiquadCoefficients::from_filter(&profile.filters[0], 48_000.0).is_err());
    }

    #[test]
    fn every_filter_family_matches_analytic_landmarks_at_all_rates() {
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let gain = |command: &str, frequency: f64| {
                let profile = parse_text("golden", command);
                let coefficients =
                    BiquadCoefficients::from_filter(&profile.filters[0], rate).unwrap();
                20.0 * coefficients
                    .transfer(frequency, rate)
                    .norm()
                    .max(1e-30)
                    .log10()
            };
            assert!((gain("Filter: ON PK Fc 1000 Hz Gain 9 dB Q 2", 1000.0) - 9.0).abs() < 0.1);
            assert!(
                (gain("Filter: ON LSC Fc 100 Hz Gain 6 dB Q 0.70710678", 1.0) - 6.0).abs() < 0.1
            );
            assert!(
                (gain(
                    "Filter: ON HSC Fc 8000 Hz Gain -6 dB Q 0.70710678",
                    rate * 0.49
                ) + 6.0)
                    .abs()
                    < 0.1
            );
            assert!((gain("Filter: ON LPQ Fc 1000 Hz Q 0.70710678", 1000.0) + 3.0103).abs() < 0.1);
            assert!((gain("Filter: ON HPQ Fc 1000 Hz Q 0.70710678", 1000.0) + 3.0103).abs() < 0.1);
            assert!(gain("Filter: ON BP Fc 1000 Hz Q 2", 1000.0).abs() < 0.1);
            assert!(gain("Filter: ON NO Fc 1000 Hz Q 2", 1000.0) < -100.0);
            for frequency in [20.0, 1000.0, 10_000.0] {
                assert!(gain("Filter: ON AP Fc 1000 Hz Q 2", frequency).abs() < 0.001);
            }
        }
    }
}
