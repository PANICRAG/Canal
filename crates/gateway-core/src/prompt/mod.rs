//! Prompt Constraint System (A19)
//!
//! This module implements a three-layer Prompt Constraint System:
//!
//! - **Hard Constraints**: Security boundaries, format enforcement (system config)
//! - **Soft Constraints**: Role anchoring, task rules (admin config)
//! - **User Preferences**: Custom instructions, examples (App UI editable)
//!
//! # Feature Gate
//!
//! This module is gated behind the `prompt-constraints` feature:
//!
//! ```toml
//! [features]
//! prompt-constraints = []
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::prompt::{ConstraintProfile, ConstraintLevel, ReasoningMode};
//!
//! let profile = ConstraintProfile::default();
//! assert_eq!(profile.reasoning_mode, ReasoningMode::Direct);
//! ```
//!
//! # Validation
//!
//! The [`ConstraintValidator`] validates user input and LLM output against
//! constraint profiles:
//!
//! ```rust,ignore
//! use gateway_core::prompt::{ConstraintProfile, ConstraintValidator};
//!
//! let profile = ConstraintProfile::default_secure();
//! let validator = ConstraintValidator::new(profile);
//!
//! // Pre-flight validation
//! let result = validator.validate_input("rm -rf /");
//! assert!(!result.is_valid());
//!
//! // Post-flight validation
//! let result = validator.validate_output(r#"{"action": "done"}"#);
//! assert!(result.is_valid());
//! ```

mod constraints;
mod postflight;
mod preflight;
mod profiles;
mod repair;
mod user_overrides;
mod validator;

pub use constraints::{
    ConstraintLevel, ConstraintProfile, OutputConstraint, ReasoningMode, RoleAnchor,
    SecurityBoundary, TokenLimits, ValidationMode,
};

pub use profiles::{ProfileRegistry, ProfileSummary};

pub use user_overrides::{CustomExample, PromptSectionRef, ToolPreferences, UserPromptOverrides};

pub use postflight::PostflightValidator;
pub use preflight::PreflightGuard;
pub use repair::OutputRepairer;
pub use validator::{ConstraintValidator, ValidationIssue, ValidationResult};
