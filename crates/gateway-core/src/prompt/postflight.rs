//! Post-flight validation of LLM output.
//!
//! Validates output after receiving from the LLM:
//! - JSON schema validation
//! - Response length validation
//!
//! # Example
//!
//! ```rust
//! use gateway_core::prompt::{
//!     ConstraintProfile, PostflightValidator, OutputConstraint, ValidationMode,
//! };
//! use serde_json::json;
//!
//! let constraint = OutputConstraint {
//!     name: "json".to_string(),
//!     description: "JSON required".to_string(),
//!     json_schema: Some(json!({"type": "object"})),
//!     prompt_injection: "Respond with JSON".to_string(),
//!     validation_mode: ValidationMode::Strict,
//!     enabled: true,
//! };
//!
//! let profile = ConstraintProfile::default().with_output_constraint(constraint);
//! let validator = PostflightValidator::new(profile);
//!
//! let result = validator.validate("This is not JSON");
//! assert!(!result.is_valid());
//! ```

use super::constraints::ConstraintProfile;
use super::validator::{ValidationIssue, ValidationResult};

/// Post-flight validator for LLM output.
///
/// Validates LLM output after receiving it, checking for:
/// - Valid JSON if a json_schema constraint is enabled
/// - Response length against token limits
///
/// # Example
///
/// ```rust
/// use gateway_core::prompt::{ConstraintProfile, PostflightValidator};
///
/// let profile = ConstraintProfile::default();
/// let validator = PostflightValidator::new(profile);
///
/// let result = validator.validate("Short response");
/// assert!(result.is_valid());
/// ```
pub struct PostflightValidator {
    /// The constraint profile to validate against.
    profile: ConstraintProfile,
}

impl PostflightValidator {
    /// Create a new post-flight validator with the given constraint profile.
    ///
    /// # Arguments
    ///
    /// * `profile` - The constraint profile defining validation rules
    pub fn new(profile: ConstraintProfile) -> Self {
        Self { profile }
    }

    /// Validate LLM output against post-flight constraints.
    ///
    /// Checks for valid JSON (if configured) and response length.
    ///
    /// # Arguments
    ///
    /// * `output` - The LLM's response
    ///
    /// # Returns
    ///
    /// A `ValidationResult` containing any issues found.
    pub fn validate(&self, output: &str) -> ValidationResult {
        let mut result = ValidationResult::valid();

        self.check_json_output(output, &mut result);
        self.check_output_length(output, &mut result);

        result
    }

    /// Check output for valid JSON if schema is configured.
    fn check_json_output(&self, output: &str, result: &mut ValidationResult) {
        for constraint in &self.profile.output_constraints {
            if !constraint.enabled {
                continue;
            }

            if constraint.json_schema.is_some() {
                match serde_json::from_str::<serde_json::Value>(output) {
                    Ok(_json) => {
                        // JSON is valid - full schema validation would require
                        // a JSON schema validation library like jsonschema
                        // For now, we just validate that it's parseable JSON
                    }
                    Err(e) => {
                        result.add_issue(
                            ValidationIssue::hard(format!("Invalid JSON: {}", e))
                                .with_suggestion("Please respond with valid JSON only."),
                        );
                    }
                }
            }
        }
    }

    /// Check output length against token limits.
    ///
    /// Note: This is a simple character-based approximation. Real token
    /// counting would require a tokenizer for the specific model.
    fn check_output_length(&self, output: &str, result: &mut ValidationResult) {
        // Rough approximation: ~4 characters per token for English text
        let estimated_tokens = output.len() / 4;
        let limit = self.profile.token_limits.response_max;

        if estimated_tokens > limit {
            result.add_issue(
                ValidationIssue::soft(format!(
                    "Response may exceed token limit: ~{} tokens (limit: {})",
                    estimated_tokens, limit
                ))
                .with_suggestion("Consider requesting a more concise response."),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::{ConstraintLevel, OutputConstraint, TokenLimits, ValidationMode};
    use std::collections::HashMap;

    // ==================== JSON Output Validation Tests ====================

    #[test]
    fn test_postflight_valid_json() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            description: "JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Use JSON".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = PostflightValidator::new(profile);

        let result = validator.validate(r#"{"key": "value"}"#);
        assert!(result.is_valid());
    }

    #[test]
    fn test_postflight_invalid_json() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            description: "JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Use JSON".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = PostflightValidator::new(profile);

        let result = validator.validate("This is not JSON at all");
        assert!(!result.is_valid());

        let issue = &result.issues[0];
        assert!(matches!(issue.level, ConstraintLevel::Hard));
        assert!(issue.message.contains("Invalid JSON"));
    }

    #[test]
    fn test_postflight_malformed_json() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            description: "JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Use JSON".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = PostflightValidator::new(profile);

        // Missing closing brace
        let result = validator.validate(r#"{"key": "value""#);
        assert!(!result.is_valid());

        // Trailing comma
        let result = validator.validate(r#"{"key": "value",}"#);
        assert!(!result.is_valid());
    }

    #[test]
    fn test_postflight_json_constraint_disabled() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            description: "JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Use JSON".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: false,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = PostflightValidator::new(profile);

        let result = validator.validate("Not JSON");
        assert!(result.is_valid());
    }

    #[test]
    fn test_postflight_no_json_schema() {
        let constraint = OutputConstraint {
            name: "text".to_string(),
            description: "Text output".to_string(),
            json_schema: None,
            prompt_injection: "".to_string(),
            validation_mode: ValidationMode::WarnOnly,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = PostflightValidator::new(profile);

        let result = validator.validate("Just plain text");
        assert!(result.is_valid());
    }

    #[test]
    fn test_postflight_empty_output() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            enabled: true,
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = PostflightValidator::new(profile);

        let result = validator.validate("");
        assert!(!result.is_valid());
    }

    // ==================== Output Length Tests ====================

    #[test]
    fn test_postflight_length_within_limit() {
        let profile = ConstraintProfile::default();
        let validator = PostflightValidator::new(profile);

        let result = validator.validate("Short response");
        assert!(result.is_valid());
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_postflight_length_exceeds_limit() {
        let limits = TokenLimits {
            system_prompt_max: 8000,
            response_max: 10,
            section_budgets: HashMap::new(),
        };

        let profile = ConstraintProfile::default().with_token_limits(limits);
        let validator = PostflightValidator::new(profile);

        let result = validator.validate("This is a longer response that exceeds the limit");
        assert!(result.has_warnings());
        assert!(result.is_valid()); // Length is Soft warning, not Hard
    }

    #[test]
    fn test_postflight_valid_json_array() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            description: "JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "array"})),
            prompt_injection: "Use JSON".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = PostflightValidator::new(profile);

        let result = validator.validate("[1, 2, 3]");
        assert!(result.is_valid());
    }
}
