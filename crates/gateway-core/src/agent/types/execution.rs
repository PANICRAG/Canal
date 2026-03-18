//! Execution Strategy Types
//!
//! This module defines execution strategies for agent tool calls,
//! implementing the hybrid Manus + Claude Code pattern for optimal
//! parallel/serial execution decisions.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

// ============================================================================
// Execution Strategy
// ============================================================================

/// Execution strategy for agent tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionStrategy {
    /// Simple tasks - parallel execution (Claude Code style)
    /// Best for read operations and independent writes
    Parallel {
        /// Maximum concurrent tool calls
        #[serde(default = "default_max_concurrent")]
        max_concurrent: usize,
    },

    /// Complex/sensitive tasks - serial with verification (Manus style)
    /// Best for destructive operations and dependent chains
    Serial {
        /// Whether to verify each step before proceeding
        #[serde(default)]
        verify_each_step: bool,
    },

    /// Auto-select based on task analysis (Recommended)
    /// Combines parallel for safe ops with serial for sensitive ones
    Hybrid {
        /// Task complexity threshold for serial execution (0.0-1.0)
        #[serde(default = "default_parallel_threshold")]
        parallel_threshold: f32,
        /// Tools that always require serial execution
        #[serde(default)]
        sensitive_tools: Vec<String>,
        /// Maximum concurrent parallel calls
        #[serde(default = "default_max_concurrent")]
        max_concurrent: usize,
    },
}

fn default_max_concurrent() -> usize {
    5
}

fn default_parallel_threshold() -> f32 {
    0.3 // Below 0.3 complexity = parallel, above = serial
}

impl Default for ExecutionStrategy {
    fn default() -> Self {
        Self::Hybrid {
            parallel_threshold: default_parallel_threshold(),
            sensitive_tools: vec![
                "Write".to_string(),
                "Edit".to_string(),
                "Bash".to_string(),
                "file.delete".to_string(),
                "creative.render".to_string(),
            ],
            max_concurrent: default_max_concurrent(),
        }
    }
}

impl ExecutionStrategy {
    /// Create a parallel-first strategy
    pub fn parallel(max_concurrent: usize) -> Self {
        Self::Parallel { max_concurrent }
    }

    /// Create a serial-only strategy
    pub fn serial(verify_each_step: bool) -> Self {
        Self::Serial { verify_each_step }
    }

    /// Create a hybrid strategy with custom settings
    pub fn hybrid(parallel_threshold: f32, sensitive_tools: Vec<String>) -> Self {
        Self::Hybrid {
            parallel_threshold,
            sensitive_tools,
            max_concurrent: default_max_concurrent(),
        }
    }

    /// Check if a tool can be executed in parallel given current context
    pub fn can_parallelize(&self, tool_name: &str, tool_category: &ToolCategory) -> bool {
        match self {
            Self::Parallel { .. } => tool_category.is_safe_parallel(),
            Self::Serial { .. } => false,
            Self::Hybrid {
                sensitive_tools, ..
            } => {
                // Check if tool is in sensitive list
                if sensitive_tools.iter().any(|t| t == tool_name) {
                    return false;
                }
                // Use category for other tools
                tool_category.is_safe_parallel()
            }
        }
    }

    /// Get the maximum concurrent executions allowed
    pub fn max_concurrent(&self) -> usize {
        match self {
            Self::Parallel { max_concurrent } => *max_concurrent,
            Self::Serial { .. } => 1,
            Self::Hybrid { max_concurrent, .. } => *max_concurrent,
        }
    }
}

// ============================================================================
// Tool Category
// ============================================================================

/// Tool category for execution planning
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// Safe, idempotent operations - can run in parallel
    ReadOnly,

    /// State-changing but reversible operations
    Reversible,

    /// Destructive or expensive operations - serial only
    Sensitive,

    /// External service calls - rate limited
    External,

    /// Unknown category - defaults to serial for safety
    Unknown,
}

impl ToolCategory {
    /// Check if this category is safe for parallel execution
    pub fn is_safe_parallel(&self) -> bool {
        matches!(self, Self::ReadOnly | Self::Reversible)
    }

    /// Check if this category requires user confirmation
    pub fn requires_confirmation(&self) -> bool {
        matches!(self, Self::Sensitive)
    }

    /// Check if this category should create a checkpoint
    pub fn creates_checkpoint(&self) -> bool {
        matches!(self, Self::Reversible | Self::Sensitive)
    }

