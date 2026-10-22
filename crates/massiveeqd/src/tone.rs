//! A test tone played through an output's current EQ, for finding peaks and
//! dips on the listener's own ears.

use anyhow::{Context, Result};
use massiveeq_core::DeviceInfo;
use pipewire as pw;
use pw::{properties::properties, spa};
use serde::Deserialize;
use spa::pod::Pod;
use std::{
    mem,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// The quietest and loudest levels a tone may play at, in dBFS.
pub const MIN_LEVEL_DB: f32 = -60.0;
pub const MAX_LEVEL_DB: f32 = -12.0;
/// A tone stops unless the editor renews it within this time.
pub const KEEPALIVE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    Sine,
    Pink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Band {
    Full,
    Octave,
    Third,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ears {
    Left,
    Both,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct ToneSettings {
    pub signal: Signal,
    pub band: Band,
    pub ears: Ears,
    pub frequency: f32,
    pub level_db: f32,
}

impl ToneSettings {
    /// Keep a request inside safe, audible limits.
    pub fn clamped(self) -> Self {
        Self {
            frequency: if self.frequency.is_finite() {
                self.frequency.clamp(20.0, 20_000.0)
            } else {
                1_000.0
            },
            level_db: if self.level_db.is_finite() {
                self.level_db.clamp(MIN_LEVEL_DB, MAX_LEVEL_DB)
            } else {
                MIN_LEVEL_DB
            },
            ..self
        }
    }
}

/// Settings shared with the real-time thread, packed into atomics.
struct Shared {
    signal: AtomicU32,
    band: AtomicU32,
    ears: AtomicU32,
    frequency: AtomicU32,
    level_db: AtomicU32,
    /// Fade to silence; set before the stream is torn down.
    muted: AtomicBool,
}

impl Shared {
    fn new(settings: ToneSettings) -> Self {
        let shared = Self {
            signal: AtomicU32::new(0),
            band: AtomicU32::new(0),
            ears: AtomicU32::new(0),
            frequency: AtomicU32::new(0),
            level_db: AtomicU32::new(0),
            muted: AtomicBool::new(false),
        };
        shared.store(settings);
        shared
    }

    fn store(&self, settings: ToneSettings) {
        self.signal.store(settings.signal as u32, Ordering::Relaxed);
        self.band.store(settings.band as u32, Ordering::Relaxed);
        self.ears.store(settings.ears as u32, Ordering::Relaxed);
        self.frequency
            .store(settings.frequency.to_bits(), Ordering::Relaxed);
        self.level_db
            .store(settings.level_db.to_bits(), Ordering::Relaxed);
    }

    fn load(&self) -> ToneSettings {
        ToneSettings {
            signal: match self.signal.load(Ordering::Relaxed) {
                0 => Signal::Sine,
                _ => Signal::Pink,
            },
            band: match self.band.load(Ordering::Relaxed) {
                0 => Band::Full,
                1 => Band::Octave,
                _ => Band::Third,
            },
            ears: match self.ears.load(Ordering::Relaxed) {
                0 => Ears::Left,
                1 => Ears::Both,
                _ => Ears::Right,
            },
            frequency: f32::from_bits(self.frequency.load(Ordering::Relaxed)),
            level_db: f32::from_bits(self.level_db.load(Ordering::Relaxed)),
        }
    }
}

/// Band-pass biquad (RBJ, 0 dB peak) for band-limited pink noise.
#[derive(Clone, Copy, Default)]
struct BandPass {
    b0: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    s1: f32,
    s2: f32,
}

impl BandPass {
    fn design(&mut self, frequency: f32, q: f32, rate: f32) {
        let w0 = std::f32::consts::TAU * frequency.min(rate * 0.45) / rate;
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.b2 = -alpha / a0;
        self.a1 = -2.0 * w0.cos() / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    /// Power gain |H|² at `frequency`.
    fn power_gain(&self, frequency: f32, rate: f32) -> f32 {
        let w = std::f32::consts::TAU * frequency / rate;
        let numerator = self.b0 * self.b0 * (2.0 - 2.0 * (2.0 * w).cos());
        let real = 1.0 + self.a1 * w.cos() + self.a2 * (2.0 * w).cos();
        let imaginary = self.a1 * w.sin() + self.a2 * (2.0 * w).sin();
        numerator / (real * real + imaginary * imaginary)
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.s1;
        self.s1 = -self.a1 * output + self.s2;
        self.s2 = self.b2 * input - self.a2 * output;
        output
    }
}

/// Mean power of `Generator::pink`, measured over a minute of output.
const PINK_POWER: f32 = 0.0376;
/// Pink noise has equal power per octave from where Kellett's filter turns
/// white (about 8.7 Hz at 48 kHz, scaling with the rate) up to Nyquist.
const PINK_LOWEST: f32 = 8.7 / 48_000.0;

/// Produces the tone. All state is fixed-size: safe on the real-time thread.
pub(crate) struct Generator {
    rate: f32,
    phase: f32,
    seed: u32,
    pink: [f32; 7],
    filters: [BandPass; 2],
    designed: Option<(Band, f32)>,
    /// Power of the noise after the band filters, for exact level.
    band_power: f32,
    gain: f32,
    left: f32,
    right: f32,
}

impl Generator {
    pub(crate) fn new(rate: u32) -> Self {
        Self {
            rate: rate as f32,
            phase: 0.0,
            seed: 0x1234_5678,
            pink: [0.0; 7],
            filters: [BandPass::default(); 2],
            designed: None,
            band_power: PINK_POWER,
            gain: 0.0,
            left: 0.0,
            right: 0.0,
        }
    }

    /// Share of pink noise power that passes both band filters: |H|⁴
    /// averaged over log frequency, as pink noise spreads its power.
    fn band_share(&self) -> f32 {
        const STEPS: usize = 512;
        let low = (PINK_LOWEST * self.rate).ln();
        let high = (0.5 * self.rate).ln();
        let step = (high - low) / STEPS as f32;
        (0..STEPS)
            .map(|index| {
                let frequency = (low + (index as f32 + 0.5) * step).exp();
                self.filters[0].power_gain(frequency, self.rate).powi(2)
            })
            .sum::<f32>()
            / STEPS as f32
    }

    fn white(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    /// Paul Kellett's economy pink-noise filter.
    fn pink(&mut self) -> f32 {
        let white = self.white();
        let b = &mut self.pink;
        b[0] = 0.99886 * b[0] + white * 0.0555179;
        b[1] = 0.99332 * b[1] + white * 0.0750759;
        b[2] = 0.96900 * b[2] + white * 0.153852;
        b[3] = 0.86650 * b[3] + white * 0.3104856;
        b[4] = 0.55000 * b[4] + white * 0.5329522;
        b[5] = -0.7616 * b[5] - white * 0.0168980;
        let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.5362;
        b[6] = white * 0.115926;
        pink * 0.11
    }

    /// Fill interleaved stereo frames.
    pub(crate) fn fill(&mut self, settings: ToneSettings, muted: bool, output: &mut [f32]) {
        let settings = settings.clamped();
        let amplitude = if muted {
            0.0
        } else {
            10.0_f32.powf(settings.level_db / 20.0)
        };
        let ceiling = 10.0_f32.powf(MAX_LEVEL_DB / 20.0);
        let (left, right) = match settings.ears {
            Ears::Left => (1.0, 0.0),
            Ears::Both => (1.0, 1.0),
            Ears::Right => (0.0, 1.0),
        };
        if settings.signal == Signal::Pink && settings.band != Band::Full {
            let wanted = (settings.band, settings.frequency);
            if self.designed != Some(wanted) {
                // Two cascaded sections give steeper skirts around the band.
                let q = if settings.band == Band::Octave {
                    std::f32::consts::SQRT_2
                } else {
                    4.318
                };
                for filter in &mut self.filters {
                    filter.design(settings.frequency, q, self.rate);
                }
                self.band_power = PINK_POWER * self.band_share();
                self.designed = Some(wanted);
            }
        }
        let smoothing = 1.0 - (-1.0 / (0.02 * self.rate)).exp();
        let noise_power = if settings.band == Band::Full {
            PINK_POWER
        } else {
            self.band_power
        };
        // Match a sine's RMS at the same level.
        let noise_scale = std::f32::consts::FRAC_1_SQRT_2 / noise_power.max(1e-12).sqrt();
        let step = settings.frequency / self.rate;
        for frame in output.as_chunks_mut::<2>().0 {
            self.gain += (amplitude - self.gain) * smoothing;
            self.left += (left - self.left) * smoothing;
            self.right += (right - self.right) * smoothing;
            let sample = match settings.signal {
                Signal::Sine => {
                    self.phase = (self.phase + step).fract();
                    (self.phase * std::f32::consts::TAU).sin() * self.gain
                }
                Signal::Pink => {
                    let mut noise = self.pink();
                    if settings.band != Band::Full {
                        for filter in &mut self.filters {
                            noise = filter.process(noise);
                        }
                    }
                    noise * noise_scale * self.gain
                }
            };
            let sample = sample.clamp(-ceiling, ceiling);
            frame[0] = sample * self.left;
            frame[1] = sample * self.right;
        }
    }
}

/// A playing tone. Dropping it, or not renewing it, stops playback.
pub struct Tone {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
    pub device_key: String,
    pub last_seen: Instant,
}

impl Tone {
    pub fn start(device: &DeviceInfo, sample_rate: u32, settings: ToneSettings) -> Result<Self> {
        let shared = Arc::new(Shared::new(settings.clamped()));
        let stop = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let thread_shared = shared.clone();
        let thread_stop = stop.clone();
        let target = device.node_name.clone();
        let thread = thread::Builder::new()
            .name("massiveeq-tone".into())
            .spawn(move || run(target, sample_rate, thread_shared, thread_stop, started_tx))
            .context("failed to start the tone thread")?;
        match started_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                shared,
                stop,
                thread: Some(thread),
                device_key: device.key.as_storage_key(),
                last_seen: Instant::now(),
            }),
            Ok(Err(message)) => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                anyhow::bail!(message)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                anyhow::bail!("the tone did not start within three seconds")
            }
        }
    }

    pub fn update(&mut self, settings: ToneSettings) {
        self.shared.store(settings.clamped());
        self.last_seen = Instant::now();
    }

    pub fn expired(&self) -> bool {
        self.last_seen.elapsed() > KEEPALIVE
    }

    /// Fade out, then tear the stream down off the caller's thread.
    pub fn stop(mut self) {
        self.shared.muted.store(true, Ordering::Relaxed);
        let stop = self.stop.clone();
        let thread = self.thread.take();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            stop.store(true, Ordering::Release);
            if let Some(thread) = thread {
                let _ = thread.join();
            }
        });
    }
}

