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
    /// More measurements of the same headphone after putting it on again.
    /// With the measurement they show how much its response moves between
    /// placements; they share the measurement's provenance.
    #[serde(default)]
    pub placements: Vec<Vec<CurvePoint>>,
}

/// Placements kept beside the measurement, so 16 measurements in all.
pub const MAX_PLACEMENTS: usize = 15;

/// Measurement fixtures the editor names, as (key, label). Preference bounds
/// and targets are specific to a fixture's ears; these let a mismatch be
/// recognised however the fixture was typed.
pub const FIXTURES: [(&str, &str); 4] = [
    ("bk5128", "B&K 5128"),
    ("gras_kb50", "GRAS 43AG / 45CA (KB50xx pinna)"),
    ("bk4128", "B&K 4128"),
    ("iec711", "IEC 60318-4 (711) coupler"),
];

/// The known fixture a free-text fixture name refers to, if any.
pub fn fixture_key(name: &str) -> Option<&'static str> {
    let name = name
        .to_ascii_lowercase()
        .replace([' ', '-', '_', '&', '/', '.'], "");
    if name.contains("5128") {
        Some("bk5128")
    } else if name.contains("4128") {
        Some("bk4128")
    } else if ["43ag", "45ca", "kb50", "kb5"]
        .iter()
        .any(|alias| name.contains(alias))
    {
        Some("gras_kb50")
    } else if name.contains("711") || name.contains("603184") {
        Some("iec711")
    } else {
        None
    }
}

