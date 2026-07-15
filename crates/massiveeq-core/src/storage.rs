use crate::{
    ChannelSelection, Convolution, ProfileDocument, ProfileInfo, RememberedDevice, parse_file,
    parse_text, serialize_profile,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Parse(#[from] crate::ParseError),
    #[error("profile {0} does not exist")]
    MissingProfile(String),
    #[error("profile cannot be activated: {0}")]
    InvalidProfile(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub manual_trim_db: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub schema_version: u32,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileRecord>,
    #[serde(default)]
    pub assignments: HashMap<String, String>,
    #[serde(default)]
    pub bypassed_devices: HashSet<String>,
    #[serde(default)]
    pub remembered_devices: HashMap<String, RememberedDevice>,
    #[serde(default)]
    pub global_bypass: bool,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            schema_version: 2,
            profiles: BTreeMap::new(),
            assignments: HashMap::new(),
            bypassed_devices: HashSet::new(),
            remembered_devices: HashMap::new(),
            global_bypass: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Storage {
    data_dir: PathBuf,
    state_path: PathBuf,
}

impl Storage {
    pub fn discover() -> Result<Self, StorageError> {
        let data = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("massiveeq");
        let config = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("massiveeq");
        Self::new(data, config)
    }

    pub fn new(data_dir: PathBuf, config_dir: PathBuf) -> Result<Self, StorageError> {
        fs::create_dir_all(data_dir.join("profiles"))?;
        fs::create_dir_all(&config_dir)?;
        Ok(Self {
            data_dir,
            state_path: config_dir.join("state.json"),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn load_library(&self) -> Result<Library, StorageError> {
        if !self.state_path.exists() {
            return Ok(Library::default());
        }
        let mut library: Library = serde_json::from_slice(&fs::read(&self.state_path)?)?;
        library.schema_version = 2;
        Ok(library)
    }

    pub fn save_library(&self, library: &Library) -> Result<(), StorageError> {
        atomic_write(&self.state_path, &serde_json::to_vec_pretty(library)?)
    }

    pub fn profile_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("profiles").join(id)
    }
    pub fn profile_path(&self, id: &str) -> PathBuf {
        self.profile_dir(id).join("profile.txt")
    }

    pub fn list_profiles(&self, library: &Library) -> Result<Vec<ProfileInfo>, StorageError> {
        library
            .profiles
            .values()
            .map(|record| self.get_profile(record))
            .collect()
    }

    pub fn get_profile(&self, record: &ProfileRecord) -> Result<ProfileInfo, StorageError> {
        let text = fs::read_to_string(self.profile_path(&record.id))?;
        let parsed = parse_text(record.name.clone(), &text);
        Ok(ProfileInfo {
            id: record.id.clone(),
            name: record.name.clone(),
            text,
            manual_trim_db: record.manual_trim_db,
            activatable: parsed.is_activatable(),
            diagnostics: parsed.diagnostics,
        })
    }

    pub fn create_profile(
        &self,
        library: &mut Library,
        name: &str,
    ) -> Result<ProfileInfo, StorageError> {
        let id = Uuid::new_v4().to_string();
        let record = ProfileRecord {
            id: id.clone(),
            name: unique_name(library, name),
            manual_trim_db: 0.0,
        };
        fs::create_dir_all(self.profile_dir(&id).join("assets"))?;
        let profile = ProfileDocument::empty(record.name.clone());
        atomic_write(
            &self.profile_path(&id),
            serialize_profile(&profile).as_bytes(),
        )?;
        library.profiles.insert(id.clone(), record.clone());
        self.save_library(library)?;
        self.get_profile(&record)
    }

    pub fn put_profile(
        &self,
        library: &mut Library,
        id: &str,
        name: &str,
        text: &str,
        manual_trim_db: f64,
    ) -> Result<ProfileInfo, StorageError> {
        let parsed = parse_text(name, text);
        if !parsed.is_activatable() {
            let message = parsed
                .diagnostics
                .iter()
                .map(|d| format!("{}:{}: {}", d.source, d.line, d.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(StorageError::InvalidProfile(message));
        }
        self.validate_stored_convolutions(id, &parsed)?;
        let record = library
            .profiles
            .get_mut(id)
            .ok_or_else(|| StorageError::MissingProfile(id.into()))?;
        record.name = name.trim().to_owned();
        record.manual_trim_db = manual_trim_db.clamp(-24.0, 24.0);
        atomic_write(&self.profile_path(id), text.as_bytes())?;
        let record = record.clone();
        self.save_library(library)?;
        self.get_profile(&record)
    }

    pub fn import_profile(
        &self,
        library: &mut Library,
        source: &Path,
    ) -> Result<ProfileInfo, StorageError> {
        let mut parsed = parse_file(source)?;
        if !parsed.is_activatable() {
            let message = parsed
                .diagnostics
                .iter()
                .filter(|d| matches!(d.level, crate::DiagnosticLevel::Error))
                .map(|d| format!("{}:{}: {}", d.source, d.line, d.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(StorageError::InvalidProfile(message));
        }
        let id = Uuid::new_v4().to_string();
        parsed.name = unique_name(library, &parsed.name);
        let profile_dir = self.profile_dir(&id);
        let asset_dir = profile_dir.join("assets");
        fs::create_dir_all(&asset_dir)?;
        for convolution in &mut parsed.convolutions {
            let Some((frames, sample_rate, _)) =
                crate::pipewire::audio_file_info(&convolution.path)
            else {
                return Err(StorageError::InvalidProfile(format!(
                    "Could not read convolution file {} with libsndfile",
                    convolution.path.display()
                )));
            };
            if frames as f64 / sample_rate as f64 > 10.0 {
                return Err(StorageError::InvalidProfile(format!(
                    "Convolution file {} is longer than the 10 second version 1 limit",
                    convolution.path.display()
                )));
            }
            let file_name = convolution
                .path
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("impulse.wav");
            let destination = unique_asset_path(&asset_dir, file_name);
            fs::copy(&convolution.path, &destination)?;
            convolution.path =
                PathBuf::from("assets").join(destination.file_name().expect("asset filename"));
        }
        atomic_write(
            &profile_dir.join("profile.txt"),
            serialize_profile(&parsed).as_bytes(),
        )?;
        let record = ProfileRecord {
            id: id.clone(),
            name: parsed.name.clone(),
            manual_trim_db: 0.0,
        };
        library.profiles.insert(id.clone(), record.clone());
        self.save_library(library)?;
        self.get_profile(&record)
    }

    pub fn set_convolution(
        &self,
        library: &Library,
        id: &str,
        source: &Path,
        channels: ChannelSelection,
    ) -> Result<ProfileInfo, StorageError> {
        let record = library
            .profiles
            .get(id)
            .ok_or_else(|| StorageError::MissingProfile(id.into()))?;
        let Some((frames, sample_rate, _)) = crate::pipewire::audio_file_info(source) else {
            return Err(StorageError::InvalidProfile(format!(
                "Could not read convolution file {} with libsndfile",
                source.display()
            )));
        };
        if frames as f64 / sample_rate as f64 > 10.0 {
            return Err(StorageError::InvalidProfile(
                "Convolution files are limited to 10 seconds".into(),
            ));
        }

        let asset_dir = self.profile_dir(id).join("assets");
        fs::create_dir_all(&asset_dir)?;
        let file_name = source
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("impulse.wav");
        let destination = unique_asset_path(&asset_dir, file_name);
        fs::copy(source, &destination)?;

        let text = fs::read_to_string(self.profile_path(id))?;
        let mut document = parse_text(&record.name, &text);
        document.filters.clear();
        document.graphic_eqs.clear();
        document.convolutions.clear();
        document.convolutions.push(Convolution {
            channels,
            path: PathBuf::from("assets").join(destination.file_name().expect("asset filename")),
        });
        atomic_write(
            &self.profile_path(id),
            serialize_profile(&document).as_bytes(),
        )?;
        self.get_profile(record)
    }

    pub fn delete_profile(&self, library: &mut Library, id: &str) -> Result<(), StorageError> {
        if library.profiles.remove(id).is_none() {
            return Err(StorageError::MissingProfile(id.into()));
        }
        library.assignments.retain(|_, assigned| assigned != id);
        let path = self.profile_dir(id);
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        self.save_library(library)
    }

    pub fn export_profile(
        &self,
        library: &Library,
        id: &str,
        destination: &Path,
    ) -> Result<(), StorageError> {
        if !library.profiles.contains_key(id) {
            return Err(StorageError::MissingProfile(id.into()));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(self.profile_path(id), destination)?;
        let source_assets = self.profile_dir(id).join("assets");
        if source_assets.exists() {
            let destination_assets = destination
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("assets");
            fs::create_dir_all(&destination_assets)?;
            for entry in fs::read_dir(source_assets)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    fs::copy(entry.path(), destination_assets.join(entry.file_name()))?;
                }
            }
        }
        Ok(())
    }

    pub fn parsed_profile(
        &self,
        library: &Library,
        id: &str,
    ) -> Result<(ProfileDocument, f64), StorageError> {
        let record = library
            .profiles
            .get(id)
            .ok_or_else(|| StorageError::MissingProfile(id.into()))?;
        let text = fs::read_to_string(self.profile_path(id))?;
        Ok((parse_text(&record.name, &text), record.manual_trim_db))
    }

    fn validate_stored_convolutions(
        &self,
        id: &str,
        profile: &ProfileDocument,
    ) -> Result<(), StorageError> {
        if profile.convolutions.is_empty() {
            return Ok(());
        }
        let root = fs::canonicalize(self.profile_dir(id))?;
        for convolution in &profile.convolutions {
            if convolution.path.is_absolute() {
                return Err(StorageError::InvalidProfile(format!(
                    "Convolution asset must use a portable path inside the profile: {}",
                    convolution.path.display()
                )));
            }
            let requested = self.profile_dir(id).join(&convolution.path);
            let canonical = fs::canonicalize(&requested).map_err(|_| {
                StorageError::InvalidProfile(format!(
                    "Convolution asset does not exist: {}",
                    convolution.path.display()
                ))
            })?;
            if !canonical.starts_with(&root) {
                return Err(StorageError::InvalidProfile(format!(
                    "Convolution asset escapes the profile directory: {}",
                    convolution.path.display()
                )));
            }
            let Some((frames, sample_rate, channels)) =
                crate::pipewire::audio_file_info(&canonical)
            else {
                return Err(StorageError::InvalidProfile(format!(
                    "Could not decode convolution asset {}",
                    convolution.path.display()
                )));
            };
            if frames <= 0 || sample_rate <= 0 || channels == 0 {
                return Err(StorageError::InvalidProfile(format!(
                    "Convolution asset is empty: {}",
                    convolution.path.display()
                )));
            }
            if frames as f64 / sample_rate as f64 > 10.0 {
                return Err(StorageError::InvalidProfile(format!(
                    "Convolution asset is longer than 10 seconds: {}",
                    convolution.path.display()
                )));
            }
        }
        Ok(())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| StorageError::Io(error.error))?;
    Ok(())
}

fn unique_name(library: &Library, requested: &str) -> String {
    let base = if requested.trim().is_empty() {
        "Untitled Profile"
    } else {
        requested.trim()
    };
    if !library.profiles.values().any(|p| p.name == base) {
        return base.to_owned();
    }
    for number in 2.. {
        let candidate = format!("{base} {number}");
        if !library.profiles.values().any(|p| p.name == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

fn unique_asset_path(dir: &Path, file_name: &str) -> PathBuf {
    let requested = dir.join(file_name);
    if !requested.exists() {
        return requested;
    }
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("impulse");
    let extension = Path::new(file_name)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("wav");
    for number in 2.. {
        let candidate = dir.join(format!("{stem}-{number}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_wav(path: &Path) {
        let samples = [0_i16; 48];
        let data_size = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn profile_roundtrip_and_atomic_update() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "AirPods").unwrap();
        let updated = storage
            .put_profile(
                &mut library,
                &profile.id,
                "AirPods",
                "Preamp: -6 dB\nFilter: ON PK Fc 1000 Hz Gain 4 dB Q 1",
                0.5,
            )
            .unwrap();
        assert!(updated.text.contains("1000"));
        assert_eq!(storage.load_library().unwrap().profiles.len(), 1);
    }

    #[test]
    fn older_state_migrates_with_an_empty_remembered_device_registry() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("state.json"),
            r#"{"schema_version":1,"profiles":{},"assignments":{},"bypassed_devices":[],"global_bypass":false}"#,
        )
        .unwrap();
        let storage = Storage::new(temp.path().join("data"), config).unwrap();
        let library = storage.load_library().unwrap();
        assert_eq!(library.schema_version, 2);
        assert!(library.remembered_devices.is_empty());
    }

    #[test]
    fn export_copies_profile_and_assets() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "Convolution").unwrap();
        std::fs::write(
            storage.profile_dir(&profile.id).join("assets/left.wav"),
            b"fixture",
        )
        .unwrap();
        let destination = temp.path().join("export/profile.txt");
        storage
            .export_profile(&library, &profile.id, &destination)
            .unwrap();
        assert!(destination.exists());
        assert!(temp.path().join("export/assets/left.wav").exists());
    }

    #[test]
    fn convolution_mode_copies_asset_and_replaces_parametric_filters() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "IR").unwrap();
        storage
            .put_profile(
                &mut library,
                &profile.id,
                "IR",
                "Filter: ON PK Fc 1000 Hz Gain 3 dB Q 1",
                0.0,
            )
            .unwrap();
        let source = temp.path().join("room.wav");
        write_test_wav(&source);
        let updated = storage
            .set_convolution(&library, &profile.id, &source, ChannelSelection::All)
            .unwrap();
        let parsed = parse_text(&updated.name, &updated.text);
        assert!(parsed.filters.is_empty());
        assert_eq!(parsed.convolutions.len(), 1);
        assert!(parsed.convolutions[0].path.starts_with("assets"));
        assert!(
            storage
                .profile_dir(&profile.id)
                .join(&parsed.convolutions[0].path)
                .exists()
        );
    }

    #[test]
    fn raw_profile_update_rejects_missing_or_external_convolution_assets() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "IR").unwrap();
        assert!(
            storage
                .put_profile(
                    &mut library,
                    &profile.id,
                    "IR",
                    "Convolution: assets/missing.wav",
                    0.0,
                )
                .is_err()
        );
        assert!(
            storage
                .put_profile(
                    &mut library,
                    &profile.id,
                    "IR",
                    "Convolution: /tmp/external.wav",
                    0.0,
                )
                .is_err()
        );
    }
}