impl Drop for Tone {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stop.store(true, Ordering::Release);
        }
    }
}

fn run(
    target: String,
    sample_rate: u32,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    started: mpsc::SyncSender<std::result::Result<(), String>>,
) -> Result<()> {
    let result = (|| -> Result<()> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        // Aimed at the physical output, so the smart filter routes the tone
        // through the output's current EQ. The name keeps it out of the
        // service's own source-rate readout.
        let props = properties! {
            *pw::keys::NODE_NAME => "massiveeq.tone",
            *pw::keys::NODE_DESCRIPTION => "MassiveEQ test tone",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Production",
            *pw::keys::TARGET_OBJECT => target,
            *pw::keys::NODE_RATE => format!("1/{sample_rate}"),
            *pw::keys::AUDIO_CHANNELS => "2",
            "audio.position" => "FL,FR",
            "node.stream.restore-props" => "false",
        };
        let stream = pw::stream::StreamBox::new(&core, "MassiveEQ test tone", props)?;
        let generator = Generator::new(sample_rate);
        let _listener = stream
            .add_local_listener_with_user_data(generator)
            .process(move |stream, generator| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let requested = buffer.requested() as usize;
                let Some(data) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let stride = mem::size_of::<f32>() * 2;
                let byte_len = {
                    let Some(bytes) = data.data() else {
                        return;
                    };
                    let byte_len = if requested == 0 {
                        bytes.len()
                    } else {
                        requested.saturating_mul(stride).min(bytes.len())
                    };
                    let byte_len = byte_len - byte_len % stride;
                    let Ok(samples) =
                        bytemuck::try_cast_slice_mut::<u8, f32>(&mut bytes[..byte_len])
                    else {
                        return;
                    };
                    generator.fill(shared.load(), shared.muted.load(Ordering::Relaxed), samples);
                    byte_len
                };
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride as i32;
                *chunk.size_mut() = byte_len as u32;
            })
            .register()?;
        let format = stereo_format(sample_rate)?;
        let mut params = [Pod::from_bytes(&format).context("invalid tone format")?];
        stream.connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;
        let weak = mainloop.downgrade();
        let timer = mainloop.loop_().add_timer(move |_| {
            if stop.load(Ordering::Acquire)
                && let Some(mainloop) = weak.upgrade()
            {
                mainloop.quit();
            }
        });
        timer
            .update_timer(
                Some(Duration::from_millis(20)),
                Some(Duration::from_millis(20)),
            )
            .into_result()?;
        let _ = started.send(Ok(()));
        mainloop.run();
        Ok(())
    })();
    if let Err(error) = &result {
        let _ = started.send(Err(error.to_string()));
    }
    result
}

