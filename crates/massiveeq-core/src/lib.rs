//! Shared profile, analysis, storage, device, and PipeWire configuration logic.

pub mod analysis;
pub mod model;
pub mod parser;
pub mod pipewire;
pub mod storage;

pub use analysis::{ChannelAnalysis, ProfileAnalysis, analyze_profile, analyze_profile_preview};
pub use model::*;
pub use parser::{ParseError, parse_file, parse_text, serialize_profile};
pub use storage::{Library, Storage, StorageError};
