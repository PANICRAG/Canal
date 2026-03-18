//! Agent Automatic Mode Selector
//!
//! This module provides intelligent automatic mode selection for the agent,
//! deciding whether to execute tasks locally or in the cloud based on:
//!
//! - Task type and resource requirements analysis
//! - Local and cloud capability evaluation
//! - Cost and performance trade-offs
//! - User preference configuration
//! - Decision logging for auditability
//!
//! # Architecture
//!
//! ```text
//! AutoModeSelector
//!        |
//!        |-- TaskAnalyzer ---------> Analyze task characteristics
//!        |
//!        |-- CapabilityChecker ----> Check local/cloud capabilities
//!        |
//!        |-- CostEstimator --------> Estimate execution costs
//!        |
//!        |-- DecisionLogger -------> Log decision reasoning
//!        |
//!        |-- UserPreferences ------> Apply user overrides
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::agent::mode_selector::{
//!     AutoModeSelector, ModeSelectionRequest, UserPreferences, OptimizationGoal,
//! };
//!
//! let selector = AutoModeSelector::builder()
//!     .local_capabilities(local_caps)
//!     .cloud_capabilities(cloud_caps)
//!     .user_preferences(prefs)
//!     .build();
//!
//! let request = ModeSelectionRequest::new(task);
//! let decision = selector.select_mode(request).await?;
//!
//! match decision.selected_mode {
//!     ExecutionMode::Local => { /* execute locally */ }
//!     ExecutionMode::Cloud => { /* execute in cloud */ }
//!     ExecutionMode::Hybrid => { /* split execution */ }
//! }
//! ```

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument};
use uuid::Uuid;

use crate::error::{Error, Result};

// ============================================================================
// Core Types
// ============================================================================

/// Execution mode for agent tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Execute locally (Docker containers, local resources)
    #[default]
    Local,
    /// Execute in the cloud (VMs, cloud resources)
    Cloud,
    /// Hybrid execution (split between local and cloud)
    Hybrid,
    /// Auto-select based on analysis
    Auto,
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::Local => write!(f, "local"),
            ExecutionMode::Cloud => write!(f, "cloud"),
            ExecutionMode::Hybrid => write!(f, "hybrid"),
            ExecutionMode::Auto => write!(f, "auto"),
        }
    }
}

/// Task complexity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TaskComplexity {
    /// Simple task, minimal resources
    Simple,
    /// Moderate task, standard resources
    Moderate,
    /// Complex task, high resources
    Complex,
    /// Intensive task, maximum resources
    Intensive,
}

impl Default for TaskComplexity {
    fn default() -> Self {
        TaskComplexity::Simple
    }
}

impl TaskComplexity {
    /// Get the weight for this complexity level (1-4)
    pub fn weight(&self) -> u32 {
        match self {
            TaskComplexity::Simple => 1,
            TaskComplexity::Moderate => 2,
            TaskComplexity::Complex => 3,
            TaskComplexity::Intensive => 4,
        }
    }
}

/// Type of task to execute
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCategory {
    /// Code execution (Python, JavaScript, etc.)
    CodeExecution,
    /// Browser automation
    BrowserAutomation,
    /// File operations
    FileOperation,
    /// Network operations
    NetworkOperation,
    /// Machine learning / AI inference
    MachineLearning,
    /// Data processing
    DataProcessing,
    /// Build / compilation
    Build,
    /// Testing
    Testing,
    /// Generic task
    Generic,
}

impl Default for TaskCategory {
    fn default() -> Self {
        TaskCategory::Generic
    }
}

/// Optimization goal for mode selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OptimizationGoal {
    /// Minimize execution cost
    Cost,
    /// Minimize execution time (performance)
    Performance,
    /// Balance between cost and performance
    #[default]
    Balanced,
    /// Prefer reliability and availability
    Reliability,
    /// Prefer local execution when possible
    LocalFirst,
    /// Prefer cloud execution when possible
    CloudFirst,
}

impl std::fmt::Display for OptimizationGoal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptimizationGoal::Cost => write!(f, "cost"),
            OptimizationGoal::Performance => write!(f, "performance"),
            OptimizationGoal::Balanced => write!(f, "balanced"),
            OptimizationGoal::Reliability => write!(f, "reliability"),
            OptimizationGoal::LocalFirst => write!(f, "local_first"),
            OptimizationGoal::CloudFirst => write!(f, "cloud_first"),
        }
    }
}

// ============================================================================
// Task Analysis
// ============================================================================

/// Resource requirements for a task
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceRequirements {
    /// Required CPU cores
    pub cpu_cores: f64,
    /// Required memory in MB
    pub memory_mb: u64,
    /// Required disk space in MB
    pub disk_mb: u64,
    /// Required GPU (true if GPU needed)
    pub gpu_required: bool,
    /// Estimated execution time in milliseconds
    pub estimated_time_ms: u64,
    /// Network bandwidth requirements (MB/s)
    pub network_bandwidth_mbps: f64,
    /// Whether task requires persistent storage
    pub persistent_storage: bool,
    /// Whether task requires specific software/tools
    pub required_tools: Vec<String>,
}

impl ResourceRequirements {
    /// Create new resource requirements
    pub fn new() -> Self {
        Self::default()
    }

    /// Set CPU requirements
    pub fn with_cpu(mut self, cores: f64) -> Self {
        self.cpu_cores = cores;
        self
    }

    /// Set memory requirements
    pub fn with_memory(mut self, memory_mb: u64) -> Self {
        self.memory_mb = memory_mb;
        self
    }

    /// Set disk requirements
    pub fn with_disk(mut self, disk_mb: u64) -> Self {
        self.disk_mb = disk_mb;
        self
    }

    /// Set GPU requirement
    pub fn with_gpu(mut self, required: bool) -> Self {
        self.gpu_required = required;
        self
    }

    /// Set estimated time
    pub fn with_time(mut self, time_ms: u64) -> Self {
        self.estimated_time_ms = time_ms;
        self
    }

    /// Add a required tool
    pub fn with_tool(mut self, tool: impl Into<String>) -> Self {
        self.required_tools.push(tool.into());
        self
    }

    /// Calculate a complexity score based on resources
    pub fn complexity_score(&self) -> f64 {
        let cpu_score = self.cpu_cores * 10.0;
        let memory_score = (self.memory_mb as f64) / 100.0;
        let time_score = (self.estimated_time_ms as f64) / 1000.0;
        let gpu_score = if self.gpu_required { 50.0 } else { 0.0 };

        cpu_score + memory_score + time_score + gpu_score
    }
}

/// Task information for analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    /// Task ID
    pub id: String,
    /// Task category
    pub category: TaskCategory,
    /// Task description
    pub description: String,
    /// Code to execute (if applicable)
    pub code: Option<String>,
    /// Programming language (if applicable)
    pub language: Option<String>,
    /// Resource requirements (if known)
    pub requirements: Option<ResourceRequirements>,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

impl TaskInfo {
    /// Create a new task info
    pub fn new(category: TaskCategory, description: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            category,
            description: description.into(),
            code: None,
            language: None,
            requirements: None,
            metadata: HashMap::new(),
        }
    }

    /// Set code
    pub fn with_code(mut self, code: impl Into<String>, language: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self.language = Some(language.into());
        self
    }

    /// Set requirements
    pub fn with_requirements(mut self, requirements: ResourceRequirements) -> Self {
        self.requirements = Some(requirements);
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// Analysis result from TaskAnalyzer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAnalysis {
    /// Task ID
    pub task_id: String,
    /// Detected task category
    pub category: TaskCategory,
    /// Assessed complexity level
    pub complexity: TaskComplexity,
    /// Estimated resource requirements
    pub requirements: ResourceRequirements,
    /// Complexity score (0-100)
    pub complexity_score: f64,
    /// Suggested execution mode
    pub suggested_mode: ExecutionMode,
    /// Confidence in the analysis (0-1)
    pub confidence: f64,
    /// Analysis notes
    pub notes: Vec<String>,
}

/// Trait for task analysis
#[async_trait]
pub trait TaskAnalyzer: Send + Sync {
    /// Analyze a task and determine its characteristics
    async fn analyze(&self, task: &TaskInfo) -> Result<TaskAnalysis>;

    /// Estimate resource requirements for a task
    async fn estimate_resources(&self, task: &TaskInfo) -> Result<ResourceRequirements>;

    /// Detect task complexity
    fn detect_complexity(&self, requirements: &ResourceRequirements) -> TaskComplexity;
}

/// Default task analyzer implementation
pub struct DefaultTaskAnalyzer {
    /// Thresholds for complexity detection
    complexity_thresholds: ComplexityThresholds,
}

/// Thresholds for determining task complexity
#[derive(Debug, Clone)]
pub struct ComplexityThresholds {
    /// CPU threshold for moderate complexity
    pub moderate_cpu: f64,
    /// CPU threshold for complex tasks
    pub complex_cpu: f64,
    /// CPU threshold for intensive tasks
    pub intensive_cpu: f64,
    /// Memory threshold for moderate complexity (MB)
    pub moderate_memory_mb: u64,
    /// Memory threshold for complex tasks (MB)
    pub complex_memory_mb: u64,
    /// Memory threshold for intensive tasks (MB)
    pub intensive_memory_mb: u64,
    /// Time threshold for moderate complexity (ms)
    pub moderate_time_ms: u64,
    /// Time threshold for complex tasks (ms)
    pub complex_time_ms: u64,
    /// Time threshold for intensive tasks (ms)
    pub intensive_time_ms: u64,
}

impl Default for ComplexityThresholds {
    fn default() -> Self {
        Self {
            moderate_cpu: 1.0,
            complex_cpu: 2.0,
            intensive_cpu: 4.0,
            moderate_memory_mb: 512,
            complex_memory_mb: 2048,
            intensive_memory_mb: 8192,
            moderate_time_ms: 5000,
            complex_time_ms: 30000,
            intensive_time_ms: 120000,
        }
    }
}

impl DefaultTaskAnalyzer {
    /// Create a new default task analyzer
    pub fn new() -> Self {
        Self {
            complexity_thresholds: ComplexityThresholds::default(),
        }
    }

    /// Create with custom thresholds
    pub fn with_thresholds(thresholds: ComplexityThresholds) -> Self {
        Self {
            complexity_thresholds: thresholds,
        }
    }

    /// Analyze code to estimate resource requirements
    fn analyze_code(&self, code: &str, language: &str) -> ResourceRequirements {
        let lines = code.lines().count();
        let chars = code.len();

        // Base estimates based on code size and language
        let (base_cpu, base_memory, base_time) = match language {
            "python" => (0.5, 256, 1000),
            "javascript" | "typescript" => (0.3, 128, 500),
            "rust" => (2.0, 1024, 30000), // Compilation is resource-intensive
            "go" => (1.0, 512, 5000),
            "java" => (1.0, 512, 10000),
            _ => (0.5, 256, 2000),
        };

        // Scale based on code size
        let size_factor = 1.0 + (lines as f64 / 1000.0).min(2.0);

        // Check for resource-intensive patterns
        let mut gpu_required = false;
        let mut cpu_multiplier: f64 = 1.0;
        let mut memory_multiplier: f64 = 1.0;

        let code_lower = code.to_lowercase();

        // ML/AI patterns
        if code_lower.contains("tensorflow")
            || code_lower.contains("pytorch")
            || code_lower.contains("torch")
            || code_lower.contains("cuda")
        {
            gpu_required = true;
            memory_multiplier = 4.0;
            cpu_multiplier = 2.0;
        }

        // Data processing patterns
        if code_lower.contains("pandas")
            || code_lower.contains("numpy")
            || code_lower.contains("dataframe")
        {
            memory_multiplier = memory_multiplier.max(2.0);
        }

        // Parallel processing patterns
        if code_lower.contains("multiprocessing")
            || code_lower.contains("threading")
            || code_lower.contains("async")
            || code_lower.contains("tokio")
        {
            cpu_multiplier = cpu_multiplier.max(2.0);
        }

        ResourceRequirements {
            cpu_cores: base_cpu * size_factor * cpu_multiplier,
            memory_mb: (base_memory as f64 * size_factor * memory_multiplier) as u64,
            disk_mb: (chars / 1000) as u64 + 100, // Estimate some disk space
            gpu_required,
            estimated_time_ms: (base_time as f64 * size_factor) as u64,
            network_bandwidth_mbps: 0.0,
            persistent_storage: false,
            required_tools: vec![],
        }
    }

