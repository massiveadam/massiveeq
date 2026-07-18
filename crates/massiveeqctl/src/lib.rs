use massiveeq_core::{ComparisonSet, DeviceInfo, ProfileInfo, parse_text};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    Unavailable,
    EngineOff,
    Active,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickProfile {
    pub id: String,
    pub name: String,
    pub activatable: bool,
    pub band_count: usize,
    pub filters: Vec<QuickFilter>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickFilter {
    pub index: usize,
    pub enabled: bool,
    pub kind: String,
    pub frequency_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickComparison {
    pub enabled: bool,
    pub active_profile_id: String,
    pub profile_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickOutput {
    pub key: String,
    pub node_name: String,
    pub description: String,
    pub connected: bool,
    pub bypassed: bool,
    pub assigned_profile_id: Option<String>,
    pub comparison: Option<QuickComparison>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickSnapshot {
    pub schema_version: u32,
    pub online: bool,
    pub state: EngineState,
    pub global_bypass: bool,
    pub profiles: Vec<QuickProfile>,
    pub outputs: Vec<QuickOutput>,
    pub error: Option<String>,
}

impl QuickSnapshot {
    pub fn online(
        profiles: Vec<ProfileInfo>,
        devices: Vec<DeviceInfo>,
        comparisons: HashMap<String, ComparisonSet>,
        status: &serde_json::Value,
    ) -> Self {
        let global_bypass = status
            .get("global_bypass")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let outputs = devices
            .into_iter()
            .map(|device| {
                let key = device.key.as_storage_key();
                let comparison = comparisons.get(&key).map(|comparison| QuickComparison {
                    enabled: comparison.enabled,
                    active_profile_id: comparison.active_profile_id.clone(),
                    profile_ids: comparison.profile_ids.clone(),
                });
                QuickOutput {
                    key,
                    node_name: device.node_name,
                    description: device.description,
                    connected: device.connected,
                    bypassed: device.bypassed,
                    assigned_profile_id: device.assigned_profile,
                    comparison,
                }
            })
            .collect::<Vec<_>>();
        let state = derive_state(global_bypass, &outputs);
        let profiles = profiles
            .into_iter()
            .map(|profile| {
                let document = parse_text(&profile.name, &profile.text);
                let band_count = document.filters.len()
                    + document
                        .graphic_eqs
                        .iter()
                        .map(|equalizer| equalizer.points.len())
                        .sum::<usize>();
                let filters = document
                    .filters
                    .iter()
                    .enumerate()
                    .map(|(index, filter)| QuickFilter {
                        index,
                        enabled: filter.enabled,
                        kind: filter.kind.apo_name().to_owned(),
                        frequency_hz: filter.frequency,
                        gain_db: filter.gain_db,
                        q: filter.q,
                    })
                    .collect();
                QuickProfile {
                    id: profile.id,
                    name: profile.name,
                    activatable: profile.activatable,
                    band_count,
                    filters,
                }
            })
            .collect();

        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            online: true,
            state,
            global_bypass,
            profiles,
            outputs,
            error: None,
        }
    }

    pub fn offline(error: impl std::fmt::Display) -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            online: false,
            state: EngineState::Unavailable,
            global_bypass: false,
            profiles: Vec::new(),
            outputs: Vec::new(),
            error: Some(error.to_string()),
        }
    }
}

fn derive_state(global_bypass: bool, outputs: &[QuickOutput]) -> EngineState {
    if global_bypass {
        EngineState::EngineOff
    } else if outputs.iter().any(|output| {
        output.connected
            && !output.bypassed
            && (output.assigned_profile_id.is_some()
                || output
                    .comparison
                    .as_ref()
                    .is_some_and(|comparison| comparison.enabled))
    }) {
        EngineState::Active
    } else {
        EngineState::Idle
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Status {
        watch: bool,
    },
    Engine {
        enabled: bool,
    },
    Filters {
        device_key: String,
        enabled: bool,
    },
    Assign {
        device_key: String,
        profile_id: String,
    },
    Unassign {
        device_key: String,
    },
    Compare {
        device_key: String,
        profile_id: String,
    },
    SetFilter {
        profile_id: String,
        filter_index: usize,
        frequency_hz: f64,
        gain_db: f64,
        q: f64,
    },
}

impl Command {
    pub fn parse<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
        match args.as_slice() {
            [command] if command == "status" => Ok(Self::Status { watch: false }),
            [command, option] if command == "status" && option == "--watch" => {
                Ok(Self::Status { watch: true })
            }
            [command, value] if command == "engine" => Ok(Self::Engine {
                enabled: parse_on_off(value)?,
            }),
            [command, device_key, value] if command == "filters" => Ok(Self::Filters {
                device_key: require_value("device key", device_key)?,
                enabled: parse_on_off(value)?,
            }),
            [command, device_key, profile_id] if command == "assign" => Ok(Self::Assign {
                device_key: require_value("device key", device_key)?,
                profile_id: require_value("profile id", profile_id)?,
            }),
            [command, device_key] if command == "unassign" => Ok(Self::Unassign {
                device_key: require_value("device key", device_key)?,
            }),
            [command, device_key, profile_id] if command == "compare" => Ok(Self::Compare {
                device_key: require_value("device key", device_key)?,
                profile_id: require_value("profile id", profile_id)?,
            }),
            [command, profile_id, filter_index, frequency_hz, gain_db, q]
                if command == "set-filter" =>
            {
                let filter_index = parse_usize("filter index", filter_index)?;
                let frequency_hz = parse_number("frequency", frequency_hz, 20.0, 20_000.0)?;
                let gain_db = parse_number("gain", gain_db, -60.0, 60.0)?;
                let q = parse_number("Q", q, 0.01, 1_000.0)?;
                Ok(Self::SetFilter {
                    profile_id: require_value("profile id", profile_id)?,
                    filter_index,
                    frequency_hz,
                    gain_db,
                    q,
                })
            }
            _ => Err(usage().into()),
        }
    }
}

impl Command {
    pub fn bypassed_value(&self) -> Option<bool> {
        match self {
            Self::Filters { enabled, .. } => Some(!enabled),
            _ => None,
        }
    }
}

fn parse_on_off(value: &str) -> Result<bool, String> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("expected 'on' or 'off', got '{value}'")),
    }
}

