use anyhow::{Context, Result};
use massiveeq_core::{
    Channel, DeviceInfo, Library, ProfileAnalysis, ProfileDocument, Storage, analyze_profile,
    pipewire::build_filter_config,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    process::Stdio,
};
use tokio::{
    fs,
    process::{Child, Command},
    time::{Duration, sleep},
};

pub struct FilterHost {
    children: HashMap<String, ActiveFilter>,
    runtime_dir: PathBuf,
}

struct ActiveFilter {
    child: Child,
    topology: String,
}

impl FilterHost {
    pub async fn new() -> Result<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("massiveeq");
        fs::create_dir_all(&runtime).await?;
        Ok(Self {
            children: HashMap::new(),
            runtime_dir: runtime,
        })
    }

    pub async fn stop_all(&mut self) {
        for (_, mut active) in self.children.drain() {
            let _ = active.child.kill().await;
        }
    }

    pub async fn reconcile(
        &mut self,
        storage: &Storage,
        library: &Library,
        devices: &[DeviceInfo],
        force_restart: bool,
    ) -> Result<()> {
        let exited = self
            .children
            .iter_mut()
            .filter_map(|(key, active)| match active.child.try_wait() {
                Ok(Some(_)) | Err(_) => Some(key.clone()),
                Ok(None) => None,
            })
            .collect::<Vec<_>>();
        for key in exited {
            self.children.remove(&key);
        }
        let desired = devices
            .iter()
            .filter(|device| {
                device.connected
                    && device.channels <= 2
                    && !device.bypassed
                    && device
                        .assigned_profile
                        .as_ref()
                        .is_some_and(|id| library.profiles.contains_key(id))
            })
            .map(|device| device.key.as_storage_key())
            .collect::<HashSet<_>>();

        let stale = self
            .children
            .keys()
            .filter(|key| !desired.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in stale {
            if let Some(mut active) = self.children.remove(&key) {
                let _ = active.child.kill().await;
            }
        }

        for device in devices {
            let key = device.key.as_storage_key();
            if !desired.contains(&key) {
                continue;
            }
            let profile_id = device.assigned_profile.as_ref().expect("desired profile");
            let (profile, trim) = storage.parsed_profile(library, profile_id)?;
            if !profile.is_activatable() {
                continue;
            }
            let analysis = analyze_profile(&profile, 48_000.0, trim);
            let topology = topology_signature(&profile);
            let filter_node_name = format!("massiveeq.{:x}", stable_hash(&key));

            if let Some(active) = self.children.get(&key) {
                let _ =
                    restore_physical_default_if_needed(&filter_node_name, &device.node_name).await;
                if !force_restart {
                    continue;
                }
                if active.topology == topology
                    && update_live_params(&key, &profile, &analysis).await.is_ok()
                {
                    continue;
                }
            }

            if let Some(mut active) = self.children.remove(&key) {
                let _ = active.child.kill().await;
            }
            let config = build_filter_config(
                &profile,
                &analysis,
                device,
                &storage.profile_dir(profile_id),
                48_000,
            );
            let config_path = self
                .runtime_dir
                .join(format!("{:x}.conf", stable_hash(&key)));
            fs::write(&config_path, config).await?;
            let child = Command::new("pipewire")
                .arg("-c")
                .arg(&config_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .with_context(|| format!("failed to start filter for {}", device.description))?;
            let child_pid = child
                .id()
                .context("PipeWire filter process did not expose a process ID")?;
            set_filter_volume_unity(child_pid).await?;
            let _ = restore_physical_default_if_needed(&filter_node_name, &device.node_name).await;
            self.children.insert(key, ActiveFilter { child, topology });
        }
        Ok(())
    }

    pub fn active_count(&self) -> usize {
        self.children.len()
    }
}

async fn set_filter_volume_unity(pid: u32) -> Result<()> {
    // WirePlumber can restore a stale volume for the filter's virtual sink.
    // That volume is then multiplied by the physical device volume, making
    // an assigned output much quieter than the same device in fail-open mode.
    // Give the newly exported nodes time to appear, then keep both halves of
    // this filter process at unity so it adds no second volume stage. The
    // physical device's persistent volume is left untouched.
    sleep(Duration::from_millis(250)).await;
    let status = Command::new("wpctl")
        .arg("set-volume")
        .arg("--pid")
        .arg(pid.to_string())
        .arg("1.0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to normalize the MassiveEQ filter volume")?;
    anyhow::ensure!(status.success(), "failed to set the filter volume to unity");
    Ok(())
}

async fn restore_physical_default_if_needed(
    filter_node_name: &str,
    target_node_name: &str,
) -> Result<()> {
    let output = Command::new("wpctl")
        .arg("status")
        .arg("--name")
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
        .arg("set-default")
        .arg(target_id.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to restore the physical default audio output")?;
    anyhow::ensure!(
        result.success(),
        "failed to restore the physical audio output"
    );
    Ok(())
}

fn node_id_from_status_line(line: &str, node_name: &str) -> Option<u32> {
    line.contains(node_name).then(|| {
        line.split_whitespace()
            .find_map(|token| token.strip_suffix('.')?.parse().ok())
    })?
}

fn topology_signature(profile: &ProfileDocument) -> String {
    let mut signature = String::new();
    for channel in [Channel::Left, Channel::Right] {
        signature.push_str(if channel == Channel::Left { "L:" } else { "R:" });
        for filter in profile.filters_for(channel) {
            signature.push_str(filter.kind.pipewire_label());
            signature.push(',');
        }
        signature.push('|');
    }
    // GraphicEQ and convolution kernels cannot be changed through the live
    // biquad controls, so any change to them intentionally changes topology.
    signature.push_str(&serde_json::to_string(&profile.graphic_eqs).unwrap_or_default());
    signature.push('|');
    signature.push_str(&serde_json::to_string(&profile.convolutions).unwrap_or_default());
    signature
}

fn live_params(profile: &ProfileDocument, analysis: &ProfileAnalysis) -> String {
    let mut values = Vec::new();
    for channel in [Channel::Left, Channel::Right] {
        let suffix = if channel == Channel::Left { "l" } else { "r" };
        let gain = 10.0_f64.powf((profile.preamp_for(channel) + analysis.effective_gain_db) / 20.0);
        values.push(format!(r#""gain_{suffix}:Mult" {gain:.10}"#));
        for (index, filter) in profile.filters_for(channel).enumerate() {
            values.push(format!(
                r#""eq_{suffix}_{index}:Freq" {:.10}"#,
                filter.frequency
            ));
            values.push(format!(r#""eq_{suffix}_{index}:Q" {:.10}"#, filter.q));
            values.push(format!(
                r#""eq_{suffix}_{index}:Gain" {:.10}"#,
                filter.gain_db
            ));
            let (wet, dry) = if filter.enabled {
                (1.0, 0.0)
            } else {
                (0.0, 1.0)
            };
            values.push(format!(r#""mix_{suffix}_{index}:Gain 1" {wet:.10}"#));
            values.push(format!(r#""mix_{suffix}_{index}:Gain 2" {dry:.10}"#));
        }
    }
    format!("{{ params = [ {} ] }}", values.join(" "))
}

async fn update_live_params(
    key: &str,
    profile: &ProfileDocument,
    analysis: &ProfileAnalysis,
) -> Result<()> {
    let node_name = format!("massiveeq.{:x}", stable_hash(key));
    let status = Command::new("pw-cli")
        .arg("set-param")
        .arg(node_name)
        .arg("Props")
        .arg(live_params(profile, analysis))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("failed to run pw-cli for a live filter update")?;
    anyhow::ensure!(status.success(), "PipeWire rejected a live filter update");
    Ok(())
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
    fn value_and_enable_edits_keep_topology() {
        let first = parse_text(
            "test",
            "Filter 1: ON PK Fc 100 Hz Gain 3 dB Q 1\nFilter 2: ON PK Fc 1 kHz Gain -2 dB Q 2",
        );
        let values_changed = parse_text(
            "test",
            "Filter 1: ON PK Fc 120 Hz Gain 4 dB Q 1.2\nFilter 2: ON PK Fc 2 kHz Gain -1 dB Q 3",
        );
        let kind_changed = parse_text(
            "test",
            "Filter 1: ON LSC Fc 120 Hz Gain 4 dB Q 1.2\nFilter 2: ON PK Fc 2 kHz Gain -1 dB Q 3",
        );
        let disabled = parse_text(
            "test",
            "Filter 1: OFF PK Fc 100 Hz Gain 3 dB Q 1\nFilter 2: ON PK Fc 1 kHz Gain -2 dB Q 2",
        );
        let slot_added = parse_text(
            "test",
            "Filter 1: ON PK Fc 100 Hz Gain 3 dB Q 1\nFilter 2: ON PK Fc 1 kHz Gain -2 dB Q 2\nFilter 3: ON PK Fc 3 kHz Gain 1 dB Q 1",
        );
        assert_eq!(
            topology_signature(&first),
            topology_signature(&values_changed)
        );
        assert_ne!(
            topology_signature(&first),
            topology_signature(&kind_changed)
        );
        assert_eq!(topology_signature(&first), topology_signature(&disabled));
        assert_ne!(topology_signature(&first), topology_signature(&slot_added));
    }

    #[test]
    fn live_update_contains_both_channel_controls() {
        let profile = parse_text("test", "Filter 1: ON PK Fc 1000 Hz Gain 3 dB Q 1");
        let analysis = analyze_profile(&profile, 48_000.0, 0.0);
        let params = live_params(&profile, &analysis);
        assert!(params.contains("gain_l:Mult"));
        assert!(params.contains("eq_l_0:Freq"));
        assert!(params.contains("eq_r_0:Gain"));
    }

    #[test]
    fn disabled_filter_uses_the_dry_mixer_path() {
        let profile = parse_text("test", "Filter 1: OFF HPQ Fc 100 Hz Q 1");
        let analysis = analyze_profile(&profile, 48_000.0, 0.0);
        let params = live_params(&profile, &analysis);
        assert!(params.contains(r#""eq_l_0:Freq" 100.0000000000"#));
        assert!(params.contains(r#""mix_l_0:Gain 1" 0.0000000000"#));
        assert!(params.contains(r#""mix_l_0:Gain 2" 1.0000000000"#));
        assert!(!params.contains(":b0"));
    }

    #[test]
    fn extracts_the_physical_node_id_from_wpctl_status() {
        let line = " │  *   97. bluez_output.AA_BB.1    [vol: 0.43]";
        assert_eq!(
            node_id_from_status_line(line, "bluez_output.AA_BB.1"),
            Some(97)
        );
        assert_eq!(node_id_from_status_line(line, "massiveeq.test"), None);
    }
}
