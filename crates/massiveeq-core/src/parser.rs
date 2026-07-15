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
}

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
    };
    parse_path_into(path, 0, &mut state, &mut profile)?;
    enforce_limits(&mut profile);
    Ok(profile)
}

pub fn parse_text(name: impl Into<String>, text: &str) -> ProfileDocument {
    let mut profile = ProfileDocument::empty(name);
    let mut state = ParserState {
        channels: ChannelSelection::All,
        visited: HashSet::new(),
    };
    parse_lines(text, Path::new("profile.txt"), 0, &mut state, &mut profile);
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
            "preamp" => match first_number(args) {
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
                let path = source_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(unquote(args));
                profile.convolutions.push(Convolution {
                    channels: state.channels,
                    path,
                });
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
        "PK" | "PEQ" => FilterKind::Peaking,
        "LS" | "LSC" => FilterKind::LowShelf,
        "HS" | "HSC" => FilterKind::HighShelf,
        "LP" | "LPQ" => FilterKind::LowPass,
        "HP" | "HPQ" => FilterKind::HighPass,
        "BP" => FilterKind::BandPass,
        "NO" => FilterKind::Notch,
        "AP" => FilterKind::AllPass,
        _ => return Err(format!("Unsupported filter type: {}", tokens[1])),
    };
    let frequency = number_after(&tokens, "Fc").ok_or("Filter is missing Fc")?;
    let gain_db = number_after(&tokens, "Gain").unwrap_or(0.0);
    let q = if let Some(q) = number_after(&tokens, "Q") {
        q
    } else if let Some(bw) = bandwidth_after(&tokens) {
        bandwidth_to_q(bw)
    } else {
        std::f64::consts::FRAC_1_SQRT_2
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
        channels,
    })
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
        if frequency <= 0.0 {
            return Err("GraphicEQ frequencies must be positive".into());
        }
        points.push(GraphicPoint { frequency, gain_db });
    }
    points.sort_by(|a, b| a.frequency.total_cmp(&b.frequency));
    if points.is_empty() {
        return Err("GraphicEQ needs at least one point".into());
    }
    Ok(GraphicEq { channels, points })
}

fn number_after(tokens: &[&str], key: &str) -> Option<f64> {
    tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case(key))
        .and_then(|index| tokens.get(index + 1))
        .and_then(|value| value.parse().ok())
}

fn bandwidth_after(tokens: &[&str]) -> Option<f64> {
    let index = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case("BW"))?;
    let next = tokens.get(index + 1)?;
    if next.eq_ignore_ascii_case("Oct") {
        tokens.get(index + 2)?.parse().ok()
    } else {
        next.parse().ok()
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

pub fn serialize_profile(profile: &ProfileDocument) -> String {
    let mut out = format!("# MassiveEQ profile: {}\n", profile.name);
    for channel in [
        ChannelSelection::All,
        ChannelSelection::Left,
        ChannelSelection::Right,
    ] {
        let channel_name = match channel {
            ChannelSelection::All => "ALL",
            ChannelSelection::Left => "L",
            ChannelSelection::Right => "R",
        };
        let has_content = profile.preamps.iter().any(|v| v.channels == channel)
            || profile.filters.iter().any(|v| v.channels == channel)
            || profile.graphic_eqs.iter().any(|v| v.channels == channel)
            || profile.convolutions.iter().any(|v| v.channels == channel);
        if !has_content {
            continue;
        }
        out.push_str(&format!("\nChannel: {channel_name}\n"));
        for gain in profile.preamps.iter().filter(|v| v.channels == channel) {
            out.push_str(&format!("Preamp: {:.2} dB\n", gain.gain_db));
        }
        for (index, filter) in profile
            .filters
            .iter()
            .filter(|v| v.channels == channel)
            .enumerate()
        {
            out.push_str(&format!(
                "Filter {}: {} {} Fc {:.2} Hz Gain {:.2} dB Q {:.4}\n",
                index + 1,
                if filter.enabled { "ON" } else { "OFF" },
                filter.kind.apo_name(),
                filter.frequency,
                filter.gain_db,
                filter.q
            ));
        }
        for eq in profile.graphic_eqs.iter().filter(|v| v.channels == channel) {
            let points = eq
                .points
                .iter()
                .map(|p| format!("{:.2} {:.2}", p.frequency, p.gain_db))
                .collect::<Vec<_>>()
                .join("; ");
            out.push_str(&format!("GraphicEQ: {points}\n"));
        }
        for convolution in profile
            .convolutions
            .iter()
            .filter(|v| v.channels == channel)
        {
            out.push_str(&format!("Convolution: {}\n", convolution.path.display()));
        }
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
