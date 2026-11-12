//! Microphone profiles are independent of playback's library and gain matching.
use crate::{ChannelSelection, Filter, FilterKind, ProfileDocument, parse_text};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
};

pub const MAX_INPUT_FILTERS: usize = 16;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputLibrary {
    pub version: u32,
    pub active_profile: String,
    pub profiles: Vec<InputProfile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputProfile {
    pub id: String,
    pub name: String,
    pub source: String,
    pub channels: Vec<String>,
    pub eq_enabled: bool,
    pub filters: Vec<Filter>,
    pub rnnoise: NoiseReduction,
    pub gate: Gate,
    pub compressor: Compressor,
    pub advanced_project: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NoiseReduction {
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Gate {
    pub enabled: bool,
    pub threshold_db: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Compressor {
    pub enabled: bool,
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub makeup_db: f64,
}

impl Default for InputProfile {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Microphone".into(),
            source: String::new(),
            channels: vec!["MONO".into()],
            eq_enabled: true,
            filters: Vec::new(),
            rnnoise: NoiseReduction::default(),
            gate: Gate {
                enabled: false,
                threshold_db: -45.0,
                attack_ms: 5.0,
                release_ms: 100.0,
            },
            compressor: Compressor {
                enabled: false,
                threshold_db: -18.0,
                ratio: 3.0,
                attack_ms: 10.0,
                release_ms: 120.0,
                makeup_db: 0.0,
            },
            advanced_project: None,
        }
    }
}

impl Default for InputLibrary {
    fn default() -> Self {
        let profile = InputProfile::default();
        Self {
            version: 1,
            active_profile: profile.id.clone(),
            profiles: vec![profile],
        }
    }
}

impl InputProfile {
    pub fn document(&self) -> ProfileDocument {
        let mut document = parse_text(&self.name, "");
        document.filters = self.filters.clone();
        document
    }
}

impl InputLibrary {
    pub fn path() -> Result<PathBuf, String> {
        dirs::config_dir()
            .map(|p| p.join("massiveeq/input.json"))
            .ok_or_else(|| "Could not find the user configuration directory".into())
    }

    pub fn active(&self) -> &InputProfile {
        self.profiles
            .iter()
            .find(|p| p.id == self.active_profile)
            .expect("validated input library has an active profile")
    }

    pub fn active_mut(&mut self) -> &mut InputProfile {
        self.profiles
            .iter_mut()
            .find(|p| p.id == self.active_profile)
            .expect("validated input library has an active profile")
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.to_string()),
        };
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let library = if value.get("version").is_some() {
            serde_json::from_value(value).map_err(|e| e.to_string())?
        } else {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Legacy {
                source: String,
                project: PathBuf,
                #[serde(default = "mono_channels")]
                channels: Vec<String>,
            }
            let old: Legacy = serde_json::from_value(value).map_err(|e| e.to_string())?;
            let mut library = Self::default();
            let profile = library.active_mut();
            profile.source = old.source;
            profile.channels = old.channels;
            profile.advanced_project = Some(old.project);
            library
        };
        library.validate()?;
        Ok(library)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let parent = path
            .parent()
            .ok_or("Invalid microphone configuration path")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(&mut file, self).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.profiles.is_empty() || self.profiles.len() > 128 {
            return Err("Unsupported or empty microphone profile library".into());
        }
        let mut ids = HashSet::new();
        for p in &self.profiles {
            if p.id.trim().is_empty()
                || p.id.len() > 128
                || !ids.insert(&p.id)
                || p.name.trim().is_empty()
                || p.name.len() > 128
            {
                return Err("Microphone profiles need unique IDs and a name".into());
            }
            if p.source.starts_with("massiveeq") || p.source.contains('\0') {
                return Err("Select a physical microphone".into());
            }
            if p.channels != ["MONO"] && p.channels != ["FL", "FR"] {
                return Err("Microphone channels must be mono or stereo".into());
            }
            if p.advanced_project
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
            {
                return Err("A custom rack needs an absolute filename".into());
            }
            if p.filters.len() > MAX_INPUT_FILTERS {
                return Err("Microphone EQ supports up to 16 bands".into());
            }
            for f in &p.filters {
                if !matches!(
                    f.kind,
                    FilterKind::Peaking
                        | FilterKind::LowShelf
                        | FilterKind::HighShelf
                        | FilterKind::LowPass
                        | FilterKind::HighPass
                ) || f.channels != ChannelSelection::All
                    || f.bandwidth.is_some()
                    || !in_range(f.frequency, 20.0, 20_000.0)
                    || !in_range(f.gain_db, -24.0, 24.0)
                    || !in_range(f.q, 0.1, 20.0)
                {
                    return Err("Invalid microphone EQ band".into());
                }
            }
            if !in_range(p.gate.threshold_db, -80.0, 0.0)
                || !in_range(p.gate.attack_ms, 0.1, 100.0)
                || !in_range(p.gate.release_ms, 10.0, 1000.0)
                || !in_range(p.compressor.threshold_db, -60.0, 0.0)
                || !in_range(p.compressor.ratio, 1.0, 20.0)
                || !in_range(p.compressor.attack_ms, 0.1, 100.0)
                || !in_range(p.compressor.release_ms, 10.0, 1000.0)
                || !in_range(p.compressor.makeup_db, 0.0, 24.0)
            {
                return Err("Invalid microphone voice-processing setting".into());
            }
        }
        if !ids.contains(&self.active_profile) {
            return Err("The selected microphone profile is missing".into());
        }
        Ok(())
    }
}

fn mono_channels() -> Vec<String> {
    vec!["MONO".into()]
}

fn in_range(value: f64, min: f64, max: f64) -> bool {
    value.is_finite() && (min..=max).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_round_trip_separately_and_keep_disabled_voice_settings() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input.json");
        let mut library = InputLibrary::default();
        library.active_mut().gate.threshold_db = -38.5;
        library.save(&path).unwrap();
        let read = InputLibrary::load(&path).unwrap();
        assert_eq!(read.active_profile, library.active_profile);
        assert_eq!(read.active().gate.threshold_db, -38.5);
        assert!(!read.active().gate.enabled);
        assert!(!root.path().join("state.json").exists());
    }
    #[test]
    fn legacy_custom_rack_is_migrated_without_overwriting_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input.json");
        let text = r#"{"source":"mic","project":"/tmp/my-rack.carxp","channels":["MONO"]}"#;
        std::fs::write(&path, text).unwrap();
        let library = InputLibrary::load(&path).unwrap();
        assert_eq!(
            library.active().advanced_project.as_deref(),
            Some(Path::new("/tmp/my-rack.carxp"))
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        std::fs::write(&path, r#"{"source":"mic","project":"/tmp/my-rack.carxp"}"#).unwrap();
        assert_eq!(
            InputLibrary::load(&path).unwrap().active().channels,
            ["MONO"]
        );
    }
    #[test]
    fn invalid_or_partial_library_never_overwrites_saved_settings() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input.json");
        let mut library = InputLibrary::default();
        library.save(&path).unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        library.active_mut().compressor.ratio = f64::NAN;
        assert!(library.save(&path).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }
}
