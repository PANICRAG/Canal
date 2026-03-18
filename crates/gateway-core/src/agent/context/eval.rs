//! Prompt evaluation framework.
//!
//! Provides a lightweight evaluation system for testing prompt quality,
//! including static assertions for checking prompt structure, content,
//! and token budgets. Inspired by Promptfoo but designed for static
//! analysis without external dependencies.
//!
//! # Overview
//!
//! The eval framework consists of:
//! - [`EvalCase`] - A single test case with input and assertions
//! - [`EvalAssertion`] - Individual checks (contains, token range, etc.)
//! - [`PromptEvaluator`] - Runs cases against a prompt string
//! - [`EvalReport`] - Aggregated results from running all cases
//!
//! # Example
//!
//! ```rust
//! use gateway_core::agent::context::eval::{
//!     EvalCase, EvalAssertion, PromptEvaluator,
//! };
//!
//! let case = EvalCase {
//!     name: "basic_prompt_check".to_string(),
//!     description: "Verify system prompt has required sections".to_string(),
//!     input: "Fix the login bug".to_string(),
//!     assertions: vec![
//!         EvalAssertion::Contains { value: "You are".to_string() },
//!         EvalAssertion::SectionExists { section_name: "## Rules".to_string() },
//!         EvalAssertion::TokenRange { min: 100, max: 5000 },
//!     ],
//!     tags: vec!["basic".to_string()],
//! };
//!
//! let prompt = "You are a helpful assistant.\n\n## Rules\nBe concise and accurate.";
//! let result = PromptEvaluator::evaluate(&case, prompt);
//! assert!(result.passed);
//! ```

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// A single evaluation case for testing prompt quality.
///
/// Each case has a name, description, input (the task that generated the prompt),
/// a set of assertions to check, and optional tags for filtering.
///
/// Cases can be loaded from YAML/JSON for config-driven evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    /// Name/identifier for this eval case.
    pub name: String,
    /// Description of what this case tests.
    pub description: String,
    /// Input task or prompt to evaluate.
    pub input: String,
    /// Expected assertions to check against the resolved prompt.
    pub assertions: Vec<EvalAssertion>,
    /// Tags for filtering eval cases.
    pub tags: Vec<String>,
}

/// An assertion to check against a resolved prompt or LLM output.
///
/// All assertion types use simple string matching (no regex dependency).
/// Each variant maps to a specific check with clear pass/fail semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EvalAssertion {
    /// Check that the prompt contains a specific string.
    Contains {
        /// The substring to search for.
        value: String,
    },
    /// Check that the prompt does NOT contain a specific string.
    NotContains {
        /// The substring that must NOT be present.
        value: String,
    },
    /// Check that estimated token count is within a range.
    ///
    /// Token count is estimated as `prompt.len() / 4` (rough approximation
    /// for English text with typical tokenizers).
    TokenRange {
        /// Minimum acceptable token count (inclusive).
        min: usize,
        /// Maximum acceptable token count (inclusive).
        max: usize,
    },
    /// Check that a specific section exists in the prompt.
    ///
    /// Checks for the exact section name string (e.g., "## Rules").
    SectionExists {
        /// The section name or header to find.
        section_name: String,
    },
    /// Check that a skill was included in the prompt (case-insensitive).
    SkillIncluded {
        /// The skill name to search for (case-insensitive).
        skill_name: String,
    },
    /// Check that a skill was NOT included in the prompt (case-insensitive).
    SkillExcluded {
        /// The skill name that must NOT be present (case-insensitive).
        skill_name: String,
    },
    /// Check that knowledge was injected into the prompt.
    ///
    /// Looks for common knowledge injection markers:
    /// `[Knowledge]` or `learned knowledge`.
    KnowledgeInjected,
    /// Check that the prompt starts with a specific prefix.
    StartsWith {
        /// The expected prefix.
        prefix: String,
    },
    /// Check that the prompt ends with a specific suffix.
    EndsWith {
        /// The expected suffix.
        suffix: String,
    },
    /// Check that the prompt contains a line matching a specific prefix.
    ///
    /// Splits the prompt into lines and checks if any line starts with the given prefix.
    /// Useful for checking structured prompts with known line formats.
    LineStartsWith {
        /// The line prefix to search for.
        prefix: String,
    },
    /// Check the character length of the prompt is within a range.
    LengthRange {
        /// Minimum character length (inclusive).
        min: usize,
        /// Maximum character length (inclusive).
        max: usize,
    },
}

/// Result of running a single eval case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    /// Name of the eval case.
    pub case_name: String,
    /// Whether all assertions passed.
    pub passed: bool,
    /// Individual assertion results.
    pub assertions: Vec<AssertionResult>,
    /// Duration of the eval in milliseconds.
    pub duration_ms: u64,
}

/// Result of a single assertion check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionResult {
    /// Description of the assertion.
    pub description: String,
    /// Whether this assertion passed.
    pub passed: bool,
    /// Optional message explaining the result.
    pub message: Option<String>,
}

