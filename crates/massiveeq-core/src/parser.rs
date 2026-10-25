use crate::model::*;
use regex::Regex;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("include nesting exceeds {MAX_INCLUDE_DEPTH} levels")]
    IncludeDepth,
    #[error("include cycle detected at {0}")]
    IncludeCycle(PathBuf),
}

#[derive(Debug)]
struct ParserState {
    channels: ChannelSelection,
    visited: HashSet<PathBuf>,
    /// Slope from a `# MassiveEQ tilt:` line and the index of the first
    /// filter after it, where the generated shelves should follow.
    tilt: Option<(f64, usize)>,
}

/// Comment that marks the generated tilt shelves in a saved profile. Other
/// APO hosts ignore it and simply run the shelves as ordinary filters.
const TILT_MARKER: &str = "# MassiveEQ tilt:";

pub fn parse_file(path: impl AsRef<Path>) -> Result<ProfileDocument, ParseError> {
    let path = path.as_ref();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported profile");
    let mut profile = ProfileDocument::empty(name);
    let mut state = ParserState {
        channels: ChannelSelection::All,
        visited: HashSet::new(),
        tilt: None,
    };
    parse_path_into(path, 0, &mut state, &mut profile)?;
    extract_tilt(&mut profile, state.tilt);
    enforce_limits(&mut profile);
    Ok(profile)
}

pub fn parse_text(name: impl Into<String>, text: &str) -> ProfileDocument {
    let mut profile = ProfileDocument::empty(name);
    let mut state = ParserState {
        channels: ChannelSelection::All,
        visited: HashSet::new(),
        tilt: None,
    };
    parse_lines(text, Path::new("profile.txt"), 0, &mut state, &mut profile);
    extract_tilt(&mut profile, state.tilt);
    enforce_limits(&mut profile);
    profile
}

fn parse_path_into(
    path: &Path,
    depth: usize,
    state: &mut ParserState,
    profile: &mut ProfileDocument,
) -> Result<(), ParseError> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(ParseError::IncludeDepth);
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !state.visited.insert(canonical.clone()) {
        return Err(ParseError::IncludeCycle(canonical));
    }
    let text = fs::read_to_string(path).map_err(|source| ParseError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_lines(&text, path, depth, state, profile);
    state.visited.remove(&canonical);
    Ok(())
}

fn parse_lines(
    text: &str,
    source_path: &Path,
    depth: usize,
    state: &mut ParserState,
    profile: &mut ProfileDocument,
) {
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();
        if let Some(slope) = line
            .strip_prefix(TILT_MARKER)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && value.abs() <= MAX_TILT_DB_PER_OCTAVE)
        {
            state.tilt = Some((slope, profile.filters.len()));
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((command, args)) = line.split_once(':') else {
            diagnostic(
                profile,
                DiagnosticLevel::Warning,
                source_path,
                line_no,
                "Ignored text without an APO command separator",
            );
            continue;
        };
        let command_lower = command.trim().to_ascii_lowercase();
        let args = args.trim();
        match command_lower.as_str() {
            "channel" => match parse_channels(args) {
                Some(channels) => state.channels = channels,
                None => diagnostic(
                    profile,
                    DiagnosticLevel::Error,
                    source_path,
                    line_no,
                    "Only ALL, L, R, 1, and 2 channels are supported in version 1",
                ),
            },
            "preamp" => match first_number(args)
                .filter(|value| value.is_finite() && (-120.0..=60.0).contains(value))
            {
                Some(value) => profile.preamps.push(ChannelGain {
                    channels: state.channels,
                    gain_db: value,
                }),
                None => diagnostic(
                    profile,
                    DiagnosticLevel::Error,
                    source_path,
                    line_no,
                    "Invalid Preamp value",
                ),
            },
            value if value == "filter" || value.starts_with("filter ") => {
                match parse_filter(args, state.channels) {
                    Ok(filter) => profile.filters.push(filter),
                    Err(message) => diagnostic(
                        profile,
                        DiagnosticLevel::Error,
                        source_path,
                        line_no,
                        &message,
                    ),
                }
            }
            "graphiceq" => match parse_graphic_eq(args, state.channels) {
                Ok(graphic) => profile.graphic_eqs.push(graphic),
                Err(message) => diagnostic(
                    profile,
                    DiagnosticLevel::Error,
                    source_path,
                    line_no,
                    &message,
                ),
            },
            "convolution" => {
                if unquote(args).trim().is_empty() {
                    diagnostic(
                        profile,
                        DiagnosticLevel::Error,
                        source_path,
                        line_no,
                        "Convolution is missing an impulse-response path",
                    );
                } else {
                    let path = source_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(unquote(args));
                    profile.convolutions.push(Convolution {
                        channels: state.channels,
                        path,
                    });
                }
            }
            "include" => {
                let path = source_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(unquote(args));
                if let Err(error) = parse_path_into(&path, depth + 1, state, profile) {
                    diagnostic(
                        profile,
                        DiagnosticLevel::Error,
                        source_path,
                        line_no,
                        &error.to_string(),
                    );
                }
            }
            "copy" | "delay" | "stage" | "device" | "if" | "elseif" | "else" | "endif" => {
                diagnostic(
                    profile,
                    DiagnosticLevel::Error,
                    source_path,
                    line_no,
                    &format!("Unsupported audio-affecting command: {command}"),
                );
            }
            _ => diagnostic(
                profile,
                DiagnosticLevel::Error,
                source_path,
                line_no,
                &format!("Unsupported command: {command}"),
            ),
        }
    }
}

