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
    #[error("state schema {found} is newer than this build supports ({supported})")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("the final profile cannot be deleted; edit it or create another profile first")]
    LastProfile,
}

const LIBRARY_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub manual_trim_db: f64,
}

pub const COMPARISON_BYPASS_ID: &str = "__bypass__";
pub const MAX_COMPARISON_PROFILES: usize = 9;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComparisonSet {
    pub profile_ids: Vec<String>,
    pub active_profile_id: String,
    #[serde(default)]
    pub enabled: bool,
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
    #[serde(default)]
    pub comparison_sets: HashMap<String, ComparisonSet>,
    /// Per-output volume handling, by device storage key.
    #[serde(default)]
    pub volume: HashMap<String, crate::VolumeSettings>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            schema_version: LIBRARY_SCHEMA_VERSION,
            profiles: BTreeMap::new(),
            assignments: HashMap::new(),
            bypassed_devices: HashSet::new(),
            remembered_devices: HashMap::new(),
            global_bypass: false,
            comparison_sets: HashMap::new(),
            volume: HashMap::new(),
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
        if library.schema_version > LIBRARY_SCHEMA_VERSION {
            return Err(StorageError::UnsupportedSchema {
                found: library.schema_version,
                supported: LIBRARY_SCHEMA_VERSION,
            });
        }
        library.schema_version = LIBRARY_SCHEMA_VERSION;
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
        let mut profiles = library
            .profiles
            .values()
            .map(|record| self.get_profile(record))
            .collect::<Result<Vec<_>, _>>()?;
        crate::sort_profiles(&mut profiles);
        Ok(profiles)
    }

    pub fn get_profile(&self, record: &ProfileRecord) -> Result<ProfileInfo, StorageError> {
        let text = fs::read_to_string(self.profile_path(&record.id))?;
        let parsed = parse_text(record.name.clone(), &text);
        let impulse_layout = parsed.convolutions.first().and_then(|convolution| {
            let path = self.profile_dir(&record.id).join(&convolution.path);
            let (_, _, channels) = crate::pipewire::audio_file_info(&path)?;
            crate::ImpulseLayout::from_channels(channels)
        });
        Ok(ProfileInfo {
            id: record.id.clone(),
            name: record.name.clone(),
            text,
            manual_trim_db: record.manual_trim_db,
            activatable: parsed.is_activatable(),
            diagnostics: parsed.diagnostics,
            impulse_layout,
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
        if !manual_trim_db.is_finite() {
            return Err(StorageError::InvalidProfile(
                "manual trim must be a finite number".into(),
            ));
        }
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
        let convolution_count = parsed.convolutions.len();
        for convolution in &mut parsed.convolutions {
            let Some((frames, sample_rate, ir_channels)) =
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
            crate::validate_impulse(ir_channels, convolution.channels, convolution_count)
                .map_err(StorageError::InvalidProfile)?;
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
        let Some((frames, sample_rate, ir_channels)) = crate::pipewire::audio_file_info(source)
        else {
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
        // Multichannel IRs feed both ears, whatever channel was picked.
        let channels = match crate::ImpulseLayout::from_channels(ir_channels) {
            Some(layout) if layout.is_matrix() => ChannelSelection::All,
            _ => channels,
        };
        crate::validate_impulse(ir_channels, channels, 1).map_err(StorageError::InvalidProfile)?;

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
        // The IR replaces any previous one; EQ stays and is applied on top.
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
        if library.profiles.len() == 1 && library.profiles.contains_key(id) {
            return Err(StorageError::LastProfile);
        }
        if library.profiles.remove(id).is_none() {
            return Err(StorageError::MissingProfile(id.into()));
        }
        library.assignments.retain(|_, assigned| assigned != id);
        library.comparison_sets.retain(|_, comparison| {
            comparison.profile_ids.retain(|profile_id| profile_id != id);
            if comparison.active_profile_id == id {
                comparison.active_profile_id =
                    comparison.profile_ids.first().cloned().unwrap_or_default();
            }
            comparison.profile_ids.len() >= 2
        });
        let path = self.profile_dir(id);
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        self.save_library(library)
    }

    /// Save (or with `None`, remove) a measurement or target curve shown
    /// behind this profile's EQ. Replacing a curve also clears its provenance
    /// metadata, since it was declared for the old data. Returns the
    /// profile's curves afterwards.
    pub fn set_curve(
        &self,
        library: &Library,
        id: &str,
        kind: crate::CurveKind,
        source: Option<&Path>,
    ) -> Result<crate::Curves, StorageError> {
        if !library.profiles.contains_key(id) {
            return Err(StorageError::MissingProfile(id.into()));
        }
        let directory = self.profile_dir(id).join("curves");
        let destination = directory.join(kind.file_name());
        match source {
            Some(source) => {
                let text = fs::read_to_string(source)?;
                crate::parse_curve(&text, kind).map_err(StorageError::InvalidProfile)?;
                fs::create_dir_all(&directory)?;
                let sidecar = directory.join(kind.metadata_file_name());
                if sidecar.exists() {
                    fs::remove_file(sidecar)?;
                }
                atomic_write(&destination, text.as_bytes())?;
                // Placements belong to the measurement they were taken with.
                if kind == crate::CurveKind::Measurement {
                    remove_measurement_set(&directory)?;
                    remove_placements(&directory)?;
                }
            }
            None => {
                if destination.exists() {
                    fs::remove_file(&destination)?;
                }
                let sidecar = directory.join(kind.metadata_file_name());
                if sidecar.exists() {
                    fs::remove_file(sidecar)?;
                }
                if kind == crate::CurveKind::Measurement {
                    remove_measurement_set(&directory)?;
                    remove_placements(&directory)?;
                }
            }
        }
        Ok(self.curves(id))
    }

    /// Add another measurement of the same headphone, taken after putting it
    /// on again, or with `None` remove every placement. Needs a measurement.
    pub fn add_placement(
        &self,
        library: &Library,
        id: &str,
        source: Option<&Path>,
    ) -> Result<crate::Curves, StorageError> {
        if !library.profiles.contains_key(id) {
            return Err(StorageError::MissingProfile(id.into()));
        }
        let directory = self.profile_dir(id).join("curves");
        let Some(source) = source else {
            remove_placements(&directory)?;
            return Ok(self.curves(id));
        };
        if directory.join("measurements.json").exists() {
            return Err(StorageError::InvalidProfile(
                "Open Measurements to add or remove per-ear takes.".into(),
            ));
        }
        if !directory
            .join(crate::CurveKind::Measurement.file_name())
            .exists()
        {
            return Err(StorageError::InvalidProfile(
                "load a measurement before adding placements".into(),
            ));
        }
        let existing = placement_files(&directory);
        if existing.len() >= crate::MAX_PLACEMENTS {
            return Err(StorageError::InvalidProfile(format!(
                "a measurement can have at most {} placements",
                crate::MAX_PLACEMENTS
            )));
        }
        let text = fs::read_to_string(source)?;
        crate::parse_curve(&text, crate::CurveKind::Measurement)
            .map_err(StorageError::InvalidProfile)?;
        let next = existing
            .iter()
            .filter_map(|path| placement_number(path))
            .max()
            .unwrap_or(0)
            + 1;
        atomic_write(
            &directory.join(format!("placement-{next:02}.txt")),
            text.as_bytes(),
        )?;
        Ok(self.curves(id))
    }

    /// Replace the user-declared provenance of an existing curve. The curve
    /// must already be stored; metadata for a missing curve would be
    /// orphaned. Strings are trimmed and length-limited before saving.
    pub fn set_curve_metadata(
        &self,
        library: &Library,
        id: &str,
        kind: crate::CurveKind,
        metadata: crate::CurveMetadata,
    ) -> Result<crate::Curves, StorageError> {
        if !library.profiles.contains_key(id) {
            return Err(StorageError::MissingProfile(id.into()));
        }
        let directory = self.profile_dir(id).join("curves");
        if kind == crate::CurveKind::Measurement
            && let Some(mut stored) = read_measurement_set(&directory)
        {
            stored.metadata = Some(validate_curve_metadata(metadata)?);
            atomic_write(
                &directory.join("measurements.json"),
                &serde_json::to_vec(&stored)?,
            )?;
            return Ok(self.curves(id));
        }
        if !directory.join(kind.file_name()).exists() {
            return Err(StorageError::InvalidProfile(format!(
                "no {kind:?} curve is stored for this profile"
            )));
        }
        let metadata = validate_curve_metadata(metadata)?;
        atomic_write(
            &directory.join(kind.metadata_file_name()),
            &serde_json::to_vec_pretty(&metadata)?,
        )?;
        Ok(self.curves(id))
    }

    /// The curves saved with a profile; unreadable files are left out. A
    /// missing or malformed metadata sidecar never hides a valid curve.
    pub fn curves(&self, id: &str) -> crate::Curves {
        let directory = self.profile_dir(id).join("curves");
        let read = |kind: crate::CurveKind| {
            crate::parse_curve(
                &fs::read_to_string(directory.join(kind.file_name())).ok()?,
                kind,
            )
            .ok()
        };
        let read_metadata = |kind: crate::CurveKind| {
            serde_json::from_slice(&fs::read(directory.join(kind.metadata_file_name())).ok()?).ok()
        };
        let measurement = read(crate::CurveKind::Measurement);
        let placements = if measurement.is_some() {
            placement_files(&directory)
                .iter()
                .filter_map(|path| {
                    crate::parse_curve(
                        &fs::read_to_string(path).ok()?,
                        crate::CurveKind::Measurement,
                    )
                    .ok()
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut curves = crate::Curves {
            measurement,
            target: read(crate::CurveKind::Target),
            measurement_metadata: read_metadata(crate::CurveKind::Measurement),
            target_metadata: read_metadata(crate::CurveKind::Target),
            placements,
            measurement_set: None,
            measurement_channels: None,
        };
        if let Some(stored) = read_measurement_set(&directory)
            && let Ok(averages) = stored.set.averages()
        {
            curves.measurement = Some(averages.merged());
            curves.measurement_channels =
                (stored.set.output == crate::MeasurementOutput::Separate).then_some(averages);
            curves.measurement_metadata = stored.metadata;
            curves.measurement_set = Some(stored.set);
            curves.placements.clear();
        }
        curves
    }

    /// Save original takes and options together. The derived average never
    /// replaces its inputs; an invalid request leaves existing curves intact.
    pub fn set_measurements(
        &self,
        library: &Library,
        id: &str,
        set: crate::MeasurementSet,
    ) -> Result<crate::Curves, StorageError> {
        if !library.profiles.contains_key(id) {
            return Err(StorageError::MissingProfile(id.into()));
        }
        set.averages().map_err(StorageError::InvalidProfile)?;
        let previous = self.curves(id);
        let unchanged = previous
            .measurement_set
            .as_ref()
            .is_some_and(|old| old.takes == set.takes)
            || (previous.measurement_set.is_none()
                && previous.measurement.as_ref().is_some_and(|measurement| {
                    let old = std::iter::once(measurement)
                        .chain(&previous.placements)
                        .collect::<Vec<_>>();
                    old.len() == set.takes.len()
                        && old.iter().zip(&set.takes).all(|(points, take)| {
                            take.ear == crate::MeasurementEar::Both && **points == take.points
                        })
                }));
        let stored = StoredMeasurementSet {
            set,
            metadata: if unchanged {
                previous.measurement_metadata
            } else {
                None
            },
        };
        let directory = self.profile_dir(id).join("curves");
        atomic_write(
            &directory.join("measurements.json"),
            &serde_json::to_vec(&stored)?,
        )?;
        Ok(self.curves(id))
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
            crate::validate_impulse(channels, convolution.channels, profile.convolutions.len())
                .map_err(StorageError::InvalidProfile)?;
        }
        Ok(())
    }
}

const MAX_METADATA_SOURCE_LEN: usize = 2048;
const MAX_METADATA_FIELD_LEN: usize = 256;

/// Trim metadata strings and enforce their length bounds.
/// A profile's placement files, oldest first.
fn placement_files(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut files = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| placement_number(path).is_some())
        .collect::<Vec<_>>();
    files.sort_by_key(|path| placement_number(path));
    files
}

fn placement_number(path: &Path) -> Option<u32> {
    path.file_name()?
        .to_str()?
        .strip_prefix("placement-")?
        .strip_suffix(".txt")?
        .parse()
        .ok()
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredMeasurementSet {
    set: crate::MeasurementSet,
    metadata: Option<crate::CurveMetadata>,
}

fn read_measurement_set(directory: &Path) -> Option<StoredMeasurementSet> {
    let stored: StoredMeasurementSet =
        serde_json::from_slice(&fs::read(directory.join("measurements.json")).ok()?).ok()?;
    stored.set.averages().ok()?;
    Some(stored)
}

fn remove_measurement_set(directory: &Path) -> Result<(), StorageError> {
    let path = directory.join("measurements.json");
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn remove_placements(directory: &Path) -> Result<(), StorageError> {
    for path in placement_files(directory) {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn validate_curve_metadata(
    metadata: crate::CurveMetadata,
) -> Result<crate::CurveMetadata, StorageError> {
    let metadata = crate::CurveMetadata {
        source: metadata.source.trim().to_owned(),
        fixture: metadata.fixture.trim().to_owned(),
        baseline: metadata.baseline.trim().to_owned(),
        ..metadata
    };
    let check = |field: &str, value: &str, limit: usize| {
        if value.chars().count() > limit {
            Err(StorageError::InvalidProfile(format!(
                "curve metadata {field} is limited to {limit} characters"
            )))
        } else {
            Ok(())
        }
    };
    check("source", &metadata.source, MAX_METADATA_SOURCE_LEN)?;
    check("fixture", &metadata.fixture, MAX_METADATA_FIELD_LEN)?;
    check("baseline", &metadata.baseline, MAX_METADATA_FIELD_LEN)?;
    Ok(metadata)
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

    #[test]
    fn retained_measurements_round_trip_and_do_not_change_eq_or_target() {
        use crate::{
            CurveKind, MeasurementEar, MeasurementOutput, MeasurementSet, MeasurementTake,
        };
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "Takes").unwrap();
        let source = temp.path().join("target.txt");
        fs::write(
            &source,
            (0..30)
                .map(|i| format!("{} 0\n", 20.0 * 1000.0_f64.powf(i as f64 / 29.0)))
                .collect::<String>(),
        )
        .unwrap();
        let before = storage
            .set_curve(&library, &profile.id, CurveKind::Target, Some(&source))
            .unwrap();
        let points = crate::parse_curve(
            &fs::read_to_string(&source).unwrap(),
            CurveKind::Measurement,
        )
        .unwrap();
        let mut set = MeasurementSet {
            takes: vec![
                MeasurementTake {
                    name: "Left.txt".into(),
                    ear: MeasurementEar::Left,
                    points: points.clone(),
                },
                MeasurementTake {
                    name: "Right.txt".into(),
                    ear: MeasurementEar::Right,
                    points,
                },
            ],
            ..Default::default()
        };
        let curves = storage
            .set_measurements(&library, &profile.id, set.clone())
            .unwrap();
        assert_eq!(curves.target, before.target);
        assert_eq!(curves.measurement_set, Some(set.clone()));
        assert!(curves.measurement_channels.is_some());
        assert_eq!(
            storage
                .get_profile(library.profiles.get(&profile.id).unwrap())
                .unwrap()
                .text,
            profile.text
        );
        let metadata = crate::CurveMetadata {
            source: "My measurements".into(),
            ..Default::default()
        };
        storage
            .set_curve_metadata(
                &library,
                &profile.id,
                CurveKind::Measurement,
                metadata.clone(),
            )
            .unwrap();
        set.output = MeasurementOutput::Merged;
        let curves = storage
            .set_measurements(&library, &profile.id, set.clone())
            .unwrap();
        assert!(curves.measurement_channels.is_none());
        assert_eq!(curves.measurement_metadata, Some(metadata));
        assert_eq!(curves.measurement_set.as_ref().unwrap().takes.len(), 2);
        let saved = fs::read(
            storage
                .profile_dir(&profile.id)
                .join("curves/measurements.json"),
        )
        .unwrap();
        let mut invalid = set.clone();
        invalid.takes.clear();
        assert!(
            storage
                .set_measurements(&library, &profile.id, invalid)
                .is_err()
        );
        assert_eq!(
            fs::read(
                storage
                    .profile_dir(&profile.id)
                    .join("curves/measurements.json")
            )
            .unwrap(),
            saved
        );
        set.takes[0].points[0].low = 1.0;
        set.takes[0].points[0].high = 1.0;
        assert!(
            storage
                .set_measurements(&library, &profile.id, set)
                .unwrap()
                .measurement_metadata
                .is_none()
        );
        assert!(
            storage
                .add_placement(&library, &profile.id, Some(&source))
                .is_err()
        );
        let restarted = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        assert_eq!(
            restarted
                .curves(&profile.id)
                .measurement_set
                .unwrap()
                .takes
                .len(),
            2
        );
        let replaced = storage
            .set_curve(&library, &profile.id, CurveKind::Measurement, Some(&source))
            .unwrap();
        assert!(replaced.measurement_set.is_none());
        storage
            .set_curve(&library, &profile.id, CurveKind::Measurement, None)
            .unwrap();
        assert!(storage.curves(&profile.id).measurement.is_none());
        assert!(storage.curves(&profile.id).target.is_some());
    }

    fn write_test_wav(path: &Path) {
        write_test_wav_channels(path, 1);
    }

    fn write_test_wav_channels(path: &Path, channels: u16) {
        let samples = vec![0_i16; 48 * channels as usize];
        let data_size = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(96_000 * channels as u32).to_le_bytes());
        bytes.extend_from_slice(&(2 * channels).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn write_test_curve_source(path: &Path) {
        let text = (0..30)
            .map(|i| format!("{},{}\n", 20 * (i + 1) * (i + 1), i))
            .collect::<String>();
        std::fs::write(path, format!("frequency,raw\n{text}")).unwrap();
    }

    fn declared_metadata() -> crate::CurveMetadata {
        crate::CurveMetadata {
            source: "  AutoEq export  ".into(),
            fixture: "B&K 5128".into(),
            compensation: crate::CurveCompensation::DiffuseField,
            baseline: "Harman 2018".into(),
            transducer: crate::CurveTransducer::OverEar,
        }
    }

    #[test]
    fn curve_metadata_roundtrips_and_is_trimmed() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "HD 650").unwrap();
        let source = temp.path().join("hd650.csv");
        write_test_curve_source(&source);
        storage
            .set_curve(
                &library,
                &profile.id,
                crate::CurveKind::Measurement,
                Some(&source),
            )
            .unwrap();

        let curves = storage
            .set_curve_metadata(
                &library,
                &profile.id,
                crate::CurveKind::Measurement,
                declared_metadata(),
            )
            .unwrap();
        let metadata = curves.measurement_metadata.unwrap();
        assert_eq!(metadata.source, "AutoEq export");
        assert_eq!(
            metadata.compensation,
            crate::CurveCompensation::DiffuseField
        );
        assert_eq!(metadata.transducer, crate::CurveTransducer::OverEar);

        let reloaded = storage.curves(&profile.id);
        assert_eq!(reloaded.measurement_metadata, Some(metadata));
        assert!(reloaded.target_metadata.is_none());
    }

    #[test]
    fn curves_without_metadata_sidecars_stay_loadable() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "HD 650").unwrap();
        let source = temp.path().join("hd650.csv");
        write_test_curve_source(&source);
        let curves = storage
            .set_curve(
                &library,
                &profile.id,
                crate::CurveKind::Measurement,
                Some(&source),
            )
            .unwrap();
        assert!(curves.measurement.is_some());
        assert!(curves.measurement_metadata.is_none());
    }

    #[test]
    fn malformed_metadata_sidecar_does_not_hide_the_curve() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "HD 650").unwrap();
        let source = temp.path().join("hd650.csv");
        write_test_curve_source(&source);
        storage
            .set_curve(
                &library,
                &profile.id,
                crate::CurveKind::Measurement,
                Some(&source),
            )
            .unwrap();
        std::fs::write(
            storage
                .profile_dir(&profile.id)
                .join("curves")
                .join(crate::CurveKind::Measurement.metadata_file_name()),
            "{ not valid json",
        )
        .unwrap();
        let curves = storage.curves(&profile.id);
        assert_eq!(curves.measurement.unwrap().len(), 30);
        assert!(curves.measurement_metadata.is_none());
    }

    #[test]
    fn replacing_or_removing_a_curve_clears_its_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "HD 650").unwrap();
        let source = temp.path().join("hd650.csv");
        write_test_curve_source(&source);
        storage
            .set_curve(
                &library,
                &profile.id,
                crate::CurveKind::Measurement,
                Some(&source),
            )
            .unwrap();
        storage
            .set_curve_metadata(
                &library,
                &profile.id,
                crate::CurveKind::Measurement,
                declared_metadata(),
            )
            .unwrap();
        let sidecar = storage
            .profile_dir(&profile.id)
            .join("curves")
            .join(crate::CurveKind::Measurement.metadata_file_name());
        assert!(sidecar.exists());

        // Replacing the curve drops the old provenance.
        let replacement = temp.path().join("other.csv");
        write_test_curve_source(&replacement);
        let curves = storage
            .set_curve(
                &library,
                &profile.id,
                crate::CurveKind::Measurement,
                Some(&replacement),
            )
            .unwrap();
        assert!(curves.measurement.is_some());
        assert!(curves.measurement_metadata.is_none());
        assert!(!sidecar.exists());

        // Removing the curve removes the sidecar too.
        storage
            .set_curve_metadata(
                &library,
                &profile.id,
                crate::CurveKind::Measurement,
                declared_metadata(),
            )
            .unwrap();
        assert!(sidecar.exists());
        let curves = storage
            .set_curve(&library, &profile.id, crate::CurveKind::Measurement, None)
            .unwrap();
        assert!(curves.measurement.is_none());
        assert!(curves.measurement_metadata.is_none());
        assert!(!sidecar.exists());
    }

    #[test]
    fn curve_metadata_requires_a_known_profile_an_existing_curve_and_bounded_strings() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "HD 650").unwrap();

        assert!(matches!(
            storage.set_curve_metadata(
                &library,
                "no-such-profile",
                crate::CurveKind::Measurement,
                declared_metadata()
            ),
            Err(StorageError::MissingProfile(_))
        ));
        // No curve stored yet.
        assert!(matches!(
            storage.set_curve_metadata(
                &library,
                &profile.id,
                crate::CurveKind::Target,
                declared_metadata()
            ),
            Err(StorageError::InvalidProfile(_))
        ));

        let source = temp.path().join("hd650.csv");
        write_test_curve_source(&source);
        storage
            .set_curve(
                &library,
                &profile.id,
                crate::CurveKind::Target,
                Some(&source),
            )
            .unwrap();
        let mut long_source = declared_metadata();
        long_source.source = "x".repeat(2049);
        assert!(matches!(
            storage.set_curve_metadata(
                &library,
                &profile.id,
                crate::CurveKind::Target,
                long_source
            ),
            Err(StorageError::InvalidProfile(_))
        ));
        let mut long_fixture = declared_metadata();
        long_fixture.fixture = "y".repeat(257);
        assert!(matches!(
            storage.set_curve_metadata(
                &library,
                &profile.id,
                crate::CurveKind::Target,
                long_fixture
            ),
            Err(StorageError::InvalidProfile(_))
        ));
        // Bounds are inclusive.
        let mut at_bounds = declared_metadata();
        at_bounds.source = "x".repeat(2048);
        at_bounds.fixture = "y".repeat(256);
        at_bounds.baseline = "z".repeat(256);
        storage
            .set_curve_metadata(&library, &profile.id, crate::CurveKind::Target, at_bounds)
            .unwrap();
    }

    #[test]
    fn curves_are_saved_with_the_profile_and_can_be_cleared() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "HD 650").unwrap();
        let source = temp.path().join("hd650.csv");
        let text = (0..30)
            .map(|i| format!("{},{}\n", 20 * (i + 1) * (i + 1), i))
            .collect::<String>();
        std::fs::write(&source, format!("frequency,raw\n{text}")).unwrap();
        let curves = storage
            .set_curve(
                &library,
                &profile.id,
                crate::CurveKind::Measurement,
                Some(&source),
            )
            .unwrap();
        assert_eq!(curves.measurement.unwrap().len(), 30);
        assert!(curves.target.is_none());
        let curves = storage
            .set_curve(&library, &profile.id, crate::CurveKind::Measurement, None)
            .unwrap();
        assert!(curves.measurement.is_none());
        std::fs::write(&source, "not a curve").unwrap();
        assert!(matches!(
            storage.set_curve(
                &library,
                &profile.id,
                crate::CurveKind::Target,
                Some(&source)
            ),
            Err(StorageError::InvalidProfile(_))
        ));
    }

    #[test]
    fn placements_follow_their_measurement() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "HD 600").unwrap();
        let source = temp.path().join("take.csv");
        write_test_curve_source(&source);
        // Placements need a measurement to belong to.
        assert!(
            storage
                .add_placement(&library, &profile.id, Some(&source))
                .is_err()
        );
        let measurement = crate::CurveKind::Measurement;
        storage
            .set_curve(&library, &profile.id, measurement, Some(&source))
            .unwrap();
        for _ in 0..2 {
            storage
                .add_placement(&library, &profile.id, Some(&source))
                .unwrap();
        }
        assert_eq!(storage.curves(&profile.id).placements.len(), 2);
        for _ in 2..crate::MAX_PLACEMENTS {
            storage
                .add_placement(&library, &profile.id, Some(&source))
                .unwrap();
        }
        assert!(
            storage
                .add_placement(&library, &profile.id, Some(&source))
                .is_err()
        );
        // A new measurement is a new headphone or session: placements go.
        let curves = storage
            .set_curve(&library, &profile.id, measurement, Some(&source))
            .unwrap();
        assert!(curves.placements.is_empty());
        storage
            .add_placement(&library, &profile.id, Some(&source))
            .unwrap();
        let curves = storage.add_placement(&library, &profile.id, None).unwrap();
        assert!(curves.placements.is_empty() && curves.measurement.is_some());
    }

    #[test]
    fn surround_impulse_feeds_both_ears_and_reports_its_layout() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "BRIR").unwrap();
        let source = temp.path().join("hesuvi.wav");
        write_test_wav_channels(&source, 14);
        let updated = storage
            .set_convolution(&library, &profile.id, &source, ChannelSelection::Left)
            .unwrap();
        assert_eq!(
            updated.impulse_layout,
            Some(crate::ImpulseLayout::Surround71)
        );
        let parsed = parse_text(&updated.name, &updated.text);
        assert_eq!(parsed.convolutions[0].channels, ChannelSelection::All);

        let odd = temp.path().join("six.wav");
        write_test_wav_channels(&odd, 6);
        assert!(matches!(
            storage.set_convolution(&library, &profile.id, &odd, ChannelSelection::All),
            Err(StorageError::InvalidProfile(_))
        ));
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
        assert_eq!(library.schema_version, 3);
        assert!(library.remembered_devices.is_empty());
        assert!(library.comparison_sets.is_empty());
    }

    #[test]
    fn newer_state_schema_is_rejected_without_rewriting_it() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        let state = config.join("state.json");
        let original = br#"{"schema_version":999,"profiles":{},"assignments":{},"bypassed_devices":[],"global_bypass":false}"#;
        std::fs::write(&state, original).unwrap();
        let storage = Storage::new(temp.path().join("data"), config).unwrap();

        assert!(matches!(
            storage.load_library(),
            Err(StorageError::UnsupportedSchema { found: 999, .. })
        ));
        assert_eq!(std::fs::read(state).unwrap(), original);
    }

    #[test]
    fn final_profile_and_non_finite_trim_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = Library::default();
        let profile = storage
            .create_profile(&mut library, "Only profile")
            .unwrap();

        assert!(matches!(
            storage.delete_profile(&mut library, &profile.id),
            Err(StorageError::LastProfile)
        ));
        assert!(library.profiles.contains_key(&profile.id));
        assert!(matches!(
            storage.put_profile(&mut library, &profile.id, "Only profile", "", f64::NAN),
            Err(StorageError::InvalidProfile(_))
        ));
        assert_eq!(library.profiles[&profile.id].manual_trim_db, 0.0);
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
    fn convolution_copies_asset_and_keeps_parametric_filters() {
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
        assert_eq!(parsed.filters.len(), 1);
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
