//! Output repair strategies.
//!
//! Attempts to fix LLM output that failed post-flight validation:
//! - JSON extraction from mixed text
//! - Truncation for over-length responses
//!
//! # Example
//!
//! ```rust
//! use gateway_core::prompt::{ConstraintProfile, OutputRepairer, ValidationIssue};
//!
//! let profile = ConstraintProfile::default();
//! let repairer = OutputRepairer::new(profile);
//!
//! let issues = vec![
//!     ValidationIssue::hard("Invalid JSON: expected value at line 1 column 1"),
//! ];
//!
//! let mixed = "Here is the result: {\"key\": \"value\"}";
//! let repaired = repairer.repair(mixed, &issues);
//! assert_eq!(repaired, Some("{\"key\": \"value\"}".to_string()));
//! ```

use super::constraints::ConstraintProfile;
use super::validator::ValidationIssue;

/// Attempts to repair LLM output based on validation issues.
///
/// Currently supports two repair strategies:
/// - **JSON extraction**: Finds valid JSON in mixed text/JSON output
/// - **Truncation**: Truncates output that exceeds token limits
///
/// # Example
///
/// ```rust
/// use gateway_core::prompt::{ConstraintProfile, OutputRepairer, ValidationIssue};
///
/// let profile = ConstraintProfile::default();
/// let repairer = OutputRepairer::new(profile);
///
/// let issues = vec![
///     ValidationIssue::hard("Invalid JSON: expected value"),
/// ];
///
/// let output = "Result: [1, 2, 3] done";
/// let repaired = repairer.repair(output, &issues);
/// assert_eq!(repaired, Some("[1, 2, 3]".to_string()));
/// ```
pub struct OutputRepairer {
    /// The constraint profile used for repair parameters (e.g., token limits).
    profile: ConstraintProfile,
}

impl OutputRepairer {
    /// Create a new output repairer with the given constraint profile.
    ///
    /// # Arguments
    ///
    /// * `profile` - The constraint profile defining repair parameters
    pub fn new(profile: ConstraintProfile) -> Self {
        Self { profile }
    }

    /// Attempt to repair LLM output based on validation issues.
    ///
    /// Returns `Some(repaired)` if repair was successful, `None` if the output
    /// cannot be automatically repaired.
    ///
    /// # Arguments
    ///
    /// * `output` - The original LLM output
    /// * `issues` - Validation issues found during post-flight validation
    pub fn repair(&self, output: &str, issues: &[ValidationIssue]) -> Option<String> {
        // Attempt 1: If JSON parsing fails but output contains JSON, extract it
        let has_json_issue = issues.iter().any(|i| i.message.contains("Invalid JSON"));
        if has_json_issue {
            if let Some(json) = self.extract_json(output) {
                return Some(json);
            }
        }

        // Attempt 2: If output too long, truncate to within limits
        let has_length_issue = issues.iter().any(|i| i.message.contains("exceed"));
        if has_length_issue {
            if let Some(truncated) = self.truncate_output(output) {
                return Some(truncated);
            }
        }

        None
    }

