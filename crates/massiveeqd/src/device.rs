use anyhow::{Context, Result};
use massiveeq_core::{DeviceInfo, DeviceKey, Library};
use serde_json::{Map, Value};
use tokio::process::Command;

pub async fn discover(library: &Library) -> Result<Vec<DeviceInfo>> {
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
            bypassed: library.global_bypass || library.bypassed_devices.contains(&storage_key),
        });
    }
    devices.sort_by(|a, b| a.description.cmp(&b.description));
    Ok(devices)
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
