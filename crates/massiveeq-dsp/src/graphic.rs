use massiveeq_core::GraphicEq;
use rustfft::{FftPlanner, num_complex::Complex64};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GraphicError {
    #[error("GraphicEQ contains no points")]
    Empty,
    #[error("GraphicEQ contains duplicate frequencies")]
    DuplicateFrequency,
    #[error("GraphicEQ contains a non-finite value")]
    NonFinite,
    #[error("GraphicEQ frequency {frequency} Hz is not below Nyquist ({nyquist} Hz)")]
    FrequencyAboveNyquist { frequency: f64, nyquist: f64 },
    #[error("GraphicEQ design error {error_db:.3} dB exceeds the 0.25 dB limit")]
    Inaccurate { error_db: f64 },
}

#[derive(Debug, Clone)]
pub struct GraphicDesign {
    pub sample_rate: u32,
    pub impulse: Vec<f32>,
    pub max_error_db: f64,
    pub latency_frames: u32,
}

/// Designs a deterministic minimum-phase FIR on a sub-Hz FFT grid. Keeping the
/// complete impulse avoids the low-frequency truncation error of the previous
/// fixed 2048-tap linear-phase implementation.
pub fn design_graphic_eq(
    equalizers: &[&GraphicEq],
    sample_rate: u32,
) -> Result<GraphicDesign, GraphicError> {
    for equalizer in equalizers {
        validate(equalizer, sample_rate)?;
    }
    let fft_len = ((sample_rate as usize) * 2).next_power_of_two().max(16_384);
    let half = fft_len / 2;
    let mut log_spectrum = vec![Complex64::new(0.0, 0.0); fft_len];
    for bin in 0..=half {
        let frequency = bin as f64 * sample_rate as f64 / fft_len as f64;
        let gain_db = equalizers
            .iter()
            .map(|equalizer| target_gain(equalizer, frequency))
            .sum::<f64>();
        log_spectrum[bin].re = gain_db * std::f64::consts::LN_10 / 20.0;
        if bin != 0 && bin != half {
            log_spectrum[fft_len - bin].re = log_spectrum[bin].re;
        }
    }

    let mut planner = FftPlanner::<f64>::new();
    planner.plan_fft_inverse(fft_len).process(&mut log_spectrum);
    let scale = 1.0 / fft_len as f64;
    for value in &mut log_spectrum {
        *value *= scale;
    }

    // Convert the real cepstrum into its causal/minimum-phase equivalent.
    for value in log_spectrum.iter_mut().take(half).skip(1) {
        *value *= 2.0;
    }
    for value in log_spectrum.iter_mut().skip(half + 1) {
        *value = Complex64::new(0.0, 0.0);
    }

    planner.plan_fft_forward(fft_len).process(&mut log_spectrum);
    for value in &mut log_spectrum {
        *value = value.exp();
    }
    let designed_spectrum = log_spectrum.clone();
    planner.plan_fft_inverse(fft_len).process(&mut log_spectrum);
    let impulse = log_spectrum
        .iter()
        .map(|value| (value.re * scale) as f32)
        .collect::<Vec<_>>();
    if !impulse.iter().all(|value| value.is_finite()) {
        return Err(GraphicError::NonFinite);
    }

    let mut max_error_db = 0.0_f64;
    let start = ((20.0 * fft_len as f64 / sample_rate as f64).ceil() as usize).min(half);
    let end = ((20_000.0_f64.min(sample_rate as f64 * 0.499) * fft_len as f64 / sample_rate as f64)
        .floor() as usize)
        .min(half);
    for (bin, measured) in designed_spectrum
        .iter()
        .enumerate()
        .take(end + 1)
        .skip(start)
    {
        let frequency = bin as f64 * sample_rate as f64 / fft_len as f64;
        let wanted = equalizers
            .iter()
            .map(|equalizer| target_gain(equalizer, frequency))
            .sum::<f64>();
        let actual = 20.0 * measured.norm().max(1e-30).log10();
        max_error_db = max_error_db.max((actual - wanted).abs());
    }
    if max_error_db > 0.25 {
        return Err(GraphicError::Inaccurate {
            error_db: max_error_db,
        });
    }
    Ok(GraphicDesign {
        sample_rate,
        impulse,
        max_error_db,
        latency_frames: 0,
    })
}

pub fn target_gain(equalizer: &GraphicEq, frequency: f64) -> f64 {
    let points = &equalizer.points;
    if frequency <= points[0].frequency {
        return points[0].gain_db;
    }
    if frequency >= points[points.len() - 1].frequency {
        return points[points.len() - 1].gain_db;
    }
    for pair in points.windows(2) {
        if frequency >= pair[0].frequency && frequency <= pair[1].frequency {
            let t =
                (frequency / pair[0].frequency).ln() / (pair[1].frequency / pair[0].frequency).ln();
            return pair[0].gain_db + t * (pair[1].gain_db - pair[0].gain_db);
        }
    }
    points[points.len() - 1].gain_db
}

fn validate(equalizer: &GraphicEq, sample_rate: u32) -> Result<(), GraphicError> {
    if equalizer.points.is_empty() {
        return Err(GraphicError::Empty);
    }
    let nyquist = sample_rate as f64 * 0.5;
    for (index, point) in equalizer.points.iter().enumerate() {
        if !point.frequency.is_finite() || !point.gain_db.is_finite() {
            return Err(GraphicError::NonFinite);
        }
        if point.frequency >= nyquist {
            return Err(GraphicError::FrequencyAboveNyquist {
                frequency: point.frequency,
                nyquist,
            });
        }
        if index > 0 && point.frequency <= equalizer.points[index - 1].frequency {
            return Err(GraphicError::DuplicateFrequency);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use massiveeq_core::parse_text;

    #[test]
    fn fifteen_band_fixture_meets_tolerance_at_all_rates() {
        let profile = parse_text(
            "test",
            "GraphicEQ: 25 6; 40 4.5; 63 3; 100 1.5; 160 0; 250 0; 400 0; 630 0; 1000 0; 1600 0; 2500 0; 4000 0; 6300 1.5; 10000 3; 16000 3",
        );
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let result = design_graphic_eq(&[&profile.graphic_eqs[0]], rate).unwrap();
            assert!(
                result.max_error_db <= 0.25,
                "{rate}: {}",
                result.max_error_db
            );
        }
    }

    #[test]
    fn response_is_constant_outside_outer_bands() {
        let profile = parse_text("test", "GraphicEQ: 100 3; 1000 -2");
        let equalizer = &profile.graphic_eqs[0];
        assert_eq!(target_gain(equalizer, 20.0), 3.0);
        assert_eq!(target_gain(equalizer, 20_000.0), -2.0);
    }
}
