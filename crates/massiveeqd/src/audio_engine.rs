use crate::{
    dsp_volume::{DspVolume, MAX_CHANNELS},
    latest_processor::LatestProcessor,
};
use massiveeq_dsp::ProfileProcessor;
use rtrb::{Producer, PushError};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

/// Owns all mutable DSP state on the PipeWire real-time thread. Every buffer
/// and processor is created by the control thread before it reaches here.
pub struct AudioEngine {
    active: Box<ProfileProcessor>,
    pending: Option<Box<ProfileProcessor>>,
    updates: Arc<LatestProcessor>,
    retired: Producer<Box<ProfileProcessor>>,
    retire_backlog: Option<Box<ProfileProcessor>>,
    /// The rate PipeWire negotiated for the sink, 0 until it has. It follows
    /// the device when the graph switches rate for the music playing.
    stream_rate: Arc<AtomicU32>,
    /// The sink's volume, when MassiveEQ applies it instead of PipeWire.
    volume: Option<Arc<DspVolume>>,
    /// Gains reached at the end of the last cycle, ramped from per cycle.
    volume_gains: [f32; MAX_CHANNELS],
    /// Channels arriving from the sink; more than `outputs` for surround.
    inputs: usize,
    outputs: usize,
    capacity: usize,
    fade_frames: usize,
    fade_position: usize,
    input: Vec<Vec<f32>>,
    active_output: Vec<Vec<f32>>,
    pending_output: Vec<Vec<f32>>,
    interleaved_output: Vec<f32>,
}

/// How much longer than the first chain's capacity a cycle may grow. PipeWire
/// scales cycles with the rate, so 384 kHz cycles are eight times 48 kHz ones.
pub const RATE_HEADROOM: usize = 8;

impl AudioEngine {
    pub fn new(
        active: Box<ProfileProcessor>,
        updates: Arc<LatestProcessor>,
        retired: Producer<Box<ProfileProcessor>>,
        stream_rate: Arc<AtomicU32>,
        sample_rate: u32,
    ) -> Self {
        let capacity = active.capacity() * RATE_HEADROOM;
        let inputs = active.input_channels();
        let outputs = active.output_channels();
        Self {
            active,
            pending: None,
            updates,
            retired,
            retire_backlog: None,
            stream_rate,
            volume: None,
            volume_gains: [1.0; MAX_CHANNELS],
            inputs,
            outputs,
            capacity,
            fade_frames: ((sample_rate as f64 * 0.008).round() as usize).max(1),
            fade_position: 0,
            input: vec![vec![0.0; capacity]; inputs],
            active_output: vec![vec![0.0; capacity]; outputs],
            pending_output: vec![vec![0.0; capacity]; outputs],
            interleaved_output: vec![0.0; capacity * outputs],
        }
    }

    /// Apply `volume` to the sink's input, starting at its current gains.
    pub fn with_volume(mut self, volume: Arc<DspVolume>) -> Self {
        for (channel, gain) in self.volume_gains.iter_mut().enumerate() {
            *gain = volume.gain(channel);
        }
        self.volume = Some(volume);
        self
    }

    /// Processes interleaved F32 audio without allocation, locking, logging or
    /// ownership destruction. A completed chain is returned to the control
    /// thread for reclamation. Returns interleaved output-channel audio.
    #[inline]
    pub fn process(&mut self, interleaved: &[f32]) -> &[f32] {
        let frames = interleaved.len() / self.inputs;
        if frames > self.capacity || interleaved.len() != frames * self.inputs {
            return &[];
        }
        let output_len = frames * self.outputs;
        self.return_retired_chain();
        if self.pending.is_none()
            && self.retire_backlog.is_none()
            && let Some(next) = self.updates.take()
        {
            self.pending = Some(next);
            self.fade_position = 0;
        }

        for (frame_index, frame) in interleaved.chunks_exact(self.inputs).enumerate() {
            for (channel, sample) in frame.iter().enumerate() {
                self.input[channel][frame_index] = *sample;
            }
        }
        if let Some(volume) = &self.volume {
            // Ramp across the cycle so a volume step never clicks.
            for (channel, input) in self.input.iter_mut().enumerate().take(MAX_CHANNELS) {
                let start = self.volume_gains[channel];
                let target = volume.gain(channel);
                let step = (target - start) / frames.max(1) as f32;
                for (frame, sample) in input[..frames].iter_mut().enumerate() {
                    *sample *= start + step * (frame + 1) as f32;
                }
                self.volume_gains[channel] = target;
            }
        }
        // A chain built for another rate would put every filter in the wrong
        // place, and one for shorter cycles cannot take this one, so it stays
        // silent until the control thread sends one that fits, which fades in.
        let rate = self.stream_rate.load(Ordering::Relaxed);
        let fits = |processor: &ProfileProcessor| {
            (rate == 0 || processor.sample_rate() == rate) && frames <= processor.capacity()
        };
        if !fits(&self.active) {
            for output in &mut self.active_output {
                output[..frames].fill(0.0);
            }
        } else if self
            .active
            .process(&self.input, &mut self.active_output, frames)
            .is_err()
        {
            // Fail open: pass the leading channels through untouched.
            for (frame, samples) in interleaved.chunks_exact(self.inputs).enumerate() {
                for channel in 0..self.outputs {
                    self.interleaved_output[frame * self.outputs + channel] =
                        samples[channel % self.inputs];
                }
            }
            return &self.interleaved_output[..output_len];
        }

        let mut finish_crossfade = false;
        if let Some(pending) = &mut self.pending {
            if !fits(pending) {
                for output in &mut self.pending_output {
                    output[..frames].fill(0.0);
                }
            } else if pending
                .process(&self.input, &mut self.pending_output, frames)
                .is_err()
            {
                self.pending = None;
                self.interleave_active(frames);
                return &self.interleaved_output[..output_len];
            }
            for frame in 0..frames {
                let amount =
                    ((self.fade_position + frame) as f32 / self.fade_frames as f32).clamp(0.0, 1.0);
                for channel in 0..self.outputs {
                    self.interleaved_output[frame * self.outputs + channel] =
                        self.active_output[channel][frame] * (1.0 - amount)
                            + self.pending_output[channel][frame] * amount;
                }
            }
            self.fade_position = self.fade_position.saturating_add(frames);
            finish_crossfade = self.fade_position >= self.fade_frames;
        } else {
            self.interleave_active(frames);
        }
        if finish_crossfade {
            let next = self.pending.take().expect("pending crossfade processor");
            let old = std::mem::replace(&mut self.active, next);
            match self.retired.push(old) {
                Ok(()) => {}
                Err(PushError::Full(old)) => self.retire_backlog = Some(old),
            }
        }
        &self.interleaved_output[..output_len]
    }

