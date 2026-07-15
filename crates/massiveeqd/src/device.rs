use anyhow::{Context, Result};
use massiveeq_core::{DeviceInfo, DeviceKey, Library, RememberedDevice};
use serde_json::{Map, Value};
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphSettings {
    pub sample_rate: u32,
    pub quantum: u32,
}

pub async fn graph_settings() -> GraphSettings {
    let output = Command::new("pw-metadata")
        .args(["-n", "settings", "0"])
        .output()
        .await;
    let text = output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    GraphSettings {
        sample_rate: metadata_number(&text, "clock.rate").unwrap_or(48_000),
        // Capacity must cover a runtime quantum increase without allocating in
        // the audio callback. Recompilation later reduces this after a graph
        // setting change is observed.
        quantum: metadata_number(&text, "clock.max-quantum")
            .or_else(|| metadata_number(&text, "clock.quantum"))
            .unwrap_or(2_048),
    }
}

pub async fn discover(library: &mut Library) -> Result<Vec<DeviceInfo>> {
    let output = Command::new("pw-dump")
        .output()
        .await
        .context("could not run pw-dump")?;
    if !output.status.success() {
        anyhow::bail!("pw-dump exited with {}", output.status);
    }
    let objects: Vec<Value> =
        serde_json::from_slice(&output.stdout).context("invalid pw-dump JSON")?;
    let mut devices = Vec::new();
    for object in objects {
        if object.get("type").and_then(Value::as_str) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let Some(props) = object.pointer("/info/props").and_then(Value::as_object) else {
            continue;
        };
        if string(props, "media.class") != Some("Audio/Sink") {
            continue;
        }
        let node_name = string(props, "node.name").unwrap_or_default().to_owned();
        if node_name.starts_with("massiveeq.") {
            continue;
        }
        let backend = string(props, "device.api").unwrap_or("unknown").to_owned();
        let (stable_id, route) = if backend == "bluez5" {
            (
                string(props, "api.bluez5.address")
                    .or_else(|| string(props, "device.string"))
                    .unwrap_or(&node_name)
                    .to_owned(),
                string(props, "api.bluez5.profile")
                    .unwrap_or("playback")
                    .to_owned(),
            )
        } else {
            (
                string(props, "device.serial")
                    .or_else(|| string(props, "device.bus-path"))
                    .or_else(|| string(props, "object.path"))
                    .unwrap_or(&node_name)
                    .to_owned(),
                string(props, "device.profile.name")
                    .or_else(|| string(props, "api.alsa.path"))
                    .unwrap_or("playback")
                    .to_owned(),
            )
        };
        let key = DeviceKey {
            backend,
            stable_id,
            route,
        };
        let storage_key = key.as_storage_key();
        let channels = integer(props, "audio.channels")
            .unwrap_or_else(|| channel_count(string(props, "audio.position")))
            as u32;
        devices.push(DeviceInfo {
            key,
            node_name,
            description: string(props, "node.description")
                .or_else(|| string(props, "media.name"))
                .unwrap_or("Audio output")
                .to_owned(),
            channels: channels.max(1),
            connected: true,
            assigned_profile: library.assignments.get(&storage_key).cloned(),
            bypassed: library.bypassed_devices.contains(&storage_key),
        });
    }
    let connected_keys = devices
        .iter()
        .map(|device| device.key.as_storage_key())
        .collect::<std::collections::HashSet<_>>();
    for device in &devices {
        library
            .remembered_devices
            .insert(device.key.as_storage_key(), RememberedDevice::from(device));
    }
    let legacy_keys = library
        .assignments
        .keys()
        .chain(library.bypassed_devices.iter())
        .filter(|key| !library.remembered_devices.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    for storage_key in legacy_keys {
        if let Some(remembered) = remembered_from_storage_key(&storage_key) {
            library.remembered_devices.insert(storage_key, remembered);
        }
    }
    devices.extend(
        library
            .remembered_devices
            .iter()
            .filter(|(key, _)| !connected_keys.contains(*key))
            .map(|(storage_key, remembered)| DeviceInfo {
                key: remembered.key.clone(),
                node_name: remembered.node_name.clone(),
                description: remembered.description.clone(),
                channels: remembered.channels,
                connected: false,
                assigned_profile: library.assignments.get(storage_key).cloned(),
                bypassed: library.bypassed_devices.contains(storage_key),
            }),
    );
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.description.cmp(&b.description))
    });
    Ok(devices)
}

fn remembered_from_storage_key(storage_key: &str) -> Option<RememberedDevice> {
    let mut parts = storage_key.splitn(3, '|');
    let backend = parts.next()?.to_owned();
    let stable_id = parts.next()?.to_owned();
    let route = parts.next()?.to_owned();
    if backend.is_empty() || stable_id.is_empty() || route.is_empty() {
        return None;
    }
    let description = if backend == "bluez5" {
        format!("Previously used Bluetooth output · {stable_id}")
    } else {
        format!("Previously used audio output · {stable_id}")
    };
    Some(RememberedDevice {
        key: DeviceKey {
            backend,
            stable_id,
            route,
        },
        node_name: String::new(),
        description,
        channels: 2,
    })
}

fn string<'a>(props: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    props.get(key).and_then(Value::as_str)
}
fn integer(props: &Map<String, Value>, key: &str) -> Option<u64> {
    props
        .get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
}
fn channel_count(position: Option<&str>) -> u64 {
    position
        .map(|value| {
            value
                .trim_matches(|c| c == '[' || c == ']')
                .split_whitespace()
                .count() as u64
        })
        .filter(|v| *v > 0)
        .unwrap_or(2)
}

fn metadata_number(text: &str, key: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        line.contains(&format!("key:'{key}'"))
            .then(|| {
                line.split("value:'")
                    .nth(1)?
                    .split('\'')
                    .next()?
                    .trim()
                    .parse()
                    .ok()
            })
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_graph_settings_metadata() {
        let text = "update: id:0 key:'clock.rate' value:'96000' type:''\nupdate: id:0 key:'clock.max-quantum' value:'4096' type:''";
        assert_eq!(metadata_number(text, "clock.rate"), Some(96_000));
        assert_eq!(metadata_number(text, "clock.max-quantum"), Some(4_096));
    }

    #[test]
    fn preserves_legacy_bluetooth_assignment_as_a_remembered_output() {
        let remembered = remembered_from_storage_key("bluez5|6C:12:70:4B:F7:C9|a2dp-sink").unwrap();
        assert_eq!(remembered.key.backend, "bluez5");
        assert_eq!(remembered.key.route, "a2dp-sink");
        assert!(remembered.description.contains("Bluetooth"));
    }
}
