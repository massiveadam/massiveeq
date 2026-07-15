use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
}