/// Aggregated result of running all eval cases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    /// Individual case results.
    pub results: Vec<EvalResult>,
    /// Total cases run.
    pub total: usize,
    /// Cases passed.
    pub passed: usize,
    /// Cases failed.
    pub failed: usize,
    /// Total duration in milliseconds.
    pub total_duration_ms: u64,
}

impl EvalReport {
    /// Returns the pass rate as a percentage (0.0 to 100.0).
    ///
    /// # Example
    ///
    /// ```rust
    /// # use gateway_core::agent::context::eval::EvalReport;
    /// let report = EvalReport {
    ///     results: vec![],
    ///     total: 10,
    ///     passed: 8,
    ///     failed: 2,
    ///     total_duration_ms: 100,
    /// };
    /// assert!((report.pass_rate() - 80.0).abs() < f64::EPSILON);
    /// ```
    pub fn pass_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.passed as f64 / self.total as f64) * 100.0
    }

    /// Returns the names of all failed cases.
    pub fn failed_cases(&self) -> Vec<&str> {
        self.results
            .iter()
            .filter(|r| !r.passed)
            .map(|r| r.case_name.as_str())
            .collect()
    }

    /// Returns true if all cases passed.
    pub fn all_passed(&self) -> bool {
        self.failed == 0
    }
}

/// Evaluates assertions against a prompt string.
///
/// This is a stateless evaluator -- all methods are static. It checks
/// each assertion in an [`EvalCase`] against a given prompt and produces
/// structured results.
///
/// # Example
///
/// ```rust
/// use gateway_core::agent::context::eval::{EvalCase, EvalAssertion, PromptEvaluator};
///
/// let case = EvalCase {
///     name: "test".to_string(),
///     description: "Test case".to_string(),
///     input: "task".to_string(),
///     assertions: vec![
///         EvalAssertion::Contains { value: "hello".to_string() },
///     ],
///     tags: vec![],
/// };
///
/// let result = PromptEvaluator::evaluate(&case, "hello world");
/// assert!(result.passed);
/// ```
pub struct PromptEvaluator;

