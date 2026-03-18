//! Worker types for Orchestrator-Worker pattern
//!
//! Defines the core types for the lead agent (Orchestrator) to decompose tasks
//! and dispatch them to multiple worker agents for parallel execution.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// Specification for a single worker agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSpec {
    /// Unique identifier for this worker
    pub id: Uuid,
    /// Human-readable name for the worker
    pub name: String,
    /// Prompt/instructions for the worker
    pub prompt: String,
    /// Agent type (e.g., "Explore", "Bash", "Plan", "Code")
    pub agent_type: String,
    /// Model to use (defaults to Sonnet if not specified)
    #[serde(default)]
    pub model: Option<String>,
    /// Maximum number of agentic turns
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Tools available to this worker
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Execution timeout for this worker
    #[serde(default, with = "optional_duration_secs")]
    pub timeout: Option<Duration>,
    /// Worker IDs that must complete before this worker starts (DAG dependencies)
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    /// Priority level (higher = more important)
    #[serde(default)]
    pub priority: u32,
}

impl WorkerSpec {
    /// Create a new worker spec with minimal configuration
    pub fn new(name: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            prompt: prompt.into(),
            agent_type: "general-purpose".to_string(),
            model: None,
            max_turns: None,
            allowed_tools: None,
            timeout: None,
            depends_on: Vec::new(),
            priority: 0,
        }
    }

    /// Set the agent type
    pub fn with_agent_type(mut self, agent_type: impl Into<String>) -> Self {
        self.agent_type = agent_type.into();
        self
    }

    /// Set the model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set max turns
    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = Some(max_turns);
        self
    }

    /// Set allowed tools
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Add a dependency on another worker
    pub fn depends_on(mut self, worker_id: Uuid) -> Self {
        self.depends_on.push(worker_id);
        self
    }

    /// Set priority
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }
}

/// Result from a single worker's execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerResult {
    /// ID of the worker that produced this result
    pub worker_id: Uuid,
    /// Whether the worker completed successfully
    pub success: bool,
    /// Content/output produced by the worker
    pub content: String,
    /// Error message if the worker failed
    #[serde(default)]
    pub error: Option<String>,
    /// Token usage statistics
    #[serde(default)]
    pub usage: Option<WorkerUsage>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Token usage for a worker execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Status of a worker during execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    /// Worker is waiting for dependencies
    Pending,
    /// Worker is currently executing
    Running,
    /// Worker completed successfully
    Completed,
    /// Worker failed with an error
    Failed,
    /// Worker timed out
    TimedOut,
    /// Worker was cancelled (e.g., budget exhausted)
    Cancelled,
}

/// Combined result from all workers in an orchestration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratedResult {
    /// Results from all workers
    pub worker_results: Vec<WorkerResult>,
    /// Whether all workers succeeded
    pub all_succeeded: bool,
    /// Synthesized result from the lead agent (if synthesize_results was true)
    #[serde(default)]
    pub synthesized_output: Option<String>,
    /// Total execution duration in milliseconds
    pub total_duration_ms: u64,
    /// Total token usage across all workers
    #[serde(default)]
    pub total_usage: Option<WorkerUsage>,
}

/// Configuration for the orchestrator-worker system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Model to use for the lead agent (default: opus)
    #[serde(default = "default_lead_model")]
    pub lead_model: String,
    /// Default model for worker agents (default: sonnet)
    #[serde(default = "default_worker_model")]
    pub default_worker_model: String,
    /// Maximum number of concurrent workers
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_workers: usize,
    /// Default timeout for individual workers
    #[serde(default = "default_worker_timeout", with = "duration_secs")]
    pub default_worker_timeout: Duration,
    /// Overall orchestration timeout
    #[serde(default = "default_orchestration_timeout", with = "duration_secs")]
    pub orchestration_timeout: Duration,
    /// Maximum total budget in USD for all workers
    #[serde(default)]
    pub max_total_budget_usd: Option<f64>,
    /// Whether to synthesize results from all workers using the lead agent
    #[serde(default = "default_synthesize")]
    pub synthesize_results: bool,
    /// Maximum retries for failed workers
    #[serde(default = "default_max_retries")]
    pub max_worker_retries: u32,
}