/// Whether two declared fixture names describe the same fixture.
fn same_fixture(a: &str, b: &str) -> bool {
    match (fixture_key(a), fixture_key(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
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
    let fixtures_differ = !measurement.fixture.is_empty()
        && !target.fixture.is_empty()
        && !same_fixture(&measurement.fixture, &target.fixture);
    if fixtures_differ {
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
            "The declared contexts differ in {}. Comparing these curves may be misleading.{}",
            mismatches.join(", "),
            if fixtures_differ {
                " Targets and preference bounds are made for one fixture's ears; headphones \
                 behave differently on another, so they do not transfer."
            } else {
                ""
            }
        );
    }
    let mut matched = Vec::new();
    if !measurement.fixture.is_empty() && same_fixture(&measurement.fixture, &target.fixture) {
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

/// A curve's value at `frequency`, interpolated in log frequency, as
/// (low, high). Outside the curve the nearest end is held.
pub fn curve_at(points: &[CurvePoint], frequency: f64) -> (f64, f64) {
    let Some(first) = points.first() else {
        return (0.0, 0.0);
    };
    if frequency <= first.frequency {
        return (first.low, first.high);
    }
    let index = points.partition_point(|point| point.frequency < frequency);
    match (points.get(index.wrapping_sub(1)), points.get(index)) {
        (Some(a), Some(b)) => {
            let span = (b.frequency / a.frequency).ln();
            let amount = if span > 0.0 {
                (frequency / a.frequency).ln() / span
            } else {
                0.0
            };
            let lerp = |x: f64, y: f64| x + amount * (y - x);
            (lerp(a.low, b.low), lerp(a.high, b.high))
        }
        _ => points
            .last()
            .map_or((0.0, 0.0), |point| (point.low, point.high)),
    }
}

/// The level curves are aligned to: the average over 630 Hz ± one octave,
/// as GraphWorks normalises, so imported bounds line up the same way.
pub fn alignment_level(value: &dyn Fn(f64) -> f64) -> f64 {
    const STEPS: usize = 25;
    (0..STEPS)
        .map(|index| value(315.0 * 4.0_f64.powf(index as f64 / (STEPS - 1) as f64)))
        .sum::<f64>()
        / STEPS as f64
}

/// Whether a target is a range (preference bounds) rather than a line.
pub fn is_range(target: &[CurvePoint]) -> bool {
    target.iter().any(|point| point.high > point.low + 1e-9)
}

/// A stretch where a response leaves a target range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Excursion {
    pub start: f64,
    pub end: f64,
    /// Furthest distance outside the range: positive above, negative below.
    pub worst_db: f64,
}

/// Where `response` leaves the `target` range, both aligned by
/// [`alignment_level`] (the range by its centre). Brushes narrower than a
/// twelfth of an octave or under half a decibel are ignored.
pub fn excursions(response: &dyn Fn(f64) -> f64, target: &[CurvePoint]) -> Vec<Excursion> {
    const POINTS: usize = 481;
    if !is_range(target) {
        return Vec::new();
    }
    let center = |frequency: f64| {
        let (low, high) = curve_at(target, frequency);
        (low + high) / 2.0
    };
    let response_offset = alignment_level(response);
    let target_offset = alignment_level(&center);
    let mut found: Vec<Excursion> = Vec::new();
    let mut open: Option<Excursion> = None;
    for index in 0..POINTS {
        let frequency = 20.0 * 1000.0_f64.powf(index as f64 / (POINTS - 1) as f64);
        let value = response(frequency) - response_offset;
        let (low, high) = curve_at(target, frequency);
        let (low, high) = (low - target_offset, high - target_offset);
        let outside = if value > high {
            value - high
        } else if value < low {
            value - low
        } else {
            0.0
        };
        match open.as_mut() {
            Some(current) if outside != 0.0 && outside.signum() == current.worst_db.signum() => {
                current.end = frequency;
                if outside.abs() > current.worst_db.abs() {
                    current.worst_db = outside;
                }
            }
            _ => {
                found.extend(open.take());
                if outside != 0.0 {
                    open = Some(Excursion {
                        start: frequency,
                        end: frequency,
                        worst_db: outside,
                    });
                }
            }
        }
    }
    found.extend(open);
    found.retain(|excursion| {
        excursion.worst_db.abs() >= 0.5 && (excursion.end / excursion.start).log2() >= 1.0 / 12.0
    });
    found
}

/// Several measurements of one headphone summarised per frequency: the
/// median, and the bands holding the middle half and 90% of placements.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasurementField {
    pub median: Vec<CurvePoint>,
    pub inner: Vec<CurvePoint>,
    pub outer: Vec<CurvePoint>,
    pub count: usize,
}

/// The field of a measurement and its placements, each aligned to the
/// first's level at 630 Hz ± one octave. `None` with fewer than two.
pub fn measurement_field(takes: &[&[CurvePoint]]) -> Option<MeasurementField> {
    const POINTS: usize = 481;
    if takes.len() < 2 {
        return None;
    }
    let level = |take: &[CurvePoint]| alignment_level(&|frequency| curve_at(take, frequency).0);
    let reference = level(takes[0]);
    let offsets = takes
        .iter()
        .map(|take| level(take) - reference)
        .collect::<Vec<_>>();
    let percentile = |sorted: &[f64], fraction: f64| {
        let position = fraction * (sorted.len() - 1) as f64;
        let below = position.floor() as usize;
        let above = position.ceil() as usize;
        sorted[below] + (position - below as f64) * (sorted[above] - sorted[below])
    };
    let mut field = MeasurementField {
        median: Vec::with_capacity(POINTS),
        inner: Vec::with_capacity(POINTS),
        outer: Vec::with_capacity(POINTS),
        count: takes.len(),
    };
    for index in 0..POINTS {
        let frequency = 20.0 * 1000.0_f64.powf(index as f64 / (POINTS - 1) as f64);
        let mut values = takes
            .iter()
            .zip(&offsets)
            .map(|(take, offset)| curve_at(take, frequency).0 - offset)
            .collect::<Vec<_>>();
        values.sort_by(f64::total_cmp);
        let point = |low: f64, high: f64| CurvePoint {
            frequency,
            low,
            high,
        };
        let median = percentile(&values, 0.5);
        field.median.push(point(median, median));
        field
            .inner
            .push(point(percentile(&values, 0.25), percentile(&values, 0.75)));
        field
            .outer
            .push(point(percentile(&values, 0.05), percentile(&values, 0.95)));
    }
    Some(field)
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

    fn flat(level: f64) -> Vec<CurvePoint> {
        (0..40)
            .map(|index| {
                let frequency = 20.0 * 1000.0_f64.powf(index as f64 / 39.0);
                CurvePoint {
                    frequency,
                    low: level,
                    high: level,
                }
            })
            .collect()
    }

    #[test]
    fn fixtures_are_recognised_however_they_are_typed() {
        assert_eq!(fixture_key("B&K 5128"), Some("bk5128"));
        assert_eq!(fixture_key("bk-5128 HATS"), Some("bk5128"));
        assert_eq!(fixture_key("GRAS 45CA-10 with KB5000"), Some("gras_kb50"));
        assert_eq!(fixture_key("43AG-7"), Some("gras_kb50"));
        assert_eq!(fixture_key("IEC 60318-4"), Some("iec711"));
        assert_eq!(fixture_key("Super Review 711 clone"), Some("iec711"));
        assert_eq!(fixture_key("my ears"), None);
        let mut measurement = declared_metadata();
        measurement.fixture = "5128".into();
        let mut target = measurement.clone();
        target.fixture = "B&K 5128".into();
        assert!(context_relationship(&measurement, &target).contains("matching"));
        target.fixture = "GRAS 43AG".into();
        let note = context_relationship(&measurement, &target);
        assert!(
            note.contains("differ") && note.contains("do not transfer"),
            "{note}"
        );
    }

    #[test]
    fn excursions_report_where_a_response_leaves_the_bounds() {
        let bounds = (0..40)
            .map(|index| {
                let frequency = 20.0 * 1000.0_f64.powf(index as f64 / 39.0);
                CurvePoint {
                    frequency,
                    low: 10.0,
                    high: 12.0,
                }
            })
            .collect::<Vec<_>>();
        // A 3 dB bump an octave wide at 4 kHz, otherwise centred.
        let response = |frequency: f64| {
            let octaves = (frequency / 4000.0).log2();
            if octaves.abs() < 0.5 { 3.0 } else { 0.0 }
        };
        let found = excursions(&response, &bounds);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0].worst_db - 2.0).abs() < 1e-9);
        assert!(found[0].start > 2700.0 && found[0].end < 5800.0);
        // Alignment ignores absolute level; a line target has no bounds.
        assert!(excursions(&|_| 40.0, &bounds).is_empty());
        assert!(excursions(&response, &flat(0.0)).is_empty());
    }

    #[test]
    fn placements_form_a_field_around_their_median() {
        let takes = [flat(0.0), flat(0.0), flat(3.0)];
        let mut varied = takes.to_vec();
        // The third placement loses 6 dB of treble.
        for point in &mut varied[2] {
            if point.frequency > 5000.0 {
                point.low -= 6.0;
                point.high -= 6.0;
            }
        }
        let refs = varied.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let field = measurement_field(&refs).unwrap();
        assert_eq!(field.count, 3);
        let at = |points: &[CurvePoint], frequency| curve_at(points, frequency);
        // Levels are aligned, so the 3 dB offset vanishes below 5 kHz.
        assert!(at(&field.median, 1000.0).0.abs() < 1e-9);
        assert!(at(&field.outer, 1000.0).0.abs() < 1e-9);
        let (low, high) = at(&field.outer, 10_000.0);
        assert!(low < -5.0 && high.abs() < 1e-9, "{low} {high}");
        assert!(measurement_field(&refs[..1]).is_none());
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
            placements: Vec::new(),
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
            placements: Vec::new(),
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
            placements: Vec::new(),
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
