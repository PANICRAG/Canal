//! Core constraint types for the Prompt Constraint System.
//!
//! This module defines the fundamental data structures for constraining
//! LLM behavior, including output format validation, role anchoring,
//! security boundaries, and token limits.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Constraint enforcement level.
///
/// Determines how strictly a constraint should be enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintLevel {
    /// Hard constraint: violations are blocked and cause errors.
    Hard,
    /// Soft constraint: violations trigger warnings but allow continuation.
    Soft,
    /// Preference: used as suggestions only, no enforcement.
    Preference,
}

impl Default for ConstraintLevel {
    fn default() -> Self {
        Self::Soft
    }
}

/// Validation mode for output constraints.
///
/// Controls how validation failures are handled.
///
/// # YAML Representation
///
/// In YAML, use simple strings for Strict and WarnOnly, or a tagged map for RepairAndRetry:
/// ```yaml
/// validation_mode: Strict
/// validation_mode: WarnOnly
/// validation_mode:
///   RepairAndRetry:
///     max_attempts: 3
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ValidationMode {
    /// Strict validation, reject on failure.
    Strict,
    /// Attempt to repair and retry up to max_attempts times.
    RepairAndRetry {
        /// Maximum number of repair attempts before giving up.
        max_attempts: u32,
    },
    /// Log warning only, allow the output through.
    WarnOnly,
}

impl Default for ValidationMode {
    fn default() -> Self {
        Self::WarnOnly
    }
}

/// Output format constraint.
///
/// Defines rules for validating and enforcing LLM output format.
///
/// # Example
///
/// ```rust
/// use gateway_core::prompt::{OutputConstraint, ValidationMode};
///
/// let constraint = OutputConstraint {
///     name: "json_only".to_string(),
///     description: "Output must be valid JSON".to_string(),
///     json_schema: None,
///     prompt_injection: "Respond with valid JSON only.".to_string(),
///     validation_mode: ValidationMode::Strict,
///     enabled: true,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputConstraint {
    /// Constraint identifier.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON schema for validation (optional).
    pub json_schema: Option<serde_json::Value>,
    /// Prompt text to inject for this constraint.
    pub prompt_injection: String,
    /// How to handle validation failures.
    pub validation_mode: ValidationMode,
    /// Whether this constraint is enabled.
    pub enabled: bool,
}

impl Default for OutputConstraint {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            json_schema: None,
            prompt_injection: String::new(),
            validation_mode: ValidationMode::default(),
            enabled: true,
        }
    }
}

/// Role anchoring configuration.
///
/// Prevents role drift by periodically re-injecting the role definition
/// and detecting when the LLM strays from its assigned role.
///
/// # Example
///
/// ```rust
/// use gateway_core::prompt::RoleAnchor;
///
/// let anchor = RoleAnchor {
///     role_name: "Browser Automation Agent".to_string(),
///     anchor_prompt: "You are a browser automation agent.".to_string(),
///     reanchor_interval: Some(5),
///     drift_detection: true,
///     drift_keywords: vec!["pretend".to_string(), "roleplay".to_string()],
///     drift_response: Some("I'm a browser automation agent.".to_string()),
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoleAnchor {
    /// Role identifier.
    pub role_name: String,
    /// The anchor prompt that defines the role.
    pub anchor_prompt: String,
    /// Re-inject anchor every N turns (None = only at start).
    pub reanchor_interval: Option<u32>,
    /// Enable drift detection.
    pub drift_detection: bool,
    /// Keywords that suggest role drift.
    pub drift_keywords: Vec<String>,
    /// Response when drift is detected.
    pub drift_response: Option<String>,
}

impl Default for RoleAnchor {
    fn default() -> Self {
        Self {
            role_name: String::new(),
            anchor_prompt: String::new(),
            reanchor_interval: None,
            drift_detection: false,
            drift_keywords: Vec::new(),
            drift_response: None,
        }
    }
}