impl PromptEvaluator {
    /// Run a single eval case against a prompt.
    ///
    /// Checks every assertion in the case and returns an [`EvalResult`]
    /// indicating whether all assertions passed.
    pub fn evaluate(case: &EvalCase, prompt: &str) -> EvalResult {
        let start = Instant::now();
        let mut assertion_results = Vec::new();

        for assertion in &case.assertions {
            let result = Self::check_assertion(assertion, prompt);
            assertion_results.push(result);
        }

        let passed = assertion_results.iter().all(|r| r.passed);

        EvalResult {
            case_name: case.name.clone(),
            passed,
            assertions: assertion_results,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Run multiple eval cases and produce an aggregated report.
    ///
    /// Each case is evaluated independently against the same prompt.
    pub fn evaluate_all(cases: &[EvalCase], prompt: &str) -> EvalReport {
        let start = Instant::now();
        let mut results = Vec::new();

        for case in cases {
            results.push(Self::evaluate(case, prompt));
        }

        let total = results.len();
        let passed = results.iter().filter(|r| r.passed).count();
        let failed = total - passed;

        EvalReport {
            results,
            total,
            passed,
            failed,
            total_duration_ms: start.elapsed().as_millis() as u64,
        }
    }

    /// Run eval cases filtered by tag.
    ///
    /// Only cases that have at least one of the specified tags will be evaluated.
    pub fn evaluate_with_tags(cases: &[EvalCase], prompt: &str, tags: &[&str]) -> EvalReport {
        let filtered: Vec<EvalCase> = cases
            .iter()
            .filter(|c| c.tags.iter().any(|t| tags.contains(&t.as_str())))
            .cloned()
            .collect();
        Self::evaluate_all(&filtered, prompt)
    }

    fn check_assertion(assertion: &EvalAssertion, prompt: &str) -> AssertionResult {
        match assertion {
            EvalAssertion::Contains { value } => {
                let passed = prompt.contains(value.as_str());
                AssertionResult {
                    description: format!("Contains '{}'", value),
                    passed,
                    message: if !passed {
                        Some(format!(
                            "Expected prompt to contain '{}' but it was not found",
                            value
                        ))
                    } else {
                        None
                    },
                }
            }
            EvalAssertion::NotContains { value } => {
                let passed = !prompt.contains(value.as_str());
                AssertionResult {
                    description: format!("Does not contain '{}'", value),
                    passed,
                    message: if !passed {
                        Some(format!(
                            "Expected prompt NOT to contain '{}' but it was found",
                            value
                        ))
                    } else {
                        None
                    },
                }
            }
            EvalAssertion::TokenRange { min, max } => {
                // Rough approximation: ~4 chars per token for English text
                let estimated_tokens = prompt.len() / 4;
                let passed = estimated_tokens >= *min && estimated_tokens <= *max;
                AssertionResult {
                    description: format!("Token count in range [{}, {}]", min, max),
                    passed,
                    message: Some(format!("Estimated tokens: {}", estimated_tokens)),
                }
            }
            EvalAssertion::SectionExists { section_name } => {
                let passed = prompt.contains(section_name.as_str());
                AssertionResult {
                    description: format!("Section '{}' exists", section_name),
                    passed,
                    message: if !passed {
                        Some(format!("Section '{}' not found in prompt", section_name))
                    } else {
                        None
                    },
                }
            }
            EvalAssertion::SkillIncluded { skill_name } => {
                let passed = prompt.to_lowercase().contains(&skill_name.to_lowercase());
                AssertionResult {
                    description: format!("Skill '{}' included", skill_name),
                    passed,
                    message: if !passed {
                        Some(format!("Skill '{}' not found in prompt", skill_name))
                    } else {
                        None
                    },
                }
            }
            EvalAssertion::SkillExcluded { skill_name } => {
                let passed = !prompt.to_lowercase().contains(&skill_name.to_lowercase());
                AssertionResult {
                    description: format!("Skill '{}' excluded", skill_name),
                    passed,
                    message: if !passed {
                        Some(format!(
                            "Skill '{}' should not be in prompt but was found",
                            skill_name
                        ))
                    } else {
                        None
                    },
                }
            }
            EvalAssertion::KnowledgeInjected => {
                // Check for common knowledge injection markers
                let passed = prompt.contains("[Knowledge]") || prompt.contains("learned knowledge");
                AssertionResult {
                    description: "Knowledge injected".to_string(),
                    passed,
                    message: if !passed {
                        Some("No knowledge injection markers found in prompt".to_string())
                    } else {
                        None
                    },
                }
            }
            EvalAssertion::StartsWith { prefix } => {
                let passed = prompt.starts_with(prefix.as_str());
                AssertionResult {
                    description: format!("Starts with '{}'", prefix),
                    passed,
                    message: if !passed {
                        Some(format!(
                            "Expected prompt to start with '{}' but it starts with '{}'",
                            prefix,
                            &prompt[..prompt.len().min(prefix.len() + 20)]
                        ))
                    } else {
                        None
                    },
                }
            }
            EvalAssertion::EndsWith { suffix } => {
                let passed = prompt.ends_with(suffix.as_str());
                AssertionResult {
                    description: format!("Ends with '{}'", suffix),
                    passed,
                    message: if !passed {
                        let start = prompt.len().saturating_sub(suffix.len() + 20);
                        Some(format!(
                            "Expected prompt to end with '{}' but it ends with '{}'",
                            suffix,
                            &prompt[start..]
                        ))
                    } else {
                        None
                    },
                }
            }
            EvalAssertion::LineStartsWith { prefix } => {
                let passed = prompt.lines().any(|line| line.starts_with(prefix.as_str()));
                AssertionResult {
                    description: format!("Has line starting with '{}'", prefix),
                    passed,
                    message: if !passed {
                        Some(format!("No line in prompt starts with '{}'", prefix))
                    } else {
                        None
                    },
                }
            }
            EvalAssertion::LengthRange { min, max } => {
                let len = prompt.len();
                let passed = len >= *min && len <= *max;
                AssertionResult {
                    description: format!("Character length in range [{}, {}]", min, max),
                    passed,
                    message: Some(format!("Actual length: {}", len)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------
    // Helper to build a simple case with a single assertion
    // -------------------------------------------------------
    fn single_assertion_case(name: &str, assertion: EvalAssertion) -> EvalCase {
        EvalCase {
            name: name.to_string(),
            description: format!("Test {}", name),
            input: "test input".to_string(),
            assertions: vec![assertion],
            tags: vec!["unit".to_string()],
        }
    }

    // -------------------------------------------------------
    // Contains assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_contains_pass() {
        let case = single_assertion_case(
            "contains_pass",
            EvalAssertion::Contains {
                value: "hello".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "hello world");
        assert!(result.passed);
        assert_eq!(result.assertions.len(), 1);
        assert!(result.assertions[0].passed);
        assert!(result.assertions[0].message.is_none());
    }

    #[test]
    fn test_contains_fail() {
        let case = single_assertion_case(
            "contains_fail",
            EvalAssertion::Contains {
                value: "missing".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "hello world");
        assert!(!result.passed);
        assert!(!result.assertions[0].passed);
        assert!(result.assertions[0].message.is_some());
        assert!(result.assertions[0]
            .message
            .as_ref()
            .unwrap()
            .contains("missing"));
    }

    #[test]
    fn test_contains_empty_prompt() {
        let case = single_assertion_case(
            "contains_empty",
            EvalAssertion::Contains {
                value: "anything".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "");
        assert!(!result.passed);
    }

    #[test]
    fn test_contains_empty_value() {
        let case = single_assertion_case(
            "contains_empty_val",
            EvalAssertion::Contains {
                value: "".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "any prompt");
        // Empty string is always contained
        assert!(result.passed);
    }

    // -------------------------------------------------------
    // NotContains assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_not_contains_pass() {
        let case = single_assertion_case(
            "not_contains_pass",
            EvalAssertion::NotContains {
                value: "secret".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "hello world");
        assert!(result.passed);
        assert!(result.assertions[0].message.is_none());
    }

    #[test]
    fn test_not_contains_fail() {
        let case = single_assertion_case(
            "not_contains_fail",
            EvalAssertion::NotContains {
                value: "hello".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "hello world");
        assert!(!result.passed);
        assert!(result.assertions[0]
            .message
            .as_ref()
            .unwrap()
            .contains("hello"));
    }

    // -------------------------------------------------------
    // TokenRange assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_token_range_within() {
        // "hello world" = 11 chars, ~2 tokens
        let case = single_assertion_case(
            "token_in_range",
            EvalAssertion::TokenRange { min: 1, max: 10 },
        );
        let result = PromptEvaluator::evaluate(&case, "hello world");
        assert!(result.passed);
        assert!(result.assertions[0].message.is_some()); // always has estimated count
    }

    #[test]
    fn test_token_range_too_small() {
        let case = single_assertion_case(
            "token_too_small",
            EvalAssertion::TokenRange { min: 100, max: 200 },
        );
        let result = PromptEvaluator::evaluate(&case, "short");
        assert!(!result.passed);
    }

    #[test]
    fn test_token_range_too_large() {
        let long_prompt = "a".repeat(10000); // ~2500 tokens
        let case = single_assertion_case(
            "token_too_large",
            EvalAssertion::TokenRange { min: 1, max: 100 },
        );
        let result = PromptEvaluator::evaluate(&case, &long_prompt);
        assert!(!result.passed);
    }

    #[test]
    fn test_token_range_exact_boundary() {
        // 400 chars = 100 estimated tokens
        let prompt = "a".repeat(400);
        let case = single_assertion_case(
            "token_exact",
            EvalAssertion::TokenRange { min: 100, max: 100 },
        );
        let result = PromptEvaluator::evaluate(&case, &prompt);
        assert!(result.passed);
    }

    // -------------------------------------------------------
    // SectionExists assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_section_exists_pass() {
        let case = single_assertion_case(
            "section_exists",
            EvalAssertion::SectionExists {
                section_name: "## Rules".to_string(),
            },
        );
        let prompt = "You are an assistant.\n\n## Rules\n- Be helpful\n- Be concise";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(result.passed);
    }

    #[test]
    fn test_section_exists_fail() {
        let case = single_assertion_case(
            "section_missing",
            EvalAssertion::SectionExists {
                section_name: "## Context".to_string(),
            },
        );
        let prompt = "You are an assistant.\n\n## Rules\n- Be helpful";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(!result.passed);
        assert!(result.assertions[0]
            .message
            .as_ref()
            .unwrap()
            .contains("Context"));
    }

    // -------------------------------------------------------
    // SkillIncluded assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_skill_included_pass() {
        let case = single_assertion_case(
            "skill_included",
            EvalAssertion::SkillIncluded {
                skill_name: "code_review".to_string(),
            },
        );
        let prompt = "You have the Code_Review skill loaded.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(result.passed); // case-insensitive match
    }

    #[test]
    fn test_skill_included_case_insensitive() {
        let case = single_assertion_case(
            "skill_case",
            EvalAssertion::SkillIncluded {
                skill_name: "DEBUGGING".to_string(),
            },
        );
        let prompt = "Your debugging skills are available.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(result.passed);
    }

    #[test]
    fn test_skill_included_fail() {
        let case = single_assertion_case(
            "skill_missing",
            EvalAssertion::SkillIncluded {
                skill_name: "browser_automation".to_string(),
            },
        );
        let prompt = "You are a coding assistant.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(!result.passed);
    }

    // -------------------------------------------------------
    // SkillExcluded assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_skill_excluded_pass() {
        let case = single_assertion_case(
            "skill_excluded_pass",
            EvalAssertion::SkillExcluded {
                skill_name: "admin_tools".to_string(),
            },
        );
        let prompt = "You are a regular user assistant.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(result.passed);
    }

    #[test]
    fn test_skill_excluded_fail() {
        let case = single_assertion_case(
            "skill_excluded_fail",
            EvalAssertion::SkillExcluded {
                skill_name: "admin".to_string(),
            },
        );
        let prompt = "You have Admin privileges.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(!result.passed);
    }

    // -------------------------------------------------------
    // KnowledgeInjected assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_knowledge_injected_bracket_marker() {
        let case = single_assertion_case("knowledge_bracket", EvalAssertion::KnowledgeInjected);
        let prompt = "Context:\n[Knowledge]\n- User prefers Rust.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(result.passed);
    }

    #[test]
    fn test_knowledge_injected_text_marker() {
        let case = single_assertion_case("knowledge_text", EvalAssertion::KnowledgeInjected);
        let prompt = "Based on learned knowledge from previous sessions.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(result.passed);
    }

    #[test]
    fn test_knowledge_not_injected() {
        let case = single_assertion_case("no_knowledge", EvalAssertion::KnowledgeInjected);
        let prompt = "You are a helpful assistant.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(!result.passed);
        assert!(result.assertions[0]
            .message
            .as_ref()
            .unwrap()
            .contains("No knowledge"));
    }

    // -------------------------------------------------------
    // StartsWith assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_starts_with_pass() {
        let case = single_assertion_case(
            "starts_with",
            EvalAssertion::StartsWith {
                prefix: "You are".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "You are an AI assistant.");
        assert!(result.passed);
    }

    #[test]
    fn test_starts_with_fail() {
        let case = single_assertion_case(
            "starts_with_fail",
            EvalAssertion::StartsWith {
                prefix: "System:".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "You are an AI assistant.");
        assert!(!result.passed);
    }

    // -------------------------------------------------------
    // EndsWith assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_ends_with_pass() {
        let case = single_assertion_case(
            "ends_with",
            EvalAssertion::EndsWith {
                suffix: "carefully.".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "Please respond carefully.");
        assert!(result.passed);
    }

    #[test]
    fn test_ends_with_fail() {
        let case = single_assertion_case(
            "ends_with_fail",
            EvalAssertion::EndsWith {
                suffix: "period.".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "No period here");
        assert!(!result.passed);
    }

    // -------------------------------------------------------
    // LineStartsWith assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_line_starts_with_pass() {
        let case = single_assertion_case(
            "line_starts",
            EvalAssertion::LineStartsWith {
                prefix: "- Rule:".to_string(),
            },
        );
        let prompt = "## Rules\n- Rule: Be concise\n- Rule: Be accurate";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(result.passed);
    }

    #[test]
    fn test_line_starts_with_fail() {
        let case = single_assertion_case(
            "line_starts_fail",
            EvalAssertion::LineStartsWith {
                prefix: "ERROR:".to_string(),
            },
        );
        let prompt = "line one\nline two\nline three";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(!result.passed);
    }

    // -------------------------------------------------------
    // LengthRange assertion tests
    // -------------------------------------------------------
    #[test]
    fn test_length_range_within() {
        let case = single_assertion_case(
            "length_in_range",
            EvalAssertion::LengthRange { min: 5, max: 20 },
        );
        let result = PromptEvaluator::evaluate(&case, "hello world"); // 11 chars
        assert!(result.passed);
    }

    #[test]
    fn test_length_range_out_of_range() {
        let case = single_assertion_case(
            "length_out",
            EvalAssertion::LengthRange { min: 100, max: 200 },
        );
        let result = PromptEvaluator::evaluate(&case, "short");
        assert!(!result.passed);
    }

    // -------------------------------------------------------
    // evaluate() tests
    // -------------------------------------------------------
    #[test]
    fn test_evaluate_multiple_assertions_all_pass() {
        let case = EvalCase {
            name: "multi_pass".to_string(),
            description: "All assertions pass".to_string(),
            input: "task".to_string(),
            assertions: vec![
                EvalAssertion::Contains {
                    value: "assistant".to_string(),
                },
                EvalAssertion::NotContains {
                    value: "secret".to_string(),
                },
                EvalAssertion::SectionExists {
                    section_name: "## Rules".to_string(),
                },
            ],
            tags: vec![],
        };
        let prompt = "You are an assistant.\n\n## Rules\nBe helpful.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(result.passed);
        assert_eq!(result.assertions.len(), 3);
        assert!(result.assertions.iter().all(|a| a.passed));
    }

    #[test]
    fn test_evaluate_multiple_assertions_one_fails() {
        let case = EvalCase {
            name: "multi_fail".to_string(),
            description: "One assertion fails".to_string(),
            input: "task".to_string(),
            assertions: vec![
                EvalAssertion::Contains {
                    value: "assistant".to_string(),
                },
                EvalAssertion::Contains {
                    value: "MISSING_SECTION".to_string(),
                },
                EvalAssertion::NotContains {
                    value: "secret".to_string(),
                },
            ],
            tags: vec![],
        };
        let prompt = "You are an assistant.";
        let result = PromptEvaluator::evaluate(&case, prompt);
        assert!(!result.passed);
        assert!(result.assertions[0].passed);
        assert!(!result.assertions[1].passed);
        assert!(result.assertions[2].passed);
    }

    #[test]
    fn test_evaluate_empty_assertions() {
        let case = EvalCase {
            name: "empty".to_string(),
            description: "No assertions".to_string(),
            input: "task".to_string(),
            assertions: vec![],
            tags: vec![],
        };
        let result = PromptEvaluator::evaluate(&case, "any prompt");
        // No assertions means vacuously true
        assert!(result.passed);
        assert_eq!(result.assertions.len(), 0);
    }

    #[test]
    fn test_evaluate_has_duration() {
        let case = single_assertion_case(
            "timing",
            EvalAssertion::Contains {
                value: "test".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "test prompt");
        // Duration should be non-negative (may be 0 for fast operations)
        assert!(result.duration_ms <= 1000); // sanity check: should be fast
    }

    #[test]
    fn test_evaluate_case_name_preserved() {
        let case = single_assertion_case(
            "my_unique_name",
            EvalAssertion::Contains {
                value: "x".to_string(),
            },
        );
        let result = PromptEvaluator::evaluate(&case, "x");
        assert_eq!(result.case_name, "my_unique_name");
    }

    // -------------------------------------------------------
    // evaluate_all() tests
    // -------------------------------------------------------
    #[test]
    fn test_evaluate_all_all_pass() {
        let cases = vec![
            single_assertion_case(
                "case1",
                EvalAssertion::Contains {
                    value: "hello".to_string(),
                },
            ),
            single_assertion_case(
                "case2",
                EvalAssertion::NotContains {
                    value: "secret".to_string(),
                },
            ),
        ];
        let report = PromptEvaluator::evaluate_all(&cases, "hello world");
        assert_eq!(report.total, 2);
        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 0);
        assert!(report.all_passed());
    }

    #[test]
    fn test_evaluate_all_mixed_results() {
        let cases = vec![
            single_assertion_case(
                "pass_case",
                EvalAssertion::Contains {
                    value: "hello".to_string(),
                },
            ),
            single_assertion_case(
                "fail_case",
                EvalAssertion::Contains {
                    value: "missing".to_string(),
                },
            ),
            single_assertion_case(
                "pass_case_2",
                EvalAssertion::NotContains {
                    value: "secret".to_string(),
                },
            ),
        ];
        let report = PromptEvaluator::evaluate_all(&cases, "hello world");
        assert_eq!(report.total, 3);
        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 1);
        assert!(!report.all_passed());
    }

    #[test]
    fn test_evaluate_all_empty_cases() {
        let cases: Vec<EvalCase> = vec![];
        let report = PromptEvaluator::evaluate_all(&cases, "any prompt");
        assert_eq!(report.total, 0);
        assert_eq!(report.passed, 0);
        assert_eq!(report.failed, 0);
        assert!(report.all_passed());
    }

    // -------------------------------------------------------
    // evaluate_with_tags() tests
    // -------------------------------------------------------
    #[test]
    fn test_evaluate_with_tags_filters_correctly() {
        let cases = vec![
            EvalCase {
                name: "basic_check".to_string(),
                description: "Basic check".to_string(),
                input: "task".to_string(),
                assertions: vec![EvalAssertion::Contains {
                    value: "hello".to_string(),
                }],
                tags: vec!["basic".to_string(), "smoke".to_string()],
            },
            EvalCase {
                name: "advanced_check".to_string(),
                description: "Advanced check".to_string(),
                input: "task".to_string(),
                assertions: vec![EvalAssertion::Contains {
                    value: "advanced".to_string(),
                }],
                tags: vec!["advanced".to_string()],
            },
            EvalCase {
                name: "smoke_check".to_string(),
                description: "Smoke check".to_string(),
                input: "task".to_string(),
                assertions: vec![EvalAssertion::Contains {
                    value: "hello".to_string(),
                }],
                tags: vec!["smoke".to_string()],
            },
        ];

        // Only run "smoke" tagged cases
        let report = PromptEvaluator::evaluate_with_tags(&cases, "hello world", &["smoke"]);
        assert_eq!(report.total, 2); // basic_check (has smoke) + smoke_check
        assert_eq!(report.passed, 2);
    }

    #[test]
    fn test_evaluate_with_tags_no_match() {
        let cases = vec![single_assertion_case(
            "tagged",
            EvalAssertion::Contains {
                value: "x".to_string(),
            },
        )];
        let report = PromptEvaluator::evaluate_with_tags(&cases, "x", &["nonexistent_tag"]);
        assert_eq!(report.total, 0);
    }

    // -------------------------------------------------------
    // EvalReport helper method tests
    // -------------------------------------------------------
    #[test]
    fn test_report_pass_rate() {
        let report = EvalReport {
            results: vec![],
            total: 10,
            passed: 7,
            failed: 3,
            total_duration_ms: 100,
        };
        assert!((report.pass_rate() - 70.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_report_pass_rate_all_passed() {
        let report = EvalReport {
            results: vec![],
            total: 5,
            passed: 5,
            failed: 0,
            total_duration_ms: 50,
        };
        assert!((report.pass_rate() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_report_pass_rate_none_passed() {
        let report = EvalReport {
            results: vec![],
            total: 3,
            passed: 0,
            failed: 3,
            total_duration_ms: 30,
        };
        assert!((report.pass_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_report_pass_rate_empty() {
        let report = EvalReport {
            results: vec![],
            total: 0,
            passed: 0,
            failed: 0,
            total_duration_ms: 0,
        };
        assert!((report.pass_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_report_failed_cases() {
        let report = EvalReport {
            results: vec![
                EvalResult {
                    case_name: "pass_case".to_string(),
                    passed: true,
                    assertions: vec![],
                    duration_ms: 0,
                },
                EvalResult {
                    case_name: "fail_case_1".to_string(),
                    passed: false,
                    assertions: vec![],
                    duration_ms: 0,
                },
                EvalResult {
                    case_name: "fail_case_2".to_string(),
                    passed: false,
                    assertions: vec![],
                    duration_ms: 0,
                },
            ],
            total: 3,
            passed: 1,
            failed: 2,
            total_duration_ms: 0,
        };
        let failed = report.failed_cases();
        assert_eq!(failed.len(), 2);
        assert!(failed.contains(&"fail_case_1"));
        assert!(failed.contains(&"fail_case_2"));
    }

    #[test]
    fn test_report_all_passed_true() {
        let report = EvalReport {
            results: vec![],
            total: 5,
            passed: 5,
            failed: 0,
            total_duration_ms: 0,
        };
        assert!(report.all_passed());
    }

    #[test]
    fn test_report_all_passed_false() {
        let report = EvalReport {
            results: vec![],
            total: 5,
            passed: 4,
            failed: 1,
            total_duration_ms: 0,
        };
        assert!(!report.all_passed());
    }

    // -------------------------------------------------------
    // Serialization tests
    // -------------------------------------------------------
    #[test]
    fn test_eval_case_json_serialization() {
        let case = EvalCase {
            name: "test_case".to_string(),
            description: "A test".to_string(),
            input: "Fix bug".to_string(),
            assertions: vec![
                EvalAssertion::Contains {
                    value: "hello".to_string(),
                },
                EvalAssertion::TokenRange { min: 10, max: 100 },
            ],
            tags: vec!["smoke".to_string()],
        };

        let json = serde_json::to_string(&case).expect("serialize");
        let deserialized: EvalCase = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.name, "test_case");
        assert_eq!(deserialized.assertions.len(), 2);
        assert_eq!(deserialized.tags, vec!["smoke"]);
    }

    #[test]
    fn test_eval_case_yaml_serialization() {
        let case = EvalCase {
            name: "yaml_case".to_string(),
            description: "YAML test".to_string(),
            input: "Test task".to_string(),
            assertions: vec![
                EvalAssertion::SectionExists {
                    section_name: "## Rules".to_string(),
                },
                EvalAssertion::SkillIncluded {
                    skill_name: "coding".to_string(),
                },
            ],
            tags: vec!["integration".to_string()],
        };

        let yaml = serde_yaml::to_string(&case).expect("serialize yaml");
        let deserialized: EvalCase = serde_yaml::from_str(&yaml).expect("deserialize yaml");
        assert_eq!(deserialized.name, "yaml_case");
        assert_eq!(deserialized.assertions.len(), 2);
    }

    #[test]
    fn test_eval_report_serialization() {
        let report = EvalReport {
            results: vec![EvalResult {
                case_name: "test".to_string(),
                passed: true,
                assertions: vec![AssertionResult {
                    description: "Contains 'x'".to_string(),
                    passed: true,
                    message: None,
                }],
                duration_ms: 5,
            }],
            total: 1,
            passed: 1,
            failed: 0,
            total_duration_ms: 5,
        };

        let json = serde_json::to_string(&report).expect("serialize");
        let deserialized: EvalReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.total, 1);
        assert_eq!(deserialized.results[0].case_name, "test");
    }

    #[test]
    fn test_eval_assertion_json_tagged_format() {
        // Verify serde tagged format works as expected for config-driven evals
        let json = r#"{"type": "contains", "value": "hello"}"#;
        let assertion: EvalAssertion = serde_json::from_str(json).expect("parse");
        match assertion {
            EvalAssertion::Contains { value } => assert_eq!(value, "hello"),
            _ => panic!("Expected Contains variant"),
        }
    }

    #[test]
    fn test_eval_assertion_all_variants_serialize() {
        let assertions = vec![
            EvalAssertion::Contains {
                value: "x".to_string(),
            },
            EvalAssertion::NotContains {
                value: "y".to_string(),
            },
            EvalAssertion::TokenRange { min: 1, max: 100 },
            EvalAssertion::SectionExists {
                section_name: "## S".to_string(),
            },
            EvalAssertion::SkillIncluded {
                skill_name: "s".to_string(),
            },
            EvalAssertion::SkillExcluded {
                skill_name: "t".to_string(),
            },
            EvalAssertion::KnowledgeInjected,
            EvalAssertion::StartsWith {
                prefix: "a".to_string(),
            },
            EvalAssertion::EndsWith {
                suffix: "z".to_string(),
            },
            EvalAssertion::LineStartsWith {
                prefix: "- ".to_string(),
            },
            EvalAssertion::LengthRange { min: 0, max: 1000 },
        ];

        for assertion in &assertions {
            let json = serde_json::to_string(assertion).expect("serialize");
            let _: EvalAssertion = serde_json::from_str(&json).expect("deserialize");
        }
    }

    // -------------------------------------------------------
    // Integration-style tests (realistic prompt scenarios)
    // -------------------------------------------------------
    #[test]
    fn test_realistic_system_prompt_eval() {
        let prompt = "\
You are Canal, an AI coding assistant.

## Rules
- Always respond in English
- Be concise and accurate
- Use code blocks for code

## Skills
- code_review: Analyze code for issues
- debugging: Help debug problems

## Context
[Knowledge]
- User prefers Rust programming language
- User uses VS Code editor";

        let cases = vec![
            EvalCase {
                name: "has_identity".to_string(),
                description: "Prompt establishes agent identity".to_string(),
                input: "any".to_string(),
                assertions: vec![
                    EvalAssertion::StartsWith {
                        prefix: "You are".to_string(),
                    },
                    EvalAssertion::Contains {
                        value: "Canal".to_string(),
                    },
                ],
                tags: vec!["identity".to_string()],
            },
            EvalCase {
                name: "has_rules".to_string(),
                description: "Prompt has rules section".to_string(),
                input: "any".to_string(),
                assertions: vec![
                    EvalAssertion::SectionExists {
                        section_name: "## Rules".to_string(),
                    },
                    EvalAssertion::Contains {
                        value: "English".to_string(),
                    },
                ],
                tags: vec!["structure".to_string()],
            },
            EvalCase {
                name: "skills_loaded".to_string(),
                description: "Required skills are present".to_string(),
                input: "code review task".to_string(),
                assertions: vec![
                    EvalAssertion::SkillIncluded {
                        skill_name: "code_review".to_string(),
                    },
                    EvalAssertion::SkillIncluded {
                        skill_name: "debugging".to_string(),
                    },
                    EvalAssertion::SkillExcluded {
                        skill_name: "browser_automation".to_string(),
                    },
                ],
                tags: vec!["skills".to_string()],
            },
            EvalCase {
                name: "knowledge_present".to_string(),
                description: "Knowledge is injected".to_string(),
                input: "any".to_string(),
                assertions: vec![EvalAssertion::KnowledgeInjected],
                tags: vec!["knowledge".to_string()],
            },
            EvalCase {
                name: "no_secrets".to_string(),
                description: "No secrets or API keys leaked".to_string(),
                input: "any".to_string(),
                assertions: vec![
                    EvalAssertion::NotContains {
                        value: "API_KEY".to_string(),
                    },
                    EvalAssertion::NotContains {
                        value: "password".to_string(),
                    },
                    EvalAssertion::NotContains {
                        value: "Bearer ".to_string(),
                    },
                ],
                tags: vec!["security".to_string()],
            },
            EvalCase {
                name: "token_budget".to_string(),
                description: "Prompt is within token budget".to_string(),
                input: "any".to_string(),
                assertions: vec![EvalAssertion::TokenRange { min: 10, max: 5000 }],
                tags: vec!["budget".to_string()],
            },
        ];

        let report = PromptEvaluator::evaluate_all(&cases, prompt);
        assert_eq!(report.total, 6);
        assert_eq!(report.passed, 6);
        assert_eq!(report.failed, 0);
        assert!(report.all_passed());
        assert!((report.pass_rate() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_realistic_prompt_with_failures() {
        let prompt = "You are an assistant.\nDo stuff.";

        let cases = vec![
            EvalCase {
                name: "has_rules".to_string(),
                description: "Must have rules".to_string(),
                input: "any".to_string(),
                assertions: vec![EvalAssertion::SectionExists {
                    section_name: "## Rules".to_string(),
                }],
                tags: vec!["structure".to_string()],
            },
            EvalCase {
                name: "has_skills".to_string(),
                description: "Must have skills".to_string(),
                input: "any".to_string(),
                assertions: vec![EvalAssertion::SectionExists {
                    section_name: "## Skills".to_string(),
                }],
                tags: vec!["structure".to_string()],
            },
            EvalCase {
                name: "has_identity".to_string(),
                description: "Must start with identity".to_string(),
                input: "any".to_string(),
                assertions: vec![EvalAssertion::StartsWith {
                    prefix: "You are".to_string(),
                }],
                tags: vec!["identity".to_string()],
            },
        ];

        let report = PromptEvaluator::evaluate_all(&cases, prompt);
        assert_eq!(report.total, 3);
        assert_eq!(report.passed, 1); // only "has_identity" passes
        assert_eq!(report.failed, 2);

        let failed = report.failed_cases();
        assert!(failed.contains(&"has_rules"));
        assert!(failed.contains(&"has_skills"));
        assert!(!failed.contains(&"has_identity"));
    }

    #[test]
    fn test_eval_case_from_json_config() {
        // Demonstrates loading eval cases from JSON config (future use)
        let config_json = r###"[
            {
                "name": "basic_check",
                "description": "Basic prompt structure",
                "input": "any task",
                "assertions": [
                    {"type": "contains", "value": "You are"},
                    {"type": "section_exists", "section_name": "## Rules"},
                    {"type": "token_range", "min": 5, "max": 10000}
                ],
                "tags": ["basic", "smoke"]
            },
            {
                "name": "security_check",
                "description": "No sensitive data in prompt",
                "input": "any task",
                "assertions": [
                    {"type": "not_contains", "value": "sk-"},
                    {"type": "not_contains", "value": "password"}
                ],
                "tags": ["security"]
            }
        ]"###;

        let cases: Vec<EvalCase> = serde_json::from_str(config_json).expect("parse config");
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].assertions.len(), 3);
        assert_eq!(cases[1].assertions.len(), 2);

        let prompt = "You are an assistant.\n\n## Rules\nNo sharing secrets.";
        let report = PromptEvaluator::evaluate_all(&cases, prompt);
        assert!(report.all_passed());
    }
}