fn default_lead_model() -> String {
    "claude-opus-4-6".to_string()
}

fn default_worker_model() -> String {
    "claude-sonnet-4-6".to_string()
}

fn default_max_concurrent() -> usize {
    8
}

fn default_worker_timeout() -> Duration {
    Duration::from_secs(300)
}

fn default_orchestration_timeout() -> Duration {
    Duration::from_secs(600)
}

fn default_synthesize() -> bool {
    true
}

fn default_max_retries() -> u32 {
    1
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            lead_model: default_lead_model(),
            default_worker_model: default_worker_model(),
            max_concurrent_workers: default_max_concurrent(),
            default_worker_timeout: default_worker_timeout(),
            orchestration_timeout: default_orchestration_timeout(),
            max_total_budget_usd: Some(5.0),
            synthesize_results: default_synthesize(),
            max_worker_retries: default_max_retries(),
        }
    }
}

/// JSON-friendly worker spec for use in StepAction serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSpecJson {
    /// Human-readable name for the worker
    pub name: String,
    /// Prompt/instructions for the worker
    pub prompt: String,
    /// Agent type
    #[serde(default = "default_agent_type")]
    pub agent_type: String,
    /// Model to use
    #[serde(default)]
    pub model: Option<String>,
    /// Maximum turns
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Allowed tools
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Timeout in milliseconds
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Names of workers this depends on
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Priority
    #[serde(default)]
    pub priority: u32,
}

fn default_agent_type() -> String {
    "general-purpose".to_string()
}

/// Allowlisted model name prefixes that workers may request.
/// Any model not matching one of these prefixes triggers a warning.
const ALLOWED_MODEL_PREFIXES: &[&str] = &[
    "claude-",
    "gpt-",
    "gemini-",
    "deepseek-",
    "qwen-",
    "o1-",
    "o3-",
    "o4-",
];

/// Recognised agent type values.
const ALLOWED_AGENT_TYPES: &[&str] = &["general-purpose", "Explore", "Bash", "Code", "Plan"];

/// Hard-cap on max_turns a worker may request (prevents runaway loops).
const MAX_ALLOWED_TURNS: u32 = 50;

/// Hard-cap on the number of tools a single worker may request.
const MAX_ALLOWED_TOOLS: usize = 30;

/// Hard-cap on the number of dependencies a single worker may declare.
const MAX_ALLOWED_DEPS: usize = 20;

impl WorkerSpecJson {
    /// Validate a worker spec parsed from LLM output.
    ///
    /// Returns a list of human-readable warnings. An empty vec means the spec
    /// is clean. Callers decide whether warnings are fatal or merely logged.
    ///
    /// R1-H12: Prevents LLM injection from escalating privileges via crafted
    /// worker specs (unexpected models, excessive tools, runaway turns, etc.).
    pub fn validate(&self) -> Vec<String> {
        let mut warnings: Vec<String> = Vec::new();

        // --- Model allowlist ---
        if let Some(ref model) = self.model {
            let model_lower = model.to_lowercase();
            let recognised = ALLOWED_MODEL_PREFIXES
                .iter()
                .any(|prefix| model_lower.starts_with(prefix));
            if !recognised {
                warnings.push(format!(
                    "worker '{}': model '{}' is not in the recognised model allowlist",
                    self.name, model
                ));
            }
        }

        // --- Agent type allowlist ---
        if !ALLOWED_AGENT_TYPES.contains(&self.agent_type.as_str()) {
            warnings.push(format!(
                "worker '{}': agent_type '{}' is not a recognised type (allowed: {:?})",
                self.name, self.agent_type, ALLOWED_AGENT_TYPES
            ));
        }

        // --- Max turns cap ---
        if let Some(turns) = self.max_turns {
            if turns > MAX_ALLOWED_TURNS {
                warnings.push(format!(
                    "worker '{}': max_turns {} exceeds cap of {}",
                    self.name, turns, MAX_ALLOWED_TURNS
                ));
            }
        }

        // --- Tool count cap ---
        if let Some(ref tools) = self.allowed_tools {
            if tools.len() > MAX_ALLOWED_TOOLS {
                warnings.push(format!(
                    "worker '{}': requested {} tools, exceeds cap of {}",
                    self.name,
                    tools.len(),
                    MAX_ALLOWED_TOOLS
                ));
            }
        }

        // --- Dependency count cap ---
        if self.depends_on.len() > MAX_ALLOWED_DEPS {
            warnings.push(format!(
                "worker '{}': {} dependencies exceeds cap of {}",
                self.name,
                self.depends_on.len(),
                MAX_ALLOWED_DEPS
            ));
        }

        // --- Name sanity (no control chars, reasonable length) ---
        if self.name.len() > 128 {
            warnings.push(format!(
                "worker '{}…': name length {} exceeds 128-char limit",
                &self.name[..32],
                self.name.len()
            ));
        }
        if self.name.chars().any(|c| c.is_control()) {
            warnings.push(format!(
                "worker '{}': name contains control characters",
                self.name.escape_default()
            ));
        }

        warnings
    }