    /// Estimate requirements based on task category
    fn estimate_by_category(&self, category: &TaskCategory) -> ResourceRequirements {
        match category {
            TaskCategory::CodeExecution => ResourceRequirements {
                cpu_cores: 1.0,
                memory_mb: 512,
                estimated_time_ms: 5000,
                ..Default::default()
            },
            TaskCategory::BrowserAutomation => ResourceRequirements {
                cpu_cores: 1.0,
                memory_mb: 1024,
                estimated_time_ms: 10000,
                network_bandwidth_mbps: 10.0,
                ..Default::default()
            },
            TaskCategory::FileOperation => ResourceRequirements {
                cpu_cores: 0.5,
                memory_mb: 256,
                estimated_time_ms: 2000,
                ..Default::default()
            },
            TaskCategory::NetworkOperation => ResourceRequirements {
                cpu_cores: 0.5,
                memory_mb: 256,
                estimated_time_ms: 5000,
                network_bandwidth_mbps: 50.0,
                ..Default::default()
            },
            TaskCategory::MachineLearning => ResourceRequirements {
                cpu_cores: 4.0,
                memory_mb: 8192,
                gpu_required: true,
                estimated_time_ms: 60000,
                ..Default::default()
            },
            TaskCategory::DataProcessing => ResourceRequirements {
                cpu_cores: 2.0,
                memory_mb: 4096,
                estimated_time_ms: 30000,
                ..Default::default()
            },
            TaskCategory::Build => ResourceRequirements {
                cpu_cores: 4.0,
                memory_mb: 4096,
                disk_mb: 2048,
                estimated_time_ms: 120000,
                ..Default::default()
            },
            TaskCategory::Testing => ResourceRequirements {
                cpu_cores: 2.0,
                memory_mb: 2048,
                estimated_time_ms: 30000,
                ..Default::default()
            },
            TaskCategory::Generic => ResourceRequirements {
                cpu_cores: 1.0,
                memory_mb: 512,
                estimated_time_ms: 5000,
                ..Default::default()
            },
        }
    }
}

impl Default for DefaultTaskAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskAnalyzer for DefaultTaskAnalyzer {
    #[instrument(skip(self, task), fields(task_id = %task.id))]
    async fn analyze(&self, task: &TaskInfo) -> Result<TaskAnalysis> {
        // Get or estimate requirements
        let requirements = if let Some(ref req) = task.requirements {
            req.clone()
        } else {
            self.estimate_resources(task).await?
        };

        let complexity = self.detect_complexity(&requirements);
        let complexity_score = requirements.complexity_score();

        // Suggest execution mode based on analysis
        let suggested_mode = if requirements.gpu_required {
            ExecutionMode::Cloud // GPU tasks typically need cloud
        } else if complexity >= TaskComplexity::Complex {
            ExecutionMode::Cloud // Complex tasks benefit from cloud resources
        } else if complexity == TaskComplexity::Simple {
            ExecutionMode::Local // Simple tasks can run locally
        } else {
            ExecutionMode::Auto // Let other factors decide
        };

        let mut notes = Vec::new();
        if requirements.gpu_required {
            notes.push("GPU required - cloud recommended".to_string());
        }
        if complexity >= TaskComplexity::Complex {
            notes.push(format!(
                "High complexity ({:?}) - consider cloud",
                complexity
            ));
        }
        if requirements.estimated_time_ms > self.complexity_thresholds.complex_time_ms {
            notes.push("Long-running task - cloud may provide better reliability".to_string());
        }

        let confidence = if task.requirements.is_some() {
            0.9 // High confidence if requirements were provided
        } else if task.code.is_some() {
            0.7 // Medium-high if we analyzed code
        } else {
            0.5 // Lower confidence for category-based estimates
        };

        Ok(TaskAnalysis {
            task_id: task.id.clone(),
            category: task.category.clone(),
            complexity,
            requirements,
            complexity_score,
            suggested_mode,
            confidence,
            notes,
        })
    }

    async fn estimate_resources(&self, task: &TaskInfo) -> Result<ResourceRequirements> {
        if let Some(ref req) = task.requirements {
            return Ok(req.clone());
        }

        let requirements = if let (Some(ref code), Some(ref lang)) = (&task.code, &task.language) {
            self.analyze_code(code, lang)
        } else {
            self.estimate_by_category(&task.category)
        };

        Ok(requirements)
    }

    fn detect_complexity(&self, requirements: &ResourceRequirements) -> TaskComplexity {
        let t = &self.complexity_thresholds;

        // Check for intensive level
        if requirements.cpu_cores >= t.intensive_cpu
            || requirements.memory_mb >= t.intensive_memory_mb
            || requirements.estimated_time_ms >= t.intensive_time_ms
            || requirements.gpu_required
        {
            return TaskComplexity::Intensive;
        }

        // Check for complex level
        if requirements.cpu_cores >= t.complex_cpu
            || requirements.memory_mb >= t.complex_memory_mb
            || requirements.estimated_time_ms >= t.complex_time_ms
        {
            return TaskComplexity::Complex;
        }

        // Check for moderate level
        if requirements.cpu_cores >= t.moderate_cpu
            || requirements.memory_mb >= t.moderate_memory_mb
            || requirements.estimated_time_ms >= t.moderate_time_ms
        {
            return TaskComplexity::Moderate;
        }

        TaskComplexity::Simple
    }
}

// ============================================================================
// Capability Checking
// ============================================================================

/// Available capabilities for execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutorCapabilities {
    /// Available CPU cores
    pub cpu_cores: f64,
    /// Available memory in MB
    pub memory_mb: u64,
    /// Available disk space in MB
    pub disk_mb: u64,
    /// GPU available
    pub gpu_available: bool,
    /// GPU memory in MB (if available)
    pub gpu_memory_mb: u64,
    /// Maximum concurrent executions
    pub max_concurrent: usize,
    /// Current active executions
    pub active_executions: usize,
    /// Available tools/languages
    pub available_tools: Vec<String>,
    /// Whether the executor is healthy
    pub healthy: bool,
    /// Executor latency in ms
    pub latency_ms: u64,
    /// Executor uptime percentage
    pub uptime_percent: f64,
}

impl ExecutorCapabilities {
    /// Check if capabilities meet requirements
    pub fn meets_requirements(&self, requirements: &ResourceRequirements) -> bool {
        if !self.healthy {
            return false;
        }

        if requirements.cpu_cores > self.cpu_cores {
            return false;
        }

        if requirements.memory_mb > self.memory_mb {
            return false;
        }

        if requirements.disk_mb > self.disk_mb {
            return false;
        }

        if requirements.gpu_required && !self.gpu_available {
            return false;
        }

        if self.active_executions >= self.max_concurrent {
            return false;
        }

        // Check required tools
        for tool in &requirements.required_tools {
            if !self.available_tools.iter().any(|t| t == tool) {
                return false;
            }
        }

        true
    }

    /// Calculate available capacity percentage
    pub fn available_capacity(&self) -> f64 {
        if self.max_concurrent == 0 {
            return 0.0;
        }
        let used = self.active_executions as f64 / self.max_concurrent as f64;
        (1.0 - used) * 100.0
    }
}

/// Capability check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityCheckResult {
    /// Local capabilities
    pub local: ExecutorCapabilities,
    /// Cloud capabilities
    pub cloud: ExecutorCapabilities,
    /// Can execute locally
    pub can_execute_local: bool,
    /// Can execute in cloud
    pub can_execute_cloud: bool,
    /// Reason if local is not available
    pub local_unavailable_reason: Option<String>,
    /// Reason if cloud is not available
    pub cloud_unavailable_reason: Option<String>,
}

/// Trait for capability checking
#[async_trait]
pub trait CapabilityChecker: Send + Sync {
    /// Get local executor capabilities
    async fn get_local_capabilities(&self) -> Result<ExecutorCapabilities>;

    /// Get cloud executor capabilities
    async fn get_cloud_capabilities(&self) -> Result<ExecutorCapabilities>;

    /// Check if requirements can be met
    async fn check_capabilities(
        &self,
        requirements: &ResourceRequirements,
    ) -> Result<CapabilityCheckResult>;
}

/// Default capability checker implementation
pub struct DefaultCapabilityChecker {
    /// Local capabilities (configurable)
    local_caps: Arc<RwLock<ExecutorCapabilities>>,
    /// Cloud capabilities (configurable)
    cloud_caps: Arc<RwLock<ExecutorCapabilities>>,
}

impl DefaultCapabilityChecker {
    /// Create a new capability checker
    pub fn new(local: ExecutorCapabilities, cloud: ExecutorCapabilities) -> Self {
        Self {
            local_caps: Arc::new(RwLock::new(local)),
            cloud_caps: Arc::new(RwLock::new(cloud)),
        }
    }

    /// Update local capabilities
    pub async fn update_local(&self, caps: ExecutorCapabilities) {
        let mut local = self.local_caps.write().await;
        *local = caps;
    }

    /// Update cloud capabilities
    pub async fn update_cloud(&self, caps: ExecutorCapabilities) {
        let mut cloud = self.cloud_caps.write().await;
        *cloud = caps;
    }
}

impl Default for DefaultCapabilityChecker {
    fn default() -> Self {
        // Default local capabilities (typical developer machine)
        let local = ExecutorCapabilities {
            cpu_cores: 4.0,
            memory_mb: 8192,
            disk_mb: 50000,
            gpu_available: false,
            gpu_memory_mb: 0,
            max_concurrent: 5,
            active_executions: 0,
            available_tools: vec![
                "python".to_string(),
                "node".to_string(),
                "go".to_string(),
                "rust".to_string(),
            ],
            healthy: true,
            latency_ms: 10,
            uptime_percent: 99.0,
        };

        // Default cloud capabilities (cloud VM)
        let cloud = ExecutorCapabilities {
            cpu_cores: 16.0,
            memory_mb: 32768,
            disk_mb: 200000,
            gpu_available: true,
            gpu_memory_mb: 16384,
            max_concurrent: 20,
            active_executions: 0,
            available_tools: vec![
                "python".to_string(),
                "node".to_string(),
                "go".to_string(),
                "rust".to_string(),
                "java".to_string(),
                "tensorflow".to_string(),
                "pytorch".to_string(),
            ],
            healthy: true,
            latency_ms: 100,
            uptime_percent: 99.9,
        };

        Self::new(local, cloud)
    }
}

#[async_trait]
impl CapabilityChecker for DefaultCapabilityChecker {
    async fn get_local_capabilities(&self) -> Result<ExecutorCapabilities> {
        Ok(self.local_caps.read().await.clone())
    }

    async fn get_cloud_capabilities(&self) -> Result<ExecutorCapabilities> {
        Ok(self.cloud_caps.read().await.clone())
    }

    async fn check_capabilities(
        &self,
        requirements: &ResourceRequirements,
    ) -> Result<CapabilityCheckResult> {
        let local = self.get_local_capabilities().await?;
        let cloud = self.get_cloud_capabilities().await?;

        let can_execute_local = local.meets_requirements(requirements);
        let can_execute_cloud = cloud.meets_requirements(requirements);

        let local_unavailable_reason = if !can_execute_local {
            Some(get_unavailable_reason(&local, requirements))
        } else {
            None
        };

        let cloud_unavailable_reason = if !can_execute_cloud {
            Some(get_unavailable_reason(&cloud, requirements))
        } else {
            None
        };

        Ok(CapabilityCheckResult {
            local,
            cloud,
            can_execute_local,
            can_execute_cloud,
            local_unavailable_reason,
            cloud_unavailable_reason,
        })
    }
}

/// Helper to get reason why capabilities don't meet requirements
fn get_unavailable_reason(caps: &ExecutorCapabilities, reqs: &ResourceRequirements) -> String {
    if !caps.healthy {
        return "Executor is unhealthy".to_string();
    }
    if reqs.cpu_cores > caps.cpu_cores {
        return format!(
            "Insufficient CPU: need {} cores, have {}",
            reqs.cpu_cores, caps.cpu_cores
        );
    }
    if reqs.memory_mb > caps.memory_mb {
        return format!(
            "Insufficient memory: need {} MB, have {} MB",
            reqs.memory_mb, caps.memory_mb
        );
    }
    if reqs.gpu_required && !caps.gpu_available {
        return "GPU required but not available".to_string();
    }
    if caps.active_executions >= caps.max_concurrent {
        return "No available execution slots".to_string();
    }
    "Unknown reason".to_string()
}