fn stereo_format(sample_rate: u32) -> Result<Vec<u8>> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(sample_rate);
    info.set_channels(2);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    Ok(spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: spa::param::ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        }),
    )?
    .0
    .into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(signal: Signal, band: Band, ears: Ears) -> ToneSettings {
        ToneSettings {
            signal,
            band,
            ears,
            frequency: 1_000.0,
            level_db: -20.0,
        }
    }

    fn rms(samples: impl Iterator<Item = f32>) -> f32 {
        let (sum, count) = samples.fold((0.0, 0), |(sum, count), value| {
            (sum + value * value, count + 1)
        });
        (sum / count as f32).sqrt()
    }

    #[test]
    fn levels_match_across_signals_and_respect_ears() {
        let target = 10.0_f32.powf(-20.0 / 20.0) * std::f32::consts::FRAC_1_SQRT_2;
        for rate in [44_100_u32, 48_000, 96_000] {
            for frequency in [100.0, 1_000.0, 8_000.0] {
                for (signal, band) in [
                    (Signal::Sine, Band::Full),
                    (Signal::Pink, Band::Full),
                    (Signal::Pink, Band::Octave),
                    (Signal::Pink, Band::Third),
                ] {
                    let mut generator = Generator::new(rate);
                    let mut buffer = vec![0.0; rate as usize * 2 * 2];
                    let tone = ToneSettings {
                        frequency,
                        ..settings(signal, band, Ears::Left)
                    };
                    generator.fill(tone, false, &mut buffer);
                    // The level holds from the end of the fade-in.
                    let tail = &buffer[rate as usize / 5 * 2..];
                    let left = rms(tail.iter().step_by(2).copied());
                    let right = rms(tail.iter().skip(1).step_by(2).copied());
                    let error = 20.0 * (left / target).log10();
                    assert!(
                        error.abs() < 1.0,
                        "{rate} Hz, {frequency} Hz {signal:?} {band:?}: {error:+.2} dB"
                    );
                    assert!(right < 1e-6, "{signal:?} {band:?} leaked to the right ear");
                }
            }
        }
    }

    #[test]
    fn levels_are_capped_and_mute_fades_to_silence() {
        let loud = ToneSettings {
            level_db: 0.0,
            ..settings(Signal::Sine, Band::Full, Ears::Both)
        };
        assert_eq!(loud.clamped().level_db, MAX_LEVEL_DB);
        let mut generator = Generator::new(48_000);
        let mut buffer = vec![0.0; 9_600];
        generator.fill(loud, false, &mut buffer);
        let ceiling = 10.0_f32.powf(MAX_LEVEL_DB / 20.0);
        assert!(buffer.iter().all(|value| value.abs() <= ceiling + 1e-6));
        generator.fill(loud, true, &mut buffer);
        assert!(
            buffer[buffer.len() - 2..]
                .iter()
                .all(|value| value.abs() < 1e-3)
        );
    }
}
