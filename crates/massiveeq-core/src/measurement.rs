//! Retained measurement takes and reversible, per-ear dB averaging.
use crate::{CurvePoint, alignment_level, curve_at};
use serde::{Deserialize, Serialize};

pub const MAX_MEASUREMENT_TAKES: usize = 32;
pub const MAX_MEASUREMENT_POINTS: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementEar {
    Left,
    Right,
    Both,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasurementTake {
    pub name: String,
    pub ear: MeasurementEar,
    pub points: Vec<CurvePoint>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementOutput {
    #[default]
    Separate,
    Merged,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasurementSet {
    pub takes: Vec<MeasurementTake>,
    pub output: MeasurementOutput,
    pub align_takes: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasurementChannels {
    pub left: Vec<CurvePoint>,
    pub right: Vec<CurvePoint>,
}

impl MeasurementChannels {
    /// Ear-balanced mean, independent of how many takes each ear has.
    pub fn merged(&self) -> Vec<CurvePoint> {
        self.left
            .iter()
            .zip(&self.right)
            .map(|(left, right)| {
                let value = (left.low + right.low) / 2.0;
                CurvePoint {
                    frequency: left.frequency,
                    low: value,
                    high: value,
                }
            })
            .collect()
    }
}

impl MeasurementSet {
    pub fn averages(&self) -> Result<MeasurementChannels, String> {
        if self.takes.is_empty() || self.takes.len() > MAX_MEASUREMENT_TAKES {
            return Err(format!(
                "Add between 1 and {MAX_MEASUREMENT_TAKES} measurement files."
            ));
        }
        if self
            .takes
            .iter()
            .map(|take| take.points.len())
            .sum::<usize>()
            > MAX_MEASUREMENT_POINTS
        {
            return Err(format!(
                "The files exceed {MAX_MEASUREMENT_POINTS} points in total. Export fewer points or use fewer takes."
            ));
        }
        for take in &self.takes {
            if take.name.trim().is_empty() || take.name.len() > 256 {
                return Err("Each measurement needs a name of at most 256 bytes.".into());
            }
            if take.points.len() < 10
                || take.points.iter().any(|point| {
                    !point.frequency.is_finite()
                        || !(1.0..=100_000.0).contains(&point.frequency)
                        || !point.low.is_finite()
                        || point.low.abs() > 300.0
                        || point.high != point.low
                })
                || take
                    .points
                    .windows(2)
                    .any(|pair| pair[0].frequency >= pair[1].frequency)
            {
                return Err(format!(
                    "{}: expected at least 10 ordered frequency/dB points, with finite levels within ±300 dB.",
                    take.name
                ));
            }
        }
        let start = self
            .takes
            .iter()
            .map(|take| take.points[0].frequency)
            .fold(1.0, f64::max);
        let end = self
            .takes
            .iter()
            .map(|take| take.points.last().unwrap().frequency)
            .fold(100_000.0, f64::min);
        if end / start < 10.0 {
            return Err("The measurements must share at least one decade of frequency. No values are extrapolated.".into());
        }
        let average = |ear: MeasurementEar| -> Result<Vec<CurvePoint>, String> {
            let takes = self
                .takes
                .iter()
                .filter(|take| take.ear == ear || take.ear == MeasurementEar::Both)
                .collect::<Vec<_>>();
            let Some(first) = takes.first() else {
                return Err(format!(
                    "Add or assign at least one {ear:?} measurement. Both / mono files count for both ears."
                ));
            };
            if self.align_takes && (start > 315.0 || end < 1260.0) {
                return Err("Level matching needs every take to cover 315–1260 Hz.".into());
            }
            let level = |take: &MeasurementTake| {
                alignment_level(&|frequency| curve_at(&take.points, frequency).0)
            };
            let reference = if self.align_takes { level(first) } else { 0.0 };
            let offsets = takes
                .iter()
                .map(|take| {
                    if self.align_takes {
                        level(take) - reference
                    } else {
                        0.0
                    }
                })
                .collect::<Vec<_>>();
            Ok((0..481)
                .map(|index| {
                    let frequency = start * (end / start).powf(index as f64 / 480.0);
                    let value = takes
                        .iter()
                        .zip(&offsets)
                        .map(|(take, offset)| curve_at(&take.points, frequency).0 - offset)
                        .sum::<f64>()
                        / takes.len() as f64;
                    CurvePoint {
                        frequency,
                        low: value,
                        high: value,
                    }
                })
                .collect())
        };
        Ok(MeasurementChannels {
            left: average(MeasurementEar::Left)?,
            right: average(MeasurementEar::Right)?,
        })
    }
}

/// Conservative filename hints. Unknown or conflicting names need a choice.
pub fn measurement_ear_hint(name: &str) -> Option<MeasurementEar> {
    let stem = name
        .rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_ascii_lowercase();
    let words = stem
        .split(|c: char| !c.is_ascii_alphanumeric())
        .collect::<Vec<_>>();
    let left = words.iter().any(|word| matches!(*word, "left" | "l"));
    let right = words.iter().any(|word| matches!(*word, "right" | "r"));
    match (left, right) {
        (true, false) => Some(MeasurementEar::Left),
        (false, true) => Some(MeasurementEar::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn take(ear: MeasurementEar, level: f64, start: f64, end: f64) -> MeasurementTake {
        MeasurementTake {
            name: format!("{ear:?}"),
            ear,
            points: (0..21)
                .map(|i| {
                    let frequency = start * (end / start).powf(i as f64 / 20.0);
                    CurvePoint {
                        frequency,
                        low: level,
                        high: level,
                    }
                })
                .collect(),
        }
    }
    #[test]
    fn different_grids_average_per_ear_and_merge_ears_with_equal_weight() {
        let set = MeasurementSet {
            takes: vec![
                take(MeasurementEar::Left, 2.0, 20.0, 20000.0),
                take(MeasurementEar::Left, 6.0, 30.0, 18000.0),
                take(MeasurementEar::Right, 10.0, 25.0, 24000.0),
            ],
            ..Default::default()
        };
        let average = set.averages().unwrap();
        assert!(average.left.iter().all(|p| (p.low - 4.0).abs() < 1e-9));
        assert!(average.right.iter().all(|p| (p.low - 10.0).abs() < 1e-9));
        assert!(average.merged().iter().all(|p| (p.low - 7.0).abs() < 1e-9));
        assert_eq!(average.left[0].frequency, 30.0);
        assert!((average.left.last().unwrap().frequency - 18000.0).abs() < 1e-6);
        let restored: MeasurementSet =
            serde_json::from_str(&serde_json::to_string(&set).unwrap()).unwrap();
        assert_eq!(restored, set);
    }
    #[test]
    fn optional_alignment_keeps_each_ears_reference_level() {
        let mut set = MeasurementSet {
            takes: vec![
                take(MeasurementEar::Left, 70.0, 20.0, 20000.0),
                take(MeasurementEar::Left, 90.0, 20.0, 20000.0),
                take(MeasurementEar::Right, 73.0, 20.0, 20000.0),
            ],
            ..Default::default()
        };
        assert!((set.averages().unwrap().left[10].low - 80.0).abs() < 1e-9);
        set.align_takes = true;
        let average = set.averages().unwrap();
        assert!((average.left[10].low - 70.0).abs() < 1e-9);
        assert!((average.right[10].low - 73.0).abs() < 1e-9);
    }
    #[test]
    fn invalid_or_incomplete_sets_are_rejected() {
        let mut set = MeasurementSet {
            takes: vec![take(MeasurementEar::Left, 0.0, 20.0, 20000.0)],
            ..Default::default()
        };
        assert!(set.averages().unwrap_err().contains("Right"));
        set.takes[0].ear = MeasurementEar::Both;
        assert!(set.averages().is_ok());
        set.takes
            .push(take(MeasurementEar::Right, 0.0, 4000.0, 20000.0));
        assert!(set.averages().unwrap_err().contains("decade"));
        set.takes.pop();
        set.takes[0].points[0].low = f64::NAN;
        assert!(set.averages().is_err());
    }
    #[test]
    fn filename_hints_require_distinct_ear_tokens() {
        assert_eq!(
            measurement_ear_hint("HD480_L_02.txt"),
            Some(MeasurementEar::Left)
        );
        assert_eq!(
            measurement_ear_hint("right take.csv"),
            Some(MeasurementEar::Right)
        );
        assert_eq!(measurement_ear_hint("left-right.txt"), None);
        assert_eq!(measurement_ear_hint("clear.csv"), None);
    }

    #[test]
    fn a_log_frequency_slope_survives_resampling_different_grids() {
        let mut left = take(MeasurementEar::Left, 0.0, 20.0, 20000.0);
        let mut right = take(MeasurementEar::Right, 0.0, 30.0, 18000.0);
        for source in [&mut left, &mut right] {
            for point in &mut source.points {
                point.low = (point.frequency / 1000.0).log2() * 2.0;
                point.high = point.low;
            }
        }
        let set = MeasurementSet {
            takes: vec![left, right],
            ..Default::default()
        };
        for point in set.averages().unwrap().merged() {
            assert!((point.low - (point.frequency / 1000.0).log2() * 2.0).abs() < 1e-9);
        }
    }
}
