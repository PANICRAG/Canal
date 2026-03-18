//! Skill definitions and parser.
//!
//! Extracted from `gateway-core::agent::skills` to break circular deps.

pub mod definition;
pub mod parser;

pub use definition::Skill;
pub use parser::SkillParser;