fn parse_channels(args: &str) -> Option<ChannelSelection> {
    let tokens: Vec<_> = args
        .split_whitespace()
        .map(|v| v.to_ascii_uppercase())
        .collect();
    if tokens.len() == 1 {
        match tokens[0].as_str() {
            "ALL" => Some(ChannelSelection::All),
            "L" | "1" => Some(ChannelSelection::Left),
            "R" | "2" => Some(ChannelSelection::Right),
            _ => None,
        }
    } else if tokens
        .iter()
        .all(|v| matches!(v.as_str(), "L" | "R" | "1" | "2"))
    {
        let left = tokens.iter().any(|v| matches!(v.as_str(), "L" | "1"));
        let right = tokens.iter().any(|v| matches!(v.as_str(), "R" | "2"));
        match (left, right) {
            (true, true) => Some(ChannelSelection::All),
            (true, false) => Some(ChannelSelection::Left),
            (false, true) => Some(ChannelSelection::Right),
            _ => None,
        }
    } else {
        None
    }
}

fn parse_filter(args: &str, channels: ChannelSelection) -> Result<Filter, String> {
    let normalized = args.replace(',', ".");
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    if tokens.len() < 2 {
        return Err("Incomplete Filter command".into());
    }
    let enabled = match tokens[0].to_ascii_uppercase().as_str() {
        "ON" => true,
        "OFF" => false,
        _ => return Err("Filter must begin with ON or OFF".into()),
    };
    if tokens.iter().any(|v| v.eq_ignore_ascii_case("IIR")) {
        return Err("Custom IIR coefficient filters are not supported".into());
    }
    let kind_token = tokens[1].to_ascii_uppercase();
    let kind = match kind_token.as_str() {
        // Room EQ Wizard's modal filters are peaking filters with a Q.
        "PK" | "PEQ" | "MODAL" => FilterKind::Peaking,
        "LS" | "LSC" => FilterKind::LowShelf,
        "HS" | "HSC" => FilterKind::HighShelf,
        "LP" | "LPQ" => FilterKind::LowPass,
        "HP" | "HPQ" => FilterKind::HighPass,
        "BP" => FilterKind::BandPass,
        "NO" => FilterKind::Notch,
        "AP" => FilterKind::AllPass,
        _ => return Err(format!("Unsupported filter type: {}", tokens[1])),
    };
    let frequency = frequency_after(args)?;
    let gain_db = optional_number_after(&tokens, "Gain")?.unwrap_or(0.0);
    let shelf = matches!(kind, FilterKind::LowShelf | FilterKind::HighShelf);
    let given_q = optional_number_after(&tokens, "Q")?;
    // Read the way Equalizer APO reads it, so a preset sounds the same.
    let (frequency, q, bandwidth) = if shelf {
        // A slope in dB per octave right after the type (`LSC 12dB`, `LS 6dB`)
        // is 1 at 12 dB. Without a Q or slope APO uses 0.9. `LS`/`HS` with a Q
        // or slope give the corner frequency rather than the centre.
        let slope = shelf_slope(&tokens)?;
        let corner = !kind_token.ends_with('C') && (given_q.is_some() || slope.is_some());
        let q = match (slope, given_q) {
            (Some(slope), _) => shelf_slope_to_q(slope / 12.0, gain_db)?,
            (None, Some(q)) => q,
            (None, None) => shelf_slope_to_q(0.9, gain_db)?,
        };
        let frequency = if corner {
            shelf_centre(kind, frequency, gain_db, q)
        } else {
            frequency
        };
        (frequency, q, None)
    } else if let Some(q) = given_q {
        (frequency, q, None)
    } else if let Some(bandwidth) = bandwidth_after(&tokens)? {
        (frequency, bandwidth_to_q(bandwidth), Some(bandwidth))
    } else if kind == FilterKind::Notch {
        // APO's notch default, from Room EQ Wizard.
        (frequency, 30.0, None)
    } else {
        (frequency, std::f64::consts::FRAC_1_SQRT_2, None)
    };
    if !(1.0..=384000.0).contains(&frequency) {
        return Err("Filter frequency is outside 1–384000 Hz".into());
    }
    if !(-60.0..=60.0).contains(&gain_db) {
        return Err("Filter gain is outside -60–60 dB".into());
    }
    if !(0.01..=1000.0).contains(&q) {
        return Err("Filter Q is outside 0.01–1000".into());
    }
    Ok(Filter {
        enabled,
        kind,
        frequency,
        gain_db,
        q,
        bandwidth,
        channels,
    })
}