// ============================================================================
// Cost Estimation
// ============================================================================

/// Cost estimate for execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostEstimate {
    /// Estimated cost for local execution (in credits/units)
    pub local_cost: f64,
    /// Estimated cost for cloud execution
    pub cloud_cost: f64,
    /// Estimated local execution time (ms)
    pub local_time_ms: u64,
    /// Estimated cloud execution time (ms)
    pub cloud_time_ms: u64,
    /// Cost breakdown details
    pub breakdown: CostBreakdown,
    /// Recommended mode based on cost
    pub cost_recommended_mode: ExecutionMode,
    /// Cost savings percentage for recommended mode
    pub savings_percent: f64,
}

/// Detailed cost breakdown
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// CPU cost component
    pub cpu_cost: f64,
    /// Memory cost component
    pub memory_cost: f64,
    /// GPU cost component
    pub gpu_cost: f64,
    /// Network cost component
    pub network_cost: f64,
    /// Storage cost component
    pub storage_cost: f64,
    /// Time-based cost component
    pub time_cost: f64,
}

/// Pricing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    /// Local cost per CPU-hour
    pub local_cpu_per_hour: f64,
    /// Local cost per GB-hour of memory
    pub local_memory_per_gb_hour: f64,
    /// Cloud cost per CPU-hour
    pub cloud_cpu_per_hour: f64,
    /// Cloud cost per GB-hour of memory
    pub cloud_memory_per_gb_hour: f64,
    /// Cloud cost per GPU-hour
    pub cloud_gpu_per_hour: f64,
    /// Network egress cost per GB
    pub network_per_gb: f64,
    /// Storage cost per GB-hour
    pub storage_per_gb_hour: f64,
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            local_cpu_per_hour: 0.01, // Minimal local cost
            local_memory_per_gb_hour: 0.001,
            cloud_cpu_per_hour: 0.05, // Cloud CPU cost
            cloud_memory_per_gb_hour: 0.01,
            cloud_gpu_per_hour: 0.50, // GPU is expensive
            network_per_gb: 0.02,
            storage_per_gb_hour: 0.001,
        }
    }
}

/// Trait for cost estimation
#[async_trait]
pub trait CostEstimator: Send + Sync {
    /// Estimate execution cost
    async fn estimate_cost(
        &self,
        requirements: &ResourceRequirements,
        capabilities: &CapabilityCheckResult,
    ) -> Result<CostEstimate>;

    /// Get pricing configuration
    fn pricing(&self) -> &PricingConfig;
}

/// Default cost estimator implementation
pub struct DefaultCostEstimator {
    pricing: PricingConfig,
}

impl DefaultCostEstimator {
    /// Create a new cost estimator
    pub fn new(pricing: PricingConfig) -> Self {
        Self { pricing }
    }

    /// Calculate cost for given resources and pricing
    fn calculate_cost(
        &self,
        requirements: &ResourceRequirements,
        is_cloud: bool,
        latency_ms: u64,
    ) -> (f64, CostBreakdown) {
        let hours = (requirements.estimated_time_ms as f64) / 3_600_000.0;

        let cpu_rate = if is_cloud {
            self.pricing.cloud_cpu_per_hour
        } else {
            self.pricing.local_cpu_per_hour
        };

        let memory_rate = if is_cloud {
            self.pricing.cloud_memory_per_gb_hour
        } else {
            self.pricing.local_memory_per_gb_hour
        };

        let cpu_cost = requirements.cpu_cores * hours * cpu_rate;
        let memory_cost = (requirements.memory_mb as f64 / 1024.0) * hours * memory_rate;

        let gpu_cost = if is_cloud && requirements.gpu_required {
            hours * self.pricing.cloud_gpu_per_hour
        } else {
            0.0
        };

        let network_cost =
            requirements.network_bandwidth_mbps * hours * self.pricing.network_per_gb / 1000.0;
        let storage_cost =
            (requirements.disk_mb as f64 / 1024.0) * hours * self.pricing.storage_per_gb_hour;

        // Time-based cost includes latency consideration
        let time_cost = (latency_ms as f64 / 1000.0) * 0.001; // Small penalty for latency

        let breakdown = CostBreakdown {
            cpu_cost,
            memory_cost,
            gpu_cost,
            network_cost,
            storage_cost,
            time_cost,
        };

        let total = cpu_cost + memory_cost + gpu_cost + network_cost + storage_cost + time_cost;
        (total, breakdown)
    }
}

impl Default for DefaultCostEstimator {
    fn default() -> Self {
        Self::new(PricingConfig::default())
    }
}

#[async_trait]
impl CostEstimator for DefaultCostEstimator {
    async fn estimate_cost(
        &self,
        requirements: &ResourceRequirements,
        capabilities: &CapabilityCheckResult,
    ) -> Result<CostEstimate> {
        let (local_cost, local_breakdown) =
            self.calculate_cost(requirements, false, capabilities.local.latency_ms);
        let (cloud_cost, _cloud_breakdown) =
            self.calculate_cost(requirements, true, capabilities.cloud.latency_ms);

        // Adjust times based on capabilities (cloud might be faster with more resources)
        let local_time_ms = if capabilities.can_execute_local {
            requirements.estimated_time_ms + capabilities.local.latency_ms
        } else {
            u64::MAX
        };

        let cloud_time_ms = if capabilities.can_execute_cloud {
            // Cloud might be faster due to more resources
            // R1-M5: Guard against division by zero if local.cpu_cores is 0
            let local_cores = if capabilities.local.cpu_cores <= 0.0 {
                1.0
            } else {
                capabilities.local.cpu_cores
            };
            let resource_factor = capabilities.cloud.cpu_cores / local_cores;
            let adjusted_time =
                (requirements.estimated_time_ms as f64 / resource_factor.min(4.0)) as u64;
            adjusted_time + capabilities.cloud.latency_ms
        } else {
            u64::MAX
        };

        let (cost_recommended_mode, savings_percent) = if !capabilities.can_execute_cloud {
            (ExecutionMode::Local, 100.0)
        } else if !capabilities.can_execute_local {
            (ExecutionMode::Cloud, 100.0)
        } else if local_cost <= cloud_cost {
            let savings = (cloud_cost - local_cost) / cloud_cost * 100.0;
            (ExecutionMode::Local, savings)
        } else {
            let savings = (local_cost - cloud_cost) / local_cost * 100.0;
            (ExecutionMode::Cloud, savings)
        };

        Ok(CostEstimate {
            local_cost,
            cloud_cost,
            local_time_ms,
            cloud_time_ms,
            breakdown: local_breakdown, // Use local breakdown as base
            cost_recommended_mode,
            savings_percent,
        })
    }

    fn pricing(&self) -> &PricingConfig {
        &self.pricing
    }
}

// ============================================================================
// Decision Logging
// ============================================================================

/// A logged decision entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionLogEntry {
    /// Log entry ID
    pub id: String,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Task ID
    pub task_id: String,
    /// Selected mode
    pub selected_mode: ExecutionMode,
    /// Task analysis
    pub task_analysis: TaskAnalysis,
    /// Capability check result
    pub capabilities: CapabilityCheckResult,
    /// Cost estimate
    pub cost_estimate: CostEstimate,
    /// User preferences applied
    pub user_preferences: UserPreferences,
    /// Decision factors and their weights
    pub decision_factors: Vec<DecisionFactor>,
    /// Final decision reason
    pub decision_reason: String,
    /// Execution outcome (filled after execution)
    pub outcome: Option<ExecutionOutcome>,
}

/// A factor that influenced the decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionFactor {
    /// Factor name
    pub name: String,
    /// Factor value
    pub value: serde_json::Value,
    /// Weight in decision (0-1)
    pub weight: f64,
    /// Impact on decision
    pub impact: String,
}

/// Outcome of an execution (for learning/feedback)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutcome {
    /// Whether execution succeeded
    pub success: bool,
    /// Actual execution time (ms)
    pub actual_time_ms: u64,
    /// Actual cost
    pub actual_cost: f64,
    /// Error message if failed
    pub error: Option<String>,
}

/// Trait for decision logging
#[async_trait]
pub trait DecisionLogger: Send + Sync {
    /// Log a mode selection decision
    async fn log_decision(&self, entry: DecisionLogEntry) -> Result<()>;

    /// Update decision with outcome
    async fn update_outcome(&self, decision_id: &str, outcome: ExecutionOutcome) -> Result<()>;

    /// Get recent decisions
    async fn get_recent_decisions(&self, limit: usize) -> Result<Vec<DecisionLogEntry>>;

    /// Get decision by ID
    async fn get_decision(&self, id: &str) -> Result<Option<DecisionLogEntry>>;

    /// Get decisions for a task
    async fn get_decisions_for_task(&self, task_id: &str) -> Result<Vec<DecisionLogEntry>>;

    /// Get decision statistics
    async fn get_statistics(&self) -> Result<DecisionStatistics>;
}

/// Statistics about decisions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DecisionStatistics {
    /// Total decisions made
    pub total_decisions: u64,
    /// Decisions that selected local
    pub local_decisions: u64,
    /// Decisions that selected cloud
    pub cloud_decisions: u64,
    /// Decisions with outcomes
    pub decisions_with_outcomes: u64,
    /// Success rate for local execution
    pub local_success_rate: f64,
    /// Success rate for cloud execution
    pub cloud_success_rate: f64,
    /// Average cost savings
    pub average_cost_savings: f64,
}

/// In-memory decision logger
pub struct InMemoryDecisionLogger {
    /// Stored decisions
    decisions: Arc<RwLock<Vec<DecisionLogEntry>>>,
    /// Maximum entries to keep
    max_entries: usize,
    /// Statistics counters
    stats: Arc<DecisionStats>,
}

#[derive(Debug, Default)]
struct DecisionStats {
    total: AtomicU64,
    local: AtomicU64,
    cloud: AtomicU64,
    local_success: AtomicU64,
    cloud_success: AtomicU64,
    local_total_with_outcome: AtomicU64,
    cloud_total_with_outcome: AtomicU64,
}

impl InMemoryDecisionLogger {
    /// Create a new in-memory logger
    pub fn new(max_entries: usize) -> Self {
        Self {
            decisions: Arc::new(RwLock::new(Vec::new())),
            max_entries,
            stats: Arc::new(DecisionStats::default()),
        }
    }
}

impl Default for InMemoryDecisionLogger {
    fn default() -> Self {
        Self::new(1000)
    }
}