    /// Check if this category has rate limiting
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::External)
    }

    /// Get the category from a tool name
    pub fn from_tool_name(tool_name: &str) -> Self {
        // Read-only tools
        if matches!(
            tool_name,
            "Read" | "Glob" | "Grep" | "LSP" | "browser.snapshot" | "file.read" | "search"
        ) {
            return Self::ReadOnly;
        }

        // Reversible tools
        if matches!(
            tool_name,
            "Write" | "Edit" | "browser.navigate" | "browser.click" | "browser.fill"
        ) {
            return Self::Reversible;
        }

        // Sensitive tools
        if tool_name.starts_with("file.delete")
            || tool_name.starts_with("creative.render")
            || tool_name.starts_with("external.publish")
            || tool_name == "Bash"
        {
            return Self::Sensitive;
        }

        // External tools
        if tool_name.starts_with("ai.") || tool_name.starts_with("storage.") {
            return Self::External;
        }

        Self::Unknown
    }
}

impl Default for ToolCategory {
    fn default() -> Self {
        Self::Unknown
    }
}

// ============================================================================
// Tool Execution Hints
// ============================================================================

/// Execution hints for a tool - guides the execution strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionHints {
    /// Tool category
    #[serde(default)]
    pub category: ToolCategory,

    /// Is this tool safe for parallel execution?
    #[serde(default)]
    pub safe_parallel: bool,

    /// Does this tool require user confirmation?
    #[serde(default)]
    pub requires_confirmation: bool,

    /// Should a checkpoint be created before execution?
    #[serde(default)]
    pub creates_checkpoint: bool,

    /// Estimated execution duration in milliseconds
    #[serde(default)]
    pub estimated_duration_ms: Option<u64>,

    /// Tool dependencies (must complete before this tool)
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Mutex tools (cannot run simultaneously)
    #[serde(default)]
    pub mutex_with: Vec<String>,

    /// Retry policy
    #[serde(default)]
    pub retry_policy: RetryPolicy,

    /// Rollback handler name (if applicable)
    #[serde(default)]
    pub rollback_handler: Option<String>,

    /// Rate limit (requests per minute)
    #[serde(default)]
    pub rate_limit_rpm: Option<u32>,
}

impl Default for ToolExecutionHints {
    fn default() -> Self {
        Self {
            category: ToolCategory::Unknown,
            safe_parallel: false,
            requires_confirmation: false,
            creates_checkpoint: false,
            estimated_duration_ms: None,
            depends_on: Vec::new(),
            mutex_with: Vec::new(),
            retry_policy: RetryPolicy::default(),
            rollback_handler: None,
            rate_limit_rpm: None,
        }
    }
}

impl ToolExecutionHints {
    /// Create hints for a read-only tool
    pub fn read_only() -> Self {
        Self {
            category: ToolCategory::ReadOnly,
            safe_parallel: true,
            ..Default::default()
        }
    }

    /// Create hints for a reversible tool
    pub fn reversible() -> Self {
        Self {
            category: ToolCategory::Reversible,
            safe_parallel: true,
            creates_checkpoint: true,
            ..Default::default()
        }
    }

    /// Create hints for a sensitive tool
    pub fn sensitive() -> Self {
        Self {
            category: ToolCategory::Sensitive,
            safe_parallel: false,
            requires_confirmation: true,
            creates_checkpoint: true,
            ..Default::default()
        }
    }

    /// Create hints for an external tool
    pub fn external(rate_limit_rpm: u32) -> Self {
        Self {
            category: ToolCategory::External,
            safe_parallel: false,
            rate_limit_rpm: Some(rate_limit_rpm),
            retry_policy: RetryPolicy::exponential(3, Duration::from_secs(1)),
            ..Default::default()
        }
    }

    /// Set dependencies
    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.depends_on = deps;
        self
    }

    /// Set mutex tools
    pub fn with_mutex(mut self, mutex: Vec<String>) -> Self {
        self.mutex_with = mutex;
        self
    }

    /// Set rollback handler
    pub fn with_rollback(mut self, handler: impl Into<String>) -> Self {
        self.rollback_handler = Some(handler.into());
        self
    }
}

// ============================================================================
// Retry Policy
// ============================================================================

/// Retry policy for tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RetryPolicy {
    /// No retries
    None,

    /// Fixed delay between retries
    Fixed {
        max_retries: u32,
        /// Delay in milliseconds
        delay_ms: u64,
    },

    /// Exponential backoff
    Exponential {
        max_retries: u32,
        /// Initial delay in milliseconds
        initial_delay_ms: u64,
        /// Maximum delay in milliseconds
        #[serde(default = "default_max_delay_ms")]
        max_delay_ms: u64,
        #[serde(default = "default_multiplier")]
        multiplier: f64,
    },
}