/// Fc as Equalizer APO reads it: Room EQ Wizard may write a thousands
/// separator as a dot (`Fc 1.500 Hz` is 1500 Hz). A comma is a decimal point.
fn frequency_after(args: &str) -> Result<f64, String> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let raw = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case("Fc"))
        .and_then(|index| tokens.get(index + 1))
        .ok_or_else(|| "Filter is missing Fc".to_owned())?;
    let thousands =
        raw.len() >= 5 && !raw.contains(['e', 'E', ',']) && raw.as_bytes()[raw.len() - 4] == b'.';
    let value = raw
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| "Invalid numeric value after Fc".to_owned())?;
    if !value.is_finite() {
        return Err("Value after Fc must be finite".into());
    }
    Ok(if thousands { value * 1000.0 } else { value })
}

/// A shelf's slope in dB per octave, written right after its type.
fn shelf_slope(tokens: &[&str]) -> Result<Option<f64>, String> {
    let Some(next) = tokens.get(2) else {
        return Ok(None);
    };
    let (number, unit) = match next.to_ascii_lowercase().strip_suffix("db") {
        Some(number) => (number.to_owned(), true),
        None => (
            (*next).to_owned(),
            tokens
                .get(3)
                .is_some_and(|unit| unit.eq_ignore_ascii_case("dB")),
        ),
    };
    if !unit {
        return Ok(None);
    }
    let slope = number
        .parse::<f64>()
        .map_err(|_| format!("Invalid shelf slope: {next}"))?;
    if !slope.is_finite() || slope <= 0.0 {
        return Err("Shelf slope must be positive".into());
    }
    Ok(Some(slope))
}

/// The Q of a shelf with slope `s` (1 is the steepest without overshoot).
fn shelf_slope_to_q(s: f64, gain_db: f64) -> Result<f64, String> {
    let amp = 10.0_f64.powf(gain_db / 40.0);
    let inverse_square = (amp + 1.0 / amp) * (1.0 / s - 1.0) + 2.0;
    if inverse_square <= 0.0 {
        return Err("Shelf slope is too steep for its gain".into());
    }
    Ok(1.0 / inverse_square.sqrt())
}

/// The centre of a shelf whose corner frequency is given, as APO computes it.
fn shelf_centre(kind: FilterKind, corner: f64, gain_db: f64, q: f64) -> f64 {
    let amp = 10.0_f64.powf(gain_db / 40.0);
    let s = 1.0 / ((1.0 / (q * q) - 2.0) / (amp + 1.0 / amp) + 1.0);
    let factor = 10.0_f64.powf(gain_db.abs() / 80.0 / s);
    if kind == FilterKind::LowShelf {
        corner * factor
    } else {
        corner / factor
    }
}