#[async_trait]
impl DecisionLogger for InMemoryDecisionLogger {
    async fn log_decision(&self, entry: DecisionLogEntry) -> Result<()> {
        // Update stats
        self.stats.total.fetch_add(1, Ordering::SeqCst);
        match entry.selected_mode {
            ExecutionMode::Local => {
                self.stats.local.fetch_add(1, Ordering::SeqCst);
            }
            ExecutionMode::Cloud => {
                self.stats.cloud.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }

        // Store decision
        let mut decisions = self.decisions.write().await;
        decisions.push(entry);

        // R1-L167: Trim using drain instead of O(n) remove(0)
        if decisions.len() > self.max_entries {
            let drain_count = decisions.len() - self.max_entries;
            decisions.drain(..drain_count);
        }

        Ok(())
    }

    async fn update_outcome(&self, decision_id: &str, outcome: ExecutionOutcome) -> Result<()> {
        let mut decisions = self.decisions.write().await;

        if let Some(entry) = decisions.iter_mut().find(|d| d.id == decision_id) {
            // Update stats
            match entry.selected_mode {
                ExecutionMode::Local => {
                    self.stats
                        .local_total_with_outcome
                        .fetch_add(1, Ordering::SeqCst);
                    if outcome.success {
                        self.stats.local_success.fetch_add(1, Ordering::SeqCst);
                    }
                }
                ExecutionMode::Cloud => {
                    self.stats
                        .cloud_total_with_outcome
                        .fetch_add(1, Ordering::SeqCst);
                    if outcome.success {
                        self.stats.cloud_success.fetch_add(1, Ordering::SeqCst);
                    }
                }
                _ => {}
            }

            entry.outcome = Some(outcome);
            Ok(())
        } else {
            Err(Error::NotFound(format!(
                "Decision not found: {}",
                decision_id
            )))
        }
    }

    async fn get_recent_decisions(&self, limit: usize) -> Result<Vec<DecisionLogEntry>> {
        let decisions = self.decisions.read().await;
        let start = decisions.len().saturating_sub(limit);
        Ok(decisions[start..].to_vec())
    }

    async fn get_decision(&self, id: &str) -> Result<Option<DecisionLogEntry>> {
        let decisions = self.decisions.read().await;
        Ok(decisions.iter().find(|d| d.id == id).cloned())
    }

    async fn get_decisions_for_task(&self, task_id: &str) -> Result<Vec<DecisionLogEntry>> {
        let decisions = self.decisions.read().await;
        Ok(decisions
            .iter()
            .filter(|d| d.task_id == task_id)
            .cloned()
            .collect())
    }

    async fn get_statistics(&self) -> Result<DecisionStatistics> {
        let total = self.stats.total.load(Ordering::SeqCst);
        let local = self.stats.local.load(Ordering::SeqCst);
        let cloud = self.stats.cloud.load(Ordering::SeqCst);
        let local_success = self.stats.local_success.load(Ordering::SeqCst);
        let cloud_success = self.stats.cloud_success.load(Ordering::SeqCst);
        let local_with_outcome = self.stats.local_total_with_outcome.load(Ordering::SeqCst);
        let cloud_with_outcome = self.stats.cloud_total_with_outcome.load(Ordering::SeqCst);

        let local_success_rate = if local_with_outcome > 0 {
            (local_success as f64) / (local_with_outcome as f64)
        } else {
            0.0
        };

        let cloud_success_rate = if cloud_with_outcome > 0 {
            (cloud_success as f64) / (cloud_with_outcome as f64)
        } else {
            0.0
        };

        Ok(DecisionStatistics {
            total_decisions: total,
            local_decisions: local,
            cloud_decisions: cloud,
            decisions_with_outcomes: local_with_outcome + cloud_with_outcome,
            local_success_rate,
            cloud_success_rate,
            average_cost_savings: 0.0, // Would need to track this separately
        })
    }
}

// ============================================================================
// User Preferences
// ============================================================================

/// User preferences for mode selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreferences {
    /// Forced execution mode (overrides auto-selection)
    pub forced_mode: Option<ExecutionMode>,
    /// Optimization goal
    pub optimization_goal: OptimizationGoal,
    /// Maximum cost per execution
    pub max_cost: Option<f64>,
    /// Maximum execution time (ms)
    pub max_time_ms: Option<u64>,
    /// Prefer local execution
    pub prefer_local: bool,
    /// Allow cloud execution
    pub allow_cloud: bool,
    /// Allow GPU usage
    pub allow_gpu: bool,
    /// Custom weights for decision factors
    pub factor_weights: HashMap<String, f64>,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            forced_mode: None,
            optimization_goal: OptimizationGoal::Balanced,
            max_cost: None,
            max_time_ms: None,
            prefer_local: true,
            allow_cloud: true,
            allow_gpu: true,
            factor_weights: HashMap::new(),
        }
    }
}

impl UserPreferences {
    /// Create new preferences with optimization goal
    pub fn new(goal: OptimizationGoal) -> Self {
        Self {
            optimization_goal: goal,
            ..Default::default()
        }
    }

    /// Force a specific execution mode
    pub fn with_forced_mode(mut self, mode: ExecutionMode) -> Self {
        self.forced_mode = Some(mode);
        self
    }

    /// Set maximum cost
    pub fn with_max_cost(mut self, cost: f64) -> Self {
        self.max_cost = Some(cost);
        self
    }

    /// Set maximum time
    pub fn with_max_time(mut self, time_ms: u64) -> Self {
        self.max_time_ms = Some(time_ms);
        self
    }

    /// Disable cloud execution
    pub fn local_only(mut self) -> Self {
        self.allow_cloud = false;
        self
    }

    /// Disable local execution
    pub fn cloud_only(mut self) -> Self {
        self.prefer_local = false;
        self.allow_cloud = true;
        self
    }

    /// Add a custom factor weight
    pub fn with_factor_weight(mut self, factor: impl Into<String>, weight: f64) -> Self {
        self.factor_weights.insert(factor.into(), weight);
        self
    }
}

// ============================================================================
// Mode Selector Trait
// ============================================================================

/// Request for mode selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeSelectionRequest {
    /// Task information
    pub task: TaskInfo,
    /// User preferences
    pub preferences: UserPreferences,
    /// Session context (for continuity)
    pub session_id: Option<String>,
}

impl ModeSelectionRequest {
    /// Create a new mode selection request
    pub fn new(task: TaskInfo) -> Self {
        Self {
            task,
            preferences: UserPreferences::default(),
            session_id: None,
        }
    }

    /// Set preferences
    pub fn with_preferences(mut self, preferences: UserPreferences) -> Self {
        self.preferences = preferences;
        self
    }

    /// Set session ID
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

/// Result of mode selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeSelectionResult {
    /// Decision ID (for tracking)
    pub decision_id: String,
    /// Selected execution mode
    pub selected_mode: ExecutionMode,
    /// Task analysis
    pub analysis: TaskAnalysis,
    /// Capability check result
    pub capabilities: CapabilityCheckResult,
    /// Cost estimate
    pub cost_estimate: CostEstimate,
    /// Decision factors
    pub factors: Vec<DecisionFactor>,
    /// Reason for selection
    pub reason: String,
    /// Confidence in decision (0-1)
    pub confidence: f64,
    /// Alternative modes and why not selected
    pub alternatives: Vec<AlternativeMode>,
}

/// An alternative mode that was considered
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlternativeMode {
    /// The mode
    pub mode: ExecutionMode,
    /// Why it wasn't selected
    pub reason_not_selected: String,
    /// Score for this mode
    pub score: f64,
}

/// Trait for mode selection
#[async_trait]
pub trait ModeSelector: Send + Sync {
    /// Select the best execution mode for a task
    async fn select_mode(&self, request: ModeSelectionRequest) -> Result<ModeSelectionResult>;

    /// Get the task analyzer
    fn task_analyzer(&self) -> &dyn TaskAnalyzer;

    /// Get the capability checker
    fn capability_checker(&self) -> &dyn CapabilityChecker;

    /// Get the cost estimator
    fn cost_estimator(&self) -> &dyn CostEstimator;

    /// Get the decision logger
    fn decision_logger(&self) -> &dyn DecisionLogger;
}

// ============================================================================
// Auto Mode Selector Implementation
// ============================================================================

/// Automatic mode selector that combines all components
pub struct AutoModeSelector {
    /// Task analyzer
    task_analyzer: Arc<dyn TaskAnalyzer>,
    /// Capability checker
    capability_checker: Arc<dyn CapabilityChecker>,
    /// Cost estimator
    cost_estimator: Arc<dyn CostEstimator>,
    /// Decision logger
    decision_logger: Arc<dyn DecisionLogger>,
    /// Configuration
    config: AutoModeSelectorConfig,
}

/// Configuration for auto mode selector
#[derive(Debug, Clone)]
pub struct AutoModeSelectorConfig {
    /// Weight for cost factor
    pub cost_weight: f64,
    /// Weight for performance factor
    pub performance_weight: f64,
    /// Weight for reliability factor
    pub reliability_weight: f64,
    /// Weight for availability factor
    pub availability_weight: f64,
    /// Minimum confidence threshold for auto-selection
    pub min_confidence: f64,
    /// Default mode if auto-selection fails
    pub default_mode: ExecutionMode,
}

impl Default for AutoModeSelectorConfig {
    fn default() -> Self {
        Self {
            cost_weight: 0.25,
            performance_weight: 0.30,
            reliability_weight: 0.25,
            availability_weight: 0.20,
            min_confidence: 0.5,
            default_mode: ExecutionMode::Local,
        }
    }
}

impl AutoModeSelector {
    /// Create a new auto mode selector
    pub fn new(
        task_analyzer: Arc<dyn TaskAnalyzer>,
        capability_checker: Arc<dyn CapabilityChecker>,
        cost_estimator: Arc<dyn CostEstimator>,
        decision_logger: Arc<dyn DecisionLogger>,
        config: AutoModeSelectorConfig,
    ) -> Self {
        Self {
            task_analyzer,
            capability_checker,
            cost_estimator,
            decision_logger,
            config,
        }
    }

    /// Create a builder for auto mode selector
    pub fn builder() -> AutoModeSelectorBuilder {
        AutoModeSelectorBuilder::new()
    }

    /// Calculate scores for each mode
    fn calculate_mode_scores(
        &self,
        _analysis: &TaskAnalysis,
        capabilities: &CapabilityCheckResult,
        cost_estimate: &CostEstimate,
        preferences: &UserPreferences,
    ) -> (f64, f64, Vec<DecisionFactor>) {
        let mut factors = Vec::new();
        let mut local_score = 0.0;
        let mut cloud_score = 0.0;

        // Adjust weights based on optimization goal
        let (cost_w, perf_w, rel_w, avail_w) = match preferences.optimization_goal {
            OptimizationGoal::Cost => (0.50, 0.15, 0.20, 0.15),
            OptimizationGoal::Performance => (0.15, 0.50, 0.20, 0.15),
            OptimizationGoal::Reliability => (0.15, 0.20, 0.50, 0.15),
            OptimizationGoal::Balanced => (
                self.config.cost_weight,
                self.config.performance_weight,
                self.config.reliability_weight,
                self.config.availability_weight,
            ),
            OptimizationGoal::LocalFirst => {
                local_score += 0.3; // Bonus for local
                (0.20, 0.25, 0.25, 0.30)
            }
            OptimizationGoal::CloudFirst => {
                cloud_score += 0.3; // Bonus for cloud
                (0.20, 0.25, 0.25, 0.30)
            }
        };

        // Cost factor
        let cost_factor = if cost_estimate.local_cost <= cost_estimate.cloud_cost {
            let ratio = if cost_estimate.cloud_cost > 0.0 {
                cost_estimate.local_cost / cost_estimate.cloud_cost
            } else {
                1.0
            };
            local_score += (1.0 - ratio) * cost_w;
            DecisionFactor {
                name: "cost".to_string(),
                value: serde_json::json!({
                    "local": cost_estimate.local_cost,
                    "cloud": cost_estimate.cloud_cost,
                }),
                weight: cost_w,
                impact: format!("Local is {:.1}% cheaper", (1.0 - ratio) * 100.0),
            }
        } else {
            let ratio = if cost_estimate.local_cost > 0.0 {
                cost_estimate.cloud_cost / cost_estimate.local_cost
            } else {
                1.0
            };
            cloud_score += (1.0 - ratio) * cost_w;
            DecisionFactor {
                name: "cost".to_string(),
                value: serde_json::json!({
                    "local": cost_estimate.local_cost,
                    "cloud": cost_estimate.cloud_cost,
                }),
                weight: cost_w,
                impact: format!("Cloud is {:.1}% cheaper", (1.0 - ratio) * 100.0),
            }
        };
        factors.push(cost_factor);

        // Performance factor
        let perf_factor = if cost_estimate.local_time_ms <= cost_estimate.cloud_time_ms {
            let ratio = if cost_estimate.cloud_time_ms > 0 {
                cost_estimate.local_time_ms as f64 / cost_estimate.cloud_time_ms as f64
            } else {
                1.0
            };
            local_score += (1.0 - ratio) * perf_w;
            DecisionFactor {
                name: "performance".to_string(),
                value: serde_json::json!({
                    "local_time_ms": cost_estimate.local_time_ms,
                    "cloud_time_ms": cost_estimate.cloud_time_ms,
                }),
                weight: perf_w,
                impact: format!("Local is {:.1}% faster", (1.0 - ratio) * 100.0),
            }
        } else {
            let ratio = if cost_estimate.local_time_ms > 0 {
                cost_estimate.cloud_time_ms as f64 / cost_estimate.local_time_ms as f64
            } else {
                1.0
            };
            cloud_score += (1.0 - ratio) * perf_w;
            DecisionFactor {
                name: "performance".to_string(),
                value: serde_json::json!({
                    "local_time_ms": cost_estimate.local_time_ms,
                    "cloud_time_ms": cost_estimate.cloud_time_ms,
                }),
                weight: perf_w,
                impact: format!("Cloud is {:.1}% faster", (1.0 - ratio) * 100.0),
            }
        };
        factors.push(perf_factor);

        // Reliability factor
        let local_uptime = capabilities.local.uptime_percent / 100.0;
        let cloud_uptime = capabilities.cloud.uptime_percent / 100.0;
        local_score += local_uptime * rel_w;
        cloud_score += cloud_uptime * rel_w;
        factors.push(DecisionFactor {
            name: "reliability".to_string(),
            value: serde_json::json!({
                "local_uptime": capabilities.local.uptime_percent,
                "cloud_uptime": capabilities.cloud.uptime_percent,
            }),
            weight: rel_w,
            impact: format!(
                "Local uptime: {:.1}%, Cloud uptime: {:.1}%",
                capabilities.local.uptime_percent, capabilities.cloud.uptime_percent
            ),
        });

        // Availability factor
        let local_capacity = capabilities.local.available_capacity() / 100.0;
        let cloud_capacity = capabilities.cloud.available_capacity() / 100.0;
        local_score += local_capacity * avail_w;
        cloud_score += cloud_capacity * avail_w;
        factors.push(DecisionFactor {
            name: "availability".to_string(),
            value: serde_json::json!({
                "local_capacity": local_capacity * 100.0,
                "cloud_capacity": cloud_capacity * 100.0,
            }),
            weight: avail_w,
            impact: format!(
                "Local capacity: {:.1}%, Cloud capacity: {:.1}%",
                local_capacity * 100.0,
                cloud_capacity * 100.0
            ),
        });

        // Capability requirements
        if !capabilities.can_execute_local {
            local_score = 0.0;
            factors.push(DecisionFactor {
                name: "capability_local".to_string(),
                value: serde_json::json!(false),
                weight: 1.0,
                impact: capabilities
                    .local_unavailable_reason
                    .clone()
                    .unwrap_or_else(|| "Cannot execute locally".to_string()),
            });
        }
        if !capabilities.can_execute_cloud {
            cloud_score = 0.0;
            factors.push(DecisionFactor {
                name: "capability_cloud".to_string(),
                value: serde_json::json!(false),
                weight: 1.0,
                impact: capabilities
                    .cloud_unavailable_reason
                    .clone()
                    .unwrap_or_else(|| "Cannot execute in cloud".to_string()),
            });
        }

        // User preference modifiers
        if !preferences.allow_cloud {
            cloud_score = 0.0;
            factors.push(DecisionFactor {
                name: "user_preference".to_string(),
                value: serde_json::json!("cloud_disabled"),
                weight: 1.0,
                impact: "User disabled cloud execution".to_string(),
            });
        }
        if preferences.prefer_local {
            local_score *= 1.2; // 20% bonus for local preference
        }

        // Apply custom factor weights
        for (factor_name, weight) in &preferences.factor_weights {
            if let Some(f) = factors.iter_mut().find(|f| &f.name == factor_name) {
                f.weight = *weight;
            }
        }

        (local_score, cloud_score, factors)
    }
}