fn require_value(label: &str, value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Err(format!("{label} must not be empty"))
    } else {
        Ok(value.to_owned())
    }
}

fn parse_usize(label: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{label} must be a non-negative integer"))
}

fn parse_number(label: &str, value: &str, minimum: f64, maximum: f64) -> Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("{label} must be a number"))?;
    if !parsed.is_finite() || !(minimum..=maximum).contains(&parsed) {
        return Err(format!("{label} must be between {minimum} and {maximum}"));
    }
    Ok(parsed)
}

pub fn usage() -> &'static str {
    "Usage:\n  massiveeqctl status [--watch]\n  massiveeqctl engine <on|off>\n  massiveeqctl filters <device-key> <on|off>\n  massiveeqctl assign <device-key> <profile-id>\n  massiveeqctl unassign <device-key>\n  massiveeqctl compare <device-key> <profile-id>\n  massiveeqctl set-filter <profile-id> <filter-index> <frequency-hz> <gain-db> <q>"
}

#[cfg(test)]
mod tests {
    use super::*;
    use massiveeq_core::{DeviceKey, Diagnostic};

    fn profile(id: &str, name: &str, activatable: bool) -> ProfileInfo {
        ProfileInfo {
            id: id.into(),
            name: name.into(),
            text: String::new(),
            manual_trim_db: 0.0,
            activatable,
            diagnostics: Vec::<Diagnostic>::new(),
        }
    }

    fn profile_with_filter(id: &str, name: &str) -> ProfileInfo {
        ProfileInfo {
            id: id.into(),
            name: name.into(),
            text: "Filter: ON PK Fc 1000 Hz Gain 4 dB Q 1.0".into(),
            manual_trim_db: 0.0,
            activatable: true,
            diagnostics: Vec::<Diagnostic>::new(),
        }
    }

    fn device(id: &str, connected: bool, assigned: Option<&str>, bypassed: bool) -> DeviceInfo {
        DeviceInfo {
            key: DeviceKey {
                backend: "alsa".into(),
                stable_id: id.into(),
                route: "stereo".into(),
            },
            node_name: format!("node-{id}"),
            description: format!("Output {id}"),
            channels: 2,
            connected,
            assigned_profile: assigned.map(str::to_owned),
            bypassed,
        }
    }

    #[test]
    fn online_snapshot_preserves_invalid_profiles_and_comparisons() {
        let devices = vec![
            device("one", true, Some("valid"), false),
            device("two", false, None, false),
        ];
        let first_key = devices[0].key.as_storage_key();
        let comparisons = HashMap::from([(
            first_key.clone(),
            ComparisonSet {
                profile_ids: vec!["valid".into(), "invalid".into()],
                active_profile_id: "invalid".into(),
                enabled: true,
            },
        )]);
        let snapshot = QuickSnapshot::online(
            vec![
                profile("valid", "Valid", true),
                profile("invalid", "Needs work", false),
            ],
            devices,
            comparisons,
            &serde_json::json!({ "global_bypass": false }),
        );

        assert_eq!(snapshot.state, EngineState::Active);
        assert_eq!(snapshot.profiles.len(), 2);
        assert!(!snapshot.profiles[1].activatable);
        assert_eq!(snapshot.outputs[0].key, first_key);
        assert_eq!(
            snapshot.outputs[0]
                .comparison
                .as_ref()
                .unwrap()
                .active_profile_id,
            "invalid"
        );
        assert!(!snapshot.outputs[1].connected);
    }

