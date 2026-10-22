//! Frequency-response curves shown behind the EQ: a headphone measurement
//! and a target, read from AutoEQ, Squiglink, and similar text exports.

use serde::{Deserialize, Serialize};

/// Which curve a file supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CurveKind {
    Measurement,
    Target,
}

impl CurveKind {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Measurement => "measurement.txt",
            Self::Target => "target.txt",
        }
    }
}

/// One point of a curve. A plain curve has `low == high`; a target range
/// ("preference bounds") spans `low..=high`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub frequency: f64,
    pub low: f64,
    pub high: f64,
}

/// The curves saved with a profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Curves {
    pub measurement: Option<Vec<CurvePoint>>,
    pub target: Option<Vec<CurvePoint>>,
}

/// Parse a curve: one point per line, frequency first. Separators may be
/// commas, semicolons, tabs, or spaces; header and comment lines are skipped.
/// A measurement uses the first value after the frequency (AutoEQ's `raw`).
/// A target with exactly three numbers per line is a range between the
/// second and third; otherwise it uses the second.
pub fn parse_curve(text: &str, kind: CurveKind) -> Result<Vec<CurvePoint>, String> {
    let mut points = text
        .lines()
        .filter_map(|line| {
            let numbers = line
                .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .filter(|token| !token.is_empty())
                .map(|token| token.parse::<f64>().ok().filter(|value| value.is_finite()))
                .collect::<Option<Vec<_>>>()?;
            let frequency = *numbers.first()?;
            let value = *numbers.get(1)?;
            if !(1.0..=100_000.0).contains(&frequency) {
                return None;
            }
            let (low, high) = match (kind, numbers.len()) {
                (CurveKind::Target, 3) => (value.min(numbers[2]), value.max(numbers[2])),
                _ => (value, value),
            };
            Some(CurvePoint {
                frequency,
                low,
                high,
            })
        })
        .collect::<Vec<_>>();
    points.sort_by(|a, b| a.frequency.total_cmp(&b.frequency));
    points.dedup_by(|a, b| a.frequency == b.frequency);
    if points.len() < 10 {
        return Err(format!(
            "Expected at least 10 lines of \"frequency, dB\"; found {}",
            points.len()
        ));
    }
    let span = points[points.len() - 1].frequency / points[0].frequency;
    if span < 10.0 {
        return Err("The curve must cover at least a decade of frequency".into());
    }
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_autoeq_csv_and_squiglink_text() {
        let csv = "frequency,raw,error,smoothed\n".to_owned()
            + &(0..20)
                .map(|i| format!("{},{}.5,0.1,0.2\n", 20 * (i + 1) * (i + 1), i))
                .collect::<String>();
        let points = parse_curve(&csv, CurveKind::Measurement).unwrap();
        assert_eq!(points.len(), 20);
        assert_eq!(points[0].frequency, 20.0);
        assert_eq!((points[0].low, points[0].high), (0.5, 0.5));

        let squig = "* Squiglink export\n".to_owned()
            + &(0..20)
                .map(|i| format!("{}\t{}\n", 1000 * (i + 1), -(i as f64)))
                .collect::<String>();
        let points = parse_curve(&squig, CurveKind::Target).unwrap();
        assert_eq!(points[19].low, -19.0);
    }

    #[test]
    fn three_column_targets_are_ranges() {
        let text = (0..20)
            .map(|i| format!("{} 2 -1\n", 100 * (i + 1)))
            .collect::<String>();
        let target = parse_curve(&text, CurveKind::Target).unwrap();
        assert_eq!((target[0].low, target[0].high), (-1.0, 2.0));
        let measurement = parse_curve(&text, CurveKind::Measurement).unwrap();
        assert_eq!((measurement[0].low, measurement[0].high), (2.0, 2.0));
    }

    #[test]
    fn rejects_files_that_are_not_curves() {
        assert!(parse_curve("hello\nworld", CurveKind::Measurement).is_err());
        let narrow = (0..20)
            .map(|i| format!("{} 0\n", 1000 + i))
            .collect::<String>();
        assert!(parse_curve(&narrow, CurveKind::Measurement).is_err());
    }
}
