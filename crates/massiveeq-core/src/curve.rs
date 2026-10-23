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

    /// Sidecar JSON holding the curve's user-declared provenance.
    pub fn metadata_file_name(self) -> &'static str {
        match self {
            Self::Measurement => "measurement.json",
            Self::Target => "target.json",
        }
    }
}

/// How the measurement behind a curve was compensated, as declared by the
/// user. `Unknown` means no claim either way.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveCompensation {
    #[default]
    Unknown,
    Raw,
    DiffuseField,
    Other,
}

impl CurveCompensation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Raw => "raw",
            Self::DiffuseField => "diffuse-field",
            Self::Other => "other",
        }
    }
}

/// The kind of transducer a curve describes, as declared by the user.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveTransducer {
    #[default]
    Unknown,
    OverEar,
    InEar,
    Speaker,
}

impl CurveTransducer {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::OverEar => "over-ear",
            Self::InEar => "in-ear",
            Self::Speaker => "speaker",
        }
    }
}

/// User-declared provenance for an imported measurement or target curve:
/// where it came from and the context it was produced in. Empty strings and
/// `Unknown` variants mean "not declared".
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurveMetadata {
    pub source: String,
    pub fixture: String,
    pub compensation: CurveCompensation,
    pub baseline: String,
    pub transducer: CurveTransducer,
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
    #[serde(default)]
    pub measurement_metadata: Option<CurveMetadata>,
    #[serde(default)]
    pub target_metadata: Option<CurveMetadata>,
}

impl Curves {
    /// A short, honest explanation of how the measurement and target relate,
    /// based only on user-declared metadata. Declared matches never guarantee
    /// the curves were actually measured compatibly.
    pub fn context_note(&self) -> String {
        if self.measurement.is_none() && self.target.is_none() {
            return "No measurement or target curve is stored for this profile.".into();
        }
        let mut notes = Vec::new();
        if self.measurement.is_some() && self.measurement_metadata.is_none() {
            notes.push("The measurement's provenance is unknown.".to_owned());
        }
        if self.target.is_some() && self.target_metadata.is_none() {
            notes.push("The target's provenance is unknown.".to_owned());
        }
        if self.measurement.is_some() && self.target.is_some() {
            match (&self.measurement_metadata, &self.target_metadata) {
                (Some(measurement), Some(target)) => {
                    notes.push(context_relationship(measurement, target));
                }
                _ => notes.push(
                    "Without provenance for both curves their compatibility cannot be assessed."
                        .to_owned(),
                ),
            }
            notes.push("The corrected response is a prediction, not a new measurement.".to_owned());
        }
        if self.target.is_some() {
            notes.push("Target ranges are a reference, not a pass/fail test.".to_owned());
        }
        notes.join(" ")
    }
}

