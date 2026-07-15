use crate::{Biquad, compiler::CompiledChannel, compiler::CompiledProfile};
use fft_convolver::{FFTConvolverError, TwoStageFFTConvolver};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("audio block has {actual} frames but the processor capacity is {capacity}")]
    BlockTooLarge { actual: usize, capacity: usize },
    #[error("input and output block sizes differ")]
    BlockSizeMismatch,
    #[error("channel count mismatch")]
    ChannelCountMismatch,
    #[error("convolution processing failed: {0}")]
    Convolution(#[from] FFTConvolverError),
}

#[derive(Debug, Clone)]
pub struct ChannelProcessor {
    gain: f32,
    biquads: Vec<Biquad>,
    convolvers: Vec<TwoStageFFTConvolver<f32>>,
    scratch_a: Vec<f32>,
    scratch_b: Vec<f32>,
}

impl ChannelProcessor {
    pub fn new(channel: &CompiledChannel, quantum: usize) -> Result<Self, ProcessError> {
        let mut convolvers = Vec::with_capacity(channel.convolutions.len());
        for kernel in &channel.convolutions {
            let mut convolver = TwoStageFFTConvolver::<f32>::default();
            convolver.init_default(quantum, &kernel.impulse)?;
            convolvers.push(convolver);
        }
        Ok(Self {
            gain: channel.gain_linear,
            biquads: channel.biquads.iter().copied().map(Biquad::new).collect(),
            convolvers,
            scratch_a: vec![0.0; quantum],
            scratch_b: vec![0.0; quantum],
        })
    }

    #[inline]
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), ProcessError> {
        if input.len() != output.len() {
            return Err(ProcessError::BlockSizeMismatch);
        }
        if input.len() > self.scratch_a.len() {
            return Err(ProcessError::BlockTooLarge {
                actual: input.len(),
                capacity: self.scratch_a.len(),
            });
        }
        let frames = input.len();
        for (destination, source) in self.scratch_a[..frames].iter_mut().zip(input) {
            let mut sample = *source * self.gain;
            for biquad in &mut self.biquads {
                sample = biquad.process(sample);
            }
            *destination = sample;
        }
        if self.convolvers.is_empty() {
            output.copy_from_slice(&self.scratch_a[..frames]);
            return Ok(());
        }
        let mut source_is_a = true;
        for convolver in &mut self.convolvers {
            if source_is_a {
                convolver.process(&self.scratch_a[..frames], &mut self.scratch_b[..frames])?;
            } else {
                convolver.process(&self.scratch_b[..frames], &mut self.scratch_a[..frames])?;
            }
            source_is_a = !source_is_a;
        }
        if source_is_a {
            output.copy_from_slice(&self.scratch_a[..frames]);
        } else {
            output.copy_from_slice(&self.scratch_b[..frames]);
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        for biquad in &mut self.biquads {
            biquad.reset();
        }
        for convolver in &mut self.convolvers {
            convolver.reset();
        }
        self.scratch_a.fill(0.0);
        self.scratch_b.fill(0.0);
    }
}

#[derive(Debug, Clone)]
pub struct ProfileProcessor {
    channels: Vec<ChannelProcessor>,
    capacity: usize,
}

impl ProfileProcessor {
    pub fn new(profile: &CompiledProfile) -> Result<Self, ProcessError> {
        let capacity = profile.quantum as usize;
        Ok(Self {
            channels: profile
                .channels
                .iter()
                .map(|channel| ChannelProcessor::new(channel, capacity))
                .collect::<Result<_, _>>()?,
            capacity,
        })
    }

    pub fn process_planar(
        &mut self,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
    ) -> Result<(), ProcessError> {
        if inputs.len() != self.channels.len() || outputs.len() != self.channels.len() {
            return Err(ProcessError::ChannelCountMismatch);
        }
        for ((processor, input), output) in self.channels.iter_mut().zip(inputs).zip(outputs) {
            processor.process(input, output)?;
        }
        Ok(())
    }

    pub fn process_mono(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), ProcessError> {
        if self.channels.len() != 1 {
            return Err(ProcessError::ChannelCountMismatch);
        }
        self.channels[0].process(input, output)
    }

    pub fn process_stereo(
        &mut self,
        left_input: &[f32],
        right_input: &[f32],
        left_output: &mut [f32],
        right_output: &mut [f32],
    ) -> Result<(), ProcessError> {
        if self.channels.len() != 2 {
            return Err(ProcessError::ChannelCountMismatch);
        }
        let (left, right) = self.channels.split_at_mut(1);
        left[0].process(left_input, left_output)?;
        right[0].process(right_input, right_output)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileOptions, compile_profile};
    use massiveeq_core::parse_text;

    #[test]
    fn flat_profile_reserves_one_db_of_safety_headroom() {
        let profile = parse_text("test", "");
        let compiled =
            compile_profile(&profile, &CompileOptions::stereo(48_000, 128, "/tmp")).unwrap();
        let mut processor = ProfileProcessor::new(&compiled).unwrap();
        let left = (0..128)
            .map(|value| value as f32 / 128.0)
            .collect::<Vec<_>>();
        let right = left.iter().map(|value| -*value).collect::<Vec<_>>();
        let mut left_out = vec![0.0; 128];
        let mut right_out = vec![0.0; 128];
        processor
            .process_planar(&[&left, &right], &mut [&mut left_out, &mut right_out])
            .unwrap();
        let safety = 10.0_f32.powf(-1.0 / 20.0);
        for (input, output) in left.iter().zip(&left_out) {
            assert!((*output - *input * safety).abs() < 1e-6);
        }
        for (input, output) in right.iter().zip(&right_out) {
            assert!((*output - *input * safety).abs() < 1e-6);
        }
    }

    #[test]
    fn short_convolution_matches_direct_reference() {
        let channel = CompiledChannel {
            gain_linear: 1.0,
            biquads: Vec::new(),
            peak_candidates: Vec::new(),
            convolutions: vec![crate::ConvolutionKernel {
                sample_rate: 48_000,
                impulse: vec![0.5, 0.25, -0.125],
                latency_frames: 0,
            }],
        };
        let mut processor = ChannelProcessor::new(&channel, 64).unwrap();
        let mut input = vec![0.0; 64];
        input[0] = 1.0;
        let mut output = vec![0.0; 64];
        processor.process(&input, &mut output).unwrap();
        assert!((output[0] - 0.5).abs() < 1e-5);
        assert!((output[1] - 0.25).abs() < 1e-5);
        assert!((output[2] + 0.125).abs() < 1e-5);
    }
}
