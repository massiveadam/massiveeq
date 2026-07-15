use crate::{
    BiquadCoefficients, CompiledAnalysis, ConvolutionKernel, analysis::analyze_compiled,
    convolution::load_ir, design_graphic_eq,
};
use massiveeq_core::{Channel, ChannelSelection, ProfileDocument};
use std::{
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct CompileOptions {
    pub sample_rate: u32,
    pub quantum: u32,
    pub output_channels: u32,
    pub manual_trim_db: f64,
    pub profile_dir: PathBuf,
}

impl CompileOptions {
    pub fn stereo(sample_rate: u32, quantum: u32, profile_dir: impl Into<PathBuf>) -> Self {
        Self {
            sample_rate,
            quantum,
            output_channels: 2,
            manual_trim_db: 0.0,
            profile_dir: profile_dir.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("profile contains parser errors")]
    ParserErrors,
    #[error("parametric/GraphicEQ processing and convolution cannot be active in the same profile")]
    MixedProcessingModes,
    #[error("per-ear profiles cannot be assigned to a mono output")]
    PerEarOnMono,
    #[error("unsupported output channel count {0}; MassiveEQ supports mono and stereo")]
    UnsupportedChannels(u32),
    #[error("invalid sample rate {0}")]
    InvalidSampleRate(u32),
    #[error("invalid PipeWire quantum {0}")]
    InvalidQuantum(u32),
    #[error("{0}")]
    Filter(#[from] crate::biquad::BiquadError),
    #[error("{0}")]
    Graphic(#[from] crate::graphic::GraphicError),
    #[error("{0}")]
    Convolution(#[from] crate::convolution::IrError),
    #[error("convolution asset {0} does not exist")]
    MissingAsset(PathBuf),
    #[error("convolution asset escapes the profile directory: {0}")]
    AssetOutsideProfile(PathBuf),
    #[error("compiled response contains a non-finite value")]
    NonFiniteResponse,
}

#[derive(Debug, Clone)]
pub struct CompiledChannel {
    pub gain_linear: f32,
    pub biquads: Vec<BiquadCoefficients>,
    pub peak_candidates: Vec<f64>,
    pub convolutions: Vec<ConvolutionKernel>,
}

#[derive(Debug, Clone)]
pub struct CompiledProfile {
    pub sample_rate: u32,
    pub quantum: u32,
    pub output_channels: u32,
    pub channels: Vec<CompiledChannel>,
    pub analysis: CompiledAnalysis,
    pub latency_frames: u32,
}

pub fn compile_bypass(sample_rate: u32, quantum: u32, output_channels: u32) -> CompiledProfile {
    let max_frequency = 20_000.0_f64.min(sample_rate as f64 * 0.499);
    let response = (0..512)
        .map(|index| crate::ResponsePoint {
            frequency: 20.0 * (max_frequency / 20.0).powf(index as f64 / 511.0),
            gain_db: 0.0,
        })
        .collect::<Vec<_>>();
    CompiledProfile {
        sample_rate,
        quantum,
        output_channels,
        channels: (0..output_channels)
            .map(|_| CompiledChannel {
                gain_linear: 1.0,
                biquads: Vec::new(),
                peak_candidates: Vec::new(),
                convolutions: Vec::new(),
            })
            .collect(),
        analysis: CompiledAnalysis {
            channels: (0..output_channels)
                .map(|_| crate::ChannelResponse {
                    source_preamp_db: 0.0,
                    uncorrected_peak_db: 0.0,
                    response: response.clone(),
                })
                .collect(),
            match_gain_db: 0.0,
            manual_trim_db: 0.0,
            safety_attenuation_db: 0.0,
            effective_gain_db: 0.0,
            final_peak_db: 0.0,
            headroom_limited: false,
        },
        latency_frames: 0,
    }
}

pub fn compile_profile(
    profile: &ProfileDocument,
    options: &CompileOptions,
) -> Result<CompiledProfile, CompileError> {
    let has_eq = !profile.filters.is_empty() || !profile.graphic_eqs.is_empty();
    if has_eq && !profile.convolutions.is_empty() {
        return Err(CompileError::MixedProcessingModes);
    }
    if !profile.is_activatable() {
        return Err(CompileError::ParserErrors);
    }
    if !(8_000..=384_000).contains(&options.sample_rate) {
        return Err(CompileError::InvalidSampleRate(options.sample_rate));
    }
    if options.quantum == 0 || options.quantum > 65_536 {
        return Err(CompileError::InvalidQuantum(options.quantum));
    }
    if !matches!(options.output_channels, 1 | 2) {
        return Err(CompileError::UnsupportedChannels(options.output_channels));
    }
    if options.output_channels == 1 && has_per_ear_content(profile) {
        return Err(CompileError::PerEarOnMono);
    }

    let profile_root = if options.profile_dir.exists() {
        fs::canonicalize(&options.profile_dir).unwrap_or_else(|_| options.profile_dir.clone())
    } else {
        options.profile_dir.clone()
    };
    let channel_ids: &[Channel] = if options.output_channels == 1 {
        &[Channel::Left]
    } else {
        &[Channel::Left, Channel::Right]
    };
    let mut channels = Vec::with_capacity(channel_ids.len());
    let mut source_preamps = Vec::with_capacity(channel_ids.len());
    for (output_index, channel) in channel_ids.iter().copied().enumerate() {
        let active_filters = profile
            .filters_for(channel)
            .filter(|filter| filter.enabled)
            .collect::<Vec<_>>();
        let biquads = active_filters
            .iter()
            .map(|filter| BiquadCoefficients::from_filter(filter, options.sample_rate as f64))
            .collect::<Result<Vec<_>, _>>()?;
        let peak_candidates = active_filters
            .iter()
            .flat_map(|filter| {
                let width = 2.0_f64.powf(1.0 / (8.0 * filter.q.max(0.01)));
                [
                    filter.frequency / width,
                    filter.frequency,
                    filter.frequency * width,
                ]
            })
            .filter(|frequency| {
                *frequency >= 20.0 && *frequency <= options.sample_rate as f64 * 0.499
            })
            .collect();
        let graphics = profile
            .graphic_eqs
            .iter()
            .filter(|equalizer| equalizer.channels.contains(channel))
            .collect::<Vec<_>>();
        let mut convolutions = Vec::new();
        if !graphics.is_empty() {
            let design = design_graphic_eq(&graphics, options.sample_rate)?;
            convolutions.push(ConvolutionKernel {
                sample_rate: options.sample_rate,
                impulse: design.impulse,
                latency_frames: options.quantum + design.latency_frames,
            });
        }
        for convolution in profile
            .convolutions
            .iter()
            .filter(|item| item.channels.contains(channel))
        {
            let path = resolve_asset(&profile_root, &convolution.path)?;
            let ir = load_ir(&path, options.sample_rate)?;
            let ir_channel = match convolution.channels {
                ChannelSelection::All => output_index % ir.channels.len(),
                ChannelSelection::Left | ChannelSelection::Right => 0,
            };
            let impulse = ir.channels[ir_channel].clone();
            let inherent_latency = impulse
                .iter()
                .enumerate()
                .max_by(|(_, left), (_, right)| left.abs().total_cmp(&right.abs()))
                .map(|(index, _)| index as u32)
                .unwrap_or_default();
            convolutions.push(ConvolutionKernel {
                sample_rate: options.sample_rate,
                impulse,
                latency_frames: options.quantum + inherent_latency,
            });
        }
        source_preamps.push(profile.preamp_for(channel));
        channels.push(CompiledChannel {
            gain_linear: 1.0,
            biquads,
            peak_candidates,
            convolutions,
        });
    }

    let analysis = analyze_compiled(
        &channels,
        &source_preamps,
        options.sample_rate,
        options.manual_trim_db,
    )?;
    for (channel, source_preamp) in channels.iter_mut().zip(source_preamps) {
        channel.gain_linear = db_to_linear(source_preamp + analysis.effective_gain_db) as f32;
    }
    let latency_frames = channels
        .iter()
        .map(|channel| {
            channel
                .convolutions
                .iter()
                .map(|kernel| kernel.latency_frames)
                .sum::<u32>()
        })
        .max()
        .unwrap_or_default();
    Ok(CompiledProfile {
        sample_rate: options.sample_rate,
        quantum: options.quantum,
        output_channels: options.output_channels,
        channels,
        analysis,
        latency_frames,
    })
}

fn resolve_asset(profile_root: &Path, requested: &Path) -> Result<PathBuf, CompileError> {
    let joined = if requested.is_absolute() {
        requested.to_owned()
    } else {
        profile_root.join(requested)
    };
    if !joined.exists() {
        return Err(CompileError::MissingAsset(joined));
    }
    let canonical = fs::canonicalize(&joined).map_err(|_| CompileError::MissingAsset(joined))?;
    if !canonical.starts_with(profile_root) {
        return Err(CompileError::AssetOutsideProfile(canonical));
    }
    Ok(canonical)
}

fn has_per_ear_content(profile: &ProfileDocument) -> bool {
    profile
        .preamps
        .iter()
        .any(|value| value.channels != ChannelSelection::All)
        || profile
            .filters
            .iter()
            .any(|value| value.channels != ChannelSelection::All)
        || profile
            .graphic_eqs
            .iter()
            .any(|value| value.channels != ChannelSelection::All)
        || profile
            .convolutions
            .iter()
            .any(|value| value.channels != ChannelSelection::All)
}

fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use massiveeq_core::parse_text;
    use std::io::Write;

    fn write_float_wav(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
        let data_size = (samples.len() * 4) as u32;
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data_size).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&3_u16.to_le_bytes()).unwrap();
        file.write_all(&channels.to_le_bytes()).unwrap();
        file.write_all(&rate.to_le_bytes()).unwrap();
        file.write_all(&(rate * channels as u32 * 4).to_le_bytes())
            .unwrap();
        file.write_all(&(channels * 4).to_le_bytes()).unwrap();
        file.write_all(&32_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_size.to_le_bytes()).unwrap();
        for sample in samples {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
    }

    #[test]
    fn rejects_mixed_convolution_and_eq() {
        let profile = parse_text(
            "test",
            "Filter: ON PK Fc 1000 Hz Gain 3 dB Q 1\nConvolution: missing.wav",
        );
        let result = compile_profile(&profile, &CompileOptions::stereo(48_000, 256, "/tmp"));
        assert!(matches!(result, Err(CompileError::MixedProcessingModes)));
    }

    #[test]
    fn narrow_high_gain_filter_still_receives_safety_attenuation() {
        let profile = parse_text("test", "Filter: ON PK Fc 1000 Hz Gain 60 dB Q 1000");
        let compiled =
            compile_profile(&profile, &CompileOptions::stereo(48_000, 256, "/tmp")).unwrap();
        assert!(compiled.analysis.final_peak_db <= -0.999);
        assert!(compiled.analysis.safety_attenuation_db < -50.0);
    }

    #[test]
    fn k_weighted_match_is_exact_for_a_flat_gain_change() {
        let profile = parse_text("test", "Preamp: 6 dB");
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let compiled =
                compile_profile(&profile, &CompileOptions::stereo(rate, 256, "/tmp")).unwrap();
            assert!((compiled.analysis.match_gain_db + 6.0).abs() < 0.01);
            assert!((compiled.analysis.final_peak_db + 1.0).abs() < 0.01);
            assert!(compiled.analysis.channels.iter().all(|channel| {
                channel
                    .response
                    .iter()
                    .all(|point| point.gain_db.abs() < 0.01)
            }));
        }
    }

    #[test]
    fn output_preamp_does_not_move_the_display_response() {
        let profile = parse_text("test", "Filter: ON PK Fc 1000 Hz Gain 8 dB Q 1");
        let normal =
            compile_profile(&profile, &CompileOptions::stereo(48_000, 256, "/tmp")).unwrap();
        let mut adjusted_options = CompileOptions::stereo(48_000, 256, "/tmp");
        adjusted_options.manual_trim_db = -20.0;
        let adjusted = compile_profile(&profile, &adjusted_options).unwrap();

        assert_ne!(
            normal.analysis.effective_gain_db,
            adjusted.analysis.effective_gain_db
        );
        for (normal, adjusted) in normal.analysis.channels[0]
            .response
            .iter()
            .zip(&adjusted.analysis.channels[0].response)
        {
            assert!((normal.gain_db - adjusted.gain_db).abs() < 1e-12);
        }
    }

    #[test]
    fn convolution_gain_is_included_in_match_and_headroom() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("assets")).unwrap();
        write_float_wav(&temp.path().join("assets/boost.wav"), 48_000, 1, &[4.0]);
        let profile = parse_text("test", "Convolution: assets/boost.wav");
        let compiled =
            compile_profile(&profile, &CompileOptions::stereo(48_000, 128, temp.path())).unwrap();
        assert!(compiled.analysis.final_peak_db <= -0.999);
        assert!(compiled.analysis.match_gain_db < -11.9);
    }

    #[test]
    fn stereo_ir_uses_round_robin_channels_and_resamples() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("assets")).unwrap();
        write_float_wav(
            &temp.path().join("assets/stereo.wav"),
            44_100,
            2,
            &[1.0, 0.5, 0.0, 0.0],
        );
        let profile = parse_text("test", "Convolution: assets/stereo.wav");
        let compiled =
            compile_profile(&profile, &CompileOptions::stereo(48_000, 128, temp.path())).unwrap();
        assert_eq!(compiled.channels.len(), 2);
        assert!(compiled.channels[0].convolutions[0].impulse[0] > 0.8);
        assert!(compiled.channels[1].convolutions[0].impulse[0] > 0.4);
        assert_eq!(compiled.channels[0].convolutions[0].sample_rate, 48_000);
    }
}
