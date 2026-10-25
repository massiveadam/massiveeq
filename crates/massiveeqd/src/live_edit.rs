//! Expiring, in-memory edits. Saving is independent of auditioning a draft.
use anyhow::{Context, Result, ensure};
use massiveeq_core::{Library, ProfileDocument, Storage, parse_text};
use serde::Deserialize;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub const LIVE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Deserialize)]
pub struct Reference {
    pub text: String,
    pub trim: f64,
}

#[derive(Debug, Deserialize)]
pub struct Request {
    pub text: String,
    pub trim: f64,
    pub reference: Option<Reference>,
    #[serde(default)]
    pub hear_reference: bool,
    /// Output gain to keep while the tone generator plays.
    #[serde(default)]
    pub hold_gain_db: Option<f64>,
}

pub type ReferenceCache =
    std::sync::Arc<std::sync::Mutex<HashMap<(u32, u32, u32), massiveeq_dsp::CompiledProfile>>>;

#[derive(Clone)]
pub struct Session {
    /// Unique D-Bus connection that owns this preview and its saves.
    pub owner: String,
    pub draft: ProfileDocument,
    pub trim: f64,
    pub reference: Option<(ProfileDocument, f64)>,
    pub hear_reference: bool,
    pub hold_gain_db: Option<f64>,
    pub renewed: Instant,
    pub reference_cache: ReferenceCache,
}

pub type Sessions = HashMap<String, Session>;

impl Session {
    pub fn parse(
        storage: &Storage,
        library: &Library,
        id: &str,
        owner: &str,
        request: Request,
    ) -> Result<Self> {
        let (saved, _) = storage.parsed_profile(library, id)?;
        let parse = |text: &str, trim: f64| -> Result<ProfileDocument> {
            ensure!(text.len() <= 1_000_000, "profile draft is too large");
            ensure!(
                trim.is_finite() && (-24.0..=24.0).contains(&trim),
                "invalid manual trim"
            );
            let draft = parse_text(&saved.name, text);
            ensure!(draft.is_activatable(), "profile draft contains errors");
            // Live EQ cannot introduce or replace assets. The normal import
            // path validates those; compilation also confines files to the profile.
            ensure!(
                draft.convolutions == saved.convolutions,
                "load or remove impulse responses before live editing"
            );
            Ok(draft)
        };
        let draft = parse(&request.text, request.trim)?;
        let reference = request
            .reference
            .map(|reference| {
                Ok::<_, anyhow::Error>((parse(&reference.text, reference.trim)?, reference.trim))
            })
            .transpose()
            .context("invalid starting sound")?;
        ensure!(
            !request.hear_reference || reference.is_some(),
            "no starting sound captured"
        );
        ensure!(
            request
                .hold_gain_db
                .is_none_or(|held| held.is_finite() && (-60.0..=60.0).contains(&held)),
            "invalid held gain"
        );
        Ok(Self {
            owner: owner.to_owned(),
            draft,
            trim: request.trim,
            reference,
            hear_reference: request.hear_reference,
            hold_gain_db: request.hold_gain_db,
            renewed: Instant::now(),
            reference_cache: Default::default(),
        })
    }

    pub fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.renewed) >= LIVE_TIMEOUT
    }

    pub fn permits(&self, owner: &str, now: Instant) -> bool {
        self.owner == owner || self.expired(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draft_is_ephemeral_and_rejects_invalid_values_or_changed_assets() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = storage.load_library().unwrap();
        let profile = storage.create_profile(&mut library, "Live").unwrap();
        let before = storage.parsed_profile(&library, &profile.id).unwrap();
        let request = |text: &str, trim| Request {
            text: text.into(),
            trim,
            reference: None,
            hear_reference: false,
            hold_gain_db: None,
        };
        let edit = Session::parse(
            &storage,
            &library,
            &profile.id,
            ":1.10",
            request("Filter 1: ON PK Fc 1000 Hz Gain -3 dB Q 1", 0.0),
        )
        .unwrap();
        assert_eq!(edit.draft.filters[0].gain_db, -3.0);
        assert_eq!(
            storage.parsed_profile(&library, &profile.id).unwrap(),
            before
        );
        assert!(!edit.expired(edit.renewed + Duration::from_secs(4)));
        assert!(edit.expired(edit.renewed + LIVE_TIMEOUT));
        assert!(edit.permits(":1.10", edit.renewed));
        assert!(!edit.permits(":1.11", edit.renewed + Duration::from_secs(4)));
        assert!(edit.permits(":1.11", edit.renewed + LIVE_TIMEOUT));
        assert!(
            Session::parse(
                &storage,
                &library,
                &profile.id,
                ":1.10",
                request("", f64::NAN)
            )
            .is_err()
        );
        let held = |value| Request {
            hold_gain_db: Some(value),
            ..request("", 0.0)
        };
        assert!(Session::parse(&storage, &library, &profile.id, ":1.10", held(-3.5)).is_ok());
        assert!(
            Session::parse(
                &storage,
                &library,
                &profile.id,
                ":1.10",
                held(f64::INFINITY)
            )
            .is_err()
        );
        assert!(
            Session::parse(
                &storage,
                &library,
                &profile.id,
                ":1.10",
                request("Convolution: assets/other.wav", 0.0)
            )
            .is_err()
        );
    }
}
