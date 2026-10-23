//! Shared profile, analysis, storage, device, and PipeWire configuration logic.

pub mod analysis;
pub mod curve;
pub mod layout;
pub mod model;
pub mod parser;
pub mod pipewire;
pub mod storage;
pub mod upgrade;

pub use analysis::{ChannelAnalysis, ProfileAnalysis, analyze_profile, analyze_profile_preview};
pub use curve::{
    CurveCompensation, CurveKind, CurveMetadata, CurvePoint, CurveTransducer, Curves, parse_curve,
};
pub use layout::{ImpulseLayout, ImpulseRoute, SURROUND_71_POSITIONS, validate_impulse};
pub use model::*;
pub use parser::{ParseError, parse_file, parse_text, serialize_profile};
pub use storage::{
    COMPARISON_BYPASS_ID, ComparisonSet, Library, MAX_COMPARISON_PROFILES, Storage, StorageError,
};
