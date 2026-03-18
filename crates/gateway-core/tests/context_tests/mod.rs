//! Context Hierarchy Test Infrastructure
//!
//! This module provides test fixtures, helpers, and utilities for testing
//! the context hierarchy system in Canal.
//!
//! ## Modules
//!
//! - `fixtures`: Test fixtures for creating temporary directories and platform rules
//! - `helpers`: Mock context creation and assertion utilities

pub mod fixtures;
pub mod helpers;

pub use fixtures::TestFixture;
pub use helpers::*;

// Re-export commonly used test utilities
pub use tempfile::TempDir;