/// Compare the declared contexts of a measurement and its target.
fn context_relationship(measurement: &CurveMetadata, target: &CurveMetadata) -> String {
    let mut mismatches = Vec::new();
    if !measurement.fixture.is_empty()
        && !target.fixture.is_empty()
        && measurement.fixture != target.fixture
    {
        mismatches.push(format!(
            "fixture (\"{}\" vs \"{}\")",
            measurement.fixture, target.fixture
        ));
    }
    if measurement.compensation != CurveCompensation::Unknown
        && target.compensation != CurveCompensation::Unknown
        && measurement.compensation != target.compensation
    {
        mismatches.push(format!(
            "compensation ({} vs {})",
            measurement.compensation.label(),
            target.compensation.label()
        ));
    }
    if !measurement.baseline.is_empty()
        && !target.baseline.is_empty()
        && measurement.baseline != target.baseline
    {
        mismatches.push(format!(
            "baseline (\"{}\" vs \"{}\")",
            measurement.baseline, target.baseline
        ));
    }
    if measurement.transducer != CurveTransducer::Unknown
        && target.transducer != CurveTransducer::Unknown
        && measurement.transducer != target.transducer
    {
        mismatches.push(format!(
            "transducer ({} vs {})",
            measurement.transducer.label(),
            target.transducer.label()
        ));
    }
    if !mismatches.is_empty() {
        return format!(
            "The declared contexts differ in {}. Comparing these curves may be misleading.",
            mismatches.join(", ")
        );
    }
    let mut matched = Vec::new();
    if !measurement.fixture.is_empty() && measurement.fixture == target.fixture {
        matched.push(format!("fixture \"{}\"", measurement.fixture));
    }
    if measurement.compensation != CurveCompensation::Unknown
        && measurement.compensation == target.compensation
    {
        matched.push(format!("{} compensation", measurement.compensation.label()));
    }
    if !measurement.baseline.is_empty() && measurement.baseline == target.baseline {
        matched.push(format!("baseline \"{}\"", measurement.baseline));
    }
    if measurement.transducer != CurveTransducer::Unknown
        && measurement.transducer == target.transducer
    {
        matched.push(format!("{} transducer", measurement.transducer.label()));
    }
    if matched.is_empty() {
        return "Both curves have metadata but declare no concrete context, so their compatibility \
                cannot be assessed."
            .into();
    }
    format!(
        "Both curves declare a matching user-entered context ({}). Declared matches do not \
         guarantee the curves were actually measured compatibly.",
        matched.join(", ")
    )
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
    fn an_unknown_target_fixture_is_not_reported_as_a_matching_fixture() {
        let measurement = declared_metadata();
        let mut target = measurement.clone();
        target.fixture.clear();
        let note = context_relationship(&measurement, &target);
        assert!(
            !note.contains(&format!("fixture \"{}\"", measurement.fixture)),
            "{note}"
        );
    }

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

    #[test]
    fn metadata_enums_default_to_unknown_and_serialize_as_snake_case() {
        assert_eq!(CurveCompensation::default(), CurveCompensation::Unknown);
        assert_eq!(CurveTransducer::default(), CurveTransducer::Unknown);
        assert_eq!(
            serde_json::to_value(CurveCompensation::DiffuseField).unwrap(),
            "diffuse_field"
        );
        assert_eq!(
            serde_json::to_value(CurveTransducer::OverEar).unwrap(),
            "over_ear"
        );
        let metadata = CurveMetadata::default();
        assert!(metadata.source.is_empty() && metadata.fixture.is_empty());
        assert!(metadata.baseline.is_empty());
    }

    #[test]
    fn curves_json_without_metadata_still_deserializes() {
        let curves: Curves = serde_json::from_str(r#"{"measurement":null,"target":null}"#).unwrap();
        assert!(curves.measurement_metadata.is_none());
        assert!(curves.target_metadata.is_none());
    }

    fn declared_metadata() -> CurveMetadata {
        CurveMetadata {
            source: "AutoEq export".into(),
            fixture: "B&K 5128".into(),
            compensation: CurveCompensation::DiffuseField,
            baseline: "Harman 2018".into(),
            transducer: CurveTransducer::OverEar,
        }
    }

    #[test]
    fn context_note_reports_missing_metadata() {
        let mut curves = Curves {
            measurement: Some(vec![CurvePoint {
                frequency: 100.0,
                low: 0.0,
                high: 0.0,
            }]),
            target: None,
            measurement_metadata: None,
            target_metadata: None,
        };
        assert!(curves.context_note().contains("provenance is unknown"));
        curves.target = curves.measurement.clone();
        let note = curves.context_note();
        assert!(note.contains("compatibility cannot be assessed"));
        assert!(note.contains("a prediction"));
        assert!(note.contains("reference, not a pass/fail"));
    }

    #[test]
    fn context_note_flags_declared_mismatches() {
        let mut target = declared_metadata();
        target.fixture = "GRAS 45CA".into();
        target.transducer = CurveTransducer::InEar;
        target.compensation = CurveCompensation::Raw;
        let curves = Curves {
            measurement: Some(vec![]),
            target: Some(vec![]),
            measurement_metadata: Some(declared_metadata()),
            target_metadata: Some(target),
        };
        let note = curves.context_note();
        assert!(note.contains("declared contexts differ"), "{note}");
        assert!(
            note.contains("B&K 5128") && note.contains("GRAS 45CA"),
            "{note}"
        );
        assert!(
            note.contains("over-ear") && note.contains("in-ear"),
            "{note}"
        );
        assert!(
            note.contains("diffuse-field") && note.contains("raw"),
            "{note}"
        );
        assert!(note.contains("may be misleading"), "{note}");
        assert!(!note.contains("guarantee the curves were actually measured compatibly"));
    }

    #[test]
    fn context_note_describes_user_declared_matches_without_guaranteeing_compatibility() {
        let curves = Curves {
            measurement: Some(vec![]),
            target: Some(vec![]),
            measurement_metadata: Some(declared_metadata()),
            target_metadata: Some(declared_metadata()),
        };
        let note = curves.context_note();
        assert!(note.contains("matching user-entered context"), "{note}");
        assert!(note.contains("do not guarantee"), "{note}");
        assert!(
            note.contains("a prediction, not a new measurement"),
            "{note}"
        );

        let unknown = Curves {
            measurement_metadata: Some(CurveMetadata::default()),
            target_metadata: Some(CurveMetadata::default()),
            ..curves.clone()
        };
        assert!(
            unknown
                .context_note()
                .contains("declare no concrete context")
        );
    }
}
