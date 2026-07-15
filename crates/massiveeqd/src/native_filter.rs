use crate::audio_engine::AudioEngine;
use anyhow::{Context, Result};
use massiveeq_core::DeviceInfo;
use massiveeq_dsp::ProfileProcessor;
use pipewire as pw;
use pw::{properties::properties, spa};
use rtrb::{Consumer, Producer, RingBuffer};
use spa::pod::Pod;
use std::{
    mem,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Default)]
pub struct NativeStats {
    pub input_overflows: AtomicU64,
    pub output_underflows: AtomicU64,
    pub invalid_buffers: AtomicU64,
    pub process_calls: AtomicU64,
    pub process_nanos_total: AtomicU64,
    pub process_nanos_peak: AtomicU64,
}

pub struct NativeFilter {
    updates: Producer<Box<ProfileProcessor>>,
    retired: Consumer<Box<ProfileProcessor>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
    pub stats: Arc<NativeStats>,
    pub node_name: String,
}

impl NativeFilter {
    pub fn spawn(
        device: DeviceInfo,
        initial: Box<ProfileProcessor>,
        sample_rate: u32,
        quantum: u32,
        latency_frames: u32,
    ) -> Result<Self> {
        let (updates, update_consumer) = RingBuffer::new(2);
        let (retire_producer, retired) = RingBuffer::new(4);
        let audio_capacity = quantum as usize * device.channels as usize * 16;
        let (audio_producer, audio_consumer) = RingBuffer::new(audio_capacity.max(1024));
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(NativeStats::default());
        let safe_name = stable_hash(&device.key.as_storage_key());
        let node_name = format!("massiveeq.{safe_name:x}");
        let thread_stop = stop.clone();
        let thread_stats = stats.clone();
        let thread_node_name = node_name.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name(format!("massiveeq-{safe_name:x}"))
            .spawn(move || {
                run_filter_pair(
                    &device,
                    initial,
                    update_consumer,
                    retire_producer,
                    audio_producer,
                    audio_consumer,
                    thread_stop,
                    thread_stats,
                    &thread_node_name,
                    sample_rate,
                    quantum,
                    latency_frames,
                    started_tx,
                )
            })
            .context("failed to start the native PipeWire filter thread")?;
        match started_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                updates,
                retired,
                stop,
                thread: Some(thread),
                stats,
                node_name,
            }),
            Ok(Err(message)) => {
                stop.store(true, Ordering::Release);
                let _ = thread.join();
                anyhow::bail!(message)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                anyhow::bail!("native PipeWire filter did not start within three seconds")
            }
        }
    }

    pub fn update(&mut self, processor: Box<ProfileProcessor>) -> Result<()> {
        self.reap();
        self.updates
            .push(processor)
            .map_err(|_| anyhow::anyhow!("a previous DSP update is still pending"))
    }

    pub fn reap(&mut self) {
        while let Ok(processor) = self.retired.pop() {
            drop(processor);
        }
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_some_and(JoinHandle::is_finished)
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for NativeFilter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_filter_pair(
    device: &DeviceInfo,
    initial: Box<ProfileProcessor>,
    updates: Consumer<Box<ProfileProcessor>>,
    retired: Producer<Box<ProfileProcessor>>,
    mut audio_producer: Producer<f32>,
    mut audio_consumer: Consumer<f32>,
    stop: Arc<AtomicBool>,
    stats: Arc<NativeStats>,
    node_name: &str,
    sample_rate: u32,
    _quantum: u32,
    latency_frames: u32,
    started: mpsc::SyncSender<std::result::Result<(), String>>,
) -> Result<()> {
    let result = (|| -> Result<()> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let channels = device.channels as usize;
        let link_group = format!(
            "massiveeq-link-{:x}",
            stable_hash(&device.key.as_storage_key())
        );
        let node_group = format!(
            "massiveeq-group-{:x}",
            stable_hash(&device.key.as_storage_key())
        );
        let position = if channels == 1 { "MONO" } else { "FL,FR" };
        let latency = format!("{latency_frames}/{sample_rate}");
        let rate = format!("1/{sample_rate}");
        let channel_count = channels.to_string();
        let target = format!(r#"{{ "node.name": "{}" }}"#, device.node_name);

        let capture_props = properties! {
            *pw::keys::NODE_NAME => node_name,
            *pw::keys::NODE_DESCRIPTION => format!("MassiveEQ — {}", device.description),
            *pw::keys::MEDIA_CLASS => "Audio/Sink",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "DSP",
            *pw::keys::NODE_VIRTUAL => "true",
            *pw::keys::NODE_LINK_GROUP => link_group.clone(),
            *pw::keys::NODE_GROUP => node_group.clone(),
            *pw::keys::NODE_RATE => rate.clone(),
            *pw::keys::NODE_LATENCY => latency.clone(),
            *pw::keys::AUDIO_CHANNELS => channel_count.clone(),
            "audio.position" => position,
            "filter.smart" => "true",
            "filter.smart.name" => link_group.clone(),
            "filter.smart.targetable" => "false",
            "filter.smart.target" => target,
            "node.hidden" => "true",
            "node.stream.restore-props" => "false",
            "channelmix.lock-volumes" => "true",
            "resample.disable" => "true",
        };
        let playback_props = properties! {
            *pw::keys::NODE_NAME => format!("massiveeq.output.{:x}", stable_hash(&device.key.as_storage_key())),
            *pw::keys::MEDIA_CLASS => "Stream/Output/Audio",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "DSP",
            *pw::keys::NODE_LINK_GROUP => link_group,
            *pw::keys::NODE_GROUP => node_group,
            *pw::keys::NODE_PASSIVE => "true",
            *pw::keys::NODE_DONT_RECONNECT => "true",
            *pw::keys::TARGET_OBJECT => device.node_name.clone(),
            *pw::keys::STREAM_DONT_REMIX => "true",
            *pw::keys::NODE_RATE => rate,
            *pw::keys::NODE_LATENCY => latency,
            *pw::keys::AUDIO_CHANNELS => channel_count,
            "audio.position" => position,
            "node.stream.restore-props" => "false",
            "node.hidden" => "true",
            "channelmix.lock-volumes" => "true",
            "resample.disable" => "true",
        };

        let capture = pw::stream::StreamBox::new(&core, "MassiveEQ input", capture_props)?;
        let playback = pw::stream::StreamBox::new(&core, "MassiveEQ output", playback_props)?;
        let engine = AudioEngine::new(initial, updates, retired, channels, sample_rate);
        let capture_stats = stats.clone();
        let _capture_listener = capture
            .add_local_listener_with_user_data(engine)
            .process(move |stream, engine| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let Some(data) = buffer.datas_mut().first_mut() else {
                    capture_stats
                        .invalid_buffers
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let offset = data.chunk().offset() as usize;
                let size = data.chunk().size() as usize;
                let Some(bytes) = data.data() else {
                    capture_stats
                        .invalid_buffers
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let Some(bytes) = bytes.get(offset..offset.saturating_add(size)) else {
                    capture_stats
                        .invalid_buffers
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let Ok(samples) = bytemuck::try_cast_slice::<u8, f32>(bytes) else {
                    capture_stats
                        .invalid_buffers
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let started = std::time::Instant::now();
                let processed = engine.process(samples);
                let elapsed = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                capture_stats.process_calls.fetch_add(1, Ordering::Relaxed);
                capture_stats
                    .process_nanos_total
                    .fetch_add(elapsed, Ordering::Relaxed);
                capture_stats
                    .process_nanos_peak
                    .fetch_max(elapsed, Ordering::Relaxed);
                for sample in processed {
                    if audio_producer.push(*sample).is_err() {
                        capture_stats
                            .input_overflows
                            .fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                }
            })
            .register()?;

        let playback_stats = stats.clone();
        let _playback_listener = playback
            .add_local_listener_with_user_data(())
            .process(move |stream, _| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let Some(data) = buffer.datas_mut().first_mut() else {
                    playback_stats
                        .invalid_buffers
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let byte_len = {
                    let Some(bytes) = data.data() else {
                        playback_stats
                            .invalid_buffers
                            .fetch_add(1, Ordering::Relaxed);
                        return;
                    };
                    let byte_len = bytes.len();
                    let Ok(samples) = bytemuck::try_cast_slice_mut::<u8, f32>(bytes) else {
                        playback_stats
                            .invalid_buffers
                            .fetch_add(1, Ordering::Relaxed);
                        return;
                    };
                    for sample in samples.iter_mut() {
                        *sample = match audio_consumer.pop() {
                            Ok(value) => value,
                            Err(_) => {
                                playback_stats
                                    .output_underflows
                                    .fetch_add(1, Ordering::Relaxed);
                                0.0
                            }
                        };
                    }
                    byte_len
                };
                let stride = mem::size_of::<f32>() * channels;
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride as i32;
                *chunk.size_mut() = byte_len as u32;
            })
            .register()?;

        let capture_format = audio_format(sample_rate, channels as u32)?;
        let playback_format = audio_format(sample_rate, channels as u32)?;
        let mut capture_params =
            [Pod::from_bytes(&capture_format).context("invalid capture format")?];
        let mut playback_params =
            [Pod::from_bytes(&playback_format).context("invalid playback format")?];
        playback.connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS
                | pw::stream::StreamFlags::DONT_RECONNECT,
            &mut playback_params,
        )?;
        capture.connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS
                | pw::stream::StreamFlags::DRIVER,
            &mut capture_params,
        )?;

        let weak = mainloop.downgrade();
        let timer_stop = stop.clone();
        let timer = mainloop.loop_().add_timer(move |_| {
            if timer_stop.load(Ordering::Acquire)
                && let Some(mainloop) = weak.upgrade()
            {
                mainloop.quit();
            }
        });
        timer
            .update_timer(
                Some(Duration::from_millis(50)),
                Some(Duration::from_millis(50)),
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

fn audio_format(sample_rate: u32, channels: u32) -> Result<Vec<u8>> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(sample_rate);
    info.set_channels(channels);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    if channels == 1 {
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    } else {
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    }
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

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}
