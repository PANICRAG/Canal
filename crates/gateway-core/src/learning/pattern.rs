//! Pattern mining algorithms for the learning system.
//!
//! The [`PatternMiner`] analyzes collections of [`Experience`] records
//! to discover recurring patterns:
//!
//! - **Tool sequences** — common n-grams of tool calls in successful runs
//! - **Error recovery** — tool sequences that follow a failed tool call
//! - **Model selection** — which models succeed most often for which task categories

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::experience::Experience;

/// Configuration for the pattern miner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternMinerConfig {
    /// Minimum frequency for a pattern to be retained.
    #[serde(default = "default_min_frequency")]
    pub min_frequency: u32,
    /// Minimum confidence score (0.0 - 1.0).
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f32,
    /// N-gram window size for tool sequence mining.
    #[serde(default = "default_window_size")]
    pub window_size: usize,
    /// Maximum patterns to retain per type.
    #[serde(default = "default_max_patterns")]
    pub max_patterns: usize,
}

fn default_min_frequency() -> u32 {
    3
}
fn default_min_confidence() -> f32 {
    0.5
}
fn default_window_size() -> usize {
    3
}
fn default_max_patterns() -> usize {
    100
}

impl Default for PatternMinerConfig {
    fn default() -> Self {
        Self {
            min_frequency: default_min_frequency(),
            min_confidence: default_min_confidence(),
            window_size: default_window_size(),
            max_patterns: default_max_patterns(),
        }
    }
}

impl PatternMinerConfig {
    /// Validate configuration values. Returns an error message if invalid.
    pub fn validate(&self) -> Result<(), String> {
        if self.min_frequency == 0 {
            return Err("min_frequency must be > 0".into());
        }
        if !(0.0..=1.0).contains(&self.min_confidence) {
            return Err("min_confidence must be in [0.0, 1.0]".into());
        }
        if self.window_size < 2 {
            return Err("window_size must be >= 2".into());
        }
        if self.max_patterns == 0 {
            return Err("max_patterns must be > 0".into());
        }
        Ok(())
    }
}

/// A discovered pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinedPattern {
    /// What kind of pattern this is.
    pub pattern_type: PatternType,
    /// Human-readable description of the pattern.
    pub description: String,
    /// How many times this pattern was observed.
    pub frequency: u32,
    /// Confidence score between 0.0 and 1.0.
    pub confidence: f32,
    /// Example tasks where this pattern was observed.
    pub examples: Vec<String>,
    /// The tool sequence (if applicable).
    pub tool_sequence: Option<Vec<String>>,
    /// Average duration in milliseconds for experiences exhibiting this pattern.
    pub avg_duration_ms: i64,
}

/// Type of discovered pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternType {
    /// Repeated tool call sequences.
    ToolSequence,
    /// Successful error recovery strategies.
    ErrorRecovery,
    /// Common task decomposition patterns.
    TaskDecomposition,
    /// Which model works best for which task type.
    ModelSelection,
    /// Repeated tool sequences that precede failures.
    FailurePattern,
}

/// Internal statistics for an n-gram during mining.
struct NgramStats {
    count: u32,
    total_duration_ms: i64,
    tasks: HashSet<String>,
}

impl Default for NgramStats {
    fn default() -> Self {
        Self {
            count: 0,
            total_duration_ms: 0,
            tasks: HashSet::new(),
        }
    }
}

/// Mines patterns from collected experiences.
///
/// The miner is stateless — each call analyzes the provided experiences
/// independently and returns discovered patterns.
pub struct PatternMiner {
    config: PatternMinerConfig,
}

impl PatternMiner {
    /// Create a new pattern miner.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let miner = PatternMiner::new(PatternMinerConfig::default());
    /// let patterns = miner.mine_tool_sequences(&experiences);
    /// ```
    pub fn new(config: PatternMinerConfig) -> Self {
        Self { config }
    }

