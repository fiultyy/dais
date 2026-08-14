pub mod action;
pub mod action_result;
mod citation;
pub mod convert;
pub mod file_locations;
/// Orchestration plane — always compiled; gated by FeatureFlag::Orchestration
/// at runtime in the app layer. See orchestration/mod.rs for details.
pub mod orchestration;

pub use citation::{AIAgentCitation, UnknownCitationTypeError};
pub use file_locations::{group_file_contexts_for_display, FileLocations};