fn default_max_delay_ms() -> u64 {
    30_000 // 30 seconds
}

fn default_multiplier() -> f64 {
    2.0
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::Exponential {
            max_retries: 3,
            initial_delay_ms: 1_000, // 1 second
            max_delay_ms: default_max_delay_ms(),
            multiplier: default_multiplier(),
        }
    }
}

impl RetryPolicy {
    /// Create a no-retry policy
    pub fn none() -> Self {
        Self::None
    }

    /// Create a fixed delay retry policy
    pub fn fixed(max_retries: u32, delay: Duration) -> Self {
        Self::Fixed {
            max_retries,
            delay_ms: delay.as_millis() as u64,
        }
    }

    /// Create an exponential backoff retry policy
    pub fn exponential(max_retries: u32, initial_delay: Duration) -> Self {
        Self::Exponential {
            max_retries,
            initial_delay_ms: initial_delay.as_millis() as u64,
            max_delay_ms: default_max_delay_ms(),
            multiplier: default_multiplier(),
        }
    }

    /// Get the delay for a given retry attempt
    pub fn get_delay(&self, attempt: u32) -> Option<Duration> {
        match self {
            Self::None => None,
            Self::Fixed {
                max_retries,
                delay_ms,
            } => {
                if attempt < *max_retries {
                    Some(Duration::from_millis(*delay_ms))
                } else {
                    None
                }
            }
            Self::Exponential {
                max_retries,
                initial_delay_ms,
                max_delay_ms,
                multiplier,
            } => {
                if attempt < *max_retries {
                    let delay_ms_calculated =
                        (*initial_delay_ms as f64) * multiplier.powi(attempt as i32);
                    let delay =
                        Duration::from_millis(delay_ms_calculated.min(*max_delay_ms as f64) as u64);
                    Some(delay)
                } else {
                    None
                }
            }
        }
    }

    /// Get maximum number of retries
    pub fn max_retries(&self) -> u32 {
        match self {
            Self::None => 0,
            Self::Fixed { max_retries, .. } => *max_retries,
            Self::Exponential { max_retries, .. } => *max_retries,
        }
    }
}

// ============================================================================
// Execution Plan
// ============================================================================

/// A planned execution of multiple tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// Ordered steps in the execution plan
    pub steps: Vec<ExecutionStep>,
    /// Total estimated duration in milliseconds
    #[serde(default)]
    pub estimated_duration_ms: Option<u64>,
    /// Whether the entire plan requires confirmation
    #[serde(default)]
    pub requires_confirmation: bool,
}

impl ExecutionPlan {
    /// Create a new empty execution plan
    pub fn new() -> Self {
        Self {
            steps: Vec::new(),
            estimated_duration_ms: None,
            requires_confirmation: false,
        }
    }

    /// Add a step to the plan
    pub fn add_step(&mut self, step: ExecutionStep) {
        if let ExecutionStep::Single(ref call) = step {
            if call.requires_confirmation {
                self.requires_confirmation = true;
            }
        }
        self.steps.push(step);
    }

    /// Check if the plan has any steps
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Get the number of steps
    pub fn len(&self) -> usize {
        self.steps.len()
    }
}

impl Default for ExecutionPlan {
    fn default() -> Self {
        Self::new()
    }
}

/// A single step in an execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionStep {
    /// A single tool call
    Single(PlannedToolCall),
    /// Multiple tool calls that can run in parallel
    Parallel(Vec<PlannedToolCall>),
}

/// A planned tool call with execution metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedToolCall {
    /// Tool name
    pub tool: String,
    /// Tool parameters
    pub params: serde_json::Value,
    /// Target ID (for conflict detection)
    #[serde(default)]
    pub target_id: Option<String>,
    /// Whether this call requires confirmation
    #[serde(default)]
    pub requires_confirmation: bool,
    /// Whether to create a checkpoint before execution
    #[serde(default)]
    pub creates_checkpoint: bool,
    /// Execution hints
    #[serde(default)]
    pub hints: ToolExecutionHints,
}

impl PlannedToolCall {
    /// Create a new planned tool call
    pub fn new(tool: impl Into<String>, params: serde_json::Value) -> Self {
        let tool = tool.into();
        let category = ToolCategory::from_tool_name(&tool);
        Self {
            tool,
            params,
            target_id: None,
            requires_confirmation: category.requires_confirmation(),
            creates_checkpoint: category.creates_checkpoint(),
            hints: ToolExecutionHints::default(),
        }
    }

