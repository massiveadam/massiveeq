use crate::{Channel, ChannelSelection, DeviceInfo, ProfileAnalysis, ProfileDocument};
use std::{f64::consts::PI, path::Path};
#[cfg(unix)]
use std::{ffi::CString, os::unix::ffi::OsStrExt};

/// Build a standalone PipeWire client configuration containing one smart
/// filter targeted at a physical output node. PipeWire owns the real-time
/// execution; MassiveEQ can remove the client at any time to fail open.
pub fn build_filter_config(
    profile: &ProfileDocument,
    analysis: &ProfileAnalysis,
    device: &DeviceInfo,
    profile_dir: &Path,
    sample_rate: u32,
) -> String {
    let mut nodes = Vec::new();
    let mut links = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();

    for (channel_index, channel) in [(0, Channel::Left), (1, Channel::Right)] {
        let suffix = if channel == Channel::Left { "l" } else { "r" };
        let total_gain_db = profile.preamp_for(channel) + analysis.effective_gain_db;
        let gain_name = format!("gain_{suffix}");
        nodes.push(format!(r#"{{ type = builtin name = {gain_name} label = linear control = {{ "Mult" = {:.10} "Add" = 0.0 }} }}"#, db_to_linear(total_gain_db)));
        inputs.push(format!("\"{gain_name}:In\""));
        let mut previous = gain_name;

        for (filter_index, filter) in profile.filters_for(channel).enumerate() {
            let name = format!("eq_{suffix}_{filter_index}");
            let mixer_name = format!("mix_{suffix}_{filter_index}");
            let (wet, dry) = if filter.enabled {
                (1.0, 0.0)
            } else {
                (0.0, 1.0)
            };
            nodes.push(format!(
                "{{ type = builtin name = {name} label = {} control = {{ \"Freq\" = {:.10} \"Q\" = {:.10} \"Gain\" = {:.10} }} }}",
                filter.kind.pipewire_label(),
                filter.frequency,
                filter.q,
                filter.gain_db,
            ));
            nodes.push(format!(
                "{{ type = builtin name = {mixer_name} label = mixer control = {{ \"Gain 1\" = {wet:.1} \"Gain 2\" = {dry:.1} }} }}"
            ));
            links.push(format!(
                "{{ output = \"{previous}:Out\" input = \"{name}:In\" }}"
            ));
            links.push(format!(
                "{{ output = \"{name}:Out\" input = \"{mixer_name}:In 1\" }}"
            ));
            links.push(format!(
                "{{ output = \"{previous}:Out\" input = \"{mixer_name}:In 2\" }}"
            ));
            previous = mixer_name;
        }

        let graphics = profile
            .graphic_eqs
            .iter()
            .filter(|eq| eq.channels.contains(channel))
            .collect::<Vec<_>>();
        if !graphics.is_empty() {
            let name = format!("graphic_{suffix}");
            let impulse = graphic_impulse(&graphics, sample_rate, 2048);
            let inline = format!(
                "/ir:{sample_rate},{}",
                impulse
                    .iter()
                    .map(|value| format!("{value:.9}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            nodes.push(format!("{{ type = builtin name = {name} label = convolver config = {{ filename = {} blocksize = 128 resample_quality = 6 }} }}", spa_quote(&inline)));
            links.push(format!(
                "{{ output = \"{previous}:Out\" input = \"{name}:In\" }}"
            ));
            previous = name;
        }

        for (convolution_index, convolution) in profile
            .convolutions
            .iter()
            .filter(|conv| conv.channels.contains(channel))
            .enumerate()
        {
            let name = format!("conv_{suffix}_{convolution_index}");
            let path = if convolution.path.is_absolute() {
                convolution.path.clone()
            } else {
                profile_dir.join(&convolution.path)
            };
            let ir_channels = audio_file_info(&path)
                .map(|(_, _, channels)| channels)
                .unwrap_or(1)
                .max(1);
            let ir_channel = match convolution.channels {
                ChannelSelection::All => channel_index % ir_channels,
                ChannelSelection::Left | ChannelSelection::Right => 0,
            };
            nodes.push(format!("{{ type = builtin name = {name} label = convolver config = {{ filename = {} channel = {ir_channel} blocksize = 128 resample_quality = 6 }} }}", spa_quote(&path.display().to_string())));
            links.push(format!(
                "{{ output = \"{previous}:Out\" input = \"{name}:In\" }}"
            ));
            previous = name;
        }
        outputs.push(format!("\"{previous}:Out\""));
    }

    let safe_name = device
        .key
        .as_storage_key()
        .bytes()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ byte as u64).wrapping_mul(0x100000001b3)
        });
    format!(
        r#"
context.properties = {{ log.level = 0 }}
context.spa-libs = {{
  audio.convert.* = audioconvert/libspa-audioconvert
  support.* = support/libspa-support
}}
context.modules = [
  {{ name = libpipewire-module-rt flags = [ ifexists nofail ] }}
  {{ name = libpipewire-module-protocol-native }}
  {{ name = libpipewire-module-client-node }}
  {{ name = libpipewire-module-adapter }}
  {{ name = libpipewire-module-filter-chain
     args = {{
       node.description = {description}
       filter.graph = {{
         nodes = [ {nodes} ]
         links = [ {links} ]
         inputs = [ {inputs} ]
         outputs = [ {outputs} ]
       }}
       capture.props = {{
         node.name = "massiveeq.{safe_name:x}"
         media.class = Audio/Sink
         audio.channels = 2
         audio.position = [ FL FR ]
         filter.smart = true
         filter.smart.name = "massiveeq-{safe_name:x}"
         filter.smart.target = {{ node.name = {target} }}
       }}
       playback.props = {{
         node.name = "massiveeq.output.{safe_name:x}"
         node.passive = true
         stream.dont-remix = true
         audio.channels = 2
         audio.position = [ FL FR ]
       }}
     }}
  }}
]
"#,
        description = spa_quote(&format!("MassiveEQ — {}", device.description)),
        target = spa_quote(&device.node_name),
        nodes = nodes.join("\n"),
        links = links.join("\n"),
        inputs = inputs.join(" "),
        outputs = outputs.join(" "),
    )
}

fn graphic_impulse(eqs: &[&crate::GraphicEq], sample_rate: u32, taps: usize) -> Vec<f64> {
    let half = taps / 2;
    let mut magnitude = vec![1.0; half + 1];
    for (bin, value) in magnitude.iter_mut().enumerate() {
        let frequency = bin as f64 * sample_rate as f64 / taps as f64;
        let gain = eqs
            .iter()
            .map(|eq| graphic_gain(eq, frequency))
            .sum::<f64>();
        *value = 10.0_f64.powf(gain / 20.0);
    }
    let mut zero_phase = vec![0.0; taps];
    for (sample, output) in zero_phase.iter_mut().enumerate() {
        let mut value = magnitude[0] + magnitude[half] * if sample % 2 == 0 { 1.0 } else { -1.0 };
        for (bin, mag) in magnitude.iter().enumerate().take(half).skip(1) {
            value += 2.0 * mag * (2.0 * PI * bin as f64 * sample as f64 / taps as f64).cos();
        }
        *output = value / taps as f64;
    }
    let mut causal = vec![0.0; taps];
    for index in 0..taps {
        causal[(index + half) % taps] = zero_phase[index];
    }
    causal
}

fn graphic_gain(eq: &crate::GraphicEq, frequency: f64) -> f64 {
    if frequency <= 0.0
        || frequency < eq.points[0].frequency
        || frequency > eq.points[eq.points.len() - 1].frequency
    {
        return 0.0;
    }
    for pair in eq.points.windows(2) {
        if frequency >= pair[0].frequency && frequency <= pair[1].frequency {
            let t =
                (frequency / pair[0].frequency).ln() / (pair[1].frequency / pair[0].frequency).ln();
            return pair[0].gain_db + t * (pair[1].gain_db - pair[0].gain_db);
        }
    }
    0.0
}

fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

fn spa_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(unix)]
pub fn audio_file_info(path: &Path) -> Option<(i64, i32, usize)> {
    #[repr(C)]
    struct SfInfo {
        frames: i64,
        samplerate: i32,
        channels: i32,
        format: i32,
        sections: i32,
        seekable: i32,
    }
    #[link(name = "sndfile")]
    unsafe extern "C" {
        fn sf_open(
            path: *const std::ffi::c_char,
            mode: i32,
            info: *mut SfInfo,
        ) -> *mut std::ffi::c_void;
        fn sf_close(file: *mut std::ffi::c_void) -> i32;
    }
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut info = SfInfo {
        frames: 0,
        samplerate: 0,
        channels: 0,
        format: 0,
        sections: 0,
        seekable: 0,
    };
    // SAFETY: libsndfile receives a NUL-terminated path and a valid, writable
    // SF_INFO structure. The handle is closed immediately after inspection.
    let handle = unsafe { sf_open(path.as_ptr(), 0x10, &mut info) };
    if handle.is_null() {
        return None;
    }
    // SAFETY: handle was returned by sf_open and has not yet been closed.
    let _ = unsafe { sf_close(handle) };
    (info.channels > 0 && info.samplerate > 0).then_some((
        info.frames,
        info.samplerate,
        info.channels as usize,
    ))
}

#[cfg(not(unix))]
pub fn audio_file_info(_: &Path) -> Option<(i64, i32, usize)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceKey, analyze_profile, parse_text};

    #[test]
    fn generated_filter_targets_device_and_has_two_channels() {
        let profile = parse_text(
            "AirPods",
            "Preamp: -4 dB\nFilter: ON PK Fc 1000 Hz Gain 3 dB Q 1",
        );
        let analysis = analyze_profile(&profile, 48000.0, 0.0);
        let device = DeviceInfo {
            key: DeviceKey {
                backend: "bluez5".into(),
                stable_id: "AA:BB".into(),
                route: "a2dp-sink".into(),
            },
            node_name: "bluez_output.test".into(),
            description: "AirPods".into(),
            channels: 2,
            connected: true,
            assigned_profile: None,
            bypassed: false,
        };
        let config = build_filter_config(
            &profile,
            &analysis,
            &device,
            Path::new("/tmp/profile"),
            48000,
        );
        assert!(config.contains("bluez_output.test"));
        assert!(config.contains("bq_peaking"));
        assert!(config.contains("label = mixer"));
        assert!(config.contains("audio.channels = 2"));
    }

    #[test]
    fn disabled_filter_uses_a_dry_mixer_path_and_valid_biquad_controls() {
        let profile = parse_text("AirPods", "Filter: OFF HPQ Fc 100 Hz Q 1");
        let analysis = analyze_profile(&profile, 48000.0, 0.0);
        let device = DeviceInfo {
            key: DeviceKey {
                backend: "bluez5".into(),
                stable_id: "AA:BB".into(),
                route: "a2dp-sink".into(),
            },
            node_name: "bluez_output.test".into(),
            description: "AirPods".into(),
            channels: 2,
            connected: true,
            assigned_profile: None,
            bypassed: false,
        };
        let config = build_filter_config(
            &profile,
            &analysis,
            &device,
            Path::new("/tmp/profile"),
            48000,
        );

        assert!(config.contains("label = bq_highpass control = { \"Freq\" = 100.0000000000"));
        assert!(config.contains(
            "name = mix_l_0 label = mixer control = { \"Gain 1\" = 0.0 \"Gain 2\" = 1.0 }"
        ));
        assert!(!config.contains("\"b0\""));
    }
}
