//! Artifacts System
//!
//! This module provides artifact management for visual result display.
//! Artifacts are rich, interactive UI components that display task outputs
//! to users.

pub mod builder;
pub mod store;
pub mod types;

pub use builder::ArtifactBuilder;
pub use store::ArtifactStore;
pub use types::*;