/// Security boundary configuration.
///
/// Defines what file paths and commands are allowed or blocked.
///
/// # Example
///
/// ```rust
/// use gateway_core::prompt::SecurityBoundary;
///
/// let security = SecurityBoundary {
///     allowed_paths: vec!["${WORKSPACE}/**".to_string()],
///     blocked_patterns: vec!["**/.env*".to_string()],
///     blocked_commands: vec!["rm -rf /".to_string()],
///     require_confirmation: vec!["git push --force".to_string()],
///     prompt_injection: Some("Do not access sensitive files.".to_string()),
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityBoundary {
    /// Allowed file path patterns (glob).
    pub allowed_paths: Vec<String>,
    /// Blocked file path patterns (glob).
    pub blocked_patterns: Vec<String>,
    /// Blocked shell commands.
    pub blocked_commands: Vec<String>,
    /// Commands that require user confirmation.
    pub require_confirmation: Vec<String>,
    /// Prompt injection for security rules.
    pub prompt_injection: Option<String>,
}

impl Default for SecurityBoundary {
    fn default() -> Self {
        Self {
            allowed_paths: Vec::new(),
            blocked_patterns: vec![
                "**/.env*".to_string(),
                "**/secrets/**".to_string(),
                "**/*.pem".to_string(),
                "**/*.key".to_string(),
            ],
            blocked_commands: vec![
                "rm -rf /".to_string(),
                "sudo rm".to_string(),
                "> /dev/sd".to_string(),
                "mkfs".to_string(),
            ],
            require_confirmation: vec![
                "git push --force".to_string(),
                "git reset --hard".to_string(),
            ],
            prompt_injection: None,
        }
    }
}

/// Token limits configuration.
///
/// Controls the token budget for system prompts and responses.
///
/// # Example
///
/// ```rust
/// use gateway_core::prompt::TokenLimits;
/// use std::collections::HashMap;
///
/// let mut section_budgets = HashMap::new();
/// section_budgets.insert("role_anchor".to_string(), 800);
/// section_budgets.insert("examples".to_string(), 1000);
///
/// let limits = TokenLimits {
///     system_prompt_max: 8000,
///     response_max: 4000,
///     section_budgets,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenLimits {
    /// Maximum tokens for system prompt.
    pub system_prompt_max: usize,
    /// Maximum tokens for LLM response.
    pub response_max: usize,
    /// Per-section token budgets.
    pub section_budgets: HashMap<String, usize>,
}

impl Default for TokenLimits {
    fn default() -> Self {
        let mut section_budgets = HashMap::new();
        section_budgets.insert("role_anchor".to_string(), 800);
        section_budgets.insert("tool_guidance".to_string(), 600);
        section_budgets.insert("examples".to_string(), 1000);
        section_budgets.insert("security_rules".to_string(), 200);

        Self {
            system_prompt_max: 8000,
            response_max: 4000,
            section_budgets,
        }
    }
}

/// Reasoning mode for the LLM.
///
/// Controls how the LLM should structure its thinking process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasoningMode {
    /// Direct response without explicit reasoning.
    Direct,
    /// Chain-of-thought: show reasoning steps.
    ChainOfThought,
    /// ReAct: Reasoning + Acting pattern.
    ReAct,
    /// Plan then execute.
    PlanExecute,
}

impl Default for ReasoningMode {
    fn default() -> Self {
        Self::Direct
    }
}

/// Complete constraint profile.
///
/// A profile combines all constraint types into a single configuration
/// that can be applied to an agent session.
///
/// # Example
///
/// ```rust
/// use gateway_core::prompt::{ConstraintProfile, ReasoningMode};
///
/// let profile = ConstraintProfile::default();
/// assert_eq!(profile.name, "default");
/// assert_eq!(profile.reasoning_mode, ReasoningMode::Direct);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstraintProfile {
    /// Profile identifier.
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Output format constraints.
    #[serde(default)]
    pub output_constraints: Vec<OutputConstraint>,
    /// Role anchoring configuration.
    #[serde(default)]
    pub role_anchor: Option<RoleAnchor>,
    /// Security boundaries.
    #[serde(default)]
    pub security: SecurityBoundary,
    /// Token limits.
    #[serde(default)]
    pub token_limits: TokenLimits,
    /// Reasoning mode.
    #[serde(default)]
    pub reasoning_mode: ReasoningMode,
}

impl Default for ConstraintProfile {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            description: "Default constraint profile with sensible security defaults".to_string(),
            output_constraints: Vec::new(),
            role_anchor: None,
            security: SecurityBoundary::default(),
            token_limits: TokenLimits::default(),
            reasoning_mode: ReasoningMode::default(),
        }
    }
}

