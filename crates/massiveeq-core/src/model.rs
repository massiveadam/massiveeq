use crate::{
    layout::ImpulseLayout,
    storage::{COMPARISON_BYPASS_ID, ComparisonSet},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

pub const MAX_FILTERS_PER_CHANNEL: usize = 64;
pub const MAX_GRAPHIC_POINTS: usize = 4096;
pub const MAX_INCLUDE_DEPTH: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelSelection {
    All,
    Left,
    Right,
}

impl ChannelSelection {
    pub fn contains(self, channel: Channel) -> bool {
        matches!(self, Self::All)
            || matches!(
                (self, channel),
                (Self::Left, Channel::Left) | (Self::Right, Channel::Right)
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterKind {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    BandPass,
    Notch,
    AllPass,
}

impl FilterKind {
    pub fn apo_name(self) -> &'static str {
        match self {
            Self::Peaking => "PK",
            Self::LowShelf => "LSC",
            Self::HighShelf => "HSC",
            Self::LowPass => "LPQ",
            Self::HighPass => "HPQ",
            Self::BandPass => "BP",
            Self::Notch => "NO",
            Self::AllPass => "AP",
        }
    }

    pub fn pipewire_label(self) -> &'static str {
        match self {
            Self::Peaking => "bq_peaking",
            Self::LowShelf => "bq_lowshelf",
            Self::HighShelf => "bq_highshelf",
            Self::LowPass => "bq_lowpass",
            Self::HighPass => "bq_highpass",
            Self::BandPass => "bq_bandpass",
            Self::Notch => "bq_notch",
            Self::AllPass => "bq_allpass",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub enabled: bool,
    pub kind: FilterKind,
    pub frequency: f64,
    pub gain_db: f64,
    pub q: f64,
    pub channels: ChannelSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphicPoint {
    pub frequency: f64,
    pub gain_db: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphicEq {
    pub channels: ChannelSelection,
    pub points: Vec<GraphicPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Convolution {
    pub channels: ChannelSelection,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelGain {
    pub channels: ChannelSelection,
    pub gain_db: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticLevel {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub line: usize,
    pub source: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileDocument {
    pub name: String,
    pub preamps: Vec<ChannelGain>,
    pub filters: Vec<Filter>,
    pub graphic_eqs: Vec<GraphicEq>,
    pub convolutions: Vec<Convolution>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ProfileDocument {
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            preamps: Vec::new(),
            filters: Vec::new(),
            graphic_eqs: Vec::new(),
            convolutions: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn is_activatable(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|item| item.level == DiagnosticLevel::Error)
    }

    pub fn preamp_for(&self, channel: Channel) -> f64 {
        self.preamps
            .iter()
            .filter(|gain| gain.channels.contains(channel))
            .map(|gain| gain.gain_db)
            .sum()
    }

    pub fn filters_for(&self, channel: Channel) -> impl Iterator<Item = &Filter> {
        self.filters
            .iter()
            .filter(move |filter| filter.channels.contains(channel))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceKey {
    pub backend: String,
    pub stable_id: String,
    pub route: String,
}

impl DeviceKey {
    pub fn as_storage_key(&self) -> String {
        format!("{}|{}|{}", self.backend, self.stable_id, self.route)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub key: DeviceKey,
    pub node_name: String,
    pub description: String,
    pub channels: u32,
    pub connected: bool,
    pub assigned_profile: Option<String>,
    pub bypassed: bool,
    /// PipeWire's current default output, i.e. where desktop audio plays.
    #[serde(default)]
    pub is_default: bool,
}

impl DeviceInfo {
    /// The profile this output plays: the active comparison candidate while a
    /// comparison runs, otherwise its assigned profile.
    pub fn effective_profile_id<'a>(
        &'a self,
        comparisons: &'a HashMap<String, ComparisonSet>,
    ) -> Option<&'a str> {
        comparisons
            .get(&self.key.as_storage_key())
            .filter(|comparison| comparison.enabled)
            .map(|comparison| comparison.active_profile_id.as_str())
            .filter(|id| *id != COMPARISON_BYPASS_ID)
            .or(self.assigned_profile.as_deref())
    }

    /// Whether this output's filters are running: a supported channel layout,
    /// not `bypassed`, and not on the bypass side of a comparison. Takes the
    /// bypass state separately so callers can pass a not-yet-confirmed toggle.
    pub fn is_filtering(
        &self,
        comparisons: &HashMap<String, ComparisonSet>,
        bypassed: bool,
    ) -> bool {
        matches!(self.channels, 1 | 2)
            && !bypassed
            && comparisons
                .get(&self.key.as_storage_key())
                .filter(|comparison| comparison.enabled)
                .is_none_or(|comparison| comparison.active_profile_id != COMPARISON_BYPASS_ID)
    }

    /// Profile that is audible on this output right now, if any.
    pub fn playing_profile_id<'a>(
        &'a self,
        comparisons: &'a HashMap<String, ComparisonSet>,
    ) -> Option<&'a str> {
        (self.connected && self.is_filtering(comparisons, self.bypassed))
            .then(|| self.effective_profile_id(comparisons))
            .flatten()
    }

    /// The physical output, ignoring its route: a Bluetooth headset that
    /// switches between call and music profiles is still the same device.
    pub fn same_hardware(&self, other: &DeviceInfo) -> bool {
        self.key.backend == other.key.backend && self.key.stable_id == other.key.stable_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedDevice {
    pub key: DeviceKey,
    pub node_name: String,
    pub description: String,
    pub channels: u32,
}

impl From<&DeviceInfo> for RememberedDevice {
    fn from(device: &DeviceInfo) -> Self {
        Self {
            key: device.key.clone(),
            node_name: device.node_name.clone(),
            description: device.description.clone(),
            channels: device.channels,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub id: String,
    pub name: String,
    pub text: String,
    pub manual_trim_db: f64,
    pub activatable: bool,
    pub diagnostics: Vec<Diagnostic>,
    /// Layout of the profile's impulse response, when it uses convolution.
    #[serde(default)]
    pub impulse_layout: Option<ImpulseLayout>,
}

/// Profile bank order: by name, case-insensitively, then by id for equal names.
pub fn sort_profiles(profiles: &mut [ProfileInfo]) {
    profiles.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(channels: u32, profile: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            key: DeviceKey {
                backend: "bluez5".into(),
                stable_id: "AA:BB".into(),
                route: "a2dp-sink".into(),
            },
            node_name: "buds".into(),
            description: "Buds".into(),
            channels,
            connected: true,
            assigned_profile: profile.map(str::to_owned),
            bypassed: false,
            is_default: true,
        }
    }

    fn comparing(device: &DeviceInfo, active: &str) -> HashMap<String, ComparisonSet> {
        HashMap::from([(
            device.key.as_storage_key(),
            ComparisonSet {
                profile_ids: vec!["flat".into(), "warm".into(), COMPARISON_BYPASS_ID.into()],
                active_profile_id: active.into(),
                enabled: true,
            },
        )])
    }

    #[test]
    fn active_comparison_candidate_is_the_effective_profile() {
        let device = output(2, Some("flat"));
        assert_eq!(device.effective_profile_id(&HashMap::new()), Some("flat"));
        assert_eq!(
            device.effective_profile_id(&comparing(&device, "warm")),
            Some("warm")
        );
        // The bypass slot plays nothing, but the output still belongs to its profile.
        let bypass = comparing(&device, COMPARISON_BYPASS_ID);
        assert_eq!(device.effective_profile_id(&bypass), Some("flat"));
        assert_eq!(device.playing_profile_id(&bypass), None);
    }

    #[test]
    fn bypassed_disconnected_or_surround_outputs_play_nothing() {
        let none = HashMap::new();
        let device = output(2, Some("flat"));
        assert_eq!(device.playing_profile_id(&none), Some("flat"));
        assert!(!device.is_filtering(&none, true));
        let bypassed = DeviceInfo {
            bypassed: true,
            ..device.clone()
        };
        assert_eq!(bypassed.playing_profile_id(&none), None);
        let gone = DeviceInfo {
            connected: false,
            ..device.clone()
        };
        assert_eq!(gone.playing_profile_id(&none), None);
        assert_eq!(output(6, Some("flat")).playing_profile_id(&none), None);
    }

    #[test]
    fn bluetooth_route_changes_are_the_same_hardware() {
        let music = output(2, None);
        let mut call = music.clone();
        call.key.route = "headset-head-unit".into();
        assert!(music.same_hardware(&call));
        call.key.stable_id = "CC:DD".into();
        assert!(!music.same_hardware(&call));
    }

    #[test]
    fn profiles_sort_by_name_ignoring_case() {
        let profile = |id: &str, name: &str| ProfileInfo {
            id: id.into(),
            name: name.into(),
            text: String::new(),
            manual_trim_db: 0.0,
            activatable: true,
            diagnostics: Vec::new(),
            impulse_layout: None,
        };
        let mut profiles = vec![
            profile("1", "studio"),
            profile("2", "AirPods"),
            profile("3", "Harman"),
            profile("0", "airpods"),
        ];
        sort_profiles(&mut profiles);
        let ids = profiles.iter().map(|p| p.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids, ["0", "2", "3", "1"]);
    }
}