#[async_trait]
impl ModeSelector for AutoModeSelector {
    #[instrument(skip(self, request), fields(task_id = %request.task.id))]
    async fn select_mode(&self, request: ModeSelectionRequest) -> Result<ModeSelectionResult> {
        let decision_id = Uuid::new_v4().to_string();

        // Check for forced mode
        if let Some(forced_mode) = request.preferences.forced_mode {
            info!(mode = %forced_mode, "Using forced execution mode");

            let analysis = self.task_analyzer.analyze(&request.task).await?;
            let capabilities = self
                .capability_checker
                .check_capabilities(&analysis.requirements)
                .await?;
            let cost_estimate = self
                .cost_estimator
                .estimate_cost(&analysis.requirements, &capabilities)
                .await?;

            let log_entry = DecisionLogEntry {
                id: decision_id.clone(),
                timestamp: Utc::now(),
                task_id: request.task.id.clone(),
                selected_mode: forced_mode,
                task_analysis: analysis.clone(),
                capabilities: capabilities.clone(),
                cost_estimate: cost_estimate.clone(),
                user_preferences: request.preferences.clone(),
                decision_factors: vec![DecisionFactor {
                    name: "forced_mode".to_string(),
                    value: serde_json::json!(forced_mode.to_string()),
                    weight: 1.0,
                    impact: "User forced this mode".to_string(),
                }],
                decision_reason: format!("User forced {} mode", forced_mode),
                outcome: None,
            };

            self.decision_logger.log_decision(log_entry).await?;

            return Ok(ModeSelectionResult {
                decision_id,
                selected_mode: forced_mode,
                analysis,
                capabilities,
                cost_estimate,
                factors: vec![],
                reason: format!("User forced {} mode", forced_mode),
                confidence: 1.0,
                alternatives: vec![],
            });
        }

        // Analyze task
        let analysis = self.task_analyzer.analyze(&request.task).await?;
        debug!(
            complexity = ?analysis.complexity,
            score = analysis.complexity_score,
            "Task analyzed"
        );

        // Check capabilities
        let capabilities = self
            .capability_checker
            .check_capabilities(&analysis.requirements)
            .await?;
        debug!(
            can_local = capabilities.can_execute_local,
            can_cloud = capabilities.can_execute_cloud,
            "Capabilities checked"
        );

        // Estimate costs
        let cost_estimate = self
            .cost_estimator
            .estimate_cost(&analysis.requirements, &capabilities)
            .await?;
        debug!(
            local_cost = cost_estimate.local_cost,
            cloud_cost = cost_estimate.cloud_cost,
            "Costs estimated"
        );

        // Calculate scores
        let (local_score, cloud_score, factors) = self.calculate_mode_scores(
            &analysis,
            &capabilities,
            &cost_estimate,
            &request.preferences,
        );

        // Determine selected mode
        let (selected_mode, confidence, reason) =
            if !capabilities.can_execute_local && !capabilities.can_execute_cloud {
                (
                    self.config.default_mode,
                    0.0,
                    "Neither local nor cloud can execute this task".to_string(),
                )
            } else if !capabilities.can_execute_local {
                (
                    ExecutionMode::Cloud,
                    0.9,
                    "Local execution not available".to_string(),
                )
            } else if !capabilities.can_execute_cloud {
                (
                    ExecutionMode::Local,
                    0.9,
                    "Cloud execution not available".to_string(),
                )
            } else if local_score > cloud_score {
                let diff = local_score - cloud_score;
                let conf = (0.5 + diff.min(0.5)).min(1.0);
                (
                    ExecutionMode::Local,
                    conf,
                    format!(
                        "Local preferred: score {:.2} vs {:.2} (goal: {})",
                        local_score, cloud_score, request.preferences.optimization_goal
                    ),
                )
            } else if cloud_score > local_score {
                let diff = cloud_score - local_score;
                let conf = (0.5 + diff.min(0.5)).min(1.0);
                (
                    ExecutionMode::Cloud,
                    conf,
                    format!(
                        "Cloud preferred: score {:.2} vs {:.2} (goal: {})",
                        cloud_score, local_score, request.preferences.optimization_goal
                    ),
                )
            } else {
                // Tie - use default
                (
                    self.config.default_mode,
                    0.6,
                    "Scores tied, using default mode".to_string(),
                )
            };

        info!(
            mode = %selected_mode,
            confidence = confidence,
            local_score = local_score,
            cloud_score = cloud_score,
            "Mode selected"
        );

        // Build alternatives
        let mut alternatives = Vec::new();
        if selected_mode != ExecutionMode::Local && capabilities.can_execute_local {
            alternatives.push(AlternativeMode {
                mode: ExecutionMode::Local,
                reason_not_selected: format!("Score {:.2} lower than selected mode", local_score),
                score: local_score,
            });
        }
        if selected_mode != ExecutionMode::Cloud && capabilities.can_execute_cloud {
            alternatives.push(AlternativeMode {
                mode: ExecutionMode::Cloud,
                reason_not_selected: format!("Score {:.2} lower than selected mode", cloud_score),
                score: cloud_score,
            });
        }

        // Log decision
        let log_entry = DecisionLogEntry {
            id: decision_id.clone(),
            timestamp: Utc::now(),
            task_id: request.task.id.clone(),
            selected_mode,
            task_analysis: analysis.clone(),
            capabilities: capabilities.clone(),
            cost_estimate: cost_estimate.clone(),
            user_preferences: request.preferences,
            decision_factors: factors.clone(),
            decision_reason: reason.clone(),
            outcome: None,
        };

        self.decision_logger.log_decision(log_entry).await?;

        Ok(ModeSelectionResult {
            decision_id,
            selected_mode,
            analysis,
            capabilities,
            cost_estimate,
            factors,
            reason,
            confidence,
            alternatives,
        })
    }

    fn task_analyzer(&self) -> &dyn TaskAnalyzer {
        self.task_analyzer.as_ref()
    }

    fn capability_checker(&self) -> &dyn CapabilityChecker {
        self.capability_checker.as_ref()
    }

    fn cost_estimator(&self) -> &dyn CostEstimator {
        self.cost_estimator.as_ref()
    }

    fn decision_logger(&self) -> &dyn DecisionLogger {
        self.decision_logger.as_ref()
    }
}

// ============================================================================
// Collaboration Mode Selection (requires "collaboration" feature)
// ============================================================================

#[cfg(feature = "collaboration")]
impl AutoModeSelector {
    /// Select the best collaboration mode for a task.
    ///
    /// This method analyzes the task characteristics and determines the most
    /// appropriate collaboration mode:
    ///
    /// - **Direct**: Simple tasks handled by a single agent
    /// - **Swarm**: Tasks requiring handoffs between specialized agents
    /// - **Expert**: Tasks requiring supervisor coordination with specialists
    ///
    /// # Arguments
    ///
    /// * `request` - The mode selection request containing task details
    ///
    /// # Returns
    ///
    /// The recommended `CollaborationMode` for the task.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gateway_core::agent::mode_selector::{AutoModeSelector, ModeSelectionRequest};
    /// use gateway_core::collaboration::CollaborationMode;
    ///
    /// let selector = AutoModeSelector::builder()
    ///     .local_capabilities(local_caps)
    ///     .build();
    ///
    /// let request = ModeSelectionRequest::new(task);
    /// let collab_mode = selector.select_collaboration_mode(&request);
    ///
    /// match collab_mode {
    ///     CollaborationMode::Direct => { /* single agent */ }
    ///     CollaborationMode::Swarm { .. } => { /* swarm handoffs */ }
    ///     CollaborationMode::Expert { .. } => { /* supervisor + specialists */ }
    ///     CollaborationMode::Graph { .. } => { /* custom graph */ }
    /// }
    /// ```
    pub fn select_collaboration_mode(
        &self,
        request: &ModeSelectionRequest,
    ) -> crate::collaboration::CollaborationMode {
        use crate::collaboration::CollaborationMode;

        let task = &request.task;
        let category = &task.category;

        // Determine complexity from requirements
        let complexity = if let Some(ref reqs) = task.requirements {
            self.task_analyzer.detect_complexity(reqs)
        } else {
            // No requirements = simple task
            TaskComplexity::Simple
        };

        // Simple heuristics for collaboration mode selection:
        // 1. Simple tasks -> Direct mode (single agent)
        // 2. Moderate tasks with specific categories -> Swarm mode
        // 3. Complex/Intensive tasks -> Expert mode

        match complexity {
            TaskComplexity::Simple => {
                // Simple tasks are handled by a single agent
                CollaborationMode::Direct
            }
            TaskComplexity::Moderate => {
                // Moderate tasks may benefit from agent handoffs
                match category {
                    TaskCategory::BrowserAutomation => {
                        // Browser tasks often need research -> action handoffs
                        CollaborationMode::Swarm {
                            initial_agent: "browser_navigator".into(),
                            handoff_rules: vec![crate::collaboration::HandoffRule {
                                from_agent: "browser_navigator".into(),
                                to_agent: "browser_actor".into(),
                                condition: crate::collaboration::HandoffCondition::OnKeyword(
                                    "click".into(),
                                ),
                                context_transfer: crate::collaboration::ContextTransferMode::Full,
                            }],
                            agent_models: std::collections::HashMap::new(),
                        }
                    }
                    TaskCategory::Testing => {
                        // Testing benefits from code -> test -> review flow
                        CollaborationMode::Swarm {
                            initial_agent: "test_writer".into(),
                            handoff_rules: vec![crate::collaboration::HandoffRule {
                                from_agent: "test_writer".into(),
                                to_agent: "test_reviewer".into(),
                                condition: crate::collaboration::HandoffCondition::OnKeyword(
                                    "review".into(),
                                ),
                                context_transfer: crate::collaboration::ContextTransferMode::Full,
                            }],
                            agent_models: std::collections::HashMap::new(),
                        }
                    }
                    _ => CollaborationMode::Direct,
                }
            }
            TaskComplexity::Complex | TaskComplexity::Intensive => {
                // Complex tasks benefit from expert supervision
                match category {
                    TaskCategory::CodeExecution | TaskCategory::Build => {
                        CollaborationMode::Expert {
                            supervisor: "code_architect".into(),
                            specialists: vec![
                                "code_implementer".into(),
                                "code_reviewer".into(),
                                "test_writer".into(),
                            ],
                            supervisor_model: None,
                            default_specialist_model: None,
                            specialist_models: std::collections::HashMap::new(),
                        }
                    }
                    TaskCategory::MachineLearning | TaskCategory::DataProcessing => {
                        CollaborationMode::Expert {
                            supervisor: "ml_architect".into(),
                            specialists: vec![
                                "data_engineer".into(),
                                "model_trainer".into(),
                                "evaluator".into(),
                            ],
                            supervisor_model: None,
                            default_specialist_model: None,
                            specialist_models: std::collections::HashMap::new(),
                        }
                    }
                    TaskCategory::BrowserAutomation => CollaborationMode::Expert {
                        supervisor: "automation_lead".into(),
                        specialists: vec![
                            "navigator".into(),
                            "form_filler".into(),
                            "data_extractor".into(),
                        ],
                        supervisor_model: None,
                        default_specialist_model: None,
                        specialist_models: std::collections::HashMap::new(),
                    },
                    _ => {
                        // Default expert mode for complex generic tasks
                        CollaborationMode::Expert {
                            supervisor: "coordinator".into(),
                            specialists: vec!["executor".into(), "reviewer".into()],
                            supervisor_model: None,
                            default_specialist_model: None,
                            specialist_models: std::collections::HashMap::new(),
                        }
                    }
                }
            }
        }
    }

