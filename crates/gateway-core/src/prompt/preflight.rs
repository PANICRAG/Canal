//! Pre-flight validation of user input.
//!
//! Validates input before sending to the LLM:
//! - Security checks for blocked commands
//! - Role drift detection for suspicious keywords
//!
//! # Example
//!
//! ```rust
//! use gateway_core::prompt::{ConstraintProfile, PreflightGuard, RoleAnchor};
//!
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
//! let guard = PreflightGuard::new(profile);
//!
//! let result = guard.validate("pretend you are a pirate");
//! assert!(result.has_warnings());
//! ```

use super::constraints::ConstraintProfile;
use super::validator::{ValidationIssue, ValidationResult};

/// Pre-flight guard that validates user input before sending to the LLM.
///
/// Performs two categories of checks:
/// - **Security checks**: Detects blocked commands from the security configuration
/// - **Drift detection**: Identifies suspicious keywords that may indicate role drift
///
/// # Example
///
/// ```rust
/// use gateway_core::prompt::{ConstraintProfile, PreflightGuard};
///
/// let profile = ConstraintProfile::default_secure();
/// let guard = PreflightGuard::new(profile);
///
/// let result = guard.validate("rm -rf /");
/// assert!(!result.is_valid());
/// ```
pub struct PreflightGuard {
    /// The constraint profile to validate against.
    profile: ConstraintProfile,
}

impl PreflightGuard {
    /// Create a new pre-flight guard with the given constraint profile.
    ///
    /// # Arguments
    ///
    /// * `profile` - The constraint profile defining validation rules
    pub fn new(profile: ConstraintProfile) -> Self {
        Self { profile }
    }

    /// Validate user input against pre-flight constraints.
    ///
    /// Checks for blocked commands and role drift keywords.
    ///
    /// # Arguments
    ///
    /// * `input` - The user's input message
    ///
    /// # Returns
    ///
    /// A `ValidationResult` containing any issues found. Hard issues indicate
    /// the input should be blocked, Soft issues are warnings.
    pub fn validate(&self, input: &str) -> ValidationResult {
        let mut result = ValidationResult::valid();

        self.check_blocked_commands(input, &mut result);
        self.check_drift_keywords(input, &mut result);

        result
    }

