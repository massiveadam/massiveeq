use crate::{device, native_filter::NativeFilter};
use anyhow::{Context, Result};
use massiveeq_core::{COMPARISON_BYPASS_ID, ComparisonSet, DeviceInfo, Library, Storage};
use massiveeq_dsp::{
    CompileOptions, CompiledProfile, ProfileProcessor, attenuate_for_comparison,
    compile_bypass_with_gain, compile_profile, perceived_output_level_db,
};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use tokio::{
    process::Command,
    time::{Duration, sleep},
};

pub struct FilterHost {
    filters: HashMap<String, ActiveFilter>,
    errors: HashMap<String, String>,
}

struct ActiveFilter {
    filter: NativeFilter,
    revision: u64,
    sample_rate: u32,
    quantum: u32,
    latency_frames: u32,
}

impl FilterHost {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            filters: HashMap::new(),
            errors: HashMap::new(),
        })
    }

    pub async fn stop_all(&mut self) {
        for (_, active) in self.filters.drain() {
            active.filter.stop();
        }
    }

    pub async fn reconcile(
        &mut self,
        storage: &Storage,
        library: &Library,
        devices: &[DeviceInfo],
        force_update: bool,
    ) -> Result<()> {
        let finished = self
            .filters
            .iter()
            .filter(|(_, active)| active.filter.is_finished())
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in finished {
            if let Some(active) = self.filters.remove(&key) {
                active.filter.stop();
            }
        }

        let desired = devices
            .iter()
            .filter(|device| {
                device.connected
                    && matches!(device.channels, 1 | 2)
                    && device_has_runtime_profile(library, &device.key.as_storage_key())
            })
            .map(|device| device.key.as_storage_key())
            .collect::<HashSet<_>>();
        let stale = self
            .filters
            .keys()
            .filter(|key| !desired.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in stale {
            if let Some(active) = self.filters.remove(&key) {
                active.filter.stop();
            }
            self.errors.remove(&key);
        }

        let settings = device::graph_settings().await;
        for device in devices {
            let key = device.key.as_storage_key();
            if !desired.contains(&key) {
                continue;
            }
            let bypassed = device.bypassed || library.global_bypass;
            let (compiled, revision) =
                match compile_runtime_profile(storage, library, device, settings, bypassed) {
                    Ok(value) => value,
                    Err(error) => {
                        self.errors.insert(key, error.to_string());
                        continue;
                    }
                };
            if !force_update
                && self.filters.get(&key).is_some_and(|active| {
                    active.revision == revision
                        && active.sample_rate == settings.sample_rate
                        && active.quantum == settings.quantum
                })
            {
                if let Some(active) = self.filters.get_mut(&key) {
                    active.filter.reap();
                }
                continue;
            }

            let latency_frames = compiled.latency_frames;
            let processor = match ProfileProcessor::new(&compiled) {
                Ok(processor) => Box::new(processor),
                Err(error) => {
                    self.errors.insert(key, error.to_string());
                    continue;
                }
            };

            if let Some(active) = self.filters.get_mut(&key)
                && active.sample_rate == settings.sample_rate
                && active.quantum == settings.quantum
                && active.latency_frames == latency_frames
            {
                match active.filter.update(processor) {
                    Ok(()) => {
                        active.revision = revision;
                        self.errors.remove(&key);
                    }
                    Err(error) => {
                        self.errors.insert(key, error.to_string());
                    }
                }
                continue;
            }

            // Rate, capacity and reported latency are PipeWire node properties.
            // Start the verified replacement before retiring the old endpoint.
            let replacement = match NativeFilter::spawn(
                device.clone(),
                processor,
                settings.sample_rate,
                settings.quantum,
                latency_frames,
            ) {
                Ok(filter) => filter,
                Err(error) => {
                    self.errors.insert(key, error.to_string());
                    continue;
                }
            };
            if let Some(old) = self.filters.remove(&key) {
                old.filter.stop();
            }
            self.filters.insert(
                key.clone(),
                ActiveFilter {
                    filter: replacement,
                    revision,
                    sample_rate: settings.sample_rate,
                    quantum: settings.quantum,
                    latency_frames,
                },
            );
            self.errors.remove(&key);
            let _ = normalize_virtual_nodes().await;
            let _ = restore_physical_default_if_needed(
                self.filters[&key].filter.node_name.as_str(),
                &device.node_name,
            )
            .await;
        }
        Ok(())
    }

    pub fn active_count(&self) -> usize {
        self.filters.len()
    }

    pub fn status_json(&self) -> serde_json::Value {
        let active = self
            .filters
            .iter()
            .map(|(device, active)| {
                let calls = active.filter.stats.process_calls.load(std::sync::atomic::Ordering::Relaxed);
                let total = active.filter.stats.process_nanos_total.load(std::sync::atomic::Ordering::Relaxed);
                let average_nanos = if calls == 0 { 0.0 } else { total as f64 / calls as f64 };
                let deadline_nanos = active.quantum as f64 * 1_000_000_000.0 / active.sample_rate as f64;
                let cpu_percent = if deadline_nanos == 0.0 { 0.0 } else { average_nanos * 100.0 / deadline_nanos };
                serde_json::json!({
                    "device": device,
                    "node": active.filter.node_name,
                    "sample_rate": active.sample_rate,
                    "quantum_capacity": active.quantum,
                    "latency_frames": active.latency_frames,
                    "latency_ms": active.latency_frames as f64 * 1000.0 / active.sample_rate as f64,
                    "revision": active.revision,
                    "input_overflows": active.filter.stats.input_overflows.load(std::sync::atomic::Ordering::Relaxed),
                    "output_underflows": active.filter.stats.output_underflows.load(std::sync::atomic::Ordering::Relaxed),
                    "invalid_buffers": active.filter.stats.invalid_buffers.load(std::sync::atomic::Ordering::Relaxed),
                    "process_average_us": average_nanos / 1000.0,
                    "process_peak_us": active.filter.stats.process_nanos_peak.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0,
                    "cpu_percent_of_deadline": cpu_percent,
                    "cpu_warning": cpu_percent >= 50.0,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "kind": "native-rust-dsp",
            "active": active,
            "errors": self.errors,
        })
    }
}

fn device_has_runtime_profile(library: &Library, device_key: &str) -> bool {
    let comparison = library
        .comparison_sets
        .get(device_key)
        .filter(|comparison| comparison.enabled);
    if let Some(comparison) = comparison {
        return comparison.profile_ids.len() >= 2
            && comparison
                .profile_ids
                .iter()
                .all(|id| id == COMPARISON_BYPASS_ID || library.profiles.contains_key(id))
            && comparison
                .profile_ids
                .contains(&comparison.active_profile_id);
    }
    library
        .assignments
        .get(device_key)
        .is_some_and(|id| library.profiles.contains_key(id))
}

fn compile_runtime_profile(
    storage: &Storage,
    library: &Library,
    device: &DeviceInfo,
    settings: device::GraphSettings,
    bypassed: bool,
) -> Result<(CompiledProfile, u64)> {
    let key = device.key.as_storage_key();
    if let Some(comparison) = library
        .comparison_sets
        .get(&key)
        .filter(|comparison| comparison.enabled)
    {
        return compile_comparison_profile(
            storage, library, device, settings, comparison, bypassed,
        );
    }

    let profile_id = library
        .assignments
        .get(&key)
        .context("output has no assigned profile")?;
    let (profile, trim) = storage.parsed_profile(library, profile_id)?;
    let options = CompileOptions {
        sample_rate: settings.sample_rate,
        quantum: settings.quantum,
        output_channels: device.channels,
        manual_trim_db: trim,
        profile_dir: storage.profile_dir(profile_id),
    };
    let active = compile_profile(&profile, &options)?;
    let compiled = if bypassed {
        // Keep dry comparisons at the same perceived level as the active
        // profile. The gain is never allowed to exceed the profile's own
        // clipping-protected output level.
        compile_bypass_with_gain(
            settings.sample_rate,
            settings.quantum,
            device.channels,
            perceived_output_level_db(&active).min(0.0),
        )
    } else {
        active
    };
    let revision = profile_revision(
        &profile,
        trim,
        bypassed,
        settings.sample_rate,
        settings.quantum,
        device.channels,
    );
    Ok((compiled, revision))
}

fn compile_comparison_profile(
    storage: &Storage,
    library: &Library,
    device: &DeviceInfo,
    settings: device::GraphSettings,
    comparison: &ComparisonSet,
    bypassed: bool,
) -> Result<(CompiledProfile, u64)> {
    anyhow::ensure!(
        comparison.profile_ids.len() >= 2,
        "a comparison bank needs at least two candidates"
    );
    anyhow::ensure!(
        comparison
            .profile_ids
            .contains(&comparison.active_profile_id),
        "the active comparison candidate is not in the bank"
    );

    let mut candidates = Vec::with_capacity(comparison.profile_ids.len());
    let mut signature = serde_json::to_string(comparison).unwrap_or_default();
    for profile_id in &comparison.profile_ids {
        if profile_id == COMPARISON_BYPASS_ID {
            candidates.push((
                profile_id.clone(),
                compile_bypass_with_gain(
                    settings.sample_rate,
                    settings.quantum,
                    device.channels,
                    0.0,
                ),
                0.0,
            ));
            signature.push_str("|bypass");
            continue;
        }
        let (profile, trim) = storage.parsed_profile(library, profile_id)?;
        let compiled = compile_profile(
            &profile,
            &CompileOptions {
                sample_rate: settings.sample_rate,
                quantum: settings.quantum,
                output_channels: device.channels,
                manual_trim_db: trim,
                profile_dir: storage.profile_dir(profile_id),
            },
        )?;
        let perceived = perceived_output_level_db(&compiled);
        signature.push('|');
        signature.push_str(profile_id);
        signature.push('|');
        signature.push_str(&format!("{trim:.9}"));
        signature.push('|');
        signature.push_str(&serde_json::to_string(&profile).unwrap_or_default());
        candidates.push((profile_id.clone(), compiled, perceived));
    }

    let latency = candidates
        .first()
        .map(|(_, compiled, _)| compiled.latency_frames)
        .unwrap_or_default();
    anyhow::ensure!(
        candidates
            .iter()
            .all(|(_, compiled, _)| compiled.latency_frames == latency),
        "comparison candidates have incompatible processing latency"
    );
    let target_level = candidates
        .iter()
        .map(|(_, _, perceived)| *perceived)
        .fold(0.0_f64, f64::min)
        .min(0.0);
    let selected_id = if bypassed {
        COMPARISON_BYPASS_ID
    } else {
        comparison.active_profile_id.as_str()
    };
    let mut selected = if selected_id == COMPARISON_BYPASS_ID {
        compile_bypass_with_gain(
            settings.sample_rate,
            settings.quantum,
            device.channels,
            target_level,
        )
    } else {
        let (_, mut compiled, perceived) = candidates
            .into_iter()
            .find(|(id, _, _)| id == selected_id)
            .context("active comparison profile is unavailable")?;
        attenuate_for_comparison(&mut compiled, target_level - perceived)?;
        compiled
    };
    // A dry candidate is only admitted when every bank member has zero
    // latency. Preserve the common latency for processed candidates; an
    // explicit engine bypass still reports its true zero processing latency.
    if selected_id != COMPARISON_BYPASS_ID {
        selected.latency_frames = latency;
    }
    signature.push_str(&format!(
        "|active={selected_id}|bypassed={bypassed}|target={target_level:.9}|{}|{}|{}",
        settings.sample_rate, settings.quantum, device.channels
    ));
    Ok((selected, stable_hash(&signature)))
}

pub(crate) fn validate_comparison_set(
    storage: &Storage,
    library: &Library,
    device: &DeviceInfo,
    settings: device::GraphSettings,
    comparison: &ComparisonSet,
) -> Result<()> {
    compile_comparison_profile(storage, library, device, settings, comparison, false).map(|_| ())
}

fn profile_revision(
    profile: &massiveeq_core::ProfileDocument,
    trim: f64,
    bypassed: bool,
    sample_rate: u32,
    quantum: u32,
    channels: u32,
) -> u64 {
    let serialized = serde_json::to_string(profile).unwrap_or_default();
    stable_hash(&format!(
        "{serialized}|{trim:.9}|{bypassed}|{sample_rate}|{quantum}|{channels}"
    ))
}

async fn normalize_virtual_nodes() -> Result<()> {
    // Do this outside the RT thread. WirePlumber may restore an old virtual
    // node volume even though MassiveEQ's DSP gain is already explicit.
    sleep(Duration::from_millis(200)).await;
    let status = Command::new("wpctl")
        .args([
            "set-volume",
            "--pid",
            &std::process::id().to_string(),
            "1.0",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to normalize MassiveEQ virtual node volume")?;
    anyhow::ensure!(status.success(), "failed to set virtual nodes to unity");
    Ok(())
}

async fn restore_physical_default_if_needed(
    filter_node_name: &str,
    target_node_name: &str,
) -> Result<()> {
    let output = Command::new("wpctl")
        .args(["status", "--name"])
        .output()
        .await
        .context("failed to inspect the default audio output")?;
    anyhow::ensure!(output.status.success(), "failed to inspect audio outputs");
    let status = String::from_utf8_lossy(&output.stdout);
    let filter_is_default = status.lines().any(|line| {
        line.contains(filter_node_name) && line.split_whitespace().any(|token| token == "*")
    });
    if !filter_is_default {
        return Ok(());
    }
    let Some(target_id) = status
        .lines()
        .find_map(|line| node_id_from_status_line(line, target_node_name))
    else {
        return Ok(());
    };
    let result = Command::new("wpctl")
        .args(["set-default", &target_id.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to restore the physical default audio output")?;
    anyhow::ensure!(result.success(), "failed to restore physical output");
    Ok(())
}

fn node_id_from_status_line(line: &str, node_name: &str) -> Option<u32> {
    line.contains(node_name).then(|| {
        line.split_whitespace()
            .find_map(|token| token.strip_suffix('.')?.parse().ok())
    })?
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use massiveeq_core::{DeviceKey, parse_text};

    #[test]
    fn profile_revision_changes_for_audio_relevant_state() {
        let profile = parse_text("test", "Filter: ON PK Fc 1000 Hz Gain 3 dB Q 1");
        let base = profile_revision(&profile, 0.0, false, 48_000, 1_024, 2);
        assert_ne!(
            base,
            profile_revision(&profile, 1.0, false, 48_000, 1_024, 2)
        );
        assert_ne!(
            base,
            profile_revision(&profile, 0.0, true, 48_000, 1_024, 2)
        );
        assert_ne!(
            base,
            profile_revision(&profile, 0.0, false, 96_000, 1_024, 2)
        );
    }

    #[test]
    fn extracts_physical_node_id_from_wpctl_status() {
        let line = " │  *   97. bluez_output.AA_BB.1    [vol: 0.43]";
        assert_eq!(
            node_id_from_status_line(line, "bluez_output.AA_BB.1"),
            Some(97)
        );
    }

    #[test]
    fn three_way_comparison_uses_one_safe_perceived_level() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = Library::default();
        let flat = storage.create_profile(&mut library, "Flat").unwrap();
        let bass = storage.create_profile(&mut library, "Bass").unwrap();
        storage
            .put_profile(
                &mut library,
                &bass.id,
                "Bass",
                "Filter: ON LSC Fc 100 Hz Gain 6 dB Q 0.707",
                0.0,
            )
            .unwrap();
        let presence = storage.create_profile(&mut library, "Presence").unwrap();
        storage
            .put_profile(
                &mut library,
                &presence.id,
                "Presence",
                "Filter: ON PK Fc 3000 Hz Gain 4 dB Q 1.2",
                0.0,
            )
            .unwrap();
        let device = DeviceInfo {
            key: DeviceKey {
                backend: "test".into(),
                stable_id: "airpods".into(),
                route: "a2dp-sink".into(),
            },
            node_name: "test.airpods".into(),
            description: "AirPods".into(),
            channels: 2,
            connected: true,
            assigned_profile: None,
            bypassed: false,
        };
        let ids = vec![
            COMPARISON_BYPASS_ID.to_owned(),
            flat.id,
            bass.id,
            presence.id,
        ];
        let mut levels = Vec::new();
        for active in &ids {
            let comparison = ComparisonSet {
                profile_ids: ids.clone(),
                active_profile_id: active.clone(),
                enabled: true,
            };
            let (compiled, _) = compile_comparison_profile(
                &storage,
                &library,
                &device,
                device::GraphSettings {
                    sample_rate: 48_000,
                    quantum: 256,
                },
                &comparison,
                false,
            )
            .unwrap();
            assert_eq!(compiled.latency_frames, 0);
            levels.push(perceived_output_level_db(&compiled));
        }
        assert!(
            levels
                .windows(2)
                .all(|pair| (pair[0] - pair[1]).abs() < 1e-9)
        );
        assert!(levels[0] <= -1.0);
    }
}