    /// Suggest the best collaboration mode based on task description text.
    ///
    /// This is a convenience method that analyzes raw task text to determine
    /// the collaboration mode without requiring a full ModeSelectionRequest.
    ///
    /// # Arguments
    ///
    /// * `task_description` - The natural language task description
    ///
    /// # Returns
    ///
    /// The recommended `CollaborationMode` for the task.
    pub fn suggest_collaboration_mode_for_text(
        &self,
        task_description: &str,
    ) -> crate::collaboration::CollaborationMode {
        use crate::collaboration::CollaborationMode;

        let desc_lower = task_description.to_lowercase();

        // Detect planning keywords - explicit requests for planning
        let needs_planning = desc_lower.contains("plan")
            || desc_lower.contains("规划")
            || desc_lower.contains("步骤")
            || desc_lower.contains("strategy")
            || desc_lower.contains("design")
            || desc_lower.contains("architect")
            || desc_lower.contains("breakdown")
            || desc_lower.contains("decompose");

        // Detect multi-step execution keywords
        let needs_multi_step = desc_lower.contains("then")
            || desc_lower.contains("and then")
            || desc_lower.contains("next")
            || desc_lower.contains("step by step")
            || desc_lower.contains("workflow")
            || desc_lower.contains("pipeline")
            || desc_lower.contains("first")
            || desc_lower.contains("finally")
            || desc_lower.contains("然后")
            || desc_lower.contains("接着")
            || desc_lower.contains("最后");

        let needs_review = desc_lower.contains("review")
            || desc_lower.contains("verify")
            || desc_lower.contains("check")
            || desc_lower.contains("validate")
            || desc_lower.contains("审查")
            || desc_lower.contains("验证");

        let needs_specialist = desc_lower.contains("specialist")
            || desc_lower.contains("expert")
            || desc_lower.contains("complex")
            || desc_lower.contains("comprehensive")
            || desc_lower.contains("专家");

        // Priority: Expert > PlanExecute > Swarm > Direct
        if needs_specialist || (needs_multi_step && needs_review) {
            // Complex tasks with multiple concerns -> Expert mode
            CollaborationMode::Expert {
                supervisor: "coordinator".into(),
                specialists: vec!["executor".into(), "reviewer".into()],
                supervisor_model: None,
                default_specialist_model: None,
                specialist_models: std::collections::HashMap::new(),
            }
        } else if needs_planning || (needs_multi_step && desc_lower.len() > 100) {
            // Tasks that need explicit planning or long multi-step tasks -> PlanExecute mode
            CollaborationMode::PlanExecute
        } else if needs_multi_step {
            // Simple sequential tasks -> Swarm mode
            CollaborationMode::Swarm {
                initial_agent: "primary".into(),
                handoff_rules: vec![],
                agent_models: std::collections::HashMap::new(),
            }
        } else {
            // Simple tasks -> Direct mode
            CollaborationMode::Direct
        }
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for AutoModeSelector
pub struct AutoModeSelectorBuilder {
    task_analyzer: Option<Arc<dyn TaskAnalyzer>>,
    capability_checker: Option<Arc<dyn CapabilityChecker>>,
    cost_estimator: Option<Arc<dyn CostEstimator>>,
    decision_logger: Option<Arc<dyn DecisionLogger>>,
    config: AutoModeSelectorConfig,
}

impl AutoModeSelectorBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            task_analyzer: None,
            capability_checker: None,
            cost_estimator: None,
            decision_logger: None,
            config: AutoModeSelectorConfig::default(),
        }
    }

    /// Set task analyzer
    pub fn task_analyzer(mut self, analyzer: Arc<dyn TaskAnalyzer>) -> Self {
        self.task_analyzer = Some(analyzer);
        self
    }

    /// Set capability checker
    pub fn capability_checker(mut self, checker: Arc<dyn CapabilityChecker>) -> Self {
        self.capability_checker = Some(checker);
        self
    }

    /// Set cost estimator
    pub fn cost_estimator(mut self, estimator: Arc<dyn CostEstimator>) -> Self {
        self.cost_estimator = Some(estimator);
        self
    }

    /// Set decision logger
    pub fn decision_logger(mut self, logger: Arc<dyn DecisionLogger>) -> Self {
        self.decision_logger = Some(logger);
        self
    }

    /// Set local capabilities
    pub fn local_capabilities(mut self, caps: ExecutorCapabilities) -> Self {
        if self.capability_checker.is_none() {
            let checker = DefaultCapabilityChecker::new(caps, ExecutorCapabilities::default());
            self.capability_checker = Some(Arc::new(checker));
        }
        self
    }

    /// Set cloud capabilities
    pub fn cloud_capabilities(mut self, caps: ExecutorCapabilities) -> Self {
        if self.capability_checker.is_none() {
            let checker = DefaultCapabilityChecker::new(ExecutorCapabilities::default(), caps);
            self.capability_checker = Some(Arc::new(checker));
        }
        self
    }

    /// Set configuration
    pub fn config(mut self, config: AutoModeSelectorConfig) -> Self {
        self.config = config;
        self
    }

    /// Set cost weight
    pub fn cost_weight(mut self, weight: f64) -> Self {
        self.config.cost_weight = weight;
        self
    }

    /// Set performance weight
    pub fn performance_weight(mut self, weight: f64) -> Self {
        self.config.performance_weight = weight;
        self
    }

    /// Set default mode
    pub fn default_mode(mut self, mode: ExecutionMode) -> Self {
        self.config.default_mode = mode;
        self
    }

    /// Build the selector
    pub fn build(self) -> AutoModeSelector {
        let task_analyzer = self
            .task_analyzer
            .unwrap_or_else(|| Arc::new(DefaultTaskAnalyzer::new()));
        let capability_checker = self
            .capability_checker
            .unwrap_or_else(|| Arc::new(DefaultCapabilityChecker::default()));
        let cost_estimator = self
            .cost_estimator
            .unwrap_or_else(|| Arc::new(DefaultCostEstimator::default()));
        let decision_logger = self
            .decision_logger
            .unwrap_or_else(|| Arc::new(InMemoryDecisionLogger::default()));

        AutoModeSelector::new(
            task_analyzer,
            capability_checker,
            cost_estimator,
            decision_logger,
            self.config,
        )
    }
}

