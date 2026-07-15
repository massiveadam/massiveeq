use crate::{CompileError, compiler::CompiledChannel};
use rustfft::{FftPlanner, num_complex::Complex64};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsePoint {
    pub frequency: f64,
    pub gain_db: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelResponse {
    pub source_preamp_db: f64,
    pub uncorrected_peak_db: f64,
    pub response: Vec<ResponsePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledAnalysis {
    pub channels: Vec<ChannelResponse>,
    pub match_gain_db: f64,
    pub manual_trim_db: f64,
    pub safety_attenuation_db: f64,
    pub effective_gain_db: f64,
    pub final_peak_db: f64,
    pub headroom_limited: bool,
}

struct FirTransfer {
    values: Vec<Complex64>,
    fft_len: usize,
    sample_rate: f64,
}

impl FirTransfer {
    fn new(impulse: &[f32], sample_rate: u32) -> Self {
        let fft_len = (impulse.len().saturating_mul(2))
            .max(sample_rate as usize * 2)
            .next_power_of_two();
        let mut values = vec![Complex64::new(0.0, 0.0); fft_len];
        for (value, sample) in values.iter_mut().zip(impulse) {
            value.re = *sample as f64;
        }
        FftPlanner::<f64>::new()
            .plan_fft_forward(fft_len)
            .process(&mut values);
        Self {
            values,
            fft_len,
            sample_rate: sample_rate as f64,
        }
    }

    fn at(&self, frequency: f64) -> Complex64 {
        let position = frequency * self.fft_len as f64 / self.sample_rate;
        let lower = position.floor() as usize;
        let upper = (lower + 1).min(self.fft_len / 2);
        let amount = position - lower as f64;
        self.values[lower] * (1.0 - amount) + self.values[upper] * amount
    }
}

pub(crate) fn analyze_compiled(
    channels: &[CompiledChannel],
    source_preamps: &[f64],
    sample_rate: u32,
    manual_trim_db: f64,
) -> Result<CompiledAnalysis, CompileError> {
    let firs = channels
        .iter()
        .map(|channel| {
            channel
                .convolutions
                .iter()
                .map(|kernel| FirTransfer::new(&kernel.impulse, sample_rate))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let max_frequency = 20_000.0_f64.min(sample_rate as f64 * 0.499);
    let mut energy_frequencies = log_grid(20.0, max_frequency, 16_384);
    for channel in channels {
        energy_frequencies.extend(channel.peak_candidates.iter().copied());
    }
    energy_frequencies.sort_by(f64::total_cmp);
    energy_frequencies.dedup_by(|left, right| (*left - *right).abs() < 1e-9);

    let transfer = |channel_index: usize, frequency: f64| {
        let channel = &channels[channel_index];
        let mut response =
            Complex64::from_polar(10.0_f64.powf(source_preamps[channel_index] / 20.0), 0.0);
        for coefficients in &channel.biquads {
            response *= coefficients.transfer(frequency, sample_rate as f64);
        }
        for fir in &firs[channel_index] {
            response *= fir.at(frequency);
        }
        response
    };

    let mut bypass_energy = 0.0;
    let mut profile_energy = 0.0;
    let mut previous_log = None;
    for frequency in &energy_frequencies {
        let log_frequency = frequency.ln();
        let width = previous_log
            .map(|value| log_frequency - value)
            .unwrap_or(0.0);
        previous_log = Some(log_frequency);
        let weight = k_weight_power(*frequency, sample_rate as f64) * width;
        bypass_energy += weight * channels.len() as f64;
        profile_energy += (0..channels.len())
            .map(|channel| transfer(channel, *frequency).norm_sqr() * weight)
            .sum::<f64>();
    }
    if !bypass_energy.is_finite() || !profile_energy.is_finite() || profile_energy <= 0.0 {
        return Err(CompileError::NonFiniteResponse);
    }
    let match_gain_db = -10.0 * (profile_energy / bypass_energy).log10();

    let mut peak = f64::NEG_INFINITY;
    for channel_index in 0..channels.len() {
        for frequency in &energy_frequencies {
            peak = peak.max(linear_to_db(transfer(channel_index, *frequency).norm()));
        }
    }
    // Iteratively refine the largest composite peak in log-frequency space.
    for channel_index in 0..channels.len() {
        let coarse = log_grid(20.0, max_frequency, 8_192);
        for window in coarse.windows(3) {
            let center = linear_to_db(transfer(channel_index, window[1]).norm());
            if center >= linear_to_db(transfer(channel_index, window[0]).norm())
                && center >= linear_to_db(transfer(channel_index, window[2]).norm())
            {
                peak = peak.max(refine_peak(window[0], window[2], |frequency| {
                    linear_to_db(transfer(channel_index, frequency).norm())
                }));
            }
        }
    }
    let safety_attenuation_db = (-1.0 - peak - match_gain_db - manual_trim_db).min(0.0);
    let effective_gain_db = match_gain_db + manual_trim_db + safety_attenuation_db;
    let final_peak_db = peak + effective_gain_db;
    if ![
        match_gain_db,
        safety_attenuation_db,
        effective_gain_db,
        final_peak_db,
    ]
    .into_iter()
    .all(f64::is_finite)
    {
        return Err(CompileError::NonFiniteResponse);
    }

    let graph_frequencies = log_grid(20.0, max_frequency, 512);
    let responses = channels
        .iter()
        .enumerate()
        .map(|(channel_index, _)| ChannelResponse {
            source_preamp_db: source_preamps[channel_index],
            uncorrected_peak_db: graph_frequencies
                .iter()
                .map(|frequency| linear_to_db(transfer(channel_index, *frequency).norm()))
                .fold(f64::NEG_INFINITY, f64::max),
            response: graph_frequencies
                .iter()
                .map(|frequency| ResponsePoint {
                    frequency: *frequency,
                    // Match Squiglink's editing model: graph only the EQ/IR
                    // transfer. Source preamp, perceptual match, correction,
                    // and safety attenuation belong to the separate preamp
                    // readout and must not vertically translate the curve.
                    gain_db: linear_to_db(transfer(channel_index, *frequency).norm())
                        - source_preamps[channel_index],
                })
                .collect(),
        })
        .collect();
    Ok(CompiledAnalysis {
        channels: responses,
        match_gain_db,
        manual_trim_db,
        safety_attenuation_db,
        effective_gain_db,
        final_peak_db,
        headroom_limited: safety_attenuation_db < -0.005,
    })
}

fn refine_peak(mut low: f64, mut high: f64, value: impl Fn(f64) -> f64) -> f64 {
    let ratio = (5.0_f64.sqrt() - 1.0) * 0.5;
    let mut left = (low.ln() + (1.0 - ratio) * (high.ln() - low.ln())).exp();
    let mut right = (low.ln() + ratio * (high.ln() - low.ln())).exp();
    let mut left_value = value(left);
    let mut right_value = value(right);
    for _ in 0..48 {
        if left_value < right_value {
            low = left;
            left = right;
            left_value = right_value;
            right = (low.ln() + ratio * (high.ln() - low.ln())).exp();
            right_value = value(right);
        } else {
            high = right;
            right = left;
            right_value = left_value;
            left = (low.ln() + (1.0 - ratio) * (high.ln() - low.ln())).exp();
            left_value = value(left);
        }
    }
    left_value.max(right_value)
}

fn log_grid(low: f64, high: f64, count: usize) -> Vec<f64> {
    (0..count)
        .map(|index| low * (high / low).powf(index as f64 / (count - 1) as f64))
        .collect()
}

fn k_weight_power(frequency: f64, sample_rate: f64) -> f64 {
    let (shelf_b, shelf_a, rlb_b, rlb_a) = bs1770_k_weighting_coefficients(sample_rate);
    (normalized_transfer(frequency, sample_rate, shelf_b, shelf_a)
        * normalized_transfer(frequency, sample_rate, rlb_b, rlb_a))
    .norm_sqr()
}

fn bs1770_k_weighting_coefficients(sample_rate: f64) -> ([f64; 3], [f64; 3], [f64; 3], [f64; 3]) {
    // ITU-R BS.1770-5 Annex 1 filter parameters. The bilinear-transform
    // equations reproduce the normative 48 kHz coefficients and retain the
    // same analogue corner frequencies at every supported PipeWire rate.
    const SHELF_F0: f64 = 1_681.974_450_955_533;
    const SHELF_GAIN_DB: f64 = 3.999_843_853_973_347;
    const SHELF_Q: f64 = 0.707_175_236_955_419_6;
    const SHELF_VB_EXPONENT: f64 = 0.499_666_774_154_541_6;
    const RLB_F0: f64 = 38.135_470_876_024_44;
    const RLB_Q: f64 = 0.500_327_037_323_877_3;

    let k = (PI * SHELF_F0 / sample_rate).tan();
    let vh = 10.0_f64.powf(SHELF_GAIN_DB / 20.0);
    let vb = vh.powf(SHELF_VB_EXPONENT);
    let a0 = 1.0 + k / SHELF_Q + k * k;
    let shelf_b = [
        (vh + vb * k / SHELF_Q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / SHELF_Q + k * k) / a0,
    ];
    let shelf_a = [
        1.0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / SHELF_Q + k * k) / a0,
    ];

    let k = (PI * RLB_F0 / sample_rate).tan();
    let a0 = 1.0 + k / RLB_Q + k * k;
    let rlb_b = [1.0, -2.0, 1.0];
    let rlb_a = [
        1.0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / RLB_Q + k * k) / a0,
    ];
    (shelf_b, shelf_a, rlb_b, rlb_a)
}

fn normalized_transfer(f: f64, rate: f64, b: [f64; 3], a: [f64; 3]) -> Complex64 {
    let z1 = Complex64::from_polar(1.0, -2.0 * PI * f / rate);
    let z2 = z1 * z1;
    (b[0] + b[1] * z1 + b[2] * z2) / (a[0] + a[1] * z1 + a[2] * z2)
}

fn linear_to_db(value: f64) -> f64 {
    20.0 * value.max(1e-30).log10()
}

#[cfg(test)]
mod tests {
    use super::bs1770_k_weighting_coefficients;

    #[test]
    fn k_weighting_reproduces_normative_48khz_coefficients() {
        let (shelf_b, shelf_a, rlb_b, rlb_a) = bs1770_k_weighting_coefficients(48_000.0);
        let expected_shelf_b = [
            1.535_124_859_586_97,
            -2.691_696_189_406_38,
            1.198_392_810_852_85,
        ];
        let expected_shelf_a = [1.0, -1.690_659_293_182_41, 0.732_480_774_215_85];
        let expected_rlb_a = [1.0, -1.990_047_454_833_98, 0.990_072_250_366_21];
        for (actual, expected) in shelf_b.into_iter().zip(expected_shelf_b) {
            assert!((actual - expected).abs() < 1e-12);
        }
        for (actual, expected) in shelf_a.into_iter().zip(expected_shelf_a) {
            assert!((actual - expected).abs() < 1e-12);
        }
        assert_eq!(rlb_b, [1.0, -2.0, 1.0]);
        for (actual, expected) in rlb_a.into_iter().zip(expected_rlb_a) {
            assert!((actual - expected).abs() < 1e-12);
        }
    }
}
