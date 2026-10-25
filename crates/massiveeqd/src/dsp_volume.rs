use massiveeq_core::{level_gain, volume_level};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Most channels a MassiveEQ sink takes in (7.1).
pub const MAX_CHANNELS: usize = 8;

/// Below this PipeWire volume (about -140 dB) a channel counts as silent.
const SILENT: f32 = 1e-7;

/// The volume of a MassiveEQ sink that is the system output, on MassiveEQ's
/// even decibel scale. PipeWire still applies the sink's own (cubic) volume
/// to the samples it hands over; the DSP multiplies by the ratio between the
/// two, so the result follows the decibel scale exactly. The sink's PipeWire
/// thread writes this; the real-time thread only reads it.
pub struct DspVolume {
    channels: usize,
    /// The user's volume levels (0 to 1, as desktops show them) per sink
    /// channel, as f32 bits.
    levels: [AtomicU32; MAX_CHANNELS],
    /// The linear volumes PipeWire applies per sink channel, as f32 bits.
    applied: [AtomicU32; MAX_CHANNELS],
    muted: AtomicBool,
    /// PipeWire reports a new sink at 100% before the saved level is put
    /// back. The level follows reports only once the service restored it.
    armed: AtomicBool,
}

impl DspVolume {
    pub fn new(channels: usize, level: f64) -> Self {
        Self {
            channels: channels.clamp(1, MAX_CHANNELS),
            levels: std::array::from_fn(|_| AtomicU32::new((level as f32).to_bits())),
            applied: std::array::from_fn(|_| AtomicU32::new(1.0_f32.to_bits())),
            muted: AtomicBool::new(false),
            armed: AtomicBool::new(false),
        }
    }

    pub fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }

    /// Linear PipeWire channel volumes, as the sink reports them.
    pub fn set_channel_volumes(&self, volumes: &[f32]) {
        if volumes.is_empty() {
            return;
        }
        let armed = self.armed.load(Ordering::Acquire);
        for channel in 0..self.channels {
            let volume = volumes[channel.min(volumes.len() - 1)].max(0.0);
            self.applied[channel].store(volume.to_bits(), Ordering::Relaxed);
            if armed {
                self.levels[channel].store(
                    (volume_level(volume as f64) as f32).to_bits(),
                    Ordering::Relaxed,
                );
            }
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// The loudest channel's level, which is what desktops show.
    pub fn level(&self) -> f64 {
        self.levels
            .iter()
            .take(self.channels)
            .map(|level| f32::from_bits(level.load(Ordering::Relaxed)) as f64)
            .fold(0.0, f64::max)
    }

    /// The gain the DSP applies to one sink channel, on top of PipeWire's.
    #[inline]
    pub fn gain(&self, channel: usize) -> f32 {
        let channel = channel.min(self.channels - 1);
        let applied = f32::from_bits(self.applied[channel].load(Ordering::Relaxed));
        if self.muted.load(Ordering::Relaxed) || applied < SILENT {
            return 0.0;
        }
        let level = f32::from_bits(self.levels[channel].load(Ordering::Relaxed));
        (level_gain(level as f64) / applied as f64) as f32
    }

    /// The total gain from the sink's input to the speakers for a channel.
    #[cfg(test)]
    fn net_gain(&self, channel: usize) -> f32 {
        self.gain(channel) * f32::from_bits(self.applied[channel].load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(gain: f32) -> f32 {
        20.0 * gain.log10()
    }

    #[test]
    fn the_saved_level_holds_while_the_new_sink_reports_100_percent() {
        let volume = DspVolume::new(2, 0.5);
        volume.set_channel_volumes(&[1.0, 1.0]);
        assert!((db(volume.net_gain(0)) + 25.0).abs() < 1e-3);
        // The service shows the saved level on the sink: still -25 dB.
        volume.set_channel_volumes(&[0.125, 0.125]);
        assert!((db(volume.net_gain(0)) + 25.0).abs() < 1e-3);
        assert!((volume.level() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn once_armed_the_desktop_level_maps_to_half_a_decibel_per_percent() {
        let volume = DspVolume::new(2, 1.0);
        volume.arm();
        // 0.512 is 0.8 on the desktop scale: -10 dB, though PipeWire's own
        // cubic curve would make it -5.8 dB.
        volume.set_channel_volumes(&[0.512, 0.125]);
        assert!((volume.level() - 0.8).abs() < 1e-6);
        assert!((db(volume.net_gain(0)) + 10.0).abs() < 1e-3);
        assert!((db(volume.net_gain(1)) + 25.0).abs() < 1e-3);
        volume.set_muted(true);
        assert_eq!(volume.gain(0), 0.0);
        volume.set_muted(false);
        volume.set_channel_volumes(&[0.0, 0.0]);
        assert_eq!(volume.gain(0), 0.0);
    }

    #[test]
    fn a_single_reported_volume_covers_every_channel() {
        let volume = DspVolume::new(8, 1.0);
        volume.arm();
        volume.set_channel_volumes(&[0.125]);
        assert!((0..8).all(|channel| volume.gain(channel) == volume.gain(0)));
    }
}