impl ConstraintProfile {
    /// Create a new constraint profile with the given name.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::ConstraintProfile;
    ///
    /// let profile = ConstraintProfile::new("browser_automation", "Profile for browser tasks");
    /// assert_eq!(profile.name, "browser_automation");
    /// ```
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            ..Default::default()
        }
    }

    /// Create a secure default profile with common security constraints.
    ///
    /// This profile blocks dangerous commands and sensitive file patterns.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::ConstraintProfile;
    ///
    /// let profile = ConstraintProfile::default_secure();
    /// assert!(!profile.security.blocked_commands.is_empty());
    /// ```
    pub fn default_secure() -> Self {
        Self {
            name: "secure".to_string(),
            description: "Secure default profile with strict security boundaries".to_string(),
            output_constraints: Vec::new(),
            role_anchor: None,
            security: SecurityBoundary::default(),
            token_limits: TokenLimits::default(),
            reasoning_mode: ReasoningMode::Direct,
        }
    }

    /// Set the role anchor for this profile.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ConstraintProfile, RoleAnchor};
    ///
    /// let anchor = RoleAnchor {
    ///     role_name: "Assistant".to_string(),
    ///     anchor_prompt: "You are a helpful assistant.".to_string(),
    ///     ..Default::default()
    /// };
    ///
    /// let profile = ConstraintProfile::default().with_role_anchor(anchor);
    /// assert!(profile.role_anchor.is_some());
    /// ```
    pub fn with_role_anchor(mut self, anchor: RoleAnchor) -> Self {
        self.role_anchor = Some(anchor);
        self
    }

    /// Add an output constraint to this profile.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ConstraintProfile, OutputConstraint};
    ///
    /// let constraint = OutputConstraint {
    ///     name: "json".to_string(),
    ///     description: "JSON output".to_string(),
    ///     ..Default::default()
    /// };
    ///
    /// let profile = ConstraintProfile::default().with_output_constraint(constraint);
    /// assert_eq!(profile.output_constraints.len(), 1);
    /// ```
    pub fn with_output_constraint(mut self, constraint: OutputConstraint) -> Self {
        self.output_constraints.push(constraint);
        self
    }

    /// Set the reasoning mode for this profile.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ConstraintProfile, ReasoningMode};
    ///
    /// let profile = ConstraintProfile::default()
    ///     .with_reasoning_mode(ReasoningMode::ChainOfThought);
    /// assert_eq!(profile.reasoning_mode, ReasoningMode::ChainOfThought);
    /// ```
    pub fn with_reasoning_mode(mut self, mode: ReasoningMode) -> Self {
        self.reasoning_mode = mode;
        self
    }

    /// Set the token limits for this profile.
    pub fn with_token_limits(mut self, limits: TokenLimits) -> Self {
        self.token_limits = limits;
        self
    }

    /// Set the security boundary for this profile.
    pub fn with_security(mut self, security: SecurityBoundary) -> Self {
        self.security = security;
        self
    }

    /// Check if this profile has role anchoring enabled.
    pub fn has_role_anchor(&self) -> bool {
        self.role_anchor.is_some()
    }

    /// Check if this profile has any output constraints enabled.
    pub fn has_output_constraints(&self) -> bool {
        self.output_constraints.iter().any(|c| c.enabled)
    }

    /// Get all enabled output constraints.
    pub fn enabled_output_constraints(&self) -> impl Iterator<Item = &OutputConstraint> {
        self.output_constraints.iter().filter(|c| c.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constraint_level_default() {
        let level = ConstraintLevel::default();
        assert_eq!(level, ConstraintLevel::Soft);
    }

    #[test]
    fn test_validation_mode_default() {
        let mode = ValidationMode::default();
        assert_eq!(mode, ValidationMode::WarnOnly);
    }

    #[test]
    fn test_validation_mode_serialization() {
        let mode = ValidationMode::RepairAndRetry { max_attempts: 3 };
        let json = serde_json::to_string(&mode).unwrap();
        assert!(json.contains("RepairAndRetry"));
        assert!(json.contains("max_attempts"));

        let deserialized: ValidationMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, mode);
    }

    #[test]
    fn test_output_constraint_default() {
        let constraint = OutputConstraint::default();
        assert!(constraint.name.is_empty());
        assert!(constraint.enabled);
        assert_eq!(constraint.validation_mode, ValidationMode::WarnOnly);
    }

    #[test]
    fn test_output_constraint_serialization() {
        let constraint = OutputConstraint {
            name: "json_format".to_string(),
            description: "Enforce JSON output".to_string(),
            json_schema: Some(serde_json::json!({"type": "object"})),
            prompt_injection: "Respond with JSON only.".to_string(),
            validation_mode: ValidationMode::Strict,
            enabled: true,
        };

        let json = serde_json::to_string(&constraint).unwrap();
        let deserialized: OutputConstraint = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, constraint);
    }

    #[test]
    fn test_role_anchor_default() {
        let anchor = RoleAnchor::default();
        assert!(anchor.role_name.is_empty());
        assert!(!anchor.drift_detection);
        assert!(anchor.drift_keywords.is_empty());
    }

    #[test]
    fn test_role_anchor_serialization() {
        let anchor = RoleAnchor {
            role_name: "Browser Agent".to_string(),
            anchor_prompt: "You are a browser automation agent.".to_string(),
            reanchor_interval: Some(5),
            drift_detection: true,
            drift_keywords: vec!["pretend".to_string(), "roleplay".to_string()],
            drift_response: Some("I'm a browser agent.".to_string()),
        };

        let json = serde_json::to_string(&anchor).unwrap();
        let deserialized: RoleAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, anchor);
    }

    #[test]
    fn test_security_boundary_default() {
        let security = SecurityBoundary::default();
        assert!(security.allowed_paths.is_empty());
        assert!(!security.blocked_patterns.is_empty());
        assert!(!security.blocked_commands.is_empty());
        assert!(!security.require_confirmation.is_empty());
    }

    #[test]
    fn test_security_boundary_serialization() {
        let security = SecurityBoundary {
            allowed_paths: vec!["${WORKSPACE}/**".to_string()],
            blocked_patterns: vec!["**/.env*".to_string()],
            blocked_commands: vec!["rm -rf /".to_string()],
            require_confirmation: vec!["git push --force".to_string()],
            prompt_injection: Some("Security rules apply.".to_string()),
        };

        let yaml = serde_yaml::to_string(&security).unwrap();
        let deserialized: SecurityBoundary = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized, security);
    }

    #[test]
    fn test_token_limits_default() {
        let limits = TokenLimits::default();
        assert_eq!(limits.system_prompt_max, 8000);
        assert_eq!(limits.response_max, 4000);
        assert!(limits.section_budgets.contains_key("role_anchor"));
        assert!(limits.section_budgets.contains_key("examples"));
    }

    #[test]
    fn test_token_limits_serialization() {
        let mut section_budgets = HashMap::new();
        section_budgets.insert("custom".to_string(), 500);

        let limits = TokenLimits {
            system_prompt_max: 10000,
            response_max: 5000,
            section_budgets,
        };

        let json = serde_json::to_string(&limits).unwrap();
        let deserialized: TokenLimits = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, limits);
    }

    #[test]
    fn test_reasoning_mode_default() {
        let mode = ReasoningMode::default();
        assert_eq!(mode, ReasoningMode::Direct);
    }

    #[test]
    fn test_reasoning_mode_serialization() {
        let modes = [
            ReasoningMode::Direct,
            ReasoningMode::ChainOfThought,
            ReasoningMode::ReAct,
            ReasoningMode::PlanExecute,
        ];

        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: ReasoningMode = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, mode);
        }
    }

    #[test]
    fn test_constraint_profile_default() {
        let profile = ConstraintProfile::default();
        assert_eq!(profile.name, "default");
        assert!(!profile.description.is_empty());
        assert!(profile.output_constraints.is_empty());
        assert!(profile.role_anchor.is_none());
        assert_eq!(profile.reasoning_mode, ReasoningMode::Direct);
    }

    #[test]
    fn test_constraint_profile_new() {
        let profile = ConstraintProfile::new("custom", "A custom profile");
        assert_eq!(profile.name, "custom");
        assert_eq!(profile.description, "A custom profile");
    }

    #[test]
    fn test_constraint_profile_default_secure() {
        let profile = ConstraintProfile::default_secure();
        assert_eq!(profile.name, "secure");
        assert!(!profile.security.blocked_commands.is_empty());
        assert!(!profile.security.blocked_patterns.is_empty());
    }

    #[test]
    fn test_constraint_profile_with_role_anchor() {
        let anchor = RoleAnchor {
            role_name: "Test".to_string(),
            anchor_prompt: "You are a test agent.".to_string(),
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_role_anchor(anchor.clone());
        assert!(profile.has_role_anchor());
        assert_eq!(profile.role_anchor.unwrap().role_name, "Test");
    }

    #[test]
    fn test_constraint_profile_with_output_constraint() {
        let constraint = OutputConstraint {
            name: "test".to_string(),
            enabled: true,
            ..Default::default()
        };

        let profile = ConstraintProfile::default().with_output_constraint(constraint);
        assert!(profile.has_output_constraints());
        assert_eq!(profile.output_constraints.len(), 1);
    }

    #[test]
    fn test_constraint_profile_with_reasoning_mode() {
        let profile =
            ConstraintProfile::default().with_reasoning_mode(ReasoningMode::ChainOfThought);
        assert_eq!(profile.reasoning_mode, ReasoningMode::ChainOfThought);
    }

    #[test]
    fn test_constraint_profile_enabled_output_constraints() {
        let enabled = OutputConstraint {
            name: "enabled".to_string(),
            enabled: true,
            ..Default::default()
        };
        let disabled = OutputConstraint {
            name: "disabled".to_string(),
            enabled: false,
            ..Default::default()
        };

        let profile = ConstraintProfile::default()
            .with_output_constraint(enabled)
            .with_output_constraint(disabled);

        let enabled_count = profile.enabled_output_constraints().count();
        assert_eq!(enabled_count, 1);
    }

    #[test]
    fn test_constraint_profile_serialization() {
        let profile = ConstraintProfile {
            name: "test_profile".to_string(),
            description: "Test profile".to_string(),
            output_constraints: vec![OutputConstraint {
                name: "json".to_string(),
                description: "JSON format".to_string(),
                json_schema: None,
                prompt_injection: "Use JSON".to_string(),
                validation_mode: ValidationMode::Strict,
                enabled: true,
            }],
            role_anchor: Some(RoleAnchor {
                role_name: "Agent".to_string(),
                anchor_prompt: "You are an agent.".to_string(),
                reanchor_interval: Some(5),
                drift_detection: true,
                drift_keywords: vec!["pretend".to_string()],
                drift_response: None,
            }),
            security: SecurityBoundary::default(),
            token_limits: TokenLimits::default(),
            reasoning_mode: ReasoningMode::ChainOfThought,
        };

        // Test JSON serialization
        let json = serde_json::to_string_pretty(&profile).unwrap();
        let from_json: ConstraintProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(from_json.name, profile.name);
        assert_eq!(from_json.reasoning_mode, profile.reasoning_mode);

        // Test YAML serialization
        let yaml = serde_yaml::to_string(&profile).unwrap();
        let from_yaml: ConstraintProfile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(from_yaml.name, profile.name);
        assert_eq!(from_yaml.reasoning_mode, profile.reasoning_mode);
    }

    #[test]
    fn test_constraint_profile_builder_chain() {
        let limits = TokenLimits {
            system_prompt_max: 10000,
            response_max: 5000,
            section_budgets: HashMap::new(),
        };

        let security = SecurityBoundary {
            allowed_paths: vec!["/workspace/**".to_string()],
            ..Default::default()
        };

        let profile = ConstraintProfile::new("chained", "Builder pattern test")
            .with_reasoning_mode(ReasoningMode::ReAct)
            .with_token_limits(limits.clone())
            .with_security(security.clone())
            .with_output_constraint(OutputConstraint::default());

        assert_eq!(profile.name, "chained");
        assert_eq!(profile.reasoning_mode, ReasoningMode::ReAct);
        assert_eq!(profile.token_limits.system_prompt_max, 10000);
        assert_eq!(profile.security.allowed_paths.len(), 1);
        assert_eq!(profile.output_constraints.len(), 1);
    }
}
