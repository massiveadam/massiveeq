use massiveeq_dsp::ProfileProcessor;
use rtrb::{Consumer, Producer, PushError};

/// Owns all mutable DSP state on the PipeWire real-time thread. Every buffer
/// and processor is created by the control thread before it reaches here.
pub struct AudioEngine {
    active: Box<ProfileProcessor>,
    pending: Option<Box<ProfileProcessor>>,
    updates: Consumer<Box<ProfileProcessor>>,
    retired: Producer<Box<ProfileProcessor>>,
    retire_backlog: Option<Box<ProfileProcessor>>,
    channels: usize,
    capacity: usize,
    fade_frames: usize,
    fade_position: usize,
    input: Vec<Vec<f32>>,
    active_output: Vec<Vec<f32>>,
    pending_output: Vec<Vec<f32>>,
    interleaved_output: Vec<f32>,
}

impl AudioEngine {
    pub fn new(
        active: Box<ProfileProcessor>,
        updates: Consumer<Box<ProfileProcessor>>,
        retired: Producer<Box<ProfileProcessor>>,
        channels: usize,
        sample_rate: u32,
    ) -> Self {
        let capacity = active.capacity();
        Self {
            active,
            pending: None,
            updates,
            retired,
            retire_backlog: None,
            channels,
            capacity,
            fade_frames: ((sample_rate as f64 * 0.008).round() as usize).max(1),
            fade_position: 0,
            input: vec![vec![0.0; capacity]; channels],
            active_output: vec![vec![0.0; capacity]; channels],
            pending_output: vec![vec![0.0; capacity]; channels],
            interleaved_output: vec![0.0; capacity * channels],
        }
    }

    /// Processes interleaved F32 audio without allocation, locking, logging or
    /// ownership destruction. A completed chain is returned to the control
    /// thread for reclamation.
    #[inline]
    pub fn process(&mut self, interleaved: &[f32]) -> &[f32] {
        let frames = interleaved.len() / self.channels;
        if frames > self.capacity || interleaved.len() != frames * self.channels {
            return &[];
        }
        self.return_retired_chain();
        if self.pending.is_none()
            && self.retire_backlog.is_none()
            && let Ok(next) = self.updates.pop()
        {
            self.pending = Some(next);
            self.fade_position = 0;
        }

        for (frame_index, frame) in interleaved.chunks_exact(self.channels).enumerate() {
            for (channel, sample) in frame.iter().enumerate() {
                self.input[channel][frame_index] = *sample;
            }
        }
        if self.process_active(frames).is_err() {
            self.interleaved_output[..interleaved.len()].copy_from_slice(interleaved);
            return &self.interleaved_output[..interleaved.len()];
        }

        let mut finish_crossfade = false;
        if let Some(pending) = &mut self.pending {
            let result = match self.channels {
                1 => pending.process_mono(
                    &self.input[0][..frames],
                    &mut self.pending_output[0][..frames],
                ),
                2 => {
                    let (output_left, output_right) = self.pending_output.split_at_mut(1);
                    pending.process_stereo(
                        &self.input[0][..frames],
                        &self.input[1][..frames],
                        &mut output_left[0][..frames],
                        &mut output_right[0][..frames],
                    )
                }
                _ => unreachable!(),
            };
            if result.is_err() {
                self.pending = None;
                self.interleave_active(frames);
                return &self.interleaved_output[..interleaved.len()];
            }
            for frame in 0..frames {
                let amount =
                    ((self.fade_position + frame) as f32 / self.fade_frames as f32).clamp(0.0, 1.0);
                for channel in 0..self.channels {
                    self.interleaved_output[frame * self.channels + channel] =
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
        &self.interleaved_output[..interleaved.len()]
    }

    fn process_active(&mut self, frames: usize) -> Result<(), massiveeq_dsp::ProcessError> {
        match self.channels {
            1 => self.active.process_mono(
                &self.input[0][..frames],
                &mut self.active_output[0][..frames],
            ),
            2 => {
                let (output_left, output_right) = self.active_output.split_at_mut(1);
                self.active.process_stereo(
                    &self.input[0][..frames],
                    &self.input[1][..frames],
                    &mut output_left[0][..frames],
                    &mut output_right[0][..frames],
                )
            }
            _ => unreachable!(),
        }
    }

    fn interleave_active(&mut self, frames: usize) {
        for frame in 0..frames {
            for channel in 0..self.channels {
                self.interleaved_output[frame * self.channels + channel] =
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
    fn processing_and_crossfade_do_not_allocate_or_free() {
        let compiled = compile_bypass(48_000, 128, 2);
        let active = Box::new(ProfileProcessor::new(&compiled).unwrap());
        let next = Box::new(ProfileProcessor::new(&compiled).unwrap());
        let (mut update_producer, update_consumer) = RingBuffer::new(2);
        let (retire_producer, mut retire_consumer) = RingBuffer::new(4);
        let mut engine = AudioEngine::new(active, update_consumer, retire_producer, 2, 48_000);
        update_producer.push(next).unwrap();
        let input = vec![0.25_f32; 256];

        allocation_test_support::begin();
        for _ in 0..4 {
            assert_eq!(engine.process(&input).len(), input.len());
        }
        let allocations = allocation_test_support::end();
        assert_eq!(allocations, 0);
        assert!(retire_consumer.pop().is_ok());
    }
}
