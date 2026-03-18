//! Git integration module
//!
//! Provides Git operations for version control within user sessions.

pub mod operations;
pub mod repository;

pub use operations::{GitDiff, GitFileDiff, GitOperations, GitStatus};
pub use repository::{GitRepository, RepositoryManager};