    /// Set the target ID
    pub fn with_target(mut self, target_id: impl Into<String>) -> Self {
        self.target_id = Some(target_id.into());
        self
    }

    /// Set confirmation requirement
    pub fn with_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    /// Set checkpoint requirement
    pub fn with_checkpoint(mut self, creates: bool) -> Self {
        self.creates_checkpoint = creates;
        self
    }

    /// Set execution hints
    pub fn with_hints(mut self, hints: ToolExecutionHints) -> Self {
        self.hints = hints;
        self
    }
}

// ============================================================================
// Execution Plan Builder
// ============================================================================

/// Builder for creating execution plans
pub struct ExecutionPlanBuilder {
    pending_parallel: Vec<PlannedToolCall>,
    steps: Vec<ExecutionStep>,
    strategy: ExecutionStrategy,
    seen_targets: HashSet<String>,
}

impl ExecutionPlanBuilder {
    /// Create a new execution plan builder
    pub fn new(strategy: ExecutionStrategy) -> Self {
        Self {
            pending_parallel: Vec::new(),
            steps: Vec::new(),
            strategy,
            seen_targets: HashSet::new(),
        }
    }

    /// Add a tool call to the plan
    pub fn add_call(&mut self, call: PlannedToolCall) {
        let category = ToolCategory::from_tool_name(&call.tool);

        // Check for target conflicts
        let has_conflict = call
            .target_id
            .as_ref()
            .map(|t| self.seen_targets.contains(t))
            .unwrap_or(false);

        // Check if can parallelize
        let can_parallel = self.strategy.can_parallelize(&call.tool, &category) && !has_conflict;

        if can_parallel {
            // Track target
            if let Some(ref target) = call.target_id {
                self.seen_targets.insert(target.clone());
            }
            self.pending_parallel.push(call);
        } else {
            // Flush pending parallel calls first
            self.flush_parallel();

            // Add as single step
            if let Some(ref target) = call.target_id {
                self.seen_targets.insert(target.clone());
            }
            self.steps.push(ExecutionStep::Single(call));
        }
    }

    /// Flush pending parallel calls into a parallel step
    fn flush_parallel(&mut self) {
        if self.pending_parallel.is_empty() {
            return;
        }

        let calls = std::mem::take(&mut self.pending_parallel);

        if calls.len() == 1 {
            self.steps
                .push(ExecutionStep::Single(calls.into_iter().next().unwrap()));
        } else {
            // Limit to max concurrent
            let max = self.strategy.max_concurrent();
            if calls.len() <= max {
                self.steps.push(ExecutionStep::Parallel(calls));
            } else {
                // Split into batches
                for chunk in calls.chunks(max) {
                    if chunk.len() == 1 {
                        self.steps.push(ExecutionStep::Single(chunk[0].clone()));
                    } else {
                        self.steps.push(ExecutionStep::Parallel(chunk.to_vec()));
                    }
                }
            }
        }

        self.seen_targets.clear();
    }