impl Default for AutoModeSelectorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Test fixtures
    fn simple_task() -> TaskInfo {
        TaskInfo::new(TaskCategory::CodeExecution, "Simple print statement")
            .with_code("print('hello')", "python")
    }

    fn complex_task() -> TaskInfo {
        TaskInfo::new(TaskCategory::MachineLearning, "Train ML model")
            .with_code(
                r#"
                import torch
                import tensorflow as tf
                model = tf.keras.Sequential([...])
                model.fit(data, epochs=100)
                "#,
                "python",
            )
            .with_requirements(
                ResourceRequirements::new()
                    .with_cpu(4.0)
                    .with_memory(8192)
                    .with_gpu(true)
                    .with_time(60000),
            )
    }

    fn limited_local_caps() -> ExecutorCapabilities {
        ExecutorCapabilities {
            cpu_cores: 2.0,
            memory_mb: 4096,
            disk_mb: 20000,
            gpu_available: false,
            max_concurrent: 3,
            active_executions: 0,
            available_tools: vec!["python".to_string()],
            healthy: true,
            latency_ms: 5,
            uptime_percent: 99.0,
            ..Default::default()
        }
    }

    fn full_cloud_caps() -> ExecutorCapabilities {
        ExecutorCapabilities {
            cpu_cores: 32.0,
            memory_mb: 65536,
            disk_mb: 500000,
            gpu_available: true,
            gpu_memory_mb: 32768,
            max_concurrent: 50,
            active_executions: 0,
            available_tools: vec![
                "python".to_string(),
                "tensorflow".to_string(),
                "pytorch".to_string(),
            ],
            healthy: true,
            latency_ms: 50,
            uptime_percent: 99.9,
        }
    }

    // ========== ExecutionMode Tests ==========

    #[test]
    fn test_execution_mode_display() {
        assert_eq!(ExecutionMode::Local.to_string(), "local");
        assert_eq!(ExecutionMode::Cloud.to_string(), "cloud");
        assert_eq!(ExecutionMode::Hybrid.to_string(), "hybrid");
        assert_eq!(ExecutionMode::Auto.to_string(), "auto");
    }

    #[test]
    fn test_execution_mode_default() {
        assert_eq!(ExecutionMode::default(), ExecutionMode::Local);
    }

    #[test]
    fn test_execution_mode_serialization() {
        let mode = ExecutionMode::Cloud;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"cloud\"");

        let deserialized: ExecutionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ExecutionMode::Cloud);
    }

    // ========== TaskComplexity Tests ==========

    #[test]
    fn test_task_complexity_weight() {
        assert_eq!(TaskComplexity::Simple.weight(), 1);
        assert_eq!(TaskComplexity::Moderate.weight(), 2);
        assert_eq!(TaskComplexity::Complex.weight(), 3);
        assert_eq!(TaskComplexity::Intensive.weight(), 4);
    }

    #[test]
    fn test_task_complexity_ordering() {
        assert!(TaskComplexity::Simple < TaskComplexity::Moderate);
        assert!(TaskComplexity::Moderate < TaskComplexity::Complex);
        assert!(TaskComplexity::Complex < TaskComplexity::Intensive);
    }

    // ========== OptimizationGoal Tests ==========

    #[test]
    fn test_optimization_goal_display() {
        assert_eq!(OptimizationGoal::Cost.to_string(), "cost");
        assert_eq!(OptimizationGoal::Performance.to_string(), "performance");
        assert_eq!(OptimizationGoal::Balanced.to_string(), "balanced");
        assert_eq!(OptimizationGoal::LocalFirst.to_string(), "local_first");
    }

    // ========== ResourceRequirements Tests ==========

    #[test]
    fn test_resource_requirements_builder() {
        let req = ResourceRequirements::new()
            .with_cpu(2.0)
            .with_memory(4096)
            .with_disk(1000)
            .with_gpu(true)
            .with_time(5000)
            .with_tool("python");

        assert_eq!(req.cpu_cores, 2.0);
        assert_eq!(req.memory_mb, 4096);
        assert_eq!(req.disk_mb, 1000);
        assert!(req.gpu_required);
        assert_eq!(req.estimated_time_ms, 5000);
        assert_eq!(req.required_tools, vec!["python".to_string()]);
    }

    #[test]
    fn test_resource_requirements_complexity_score() {
        let simple = ResourceRequirements::new().with_cpu(0.5).with_memory(256);
        let complex = ResourceRequirements::new()
            .with_cpu(4.0)
            .with_memory(8192)
            .with_gpu(true);

        assert!(complex.complexity_score() > simple.complexity_score());
    }

    // ========== TaskInfo Tests ==========

    #[test]
    fn test_task_info_creation() {
        let task = TaskInfo::new(TaskCategory::CodeExecution, "Test task")
            .with_code("print('hello')", "python")
            .with_metadata("key", serde_json::json!("value"));

        assert_eq!(task.category, TaskCategory::CodeExecution);
        assert_eq!(task.description, "Test task");
        assert_eq!(task.code, Some("print('hello')".to_string()));
        assert_eq!(task.language, Some("python".to_string()));
        assert!(task.metadata.contains_key("key"));
    }

    // ========== DefaultTaskAnalyzer Tests ==========

    #[tokio::test]
    async fn test_task_analyzer_simple_task() {
        let analyzer = DefaultTaskAnalyzer::new();
        let task = simple_task();

        let analysis = analyzer.analyze(&task).await.unwrap();

        assert_eq!(analysis.category, TaskCategory::CodeExecution);
        assert!(analysis.complexity <= TaskComplexity::Moderate);
        assert!(analysis.confidence > 0.0);
    }

    #[tokio::test]
    async fn test_task_analyzer_complex_task() {
        let analyzer = DefaultTaskAnalyzer::new();
        let task = complex_task();

        let analysis = analyzer.analyze(&task).await.unwrap();

        assert_eq!(analysis.category, TaskCategory::MachineLearning);
        assert!(analysis.complexity >= TaskComplexity::Complex);
        assert!(analysis.requirements.gpu_required);
    }

    #[tokio::test]
    async fn test_task_analyzer_estimate_resources() {
        let analyzer = DefaultTaskAnalyzer::new();
        let task = TaskInfo::new(TaskCategory::DataProcessing, "Process data");

        let requirements = analyzer.estimate_resources(&task).await.unwrap();

        assert!(requirements.cpu_cores > 0.0);
        assert!(requirements.memory_mb > 0);
    }

    #[test]
    fn test_task_analyzer_detect_complexity() {
        let analyzer = DefaultTaskAnalyzer::new();

        let simple_req = ResourceRequirements::new().with_cpu(0.5).with_memory(256);
        assert_eq!(
            analyzer.detect_complexity(&simple_req),
            TaskComplexity::Simple
        );

        let moderate_req = ResourceRequirements::new().with_cpu(1.5).with_memory(1024);
        assert_eq!(
            analyzer.detect_complexity(&moderate_req),
            TaskComplexity::Moderate
        );

        let intensive_req = ResourceRequirements::new()
            .with_cpu(8.0)
            .with_memory(16384)
            .with_gpu(true);
        assert_eq!(
            analyzer.detect_complexity(&intensive_req),
            TaskComplexity::Intensive
        );
    }

    // ========== ExecutorCapabilities Tests ==========

    #[test]
    fn test_executor_capabilities_meets_requirements() {
        let caps = limited_local_caps();

        let simple_req = ResourceRequirements::new().with_cpu(1.0).with_memory(1024);
        assert!(caps.meets_requirements(&simple_req));

        let complex_req = ResourceRequirements::new().with_cpu(4.0).with_memory(16384);
        assert!(!caps.meets_requirements(&complex_req));

        let gpu_req = ResourceRequirements::new().with_gpu(true);
        assert!(!caps.meets_requirements(&gpu_req));
    }

    #[test]
    fn test_executor_capabilities_available_capacity() {
        let mut caps = limited_local_caps();
        assert_eq!(caps.available_capacity(), 100.0);

        caps.active_executions = 2;
        assert!((caps.available_capacity() - 33.33).abs() < 1.0);

        caps.active_executions = 3;
        assert_eq!(caps.available_capacity(), 0.0);
    }

    // ========== DefaultCapabilityChecker Tests ==========

    #[tokio::test]
    async fn test_capability_checker_check_capabilities() {
        let checker = DefaultCapabilityChecker::new(limited_local_caps(), full_cloud_caps());

        let simple_req = ResourceRequirements::new().with_cpu(1.0).with_memory(1024);
        let result = checker.check_capabilities(&simple_req).await.unwrap();

        assert!(result.can_execute_local);
        assert!(result.can_execute_cloud);
    }

    #[tokio::test]
    async fn test_capability_checker_gpu_requirement() {
        let checker = DefaultCapabilityChecker::new(limited_local_caps(), full_cloud_caps());

        let gpu_req = ResourceRequirements::new()
            .with_cpu(2.0)
            .with_memory(4096)
            .with_gpu(true);
        let result = checker.check_capabilities(&gpu_req).await.unwrap();

        assert!(!result.can_execute_local);
        assert!(result.can_execute_cloud);
        assert!(result.local_unavailable_reason.is_some());
    }

    #[tokio::test]
    async fn test_capability_checker_update_capabilities() {
        let checker = DefaultCapabilityChecker::new(limited_local_caps(), full_cloud_caps());

        let mut new_local = limited_local_caps();
        new_local.gpu_available = true;
        checker.update_local(new_local).await;

        let caps = checker.get_local_capabilities().await.unwrap();
        assert!(caps.gpu_available);
    }

    // ========== DefaultCostEstimator Tests ==========

    #[tokio::test]
    async fn test_cost_estimator_basic() {
        let estimator = DefaultCostEstimator::default();
        let checker = DefaultCapabilityChecker::new(limited_local_caps(), full_cloud_caps());

        let req = ResourceRequirements::new()
            .with_cpu(1.0)
            .with_memory(1024)
            .with_time(3600000); // 1 hour

        let caps = checker.check_capabilities(&req).await.unwrap();
        let estimate = estimator.estimate_cost(&req, &caps).await.unwrap();

        assert!(estimate.local_cost > 0.0);
        assert!(estimate.cloud_cost > 0.0);
        assert!(estimate.local_cost < estimate.cloud_cost); // Local should be cheaper
    }

    #[tokio::test]
    async fn test_cost_estimator_gpu_cost() {
        let estimator = DefaultCostEstimator::default();
        let checker = DefaultCapabilityChecker::new(limited_local_caps(), full_cloud_caps());

        let req = ResourceRequirements::new()
            .with_cpu(2.0)
            .with_memory(8192)
            .with_gpu(true)
            .with_time(3600000);

        let caps = checker.check_capabilities(&req).await.unwrap();
        let estimate = estimator.estimate_cost(&req, &caps).await.unwrap();

        // GPU cost should be significant
        assert!(estimate.cloud_cost > 0.5);
    }

    // ========== InMemoryDecisionLogger Tests ==========

    #[tokio::test]
    async fn test_decision_logger_log_and_retrieve() {
        let logger = InMemoryDecisionLogger::new(100);

        let entry = DecisionLogEntry {
            id: "test-1".to_string(),
            timestamp: Utc::now(),
            task_id: "task-1".to_string(),
            selected_mode: ExecutionMode::Local,
            task_analysis: TaskAnalysis {
                task_id: "task-1".to_string(),
                category: TaskCategory::CodeExecution,
                complexity: TaskComplexity::Simple,
                requirements: ResourceRequirements::default(),
                complexity_score: 10.0,
                suggested_mode: ExecutionMode::Local,
                confidence: 0.8,
                notes: vec![],
            },
            capabilities: CapabilityCheckResult {
                local: limited_local_caps(),
                cloud: full_cloud_caps(),
                can_execute_local: true,
                can_execute_cloud: true,
                local_unavailable_reason: None,
                cloud_unavailable_reason: None,
            },
            cost_estimate: CostEstimate::default(),
            user_preferences: UserPreferences::default(),
            decision_factors: vec![],
            decision_reason: "Test".to_string(),
            outcome: None,
        };

        logger.log_decision(entry.clone()).await.unwrap();

        let retrieved = logger.get_decision("test-1").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().task_id, "task-1");
    }

    #[tokio::test]
    async fn test_decision_logger_update_outcome() {
        let logger = InMemoryDecisionLogger::new(100);

        let entry = DecisionLogEntry {
            id: "test-2".to_string(),
            timestamp: Utc::now(),
            task_id: "task-2".to_string(),
            selected_mode: ExecutionMode::Local,
            task_analysis: TaskAnalysis {
                task_id: "task-2".to_string(),
                category: TaskCategory::CodeExecution,
                complexity: TaskComplexity::Simple,
                requirements: ResourceRequirements::default(),
                complexity_score: 10.0,
                suggested_mode: ExecutionMode::Local,
                confidence: 0.8,
                notes: vec![],
            },
            capabilities: CapabilityCheckResult {
                local: limited_local_caps(),
                cloud: full_cloud_caps(),
                can_execute_local: true,
                can_execute_cloud: true,
                local_unavailable_reason: None,
                cloud_unavailable_reason: None,
            },
            cost_estimate: CostEstimate::default(),
            user_preferences: UserPreferences::default(),
            decision_factors: vec![],
            decision_reason: "Test".to_string(),
            outcome: None,
        };

        logger.log_decision(entry).await.unwrap();

        let outcome = ExecutionOutcome {
            success: true,
            actual_time_ms: 1000,
            actual_cost: 0.01,
            error: None,
        };

        logger.update_outcome("test-2", outcome).await.unwrap();

        let retrieved = logger.get_decision("test-2").await.unwrap().unwrap();
        assert!(retrieved.outcome.is_some());
        assert!(retrieved.outcome.unwrap().success);
    }

    #[tokio::test]
    async fn test_decision_logger_statistics() {
        let logger = InMemoryDecisionLogger::new(100);

        // Log some decisions
        for i in 0..5 {
            let entry = DecisionLogEntry {
                id: format!("stat-{}", i),
                timestamp: Utc::now(),
                task_id: format!("task-{}", i),
                selected_mode: if i % 2 == 0 {
                    ExecutionMode::Local
                } else {
                    ExecutionMode::Cloud
                },
                task_analysis: TaskAnalysis {
                    task_id: format!("task-{}", i),
                    category: TaskCategory::CodeExecution,
                    complexity: TaskComplexity::Simple,
                    requirements: ResourceRequirements::default(),
                    complexity_score: 10.0,
                    suggested_mode: ExecutionMode::Local,
                    confidence: 0.8,
                    notes: vec![],
                },
                capabilities: CapabilityCheckResult {
                    local: limited_local_caps(),
                    cloud: full_cloud_caps(),
                    can_execute_local: true,
                    can_execute_cloud: true,
                    local_unavailable_reason: None,
                    cloud_unavailable_reason: None,
                },
                cost_estimate: CostEstimate::default(),
                user_preferences: UserPreferences::default(),
                decision_factors: vec![],
                decision_reason: "Test".to_string(),
                outcome: None,
            };
            logger.log_decision(entry).await.unwrap();
        }

        let stats = logger.get_statistics().await.unwrap();
        assert_eq!(stats.total_decisions, 5);
        assert_eq!(stats.local_decisions, 3);
        assert_eq!(stats.cloud_decisions, 2);
    }

    // ========== UserPreferences Tests ==========

    #[test]
    fn test_user_preferences_builder() {
        let prefs = UserPreferences::new(OptimizationGoal::Performance)
            .with_max_cost(10.0)
            .with_max_time(60000)
            .with_factor_weight("cost", 0.5);

        assert_eq!(prefs.optimization_goal, OptimizationGoal::Performance);
        assert_eq!(prefs.max_cost, Some(10.0));
        assert_eq!(prefs.max_time_ms, Some(60000));
        assert_eq!(prefs.factor_weights.get("cost"), Some(&0.5));
    }

    #[test]
    fn test_user_preferences_local_only() {
        let prefs = UserPreferences::default().local_only();
        assert!(!prefs.allow_cloud);
    }

    #[test]
    fn test_user_preferences_forced_mode() {
        let prefs = UserPreferences::default().with_forced_mode(ExecutionMode::Cloud);
        assert_eq!(prefs.forced_mode, Some(ExecutionMode::Cloud));
    }

    // ========== AutoModeSelector Tests ==========

    #[tokio::test]
    async fn test_auto_mode_selector_simple_task_selects_local() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        let request = ModeSelectionRequest::new(simple_task());
        let result = selector.select_mode(request).await.unwrap();

        // Simple task should prefer local
        assert_eq!(result.selected_mode, ExecutionMode::Local);
        assert!(result.confidence > 0.5);
    }

    #[tokio::test]
    async fn test_auto_mode_selector_gpu_task_selects_cloud() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        let request = ModeSelectionRequest::new(complex_task());
        let result = selector.select_mode(request).await.unwrap();

        // GPU task should select cloud since local doesn't have GPU
        assert_eq!(result.selected_mode, ExecutionMode::Cloud);
        assert!(result.capabilities.can_execute_cloud);
        assert!(!result.capabilities.can_execute_local);
    }

    #[tokio::test]
    async fn test_auto_mode_selector_forced_mode() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        let prefs = UserPreferences::default().with_forced_mode(ExecutionMode::Cloud);
        let request = ModeSelectionRequest::new(simple_task()).with_preferences(prefs);
        let result = selector.select_mode(request).await.unwrap();

        // Should use forced mode
        assert_eq!(result.selected_mode, ExecutionMode::Cloud);
        assert_eq!(result.confidence, 1.0);
    }

    #[tokio::test]
    async fn test_auto_mode_selector_cost_optimization() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        let prefs = UserPreferences::new(OptimizationGoal::Cost);
        let request = ModeSelectionRequest::new(simple_task()).with_preferences(prefs);
        let result = selector.select_mode(request).await.unwrap();

        // Cost optimization should prefer local (cheaper)
        assert_eq!(result.selected_mode, ExecutionMode::Local);
    }

    #[tokio::test]
    async fn test_auto_mode_selector_logs_decision() {
        let logger = Arc::new(InMemoryDecisionLogger::new(100));
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .decision_logger(logger.clone())
            .build();

        let request = ModeSelectionRequest::new(simple_task());
        let result = selector.select_mode(request).await.unwrap();

        let decisions = logger.get_recent_decisions(10).await.unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].id, result.decision_id);
    }

    #[tokio::test]
    async fn test_auto_mode_selector_local_first_goal() {
        let mut cloud_caps = full_cloud_caps();
        cloud_caps.latency_ms = 10; // Make cloud faster than normal

        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                cloud_caps,
            )))
            .build();

        let prefs = UserPreferences::new(OptimizationGoal::LocalFirst);
        let request = ModeSelectionRequest::new(simple_task()).with_preferences(prefs);
        let result = selector.select_mode(request).await.unwrap();

        // LocalFirst should prefer local
        assert_eq!(result.selected_mode, ExecutionMode::Local);
    }

    #[tokio::test]
    async fn test_auto_mode_selector_cloud_first_goal() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        let prefs = UserPreferences::new(OptimizationGoal::CloudFirst);
        let request = ModeSelectionRequest::new(simple_task()).with_preferences(prefs);
        let result = selector.select_mode(request).await.unwrap();

        // CloudFirst should prefer cloud
        assert_eq!(result.selected_mode, ExecutionMode::Cloud);
    }

    #[tokio::test]
    async fn test_auto_mode_selector_no_cloud_allowed() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        let prefs = UserPreferences::default().local_only();
        let request = ModeSelectionRequest::new(simple_task()).with_preferences(prefs);
        let result = selector.select_mode(request).await.unwrap();

        assert_eq!(result.selected_mode, ExecutionMode::Local);
    }

    #[tokio::test]
    async fn test_auto_mode_selector_neither_available() {
        let mut local_caps = limited_local_caps();
        local_caps.healthy = false;

        let mut cloud_caps = full_cloud_caps();
        cloud_caps.healthy = false;

        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                local_caps, cloud_caps,
            )))
            .default_mode(ExecutionMode::Local)
            .build();

        let request = ModeSelectionRequest::new(simple_task());
        let result = selector.select_mode(request).await.unwrap();

        // Should fall back to default mode with low confidence
        assert_eq!(result.selected_mode, ExecutionMode::Local);
        assert_eq!(result.confidence, 0.0);
    }

    #[tokio::test]
    async fn test_auto_mode_selector_alternatives() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        let request = ModeSelectionRequest::new(simple_task());
        let result = selector.select_mode(request).await.unwrap();

        // Should have alternatives since both are available
        assert!(!result.alternatives.is_empty());
    }

    #[tokio::test]
    async fn test_auto_mode_selector_decision_factors() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        let request = ModeSelectionRequest::new(simple_task());
        let result = selector.select_mode(request).await.unwrap();

        // Should have multiple decision factors
        assert!(!result.factors.is_empty());

        // Should include cost, performance, reliability, availability
        let factor_names: Vec<_> = result.factors.iter().map(|f| f.name.as_str()).collect();
        assert!(factor_names.contains(&"cost"));
        assert!(factor_names.contains(&"performance"));
    }

    // ========== Builder Tests ==========

    #[test]
    fn test_auto_mode_selector_builder() {
        let selector = AutoModeSelector::builder()
            .cost_weight(0.5)
            .performance_weight(0.3)
            .default_mode(ExecutionMode::Cloud)
            .build();

        assert_eq!(selector.config.cost_weight, 0.5);
        assert_eq!(selector.config.performance_weight, 0.3);
        assert_eq!(selector.config.default_mode, ExecutionMode::Cloud);
    }

    #[tokio::test]
    async fn test_mode_selector_trait_access() {
        let selector = AutoModeSelector::builder().build();

        // Should be able to access components via trait
        let _ = selector.task_analyzer();
        let _ = selector.capability_checker();
        let _ = selector.cost_estimator();
        let _ = selector.decision_logger();
    }

    // ========== Integration-like Tests ==========

    #[tokio::test]
    async fn test_full_workflow_simple_task() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        // Create and analyze task
        let task = TaskInfo::new(TaskCategory::CodeExecution, "Print hello world")
            .with_code("print('Hello, World!')", "python");

        let request = ModeSelectionRequest::new(task);
        let result = selector.select_mode(request).await.unwrap();

        assert!(!result.decision_id.is_empty());
        assert!(result.confidence > 0.0);
        assert!(!result.reason.is_empty());
    }

    #[tokio::test]
    async fn test_full_workflow_complex_task() {
        let selector = AutoModeSelector::builder()
            .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                limited_local_caps(),
                full_cloud_caps(),
            )))
            .build();

        // Create complex ML task
        let task = TaskInfo::new(TaskCategory::MachineLearning, "Train neural network").with_code(
            r#"
                import torch
                import torch.nn as nn
                model = nn.Sequential(
                    nn.Linear(784, 256),
                    nn.ReLU(),
                    nn.Linear(256, 10)
                )
                optimizer = torch.optim.Adam(model.parameters())
                for epoch in range(100):
                    # Training loop
                    pass
                "#,
            "python",
        );

        let request = ModeSelectionRequest::new(task);
        let result = selector.select_mode(request).await.unwrap();

        // Complex ML task should prefer cloud
        assert_eq!(result.selected_mode, ExecutionMode::Cloud);
    }

    // =========================================================================
    // Collaboration Mode Selection Tests (require "collaboration" feature)
    // =========================================================================

    #[cfg(feature = "collaboration")]
    mod collaboration_mode_tests {
        use super::*;
        use crate::collaboration::CollaborationMode;

        #[test]
        fn test_simple_task_uses_direct_mode() {
            let selector = AutoModeSelector::builder()
                .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                    limited_local_caps(),
                    full_cloud_caps(),
                )))
                .build();

            // Simple task with no resource requirements
            let task = TaskInfo::new(TaskCategory::Generic, "Hello world");
            let request = ModeSelectionRequest::new(task);

            let mode = selector.select_collaboration_mode(&request);
            assert!(matches!(mode, CollaborationMode::Direct));
        }

        #[test]
        fn test_complex_code_task_uses_expert_mode() {
            let selector = AutoModeSelector::builder()
                .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                    limited_local_caps(),
                    full_cloud_caps(),
                )))
                .build();

            // Complex task with high resource requirements
            let task = TaskInfo::new(TaskCategory::CodeExecution, "Build complex system")
                .with_requirements(
                    ResourceRequirements::new()
                        .with_cpu(8.0)
                        .with_memory(16384)
                        .with_time(3600000), // 1 hour
                );
            let request = ModeSelectionRequest::new(task);

            let mode = selector.select_collaboration_mode(&request);
            if let CollaborationMode::Expert {
                supervisor,
                specialists,
                ..
            } = mode
            {
                assert_eq!(supervisor, "code_architect");
                assert!(specialists.contains(&"code_implementer".to_string()));
            } else {
                panic!("Expected Expert mode for complex code task");
            }
        }

        #[test]
        fn test_moderate_browser_task_uses_swarm_mode() {
            let selector = AutoModeSelector::builder()
                .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                    limited_local_caps(),
                    full_cloud_caps(),
                )))
                .build();

            // Moderate browser task (cpu 1.5 < complex_cpu 2.0, memory 1024 < complex_memory 2048)
            let task = TaskInfo::new(TaskCategory::BrowserAutomation, "Navigate and click")
                .with_requirements(
                    ResourceRequirements::new()
                        .with_cpu(1.5)
                        .with_memory(1024)
                        .with_time(10000), // 10 seconds (moderate range)
                );
            let request = ModeSelectionRequest::new(task);

            let mode = selector.select_collaboration_mode(&request);
            if let CollaborationMode::Swarm {
                initial_agent,
                handoff_rules,
                ..
            } = mode
            {
                assert_eq!(initial_agent, "browser_navigator");
                assert!(!handoff_rules.is_empty());
            } else {
                panic!("Expected Swarm mode for moderate browser task");
            }
        }

        #[test]
        fn test_intensive_ml_task_uses_expert_mode() {
            let selector = AutoModeSelector::builder()
                .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                    limited_local_caps(),
                    full_cloud_caps(),
                )))
                .build();

            // Intensive ML task with GPU
            let task = TaskInfo::new(TaskCategory::MachineLearning, "Train large model")
                .with_requirements(
                    ResourceRequirements::new()
                        .with_cpu(16.0)
                        .with_memory(65536)
                        .with_gpu(true)
                        .with_time(7200000), // 2 hours
                );
            let request = ModeSelectionRequest::new(task);

            let mode = selector.select_collaboration_mode(&request);
            if let CollaborationMode::Expert {
                supervisor,
                specialists,
                ..
            } = mode
            {
                assert_eq!(supervisor, "ml_architect");
                assert!(specialists.contains(&"model_trainer".to_string()));
            } else {
                panic!("Expected Expert mode for intensive ML task");
            }
        }

        #[test]
        fn test_suggest_collaboration_mode_simple_text() {
            let selector = AutoModeSelector::builder()
                .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                    limited_local_caps(),
                    full_cloud_caps(),
                )))
                .build();

            let mode = selector.suggest_collaboration_mode_for_text("Print hello world");
            assert!(matches!(mode, CollaborationMode::Direct));
        }

        #[test]
        fn test_suggest_collaboration_mode_workflow_text() {
            let selector = AutoModeSelector::builder()
                .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                    limited_local_caps(),
                    full_cloud_caps(),
                )))
                .build();

            let mode = selector.suggest_collaboration_mode_for_text(
                "First analyze the data, then create a report",
            );
            assert!(matches!(mode, CollaborationMode::Swarm { .. }));
        }

        #[test]
        fn test_suggest_collaboration_mode_expert_text() {
            let selector = AutoModeSelector::builder()
                .capability_checker(Arc::new(DefaultCapabilityChecker::new(
                    limited_local_caps(),
                    full_cloud_caps(),
                )))
                .build();

            let mode = selector.suggest_collaboration_mode_for_text(
                "This is a complex task that needs review and validation by specialists",
            );
            if let CollaborationMode::Expert { supervisor, .. } = mode {
                assert_eq!(supervisor, "coordinator");
            } else {
                panic!("Expected Expert mode for complex specialist task");
            }
        }
    }
}
