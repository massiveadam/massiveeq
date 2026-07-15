use crate::{device, native_filter::NativeFilter};
use anyhow::{Context, Result};
use massiveeq_core::{DeviceInfo, Library, Storage};
use massiveeq_dsp::{CompileOptions, ProfileProcessor, compile_bypass, compile_profile};
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
                    && device
                        .assigned_profile
                        .as_ref()
                        .is_some_and(|id| library.profiles.contains_key(id))
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
            let profile_id = device.assigned_profile.as_ref().expect("desired profile");
            let (profile, trim) = match storage.parsed_profile(library, profile_id) {
                Ok(value) => value,
                Err(error) => {
                    self.errors.insert(key, error.to_string());
                    continue;
                }
            };
            let bypassed = device.bypassed || library.global_bypass;
            let revision = profile_revision(
                &profile,
                trim,
                bypassed,
                settings.sample_rate,
                settings.quantum,
                device.channels,
            );
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

            let compiled = if bypassed {
                compile_bypass(settings.sample_rate, settings.quantum, device.channels)
            } else {
                let options = CompileOptions {
                    sample_rate: settings.sample_rate,
                    quantum: settings.quantum,
                    output_channels: device.channels,
                    manual_trim_db: trim,
                    profile_dir: storage.profile_dir(profile_id),
                };
                match compile_profile(&profile, &options) {
                    Ok(compiled) => compiled,
                    Err(error) => {
                        self.errors.insert(key, error.to_string());
                        // Transactional activation: a bad candidate never
                        // replaces the last chain that is already playing.
                        continue;
                    }
                }
            };
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
    use massiveeq_core::parse_text;

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
}
