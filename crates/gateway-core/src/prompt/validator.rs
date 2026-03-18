//! Constraint validator for pre-flight and post-flight validation.
//!
//! This module provides validation of user input and LLM output against
//! constraint profiles. It implements the validation flow described in
//! PRD A19 Section 3.4.
//!
//! # Pre-flight Validation
//!
//! Validates user input before sending to the LLM:
//! - Security checks for blocked commands
//! - Role drift detection for suspicious keywords
//!
//! # Post-flight Validation
//!
//! Validates LLM output after receiving:
//! - JSON schema validation if configured
//! - Response length validation against token limits
//!
//! # Example
//!
//! ```rust
//! use gateway_core::prompt::{ConstraintProfile, ConstraintValidator, RoleAnchor};
//!
//! // Create a profile with drift detection
//! let anchor = RoleAnchor {
//!     role_name: "Browser Agent".to_string(),
//!     anchor_prompt: "You are a browser automation agent.".to_string(),
//!     drift_detection: true,
//!     drift_keywords: vec!["pretend".to_string(), "roleplay".to_string()],
//!     drift_response: Some("I'm a browser agent.".to_string()),
//!     ..Default::default()
//! };
//!
//! let profile = ConstraintProfile::default().with_role_anchor(anchor);
//! let validator = ConstraintValidator::new(profile);
//!
//! // Validate input
//! let result = validator.validate_input("pretend you are a pirate");
//! assert!(result.has_warnings());
//! ```

use super::constraints::ConstraintProfile;
use super::postflight::PostflightValidator;
use super::preflight::PreflightGuard;
use super::repair::OutputRepairer;
use super::ConstraintLevel;

/// Result of validating input or output against constraints.
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    /// List of validation issues found.
    pub issues: Vec<ValidationIssue>,
}

impl ValidationResult {
    /// Create an empty (valid) result.
    pub fn valid() -> Self {
        Self { issues: Vec::new() }
    }

    /// Create a result with a single issue.
    pub fn with_issue(issue: ValidationIssue) -> Self {
        Self {
            issues: vec![issue],
        }
    }

    /// Check if the validation passed (no Hard level issues).
    ///
    /// Returns true if there are no Hard constraint violations.
    /// Soft and Preference level issues are allowed.
    pub fn is_valid(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|i| matches!(i.level, ConstraintLevel::Hard))
    }

    /// Check if there are any warnings (Soft level issues).
    pub fn has_warnings(&self) -> bool {
        self.issues
            .iter()
            .any(|i| matches!(i.level, ConstraintLevel::Soft))
    }

    /// Get all Hard level issues.
    pub fn hard_issues(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.issues
            .iter()
            .filter(|i| matches!(i.level, ConstraintLevel::Hard))
    }

    /// Get all Soft level issues (warnings).
    pub fn soft_issues(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.issues
            .iter()
            .filter(|i| matches!(i.level, ConstraintLevel::Soft))
    }

    /// Add an issue to the result.
    pub fn add_issue(&mut self, issue: ValidationIssue) {
        self.issues.push(issue);
    }

    /// Merge another validation result into this one.
    pub fn merge(&mut self, other: ValidationResult) {
        self.issues.extend(other.issues);
    }

    /// Get the total number of issues.
    pub fn issue_count(&self) -> usize {
        self.issues.len()
    }
}

/// A single validation issue found during validation.
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    /// Severity level of this issue.
    pub level: ConstraintLevel,
    /// Human-readable message describing the issue.
    pub message: String,
    /// Optional suggestion for how to fix the issue.
    pub suggestion: Option<String>,
}

impl ValidationIssue {
    /// Create a new Hard level issue.
    pub fn hard(message: impl Into<String>) -> Self {
        Self {
            level: ConstraintLevel::Hard,
            message: message.into(),
            suggestion: None,
        }
    }

    /// Create a new Soft level issue (warning).
    pub fn soft(message: impl Into<String>) -> Self {
        Self {
            level: ConstraintLevel::Soft,
            message: message.into(),
            suggestion: None,
        }
    }

