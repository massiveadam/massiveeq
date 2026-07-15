//! Rate-aware, allocation-free-at-runtime DSP for MassiveEQ.

mod analysis;
mod biquad;
mod compiler;
mod convolution;
mod graphic;
mod runtime;

pub use analysis::{ChannelResponse, CompiledAnalysis, ResponsePoint};
pub use biquad::{Biquad, BiquadCoefficients};
pub use compiler::{
    CompileError, CompileOptions, CompiledProfile, attenuate_for_comparison, compile_bypass,
    compile_bypass_with_gain, compile_profile, perceived_output_level_db,
};
pub use convolution::{ConvolutionKernel, IrData, load_ir};
pub use graphic::{GraphicDesign, design_graphic_eq};
pub use runtime::{ChannelProcessor, ProcessError, ProfileProcessor};
