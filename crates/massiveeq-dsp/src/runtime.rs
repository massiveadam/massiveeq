use crate::{Biquad, compiler::CompiledChannel, compiler::CompiledProfile};
use fft_convolver::{FFTConvolver, FFTConvolverError, TwoStageFFTConvolver};
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

/// A zero-latency peak limiter linked across the output channels: any frame
/// over the ceiling is turned down to it at once, then the gain recovers
/// smoothly. Below the ceiling it is exactly unity.
#[derive(Debug, Clone)]
struct OutputLimiter {
    ceiling: f32,
    gain: f32,
    release: f32,
}

impl OutputLimiter {
    /// -1 dBFS, the same margin every profile keeps.
    const CEILING_DB: f32 = -1.0;
    const RELEASE_SECONDS: f32 = 0.1;

    fn new(sample_rate: u32) -> Self {
        Self {
            ceiling: 10.0_f32.powf(Self::CEILING_DB / 20.0),
            gain: 1.0,
            release: 1.0 - (-1.0 / (Self::RELEASE_SECONDS * sample_rate.max(1) as f32)).exp(),
        }
    }

    fn process<O: AsMut<[f32]>>(&mut self, outputs: &mut [O], frames: usize) {
        for frame in 0..frames {
            let peak = outputs
                .iter_mut()
                .map(|output| output.as_mut()[frame].abs())
                .fold(0.0_f32, f32::max);
            self.gain += (1.0 - self.gain) * self.release;
            if peak * self.gain > self.ceiling {
                self.gain = self.ceiling / peak;
            }
            if self.gain < 1.0 {
                for output in outputs.iter_mut() {
                    output.as_mut()[frame] *= self.gain;
                }
            }
        }
    }
}

/// One input channel convolved into one output channel.
#[derive(Debug, Clone)]
struct RouteProcessor {
    input: usize,
    output: usize,
    convolver: FFTConvolver<f32>,
}

#[derive(Debug, Clone)]
pub struct ProfileProcessor {
    /// Per-output gain and filters.
    channels: Vec<ChannelProcessor>,
    capacity: usize,
    input_channels: usize,
    /// `[profile input][sink channel]` gains when the sink is wider than the profile.
    downmix: Option<Vec<Vec<f32>>>,
    routes: Vec<RouteProcessor>,
    /// The profile's own inputs after downmixing.
    mixed: Vec<Vec<f32>>,
    /// Matrix-convolution sums per output.
    summed: Vec<Vec<f32>>,
    route_output: Vec<f32>,
    limiter: Option<OutputLimiter>,
}

impl ProfileProcessor {
    pub fn new(profile: &CompiledProfile) -> Result<Self, ProcessError> {
        let capacity = profile.quantum as usize;
        let mut routes = Vec::with_capacity(profile.routes.len());
        for route in &profile.routes {
            let mut convolver = FFTConvolver::<f32>::default();
            convolver.init(capacity, &route.kernel.impulse)?;
            routes.push(RouteProcessor {
                input: route.input,
                output: route.output,
                convolver,
            });
        }
        let outputs = profile.channels.len();
        let mixed = profile.downmix.as_ref().map_or(0, |downmix| downmix.len());
        Ok(Self {
            channels: profile
                .channels
                .iter()
                .map(|channel| ChannelProcessor::new(channel, capacity))
                .collect::<Result<_, _>>()?,
            capacity,
            input_channels: profile.input_channels as usize,
            downmix: profile.downmix.clone(),
            routes,
            mixed: vec![vec![0.0; capacity]; mixed],
            summed: vec![
                vec![
                    0.0;
                    if profile.routes.is_empty() {
                        0
                    } else {
                        capacity
                    }
                ];
                outputs
            ],
            route_output: vec![0.0; capacity],
            limiter: profile
                .output_limit
                .then(|| OutputLimiter::new(profile.sample_rate)),
        })
    }

    pub fn input_channels(&self) -> usize {
        self.input_channels
    }

    pub fn output_channels(&self) -> usize {
        self.channels.len()
    }

    /// Process `frames` frames from planar inputs (one buffer per sink
    /// channel) into planar outputs (one per output channel). Allocation-free.
    pub fn process<I: AsRef<[f32]>, O: AsMut<[f32]>>(
        &mut self,
        inputs: &[I],
        outputs: &mut [O],
        frames: usize,
    ) -> Result<(), ProcessError> {
        self.process_unlimited(inputs, outputs, frames)?;
        if let Some(limiter) = &mut self.limiter {
            limiter.process(outputs, frames);
        }
        Ok(())
    }

