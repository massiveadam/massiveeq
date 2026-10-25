//! Shared profile, analysis, storage, device, and PipeWire configuration logic.

pub mod analysis;
#[cfg(test)]
mod apo_golden;
pub mod curve;
pub mod layout;
pub mod measurement;
pub mod model;
pub mod parser;
pub mod pipewire;
pub mod storage;
pub mod upgrade;
pub mod volume;

pub use analysis::{ChannelAnalysis, ProfileAnalysis, analyze_profile, analyze_profile_preview};
pub use curve::{
    CurveCompensation, CurveKind, CurveMetadata, CurvePoint, CurveTransducer, Curves, Excursion,
    FIXTURES, MAX_PLACEMENTS, MeasurementField, alignment_level, curve_at, excursions, fixture_key,
    is_range, measurement_field, parse_curve,
};
pub use layout::{ImpulseLayout, ImpulseRoute, SURROUND_71_POSITIONS, validate_impulse};
pub use measurement::{
    MAX_MEASUREMENT_POINTS, MAX_MEASUREMENT_TAKES, MeasurementChannels, MeasurementEar,
    MeasurementOutput, MeasurementSet, MeasurementTake, measurement_ear_hint,
};
pub use model::*;
pub use parser::{ParseError, parse_file, parse_text, serialize_profile};
pub use storage::{
    COMPARISON_BYPASS_ID, ComparisonSet, Library, MAX_COMPARISON_PROFILES, Storage, StorageError,
};
pub use volume::{
    VOLUME_JUMP_LIMIT, VOLUME_RANGE_DB, VolumeSettings, is_volume_jump, level_db, level_gain,
    volume_level,
};