    fn interleave_active(&mut self, frames: usize) {
        for frame in 0..frames {
            for channel in 0..self.outputs {
                self.interleaved_output[frame * self.outputs + channel] =
                    self.active_output[channel][frame];
            }
        }
    }

    fn return_retired_chain(&mut self) {
        let Some(old) = self.retire_backlog.take() else {
            return;
        };
        if let Err(PushError::Full(old)) = self.retired.push(old) {
            self.retire_backlog = Some(old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation_test_support;
    use massiveeq_dsp::{ProfileProcessor, compile_bypass};
    use rtrb::RingBuffer;

    #[test]
    fn resuming_audio_uses_the_newest_edit_after_many_idle_updates() {
        let initial = compile_bypass(48000, 512, 2);
        let updates = Arc::new(LatestProcessor::default());
        let (retired, _consumer) = RingBuffer::new(4);
        let mut engine = AudioEngine::new(
            Box::new(ProfileProcessor::new(&initial).unwrap()),
            updates.clone(),
            retired,
            Arc::new(AtomicU32::new(0)),
            48000,
        );
        for gain in 1..=32 {
            let compiled = massiveeq_dsp::compile_bypass_with_gain(48000, 512, 2, -gain as f64);
            updates.publish(Box::new(ProfileProcessor::new(&compiled).unwrap()));
        }
        let input = vec![1.0; 1024];
        allocation_test_support::begin();
        let output = engine.process(&input);
        let last = *output.last().unwrap();
        let allocations = allocation_test_support::end();
        assert_eq!(allocations, 0);
        assert!((last - 10.0_f32.powf(-32.0 / 20.0)).abs() < 1e-6);
    }

    #[test]
    fn a_chain_for_another_rate_is_silent_until_one_for_the_new_rate_fades_in() {
        let at_48k = compile_bypass(48_000, 256, 2);
        let at_96k = compile_bypass(96_000, 256, 2);
        let updates = Arc::new(LatestProcessor::default());
        let (retired, _consumer) = RingBuffer::new(4);
        let rate = Arc::new(AtomicU32::new(48_000));
        let mut engine = AudioEngine::new(
            Box::new(ProfileProcessor::new(&at_48k).unwrap()),
            updates.clone(),
            retired,
            rate.clone(),
            48_000,
        );
        let input = vec![0.5_f32; 512];
        assert!(engine.process(&input).iter().all(|sample| *sample == 0.5));

        // The device switched to 96 kHz for the next track.
        rate.store(96_000, Ordering::Relaxed);
        assert!(engine.process(&input).iter().all(|sample| *sample == 0.0));
        // A late chain for the old rate stays silent too.
        updates.publish(Box::new(ProfileProcessor::new(&at_48k).unwrap()));
        assert!(engine.process(&input).iter().all(|sample| *sample == 0.0));
        for _ in 0..8 {
            engine.process(&input);
        }

        updates.publish(Box::new(ProfileProcessor::new(&at_96k).unwrap()));
        allocation_test_support::begin();
        let first = engine.process(&input);
        let allocations = allocation_test_support::end();
        assert_eq!(allocations, 0);
        assert!(first[0] < 0.01 && first[511] > first[0], "fades in");
        for _ in 0..8 {
            engine.process(&input);
        }
        assert!(
            engine
                .process(&input)
                .iter()
                .all(|sample| (sample - 0.5).abs() < 1e-6)
        );
    }

    #[test]
    fn longer_cycles_at_a_higher_rate_wait_for_a_chain_that_fits() {
        let at_48k = compile_bypass(48_000, 256, 2);
        let at_96k = compile_bypass(96_000, 512, 2);
        let updates = Arc::new(LatestProcessor::default());
        let (retired, _consumer) = RingBuffer::new(4);
        let rate = Arc::new(AtomicU32::new(96_000));
        let mut engine = AudioEngine::new(
            Box::new(ProfileProcessor::new(&at_48k).unwrap()),
            updates.clone(),
            retired,
            rate,
            48_000,
        );
        // 512 frames: twice what the first chain holds, never passed through raw.
        let input = vec![0.5_f32; 1024];
        assert_eq!(engine.process(&input).len(), 1024);
        assert!(engine.process(&input).iter().all(|sample| *sample == 0.0));
        updates.publish(Box::new(ProfileProcessor::new(&at_96k).unwrap()));
        for _ in 0..4 {
            engine.process(&input);
        }
        assert!(
            engine
                .process(&input)
                .iter()
                .all(|sample| (sample - 0.5).abs() < 1e-6)
        );
    }

    #[test]
    fn dsp_volume_ramps_to_its_decibel_step_without_allocating() {
        let compiled = compile_bypass(48_000, 256, 2);
        let updates = Arc::new(LatestProcessor::default());
        let (retired, _consumer) = RingBuffer::new(4);
        let volume = Arc::new(DspVolume::new(2, 1.0));
        volume.arm();
        let mut engine = AudioEngine::new(
            Box::new(ProfileProcessor::new(&compiled).unwrap()),
            updates,
            retired,
            Arc::new(AtomicU32::new(48_000)),
            48_000,
        )
        .with_volume(volume.clone());
        let input = vec![1.0_f32; 512];
        assert!(engine.process(&input).iter().all(|sample| *sample == 1.0));
        // A 5% key press down: 2.5 dB in all. PipeWire already applied its
        // own 0.95³ to the input, so the DSP adds the rest.
        volume.set_channel_volumes(&[0.95_f32.powi(3), 0.95_f32.powi(3)]);
        let target = 10.0_f32.powf(-2.5 / 20.0) / 0.95_f32.powi(3);
        allocation_test_support::begin();
        let ramp = engine.process(&input);
        let allocations = allocation_test_support::end();
        assert_eq!(allocations, 0);
        assert!(ramp[0] < 1.0 && ramp[0] > ramp[510]);
        assert!((ramp[511] - target).abs() < 1e-4);
        assert!(
            engine
                .process(&input)
                .iter()
                .all(|sample| (sample - target).abs() < 1e-4)
        );
        volume.set_muted(true);
        engine.process(&input);
        assert!(engine.process(&input).iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn processing_and_crossfade_do_not_allocate_or_free() {
        let compiled = compile_bypass(48_000, 128, 2);
        let active = Box::new(ProfileProcessor::new(&compiled).unwrap());
        let next = Box::new(ProfileProcessor::new(&compiled).unwrap());
        let update_producer = Arc::new(LatestProcessor::default());
        let update_consumer = update_producer.clone();
        let (retire_producer, mut retire_consumer) = RingBuffer::new(4);
        let mut engine = AudioEngine::new(
            active,
            update_consumer,
            retire_producer,
            Arc::new(AtomicU32::new(0)),
            48_000,
        );
        update_producer.publish(next);
        let input = vec![0.25_f32; 256];

        allocation_test_support::begin();
        for _ in 0..4 {
            assert_eq!(engine.process(&input).len(), input.len());
        }
        let allocations = allocation_test_support::end();
        assert_eq!(allocations, 0);
        assert!(retire_consumer.pop().is_ok());
    }

    #[test]
    fn surround_downmix_processes_without_allocating() {
        let mut compiled = compile_bypass(48_000, 128, 2);
        massiveeq_dsp::widen_input(&mut compiled, 8).unwrap();
        let active = Box::new(ProfileProcessor::new(&compiled).unwrap());
        let next = Box::new(ProfileProcessor::new(&compiled).unwrap());
        let update_producer = Arc::new(LatestProcessor::default());
        let update_consumer = update_producer.clone();
        let (retire_producer, _retire_consumer) = RingBuffer::new(4);
        let mut engine = AudioEngine::new(
            active,
            update_consumer,
            retire_producer,
            Arc::new(AtomicU32::new(0)),
            48_000,
        );
        update_producer.publish(next);
        // 128 frames of 7.1 in, 128 frames of stereo out.
        let mut input = vec![0.0_f32; 128 * 8];
        input[0] = 1.0; // front left, first frame

        allocation_test_support::begin();
        let output = engine.process(&input).len();
        for _ in 0..3 {
            engine.process(&input);
        }
        let allocations = allocation_test_support::end();
        assert_eq!(allocations, 0);
        assert_eq!(output, 128 * 2);
    }
}