    /// Normalize input for security matching.
    ///
    /// Collapses whitespace, strips control characters, and lowercases
    /// to prevent bypass via case variations, extra whitespace, or
    /// embedded control characters.
    fn normalize_for_security(input: &str) -> String {
        input
            .chars()
            .filter(|c| {
                // Strip control characters (except whitespace)
                if c.is_control() && *c != ' ' && *c != '\n' && *c != '\t' {
                    return false;
                }
                // R2-H2: Strip Unicode zero-width and invisible characters
                // that can be used to bypass blocked command detection
                !matches!(*c,
                    '\u{200B}' | // Zero-width space
                    '\u{200C}' | // Zero-width non-joiner
                    '\u{200D}' | // Zero-width joiner
                    '\u{FEFF}' | // Zero-width no-break space (BOM)
                    '\u{00AD}' | // Soft hyphen
                    '\u{034F}' | // Combining grapheme joiner
                    '\u{061C}' | // Arabic letter mark
                    '\u{2060}' | // Word joiner
                    '\u{2061}'..='\u{2064}' | // Invisible operators
                    '\u{206A}'..='\u{206F}' | // Deprecated formatting chars
                    '\u{FE00}'..='\u{FE0F}' | // Variation selectors
                    '\u{202A}'..='\u{202E}' | // Bidi overrides
                    '\u{2066}'..='\u{2069}'   // Bidi isolates
                )
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    /// Check input for blocked commands.
    fn check_blocked_commands(&self, input: &str, result: &mut ValidationResult) {
        let normalized = Self::normalize_for_security(input);
        for cmd in &self.profile.security.blocked_commands {
            let normalized_cmd = cmd.to_lowercase();
            if normalized.contains(&normalized_cmd) {
                result.add_issue(
                    ValidationIssue::hard(format!("Blocked command detected: {}", cmd))
                        .with_suggestion("This command is not allowed for security reasons."),
                );
            }
        }
    }

    /// Check input for role drift keywords.
    fn check_drift_keywords(&self, input: &str, result: &mut ValidationResult) {
        if let Some(ref anchor) = self.profile.role_anchor {
            if anchor.drift_detection {
                let input_lower = input.to_lowercase();
                for keyword in &anchor.drift_keywords {
                    if input_lower.contains(&keyword.to_lowercase()) {
                        let mut issue = ValidationIssue::soft(format!(
                            "Potential role drift detected: '{}'",
                            keyword
                        ));

                        if let Some(ref response) = anchor.drift_response {
                            issue = issue.with_suggestion(response.clone());
                        }

                        result.add_issue(issue);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::{ConstraintLevel, RoleAnchor, SecurityBoundary};

    // ==================== PreflightGuard Blocked Command Tests ====================

    #[test]
    fn test_preflight_blocked_command_detection() {
        let profile = ConstraintProfile::default_secure();
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("rm -rf /");
        assert!(!result.is_valid());

        let result = guard.validate("sudo rm -rf /etc");
        assert!(!result.is_valid());

        let result = guard.validate("> /dev/sda");
        assert!(!result.is_valid());
    }

    #[test]
    fn test_preflight_safe_input() {
        let profile = ConstraintProfile::default_secure();
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("ls -la");
        assert!(result.is_valid());

        let result = guard.validate("cat file.txt");
        assert!(result.is_valid());
    }

    #[test]
    fn test_preflight_custom_blocked_commands() {
        let security = SecurityBoundary {
            blocked_commands: vec!["DROP TABLE".to_string(), "DELETE FROM".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_security(security);
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("DROP TABLE users");
        assert!(!result.is_valid());

        let result = guard.validate("SELECT * FROM users");
        assert!(result.is_valid());
    }

    #[test]
    fn test_preflight_blocked_command_issue_details() {
        let profile = ConstraintProfile::default_secure();
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("rm -rf /");
        assert_eq!(result.issue_count(), 1);

        let issue = &result.issues[0];
        assert!(matches!(issue.level, ConstraintLevel::Hard));
        assert!(issue.message.contains("rm -rf /"));
        assert!(issue.suggestion.is_some());
    }

    #[test]
    fn test_preflight_multiple_blocked_commands() {
        let security = SecurityBoundary {
            blocked_commands: vec!["dangerous".to_string(), "risky".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_security(security);
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("dangerous risky operation");
        assert!(!result.is_valid());
        assert_eq!(result.hard_issues().count(), 2);
    }

    #[test]
    fn test_preflight_empty_input() {
        let profile = ConstraintProfile::default_secure();
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("");
        assert!(result.is_valid());
    }

    #[test]
    fn test_preflight_whitespace_only_input() {
        let profile = ConstraintProfile::default_secure();
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("   \n\t  ");
        assert!(result.is_valid());
    }

    // ==================== PreflightGuard Drift Detection Tests ====================

    #[test]
    fn test_preflight_drift_keyword_detection() {
        let anchor = RoleAnchor {
            role_name: "Assistant".to_string(),
            anchor_prompt: "You are an assistant.".to_string(),
            drift_detection: true,
            drift_keywords: vec!["pretend".to_string(), "roleplay".to_string()],
            drift_response: Some("I'm an assistant.".to_string()),
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("pretend you are a pirate");
        assert!(result.has_warnings());
        assert!(result.is_valid());

        let result = guard.validate("let's roleplay");
        assert!(result.has_warnings());
    }

    #[test]
    fn test_preflight_drift_keyword_case_insensitive() {
        let anchor = RoleAnchor {
            role_name: "Assistant".to_string(),
            anchor_prompt: "You are an assistant.".to_string(),
            drift_detection: true,
            drift_keywords: vec!["pretend".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("PRETEND you are a cat");
        assert!(result.has_warnings());

        let result = guard.validate("Pretend to be happy");
        assert!(result.has_warnings());
    }

    #[test]
    fn test_preflight_drift_detection_disabled() {
        let anchor = RoleAnchor {
            role_name: "Assistant".to_string(),
            anchor_prompt: "You are an assistant.".to_string(),
            drift_detection: false,
            drift_keywords: vec!["pretend".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("pretend you are a pirate");
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_preflight_drift_detection_no_anchor() {
        let profile = ConstraintProfile::default();
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("pretend you are a pirate");
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_preflight_drift_keyword_with_suggestion() {
        let anchor = RoleAnchor {
            role_name: "Browser Agent".to_string(),
            anchor_prompt: "You are a browser automation agent.".to_string(),
            drift_detection: true,
            drift_keywords: vec!["imagine".to_string()],
            drift_response: Some("I'm a browser agent, I can help with web tasks.".to_string()),
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor);
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("imagine a world where...");
        assert!(result.has_warnings());

        let issue = &result.issues[0];
        assert!(issue.suggestion.is_some());
        assert!(issue.suggestion.as_ref().unwrap().contains("browser agent"));
    }

    #[test]
    fn test_preflight_combined_hard_and_soft() {
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
        let guard = PreflightGuard::new(profile);

        let result = guard.validate("dangerous pretend operation");
        assert!(!result.is_valid());
        assert!(result.has_warnings());
        assert_eq!(result.issue_count(), 2);
    }

    // ==================== Normalize Tests ====================

    #[test]
    fn test_normalize_for_security() {
        let normalized = PreflightGuard::normalize_for_security("  RM  -RF  / ");
        assert_eq!(normalized, "rm -rf /");

        let normalized = PreflightGuard::normalize_for_security("Hello\x00World");
        assert_eq!(normalized, "helloworld");

        let normalized = PreflightGuard::normalize_for_security("  multiple   spaces  ");
        assert_eq!(normalized, "multiple spaces");
    }

    #[test]
    fn test_normalize_preserves_allowed_control_chars() {
        let normalized = PreflightGuard::normalize_for_security("line1\nline2\ttab");
        assert_eq!(normalized, "line1 line2 tab");
    }
}