    /// Attempt to extract valid JSON from mixed text/JSON output.
    ///
    /// Searches for JSON objects (between `{` and `}`) and arrays
    /// (between `[` and `]`) in the output.
    fn extract_json(&self, output: &str) -> Option<String> {
        // Try to find JSON object in the output (between first { and last })
        if let (Some(start), Some(end)) = (output.find('{'), output.rfind('}')) {
            if start < end {
                let candidate = &output[start..=end];
                if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                    return Some(candidate.to_string());
                }
            }
        }
        // Try array
        if let (Some(start), Some(end)) = (output.find('['), output.rfind(']')) {
            if start < end {
                let candidate = &output[start..=end];
                if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                    return Some(candidate.to_string());
                }
            }
        }
        None
    }

    /// Truncate output to within token limits.
    ///
    /// Uses a rough approximation of 4 characters per token.
    fn truncate_output(&self, output: &str) -> Option<String> {
        // Rough chars = response_max tokens * 4 chars/token
        let max_chars = self.profile.token_limits.response_max * 4;
        if output.len() > max_chars && max_chars > 0 {
            // Truncate at char boundary
            let truncated = &output[..output.floor_char_boundary(max_chars)];
            return Some(truncated.to_string());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::TokenLimits;
    use std::collections::HashMap;

    // ==================== JSON Extraction Tests ====================

    #[test]
    fn test_repair_extracts_json_object() {
        let profile = ConstraintProfile::default();
        let repairer = OutputRepairer::new(profile);

        let issues = vec![ValidationIssue::hard(
            "Invalid JSON: expected value at line 1 column 1",
        )];

        let mixed = "Here is the result: {\"key\": \"value\", \"count\": 42} as requested";
        let repaired = repairer.repair(mixed, &issues);
        assert!(repaired.is_some());
        assert_eq!(repaired.unwrap(), r#"{"key": "value", "count": 42}"#);
    }

    #[test]
    fn test_repair_extracts_json_array() {
        let profile = ConstraintProfile::default();
        let repairer = OutputRepairer::new(profile);

        let issues = vec![ValidationIssue::hard(
            "Invalid JSON: expected value at line 1 column 1",
        )];

        let mixed_array = "The items are: [1, 2, 3] and that's it";
        let repaired = repairer.repair(mixed_array, &issues);
        assert!(repaired.is_some());
        assert_eq!(repaired.unwrap(), "[1, 2, 3]");
    }

    #[test]
    fn test_repair_returns_none_for_pure_text() {
        let profile = ConstraintProfile::default();
        let repairer = OutputRepairer::new(profile);

        let issues = vec![ValidationIssue::hard(
            "Invalid JSON: expected value at line 1 column 1",
        )];

        let pure_text = "This is just plain text with no JSON at all";
        let repaired = repairer.repair(pure_text, &issues);
        assert!(repaired.is_none());
    }

    #[test]
    fn test_repair_returns_none_for_broken_json() {
        let profile = ConstraintProfile::default();
        let repairer = OutputRepairer::new(profile);

        let issues = vec![ValidationIssue::hard(
            "Invalid JSON: expected value at line 1 column 1",
        )];

        let bad_json = "Here is some { broken json without closing";
        let repaired = repairer.repair(bad_json, &issues);
        assert!(repaired.is_none());
    }

    // ==================== Truncation Tests ====================

    #[test]
    fn test_repair_truncates_long_output() {
        let limits = TokenLimits {
            system_prompt_max: 8000,
            response_max: 10, // 10 tokens * 4 chars = 40 char limit
            section_budgets: HashMap::new(),
        };

        let profile = ConstraintProfile::default().with_token_limits(limits);
        let repairer = OutputRepairer::new(profile);

        let issues = vec![ValidationIssue::soft(
            "Response may exceed token limit: ~100 tokens (limit: 10)",
        )];

        let long_output = "A".repeat(200);
        let repaired = repairer.repair(&long_output, &issues);
        assert!(repaired.is_some());
        assert!(repaired.as_ref().unwrap().len() <= 40);
    }

    #[test]
    fn test_repair_no_truncation_within_limit() {
        let limits = TokenLimits {
            system_prompt_max: 8000,
            response_max: 4000,
            section_budgets: HashMap::new(),
        };

        let profile = ConstraintProfile::default().with_token_limits(limits);
        let repairer = OutputRepairer::new(profile);

        let issues = vec![ValidationIssue::soft("Response may exceed token limit")];

        // Short output that doesn't actually exceed the char limit
        let short_output = "short";
        let repaired = repairer.repair(short_output, &issues);
        assert!(repaired.is_none());
    }

    // ==================== No Matching Issue Tests ====================

    #[test]
    fn test_repair_returns_none_for_unrelated_issues() {
        let profile = ConstraintProfile::default();
        let repairer = OutputRepairer::new(profile);

        let issues = vec![ValidationIssue::soft("Some other issue")];

        let output = "Some output text";
        let repaired = repairer.repair(output, &issues);
        assert!(repaired.is_none());
    }

    #[test]
    fn test_repair_returns_none_for_empty_issues() {
        let profile = ConstraintProfile::default();
        let repairer = OutputRepairer::new(profile);

        let output = "Some output text";
        let repaired = repairer.repair(output, &[]);
        assert!(repaired.is_none());
    }

    #[test]
    fn test_extract_json_nested_object() {
        let profile = ConstraintProfile::default();
        let repairer = OutputRepairer::new(profile);

        let issues = vec![ValidationIssue::hard("Invalid JSON: expected value")];

        let nested = "Result: {\"outer\": {\"inner\": true}} done";
        let repaired = repairer.repair(nested, &issues);
        assert_eq!(repaired, Some("{\"outer\": {\"inner\": true}}".to_string()));
    }
}
