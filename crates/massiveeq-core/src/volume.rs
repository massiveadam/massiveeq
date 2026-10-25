use serde::{Deserialize, Serialize};

/// Levels from 100% down to 1% span this many decibels, evenly: each 1% is
/// half a decibel, so a 5% volume-key press is always 2.5 dB.
pub const VOLUME_RANGE_DB: f64 = 50.0;

/// A raise of more than this much of the volume scale in one change is a
/// jump: an app or a Bluetooth device pushing the volume up, not a key press.
pub const VOLUME_JUMP_LIMIT: f64 = 0.10;

/// How an output's volume is handled.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VolumeSettings {
    /// MassiveEQ's sink is the system output and applies its volume in dB
    /// steps in the DSP, leaving the device's own volume where it is.
    #[serde(default)]
    pub in_dsp: bool,
    /// Undo sudden raises of the output's volume.
    #[serde(default)]
    pub block_jumps: bool,
    /// The last volume, 0 to 1 as desktops show it, restored on restart.
    #[serde(default = "full_level")]
    pub level: f64,
    /// The device's own volume while MassiveEQ was stopped and had turned
    /// the device down in its place; put back when MassiveEQ returns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_ceiling: Option<f64>,
}

fn full_level() -> f64 {
    1.0
}

impl Default for VolumeSettings {
    fn default() -> Self {
        Self {
            in_dsp: false,
            block_jumps: false,
            level: full_level(),
            device_ceiling: None,
        }
    }
}

/// The volume scale value desktops show for a linear PipeWire channel
/// volume, which is the cube of it.
pub fn volume_level(channel_volume: f64) -> f64 {
    channel_volume.max(0.0).cbrt()
}

/// Decibels of a volume level on MassiveEQ's even scale; `None` is muted.
pub fn level_db(level: f64) -> Option<f64> {
    (level >= 0.005).then(|| (level.min(1.0) - 1.0) * VOLUME_RANGE_DB)
}

/// Linear gain of a volume level on MassiveEQ's even scale.
pub fn level_gain(level: f64) -> f64 {
    level_db(level).map_or(0.0, |db| 10.0_f64.powf(db / 20.0))
}

/// Whether going from `previous` to `next` raises the volume too far at once.
pub fn is_volume_jump(previous: f64, next: f64) -> bool {
    next - previous > VOLUME_JUMP_LIMIT + 1e-6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_percent_is_half_a_decibel() {
        assert_eq!(level_db(1.0), Some(0.0));
        assert!((level_db(0.95).unwrap() + 2.5).abs() < 1e-9);
        assert!((level_db(0.30).unwrap() + 35.0).abs() < 1e-9);
        assert!((level_db(0.01).unwrap() + 49.5).abs() < 1e-9);
        assert_eq!(level_db(0.0), None);
        assert_eq!(level_gain(0.0), 0.0);
        assert!((level_gain(0.88) - 10.0_f64.powf(-6.0 / 20.0)).abs() < 1e-9);
        // Over 100% stays at unity: MassiveEQ never amplifies.
        assert_eq!(level_db(1.5), Some(0.0));
    }

    #[test]
    fn desktop_levels_are_cube_roots_of_channel_volumes() {
        assert!((volume_level(0.125) - 0.5).abs() < 1e-12);
        assert_eq!(volume_level(-1.0), 0.0);
    }

    #[test]
    fn key_presses_pass_and_jumps_do_not() {
        assert!(!is_volume_jump(0.30, 0.35));
        assert!(!is_volume_jump(0.30, 0.40));
        assert!(is_volume_jump(0.30, 1.0));
        assert!(is_volume_jump(0.85, 1.0));
        assert!(!is_volume_jump(1.0, 0.2));
    }

    #[test]
    fn old_libraries_read_as_untouched_volume() {
        let settings: VolumeSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, VolumeSettings::default());
        assert_eq!(settings.level, 1.0);
    }
}