    /// Build the execution plan
    pub fn build(mut self) -> ExecutionPlan {
        self.flush_parallel();

        let requires_confirmation = self.steps.iter().any(|step| match step {
            ExecutionStep::Single(call) => call.requires_confirmation,
            ExecutionStep::Parallel(calls) => calls.iter().any(|c| c.requires_confirmation),
        });

        ExecutionPlan {
            steps: self.steps,
            estimated_duration_ms: None,
            requires_confirmation,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_strategy_default() {
        let strategy = ExecutionStrategy::default();
        assert!(matches!(strategy, ExecutionStrategy::Hybrid { .. }));
    }

    #[test]
    fn test_tool_category_from_name() {
        assert_eq!(ToolCategory::from_tool_name("Read"), ToolCategory::ReadOnly);
        assert_eq!(ToolCategory::from_tool_name("Glob"), ToolCategory::ReadOnly);
        assert_eq!(
            ToolCategory::from_tool_name("Write"),
            ToolCategory::Reversible
        );
        assert_eq!(
            ToolCategory::from_tool_name("Bash"),
            ToolCategory::Sensitive
        );
        assert_eq!(
            ToolCategory::from_tool_name("ai.generate"),
            ToolCategory::External
        );
        assert_eq!(
            ToolCategory::from_tool_name("unknown"),
            ToolCategory::Unknown
        );
    }

    #[test]
    fn test_tool_category_safe_parallel() {
        assert!(ToolCategory::ReadOnly.is_safe_parallel());
        assert!(ToolCategory::Reversible.is_safe_parallel());
        assert!(!ToolCategory::Sensitive.is_safe_parallel());
        assert!(!ToolCategory::External.is_safe_parallel());
    }

    #[test]
    fn test_retry_policy_exponential() {
        let policy = RetryPolicy::exponential(3, Duration::from_secs(1));

        // First retry: 1 second
        assert_eq!(policy.get_delay(0), Some(Duration::from_secs(1)));
        // Second retry: 2 seconds
        assert_eq!(policy.get_delay(1), Some(Duration::from_secs(2)));
        // Third retry: 4 seconds
        assert_eq!(policy.get_delay(2), Some(Duration::from_secs(4)));
        // No more retries
        assert_eq!(policy.get_delay(3), None);
    }

    #[test]
    fn test_execution_plan_builder_parallel() {
        let strategy = ExecutionStrategy::parallel(5);
        let mut builder = ExecutionPlanBuilder::new(strategy);

        // Add read-only calls
        builder.add_call(PlannedToolCall::new(
            "Read",
            serde_json::json!({"path": "/a"}),
        ));
        builder.add_call(PlannedToolCall::new(
            "Read",
            serde_json::json!({"path": "/b"}),
        ));
        builder.add_call(PlannedToolCall::new(
            "Glob",
            serde_json::json!({"pattern": "*.rs"}),
        ));

        let plan = builder.build();

        // Should be one parallel step with 3 calls
        assert_eq!(plan.steps.len(), 1);
        match &plan.steps[0] {
            ExecutionStep::Parallel(calls) => assert_eq!(calls.len(), 3),
            _ => panic!("Expected parallel step"),
        }
    }

    #[test]
    fn test_execution_plan_builder_serial() {
        let strategy = ExecutionStrategy::serial(true);
        let mut builder = ExecutionPlanBuilder::new(strategy);

        builder.add_call(PlannedToolCall::new("Read", serde_json::json!({})));
        builder.add_call(PlannedToolCall::new("Write", serde_json::json!({})));

        let plan = builder.build();

        // Should be two single steps
        assert_eq!(plan.steps.len(), 2);
        assert!(matches!(plan.steps[0], ExecutionStep::Single(_)));
        assert!(matches!(plan.steps[1], ExecutionStep::Single(_)));
    }

    #[test]
    fn test_execution_plan_builder_hybrid() {
        let strategy = ExecutionStrategy::default();
        let mut builder = ExecutionPlanBuilder::new(strategy);

        // Read calls can be parallel
        builder.add_call(PlannedToolCall::new(
            "Read",
            serde_json::json!({"path": "/a"}),
        ));
        builder.add_call(PlannedToolCall::new(
            "Read",
            serde_json::json!({"path": "/b"}),
        ));
        // Bash must be serial
        builder.add_call(PlannedToolCall::new(
            "Bash",
            serde_json::json!({"cmd": "ls"}),
        ));
        // More reads
        builder.add_call(PlannedToolCall::new(
            "Read",
            serde_json::json!({"path": "/c"}),
        ));

        let plan = builder.build();

        // Should be: parallel(2 reads), single(bash), single(read)
        assert_eq!(plan.steps.len(), 3);
    }

    #[test]
    fn test_execution_plan_builder_target_conflict() {
        let strategy = ExecutionStrategy::parallel(5);
        let mut builder = ExecutionPlanBuilder::new(strategy);

        // Same target - cannot parallelize
        builder.add_call(PlannedToolCall::new("Read", serde_json::json!({})).with_target("file1"));
        builder.add_call(PlannedToolCall::new("Write", serde_json::json!({})).with_target("file1"));

        let plan = builder.build();

        // Should be two steps due to target conflict
        assert_eq!(plan.steps.len(), 2);
    }

    #[test]
    fn test_tool_execution_hints() {
        let hints = ToolExecutionHints::sensitive()
            .with_dependencies(vec!["setup".to_string()])
            .with_rollback("rollback_handler");

        assert_eq!(hints.category, ToolCategory::Sensitive);
        assert!(hints.requires_confirmation);
        assert!(hints.creates_checkpoint);
        assert_eq!(hints.depends_on, vec!["setup"]);
        assert_eq!(hints.rollback_handler, Some("rollback_handler".to_string()));
    }
}