    /// Validate and clamp fields to safe values.
    ///
    /// Logs warnings via `tracing::warn!` and applies defensive caps so that
    /// even a malicious spec cannot escalate beyond safe bounds.
    pub fn validate_and_clamp(&mut self) {
        let warnings = self.validate();
        for w in &warnings {
            tracing::warn!(target: "orchestrator.security", "{}", w);
        }

        // Clamp max_turns to the hard cap
        if let Some(ref mut turns) = self.max_turns {
            if *turns > MAX_ALLOWED_TURNS {
                tracing::warn!(
                    target: "orchestrator.security",
                    worker = %self.name,
                    requested = *turns,
                    clamped_to = MAX_ALLOWED_TURNS,
                    "R1-H12: clamping max_turns to hard cap"
                );
                *turns = MAX_ALLOWED_TURNS;
            }
        }

        // Truncate excessive tool list
        if let Some(ref mut tools) = self.allowed_tools {
            if tools.len() > MAX_ALLOWED_TOOLS {
                tracing::warn!(
                    target: "orchestrator.security",
                    worker = %self.name,
                    requested = tools.len(),
                    clamped_to = MAX_ALLOWED_TOOLS,
                    "R1-H12: truncating tool list to hard cap"
                );
                tools.truncate(MAX_ALLOWED_TOOLS);
            }
        }

        // Truncate excessive dependencies
        if self.depends_on.len() > MAX_ALLOWED_DEPS {
            tracing::warn!(
                target: "orchestrator.security",
                worker = %self.name,
                requested = self.depends_on.len(),
                clamped_to = MAX_ALLOWED_DEPS,
                "R1-H12: truncating dependency list to hard cap"
            );
            self.depends_on.truncate(MAX_ALLOWED_DEPS);
        }

        // Truncate overly long names
        if self.name.len() > 128 {
            self.name.truncate(128);
        }
    }

    /// Convert to a WorkerSpec, resolving dependency names to IDs
    pub fn to_worker_spec(
        &self,
        name_to_id: &std::collections::HashMap<String, Uuid>,
    ) -> WorkerSpec {
        // R1-M4: Log warning for unresolved dependency names instead of silent drop
        let depends_on: Vec<Uuid> = self
            .depends_on
            .iter()
            .filter_map(|name| {
                match name_to_id.get(name) {
                    Some(id) => Some(*id),
                    None => {
                        tracing::warn!(worker = %self.name, dependency = %name, "Unresolved worker dependency — name not found, skipping");
                        None
                    }
                }
            })
            .collect();

        WorkerSpec {
            id: Uuid::new_v4(),
            name: self.name.clone(),
            prompt: self.prompt.clone(),
            agent_type: self.agent_type.clone(),
            model: self.model.clone(),
            max_turns: self.max_turns,
            allowed_tools: self.allowed_tools.clone(),
            timeout: self.timeout_ms.map(Duration::from_millis),
            depends_on,
            priority: self.priority,
        }
    }
}