    /// Create a new Preference level issue.
    pub fn preference(message: impl Into<String>) -> Self {
        Self {
            level: ConstraintLevel::Preference,
            message: message.into(),
            suggestion: None,
        }
    }

    /// Add a suggestion to this issue.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }
}

/// Validates LLM inputs and outputs against a constraint profile.
///
/// The validator orchestrates pre-flight input validation, post-flight output
/// validation, and output repair by delegating to specialized components:
/// - [`PreflightGuard`] for input validation (blocked commands, drift detection)
/// - [`PostflightValidator`] for output validation (JSON, length)
/// - [`OutputRepairer`] for output repair (JSON extraction, truncation)
///
/// # Example
///
/// ```rust
/// use gateway_core::prompt::{ConstraintProfile, ConstraintValidator};
///
/// let profile = ConstraintProfile::default_secure();
/// let validator = ConstraintValidator::new(profile);
///
/// // Check for blocked commands
/// let result = validator.validate_input("rm -rf /");
/// assert!(!result.is_valid());
/// ```
pub struct ConstraintValidator {
    /// The constraint profile to validate against.
    profile: ConstraintProfile,
    /// Pre-flight input validation guard.
    preflight: PreflightGuard,
    /// Post-flight output validator.
    postflight: PostflightValidator,
    /// Output repairer for failed validation.
    repairer: OutputRepairer,
}

