use crate::model::{Channel, Filter, FilterKind, GraphicEq, ProfileDocument};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsePoint {
    pub frequency: f64,
    pub gain_db: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelAnalysis {
    pub preamp_db: f64,
    pub peak_db: f64,
    pub response: Vec<ResponsePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileAnalysis {
    pub left: ChannelAnalysis,
    pub right: ChannelAnalysis,
    pub match_gain_db: f64,
    pub manual_trim_db: f64,
    pub safety_attenuation_db: f64,
    pub effective_gain_db: f64,
    pub headroom_limited: bool,
}

#[derive(Clone, Copy, Debug)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    fn abs2(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
    fn mul(self, rhs: Self) -> Self {
        Self::new(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re,
        )
    }
    fn div(self, rhs: Self) -> Self {
        let d = rhs.abs2();
        Self::new(
            (self.re * rhs.re + self.im * rhs.im) / d,
            (self.im * rhs.re - self.re * rhs.im) / d,
        )
    }
}

pub fn analyze_profile(
    profile: &ProfileDocument,
    sample_rate: f64,
    manual_trim_db: f64,
) -> ProfileAnalysis {
    analyze_profile_with_bins(profile, sample_rate, manual_trim_db, 1024)
}

/// A lower-resolution analysis intended for direct manipulation in the UI.
/// The service always recompiles the full-resolution response after the edit
/// debounce, while this keeps point dragging comfortably within one frame.
pub fn analyze_profile_preview(
    profile: &ProfileDocument,
    sample_rate: f64,
    manual_trim_db: f64,
) -> ProfileAnalysis {
    analyze_profile_with_bins(profile, sample_rate, manual_trim_db, 256)
}

fn analyze_profile_with_bins(
    profile: &ProfileDocument,
    sample_rate: f64,
    manual_trim_db: f64,
    bins: usize,
) -> ProfileAnalysis {
    let max_frequency = 20_000.0_f64.min(sample_rate * 0.475);
    let frequencies = (0..bins)
        .map(|index| 20.0 * (max_frequency / 20.0).powf(index as f64 / (bins - 1) as f64))
        .collect::<Vec<_>>();

    let left = analyze_channel(profile, Channel::Left, sample_rate, &frequencies);
    let right = analyze_channel(profile, Channel::Right, sample_rate, &frequencies);
    let bypass_energy = frequencies
        .iter()
        .map(|f| k_weight_power(*f, sample_rate))
        .sum::<f64>()
        * 2.0;
    let profile_energy = left
        .response
        .iter()
        .zip(&right.response)
        .zip(&frequencies)
        .map(|((l, r), f)| {
            let k = k_weight_power(*f, sample_rate);
            k * (db_to_power(l.gain_db + left.preamp_db) + db_to_power(r.gain_db + right.preamp_db))
        })
        .sum::<f64>();
    let match_gain_db = -10.0 * (profile_energy / bypass_energy).log10();
    let peak = left.peak_db.max(right.peak_db);
    let safety_attenuation_db = (-1.0 - peak - match_gain_db - manual_trim_db).min(0.0);
    let effective_gain_db = match_gain_db + manual_trim_db + safety_attenuation_db;

    ProfileAnalysis {
        left,
        right,
        match_gain_db,
        manual_trim_db,
        safety_attenuation_db,
        effective_gain_db,
        headroom_limited: safety_attenuation_db < -0.005,
    }
}

fn analyze_channel(
    profile: &ProfileDocument,
    channel: Channel,
    sample_rate: f64,
    frequencies: &[f64],
) -> ChannelAnalysis {
    let preamp_db = profile.preamp_for(channel);
    let response = frequencies
        .iter()
        .map(|frequency| {
            // Keep the plotted response anchored to the EQ controls. Source
            // preamp, perceptual matching, user correction, and safety gain
            // are represented separately and must never move the graph while
            // a filter is being dragged.
            let mut gain = 0.0;
            for filter in profile.filters_for(channel).filter(|filter| filter.enabled) {
                gain += filter_gain_db(filter, *frequency, sample_rate);
            }
            for eq in profile
                .graphic_eqs
                .iter()
                .filter(|eq| eq.channels.contains(channel))
            {
                gain += graphic_gain_db(eq, *frequency);
            }
            ResponsePoint {
                frequency: *frequency,
                gain_db: gain,
            }
        })
        .collect::<Vec<_>>();
    let peak_db = response
        .iter()
        .map(|point| point.gain_db + preamp_db)
        .fold(f64::NEG_INFINITY, f64::max);
    ChannelAnalysis {
        preamp_db,
        peak_db,
        response,
    }
}

pub fn filter_gain_db(filter: &Filter, frequency: f64, sample_rate: f64) -> f64 {
    let (b, a) = filter_coefficients(filter, sample_rate);
    let omega = 2.0 * PI * frequency / sample_rate;
    let z1 = Complex::new(omega.cos(), -omega.sin());
    let z2 = z1.mul(z1);
    let numerator = Complex::new(b[0], 0.0)
        .mul(Complex::new(1.0, 0.0))
        .mul(Complex::new(1.0, 0.0));
    let numerator = Complex::new(
        numerator.re + b[1] * z1.re + b[2] * z2.re,
        numerator.im + b[1] * z1.im + b[2] * z2.im,
    );
    let denominator = Complex::new(
        a[0] + a[1] * z1.re + a[2] * z2.re,
        a[1] * z1.im + a[2] * z2.im,
    );
    10.0 * numerator.div(denominator).abs2().max(1e-30).log10()
}

pub fn filter_coefficients(filter: &Filter, sample_rate: f64) -> ([f64; 3], [f64; 3]) {
    if !filter.enabled {
        return ([1.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    }
    let omega = 2.0 * PI * filter.frequency.min(sample_rate * 0.499) / sample_rate;
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
            let alpha = sin / (2.0 * filter.q);
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
            let alpha = sin / (2.0 * filter.q);
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
    ([b0 / a0, b1 / a0, b2 / a0], [1.0, a1 / a0, a2 / a0])
}

fn graphic_gain_db(eq: &GraphicEq, frequency: f64) -> f64 {
    if frequency < eq.points[0].frequency || frequency > eq.points[eq.points.len() - 1].frequency {
        return 0.0;
    }
    for pair in eq.points.windows(2) {
        if frequency >= pair[0].frequency && frequency <= pair[1].frequency {
            let t =
                (frequency / pair[0].frequency).ln() / (pair[1].frequency / pair[0].frequency).ln();
            return pair[0].gain_db + (pair[1].gain_db - pair[0].gain_db) * t;
        }
    }
    eq.points.last().map(|point| point.gain_db).unwrap_or(0.0)
}

fn k_weight_power(frequency: f64, sample_rate: f64) -> f64 {
    let (shelf_b, shelf_a, rlb_b, rlb_a) = bs1770_k_weighting_coefficients(sample_rate);
    db_to_power(
        coefficient_gain_db(shelf_b, shelf_a, frequency, sample_rate)
            + coefficient_gain_db(rlb_b, rlb_a, frequency, sample_rate),
    )
}

fn bs1770_k_weighting_coefficients(sample_rate: f64) -> ([f64; 3], [f64; 3], [f64; 3], [f64; 3]) {
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

fn coefficient_gain_db(b: [f64; 3], a: [f64; 3], frequency: f64, sample_rate: f64) -> f64 {
    let omega = 2.0 * PI * frequency / sample_rate;
    let z1 = Complex::new(omega.cos(), -omega.sin());
    let z2 = z1.mul(z1);
    let numerator = Complex::new(
        b[0] + b[1] * z1.re + b[2] * z2.re,
        b[1] * z1.im + b[2] * z2.im,
    );
    let denominator = Complex::new(
        a[0] + a[1] * z1.re + a[2] * z2.re,
        a[1] * z1.im + a[2] * z2.im,
    );
    10.0 * numerator.div(denominator).abs2().max(1e-30).log10()
}

fn db_to_power(db: f64) -> f64 {
    10.0_f64.powf(db / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_text;

    #[test]
    fn peaking_filter_hits_requested_center_gain() {
        let profile = parse_text("test", "Filter 1: ON PK Fc 1000 Hz Gain 6 dB Q 1");
        let filter = &profile.filters[0];
        assert!((filter_gain_db(filter, 1000.0, 48000.0) - 6.0).abs() < 0.001);
    }

    #[test]
    fn headroom_prevents_positive_composite_peak() {
        let profile = parse_text("test", "Filter 1: ON PK Fc 1000 Hz Gain 8 dB Q 1");
        let analysis = analyze_profile(&profile, 48000.0, 0.0);
        assert!(analysis.headroom_limited);
        assert!(analysis.left.peak_db + analysis.effective_gain_db <= -0.999);
    }

    #[test]
    fn display_response_ignores_preamp_and_output_matching() {
        let profile = parse_text(
            "test",
            "Preamp: -8 dB\nFilter 1: ON PK Fc 1000 Hz Gain 6 dB Q 1",
        );
        let first = analyze_profile(&profile, 48_000.0, 0.0);
        let adjusted = analyze_profile(&profile, 48_000.0, 4.0);
        let center = first
            .left
            .response
            .iter()
            .min_by(|left, right| {
                (left.frequency - 1_000.0)
                    .abs()
                    .total_cmp(&(right.frequency - 1_000.0).abs())
            })
            .unwrap();

        assert!((center.gain_db - 6.0).abs() < 0.02);
        assert_eq!(first.left.response.len(), adjusted.left.response.len());
        for (first, adjusted) in first.left.response.iter().zip(&adjusted.left.response) {
            assert!((first.gain_db - adjusted.gain_db).abs() < 1e-12);
        }
        assert!((first.left.peak_db - (center.gain_db - 8.0)).abs() < 0.02);
    }
}