/// Serde helper for Duration as seconds
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

/// Serde helper for Option<Duration> as seconds
mod optional_duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match duration {
            Some(d) => serializer.serialize_some(&d.as_secs()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<u64> = Option::deserialize(deserializer)?;
        Ok(opt.map(Duration::from_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_spec_builder() {
        let spec = WorkerSpec::new("analyzer", "Analyze the files")
            .with_agent_type("Explore")
            .with_model("claude-sonnet-4-6")
            .with_max_turns(10)
            .with_timeout(Duration::from_secs(120))
            .with_priority(5);

        assert_eq!(spec.name, "analyzer");
        assert_eq!(spec.agent_type, "Explore");
        assert_eq!(spec.model, Some("claude-sonnet-4-6".to_string()));
        assert_eq!(spec.max_turns, Some(10));
        assert_eq!(spec.timeout, Some(Duration::from_secs(120)));
        assert_eq!(spec.priority, 5);
    }

    #[test]
    fn test_worker_spec_dependencies() {
        let dep_id = Uuid::new_v4();
        let spec = WorkerSpec::new("worker", "Do work").depends_on(dep_id);

        assert_eq!(spec.depends_on.len(), 1);
        assert_eq!(spec.depends_on[0], dep_id);
    }

    #[test]
    fn test_orchestrator_config_defaults() {
        let config = OrchestratorConfig::default();

        assert_eq!(config.max_concurrent_workers, 8);
        assert_eq!(config.default_worker_timeout, Duration::from_secs(300));
        assert_eq!(config.orchestration_timeout, Duration::from_secs(600));
        assert!(config.synthesize_results);
        assert_eq!(config.max_worker_retries, 1);
    }

    #[test]
    fn test_worker_spec_serialization() {
        let spec =
            WorkerSpec::new("test-worker", "Test prompt").with_timeout(Duration::from_secs(60));

        let json = serde_json::to_string(&spec).unwrap();
        let deserialized: WorkerSpec = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "test-worker");
        assert_eq!(deserialized.prompt, "Test prompt");
        assert_eq!(deserialized.timeout, Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_worker_status_variants() {
        let statuses = vec![
            WorkerStatus::Pending,
            WorkerStatus::Running,
            WorkerStatus::Completed,
            WorkerStatus::Failed,
            WorkerStatus::TimedOut,
            WorkerStatus::Cancelled,
        ];

        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let deserialized: WorkerStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, status);
        }
    }

    #[test]
    fn test_worker_spec_json_to_worker_spec() {
        let mut name_to_id = std::collections::HashMap::new();
        let dep_id = Uuid::new_v4();
        name_to_id.insert("dep-worker".to_string(), dep_id);

        let json_spec = WorkerSpecJson {
            name: "my-worker".to_string(),
            prompt: "Do something".to_string(),
            agent_type: "Bash".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            max_turns: Some(5),
            allowed_tools: Some(vec!["Bash".to_string()]),
            timeout_ms: Some(60000),
            depends_on: vec!["dep-worker".to_string()],
            priority: 1,
        };

        let spec = json_spec.to_worker_spec(&name_to_id);
        assert_eq!(spec.name, "my-worker");
        assert_eq!(spec.depends_on.len(), 1);
        assert_eq!(spec.depends_on[0], dep_id);
        assert_eq!(spec.timeout, Some(Duration::from_millis(60000)));
    }

    // ---- R1-H12 validation tests ----

    #[test]
    fn test_validate_clean_spec_no_warnings() {
        let spec = WorkerSpecJson {
            name: "analyzer-1".to_string(),
            prompt: "Analyze files".to_string(),
            agent_type: "Explore".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            max_turns: Some(10),
            allowed_tools: Some(vec!["Read".to_string()]),
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        assert!(spec.validate().is_empty());
    }

    #[test]
    fn test_validate_unrecognised_model() {
        let spec = WorkerSpecJson {
            name: "evil-worker".to_string(),
            prompt: "Do evil".to_string(),
            agent_type: "Explore".to_string(),
            model: Some("malicious-model-v1".to_string()),
            max_turns: None,
            allowed_tools: None,
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        let warnings = spec.validate();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not in the recognised model allowlist"));
    }

    #[test]
    fn test_validate_unrecognised_agent_type() {
        let spec = WorkerSpecJson {
            name: "worker".to_string(),
            prompt: "Work".to_string(),
            agent_type: "SuperAdmin".to_string(),
            model: None,
            max_turns: None,
            allowed_tools: None,
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        let warnings = spec.validate();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not a recognised type"));
    }

    #[test]
    fn test_validate_excessive_max_turns() {
        let spec = WorkerSpecJson {
            name: "worker".to_string(),
            prompt: "Work".to_string(),
            agent_type: "Code".to_string(),
            model: None,
            max_turns: Some(9999),
            allowed_tools: None,
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        let warnings = spec.validate();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("exceeds cap"));
    }

    #[test]
    fn test_validate_excessive_tools() {
        let tools: Vec<String> = (0..35).map(|i| format!("tool-{}", i)).collect();
        let spec = WorkerSpecJson {
            name: "worker".to_string(),
            prompt: "Work".to_string(),
            agent_type: "Bash".to_string(),
            model: None,
            max_turns: None,
            allowed_tools: Some(tools),
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        let warnings = spec.validate();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("exceeds cap of 30"));
    }

    #[test]
    fn test_validate_long_name() {
        let spec = WorkerSpecJson {
            name: "a".repeat(200),
            prompt: "Work".to_string(),
            agent_type: "Code".to_string(),
            model: None,
            max_turns: None,
            allowed_tools: None,
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        let warnings = spec.validate();
        assert!(warnings.iter().any(|w| w.contains("128-char limit")));
    }

    #[test]
    fn test_validate_and_clamp_caps_turns() {
        let mut spec = WorkerSpecJson {
            name: "worker".to_string(),
            prompt: "Work".to_string(),
            agent_type: "Code".to_string(),
            model: None,
            max_turns: Some(200),
            allowed_tools: None,
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        spec.validate_and_clamp();
        assert_eq!(spec.max_turns, Some(MAX_ALLOWED_TURNS));
    }

    #[test]
    fn test_validate_and_clamp_truncates_tools() {
        let tools: Vec<String> = (0..40).map(|i| format!("tool-{}", i)).collect();
        let mut spec = WorkerSpecJson {
            name: "worker".to_string(),
            prompt: "Work".to_string(),
            agent_type: "Code".to_string(),
            model: None,
            max_turns: None,
            allowed_tools: Some(tools),
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        spec.validate_and_clamp();
        assert_eq!(
            spec.allowed_tools.as_ref().unwrap().len(),
            MAX_ALLOWED_TOOLS
        );
    }

    #[test]
    fn test_validate_and_clamp_truncates_name() {
        let mut spec = WorkerSpecJson {
            name: "x".repeat(256),
            prompt: "Work".to_string(),
            agent_type: "Code".to_string(),
            model: None,
            max_turns: None,
            allowed_tools: None,
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        spec.validate_and_clamp();
        assert_eq!(spec.name.len(), 128);
    }

    #[test]
    fn test_validate_multiple_issues() {
        let spec = WorkerSpecJson {
            name: "a".repeat(200),
            prompt: "Work".to_string(),
            agent_type: "HackerMode".to_string(),
            model: Some("evil-llm".to_string()),
            max_turns: Some(500),
            allowed_tools: None,
            timeout_ms: None,
            depends_on: vec![],
            priority: 0,
        };
        let warnings = spec.validate();
        // Should flag: model, agent_type, max_turns, name length = 4 warnings
        assert_eq!(warnings.len(), 4);
    }
}