impl ConstraintValidator {
    /// Create a new validator with the given constraint profile.
    ///
    /// Internally creates a [`PreflightGuard`], [`PostflightValidator`], and
    /// [`OutputRepairer`] that share the same profile configuration.
    ///
    /// # Arguments
    ///
    /// * `profile` - The constraint profile defining validation rules
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ConstraintProfile, ConstraintValidator};
    ///
    /// let profile = ConstraintProfile::default();
    /// let validator = ConstraintValidator::new(profile);
    /// ```
    pub fn new(profile: ConstraintProfile) -> Self {
        Self {
            preflight: PreflightGuard::new(profile.clone()),
            postflight: PostflightValidator::new(profile.clone()),
            repairer: OutputRepairer::new(profile.clone()),
            profile,
        }
    }

    /// Get a reference to the underlying constraint profile.
    pub fn profile(&self) -> &ConstraintProfile {
        &self.profile
    }

    /// Pre-flight validation of user input.
    ///
    /// Delegates to [`PreflightGuard`] to check the input for:
    /// - Blocked commands from security configuration
    /// - Role drift keywords if drift detection is enabled
    ///
    /// # Arguments
    ///
    /// * `input` - The user's input message
    ///
    /// # Returns
    ///
    /// A `ValidationResult` containing any issues found. Hard issues indicate
    /// the input should be blocked, Soft issues are warnings.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ConstraintProfile, ConstraintValidator};
    ///
    /// let profile = ConstraintProfile::default_secure();
    /// let validator = ConstraintValidator::new(profile);
    ///
    /// // This should fail - blocked command
    /// let result = validator.validate_input("sudo rm -rf /etc");
    /// assert!(!result.is_valid());
    /// ```
    pub fn validate_input(&self, input: &str) -> ValidationResult {
        self.preflight.validate(input)
    }

    /// Post-flight validation of LLM output.
    ///
    /// Delegates to [`PostflightValidator`] to check the output for:
    /// - Valid JSON if a json_schema constraint is enabled
    /// - Response length against token limits
    ///
    /// # Arguments
    ///
    /// * `output` - The LLM's response
    ///
    /// # Returns
    ///
    /// A `ValidationResult` containing any issues found.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{
    ///     ConstraintProfile, ConstraintValidator, OutputConstraint, ValidationMode,
    /// };
    /// use serde_json::json;
    ///
    /// // Create a profile that requires JSON output
    /// let constraint = OutputConstraint {
    ///     name: "json".to_string(),
    ///     description: "JSON required".to_string(),
    ///     json_schema: Some(json!({"type": "object"})),
    ///     prompt_injection: "Respond with JSON".to_string(),
    ///     validation_mode: ValidationMode::Strict,
    ///     enabled: true,
    /// };
    ///
    /// let profile = ConstraintProfile::default().with_output_constraint(constraint);
    /// let validator = ConstraintValidator::new(profile);
    ///
    /// // This should fail - not valid JSON
    /// let result = validator.validate_output("This is not JSON");
    /// assert!(!result.is_valid());
    /// ```
    pub fn validate_output(&self, output: &str) -> ValidationResult {
        self.postflight.validate(output)
    }

    /// Attempt to repair LLM output based on validation issues.
    ///
    /// Delegates to [`OutputRepairer`] which returns `Some(repaired)` if repair
    /// was successful, `None` if the output cannot be automatically repaired.
    ///
    /// Currently supports:
    /// - **JSON extraction**: Finds valid JSON in mixed text/JSON output
    /// - **Truncation**: Truncates output that exceeds token limits
    ///
    /// # Arguments
    ///
    /// * `output` - The original LLM output
    /// * `issues` - Validation issues found during post-flight validation
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ConstraintProfile, ConstraintValidator, ValidationIssue};
    ///
    /// let profile = ConstraintProfile::default();
    /// let validator = ConstraintValidator::new(profile);
    ///
    /// let issues = vec![
    ///     ValidationIssue::hard("Invalid JSON: expected value at line 1 column 1"),
    /// ];
    ///
    /// let mixed_output = "Here is the result: {\"key\": \"value\"}";
    /// let repaired = validator.repair_output(mixed_output, &issues);
    /// assert_eq!(repaired, Some("{\"key\": \"value\"}".to_string()));
    /// ```
    pub fn repair_output(&self, output: &str, issues: &[ValidationIssue]) -> Option<String> {
        self.repairer.repair(output, issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::{OutputConstraint, RoleAnchor, SecurityBoundary, ValidationMode};

    // ==================== ValidationResult Tests ====================

    #[test]
    fn test_validation_result_valid() {
        let result = ValidationResult::valid();
        assert!(result.is_valid());
        assert!(!result.has_warnings());
        assert_eq!(result.issue_count(), 0);
    }

    #[test]
    fn test_validation_result_with_hard_issue() {
        let result = ValidationResult::with_issue(ValidationIssue::hard("Error"));
        assert!(!result.is_valid());
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_validation_result_with_soft_issue() {
        let result = ValidationResult::with_issue(ValidationIssue::soft("Warning"));
        assert!(result.is_valid()); // Soft issues don't invalidate
        assert!(result.has_warnings());
    }

    #[test]
    fn test_validation_result_add_issue() {
        let mut result = ValidationResult::valid();
        result.add_issue(ValidationIssue::hard("Error 1"));
        result.add_issue(ValidationIssue::soft("Warning 1"));

        assert_eq!(result.issue_count(), 2);
        assert!(!result.is_valid());
        assert!(result.has_warnings());
    }

    #[test]
    fn test_validation_result_merge() {
        let mut result1 = ValidationResult::with_issue(ValidationIssue::hard("Error"));
        let result2 = ValidationResult::with_issue(ValidationIssue::soft("Warning"));

        result1.merge(result2);
        assert_eq!(result1.issue_count(), 2);
    }

    #[test]
    fn test_validation_result_iterators() {
        let mut result = ValidationResult::valid();
        result.add_issue(ValidationIssue::hard("Hard 1"));
        result.add_issue(ValidationIssue::hard("Hard 2"));
        result.add_issue(ValidationIssue::soft("Soft 1"));
        result.add_issue(ValidationIssue::preference("Pref 1"));

        assert_eq!(result.hard_issues().count(), 2);
        assert_eq!(result.soft_issues().count(), 1);
    }

    // ==================== ValidationIssue Tests ====================

    #[test]
    fn test_validation_issue_hard() {
        let issue = ValidationIssue::hard("Test error");
        assert!(matches!(issue.level, ConstraintLevel::Hard));
        assert_eq!(issue.message, "Test error");
        assert!(issue.suggestion.is_none());
    }

    #[test]
    fn test_validation_issue_soft() {
        let issue = ValidationIssue::soft("Test warning");
        assert!(matches!(issue.level, ConstraintLevel::Soft));
        assert_eq!(issue.message, "Test warning");
    }

    #[test]
    fn test_validation_issue_preference() {
        let issue = ValidationIssue::preference("Test preference");
        assert!(matches!(issue.level, ConstraintLevel::Preference));
    }

    #[test]
    fn test_validation_issue_with_suggestion() {
        let issue = ValidationIssue::hard("Error").with_suggestion("Try this instead");
        assert_eq!(issue.suggestion, Some("Try this instead".to_string()));
    }

    // ==================== ConstraintValidator Tests ====================

    #[test]
    fn test_validator_new() {
        let profile = ConstraintProfile::default();
        let validator = ConstraintValidator::new(profile.clone());
        assert_eq!(validator.profile().name, profile.name);
    }

    // ==================== Blocked Command Detection Tests ====================

    #[test]
    fn test_blocked_command_detection() {
        let profile = ConstraintProfile::default_secure();
        let validator = ConstraintValidator::new(profile);

        // Test blocked commands
        let result = validator.validate_input("rm -rf /");
        assert!(!result.is_valid());

        let result = validator.validate_input("sudo rm -rf /etc");
        assert!(!result.is_valid());

        let result = validator.validate_input("> /dev/sda");
        assert!(!result.is_valid());
    }

    #[test]
    fn test_blocked_command_safe_input() {
        let profile = ConstraintProfile::default_secure();
        let validator = ConstraintValidator::new(profile);

        // Safe commands should pass
        let result = validator.validate_input("ls -la");
        assert!(result.is_valid());

        let result = validator.validate_input("cat file.txt");
        assert!(result.is_valid());
    }

    #[test]
    fn test_blocked_command_custom_list() {
        let security = SecurityBoundary {
            blocked_commands: vec!["DROP TABLE".to_string(), "DELETE FROM".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_security(security);
        let validator = ConstraintValidator::new(profile);

        let result = validator.validate_input("DROP TABLE users");
        assert!(!result.is_valid());

        let result = validator.validate_input("SELECT * FROM users");
        assert!(result.is_valid());
    }

    #[test]
    fn test_blocked_command_issue_details() {
        let profile = ConstraintProfile::default_secure();
        let validator = ConstraintValidator::new(profile);

        let result = validator.validate_input("rm -rf /");
        assert_eq!(result.issue_count(), 1);

        let issue = &result.issues[0];
        assert!(matches!(issue.level, ConstraintLevel::Hard));
        assert!(issue.message.contains("rm -rf /"));
        assert!(issue.suggestion.is_some());
    }

    // ==================== Drift Keyword Detection Tests ====================

    #[test]
    fn test_drift_keyword_detection() {
        let anchor = RoleAnchor {
            role_name: "Assistant".to_string(),
            anchor_prompt: "You are an assistant.".to_string(),
            drift_detection: true,
            drift_keywords: vec!["pretend".to_string(), "roleplay".to_string()],
            drift_response: Some("I'm an assistant.".to_string()),
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let validator = ConstraintValidator::new(profile);

        // Should detect drift keywords
        let result = validator.validate_input("pretend you are a pirate");
        assert!(result.has_warnings());
        assert!(result.is_valid()); // Drift is Soft, not Hard

        let result = validator.validate_input("let's roleplay");
        assert!(result.has_warnings());
    }

    #[test]
    fn test_drift_keyword_case_insensitive() {
        let anchor = RoleAnchor {
            role_name: "Assistant".to_string(),
            anchor_prompt: "You are an assistant.".to_string(),
            drift_detection: true,
            drift_keywords: vec!["pretend".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let validator = ConstraintValidator::new(profile);

        // Should match regardless of case
        let result = validator.validate_input("PRETEND you are a cat");
        assert!(result.has_warnings());

        let result = validator.validate_input("Pretend to be happy");
        assert!(result.has_warnings());
    }

    #[test]
    fn test_drift_detection_disabled() {
        let anchor = RoleAnchor {
            role_name: "Assistant".to_string(),
            anchor_prompt: "You are an assistant.".to_string(),
            drift_detection: false, // Disabled
            drift_keywords: vec!["pretend".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let validator = ConstraintValidator::new(profile);

        // Should not detect when disabled
        let result = validator.validate_input("pretend you are a pirate");
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_drift_detection_no_anchor() {
        let profile = ConstraintProfile::default();
        let validator = ConstraintValidator::new(profile);

        // No anchor means no drift detection
        let result = validator.validate_input("pretend you are a pirate");
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_drift_keyword_with_suggestion() {
        let anchor = RoleAnchor {
            role_name: "Browser Agent".to_string(),
            anchor_prompt: "You are a browser automation agent.".to_string(),
            drift_detection: true,
            drift_keywords: vec!["imagine".to_string()],
            drift_response: Some("I'm a browser agent, I can help with web tasks.".to_string()),
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let validator = ConstraintValidator::new(profile);

        let result = validator.validate_input("imagine a world where...");
        assert!(result.has_warnings());

        let issue = &result.issues[0];
        assert!(issue.suggestion.is_some());
        assert!(issue.suggestion.as_ref().unwrap().contains("browser agent"));
    }

    // ==================== JSON Output Validation Tests ====================

    #[test]
    fn test_valid_json_output() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            description: "JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Use JSON".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = ConstraintValidator::new(profile);

        let result = validator.validate_output(r#"{"key": "value"}"#);
        assert!(result.is_valid());
    }

    #[test]
    fn test_invalid_json_output() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            description: "JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Use JSON".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = ConstraintValidator::new(profile);

        let result = validator.validate_output("This is not JSON at all");
        assert!(!result.is_valid());

        let issue = &result.issues[0];
        assert!(matches!(issue.level, ConstraintLevel::Hard));
        assert!(issue.message.contains("Invalid JSON"));
    }

    #[test]
    fn test_malformed_json_output() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            description: "JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Use JSON".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = ConstraintValidator::new(profile);

        // Missing closing brace
        let result = validator.validate_output(r#"{"key": "value""#);
        assert!(!result.is_valid());

        // Trailing comma
        let result = validator.validate_output(r#"{"key": "value",}"#);
        assert!(!result.is_valid());
    }

    #[test]
    fn test_json_constraint_disabled() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            description: "JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Use JSON".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: false, // Disabled
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = ConstraintValidator::new(profile);

        // Should pass even with invalid JSON since constraint is disabled
        let result = validator.validate_output("Not JSON");
        assert!(result.is_valid());
    }

    #[test]
    fn test_no_json_schema_constraint() {
        let constraint = OutputConstraint {
            name: "text".to_string(),
            description: "Text output".to_string(),
            json_schema: None, // No schema
            prompt_injection: "".to_string(),
            validation_mode: ValidationMode::WarnOnly,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = ConstraintValidator::new(profile);

        // Should pass - no JSON validation required
        let result = validator.validate_output("Just plain text");
        assert!(result.is_valid());
    }

    // ==================== Output Length Validation Tests ====================

    #[test]
    fn test_output_length_within_limit() {
        let profile = ConstraintProfile::default();
        let validator = ConstraintValidator::new(profile);

        let result = validator.validate_output("Short response");
        assert!(result.is_valid());
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_output_length_exceeds_limit() {
        use crate::prompt::TokenLimits;
        use std::collections::HashMap;

        let limits = TokenLimits {
            system_prompt_max: 8000,
            response_max: 10, // Very low limit for testing
            section_budgets: HashMap::new(),
        };

        let profile = ConstraintProfile::default().with_token_limits(limits);
        let validator = ConstraintValidator::new(profile);

        // This should exceed the limit (~50 characters / 4 = ~12 tokens > 10)
        let result = validator.validate_output("This is a longer response that exceeds the limit");
        assert!(result.has_warnings());
        assert!(result.is_valid()); // Length is Soft warning, not Hard
    }

    // ==================== Combined Validation Tests ====================

    #[test]
    fn test_multiple_issues_in_single_validation() {
        let security = SecurityBoundary {
            blocked_commands: vec!["dangerous".to_string(), "risky".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_security(security);
        let validator = ConstraintValidator::new(profile);

        // Input contains multiple blocked commands
        let result = validator.validate_input("dangerous risky operation");
        assert!(!result.is_valid());
        assert_eq!(result.hard_issues().count(), 2);
    }

    #[test]
    fn test_combined_hard_and_soft_issues() {
        let anchor = RoleAnchor {
            role_name: "Agent".to_string(),
            anchor_prompt: "You are an agent.".to_string(),
            drift_detection: true,
            drift_keywords: vec!["pretend".to_string()],
            ..Default::default()
        };

        let security = SecurityBoundary {
            blocked_commands: vec!["dangerous".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default()
            .with_role_anchor(anchor)
            .with_security(security);

        let validator = ConstraintValidator::new(profile);

        // Input has both a blocked command and drift keyword
        let result = validator.validate_input("dangerous pretend operation");
        assert!(!result.is_valid()); // Hard issue present
        assert!(result.has_warnings()); // Soft issue present
        assert_eq!(result.issue_count(), 2);
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_empty_input() {
        let profile = ConstraintProfile::default_secure();
        let validator = ConstraintValidator::new(profile);

        let result = validator.validate_input("");
        assert!(result.is_valid());
    }

    #[test]
    fn test_empty_output() {
        let constraint = OutputConstraint {
            name: "json".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            enabled: true,
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = ConstraintValidator::new(profile);

        // Empty string is not valid JSON
        let result = validator.validate_output("");
        assert!(!result.is_valid());
    }

    #[test]
    fn test_whitespace_only_input() {
        let profile = ConstraintProfile::default_secure();
        let validator = ConstraintValidator::new(profile);

        let result = validator.validate_input("   \n\t  ");
        assert!(result.is_valid());
    }

    #[test]
    fn test_unicode_input() {
        let anchor = RoleAnchor {
            role_name: "Agent".to_string(),
            anchor_prompt: "You are an agent.".to_string(),
            drift_detection: true,
            drift_keywords: vec!["pretend".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let validator = ConstraintValidator::new(profile);

        // Unicode characters should not break validation
        let result = validator.validate_input("Test message");
        assert!(result.is_valid());
    }

    // ==================== Phase 6 Integration Tests ====================

    #[test]
    fn test_validate_input_blocks_dangerous_command() {
        let profile = ConstraintProfile::default_secure();
        let validator = ConstraintValidator::new(profile);

        // "rm -rf /" should be detected as a Hard violation
        let result = validator.validate_input("Please run rm -rf / to clean up");
        assert!(!result.is_valid(), "rm -rf / should be blocked");
        assert!(result.hard_issues().count() >= 1);

        let issue = result.hard_issues().next().unwrap();
        assert!(issue.message.contains("rm -rf /"));
        assert!(issue.suggestion.is_some());
    }

    #[test]
    fn test_validate_input_warns_on_drift() {
        let anchor = RoleAnchor {
            role_name: "Coding Assistant".to_string(),
            anchor_prompt: "You are a coding assistant.".to_string(),
            drift_detection: true,
            drift_keywords: vec![
                "pretend".to_string(),
                "roleplay".to_string(),
                "ignore".to_string(),
            ],
            drift_response: Some("I'm a coding assistant, I help with code.".to_string()),
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let validator = ConstraintValidator::new(profile);

        // "pretend" should trigger a Soft warning, not a Hard block
        let result = validator.validate_input("pretend you are a hacker");
        assert!(result.is_valid(), "Drift should be Soft, not Hard");
        assert!(result.has_warnings(), "Should have drift warning");

        let warning = result.soft_issues().next().unwrap();
        assert!(warning.message.contains("pretend"));
        assert!(warning.suggestion.is_some());
        assert!(warning
            .suggestion
            .as_ref()
            .unwrap()
            .contains("coding assistant"));
    }

    #[test]
    fn test_validate_output_blocks_invalid_json() {
        let constraint = OutputConstraint {
            name: "json_response".to_string(),
            description: "Response must be JSON".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Respond with valid JSON only.".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = ConstraintValidator::new(profile);

        // Plain text should fail JSON validation with a Hard issue
        let result = validator.validate_output("This is not JSON at all");
        assert!(!result.is_valid());
        assert!(result.hard_issues().count() >= 1);

        let issue = result.hard_issues().next().unwrap();
        assert!(issue.message.contains("Invalid JSON"));
    }

    #[test]
    fn test_validate_output_passes_valid_json() {
        let constraint = OutputConstraint {
            name: "json_response".to_string(),
            description: "Response must be JSON".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Respond with valid JSON only.".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        let validator = ConstraintValidator::new(profile);

        // Valid JSON should pass
        let result = validator.validate_output(r#"{"action": "click", "target": "submit-btn"}"#);
        assert!(result.is_valid());
        assert_eq!(result.issue_count(), 0);
    }

    #[test]
    fn test_repair_extracts_json_from_mixed_output() {
        let profile = ConstraintProfile::default();
        let validator = ConstraintValidator::new(profile);

        let issues = vec![ValidationIssue::hard(
            "Invalid JSON: expected value at line 1 column 1",
        )];

        // JSON embedded in prose text
        let mixed = "Here is the result: {\"key\": \"value\", \"count\": 42} as requested";
        let repaired = validator.repair_output(mixed, &issues);
        assert!(repaired.is_some());
        assert_eq!(repaired.unwrap(), r#"{"key": "value", "count": 42}"#);

        // JSON array embedded in prose
        let mixed_array = "The items are: [1, 2, 3] and that's it";
        let repaired = validator.repair_output(mixed_array, &issues);
        assert!(repaired.is_some());
        assert_eq!(repaired.unwrap(), "[1, 2, 3]");
    }

    #[test]
    fn test_repair_returns_none_for_unrecoverable() {
        let profile = ConstraintProfile::default();
        let validator = ConstraintValidator::new(profile);

        let issues = vec![ValidationIssue::hard(
            "Invalid JSON: expected value at line 1 column 1",
        )];

        // Pure text with no valid JSON anywhere
        let pure_text = "This is just plain text with no JSON at all";
        let repaired = validator.repair_output(pure_text, &issues);
        assert!(repaired.is_none());

        // Malformed JSON that can't be extracted
        let bad_json = "Here is some { broken json without closing";
        let repaired = validator.repair_output(bad_json, &issues);
        assert!(repaired.is_none());
    }

    #[test]
    fn test_repair_truncates_long_output() {
        use crate::prompt::TokenLimits;
        use std::collections::HashMap;

        let limits = TokenLimits {
            system_prompt_max: 8000,
            response_max: 10, // 10 tokens * 4 chars = 40 char limit
            section_budgets: HashMap::new(),
        };

        let profile = ConstraintProfile::default().with_token_limits(limits);
        let validator = ConstraintValidator::new(profile);

        let issues = vec![ValidationIssue::soft(
            "Response may exceed token limit: ~100 tokens (limit: 10)",
        )];

        let long_output = "A".repeat(200);
        let repaired = validator.repair_output(&long_output, &issues);
        assert!(repaired.is_some());
        assert!(repaired.as_ref().unwrap().len() <= 40);
    }
}