    fn process_unlimited<I: AsRef<[f32]>, O: AsMut<[f32]>>(
        &mut self,
        inputs: &[I],
        outputs: &mut [O],
        frames: usize,
    ) -> Result<(), ProcessError> {
        if inputs.len() != self.input_channels || outputs.len() != self.channels.len() {
            return Err(ProcessError::ChannelCountMismatch);
        }
        if frames > self.capacity {
            return Err(ProcessError::BlockTooLarge {
                actual: frames,
                capacity: self.capacity,
            });
        }
        if inputs.iter().any(|input| input.as_ref().len() < frames)
            || outputs
                .iter_mut()
                .any(|output| output.as_mut().len() < frames)
        {
            return Err(ProcessError::BlockSizeMismatch);
        }
        let Self {
            channels,
            downmix,
            routes,
            mixed,
            summed,
            route_output,
            ..
        } = self;
        if let Some(downmix) = downmix {
            for (target, gains) in mixed.iter_mut().zip(downmix.iter()) {
                let target = &mut target[..frames];
                target.fill(0.0);
                for (input, gain) in inputs.iter().zip(gains) {
                    if *gain != 0.0 {
                        for (sample, source) in target.iter_mut().zip(input.as_ref()) {
                            *sample += source * gain;
                        }
                    }
                }
            }
        }
        let source = |index: usize| -> &[f32] {
            if downmix.is_some() {
                &mixed[index][..frames]
            } else {
                &inputs[index].as_ref()[..frames]
            }
        };
        if routes.is_empty() {
            for (index, (processor, output)) in channels.iter_mut().zip(outputs).enumerate() {
                processor.process(source(index), &mut output.as_mut()[..frames])?;
            }
            return Ok(());
        }
        for sum in summed.iter_mut() {
            sum[..frames].fill(0.0);
        }
        for route in routes.iter_mut() {
            route
                .convolver
                .process(source(route.input), &mut route_output[..frames])?;
            for (sample, value) in summed[route.output][..frames]
                .iter_mut()
                .zip(&route_output[..frames])
            {
                *sample += value;
            }
        }
        for ((processor, sum), output) in channels.iter_mut().zip(summed.iter()).zip(outputs) {
            processor.process(&sum[..frames], &mut output.as_mut()[..frames])?;
        }
        Ok(())
    }

    pub fn process_planar(
        &mut self,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
    ) -> Result<(), ProcessError> {
        let frames = inputs.first().map_or(0, |input| input.len());
        self.process(inputs, outputs, frames)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.reset();
        }
        for route in &mut self.routes {
            route.convolver.reset();
        }
        if let Some(limiter) = &mut self.limiter {
            limiter.gain = 1.0;
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
    fn held_tone_gain_limits_music_but_never_the_tone() {
        let profile = parse_text("test", "Filter 1: ON PK Fc 1000 Hz Gain 9 dB Q 1");
        let mut compiled =
            compile_profile(&profile, &CompileOptions::stereo(48_000, 128, "/tmp")).unwrap();
        // Hold a gain 8 dB above what is safe for full-scale music.
        let safe = compiled.analysis.effective_gain_db;
        crate::hold_output_gain(&mut compiled, safe + 8.0, 11.0).unwrap();
        assert!(compiled.output_limit);
        let run = |amplitude: f32| {
            let mut processor = ProfileProcessor::new(&compiled).unwrap();
            let mut peak = 0.0_f32;
            for block in 0..400 {
                let input = (0..128)
                    .map(|index| {
                        let time = (block * 128 + index) as f32 / 48_000.0;
                        amplitude * (std::f32::consts::TAU * 1_000.0 * time).sin()
                    })
                    .collect::<Vec<_>>();
                let mut left = vec![0.0; 128];
                let mut right = vec![0.0; 128];
                processor
                    .process_planar(&[&input, &input], &mut [&mut left, &mut right])
                    .unwrap();
                if block > 40 {
                    peak = left.iter().fold(peak, |peak, value| peak.max(value.abs()));
                }
            }
            20.0 * peak.log10()
        };
        // Full-scale music at the boosted frequency would land at +7 dBFS.
        assert!(run(1.0) <= -0.99, "music reached {:.2} dBFS", run(1.0));
        // The loudest tone, -12 dBFS, passes untouched: 9 dB of boost, the
        // safe gain, and the 8 dB hold.
        let tone = run(10.0_f32.powf(-12.0 / 20.0));
        let expected = -12.0 + 9.0 + safe as f32 + 8.0;
        assert!(
            (tone - expected).abs() < 0.05,
            "tone {tone:.2} dBFS, expected {expected:.2}"
        );
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

#[cfg(test)]
mod bench {
    use crate::{CompiledProfile, MatrixRoute, compile_bypass};

    /// `cargo test --release -p massiveeq-dsp surround_load -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn surround_load() {
        let mut profile: CompiledProfile = compile_bypass(48_000, 2048, 2);
        profile.input_channels = 8;
        let mut seed = 1_u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        for route in massiveeq_core::ImpulseLayout::Surround71.routes() {
            let impulse = (0..48_000)
                .map(|index| noise() * (-(index as f32) / 7_200.0).exp())
                .collect();
            profile.routes.push(MatrixRoute {
                input: route.input,
                output: route.output,
                kernel: crate::ConvolutionKernel {
                    sample_rate: 48_000,
                    impulse,
                    latency_frames: 0,
                },
            });
        }
        let mut processor = super::ProfileProcessor::new(&profile).unwrap();
        for frames in [1024_usize, 256] {
            let inputs = (0..8)
                .map(|_| (0..frames).map(|_| noise()).collect::<Vec<f32>>())
                .collect::<Vec<_>>();
            let mut outputs = vec![vec![0.0_f32; frames]; 2];
            let mut times = Vec::new();
            for _ in 0..(400 * 1024 / frames) {
                let started = std::time::Instant::now();
                processor.process(&inputs, &mut outputs, frames).unwrap();
                times.push(started.elapsed().as_secs_f64() * 1e3);
            }
            let average = times.iter().sum::<f64>() / times.len() as f64;
            let peak = times.iter().copied().fold(0.0, f64::max);
            let budget = frames as f64 / 48.0;
            println!(
                "{frames}-frame blocks ({budget:.1} ms budget): average {average:.2} ms, peak {peak:.2} ms"
            );
        }
    }
}
