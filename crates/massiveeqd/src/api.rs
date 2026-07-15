use crate::{AppState, refresh_locked};
use massiveeq_core::{
    COMPARISON_BYPASS_ID, ChannelAnalysis, ChannelSelection, ComparisonSet,
    MAX_COMPARISON_PROFILES, ProfileAnalysis,
};
use massiveeq_dsp::{CompileOptions, compile_profile};
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::fdo;
use zbus::object_server::SignalEmitter;

#[derive(Clone)]
pub struct Service {
    pub state: Arc<Mutex<AppState>>,
}

#[zbus::interface(name = "org.massiveeq.Service1")]
impl Service {
    async fn ping(&self) -> &'static str {
        concat!("MassiveEQ ", env!("CARGO_PKG_VERSION"))
    }

    async fn list_profiles(&self) -> fdo::Result<String> {
        let state = self.state.lock().await;
        let profiles = state
            .storage
            .list_profiles(&state.library)
            .map_err(failed)?;
        serde_json::to_string(&profiles).map_err(failed)
    }

    async fn get_profile(&self, id: &str) -> fdo::Result<String> {
        let state = self.state.lock().await;
        let record = state
            .library
            .profiles
            .get(id)
            .ok_or_else(|| fdo::Error::Failed(format!("profile {id} not found")))?;
        serde_json::to_string(&state.storage.get_profile(record).map_err(failed)?).map_err(failed)
    }

    async fn create_profile(
        &self,
        name: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<String> {
        let mut state = self.state.lock().await;
        let storage = state.storage.clone();
        let profile = storage
            .create_profile(&mut state.library, name)
            .map_err(failed)?;
        let result = serde_json::to_string(&profile).map_err(failed)?;
        Self::changed(&emitter, "profiles").await.map_err(failed)?;
        Ok(result)
    }

    async fn put_profile(
        &self,
        id: &str,
        name: &str,
        text: &str,
        manual_trim_db: f64,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<String> {
        let mut state = self.state.lock().await;
        let storage = state.storage.clone();
        let profile = storage
            .put_profile(&mut state.library, id, name, text, manual_trim_db)
            .map_err(failed)?;
        refresh_locked(&mut state, true).await.map_err(failed)?;
        let result = serde_json::to_string(&profile).map_err(failed)?;
        Self::changed(&emitter, "profiles").await.map_err(failed)?;
        Ok(result)
    }

    async fn import_profile(
        &self,
        path: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<String> {
        let mut state = self.state.lock().await;
        let storage = state.storage.clone();
        let profile = storage
            .import_profile(&mut state.library, std::path::Path::new(path))
            .map_err(failed)?;
        let result = serde_json::to_string(&profile).map_err(failed)?;
        Self::changed(&emitter, "profiles").await.map_err(failed)?;
        Ok(result)
    }

    async fn set_convolution(
        &self,
        id: &str,
        path: &str,
        channel: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<String> {
        let channels = match channel {
            "L" => ChannelSelection::Left,
            "R" => ChannelSelection::Right,
            _ => ChannelSelection::All,
        };
        let mut state = self.state.lock().await;
        let storage = state.storage.clone();
        let profile = storage
            .set_convolution(&state.library, id, std::path::Path::new(path), channels)
            .map_err(failed)?;
        refresh_locked(&mut state, true).await.map_err(failed)?;
        let result = serde_json::to_string(&profile).map_err(failed)?;
        Self::changed(&emitter, "profiles").await.map_err(failed)?;
        Ok(result)
    }

    async fn delete_profile(
        &self,
        id: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let mut state = self.state.lock().await;
        let storage = state.storage.clone();
        storage
            .delete_profile(&mut state.library, id)
            .map_err(failed)?;
        refresh_locked(&mut state, true).await.map_err(failed)?;
        Self::changed(&emitter, "profiles").await.map_err(failed)
    }

    async fn export_profile(&self, id: &str, destination: &str) -> fdo::Result<()> {
        let state = self.state.lock().await;
        state
            .storage
            .export_profile(&state.library, id, std::path::Path::new(destination))
            .map_err(failed)
    }

    async fn list_devices(&self) -> fdo::Result<String> {
        let state = self.state.lock().await;
        serde_json::to_string(&state.devices).map_err(failed)
    }

    async fn list_comparisons(&self) -> fdo::Result<String> {
        let state = self.state.lock().await;
        serde_json::to_string(&state.library.comparison_sets).map_err(failed)
    }

    async fn configure_comparison(
        &self,
        device_key: &str,
        profile_ids_json: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<String> {
        let requested: Vec<String> = serde_json::from_str(profile_ids_json).map_err(failed)?;
        let mut profile_ids = Vec::with_capacity(requested.len());
        for profile_id in requested {
            if !profile_ids.contains(&profile_id) {
                profile_ids.push(profile_id);
            }
        }
        if !(2..=MAX_COMPARISON_PROFILES).contains(&profile_ids.len()) {
            return Err(fdo::Error::Failed(format!(
                "choose between 2 and {MAX_COMPARISON_PROFILES} unique comparison candidates"
            )));
        }

        let mut state = self.state.lock().await;
        if let Some(missing) = profile_ids.iter().find(|profile_id| {
            profile_id.as_str() != COMPARISON_BYPASS_ID
                && !state.library.profiles.contains_key(*profile_id)
        }) {
            return Err(fdo::Error::Failed(format!(
                "profile {missing} does not exist"
            )));
        }
        let active_profile_id = state
            .library
            .comparison_sets
            .get(device_key)
            .map(|comparison| comparison.active_profile_id.clone())
            .filter(|active| profile_ids.contains(active))
            .unwrap_or_else(|| profile_ids[0].clone());
        let comparison = ComparisonSet {
            profile_ids,
            active_profile_id,
            enabled: true,
        };
        let device = state
            .devices
            .iter()
            .find(|device| device.key.as_storage_key() == device_key)
            .cloned()
            .ok_or_else(|| fdo::Error::Failed("output is not known to MassiveEQ".into()))?;
        crate::filter_host::validate_comparison_set(
            &state.storage,
            &state.library,
            &device,
            crate::device::graph_settings().await,
            &comparison,
        )
        .map_err(failed)?;
        state
            .library
            .comparison_sets
            .insert(device_key.to_owned(), comparison.clone());
        state.library.bypassed_devices.remove(device_key);
        state.storage.save_library(&state.library).map_err(failed)?;
        refresh_locked(&mut state, false).await.map_err(failed)?;
        let result = serde_json::to_string(&comparison).map_err(failed)?;
        Self::changed(&emitter, "comparison")
            .await
            .map_err(failed)?;
        Ok(result)
    }

    async fn select_comparison_profile(
        &self,
        device_key: &str,
        profile_id: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let mut state = self.state.lock().await;
        let comparison = state
            .library
            .comparison_sets
            .get_mut(device_key)
            .ok_or_else(|| fdo::Error::Failed("this output has no comparison bank".into()))?;
        if !comparison.profile_ids.iter().any(|id| id == profile_id) {
            return Err(fdo::Error::Failed(
                "the requested profile is not in this comparison bank".into(),
            ));
        }
        comparison.active_profile_id = profile_id.to_owned();
        comparison.enabled = true;
        state.library.bypassed_devices.remove(device_key);
        state.storage.save_library(&state.library).map_err(failed)?;
        refresh_locked(&mut state, false).await.map_err(failed)?;
        Self::changed(&emitter, "comparison").await.map_err(failed)
    }

    async fn set_comparison_enabled(
        &self,
        device_key: &str,
        enabled: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let mut state = self.state.lock().await;
        let comparison = state
            .library
            .comparison_sets
            .get_mut(device_key)
            .ok_or_else(|| fdo::Error::Failed("this output has no comparison bank".into()))?;
        comparison.enabled = enabled;
        if enabled {
            state.library.bypassed_devices.remove(device_key);
        }
        state.storage.save_library(&state.library).map_err(failed)?;
        refresh_locked(&mut state, false).await.map_err(failed)?;
        Self::changed(&emitter, "comparison").await.map_err(failed)
    }

    async fn delete_comparison(
        &self,
        device_key: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let mut state = self.state.lock().await;
        state.library.comparison_sets.remove(device_key);
        state.storage.save_library(&state.library).map_err(failed)?;
        refresh_locked(&mut state, false).await.map_err(failed)?;
        Self::changed(&emitter, "comparison").await.map_err(failed)
    }

    async fn assign_profile(
        &self,
        device_key: &str,
        profile_id: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let mut state = self.state.lock().await;
        require_known_device(&state, device_key)?;
        if profile_id.is_empty() {
            state.library.assignments.remove(device_key);
        } else if state.library.profiles.contains_key(profile_id) {
            let (profile, _) = state
                .storage
                .parsed_profile(&state.library, profile_id)
                .map_err(failed)?;
            if !profile.is_activatable() {
                return Err(fdo::Error::Failed(
                    "profile has validation errors and cannot be assigned".into(),
                ));
            }
            state
                .library
                .assignments
                .insert(device_key.to_owned(), profile_id.to_owned());
        } else {
            return Err(fdo::Error::Failed(format!(
                "profile {profile_id} not found"
            )));
        }
        state.library.bypassed_devices.remove(device_key);
        if let Some(comparison) = state.library.comparison_sets.get_mut(device_key) {
            comparison.enabled = false;
        }
        state.storage.save_library(&state.library).map_err(failed)?;
        refresh_locked(&mut state, true).await.map_err(failed)?;
        Self::changed(&emitter, "assignments").await.map_err(failed)
    }

    async fn set_device_bypass(
        &self,
        device_key: &str,
        bypassed: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let mut state = self.state.lock().await;
        require_known_device(&state, device_key)?;
        if bypassed {
            state.library.bypassed_devices.insert(device_key.to_owned());
        } else {
            state.library.bypassed_devices.remove(device_key);
        }
        state.storage.save_library(&state.library).map_err(failed)?;
        refresh_locked(&mut state, true).await.map_err(failed)?;
        Self::changed(&emitter, "bypass").await.map_err(failed)
    }

    async fn set_global_bypass(
        &self,
        bypassed: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let mut state = self.state.lock().await;
        state.library.global_bypass = bypassed;
        state.storage.save_library(&state.library).map_err(failed)?;
        refresh_locked(&mut state, true).await.map_err(failed)?;
        Self::changed(&emitter, "bypass").await.map_err(failed)
    }

    async fn analyze(&self, id: &str, sample_rate: u32) -> fdo::Result<String> {
        let state = self.state.lock().await;
        let (profile, trim) = state
            .storage
            .parsed_profile(&state.library, id)
            .map_err(failed)?;
        let compiled = compile_profile(
            &profile,
            &CompileOptions {
                sample_rate: sample_rate.clamp(8_000, 384_000),
                quantum: 2_048,
                output_channels: 2,
                manual_trim_db: trim,
                profile_dir: state.storage.profile_dir(id),
            },
        )
        .map_err(failed)?;
        serde_json::to_string(&legacy_analysis(&compiled.analysis)).map_err(failed)
    }

    async fn status(&self) -> fdo::Result<String> {
        let state = self.state.lock().await;
        Ok(serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "profiles": state.library.profiles.len(), "devices": state.devices.len(), "active_filters": state.host.active_count(), "global_bypass": state.library.global_bypass, "comparisons": state.library.comparison_sets, "engine": state.host.status_json() }).to_string())
    }

    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>, topic: &str) -> zbus::Result<()>;
}

fn failed(error: impl std::fmt::Display) -> fdo::Error {
    fdo::Error::Failed(error.to_string())
}

fn require_known_device(state: &crate::AppState, device_key: &str) -> fdo::Result<()> {
    if state
        .devices
        .iter()
        .any(|device| device.key.as_storage_key() == device_key)
    {
        Ok(())
    } else {
        Err(fdo::Error::Failed(format!(
            "output {device_key} is not known to MassiveEQ"
        )))
    }
}

fn legacy_analysis(analysis: &massiveeq_dsp::CompiledAnalysis) -> ProfileAnalysis {
    let channel = |index: usize| {
        let source = analysis
            .channels
            .get(index)
            .or_else(|| analysis.channels.first())
            .expect("compiled analysis contains an output channel");
        ChannelAnalysis {
            preamp_db: source.source_preamp_db,
            peak_db: source.uncorrected_peak_db,
            response: source
                .response
                .iter()
                .map(|point| massiveeq_core::analysis::ResponsePoint {
                    frequency: point.frequency,
                    gain_db: point.gain_db,
                })
                .collect(),
        }
    };
    ProfileAnalysis {
        left: channel(0),
        right: channel(1),
        match_gain_db: analysis.match_gain_db,
        manual_trim_db: analysis.manual_trim_db,
        safety_attenuation_db: analysis.safety_attenuation_db,
        effective_gain_db: analysis.effective_gain_db,
        headroom_limited: analysis.headroom_limited,
    }
}