fn parse_graphic_eq(args: &str, channels: ChannelSelection) -> Result<GraphicEq, String> {
    let mut points = Vec::new();
    for item in args.split(';') {
        let normalized = item.trim().replace(',', ".");
        if normalized.is_empty() {
            continue;
        }
        let values: Vec<_> = normalized.split_whitespace().collect();
        if values.len() != 2 {
            return Err(format!("Invalid GraphicEQ point: {item}"));
        }
        let frequency = values[0]
            .parse::<f64>()
            .map_err(|_| format!("Invalid GraphicEQ frequency: {}", values[0]))?;
        let gain_db = values[1]
            .parse::<f64>()
            .map_err(|_| format!("Invalid GraphicEQ gain: {}", values[1]))?;
        if !frequency.is_finite() || !gain_db.is_finite() {
            return Err("GraphicEQ values must be finite".into());
        }
        if frequency <= 0.0 || frequency > 384_000.0 {
            return Err("GraphicEQ frequencies must be positive".into());
        }
        if !(-60.0..=60.0).contains(&gain_db) {
            return Err("GraphicEQ gain is outside -60–60 dB".into());
        }
        points.push(GraphicPoint { frequency, gain_db });
    }
    points.sort_by(|a, b| a.frequency.total_cmp(&b.frequency));
    if points.is_empty() {
        return Err("GraphicEQ needs at least one point".into());
    }
    if points
        .windows(2)
        .any(|pair| pair[0].frequency == pair[1].frequency)
    {
        return Err("GraphicEQ frequencies must be unique".into());
    }
    Ok(GraphicEq { channels, points })
}

fn optional_number_after(tokens: &[&str], key: &str) -> Result<Option<f64>, String> {
    let Some(index) = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case(key))
    else {
        return Ok(None);
    };
    let value = tokens
        .get(index + 1)
        .ok_or_else(|| format!("Filter is missing a value after {key}"))?
        .parse::<f64>()
        .map_err(|_| format!("Invalid numeric value after {key}"))?;
    if !value.is_finite() {
        return Err(format!("Value after {key} must be finite"));
    }
    Ok(Some(value))
}

fn bandwidth_after(tokens: &[&str]) -> Result<Option<f64>, String> {
    let Some(index) = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case("BW"))
    else {
        return Ok(None);
    };
    let next = tokens
        .get(index + 1)
        .ok_or_else(|| "Filter is missing a BW value".to_owned())?;
    let raw = if next.eq_ignore_ascii_case("Oct") {
        tokens
            .get(index + 2)
            .ok_or_else(|| "Filter is missing a BW Oct value".to_owned())?
    } else {
        next
    };
    let value = raw
        .parse::<f64>()
        .map_err(|_| "Invalid BW value".to_owned())?;
    if !value.is_finite() || value <= 0.0 {
        return Err("BW must be positive and finite".into());
    }
    Ok(Some(value))
}

/// A number as written to profile text: at most six decimals, without
/// trailing zeros. Saving never moves a value by more than 0.0000005, so a
/// saved profile sounds the same when loaded again.
fn decimal(value: f64) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn first_number(text: &str) -> Option<f64> {
    let re = Regex::new(r"[-+]?(?:\d+(?:[\.,]\d*)?|[\.,]\d+)").expect("valid regex");
    re.find(text)
        .and_then(|m| m.as_str().replace(',', ".").parse().ok())
}

fn bandwidth_to_q(bw: f64) -> f64 {
    let power = 2.0_f64.powf(bw);
    power.sqrt() / (power - 1.0)
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

fn diagnostic(
    profile: &mut ProfileDocument,
    level: DiagnosticLevel,
    source: &Path,
    line: usize,
    message: &str,
) {
    profile.diagnostics.push(Diagnostic {
        level,
        line,
        source: source.display().to_string(),
        message: message.to_owned(),
    });
}

fn enforce_limits(profile: &mut ProfileDocument) {
    for channel in [Channel::Left, Channel::Right] {
        if profile.filters_for(channel).count() > MAX_FILTERS_PER_CHANNEL {
            profile.diagnostics.push(Diagnostic {
                level: DiagnosticLevel::Error,
                line: 0,
                source: profile.name.clone(),
                message: format!("More than {MAX_FILTERS_PER_CHANNEL} filters target {channel:?}"),
            });
        }
    }
    if profile
        .graphic_eqs
        .iter()
        .map(|eq| eq.points.len())
        .sum::<usize>()
        > MAX_GRAPHIC_POINTS
    {
        profile.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            line: 0,
            source: profile.name.clone(),
            message: format!("More than {MAX_GRAPHIC_POINTS} GraphicEQ points"),
        });
    }
}