    #[test]
    fn engine_state_distinguishes_off_active_and_idle() {
        let active = QuickSnapshot::online(
            vec![profile("p", "Profile", true)],
            vec![device("one", true, Some("p"), false)],
            HashMap::new(),
            &serde_json::json!({ "global_bypass": false }),
        );
        let idle = QuickSnapshot::online(
            Vec::new(),
            vec![device("one", true, None, false)],
            HashMap::new(),
            &serde_json::json!({ "global_bypass": false }),
        );
        let off = QuickSnapshot::online(
            Vec::new(),
            Vec::new(),
            HashMap::new(),
            &serde_json::json!({ "global_bypass": true }),
        );

        assert_eq!(active.state, EngineState::Active);
        assert_eq!(idle.state, EngineState::Idle);
        assert_eq!(off.state, EngineState::EngineOff);
    }

    #[test]
    fn offline_snapshot_has_stable_schema() {
        let snapshot = QuickSnapshot::offline("service unavailable");
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(json["schema_version"], SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(json["state"], "unavailable");
        assert_eq!(json["online"], false);
        assert_eq!(json["error"], "service unavailable");
    }

    #[test]
    fn active_profile_metadata_contains_editable_filters() {
        let snapshot = QuickSnapshot::online(
            vec![profile_with_filter("filtered", "Filtered")],
            vec![device("one", true, Some("filtered"), false)],
            HashMap::new(),
            &serde_json::json!({ "global_bypass": false }),
        );

        let profile = &snapshot.profiles[0];
        assert_eq!(profile.band_count, 1);
        assert_eq!(profile.filters.len(), 1);
        assert_eq!(profile.filters[0].kind, "PK");
        assert_eq!(profile.filters[0].frequency_hz, 1_000.0);
        assert_eq!(profile.filters[0].gain_db, 4.0);
        assert_eq!(profile.filters[0].q, 1.0);
    }

    #[test]
    fn parses_all_public_commands() {
        assert_eq!(
            Command::parse(["status", "--watch"]),
            Ok(Command::Status { watch: true })
        );
        assert_eq!(
            Command::parse(["engine", "on"]),
            Ok(Command::Engine { enabled: true })
        );
        assert_eq!(
            Command::parse(["filters", "device", "off"]),
            Ok(Command::Filters {
                device_key: "device".into(),
                enabled: false,
            })
        );
        assert_eq!(
            Command::parse(["assign", "device", "profile"]),
            Ok(Command::Assign {
                device_key: "device".into(),
                profile_id: "profile".into(),
            })
        );
        assert_eq!(
            Command::parse(["unassign", "device"]),
            Ok(Command::Unassign {
                device_key: "device".into(),
            })
        );
        assert_eq!(
            Command::parse(["compare", "device", "profile"]),
            Ok(Command::Compare {
                device_key: "device".into(),
                profile_id: "profile".into(),
            })
        );
        assert_eq!(
            Command::parse(["set-filter", "profile", "2", "1200", "-3.5", "1.25"]),
            Ok(Command::SetFilter {
                profile_id: "profile".into(),
                filter_index: 2,
                frequency_hz: 1_200.0,
                gain_db: -3.5,
                q: 1.25,
            })
        );
    }

    #[test]
    fn filters_inverts_to_dbus_bypass_value() {
        let on = Command::parse(["filters", "device", "on"]).unwrap();
        let off = Command::parse(["filters", "device", "off"]).unwrap();
        assert_eq!(on.bypassed_value(), Some(false));
        assert_eq!(off.bypassed_value(), Some(true));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(Command::parse(["engine", "maybe"]).is_err());
        assert!(Command::parse(["status", "--json"]).is_err());
        assert!(Command::parse(["set-filter", "profile", "nope", "1000", "0", "1"]).is_err());
        assert!(Command::parse(["set-filter", "profile", "0", "5", "0", "1"]).is_err());
        assert!(Command::parse(["set-filter", "profile", "0", "1000", "0", "0"]).is_err());
        assert!(Command::parse(["assign", "device"]).is_err());
    }
}
