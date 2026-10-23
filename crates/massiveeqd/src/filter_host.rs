use crate::{device, native_filter::NativeFilter};
use anyhow::{Context, Result};
use massiveeq_core::{COMPARISON_BYPASS_ID, ComparisonSet, DeviceInfo, Library, Storage};
use massiveeq_dsp::{
    CompileOptions, CompiledProfile, ProfileProcessor, attenuate_for_comparison,
    compile_bypass_with_gain, compile_profile, perceived_output_level_db, widen_input,
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
    /// Channels the sink takes in; more than the device's for a surround sink.
    input_channels: u32,
    /// The sink is a visible 7.1 output that may be the system default.
    surround: bool,
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
        edits: &crate::live_edit::Sessions,
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
                if active.surround
                    && let Some(device) = devices
                        .iter()
                        .find(|device| device.connected && device.key.as_storage_key() == key)
                {
                    // Hand the system output back before the 7.1 sink disappears.
                    let _ = restore_physical_default_if_needed(
                        &active.filter.node_name,
                        &device.node_name,
                    )
                    .await;
                }
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
            let (compiled, revision) =
                match compile_output_with_edits(storage, library, device, settings, edits) {
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

            let input_channels = compiled.input_channels;
            let surround = input_channels > device.channels;
            if let Some(active) = self.filters.get_mut(&key)
                && active.sample_rate == settings.sample_rate
                && active.quantum == settings.quantum
                && active.latency_frames == latency_frames
                && active.input_channels == input_channels
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
            // Normalizing below resets every MassiveEQ node to unity. A 7.1
            // sink's volume belongs to the user, so keep and restore those.
            let surround_volumes = node_volumes(self.surround_node_names()).await;
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
                if old.surround && !surround {
                    let _ = restore_physical_default_if_needed(
                        &old.filter.node_name,
                        &device.node_name,
                    )
                    .await;
                }
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
                    input_channels,
                    surround,
                },
            );
            self.errors.remove(&key);
            let _ = normalize_virtual_nodes().await;
            for (name, volume) in &surround_volumes {
                if self
                    .filters
                    .values()
                    .any(|active| active.surround && &active.filter.node_name == name)
                {
                    let _ = set_node_volume(name, volume).await;
                }
            }
            let node_name = self.filters[&key].filter.node_name.clone();
            if surround {
                // Apps choose their channel layout from the system output, so
                // the 7.1 sink takes over while its device is the system output.
                if device.is_default {
                    let _ = make_default(&node_name).await;
                }
            } else {
                let _ = restore_physical_default_if_needed(&node_name, &device.node_name).await;
            }
        }
        Ok(())
    }

    pub fn errors_for_profile(
        &self,
        library: &Library,
        devices: &[DeviceInfo],
        id: &str,
    ) -> Option<String> {
        devices
            .iter()
            .filter(|device| device.effective_profile_id(&library.comparison_sets) == Some(id))
            .find_map(|device| self.errors.get(&device.key.as_storage_key()).cloned())
    }

    fn surround_node_names(&self) -> Vec<String> {
        self.filters
            .values()
            .filter(|active| active.surround)
            .map(|active| active.filter.node_name.clone())
            .collect()
    }

    pub fn active_count(&self) -> usize {
        self.filters.len()
    }

    pub fn status_json(&self) -> serde_json::Value {
        let active = self
            .filters
            .iter()
            .map(|(device, active)| {
                let scheduling_latency_frames = active.latency_frames.max(active.quantum.max(1));
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
                    "latency_frames": scheduling_latency_frames,
                    "latency_ms": scheduling_latency_frames as f64 * 1000.0 / active.sample_rate as f64,
                    "dsp_latency_frames": active.latency_frames,
                    "dsp_latency_ms": active.latency_frames as f64 * 1000.0 / active.sample_rate as f64,
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

fn compile_reference(
    edit: &crate::live_edit::Session,
    profile: &massiveeq_core::ProfileDocument,
    options: &CompileOptions,
) -> Result<CompiledProfile> {
    let key = (
        options.sample_rate,
        options.quantum,
        options.output_channels,
    );
    let mut cache = edit
        .reference_cache
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(compiled) = cache.get(&key) {
        return Ok(compiled.clone());
    }
    let compiled = compile_profile(profile, options)?;
    cache.insert(key, compiled.clone());
    Ok(compiled)
}

fn compile_output_with_edits(
    storage: &Storage,
    library: &Library,
    device: &DeviceInfo,
    settings: device::GraphSettings,
    edits: &crate::live_edit::Sessions,
) -> Result<(CompiledProfile, u64)> {
    if !library.global_bypass
        && let Some(bank) = library
            .comparison_sets
            .get(&device.key.as_storage_key())
            .filter(|bank| bank.enabled)
    {
        return compile_comparison_with_edits(
            storage,
            library,
            device,
            settings,
            bank,
            device.bypassed,
            Some(edits),
        );
    }
    if !library.global_bypass
        && let Some(id) = device.effective_profile_id(&library.comparison_sets)
        && let Some(edit) = edits.get(id)
        && !edit.expired(std::time::Instant::now())
    {
        let options = CompileOptions {
            sample_rate: settings.sample_rate,
            quantum: settings.quantum,
            output_channels: device.channels,
            manual_trim_db: edit.trim,
            profile_dir: storage.profile_dir(id),
        };
        let mut current = compile_profile(&edit.draft, &options)?;
        let mut revision = profile_revision(
            &edit.draft,
            edit.trim,
            device.bypassed,
            settings.sample_rate,
            settings.quantum,
            device.channels,
        );
        if let Some((reference, trim)) = &edit.reference {
            let mut original = compile_reference(
                edit,
                reference,
                &CompileOptions {
                    manual_trim_db: *trim,
                    ..options
                },
            )?;
            anyhow::ensure!(
                current.latency_frames == original.latency_frames,
                "Starting sound has different processing latency; capture a new starting sound"
            );
            let width = current.input_channels.max(original.input_channels);
            widen_input(&mut current, width)?;
            widen_input(&mut original, width)?;
            let current_level = perceived_output_level_db(&current);
            let reference_level = perceived_output_level_db(&original);
            let target = current_level.min(reference_level).min(0.0);
            attenuate_for_comparison(&mut current, target - current_level)?;
            attenuate_for_comparison(&mut original, target - reference_level)?;
            revision ^= profile_revision(
                reference,
                *trim,
                edit.hear_reference,
                settings.sample_rate,
                settings.quantum,
                device.channels,
            )
            .rotate_left(17);
            if edit.hear_reference {
                current = original;
            }
        }
        if device.bypassed {
            let gain = perceived_output_level_db(&current).min(0.0);
            current = compile_level_matched_bypass(&current, gain)?;
        }
        return Ok((current, revision));
    }
    compile_output_state(storage, library, device, settings)
}

fn compile_output_state(
    storage: &Storage,
    library: &Library,
    device: &DeviceInfo,
    settings: device::GraphSettings,
) -> Result<(CompiledProfile, u64)> {
    if library.global_bypass {
        let compiled =
            compile_bypass_with_gain(settings.sample_rate, settings.quantum, device.channels, 0.0);
        let revision = stable_hash(&format!(
            "engine-off|{}|{}|{}",
            settings.sample_rate, settings.quantum, device.channels
        ));
        Ok((compiled, revision))
    } else {
        compile_runtime_profile(storage, library, device, settings, device.bypassed)
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
        // Filters Off retains the active profile's clipping-safe perceived
        // level and reported endpoint latency. This makes it a comparison of
        // filters rather than a comparison of loudness or route stability.
        compile_level_matched_bypass(&active, perceived_output_level_db(&active).min(0.0))?
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
    compile_comparison_with_edits(
        storage, library, device, settings, comparison, bypassed, None,
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_comparison_with_edits(
    storage: &Storage,
    library: &Library,
    device: &DeviceInfo,
    settings: device::GraphSettings,
    comparison: &ComparisonSet,
    bypassed: bool,
    edits: Option<&crate::live_edit::Sessions>,
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
    let mut reference_levels = Vec::new();
    let mut signature = serde_json::to_string(comparison).unwrap_or_default();
    for profile_id in &comparison.profile_ids {
        if profile_id == COMPARISON_BYPASS_ID {
            signature.push_str("|bypass");
            continue;
        }
        let saved = storage.parsed_profile(library, profile_id)?;
        let edit = edits
            .and_then(|edits| edits.get(profile_id))
            .filter(|edit| !edit.expired(std::time::Instant::now()));
        let (profile, trim) = edit
            .map(|edit| (edit.draft.clone(), edit.trim))
            .unwrap_or(saved);
        let mut compiled = compile_profile(
            &profile,
            &CompileOptions {
                sample_rate: settings.sample_rate,
                quantum: settings.quantum,
                output_channels: device.channels,
                manual_trim_db: trim,
                profile_dir: storage.profile_dir(profile_id),
            },
        )?;
        reference_levels.push(perceived_output_level_db(&compiled));
        if let Some(edit) = edit
            && let Some((original, trim)) = &edit.reference
        {
            let reference = compile_reference(
                edit,
                original,
                &CompileOptions {
                    sample_rate: settings.sample_rate,
                    quantum: settings.quantum,
                    output_channels: device.channels,
                    manual_trim_db: *trim,
                    profile_dir: storage.profile_dir(profile_id),
                },
            )?;
            anyhow::ensure!(
                reference.latency_frames == compiled.latency_frames,
                "starting sound has incompatible latency"
            );
            reference_levels.push(perceived_output_level_db(&reference));
            signature.push_str(&format!(
                "|reference={:?}|trim={trim}|hear={}",
                original, edit.hear_reference
            ));
            if edit.hear_reference {
                compiled = reference;
            }
        }
        let perceived = perceived_output_level_db(&compiled);
        signature.push('|');
        signature.push_str(profile_id);
        signature.push('|');
        signature.push_str(&format!("{trim:.9}"));
        signature.push('|');
        signature.push_str(&serde_json::to_string(&profile).unwrap_or_default());
        candidates.push((profile_id.clone(), compiled, perceived));
    }

    anyhow::ensure!(
        !candidates.is_empty(),
        "a comparison bank needs at least one processed profile"
    );

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
    let target_level = reference_levels
        .into_iter()
        .fold(0.0_f64, f64::min)
        .min(0.0);
    // Every candidate runs on the same sink, as wide as the widest candidate.
    let sink_channels = candidates
        .iter()
        .map(|(_, compiled, _)| compiled.input_channels)
        .max()
        .unwrap_or(device.channels);
    let selected_id = if bypassed {
        COMPARISON_BYPASS_ID
    } else {
        comparison.active_profile_id.as_str()
    };
    let mut selected = if selected_id == COMPARISON_BYPASS_ID {
        compile_level_matched_bypass(&candidates[0].1, target_level)?
    } else {
        let (_, mut compiled, perceived) = candidates
            .into_iter()
            .find(|(id, _, _)| id == selected_id)
            .context("active comparison profile is unavailable")?;
        attenuate_for_comparison(&mut compiled, target_level - perceived)?;
        compiled
    };
    widen_input(&mut selected, sink_channels)?;
    selected.latency_frames = latency;
    signature.push_str(&format!(
        "|active={selected_id}|bypassed={bypassed}|target={target_level:.9}|{}|{}|{}",
        settings.sample_rate, settings.quantum, device.channels
    ));
    Ok((selected, stable_hash(&signature)))
}

/// Dry audio at `gain_db`, taking the same input channels as `active` so that
/// switching filters off never rebuilds the sink. A surround sink is downmixed.
fn compile_level_matched_bypass(active: &CompiledProfile, gain_db: f64) -> Result<CompiledProfile> {
    let mut dry = compile_bypass_with_gain(
        active.sample_rate,
        active.quantum,
        active.output_channels,
        gain_db,
    );
    widen_input(&mut dry, active.input_channels)?;
    dry.latency_frames = active.latency_frames;
    Ok(dry)
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

/// A node's volume and mute state as `wpctl get-volume` reports them.
#[derive(Debug, Clone, PartialEq)]
struct NodeVolume {
    volume: String,
    muted: bool,
}

fn parse_node_volume(text: &str) -> Option<NodeVolume> {
    let volume = text
        .trim()
        .strip_prefix("Volume:")?
        .split_whitespace()
        .next()?;
    volume.parse::<f64>().ok()?;
    Some(NodeVolume {
        volume: volume.to_owned(),
        muted: text.contains("[MUTED]"),
    })
}

async fn node_id(node_name: &str) -> Option<u32> {
    let output = Command::new("wpctl")
        .args(["status", "--name"])
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| node_id_from_status_line(line, node_name))
}

async fn node_volumes(names: Vec<String>) -> Vec<(String, NodeVolume)> {
    let mut volumes = Vec::new();
    for name in names {
        if let Some(volume) = node_volume(&name).await {
            volumes.push((name, volume));
        }
    }
    volumes
}

async fn node_volume(node_name: &str) -> Option<NodeVolume> {
    let id = node_id(node_name).await?;
    let output = Command::new("wpctl")
        .args(["get-volume", &id.to_string()])
        .output()
        .await
        .ok()?;
    parse_node_volume(&String::from_utf8_lossy(&output.stdout))
}

async fn set_node_volume(node_name: &str, volume: &NodeVolume) -> Result<()> {
    let id = node_id(node_name)
        .await
        .context("the MassiveEQ sink is not listed")?
        .to_string();
    for args in [
        ["set-volume", id.as_str(), volume.volume.as_str()],
        [
            "set-mute",
            id.as_str(),
            if volume.muted { "1" } else { "0" },
        ],
    ] {
        Command::new("wpctl")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .context("failed to restore the MassiveEQ sink volume")?;
    }
    Ok(())
}

/// Make a MassiveEQ sink the system output.
async fn make_default(node_name: &str) -> Result<()> {
    let id = node_id(node_name)
        .await
        .context("the MassiveEQ sink is not listed yet")?;
    let result = Command::new("wpctl")
        .args(["set-default", &id.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to select the MassiveEQ sink")?;
    anyhow::ensure!(result.success(), "failed to select the MassiveEQ sink");
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
    fn live_drafts_switch_reference_at_one_level_and_expire_to_saved_audio() {
        use crate::live_edit::{LIVE_TIMEOUT, Reference, Request, Session, Sessions};
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = Library::default();
        let profile = storage.create_profile(&mut library, "Live").unwrap();
        let device = DeviceInfo {
            key: DeviceKey {
                backend: "test".into(),
                stable_id: "live".into(),
                route: "stereo".into(),
            },
            node_name: "live".into(),
            description: "Live".into(),
            channels: 2,
            connected: true,
            assigned_profile: Some(profile.id.clone()),
            bypassed: false,
            is_default: true,
        };
        library
            .assignments
            .insert(device.key.as_storage_key(), profile.id.clone());
        let settings = device::GraphSettings {
            sample_rate: 48000,
            quantum: 1024,
        };
        let session = Session::parse(
            &storage,
            &library,
            &profile.id,
            Request {
                text: "Filter: ON LS Fc 100 Hz Gain 8 dB Q 0.707".into(),
                trim: 0.0,
                reference: Some(Reference {
                    text: String::new(),
                    trim: 0.0,
                }),
                hear_reference: false,
            },
        )
        .unwrap();
        let mut edits = Sessions::from([(profile.id.clone(), session)]);
        let (current, revision) =
            compile_output_with_edits(&storage, &library, &device, settings, &edits).unwrap();
        edits.get_mut(&profile.id).unwrap().hear_reference = true;
        let (original, original_revision) =
            compile_output_with_edits(&storage, &library, &device, settings, &edits).unwrap();
        assert_ne!(revision, original_revision);
        assert_eq!(current.latency_frames, original.latency_frames);
        assert!(
            (perceived_output_level_db(&current) - perceived_output_level_db(&original)).abs()
                < 1e-9
        );
        assert!(current.analysis.final_peak_db <= -0.999);
        assert!(original.analysis.final_peak_db <= -0.999);
        assert!(
            storage
                .parsed_profile(&library, &profile.id)
                .unwrap()
                .0
                .filters
                .is_empty()
        );
        edits.get_mut(&profile.id).unwrap().renewed -= LIVE_TIMEOUT;
        let (expired, _) =
            compile_output_with_edits(&storage, &library, &device, settings, &edits).unwrap();
        let (saved, _) = compile_output_state(&storage, &library, &device, settings).unwrap();
        assert_eq!(
            expired.channels[0].gain_linear,
            saved.channels[0].gain_linear
        );
        library.global_bypass = true;
        let (dry, _) =
            compile_output_with_edits(&storage, &library, &device, settings, &edits).unwrap();
        assert_eq!(dry.channels[0].gain_linear, 1.0);
    }
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
    fn output_ab_preserves_the_assigned_profiles_perceived_level() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = Library::default();
        let profile = storage
            .create_profile(&mut library, "A/B reference")
            .unwrap();
        storage
            .put_profile(
                &mut library,
                &profile.id,
                "A/B reference",
                "Filter: ON LSC Fc 90 Hz Gain 8 dB Q 0.707\nFilter: ON PK Fc 3500 Hz Gain -5 dB Q 1.4",
                0.0,
            )
            .unwrap();
        let device = DeviceInfo {
            key: DeviceKey {
                backend: "test".into(),
                stable_id: "headphones".into(),
                route: "stereo".into(),
            },
            node_name: "test.headphones".into(),
            description: "Headphones".into(),
            channels: 2,
            connected: true,
            assigned_profile: Some(profile.id.clone()),
            bypassed: false,
            is_default: false,
        };
        library
            .assignments
            .insert(device.key.as_storage_key(), profile.id);
        let settings = device::GraphSettings {
            sample_rate: 48_000,
            quantum: 1_024,
        };
        let (filtered, filtered_revision) =
            compile_runtime_profile(&storage, &library, &device, settings, false).unwrap();
        let (dry, dry_revision) =
            compile_runtime_profile(&storage, &library, &device, settings, true).unwrap();

        assert_ne!(filtered_revision, dry_revision);
        assert_eq!(filtered.latency_frames, dry.latency_frames);
        assert!(
            (perceived_output_level_db(&filtered) - perceived_output_level_db(&dry)).abs() < 1e-9
        );
    }

    #[test]
    fn engine_off_is_true_zero_db_dry_audio() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().join("data"), temp.path().join("config")).unwrap();
        let mut library = Library::default();
        let profile = storage.create_profile(&mut library, "Loud EQ").unwrap();
        storage
            .put_profile(
                &mut library,
                &profile.id,
                "Loud EQ",
                "Filter: ON PK Fc 1000 Hz Gain 12 dB Q 1",
                0.0,
            )
            .unwrap();
        let device = DeviceInfo {
            key: DeviceKey {
                backend: "test".into(),
                stable_id: "headphones".into(),
                route: "stereo".into(),
            },
            node_name: "test.headphones".into(),
            description: "Headphones".into(),
            channels: 2,
            connected: true,
            assigned_profile: Some(profile.id.clone()),
            bypassed: true,
            is_default: false,
        };
        library
            .assignments
            .insert(device.key.as_storage_key(), profile.id);
        library.global_bypass = true;
        let (dry, _) = compile_output_state(
            &storage,
            &library,
            &device,
            device::GraphSettings {
                sample_rate: 48_000,
                quantum: 1_024,
            },
        )
        .unwrap();

        assert_eq!(dry.latency_frames, 0);
        assert_eq!(perceived_output_level_db(&dry), 0.0);
        assert!(dry.channels.iter().all(|channel| {
            channel.gain_linear == 1.0
                && channel.biquads.is_empty()
                && channel.convolutions.is_empty()
        }));
    }

    #[test]
    fn level_matched_convolution_bypass_preserves_endpoint_latency() {
        let mut active = compile_bypass_with_gain(48_000, 128, 2, -2.0);
        for channel in &mut active.channels {
            channel.convolutions.push(massiveeq_dsp::ConvolutionKernel {
                sample_rate: 48_000,
                impulse: vec![0.0; 18],
                latency_frames: 145,
            });
        }
        active.latency_frames = 145;

        let dry = compile_level_matched_bypass(&active, -2.0).unwrap();
        assert_eq!(dry.latency_frames, 145);
        assert!(
            dry.channels
                .iter()
                .all(|channel| { channel.biquads.is_empty() && channel.convolutions.is_empty() })
        );
    }

    #[test]
    fn parses_wpctl_volume() {
        assert_eq!(
            super::parse_node_volume("Volume: 0.42 [MUTED]\n"),
            Some(super::NodeVolume {
                volume: "0.42".into(),
                muted: true
            })
        );
        assert_eq!(
            super::parse_node_volume("Volume: 1.00").map(|volume| volume.muted),
            Some(false)
        );
        assert_eq!(super::parse_node_volume("Error"), None);
    }

    #[test]
    fn filters_off_on_a_surround_profile_keeps_the_71_sink() {
        let mut active = compile_bypass_with_gain(48_000, 128, 2, -2.0);
        active.input_channels = 8;
        let dry = compile_level_matched_bypass(&active, -2.0).unwrap();
        assert_eq!(dry.input_channels, 8);
        assert!(dry.downmix.is_some());
        assert!(dry.routes.is_empty());
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
            is_default: false,
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