    /// Mine tool sequence n-grams from successful experiences.
    ///
    /// Extracts sliding windows of tool names from successful experiences
    /// and returns those that appear at least `min_frequency` times.
    #[tracing::instrument(skip(self, experiences), fields(experience_count = experiences.len()))]
    pub fn mine_tool_sequences(&self, experiences: &[Experience]) -> Vec<MinedPattern> {
        let mut ngram_counts: HashMap<Vec<String>, NgramStats> = HashMap::new();
        let total_successful = experiences.iter().filter(|e| e.is_success()).count();

        for exp in experiences {
            if !exp.is_success() {
                continue;
            }
            if exp.tool_calls.is_empty() {
                continue;
            }

            let tool_names: Vec<String> = exp
                .tool_calls
                .iter()
                .map(|tc| tc.tool_name.clone())
                .collect();

            for n in 2..=self.config.window_size.min(tool_names.len()) {
                for window in tool_names.windows(n) {
                    let key = window.to_vec();
                    let stats = ngram_counts.entry(key).or_default();
                    stats.count += 1;
                    stats.total_duration_ms += exp.duration_ms;
                    stats.tasks.insert(exp.task.clone());
                }
            }
        }

        let mut patterns: Vec<MinedPattern> = ngram_counts
            .into_iter()
            .filter(|(_, stats)| stats.count >= self.config.min_frequency)
            .map(|(tools, stats)| {
                let confidence = if total_successful > 0 {
                    (stats.count as f32 / total_successful as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let avg_duration = if stats.count > 0 {
                    stats.total_duration_ms / stats.count as i64
                } else {
                    0
                };
                MinedPattern {
                    pattern_type: PatternType::ToolSequence,
                    description: format!("Tool chain: {}", tools.join(" -> ")),
                    frequency: stats.count,
                    confidence,
                    examples: stats.tasks.into_iter().take(5).collect(),
                    tool_sequence: Some(tools),
                    avg_duration_ms: avg_duration,
                }
            })
            .filter(|p| p.confidence >= self.config.min_confidence)
            .collect();

        patterns.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        patterns.truncate(self.config.max_patterns);

        tracing::debug!(
            patterns_found = patterns.len(),
            "Tool sequence mining complete"
        );
        patterns
    }

    /// Detect error -> recovery -> success patterns.
    ///
    /// Looks for experiences where a failed tool call is followed by
    /// successful tool calls that ultimately lead to a successful outcome.
    #[tracing::instrument(skip(self, experiences), fields(experience_count = experiences.len()))]
    pub fn mine_error_recovery(&self, experiences: &[Experience]) -> Vec<MinedPattern> {
        let mut recovery_map: HashMap<(String, Vec<String>), u32> = HashMap::new();

        for exp in experiences {
            if !exp.is_success() {
                continue;
            }

            for (i, tc) in exp.tool_calls.iter().enumerate() {
                if tc.error.is_some() || !tc.success {
                    let recovery: Vec<String> = exp.tool_calls[i + 1..]
                        .iter()
                        .take(3)
                        .filter(|t| t.success)
                        .map(|t| t.tool_name.clone())
                        .collect();

                    if !recovery.is_empty() {
                        let key = (tc.tool_name.clone(), recovery);
                        *recovery_map.entry(key).or_default() += 1;
                    }
                }
            }
        }

        let patterns: Vec<MinedPattern> = recovery_map
            .into_iter()
            .filter(|(_, count)| *count >= self.config.min_frequency)
            .map(|((failed_tool, recovery), count)| MinedPattern {
                pattern_type: PatternType::ErrorRecovery,
                description: format!(
                    "When '{}' fails, recover with: {}",
                    failed_tool,
                    recovery.join(" -> ")
                ),
                frequency: count,
                confidence: (count as f32 / experiences.len().max(1) as f32).clamp(0.0, 1.0),
                examples: vec![],
                tool_sequence: Some(recovery),
                avg_duration_ms: 0,
            })
            .collect();

        tracing::debug!(
            patterns_found = patterns.len(),
            "Error recovery mining complete"
        );
        patterns
    }

    /// Discover optimal model-task pairings.
    ///
    /// Groups experiences by task category and model, then computes
    /// success rates to identify which models perform best for which
    /// categories of tasks.
    #[tracing::instrument(skip(self, experiences), fields(experience_count = experiences.len()))]
    pub fn mine_model_selection(&self, experiences: &[Experience]) -> Vec<MinedPattern> {
        // (category, model) -> (success_count, total_count)
        let mut model_stats: HashMap<(String, String), (u32, u32)> = HashMap::new();

        for exp in experiences {
            let category = Self::categorize_task(&exp.task);
            for model in &exp.models_used {
                let key = (category.clone(), model.clone());
                let entry = model_stats.entry(key).or_default();
                entry.1 += 1; // total
                if exp.is_success() {
                    entry.0 += 1; // success
                }
            }
        }

        let patterns: Vec<MinedPattern> = model_stats
            .into_iter()
            .filter(|(_, (_, total))| *total >= self.config.min_frequency)
            .map(|((category, model), (success, total))| {
                let rate = (success as f32 / total as f32).clamp(0.0, 1.0);
                MinedPattern {
                    pattern_type: PatternType::ModelSelection,
                    description: format!(
                        "For '{}' tasks, '{}' achieves {:.0}% success ({}/{})",
                        category,
                        model,
                        rate * 100.0,
                        success,
                        total
                    ),
                    frequency: total,
                    confidence: rate,
                    examples: vec![],
                    tool_sequence: None,
                    avg_duration_ms: 0,
                }
            })
            // R2-L160: Use configured min_confidence instead of hardcoded 0.5
            .filter(|p| p.confidence >= self.config.min_confidence)
            .collect();

        tracing::debug!(
            patterns_found = patterns.len(),
            "Model selection mining complete"
        );
        patterns
    }

    /// Mine failure patterns from failed experiences.
    ///
    /// Analyzes experiences that ended in failure and extracts repeated
    /// tool sequences that precede the failure. These patterns serve as
    /// warnings (e.g., "Avoid: navigate -> click without screenshot").
    #[tracing::instrument(skip(self, experiences), fields(experience_count = experiences.len()))]
    pub fn mine_failure_patterns(&self, experiences: &[Experience]) -> Vec<MinedPattern> {
        let mut ngram_counts: HashMap<Vec<String>, NgramStats> = HashMap::new();
        let total_failed = experiences.iter().filter(|e| !e.is_success()).count();

        for exp in experiences {
            if exp.is_success() {
                continue;
            }
            if exp.tool_calls.is_empty() {
                continue;
            }

            let tool_names: Vec<String> = exp
                .tool_calls
                .iter()
                .map(|tc| tc.tool_name.clone())
                .collect();

            for n in 2..=self.config.window_size.min(tool_names.len()) {
                for window in tool_names.windows(n) {
                    let key = window.to_vec();
                    let stats = ngram_counts.entry(key).or_default();
                    stats.count += 1;
                    stats.total_duration_ms += exp.duration_ms;
                    stats.tasks.insert(exp.task.clone());
                }
            }
        }

        let mut patterns: Vec<MinedPattern> = ngram_counts
            .into_iter()
            .filter(|(_, stats)| stats.count >= self.config.min_frequency)
            .map(|(tools, stats)| {
                let confidence = if total_failed > 0 {
                    (stats.count as f32 / total_failed as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let avg_duration = if stats.count > 0 {
                    stats.total_duration_ms / stats.count as i64
                } else {
                    0
                };
                MinedPattern {
                    pattern_type: PatternType::FailurePattern,
                    description: format!("Avoid: {}", tools.join(" -> ")),
                    frequency: stats.count,
                    confidence,
                    examples: stats.tasks.into_iter().take(5).collect(),
                    tool_sequence: Some(tools),
                    avg_duration_ms: avg_duration,
                }
            })
            .filter(|p| p.confidence >= self.config.min_confidence)
            .collect();

        patterns.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        patterns.truncate(self.config.max_patterns);

        tracing::debug!(
            patterns_found = patterns.len(),
            "Failure pattern mining complete"
        );
        patterns
    }

    /// Simple keyword-based task categorization.
    ///
    /// Returns one of: "code", "research", "browser", "writing", or "general".
    pub fn categorize_task(task: &str) -> String {
        let lower = task.to_lowercase();
        if lower.contains("code")
            || lower.contains("implement")
            || lower.contains("fix")
            || lower.contains("bug")
        {
            "code".into()
        } else if lower.contains("search") || lower.contains("research") || lower.contains("find") {
            "research".into()
        } else if lower.contains("click")
            || lower.contains("browser")
            || lower.contains("screenshot")
            || lower.contains("navigate")
        {
            "browser".into()
        } else if lower.contains("write") || lower.contains("draft") || lower.contains("email") {
            "writing".into()
        } else {
            "general".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learning::experience::ToolCallRecord;

    fn tool_call(name: &str) -> ToolCallRecord {
        ToolCallRecord {
            tool_name: name.into(),
            input_summary: "test".into(),
            success: true,
            duration_ms: 100,
            error: None,
        }
    }

    fn failed_tool_call(name: &str) -> ToolCallRecord {
        ToolCallRecord {
            tool_name: name.into(),
            input_summary: "test".into(),
            success: false,
            duration_ms: 50,
            error: Some("failed".into()),
        }
    }

    fn experience_with_tools(task: &str, tools: Vec<ToolCallRecord>) -> Experience {
        let mut exp = Experience::test_success(task);
        exp.tool_calls = tools;
        exp
    }

    #[test]
    fn test_mine_tool_sequences_basic() {
        let config = PatternMinerConfig {
            min_frequency: 2,
            min_confidence: 0.0,
            window_size: 3,
            max_patterns: 100,
        };
        let miner = PatternMiner::new(config);

        let experiences = vec![
            experience_with_tools("t1", vec![tool_call("a"), tool_call("b"), tool_call("c")]),
            experience_with_tools("t2", vec![tool_call("a"), tool_call("b"), tool_call("c")]),
            experience_with_tools("t3", vec![tool_call("a"), tool_call("b"), tool_call("d")]),
        ];

        let patterns = miner.mine_tool_sequences(&experiences);

        // "a -> b" should appear 3 times
        let ab_pattern = patterns.iter().find(|p| {
            p.tool_sequence
                .as_ref()
                .map_or(false, |ts| ts == &["a", "b"])
        });
        assert!(ab_pattern.is_some());
        assert_eq!(ab_pattern.unwrap().frequency, 3);
    }

    #[test]
    fn test_mine_tool_sequences_filters_by_frequency() {
        let config = PatternMinerConfig {
            min_frequency: 5,
            min_confidence: 0.0,
            ..Default::default()
        };
        let miner = PatternMiner::new(config);

        let experiences = vec![
            experience_with_tools("t1", vec![tool_call("a"), tool_call("b")]),
            experience_with_tools("t2", vec![tool_call("a"), tool_call("b")]),
        ];

        let patterns = miner.mine_tool_sequences(&experiences);
        assert!(patterns.is_empty()); // Only 2 occurrences, need 5
    }

    #[test]
    fn test_mine_tool_sequences_ignores_failures() {
        let config = PatternMinerConfig {
            min_frequency: 1,
            min_confidence: 0.0,
            ..Default::default()
        };
        let miner = PatternMiner::new(config);

        let mut failed = Experience::test_failure("t1", "error");
        failed.tool_calls = vec![tool_call("a"), tool_call("b")];

        let patterns = miner.mine_tool_sequences(&[failed]);
        assert!(patterns.is_empty()); // Only successful experiences
    }

    #[test]
    fn test_mine_tool_sequences_sorted_by_frequency() {
        let config = PatternMinerConfig {
            min_frequency: 1,
            min_confidence: 0.0,
            window_size: 2,
            max_patterns: 100,
        };
        let miner = PatternMiner::new(config);

        let experiences = vec![
            experience_with_tools("t1", vec![tool_call("x"), tool_call("y")]),
            experience_with_tools("t2", vec![tool_call("a"), tool_call("b")]),
            experience_with_tools("t3", vec![tool_call("a"), tool_call("b")]),
            experience_with_tools("t4", vec![tool_call("a"), tool_call("b")]),
        ];

        let patterns = miner.mine_tool_sequences(&experiences);
        // a->b (3 times) should come before x->y (1 time)
        assert!(patterns.len() >= 2);
        assert!(patterns[0].frequency >= patterns[1].frequency);
    }

    #[test]
    fn test_mine_error_recovery() {
        let config = PatternMinerConfig {
            min_frequency: 2,
            min_confidence: 0.0,
            ..Default::default()
        };
        let miner = PatternMiner::new(config);

        let experiences = vec![
            experience_with_tools(
                "t1",
                vec![
                    failed_tool_call("api_call"),
                    tool_call("retry"),
                    tool_call("api_call"),
                ],
            ),
            experience_with_tools(
                "t2",
                vec![
                    failed_tool_call("api_call"),
                    tool_call("retry"),
                    tool_call("api_call"),
                ],
            ),
        ];

        let patterns = miner.mine_error_recovery(&experiences);
        assert!(!patterns.is_empty());

        let recovery = &patterns[0];
        assert_eq!(recovery.pattern_type, PatternType::ErrorRecovery);
        assert!(recovery.description.contains("api_call"));
    }

    #[test]
    fn test_mine_error_recovery_ignores_failed_experiences() {
        let config = PatternMinerConfig {
            min_frequency: 1,
            ..Default::default()
        };
        let miner = PatternMiner::new(config);

        // This experience failed overall, so we should not learn from it
        let mut exp = Experience::test_failure("t1", "final error");
        exp.tool_calls = vec![failed_tool_call("api_call"), tool_call("retry")];

        let patterns = miner.mine_error_recovery(&[exp]);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_mine_model_selection() {
        let config = PatternMinerConfig {
            min_frequency: 2,
            min_confidence: 0.0,
            ..Default::default()
        };
        let miner = PatternMiner::new(config);

        let mut experiences = Vec::new();
        for _ in 0..5 {
            let mut exp = Experience::test_success("implement the feature");
            exp.models_used = vec!["claude-sonnet".into()];
            experiences.push(exp);
        }
        for _ in 0..2 {
            let mut exp = Experience::test_failure("implement bug fix", "timeout");
            exp.models_used = vec!["gpt-4o".into()];
            experiences.push(exp);
        }

        let patterns = miner.mine_model_selection(&experiences);

        let claude_pattern = patterns
            .iter()
            .find(|p| p.description.contains("claude-sonnet"));
        assert!(claude_pattern.is_some());
        assert!(claude_pattern.unwrap().confidence > 0.5);
    }

    #[test]
    fn test_mine_model_selection_filters_low_confidence() {
        let config = PatternMinerConfig {
            min_frequency: 2,
            ..Default::default()
        };
        let miner = PatternMiner::new(config);

        // 1 success, 3 failures -> 25% rate, below 0.5 threshold
        let mut experiences = Vec::new();
        {
            let mut exp = Experience::test_success("implement feature");
            exp.models_used = vec!["bad-model".into()];
            experiences.push(exp);
        }
        for _ in 0..3 {
            let mut exp = Experience::test_failure("implement fix", "error");
            exp.models_used = vec!["bad-model".into()];
            experiences.push(exp);
        }

        let patterns = miner.mine_model_selection(&experiences);
        let bad_model = patterns
            .iter()
            .find(|p| p.description.contains("bad-model"));
        assert!(bad_model.is_none()); // Filtered out due to low confidence
    }

    #[test]
    fn test_categorize_task() {
        assert_eq!(PatternMiner::categorize_task("Fix the login bug"), "code");
        assert_eq!(
            PatternMiner::categorize_task("Search for API docs"),
            "research"
        );
        assert_eq!(
            PatternMiner::categorize_task("Click the submit button"),
            "browser"
        );
        assert_eq!(PatternMiner::categorize_task("Draft an email"), "writing");
        assert_eq!(PatternMiner::categorize_task("Hello world"), "general");
    }

    #[test]
    fn test_categorize_task_case_insensitive() {
        assert_eq!(PatternMiner::categorize_task("IMPLEMENT feature"), "code");
        assert_eq!(PatternMiner::categorize_task("SEARCH for info"), "research");
    }

    #[test]
    fn test_empty_experiences() {
        let miner = PatternMiner::new(PatternMinerConfig::default());
        assert!(miner.mine_tool_sequences(&[]).is_empty());
        assert!(miner.mine_error_recovery(&[]).is_empty());
        assert!(miner.mine_model_selection(&[]).is_empty());
    }

    #[test]
    fn test_pattern_type_serialize() {
        let pt = PatternType::ToolSequence;
        let json = serde_json::to_string(&pt).unwrap();
        assert_eq!(json, "\"tool_sequence\"");

        let pt2: PatternType = serde_json::from_str(&json).unwrap();
        assert_eq!(pt2, PatternType::ToolSequence);
    }

    #[test]
    fn test_mined_pattern_serialize() {
        let pattern = MinedPattern {
            pattern_type: PatternType::ToolSequence,
            description: "Tool chain: a -> b".into(),
            frequency: 10,
            confidence: 0.8,
            examples: vec!["task1".into()],
            tool_sequence: Some(vec!["a".into(), "b".into()]),
            avg_duration_ms: 500,
        };
        let json = serde_json::to_string(&pattern).unwrap();
        let parsed: MinedPattern = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.frequency, 10);
        assert_eq!(parsed.tool_sequence, Some(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn test_config_defaults() {
        let config = PatternMinerConfig::default();
        assert_eq!(config.min_frequency, 3);
        assert!((config.min_confidence - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.window_size, 3);
        assert_eq!(config.max_patterns, 100);
    }

    #[test]
    fn test_config_serialize() {
        let config = PatternMinerConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: PatternMinerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.min_frequency, config.min_frequency);
    }

    #[test]
    fn test_failure_pattern_type_serialize() {
        let pt = PatternType::FailurePattern;
        let json = serde_json::to_string(&pt).unwrap();
        assert_eq!(json, "\"failure_pattern\"");

        let pt2: PatternType = serde_json::from_str(&json).unwrap();
        assert_eq!(pt2, PatternType::FailurePattern);
    }

    #[test]
    fn test_mine_failure_patterns_basic() {
        let config = PatternMinerConfig {
            min_frequency: 2,
            min_confidence: 0.0,
            window_size: 3,
            max_patterns: 100,
        };
        let miner = PatternMiner::new(config);

        // Create failed experiences with tool calls
        let mut exp1 = Experience::test_failure("navigate and click", "element not found");
        exp1.tool_calls = vec![tool_call("navigate"), tool_call("click")];
        let mut exp2 = Experience::test_failure("navigate and click page", "timeout");
        exp2.tool_calls = vec![tool_call("navigate"), tool_call("click")];

        let patterns = miner.mine_failure_patterns(&[exp1, exp2]);

        // "navigate -> click" should appear 2 times in failures
        let nav_click = patterns.iter().find(|p| {
            p.tool_sequence
                .as_ref()
                .map_or(false, |ts| ts == &["navigate", "click"])
        });
        assert!(nav_click.is_some());
        let p = nav_click.unwrap();
        assert_eq!(p.frequency, 2);
        assert_eq!(p.pattern_type, PatternType::FailurePattern);
        assert!(p.description.starts_with("Avoid:"));
    }

    #[test]
    fn test_mine_failure_patterns_ignores_successes() {
        let config = PatternMinerConfig {
            min_frequency: 1,
            min_confidence: 0.0,
            ..Default::default()
        };
        let miner = PatternMiner::new(config);

        // Only successful experiences -- no failure patterns should be found
        let experiences = vec![
            experience_with_tools("t1", vec![tool_call("a"), tool_call("b")]),
            experience_with_tools("t2", vec![tool_call("a"), tool_call("b")]),
        ];

        let patterns = miner.mine_failure_patterns(&experiences);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_mine_failure_patterns_empty() {
        let miner = PatternMiner::new(PatternMinerConfig::default());
        assert!(miner.mine_failure_patterns(&[]).is_empty());
    }

    #[test]
    fn test_mine_failure_patterns_filters_by_frequency() {
        let config = PatternMinerConfig {
            min_frequency: 5,
            min_confidence: 0.0,
            ..Default::default()
        };
        let miner = PatternMiner::new(config);

        let mut exp = Experience::test_failure("t1", "error");
        exp.tool_calls = vec![tool_call("a"), tool_call("b")];

        let patterns = miner.mine_failure_patterns(&[exp]);
        assert!(patterns.is_empty()); // Only 1 occurrence, need 5
    }

    #[test]
    fn test_mine_failure_patterns_sorted_by_frequency() {
        let config = PatternMinerConfig {
            min_frequency: 1,
            min_confidence: 0.0,
            window_size: 2,
            max_patterns: 100,
        };
        let miner = PatternMiner::new(config);

        let mut failures = Vec::new();
        // "a -> b" appears 3 times in failures
        for _ in 0..3 {
            let mut exp = Experience::test_failure("task", "err");
            exp.tool_calls = vec![tool_call("a"), tool_call("b")];
            failures.push(exp);
        }
        // "x -> y" appears 1 time
        {
            let mut exp = Experience::test_failure("task2", "err");
            exp.tool_calls = vec![tool_call("x"), tool_call("y")];
            failures.push(exp);
        }

        let patterns = miner.mine_failure_patterns(&failures);
        assert!(patterns.len() >= 2);
        assert!(patterns[0].frequency >= patterns[1].frequency);
    }

    #[test]
    fn test_mine_failure_patterns_no_tool_calls() {
        let config = PatternMinerConfig {
            min_frequency: 1,
            min_confidence: 0.0,
            ..Default::default()
        };
        let miner = PatternMiner::new(config);

        // Failed experience with no tool calls
        let exp = Experience::test_failure("t1", "error");
        let patterns = miner.mine_failure_patterns(&[exp]);
        assert!(patterns.is_empty());
    }
}