/// Turns the shelves after a tilt marker back into the profile's tilt. If
/// they were edited by hand and no longer match, they stay ordinary filters.
fn extract_tilt(profile: &mut ProfileDocument, marker: Option<(f64, usize)>) {
    let Some((slope, start)) = marker else {
        return;
    };
    let expected = tilt_filters(slope);
    let end = start + expected.len();
    let matches = profile.filters.get(start..end).is_some_and(|found| {
        found.iter().zip(&expected).all(|(found, expected)| {
            found.enabled
                && found.kind == expected.kind
                && found.channels == expected.channels
                && (found.frequency - expected.frequency).abs() < 0.006
                && (found.gain_db - expected.gain_db).abs() < 0.006
                && (found.q - expected.q).abs() < 0.00006
        })
    });
    if matches {
        profile.filters.drain(start..end);
        profile.tilt_db_per_octave = slope;
    }
}

pub fn serialize_profile(profile: &ProfileDocument) -> String {
    // Filters keep their document order, so band numbers in the editor, the
    // graph, and this text agree; a Channel: line is written whenever the
    // channel changes.
    let mut out = format!("# MassiveEQ profile: {}\n", profile.name);
    let mut current = None;
    let switch = |out: &mut String, current: &mut Option<ChannelSelection>, channel| {
        if *current != Some(channel) {
            *current = Some(channel);
            let name = match channel {
                ChannelSelection::All => "ALL",
                ChannelSelection::Left => "L",
                ChannelSelection::Right => "R",
            };
            out.push_str(&format!("\nChannel: {name}\n"));
        }
    };
    for gain in &profile.preamps {
        switch(&mut out, &mut current, gain.channels);
        out.push_str(&format!("Preamp: {} dB\n", decimal(gain.gain_db)));
    }
    for (index, filter) in profile.filters.iter().enumerate() {
        switch(&mut out, &mut current, filter.channels);
        let width = match filter.bandwidth {
            Some(bandwidth) => format!("BW Oct {}", decimal(bandwidth)),
            None => format!("Q {}", decimal(filter.q)),
        };
        out.push_str(&format!(
            "Filter {}: {} {} Fc {} Hz Gain {} dB {width}\n",
            index + 1,
            if filter.enabled { "ON" } else { "OFF" },
            filter.kind.apo_name(),
            decimal(filter.frequency),
            decimal(filter.gain_db),
        ));
    }
    let tilt = tilt_filters(profile.tilt_db_per_octave);
    if !tilt.is_empty() {
        out.push_str(&format!(
            "\n{TILT_MARKER} {:.4} dB/oct around 1 kHz (the filters below apply it)\n",
            profile.tilt_db_per_octave
        ));
        current = None;
    }
    for filter in &tilt {
        switch(&mut out, &mut current, filter.channels);
        out.push_str(&format!(
            "Filter: ON {} Fc {} Hz Gain {} dB Q {}\n",
            filter.kind.apo_name(),
            decimal(filter.frequency),
            decimal(filter.gain_db),
            decimal(filter.q)
        ));
    }
    for eq in &profile.graphic_eqs {
        switch(&mut out, &mut current, eq.channels);
        let points = eq
            .points
            .iter()
            .map(|p| format!("{} {}", decimal(p.frequency), decimal(p.gain_db)))
            .collect::<Vec<_>>()
            .join("; ");
        out.push_str(&format!("GraphicEQ: {points}\n"));
    }
    for convolution in &profile.convolutions {
        switch(&mut out, &mut current, convolution.channels);
        out.push_str(&format!("Convolution: {}\n", convolution.path.display()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_squiglink_and_per_ear_filters() {
        let profile = parse_text(
            "fixture",
            r#"
Preamp: -6.8 dB
Filter 1: ON PK Fc 21 Hz Gain 6.7 dB Q 1.100
Channel: L
Filter 2: ON LSC Fc 105 Hz Gain 2.5 dB Q 0.707
Channel: R
Filter 3: OFF HSC Fc 8000 Hz Gain -1.5 dB Q 0.8
"#,
        );
        assert!(profile.is_activatable(), "{:?}", profile.diagnostics);
        assert_eq!(profile.filters.len(), 3);
        assert_eq!(profile.preamp_for(Channel::Left), -6.8);
        assert_eq!(profile.filters_for(Channel::Left).count(), 2);
        assert_eq!(profile.filters_for(Channel::Right).count(), 2);
    }

    #[test]
    fn serializing_keeps_band_order_across_channels() {
        let profile = parse_text(
            "fixture",
            "Preamp: -3 dB\nFilter 1: ON LSC Fc 105 Hz Gain 4 dB Q 0.7\nChannel: L\n\
             Filter 2: ON PK Fc 220 Hz Gain -2 dB Q 1.2\nChannel: ALL\n\
             Filter 3: ON PK Fc 1450 Hz Gain 1.5 dB Q 1.8",
        );
        let round = parse_text("fixture", &serialize_profile(&profile));
        assert!(round.is_activatable(), "{:?}", round.diagnostics);
        assert_eq!(round.filters, profile.filters);
        assert_eq!(round.preamp_for(Channel::Left), -3.0);
        assert_eq!(round.preamp_for(Channel::Right), -3.0);
    }

    #[test]
    fn tilt_round_trips_as_plain_apo_filters() {
        let mut profile = parse_text(
            "fixture",
            "Channel: L\nFilter 1: ON PK Fc 220 Hz Gain -2 dB Q 1.2\nGraphicEQ: 20 0; 20000 0",
        );
        profile.tilt_db_per_octave = -0.35;
        let text = serialize_profile(&profile);
        // Any APO host sees five ordinary shelves on both channels.
        assert_eq!(text.matches("Filter: ON ").count(), 5, "{text}");
        let round = parse_text("fixture", &text);
        assert!(round.is_activatable(), "{:?}", round.diagnostics);
        assert_eq!(round.tilt_db_per_octave, -0.35);
        assert_eq!(round.filters, profile.filters);
        assert_eq!(round.graphic_eqs[0].channels, ChannelSelection::Left);
        assert_eq!(round.active_filters_for(Channel::Right).len(), 5);
    }

    #[test]
    fn hand_edited_tilt_shelves_stay_ordinary_filters() {
        let mut profile = ProfileDocument::empty("fixture");
        profile.tilt_db_per_octave = 0.5;
        let text = serialize_profile(&profile).replace("Gain 1 dB", "Gain 1.5 dB");
        let round = parse_text("fixture", &text);
        assert_eq!(round.tilt_db_per_octave, 0.0);
        assert_eq!(round.filters.len(), 5);
    }

    #[test]
    fn supports_bw_and_decimal_comma() {
        let profile = parse_text(
            "fixture",
            "Filter: ON PEQ Fc 100,5 Hz Gain 1,5 dB BW Oct 1,0",
        );
        assert!(profile.is_activatable());
        assert!((profile.filters[0].q - std::f64::consts::SQRT_2).abs() < 0.0001);
    }

    #[test]
    fn parses_graphic_eq() {
        let profile = parse_text("fixture", "GraphicEQ: 20 1; 1000 -2.5; 20000 0");
        assert!(profile.is_activatable());
        assert_eq!(profile.graphic_eqs[0].points.len(), 3);
    }

    #[test]
    fn rejects_duplicate_or_non_finite_graphic_points() {
        assert!(!parse_text("fixture", "GraphicEQ: 100 1; 100 -2").is_activatable());
        assert!(!parse_text("fixture", "GraphicEQ: 100 NaN").is_activatable());
    }

    #[test]
    fn rejects_invalid_optional_filter_numbers() {
        assert!(!parse_text("fixture", "Filter: ON PK Fc 1000 Hz Gain nope Q 1").is_activatable());
        assert!(!parse_text("fixture", "Filter: ON PK Fc 1000 Hz Gain 1 Q NaN").is_activatable());
    }

    #[test]
    fn accepts_eq_together_with_convolution() {
        let profile = parse_text(
            "fixture",
            "Filter: ON PK Fc 1000 Hz Gain 1 dB Q 1\nConvolution: room.wav",
        );
        assert!(profile.is_activatable());
        assert_eq!(profile.filters.len(), 1);
        assert_eq!(profile.convolutions.len(), 1);
    }

    #[test]
    fn rejects_unsupported_audio_command() {
        let profile = parse_text("fixture", "Copy: L=R");
        assert!(!profile.is_activatable());
        assert!(profile.diagnostics[0].message.contains("Unsupported"));
    }

    #[test]
    fn resolves_nested_include() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("child.txt"),
            "Filter: ON PK Fc 500 Hz Gain 2 dB Q 1",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("root.txt"),
            "Preamp: -2 dB\nInclude: child.txt",
        )
        .unwrap();
        let profile = parse_file(temp.path().join("root.txt")).unwrap();
        assert!(profile.is_activatable());
        assert_eq!(profile.filters.len(), 1);
    }
}
