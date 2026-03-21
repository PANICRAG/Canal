//! Self-Iteration Learning System
//!
//! Tracks execution, analyzes failures, records learnings, updates skills.

mod tracker;
mod updater;

pub use tracker::{ExecutionLog, ExecutionTracker, ToolExecution};
pub use updater::{LearnedIssue, SkillUpdater};
