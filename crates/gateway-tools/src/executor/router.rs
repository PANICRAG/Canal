//! Unified CodeAct Router
//!
//! This module provides a unified router that integrates both local Docker container
//! execution and cloud-based VM code execution into a single interface.
//!
//! # Features
//!
//! - **Multiple routing modes**: Local-only, cloud-only, prefer-local, prefer-cloud, or load-balanced
//! - **Automatic failover**: Code execution can automatically fail over to backup executors
//! - **Parallel execution**: Support for running multiple code executions in parallel
//! - **Resource quota management**: Track and enforce resource quotas across executions
//! - **Health monitoring**: Continuous health checks with automatic status updates
//! - **Metrics tracking**: Request counts, failure rates, and latency tracking
//!
//! # Architecture
//!
//! ```text
//! UnifiedCodeActRouter
//!        |
//!        |-- RouterMode ---> Determines routing strategy
//!        |
//!        |-- LocalExecutionStrategy ---> Docker container execution
//!        |
//!        |-- CloudExecutionStrategy ---> VM-based execution
//!        |
//!        |-- ResourceTracker -------> Track and enforce quotas
//!        |
//!        |-- Metrics -------> Track health and performance
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::executor::router::{
//!     UnifiedCodeActRouter, RouterConfig, RouterMode, LoadBalanceStrategy,
//! };
//!
//! // Create unified router with both local and cloud backends
//! let router = UnifiedCodeActRouter::builder()
//!     .local(local_executor)
//!     .cloud(cloud_executor)
//!     .mode(RouterMode::PreferLocal)
//!     .fallback_enabled(true)
//!     .build();
//!
//! // Execute code - router handles routing and failover
//! let request = ExecutionRequest { code: "print('hello')".into(), .. };
//! let result = router.execute(request).await?;
//! ```

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use super::result::CodeActResult;
use crate::error::{ServiceError as Error, ServiceResult as Result};

/// Router health status values
const STATUS_UNKNOWN: u8 = 0;
const STATUS_HEALTHY: u8 = 1;
const STATUS_UNHEALTHY: u8 = 2;

// ============================================================================
// Router Mode
// ============================================================================

/// Routing mode for the unified CodeAct router
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RouterMode {
    /// Only use local Docker execution
    LocalOnly,
    /// Only use cloud-based VM execution
    CloudOnly,
    /// Prefer local, fallback to cloud on failure
    #[default]
    PreferLocal,
    /// Prefer cloud, fallback to local on failure
    PreferCloud,
    /// Load balance between local and cloud
    LoadBalance,
}

impl std::fmt::Display for RouterMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouterMode::LocalOnly => write!(f, "local_only"),
            RouterMode::CloudOnly => write!(f, "cloud_only"),
            RouterMode::PreferLocal => write!(f, "prefer_local"),
            RouterMode::PreferCloud => write!(f, "prefer_cloud"),
            RouterMode::LoadBalance => write!(f, "load_balance"),
        }
    }
}

// ============================================================================
// Load Balance Strategy
// ============================================================================

/// Load balancing strategy for the unified router
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalanceStrategy {
    /// Round-robin between available executors
    #[default]
    RoundRobin,
    /// Route to the executor with fewer active executions
    LeastConnections,
    /// Route based on recent response times
    ResponseTime,
    /// Weighted routing based on executor capacity
    Weighted,
    /// Resource-aware routing based on available quota
    ResourceAware,
}

impl std::fmt::Display for LoadBalanceStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadBalanceStrategy::RoundRobin => write!(f, "round_robin"),
            LoadBalanceStrategy::LeastConnections => write!(f, "least_connections"),
            LoadBalanceStrategy::ResponseTime => write!(f, "response_time"),
            LoadBalanceStrategy::Weighted => write!(f, "weighted"),
            LoadBalanceStrategy::ResourceAware => write!(f, "resource_aware"),
        }
    }
}

// ============================================================================
// Execution Strategy Trait
// ============================================================================

/// Code execution request for the router
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecutionRequest {
    /// Unique request identifier
    pub id: String,
    /// Code to execute
    pub code: String,
    /// Programming language
    pub language: String,
    /// Execution timeout in milliseconds
    pub timeout_ms: u64,
    /// Working directory (optional)
    pub working_dir: Option<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Whether to stream output
    pub stream: bool,
    /// Session ID for stateful execution
    pub session_id: Option<String>,
    /// Required CPU cores
    pub required_cpu: f64,
    /// Required memory in MB
    pub required_memory_mb: u64,
}

impl CodeExecutionRequest {
    /// Create a new execution request
    pub fn new(code: impl Into<String>, language: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            code: code.into(),
            language: language.into(),
            timeout_ms: 30000,
            working_dir: None,
            env: HashMap::new(),
            stream: false,
            session_id: None,
            required_cpu: 1.0,
            required_memory_mb: 512,
        }
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Set working directory
    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Add environment variable
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set session ID
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set resource requirements
    pub fn with_resources(mut self, cpu: f64, memory_mb: u64) -> Self {
        self.required_cpu = cpu;
        self.required_memory_mb = memory_mb;
        self
    }

    /// Enable streaming
    pub fn with_streaming(mut self, enabled: bool) -> Self {
        self.stream = enabled;
        self
    }
}

impl Default for CodeExecutionRequest {
    fn default() -> Self {
        Self::new("", "python")
    }
}

/// Trait for execution strategies (local Docker, cloud VM, etc.)
#[async_trait]
pub trait ExecutionStrategy: Send + Sync {
    /// Get the name of this strategy
    fn name(&self) -> &str;

    /// Check if the executor is available
    async fn is_available(&self) -> bool;

    /// Get the number of active executions
    async fn active_executions(&self) -> usize;

    /// Get available resources
    async fn available_resources(&self) -> AvailableResources;

    /// Execute code and return the result
    async fn execute(&self, request: CodeExecutionRequest) -> Result<CodeActResult>;

    /// Cancel an execution by ID
    async fn cancel(&self, execution_id: &str) -> Result<()>;

    /// Health check
    async fn health_check(&self) -> Result<bool>;
}

/// Available resources for an executor
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AvailableResources {
    /// Available CPU cores
    pub cpu_cores: f64,
    /// Available memory in MB
    pub memory_mb: u64,
    /// Available execution slots
    pub execution_slots: usize,
    /// Current utilization percentage (0-100)
    pub utilization_percent: f64,
}

// ============================================================================
// Local Execution Strategy (Docker)
// ============================================================================

/// Local Docker-based execution strategy
pub struct LocalExecutionStrategy {
    /// Strategy name
    name: String,
    /// Maximum concurrent executions
    max_concurrent: usize,
    /// Current active executions
    active: Arc<RwLock<HashMap<String, ExecutionInfo>>>,
    /// Total CPU cores available
    total_cpu: f64,
    /// Total memory in MB
    total_memory_mb: u64,
    /// Whether the executor is enabled
    enabled: Arc<RwLock<bool>>,
}

/// Information about an active execution
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ExecutionInfo {
    id: String,
    started_at: DateTime<Utc>,
    cpu_reserved: f64,
    memory_reserved: u64,
}

impl LocalExecutionStrategy {
    /// Create a new local execution strategy
    pub fn new(max_concurrent: usize, total_cpu: f64, total_memory_mb: u64) -> Self {
        Self {
            name: "local_docker".to_string(),
            max_concurrent,
            active: Arc::new(RwLock::new(HashMap::new())),
            total_cpu,
            total_memory_mb,
            enabled: Arc::new(RwLock::new(true)),
        }
    }

    /// Set enabled state
    pub async fn set_enabled(&self, enabled: bool) {
        let mut state = self.enabled.write().await;
        *state = enabled;
    }

    /// Get reserved resources
    async fn reserved_resources(&self) -> (f64, u64) {
        let active = self.active.read().await;
        let cpu: f64 = active.values().map(|e| e.cpu_reserved).sum();
        let memory: u64 = active.values().map(|e| e.memory_reserved).sum();
        (cpu, memory)
    }
}

#[async_trait]
impl ExecutionStrategy for LocalExecutionStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        let enabled = *self.enabled.read().await;
        if !enabled {
            return false;
        }
        let active = self.active.read().await;
        active.len() < self.max_concurrent
    }

    async fn active_executions(&self) -> usize {
        self.active.read().await.len()
    }

    async fn available_resources(&self) -> AvailableResources {
        let (reserved_cpu, reserved_memory) = self.reserved_resources().await;
        let active_count = self.active.read().await.len();

        AvailableResources {
            cpu_cores: (self.total_cpu - reserved_cpu).max(0.0),
            memory_mb: self.total_memory_mb.saturating_sub(reserved_memory),
            execution_slots: self.max_concurrent.saturating_sub(active_count),
            utilization_percent: (active_count as f64 / self.max_concurrent as f64) * 100.0,
        }
    }

    #[instrument(skip(self, request), fields(request_id = %request.id))]
    async fn execute(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        // Check availability
        if !self.is_available().await {
            return Err(Error::Internal("Local executor at capacity".into()));
        }

        // Check resource availability
        let (reserved_cpu, reserved_memory) = self.reserved_resources().await;
        if reserved_cpu + request.required_cpu > self.total_cpu {
            return Err(Error::Internal("Insufficient CPU resources".into()));
        }
        if reserved_memory + request.required_memory_mb > self.total_memory_mb {
            return Err(Error::Internal("Insufficient memory resources".into()));
        }

        let execution_id = request.id.clone();
        let info = ExecutionInfo {
            id: execution_id.clone(),
            started_at: Utc::now(),
            cpu_reserved: request.required_cpu,
            memory_reserved: request.required_memory_mb,
        };

        // Register execution
        {
            let mut active = self.active.write().await;
            active.insert(execution_id.clone(), info);
        }

        let start = Instant::now();

        // Simulate execution (in real implementation, this would call ContainerManager)
        // For now, we'll create a mock result
        let result = tokio::time::timeout(
            Duration::from_millis(request.timeout_ms),
            self.do_execute(&request),
        )
        .await;

        // Unregister execution
        {
            let mut active = self.active.write().await;
            active.remove(&execution_id);
        }

        match result {
            Ok(Ok(mut result)) => {
                result.timing.total_ms = start.elapsed().as_millis() as u64;
                Ok(result)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(CodeActResult::timeout(&execution_id, request.timeout_ms)),
        }
    }

    async fn cancel(&self, execution_id: &str) -> Result<()> {
        let mut active = self.active.write().await;
        if active.remove(execution_id).is_some() {
            debug!("Cancelled execution: {}", execution_id);
            Ok(())
        } else {
            Err(Error::NotFound(format!(
                "Execution not found: {}",
                execution_id
            )))
        }
    }

    async fn health_check(&self) -> Result<bool> {
        // In real implementation, check Docker daemon health
        Ok(*self.enabled.read().await)
    }
}

impl LocalExecutionStrategy {
    /// Actual execution implementation
    async fn do_execute(&self, request: &CodeExecutionRequest) -> Result<CodeActResult> {
        debug!(
            "Executing code locally: {} bytes in {}",
            request.code.len(),
            request.language
        );

        // In a real implementation, this would:
        // 1. Get a container from the pool
        // 2. Execute the code
        // 3. Parse the result
        // For now, we create a success result
        let mut result = CodeActResult::success(&request.id, "Execution completed successfully");
        result
            .metadata
            .insert("executor".to_string(), serde_json::json!("local_docker"));
        result
            .metadata
            .insert("language".to_string(), serde_json::json!(&request.language));

        Ok(result)
    }
}

// ============================================================================
// Cloud Execution Strategy (VM)
// ============================================================================

/// Cloud VM-based execution strategy
pub struct CloudExecutionStrategy {
    /// Strategy name
    name: String,
    /// API endpoint for cloud execution
    endpoint: String,
    /// Maximum concurrent executions
    max_concurrent: usize,
    /// Current active executions
    active: Arc<RwLock<HashMap<String, ExecutionInfo>>>,
    /// Total CPU cores available
    total_cpu: f64,
    /// Total memory in MB
    total_memory_mb: u64,
    /// Whether the executor is enabled
    enabled: Arc<RwLock<bool>>,
    /// Weight for load balancing (higher = more traffic)
    weight: u32,
}

impl CloudExecutionStrategy {
    /// Create a new cloud execution strategy
    pub fn new(
        endpoint: impl Into<String>,
        max_concurrent: usize,
        total_cpu: f64,
        total_memory_mb: u64,
    ) -> Self {
        Self {
            name: "cloud_vm".to_string(),
            endpoint: endpoint.into(),
            max_concurrent,
            active: Arc::new(RwLock::new(HashMap::new())),
            total_cpu,
            total_memory_mb,
            enabled: Arc::new(RwLock::new(true)),
            weight: 100,
        }
    }

    /// Set enabled state
    pub async fn set_enabled(&self, enabled: bool) {
        let mut state = self.enabled.write().await;
        *state = enabled;
    }

    /// Set weight for load balancing
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight;
        self
    }

    /// Get the weight
    pub fn weight(&self) -> u32 {
        self.weight
    }

    /// Get reserved resources
    async fn reserved_resources(&self) -> (f64, u64) {
        let active = self.active.read().await;
        let cpu: f64 = active.values().map(|e| e.cpu_reserved).sum();
        let memory: u64 = active.values().map(|e| e.memory_reserved).sum();
        (cpu, memory)
    }
}

#[async_trait]
impl ExecutionStrategy for CloudExecutionStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        let enabled = *self.enabled.read().await;
        if !enabled {
            return false;
        }
        let active = self.active.read().await;
        active.len() < self.max_concurrent
    }

    async fn active_executions(&self) -> usize {
        self.active.read().await.len()
    }

    async fn available_resources(&self) -> AvailableResources {
        let (reserved_cpu, reserved_memory) = self.reserved_resources().await;
        let active_count = self.active.read().await.len();

        AvailableResources {
            cpu_cores: (self.total_cpu - reserved_cpu).max(0.0),
            memory_mb: self.total_memory_mb.saturating_sub(reserved_memory),
            execution_slots: self.max_concurrent.saturating_sub(active_count),
            utilization_percent: (active_count as f64 / self.max_concurrent as f64) * 100.0,
        }
    }

    #[instrument(skip(self, request), fields(request_id = %request.id, endpoint = %self.endpoint))]
    async fn execute(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        // Check availability
        if !self.is_available().await {
            return Err(Error::Internal("Cloud executor at capacity".into()));
        }

        let execution_id = request.id.clone();
        let info = ExecutionInfo {
            id: execution_id.clone(),
            started_at: Utc::now(),
            cpu_reserved: request.required_cpu,
            memory_reserved: request.required_memory_mb,
        };

        // Register execution
        {
            let mut active = self.active.write().await;
            active.insert(execution_id.clone(), info);
        }

        let start = Instant::now();

        // Simulate execution (in real implementation, this would call cloud API)
        let result = tokio::time::timeout(
            Duration::from_millis(request.timeout_ms),
            self.do_execute(&request),
        )
        .await;

        // Unregister execution
        {
            let mut active = self.active.write().await;
            active.remove(&execution_id);
        }

        match result {
            Ok(Ok(mut result)) => {
                result.timing.total_ms = start.elapsed().as_millis() as u64;
                Ok(result)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(CodeActResult::timeout(&execution_id, request.timeout_ms)),
        }
    }

    async fn cancel(&self, execution_id: &str) -> Result<()> {
        let mut active = self.active.write().await;
        if active.remove(execution_id).is_some() {
            debug!("Cancelled cloud execution: {}", execution_id);
            Ok(())
        } else {
            Err(Error::NotFound(format!(
                "Execution not found: {}",
                execution_id
            )))
        }
    }

    async fn health_check(&self) -> Result<bool> {
        // In real implementation, ping the cloud endpoint
        Ok(*self.enabled.read().await)
    }
}

impl CloudExecutionStrategy {
    /// Actual execution implementation
    async fn do_execute(&self, request: &CodeExecutionRequest) -> Result<CodeActResult> {
        debug!(
            "Executing code in cloud: {} bytes in {}",
            request.code.len(),
            request.language
        );

        // In a real implementation, this would:
        // 1. Call the cloud VM API
        // 2. Wait for result
        // 3. Parse and return
        let mut result = CodeActResult::success(&request.id, "Cloud execution completed");
        result
            .metadata
            .insert("executor".to_string(), serde_json::json!("cloud_vm"));
        result
            .metadata
            .insert("endpoint".to_string(), serde_json::json!(&self.endpoint));
        result
            .metadata
            .insert("language".to_string(), serde_json::json!(&request.language));

        Ok(result)
    }
}

// ============================================================================
// Fallback Strategy
// ============================================================================

/// Fallback execution strategy that wraps primary and backup strategies
pub struct FallbackStrategy {
    /// Primary execution strategy
    primary: Arc<dyn ExecutionStrategy>,
    /// Backup execution strategy
    backup: Arc<dyn ExecutionStrategy>,
    /// Maximum retries on primary before fallback
    max_retries: u32,
    /// Name of this strategy
    name: String,
}

impl FallbackStrategy {
    /// Create a new fallback strategy
    pub fn new(
        primary: Arc<dyn ExecutionStrategy>,
        backup: Arc<dyn ExecutionStrategy>,
        max_retries: u32,
    ) -> Self {
        let name = format!("fallback({}/{})", primary.name(), backup.name());
        Self {
            primary,
            backup,
            max_retries,
            name,
        }
    }
}

#[async_trait]
impl ExecutionStrategy for FallbackStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    async fn is_available(&self) -> bool {
        self.primary.is_available().await || self.backup.is_available().await
    }

    async fn active_executions(&self) -> usize {
        self.primary.active_executions().await + self.backup.active_executions().await
    }

    async fn available_resources(&self) -> AvailableResources {
        let primary = self.primary.available_resources().await;
        let backup = self.backup.available_resources().await;

        // Combine resources from both
        AvailableResources {
            cpu_cores: primary.cpu_cores + backup.cpu_cores,
            memory_mb: primary.memory_mb + backup.memory_mb,
            execution_slots: primary.execution_slots + backup.execution_slots,
            utilization_percent: (primary.utilization_percent + backup.utilization_percent) / 2.0,
        }
    }

    #[instrument(skip(self, request), fields(request_id = %request.id))]
    async fn execute(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        let mut last_error = None;

        // Try primary with retries
        for attempt in 0..=self.max_retries {
            if self.primary.is_available().await {
                match self.primary.execute(request.clone()).await {
                    Ok(result) if result.is_success() => return Ok(result),
                    Ok(result) => {
                        warn!(
                            attempt = attempt,
                            "Primary execution failed with error result"
                        );
                        last_error = result.error.map(|e| Error::ExecutionFailed(e.message));
                    }
                    Err(e) => {
                        warn!(attempt = attempt, error = %e, "Primary execution failed");
                        last_error = Some(e);
                    }
                }
            }
        }

        // Try backup
        if self.backup.is_available().await {
            debug!("Falling back to backup executor");
            return self.backup.execute(request).await;
        }

        Err(last_error.unwrap_or_else(|| Error::Internal("No executor available".into())))
    }

    async fn cancel(&self, execution_id: &str) -> Result<()> {
        // Try to cancel on both
        let primary_result = self.primary.cancel(execution_id).await;
        let backup_result = self.backup.cancel(execution_id).await;

        if primary_result.is_ok() || backup_result.is_ok() {
            Ok(())
        } else {
            Err(Error::NotFound(format!(
                "Execution not found: {}",
                execution_id
            )))
        }
    }

    async fn health_check(&self) -> Result<bool> {
        let primary_health = self.primary.health_check().await.unwrap_or(false);
        let backup_health = self.backup.health_check().await.unwrap_or(false);
        Ok(primary_health || backup_health)
    }
}

// ============================================================================
// Resource Quota Management
// ============================================================================

/// Resource quota configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceQuota {
    /// Maximum CPU cores that can be used
    pub max_cpu_cores: f64,
    /// Maximum memory in MB
    pub max_memory_mb: u64,
    /// Maximum concurrent executions
    pub max_concurrent_executions: usize,
    /// Maximum execution time per request in milliseconds
    pub max_execution_time_ms: u64,
    /// Maximum total execution time per day in milliseconds
    pub max_daily_execution_time_ms: u64,
    /// User or tenant ID this quota applies to
    pub owner_id: Option<String>,
}

impl Default for ResourceQuota {
    fn default() -> Self {
        Self {
            max_cpu_cores: 4.0,
            max_memory_mb: 4096,
            max_concurrent_executions: 10,
            max_execution_time_ms: 60000,
            max_daily_execution_time_ms: 3600000, // 1 hour
            owner_id: None,
        }
    }
}

impl ResourceQuota {
    /// Create a new quota with custom limits
    pub fn new(max_cpu: f64, max_memory_mb: u64, max_concurrent: usize, max_time_ms: u64) -> Self {
        Self {
            max_cpu_cores: max_cpu,
            max_memory_mb,
            max_concurrent_executions: max_concurrent,
            max_execution_time_ms: max_time_ms,
            ..Default::default()
        }
    }

    /// Set owner ID
    pub fn with_owner(mut self, owner_id: impl Into<String>) -> Self {
        self.owner_id = Some(owner_id.into());
        self
    }
}

/// Resource usage tracking
#[derive(Debug, Clone, Default)]
pub struct ResourceUsage {
    /// Current CPU usage
    pub cpu_used: f64,
    /// Current memory usage in MB
    pub memory_used_mb: u64,
    /// Current concurrent executions
    pub concurrent_executions: usize,
    /// Total execution time today in milliseconds
    pub daily_execution_time_ms: u64,
    /// Last reset timestamp
    pub last_reset: Option<DateTime<Utc>>,
}

/// Tracks resource usage and enforces quotas
pub struct ResourceTracker {
    /// Resource quotas by owner ID (None = default)
    quotas: Arc<RwLock<HashMap<Option<String>, ResourceQuota>>>,
    /// Current usage by owner ID
    usage: Arc<RwLock<HashMap<Option<String>, ResourceUsage>>>,
}

impl ResourceTracker {
    /// Create a new resource tracker
    pub fn new() -> Self {
        Self {
            quotas: Arc::new(RwLock::new(HashMap::new())),
            usage: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set a quota for an owner (None = default quota)
    pub async fn set_quota(&self, owner_id: Option<String>, quota: ResourceQuota) {
        let mut quotas = self.quotas.write().await;
        quotas.insert(owner_id, quota);
    }

    /// Get quota for an owner
    pub async fn get_quota(&self, owner_id: &Option<String>) -> ResourceQuota {
        let quotas = self.quotas.read().await;
        quotas
            .get(owner_id)
            .or_else(|| quotas.get(&None))
            .cloned()
            .unwrap_or_default()
    }

    /// Get current usage for an owner
    pub async fn get_usage(&self, owner_id: &Option<String>) -> ResourceUsage {
        let usage = self.usage.read().await;
        usage.get(owner_id).cloned().unwrap_or_default()
    }

    /// Check if a request can be executed within quota
    pub async fn check_quota(
        &self,
        owner_id: &Option<String>,
        request: &CodeExecutionRequest,
    ) -> Result<()> {
        let quota = self.get_quota(owner_id).await;
        let usage = self.get_usage(owner_id).await;

        // Check concurrent executions
        if usage.concurrent_executions >= quota.max_concurrent_executions {
            return Err(Error::RateLimited(
                "concurrent execution limit reached".into(),
            ));
        }

        // Check CPU
        if usage.cpu_used + request.required_cpu > quota.max_cpu_cores {
            return Err(Error::Internal("CPU quota exceeded".into()));
        }

        // Check memory
        if usage.memory_used_mb + request.required_memory_mb > quota.max_memory_mb {
            return Err(Error::Internal("Memory quota exceeded".into()));
        }

        // Check execution time
        if request.timeout_ms > quota.max_execution_time_ms {
            return Err(Error::Internal("Execution time exceeds quota".into()));
        }

        // Check daily execution time
        if usage.daily_execution_time_ms + request.timeout_ms > quota.max_daily_execution_time_ms {
            return Err(Error::Internal(
                "Daily execution time quota exceeded".into(),
            ));
        }

        Ok(())
    }

    /// Reserve resources for an execution
    pub async fn reserve(&self, owner_id: &Option<String>, request: &CodeExecutionRequest) {
        let mut usage = self.usage.write().await;
        let entry = usage.entry(owner_id.clone()).or_default();

        entry.cpu_used += request.required_cpu;
        entry.memory_used_mb += request.required_memory_mb;
        entry.concurrent_executions += 1;
    }

    /// Release resources after execution
    pub async fn release(
        &self,
        owner_id: &Option<String>,
        request: &CodeExecutionRequest,
        execution_time_ms: u64,
    ) {
        let mut usage = self.usage.write().await;
        if let Some(entry) = usage.get_mut(owner_id) {
            entry.cpu_used = (entry.cpu_used - request.required_cpu).max(0.0);
            entry.memory_used_mb = entry
                .memory_used_mb
                .saturating_sub(request.required_memory_mb);
            entry.concurrent_executions = entry.concurrent_executions.saturating_sub(1);
            entry.daily_execution_time_ms += execution_time_ms;
        }
    }

    /// Reset daily usage counters
    pub async fn reset_daily_usage(&self) {
        let mut usage = self.usage.write().await;
        let now = Utc::now();

        for entry in usage.values_mut() {
            entry.daily_execution_time_ms = 0;
            entry.last_reset = Some(now);
        }
    }
}

impl Default for ResourceTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Enforces resource quotas
pub struct QuotaEnforcer {
    /// Resource tracker
    tracker: Arc<ResourceTracker>,
}

impl QuotaEnforcer {
    /// Create a new quota enforcer
    pub fn new(tracker: Arc<ResourceTracker>) -> Self {
        Self { tracker }
    }

    /// Check and reserve resources, returning a guard that releases on drop
    pub async fn acquire(
        &self,
        owner_id: &Option<String>,
        request: &CodeExecutionRequest,
    ) -> Result<QuotaGuard> {
        // Check quota
        self.tracker.check_quota(owner_id, request).await?;

        // Reserve resources
        self.tracker.reserve(owner_id, request).await;

        Ok(QuotaGuard {
            tracker: self.tracker.clone(),
            owner_id: owner_id.clone(),
            request: request.clone(),
            released: false,
        })
    }
}

/// Guard that releases quota when dropped
pub struct QuotaGuard {
    tracker: Arc<ResourceTracker>,
    owner_id: Option<String>,
    request: CodeExecutionRequest,
    released: bool,
}

impl QuotaGuard {
    /// Release the quota with actual execution time
    pub async fn release(mut self, execution_time_ms: u64) {
        self.tracker
            .release(&self.owner_id, &self.request, execution_time_ms)
            .await;
        self.released = true;
    }
}

impl Drop for QuotaGuard {
    fn drop(&mut self) {
        if !self.released {
            // Create a task to release resources if not already released
            let tracker = self.tracker.clone();
            let owner_id = self.owner_id.clone();
            let request = self.request.clone();

            tokio::spawn(async move {
                tracker.release(&owner_id, &request, 0).await;
            });
        }
    }
}

// ============================================================================
// Router Metrics
// ============================================================================

/// Metrics tracking for the router
#[derive(Debug, Default)]
pub struct RouterMetrics {
    /// Local executor health status (0=unknown, 1=healthy, 2=unhealthy)
    local_status: AtomicU8,
    /// Cloud executor health status
    cloud_status: AtomicU8,
    /// Local executor average latency in milliseconds
    local_latency_ms: AtomicU64,
    /// Cloud executor average latency in milliseconds
    cloud_latency_ms: AtomicU64,
    /// Total requests processed
    requests_total: AtomicU64,
    /// Total failed requests
    failures_total: AtomicU64,
    /// Local requests count
    local_requests: AtomicU64,
    /// Cloud requests count
    cloud_requests: AtomicU64,
    /// Local failures count
    local_failures: AtomicU64,
    /// Cloud failures count
    cloud_failures: AtomicU64,
    /// Round-robin counter for load balancing
    round_robin_counter: AtomicU64,
    /// Local consecutive failures (for health tracking)
    local_consecutive_failures: AtomicU64,
    /// Cloud consecutive failures (for health tracking)
    cloud_consecutive_failures: AtomicU64,
    /// Local consecutive successes (for health tracking)
    local_consecutive_successes: AtomicU64,
    /// Cloud consecutive successes (for health tracking)
    cloud_consecutive_successes: AtomicU64,
}

impl RouterMetrics {
    /// Create new metrics with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Get local executor status
    pub fn local_status(&self) -> u8 {
        self.local_status.load(Ordering::SeqCst)
    }

    /// Get cloud executor status
    pub fn cloud_status(&self) -> u8 {
        self.cloud_status.load(Ordering::SeqCst)
    }

    /// Check if local executor is healthy
    pub fn is_local_healthy(&self) -> bool {
        self.local_status.load(Ordering::SeqCst) == STATUS_HEALTHY
    }

    /// Check if cloud executor is healthy
    pub fn is_cloud_healthy(&self) -> bool {
        self.cloud_status.load(Ordering::SeqCst) == STATUS_HEALTHY
    }

    /// Get local latency in milliseconds
    pub fn local_latency_ms(&self) -> u64 {
        self.local_latency_ms.load(Ordering::SeqCst)
    }

    /// Get cloud latency in milliseconds
    pub fn cloud_latency_ms(&self) -> u64 {
        self.cloud_latency_ms.load(Ordering::SeqCst)
    }

    /// Get total requests count
    pub fn requests_total(&self) -> u64 {
        self.requests_total.load(Ordering::SeqCst)
    }

    /// Get total failures count
    pub fn failures_total(&self) -> u64 {
        self.failures_total.load(Ordering::SeqCst)
    }

    /// Get local requests count
    pub fn local_requests(&self) -> u64 {
        self.local_requests.load(Ordering::SeqCst)
    }

    /// Get cloud requests count
    pub fn cloud_requests(&self) -> u64 {
        self.cloud_requests.load(Ordering::SeqCst)
    }

    /// Record a request to local executor
    pub fn record_local_request(&self, latency_ms: u64, success: bool) {
        self.requests_total.fetch_add(1, Ordering::SeqCst);
        self.local_requests.fetch_add(1, Ordering::SeqCst);

        // Update latency (simple moving average approximation)
        let current = self.local_latency_ms.load(Ordering::SeqCst);
        let new_latency = if current == 0 {
            latency_ms
        } else {
            (current + latency_ms) / 2
        };
        self.local_latency_ms.store(new_latency, Ordering::SeqCst);

        if success {
            self.local_consecutive_successes
                .fetch_add(1, Ordering::SeqCst);
            self.local_consecutive_failures.store(0, Ordering::SeqCst);
        } else {
            self.failures_total.fetch_add(1, Ordering::SeqCst);
            self.local_failures.fetch_add(1, Ordering::SeqCst);
            self.local_consecutive_failures
                .fetch_add(1, Ordering::SeqCst);
            self.local_consecutive_successes.store(0, Ordering::SeqCst);
        }
    }

    /// Record a request to cloud executor
    pub fn record_cloud_request(&self, latency_ms: u64, success: bool) {
        self.requests_total.fetch_add(1, Ordering::SeqCst);
        self.cloud_requests.fetch_add(1, Ordering::SeqCst);

        // Update latency (simple moving average approximation)
        let current = self.cloud_latency_ms.load(Ordering::SeqCst);
        let new_latency = if current == 0 {
            latency_ms
        } else {
            (current + latency_ms) / 2
        };
        self.cloud_latency_ms.store(new_latency, Ordering::SeqCst);

        if success {
            self.cloud_consecutive_successes
                .fetch_add(1, Ordering::SeqCst);
            self.cloud_consecutive_failures.store(0, Ordering::SeqCst);
        } else {
            self.failures_total.fetch_add(1, Ordering::SeqCst);
            self.cloud_failures.fetch_add(1, Ordering::SeqCst);
            self.cloud_consecutive_failures
                .fetch_add(1, Ordering::SeqCst);
            self.cloud_consecutive_successes.store(0, Ordering::SeqCst);
        }
    }

    /// Update health status based on thresholds
    pub fn update_health_status(&self, failure_threshold: u32, success_threshold: u32) {
        // Update local status
        let local_failures = self.local_consecutive_failures.load(Ordering::SeqCst);
        let local_successes = self.local_consecutive_successes.load(Ordering::SeqCst);

        if local_failures >= failure_threshold as u64 {
            self.local_status.store(STATUS_UNHEALTHY, Ordering::SeqCst);
        } else if local_successes >= success_threshold as u64 {
            self.local_status.store(STATUS_HEALTHY, Ordering::SeqCst);
        }

        // Update cloud status
        let cloud_failures = self.cloud_consecutive_failures.load(Ordering::SeqCst);
        let cloud_successes = self.cloud_consecutive_successes.load(Ordering::SeqCst);

        if cloud_failures >= failure_threshold as u64 {
            self.cloud_status.store(STATUS_UNHEALTHY, Ordering::SeqCst);
        } else if cloud_successes >= success_threshold as u64 {
            self.cloud_status.store(STATUS_HEALTHY, Ordering::SeqCst);
        }
    }

    /// Get next round-robin index (0 for local, 1 for cloud)
    pub fn next_round_robin(&self) -> usize {
        let counter = self.round_robin_counter.fetch_add(1, Ordering::SeqCst);
        (counter % 2) as usize
    }

    /// Reset all metrics
    pub fn reset(&self) {
        self.local_status.store(STATUS_UNKNOWN, Ordering::SeqCst);
        self.cloud_status.store(STATUS_UNKNOWN, Ordering::SeqCst);
        self.local_latency_ms.store(0, Ordering::SeqCst);
        self.cloud_latency_ms.store(0, Ordering::SeqCst);
        self.requests_total.store(0, Ordering::SeqCst);
        self.failures_total.store(0, Ordering::SeqCst);
        self.local_requests.store(0, Ordering::SeqCst);
        self.cloud_requests.store(0, Ordering::SeqCst);
        self.local_failures.store(0, Ordering::SeqCst);
        self.cloud_failures.store(0, Ordering::SeqCst);
        self.local_consecutive_failures.store(0, Ordering::SeqCst);
        self.cloud_consecutive_failures.store(0, Ordering::SeqCst);
        self.local_consecutive_successes.store(0, Ordering::SeqCst);
        self.cloud_consecutive_successes.store(0, Ordering::SeqCst);
    }
}

// ============================================================================
// Router Configuration
// ============================================================================

/// Configuration for the unified CodeAct router
#[derive(Debug, Clone)]
pub struct RouterConfig {
    /// Default routing mode
    pub default_mode: RouterMode,
    /// Whether to enable automatic fallback on failure
    pub fallback_enabled: bool,
    /// Load balancing strategy (used when mode is LoadBalance)
    pub load_balance_strategy: LoadBalanceStrategy,
    /// Health check interval
    pub health_check_interval: Duration,
    /// Timeout for health check pings
    pub health_check_timeout: Duration,
    /// Number of consecutive failures before marking as unhealthy
    pub failure_threshold: u32,
    /// Number of consecutive successes before marking as healthy
    pub success_threshold: u32,
    /// Maximum parallel executions
    pub max_parallel_executions: usize,
    /// Enable resource quota enforcement
    pub quota_enforcement_enabled: bool,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            default_mode: RouterMode::PreferLocal,
            fallback_enabled: true,
            load_balance_strategy: LoadBalanceStrategy::RoundRobin,
            health_check_interval: Duration::from_secs(30),
            health_check_timeout: Duration::from_secs(5),
            failure_threshold: 3,
            success_threshold: 2,
            max_parallel_executions: 10,
            quota_enforcement_enabled: true,
        }
    }
}

// ============================================================================
// Router Health
// ============================================================================

/// Health status for an executor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorHealth {
    /// Whether the executor is available
    pub available: bool,
    /// Whether the executor is healthy
    pub healthy: bool,
    /// Average latency in milliseconds
    pub latency_ms: u64,
    /// Number of consecutive failures
    pub consecutive_failures: u64,
    /// Active executions
    pub active_executions: usize,
    /// Available resources
    pub resources: AvailableResources,
    /// Last health check timestamp
    pub last_check: Option<u64>,
}

impl Default for ExecutorHealth {
    fn default() -> Self {
        Self {
            available: false,
            healthy: false,
            latency_ms: 0,
            consecutive_failures: 0,
            active_executions: 0,
            resources: AvailableResources::default(),
            last_check: None,
        }
    }
}

/// Overall status of the unified router
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedRouterStatus {
    /// Current routing mode
    pub mode: RouterMode,
    /// Whether fallback is enabled
    pub fallback_enabled: bool,
    /// Load balance strategy
    pub load_balance_strategy: LoadBalanceStrategy,
    /// Local executor health
    pub local: ExecutorHealth,
    /// Cloud executor health
    pub cloud: ExecutorHealth,
    /// Total requests processed
    pub requests_total: u64,
    /// Total failed requests
    pub failures_total: u64,
    /// Whether at least one executor is available
    pub any_available: bool,
}

// ============================================================================
// Unified CodeAct Router
// ============================================================================

/// Unified CodeAct router that integrates local Docker and cloud VM execution
///
/// This router provides a single interface for code execution that can route
/// requests to local Docker containers or cloud-based VMs, with support for
/// automatic failover and load balancing.
pub struct UnifiedCodeActRouter {
    /// Local execution strategy
    local: Option<Arc<dyn ExecutionStrategy>>,
    /// Cloud execution strategy
    cloud: Option<Arc<dyn ExecutionStrategy>>,
    /// Router configuration
    config: RouterConfig,
    /// Current routing mode
    mode: Arc<RwLock<RouterMode>>,
    /// Router metrics
    metrics: Arc<RouterMetrics>,
    /// Resource tracker
    resource_tracker: Arc<ResourceTracker>,
    /// Quota enforcer
    quota_enforcer: Arc<QuotaEnforcer>,
    /// Health monitor shutdown signal
    health_monitor_shutdown: Arc<RwLock<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl UnifiedCodeActRouter {
    /// Create a new unified CodeAct router
    pub fn new(
        local: Option<Arc<dyn ExecutionStrategy>>,
        cloud: Option<Arc<dyn ExecutionStrategy>>,
        config: RouterConfig,
    ) -> Self {
        let mode = config.default_mode;
        let resource_tracker = Arc::new(ResourceTracker::new());
        let quota_enforcer = Arc::new(QuotaEnforcer::new(resource_tracker.clone()));

        Self {
            local,
            cloud,
            config,
            mode: Arc::new(RwLock::new(mode)),
            metrics: Arc::new(RouterMetrics::new()),
            resource_tracker,
            quota_enforcer,
            health_monitor_shutdown: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a builder for the unified router
    pub fn builder() -> UnifiedCodeActRouterBuilder {
        UnifiedCodeActRouterBuilder::new()
    }

    /// Get reference to local strategy
    pub fn local(&self) -> Option<&Arc<dyn ExecutionStrategy>> {
        self.local.as_ref()
    }

    /// Get reference to cloud strategy
    pub fn cloud(&self) -> Option<&Arc<dyn ExecutionStrategy>> {
        self.cloud.as_ref()
    }

    /// Get reference to configuration
    pub fn config(&self) -> &RouterConfig {
        &self.config
    }

    /// Get reference to metrics
    pub fn metrics(&self) -> &Arc<RouterMetrics> {
        &self.metrics
    }

    /// Get reference to resource tracker
    pub fn resource_tracker(&self) -> &Arc<ResourceTracker> {
        &self.resource_tracker
    }

    /// Set a resource quota
    pub async fn set_quota(&self, owner_id: Option<String>, quota: ResourceQuota) {
        self.resource_tracker.set_quota(owner_id, quota).await;
    }

    /// Set the routing mode
    pub async fn set_mode(&self, mode: RouterMode) {
        let mut current = self.mode.write().await;
        *current = mode;
        info!(mode = %mode, "Router mode changed");
    }

    /// Get the current routing mode
    pub async fn get_mode(&self) -> RouterMode {
        *self.mode.read().await
    }

    /// Get the current status of the unified router
    pub async fn get_status(&self) -> UnifiedRouterStatus {
        let mode = *self.mode.read().await;

        let local_health = self.get_local_health().await;
        let cloud_health = self.get_cloud_health().await;

        UnifiedRouterStatus {
            mode,
            fallback_enabled: self.config.fallback_enabled,
            load_balance_strategy: self.config.load_balance_strategy,
            local: local_health.clone(),
            cloud: cloud_health.clone(),
            requests_total: self.metrics.requests_total(),
            failures_total: self.metrics.failures_total(),
            any_available: local_health.available || cloud_health.available,
        }
    }

    /// Get health status of local executor
    async fn get_local_health(&self) -> ExecutorHealth {
        if let Some(strategy) = &self.local {
            let available = strategy.is_available().await;
            let active = strategy.active_executions().await;
            let resources = strategy.available_resources().await;

            ExecutorHealth {
                available,
                healthy: self.metrics.is_local_healthy(),
                latency_ms: self.metrics.local_latency_ms(),
                consecutive_failures: self
                    .metrics
                    .local_consecutive_failures
                    .load(Ordering::SeqCst),
                active_executions: active,
                resources,
                last_check: Some(current_timestamp_ms()),
            }
        } else {
            ExecutorHealth::default()
        }
    }

    /// Get health status of cloud executor
    async fn get_cloud_health(&self) -> ExecutorHealth {
        if let Some(strategy) = &self.cloud {
            let available = strategy.is_available().await;
            let active = strategy.active_executions().await;
            let resources = strategy.available_resources().await;

            ExecutorHealth {
                available,
                healthy: self.metrics.is_cloud_healthy(),
                latency_ms: self.metrics.cloud_latency_ms(),
                consecutive_failures: self
                    .metrics
                    .cloud_consecutive_failures
                    .load(Ordering::SeqCst),
                active_executions: active,
                resources,
                last_check: Some(current_timestamp_ms()),
            }
        } else {
            ExecutorHealth::default()
        }
    }

    /// Perform health check on all executors
    #[instrument(skip(self))]
    pub async fn health_check(&self) -> (ExecutorHealth, ExecutorHealth) {
        let local_health = self.check_executor_health(&self.local, true).await;
        let cloud_health = self.check_executor_health(&self.cloud, false).await;

        // Update metrics based on health check
        if local_health.healthy {
            self.metrics
                .local_status
                .store(STATUS_HEALTHY, Ordering::SeqCst);
        } else if local_health.available && !local_health.healthy {
            self.metrics
                .local_status
                .store(STATUS_UNHEALTHY, Ordering::SeqCst);
        }

        if cloud_health.healthy {
            self.metrics
                .cloud_status
                .store(STATUS_HEALTHY, Ordering::SeqCst);
        } else if cloud_health.available && !cloud_health.healthy {
            self.metrics
                .cloud_status
                .store(STATUS_UNHEALTHY, Ordering::SeqCst);
        }

        (local_health, cloud_health)
    }

    /// Check health of a specific executor
    async fn check_executor_health(
        &self,
        strategy: &Option<Arc<dyn ExecutionStrategy>>,
        is_local: bool,
    ) -> ExecutorHealth {
        if let Some(strategy) = strategy {
            let healthy = strategy.health_check().await.unwrap_or(false);
            let available = strategy.is_available().await;
            let active = strategy.active_executions().await;
            let resources = strategy.available_resources().await;

            let latency_ms = if is_local {
                self.metrics.local_latency_ms()
            } else {
                self.metrics.cloud_latency_ms()
            };

            let consecutive_failures = if is_local {
                self.metrics
                    .local_consecutive_failures
                    .load(Ordering::SeqCst)
            } else {
                self.metrics
                    .cloud_consecutive_failures
                    .load(Ordering::SeqCst)
            };

            ExecutorHealth {
                available,
                healthy: healthy && consecutive_failures < self.config.failure_threshold as u64,
                latency_ms,
                consecutive_failures,
                active_executions: active,
                resources,
                last_check: Some(current_timestamp_ms()),
            }
        } else {
            ExecutorHealth::default()
        }
    }

    /// Start the health monitor background task
    pub async fn start_health_monitor(&self) {
        let local = self.local.clone();
        let cloud = self.cloud.clone();
        let metrics = self.metrics.clone();
        let interval = self.config.health_check_interval;
        let failure_threshold = self.config.failure_threshold;
        let success_threshold = self.config.success_threshold;

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

        {
            let mut shutdown = self.health_monitor_shutdown.write().await;
            *shutdown = Some(shutdown_tx);
        }

        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);

            loop {
                tokio::select! {
                    _ = interval_timer.tick() => {
                        // Check local executor
                        if let Some(ref strategy) = local {
                            let healthy = strategy.health_check().await.unwrap_or(false);
                            if healthy {
                                metrics.local_consecutive_successes.fetch_add(1, Ordering::SeqCst);
                                metrics.local_consecutive_failures.store(0, Ordering::SeqCst);
                            } else {
                                metrics.local_consecutive_failures.fetch_add(1, Ordering::SeqCst);
                                metrics.local_consecutive_successes.store(0, Ordering::SeqCst);
                            }
                        }

                        // Check cloud executor
                        if let Some(ref strategy) = cloud {
                            let healthy = strategy.health_check().await.unwrap_or(false);
                            if healthy {
                                metrics.cloud_consecutive_successes.fetch_add(1, Ordering::SeqCst);
                                metrics.cloud_consecutive_failures.store(0, Ordering::SeqCst);
                            } else {
                                metrics.cloud_consecutive_failures.fetch_add(1, Ordering::SeqCst);
                                metrics.cloud_consecutive_successes.store(0, Ordering::SeqCst);
                            }
                        }

                        // Update health status
                        metrics.update_health_status(failure_threshold, success_threshold);

                        debug!("Health check completed: local={}, cloud={}",
                            metrics.local_status(),
                            metrics.cloud_status()
                        );
                    }
                    _ = &mut shutdown_rx => {
                        info!("Health monitor shutting down");
                        break;
                    }
                }
            }
        });

        info!(interval_secs = interval.as_secs(), "Health monitor started");
    }

    /// Stop the health monitor background task
    pub async fn stop_health_monitor(&self) {
        let mut shutdown = self.health_monitor_shutdown.write().await;
        if let Some(tx) = shutdown.take() {
            let _ = tx.send(());
            info!("Health monitor stop signal sent");
        }
    }

    /// Execute code using the configured routing strategy
    #[instrument(skip(self, request), fields(request_id = %request.id))]
    pub async fn execute(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        // Check quota if enabled
        let quota_guard = if self.config.quota_enforcement_enabled {
            Some(
                self.quota_enforcer
                    .acquire(&request.session_id, &request)
                    .await?,
            )
        } else {
            None
        };

        let mode = *self.mode.read().await;
        let start = Instant::now();

        debug!(
            mode = %mode,
            request_id = %request.id,
            language = %request.language,
            "Executing code via unified router"
        );

        let result = match mode {
            RouterMode::LocalOnly => self.execute_local(request.clone()).await,
            RouterMode::CloudOnly => self.execute_cloud(request.clone()).await,
            RouterMode::PreferLocal => self.execute_with_fallback(request.clone(), true).await,
            RouterMode::PreferCloud => self.execute_with_fallback(request.clone(), false).await,
            RouterMode::LoadBalance => self.execute_balanced(request.clone()).await,
        };

        let elapsed_ms = start.elapsed().as_millis() as u64;

        // Release quota
        if let Some(guard) = quota_guard {
            guard.release(elapsed_ms).await;
        }

        result
    }

    /// Execute multiple requests in parallel
    #[instrument(skip(self, requests))]
    pub async fn execute_parallel(
        &self,
        requests: Vec<CodeExecutionRequest>,
    ) -> Vec<Result<CodeActResult>> {
        let max_parallel = self.config.max_parallel_executions.min(requests.len());
        let mut results = Vec::with_capacity(requests.len());

        // Process in batches
        for chunk in requests.chunks(max_parallel) {
            let futures: Vec<_> = chunk.iter().map(|req| self.execute(req.clone())).collect();

            let batch_results = futures::future::join_all(futures).await;
            results.extend(batch_results);
        }

        results
    }

    /// Cancel an execution by ID
    pub async fn cancel(&self, execution_id: &str) -> Result<()> {
        // Try to cancel on local
        if let Some(strategy) = &self.local {
            if strategy.cancel(execution_id).await.is_ok() {
                return Ok(());
            }
        }

        // Try to cancel on cloud
        if let Some(strategy) = &self.cloud {
            if strategy.cancel(execution_id).await.is_ok() {
                return Ok(());
            }
        }

        Err(Error::NotFound(format!(
            "Execution not found: {}",
            execution_id
        )))
    }

    /// Execute on local executor
    #[instrument(skip(self, request))]
    async fn execute_local(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        if let Some(strategy) = &self.local {
            let start = Instant::now();
            let result = strategy.execute(request).await;
            let latency_ms = start.elapsed().as_millis() as u64;

            let success = match &result {
                Ok(r) => r.is_success(),
                Err(_) => false,
            };

            self.metrics.record_local_request(latency_ms, success);
            self.metrics
                .update_health_status(self.config.failure_threshold, self.config.success_threshold);

            result
        } else {
            Err(Error::NotFound("Local executor not available".into()))
        }
    }

    /// Execute on cloud executor
    #[instrument(skip(self, request))]
    async fn execute_cloud(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        if let Some(strategy) = &self.cloud {
            let start = Instant::now();
            let result = strategy.execute(request).await;
            let latency_ms = start.elapsed().as_millis() as u64;

            let success = match &result {
                Ok(r) => r.is_success(),
                Err(_) => false,
            };

            self.metrics.record_cloud_request(latency_ms, success);
            self.metrics
                .update_health_status(self.config.failure_threshold, self.config.success_threshold);

            result
        } else {
            Err(Error::NotFound("Cloud executor not available".into()))
        }
    }

    /// Execute with automatic fallback
    #[instrument(skip(self, request))]
    async fn execute_with_fallback(
        &self,
        request: CodeExecutionRequest,
        prefer_local: bool,
    ) -> Result<CodeActResult> {
        let (primary, secondary) = if prefer_local {
            (&self.local, &self.cloud)
        } else {
            (&self.cloud, &self.local)
        };

        // Try primary
        if let Some(strategy) = primary {
            if strategy.is_available().await {
                let start = Instant::now();
                let result = strategy.execute(request.clone()).await;
                let latency_ms = start.elapsed().as_millis() as u64;

                match &result {
                    Ok(r) if r.is_success() => {
                        if prefer_local {
                            self.metrics.record_local_request(latency_ms, true);
                        } else {
                            self.metrics.record_cloud_request(latency_ms, true);
                        }
                        self.metrics.update_health_status(
                            self.config.failure_threshold,
                            self.config.success_threshold,
                        );
                        return result;
                    }
                    Ok(_) => {
                        if prefer_local {
                            self.metrics.record_local_request(latency_ms, false);
                        } else {
                            self.metrics.record_cloud_request(latency_ms, false);
                        }
                        self.metrics.update_health_status(
                            self.config.failure_threshold,
                            self.config.success_threshold,
                        );

                        if !self.config.fallback_enabled {
                            return result;
                        }
                        warn!("Primary execution failed, trying fallback");
                    }
                    Err(ref e) => {
                        if prefer_local {
                            self.metrics.record_local_request(latency_ms, false);
                        } else {
                            self.metrics.record_cloud_request(latency_ms, false);
                        }
                        self.metrics.update_health_status(
                            self.config.failure_threshold,
                            self.config.success_threshold,
                        );

                        if !self.config.fallback_enabled {
                            return result;
                        }
                        warn!(error = %e, "Primary execution error, trying fallback");
                    }
                }
            }
        }

        // Try secondary
        if self.config.fallback_enabled {
            if let Some(strategy) = secondary {
                if strategy.is_available().await {
                    debug!("Attempting fallback executor");
                    let start = Instant::now();
                    let result = strategy.execute(request).await;
                    let latency_ms = start.elapsed().as_millis() as u64;

                    let success = match &result {
                        Ok(r) => r.is_success(),
                        Err(_) => false,
                    };

                    if prefer_local {
                        self.metrics.record_cloud_request(latency_ms, success);
                    } else {
                        self.metrics.record_local_request(latency_ms, success);
                    }
                    self.metrics.update_health_status(
                        self.config.failure_threshold,
                        self.config.success_threshold,
                    );

                    return result;
                }
            }
        }

        Err(Error::NotFound("No executor available".into()))
    }

    /// Execute with load balancing
    #[instrument(skip(self, request))]
    async fn execute_balanced(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        match self.config.load_balance_strategy {
            LoadBalanceStrategy::RoundRobin => self.execute_round_robin(request).await,
            LoadBalanceStrategy::LeastConnections => self.execute_least_connections(request).await,
            LoadBalanceStrategy::ResponseTime => self.execute_response_time(request).await,
            LoadBalanceStrategy::Weighted => self.execute_weighted(request).await,
            LoadBalanceStrategy::ResourceAware => self.execute_resource_aware(request).await,
        }
    }

    /// Execute with round-robin load balancing
    async fn execute_round_robin(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        let index = self.metrics.next_round_robin();

        let prefer_local = match (index, &self.local, &self.cloud) {
            (0, Some(_), _) => true,
            (1, _, Some(_)) => false,
            (_, Some(_), None) => true,
            (_, None, Some(_)) => false,
            _ => return Err(Error::NotFound("No executor available".into())),
        };

        self.execute_with_fallback(request, prefer_local).await
    }

    /// Execute with least-connections load balancing
    async fn execute_least_connections(
        &self,
        request: CodeExecutionRequest,
    ) -> Result<CodeActResult> {
        let local_active = if let Some(strategy) = &self.local {
            strategy.active_executions().await
        } else {
            usize::MAX
        };

        let cloud_active = if let Some(strategy) = &self.cloud {
            strategy.active_executions().await
        } else {
            usize::MAX
        };

        let prefer_local = match (&self.local, &self.cloud) {
            (Some(_), Some(_)) => local_active <= cloud_active,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return Err(Error::NotFound("No executor available".into())),
        };

        self.execute_with_fallback(request, prefer_local).await
    }

    /// Execute with response-time-based load balancing
    async fn execute_response_time(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        let local_latency = self.metrics.local_latency_ms();
        let cloud_latency = self.metrics.cloud_latency_ms();

        let prefer_local = match (&self.local, &self.cloud) {
            (Some(_), Some(_)) => {
                // If both have latency data, prefer the faster one
                // If local has no data yet (0), prefer local to gather baseline
                local_latency == 0 || local_latency <= cloud_latency
            }
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return Err(Error::NotFound("No executor available".into())),
        };

        self.execute_with_fallback(request, prefer_local).await
    }

    /// Execute with weighted load balancing
    async fn execute_weighted(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        // For now, fall back to round-robin
        // In a real implementation, weights would be used
        self.execute_round_robin(request).await
    }

    /// Execute with resource-aware load balancing
    async fn execute_resource_aware(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        let local_resources = if let Some(strategy) = &self.local {
            strategy.available_resources().await
        } else {
            AvailableResources::default()
        };

        let cloud_resources = if let Some(strategy) = &self.cloud {
            strategy.available_resources().await
        } else {
            AvailableResources::default()
        };

        // Check if request fits in either
        let local_fits = local_resources.cpu_cores >= request.required_cpu
            && local_resources.memory_mb >= request.required_memory_mb
            && local_resources.execution_slots > 0;

        let cloud_fits = cloud_resources.cpu_cores >= request.required_cpu
            && cloud_resources.memory_mb >= request.required_memory_mb
            && cloud_resources.execution_slots > 0;

        let prefer_local = match (local_fits, cloud_fits) {
            (true, true) => {
                // Both fit, prefer the one with more available resources
                local_resources.utilization_percent <= cloud_resources.utilization_percent
            }
            (true, false) => true,
            (false, true) => false,
            (false, false) => {
                return Err(Error::Internal(
                    "Insufficient resources on all executors".into(),
                ))
            }
        };

        self.execute_with_fallback(request, prefer_local).await
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for creating UnifiedCodeActRouter instances
pub struct UnifiedCodeActRouterBuilder {
    local: Option<Arc<dyn ExecutionStrategy>>,
    cloud: Option<Arc<dyn ExecutionStrategy>>,
    config: RouterConfig,
}

impl UnifiedCodeActRouterBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            local: None,
            cloud: None,
            config: RouterConfig::default(),
        }
    }

    /// Set the local execution strategy
    pub fn local(mut self, strategy: Arc<dyn ExecutionStrategy>) -> Self {
        self.local = Some(strategy);
        self
    }

    /// Set the cloud execution strategy
    pub fn cloud(mut self, strategy: Arc<dyn ExecutionStrategy>) -> Self {
        self.cloud = Some(strategy);
        self
    }

    /// Set the routing mode
    pub fn mode(mut self, mode: RouterMode) -> Self {
        self.config.default_mode = mode;
        self
    }

    /// Enable or disable fallback
    pub fn fallback_enabled(mut self, enabled: bool) -> Self {
        self.config.fallback_enabled = enabled;
        self
    }

    /// Set the load balance strategy
    pub fn load_balance_strategy(mut self, strategy: LoadBalanceStrategy) -> Self {
        self.config.load_balance_strategy = strategy;
        self
    }

    /// Set the health check interval
    pub fn health_check_interval(mut self, interval: Duration) -> Self {
        self.config.health_check_interval = interval;
        self
    }

    /// Set the health check timeout
    pub fn health_check_timeout(mut self, timeout: Duration) -> Self {
        self.config.health_check_timeout = timeout;
        self
    }

    /// Set the failure threshold for marking executor as unhealthy
    pub fn failure_threshold(mut self, threshold: u32) -> Self {
        self.config.failure_threshold = threshold;
        self
    }

    /// Set the success threshold for marking executor as healthy
    pub fn success_threshold(mut self, threshold: u32) -> Self {
        self.config.success_threshold = threshold;
        self
    }

    /// Set the maximum parallel executions
    pub fn max_parallel_executions(mut self, max: usize) -> Self {
        self.config.max_parallel_executions = max;
        self
    }

    /// Enable or disable quota enforcement
    pub fn quota_enforcement_enabled(mut self, enabled: bool) -> Self {
        self.config.quota_enforcement_enabled = enabled;
        self
    }

    /// Set the entire configuration
    pub fn config(mut self, config: RouterConfig) -> Self {
        self.config = config;
        self
    }

    /// Build the unified router
    pub fn build(self) -> UnifiedCodeActRouter {
        UnifiedCodeActRouter::new(self.local, self.cloud, self.config)
    }
}

impl Default for UnifiedCodeActRouterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get current timestamp in milliseconds
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========== RouterMode Tests ==========

    #[test]
    fn test_router_mode_display() {
        assert_eq!(RouterMode::LocalOnly.to_string(), "local_only");
        assert_eq!(RouterMode::CloudOnly.to_string(), "cloud_only");
        assert_eq!(RouterMode::PreferLocal.to_string(), "prefer_local");
        assert_eq!(RouterMode::PreferCloud.to_string(), "prefer_cloud");
        assert_eq!(RouterMode::LoadBalance.to_string(), "load_balance");
    }

    #[test]
    fn test_router_mode_default() {
        assert_eq!(RouterMode::default(), RouterMode::PreferLocal);
    }

    #[test]
    fn test_router_mode_serialization() {
        let mode = RouterMode::LoadBalance;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"load_balance\"");

        let deserialized: RouterMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, RouterMode::LoadBalance);
    }

    // ========== LoadBalanceStrategy Tests ==========

    #[test]
    fn test_load_balance_strategy_display() {
        assert_eq!(LoadBalanceStrategy::RoundRobin.to_string(), "round_robin");
        assert_eq!(
            LoadBalanceStrategy::LeastConnections.to_string(),
            "least_connections"
        );
        assert_eq!(
            LoadBalanceStrategy::ResponseTime.to_string(),
            "response_time"
        );
        assert_eq!(LoadBalanceStrategy::Weighted.to_string(), "weighted");
        assert_eq!(
            LoadBalanceStrategy::ResourceAware.to_string(),
            "resource_aware"
        );
    }

    #[test]
    fn test_load_balance_strategy_serialization() {
        let strategy = LoadBalanceStrategy::LeastConnections;
        let json = serde_json::to_string(&strategy).unwrap();
        assert_eq!(json, "\"least_connections\"");

        let deserialized: LoadBalanceStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, LoadBalanceStrategy::LeastConnections);
    }

    // ========== CodeExecutionRequest Tests ==========

    #[test]
    fn test_code_execution_request_new() {
        let request = CodeExecutionRequest::new("print('hello')", "python");

        assert!(!request.id.is_empty());
        assert_eq!(request.code, "print('hello')");
        assert_eq!(request.language, "python");
        assert_eq!(request.timeout_ms, 30000);
        assert!(request.working_dir.is_none());
        assert!(request.env.is_empty());
    }

    #[test]
    fn test_code_execution_request_builder() {
        let request = CodeExecutionRequest::new("code", "python")
            .with_timeout(60000)
            .with_working_dir("/app")
            .with_env("KEY", "value")
            .with_session("session-123")
            .with_resources(2.0, 1024)
            .with_streaming(true);

        assert_eq!(request.timeout_ms, 60000);
        assert_eq!(request.working_dir, Some("/app".to_string()));
        assert_eq!(request.env.get("KEY"), Some(&"value".to_string()));
        assert_eq!(request.session_id, Some("session-123".to_string()));
        assert_eq!(request.required_cpu, 2.0);
        assert_eq!(request.required_memory_mb, 1024);
        assert!(request.stream);
    }

    #[test]
    fn test_code_execution_request_serialization() {
        let request = CodeExecutionRequest::new("code", "python").with_timeout(5000);

        let json = serde_json::to_string(&request).unwrap();
        let deserialized: CodeExecutionRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.code, "code");
        assert_eq!(deserialized.language, "python");
        assert_eq!(deserialized.timeout_ms, 5000);
    }

    // ========== AvailableResources Tests ==========

    #[test]
    fn test_available_resources_default() {
        let resources = AvailableResources::default();

        assert_eq!(resources.cpu_cores, 0.0);
        assert_eq!(resources.memory_mb, 0);
        assert_eq!(resources.execution_slots, 0);
        assert_eq!(resources.utilization_percent, 0.0);
    }

    // ========== LocalExecutionStrategy Tests ==========

    #[tokio::test]
    async fn test_local_execution_strategy_new() {
        let strategy = LocalExecutionStrategy::new(10, 4.0, 8192);

        assert_eq!(strategy.name(), "local_docker");
        assert!(strategy.is_available().await);
        assert_eq!(strategy.active_executions().await, 0);
    }

    #[tokio::test]
    async fn test_local_execution_strategy_available_resources() {
        let strategy = LocalExecutionStrategy::new(10, 4.0, 8192);
        let resources = strategy.available_resources().await;

        assert_eq!(resources.cpu_cores, 4.0);
        assert_eq!(resources.memory_mb, 8192);
        assert_eq!(resources.execution_slots, 10);
        assert_eq!(resources.utilization_percent, 0.0);
    }

    #[tokio::test]
    async fn test_local_execution_strategy_execute() {
        let strategy = LocalExecutionStrategy::new(10, 4.0, 8192);
        let request = CodeExecutionRequest::new("print('hello')", "python");

        let result = strategy.execute(request).await.unwrap();

        assert!(result.is_success());
        assert!(result
            .metadata
            .get("executor")
            .is_some_and(|v| v == "local_docker"));
    }

    #[tokio::test]
    async fn test_local_execution_strategy_set_enabled() {
        let strategy = LocalExecutionStrategy::new(10, 4.0, 8192);

        assert!(strategy.is_available().await);

        strategy.set_enabled(false).await;
        assert!(!strategy.is_available().await);

        strategy.set_enabled(true).await;
        assert!(strategy.is_available().await);
    }

    #[tokio::test]
    async fn test_local_execution_strategy_health_check() {
        let strategy = LocalExecutionStrategy::new(10, 4.0, 8192);

        assert!(strategy.health_check().await.unwrap());

        strategy.set_enabled(false).await;
        assert!(!strategy.health_check().await.unwrap());
    }

    // ========== CloudExecutionStrategy Tests ==========

    #[tokio::test]
    async fn test_cloud_execution_strategy_new() {
        let strategy = CloudExecutionStrategy::new("https://api.example.com", 20, 8.0, 16384);

        assert_eq!(strategy.name(), "cloud_vm");
        assert!(strategy.is_available().await);
        assert_eq!(strategy.active_executions().await, 0);
    }

    #[tokio::test]
    async fn test_cloud_execution_strategy_execute() {
        let strategy = CloudExecutionStrategy::new("https://api.example.com", 20, 8.0, 16384);
        let request = CodeExecutionRequest::new("console.log('hi')", "javascript");

        let result = strategy.execute(request).await.unwrap();

        assert!(result.is_success());
        assert!(result
            .metadata
            .get("executor")
            .is_some_and(|v| v == "cloud_vm"));
    }

    #[tokio::test]
    async fn test_cloud_execution_strategy_weight() {
        let strategy =
            CloudExecutionStrategy::new("https://api.example.com", 20, 8.0, 16384).with_weight(200);

        assert_eq!(strategy.weight(), 200);
    }

    // ========== FallbackStrategy Tests ==========

    #[tokio::test]
    async fn test_fallback_strategy_primary_success() {
        let primary = Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192));
        let backup = Arc::new(CloudExecutionStrategy::new(
            "https://backup.example.com",
            20,
            8.0,
            16384,
        ));

        let fallback = FallbackStrategy::new(primary, backup, 2);

        assert!(fallback.is_available().await);
        assert_eq!(fallback.name(), "fallback(local_docker/cloud_vm)");

        let request = CodeExecutionRequest::new("test", "python");
        let result = fallback.execute(request).await.unwrap();

        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_fallback_strategy_combined_resources() {
        let primary = Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192));
        let backup = Arc::new(CloudExecutionStrategy::new(
            "https://backup.example.com",
            20,
            8.0,
            16384,
        ));

        let fallback = FallbackStrategy::new(primary, backup, 2);
        let resources = fallback.available_resources().await;

        assert_eq!(resources.cpu_cores, 12.0);
        assert_eq!(resources.memory_mb, 24576);
        assert_eq!(resources.execution_slots, 30);
    }

    // ========== ResourceQuota Tests ==========

    #[test]
    fn test_resource_quota_default() {
        let quota = ResourceQuota::default();

        assert_eq!(quota.max_cpu_cores, 4.0);
        assert_eq!(quota.max_memory_mb, 4096);
        assert_eq!(quota.max_concurrent_executions, 10);
        assert_eq!(quota.max_execution_time_ms, 60000);
        assert!(quota.owner_id.is_none());
    }

    #[test]
    fn test_resource_quota_new() {
        let quota = ResourceQuota::new(8.0, 16384, 20, 120000);

        assert_eq!(quota.max_cpu_cores, 8.0);
        assert_eq!(quota.max_memory_mb, 16384);
        assert_eq!(quota.max_concurrent_executions, 20);
        assert_eq!(quota.max_execution_time_ms, 120000);
    }

    #[test]
    fn test_resource_quota_with_owner() {
        let quota = ResourceQuota::default().with_owner("user-123");

        assert_eq!(quota.owner_id, Some("user-123".to_string()));
    }

    // ========== ResourceTracker Tests ==========

    #[tokio::test]
    async fn test_resource_tracker_set_and_get_quota() {
        let tracker = ResourceTracker::new();
        let quota = ResourceQuota::new(8.0, 16384, 20, 120000);

        tracker
            .set_quota(Some("user-1".to_string()), quota.clone())
            .await;

        let retrieved = tracker.get_quota(&Some("user-1".to_string())).await;
        assert_eq!(retrieved.max_cpu_cores, 8.0);
        assert_eq!(retrieved.max_memory_mb, 16384);
    }

    #[tokio::test]
    async fn test_resource_tracker_check_quota_success() {
        let tracker = ResourceTracker::new();
        let quota = ResourceQuota::new(8.0, 16384, 20, 120000);
        tracker.set_quota(None, quota).await;

        let request = CodeExecutionRequest::new("test", "python")
            .with_resources(1.0, 512)
            .with_timeout(30000);

        let result = tracker.check_quota(&None, &request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_resource_tracker_check_quota_exceeded() {
        let tracker = ResourceTracker::new();
        let quota = ResourceQuota::new(1.0, 512, 1, 10000);
        tracker.set_quota(None, quota).await;

        let request = CodeExecutionRequest::new("test", "python")
            .with_resources(2.0, 1024)
            .with_timeout(30000);

        let result = tracker.check_quota(&None, &request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resource_tracker_reserve_and_release() {
        let tracker = ResourceTracker::new();
        let quota = ResourceQuota::new(8.0, 16384, 20, 120000);
        tracker.set_quota(None, quota).await;

        let request = CodeExecutionRequest::new("test", "python").with_resources(2.0, 1024);

        // Reserve
        tracker.reserve(&None, &request).await;

        let usage = tracker.get_usage(&None).await;
        assert_eq!(usage.cpu_used, 2.0);
        assert_eq!(usage.memory_used_mb, 1024);
        assert_eq!(usage.concurrent_executions, 1);

        // Release
        tracker.release(&None, &request, 5000).await;

        let usage = tracker.get_usage(&None).await;
        assert_eq!(usage.cpu_used, 0.0);
        assert_eq!(usage.memory_used_mb, 0);
        assert_eq!(usage.concurrent_executions, 0);
        assert_eq!(usage.daily_execution_time_ms, 5000);
    }

    #[tokio::test]
    async fn test_resource_tracker_reset_daily() {
        let tracker = ResourceTracker::new();
        let request = CodeExecutionRequest::new("test", "python").with_resources(1.0, 512);

        tracker.reserve(&None, &request).await;
        tracker.release(&None, &request, 10000).await;

        let usage = tracker.get_usage(&None).await;
        assert_eq!(usage.daily_execution_time_ms, 10000);

        tracker.reset_daily_usage().await;

        let usage = tracker.get_usage(&None).await;
        assert_eq!(usage.daily_execution_time_ms, 0);
        assert!(usage.last_reset.is_some());
    }

    // ========== RouterMetrics Tests ==========

    #[test]
    fn test_router_metrics_new() {
        let metrics = RouterMetrics::new();

        assert_eq!(metrics.local_status(), STATUS_UNKNOWN);
        assert_eq!(metrics.cloud_status(), STATUS_UNKNOWN);
        assert_eq!(metrics.local_latency_ms(), 0);
        assert_eq!(metrics.cloud_latency_ms(), 0);
        assert_eq!(metrics.requests_total(), 0);
        assert_eq!(metrics.failures_total(), 0);
    }

    #[test]
    fn test_router_metrics_record_local_request() {
        let metrics = RouterMetrics::new();

        metrics.record_local_request(100, true);
        assert_eq!(metrics.requests_total(), 1);
        assert_eq!(metrics.local_requests(), 1);
        assert_eq!(metrics.failures_total(), 0);
        assert_eq!(metrics.local_latency_ms(), 100);

        metrics.record_local_request(200, false);
        assert_eq!(metrics.requests_total(), 2);
        assert_eq!(metrics.failures_total(), 1);
        assert_eq!(metrics.local_latency_ms(), 150); // Average
    }

    #[test]
    fn test_router_metrics_record_cloud_request() {
        let metrics = RouterMetrics::new();

        metrics.record_cloud_request(50, true);
        assert_eq!(metrics.requests_total(), 1);
        assert_eq!(metrics.cloud_requests(), 1);
        assert_eq!(metrics.cloud_latency_ms(), 50);
    }

    #[test]
    fn test_router_metrics_health_status_update() {
        let metrics = RouterMetrics::new();

        // Simulate 3 consecutive failures
        for _ in 0..3 {
            metrics.record_local_request(100, false);
        }

        metrics.update_health_status(3, 2);
        assert_eq!(metrics.local_status(), STATUS_UNHEALTHY);

        // Simulate recovery with 2 successes
        for _ in 0..2 {
            metrics.record_local_request(100, true);
        }

        metrics.update_health_status(3, 2);
        assert_eq!(metrics.local_status(), STATUS_HEALTHY);
    }

    #[test]
    fn test_router_metrics_round_robin() {
        let metrics = RouterMetrics::new();

        assert_eq!(metrics.next_round_robin(), 0);
        assert_eq!(metrics.next_round_robin(), 1);
        assert_eq!(metrics.next_round_robin(), 0);
        assert_eq!(metrics.next_round_robin(), 1);
    }

    #[test]
    fn test_router_metrics_reset() {
        let metrics = RouterMetrics::new();

        metrics.record_local_request(100, true);
        metrics.record_cloud_request(200, false);
        metrics.local_status.store(STATUS_HEALTHY, Ordering::SeqCst);

        metrics.reset();

        assert_eq!(metrics.requests_total(), 0);
        assert_eq!(metrics.local_latency_ms(), 0);
        assert_eq!(metrics.local_status(), STATUS_UNKNOWN);
    }

    // ========== RouterConfig Tests ==========

    #[test]
    fn test_router_config_default() {
        let config = RouterConfig::default();

        assert_eq!(config.default_mode, RouterMode::PreferLocal);
        assert!(config.fallback_enabled);
        assert_eq!(
            config.load_balance_strategy,
            LoadBalanceStrategy::RoundRobin
        );
        assert_eq!(config.health_check_interval, Duration::from_secs(30));
        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.success_threshold, 2);
        assert_eq!(config.max_parallel_executions, 10);
        assert!(config.quota_enforcement_enabled);
    }

    // ========== UnifiedCodeActRouter Tests ==========

    #[tokio::test]
    async fn test_unified_router_builder() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;
        let cloud = Arc::new(CloudExecutionStrategy::new(
            "https://api.example.com",
            20,
            8.0,
            16384,
        )) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .cloud(cloud)
            .mode(RouterMode::PreferLocal)
            .fallback_enabled(true)
            .load_balance_strategy(LoadBalanceStrategy::ResponseTime)
            .failure_threshold(5)
            .success_threshold(3)
            .build();

        assert!(router.local().is_some());
        assert!(router.cloud().is_some());
        assert_eq!(router.config().default_mode, RouterMode::PreferLocal);
        assert_eq!(router.config().failure_threshold, 5);
        assert_eq!(router.config().success_threshold, 3);
    }

    #[tokio::test]
    async fn test_unified_router_mode_switch() {
        let router = UnifiedCodeActRouter::builder().build();

        assert_eq!(router.get_mode().await, RouterMode::PreferLocal);

        router.set_mode(RouterMode::CloudOnly).await;
        assert_eq!(router.get_mode().await, RouterMode::CloudOnly);

        router.set_mode(RouterMode::LoadBalance).await;
        assert_eq!(router.get_mode().await, RouterMode::LoadBalance);
    }

    #[tokio::test]
    async fn test_unified_router_execute_local_only() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .mode(RouterMode::LocalOnly)
            .quota_enforcement_enabled(false)
            .build();

        let request = CodeExecutionRequest::new("print('hello')", "python");
        let result = router.execute(request).await.unwrap();

        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_unified_router_execute_cloud_only() {
        let cloud = Arc::new(CloudExecutionStrategy::new(
            "https://api.example.com",
            20,
            8.0,
            16384,
        )) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .cloud(cloud)
            .mode(RouterMode::CloudOnly)
            .quota_enforcement_enabled(false)
            .build();

        let request = CodeExecutionRequest::new("console.log('hi')", "javascript");
        let result = router.execute(request).await.unwrap();

        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_unified_router_execute_prefer_local() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;
        let cloud = Arc::new(CloudExecutionStrategy::new(
            "https://api.example.com",
            20,
            8.0,
            16384,
        )) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .cloud(cloud)
            .mode(RouterMode::PreferLocal)
            .quota_enforcement_enabled(false)
            .build();

        let request = CodeExecutionRequest::new("test", "python");
        let result = router.execute(request).await.unwrap();

        assert!(result.is_success());
        assert!(result
            .metadata
            .get("executor")
            .is_some_and(|v| v == "local_docker"));
    }

    #[tokio::test]
    async fn test_unified_router_execute_load_balance() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;
        let cloud = Arc::new(CloudExecutionStrategy::new(
            "https://api.example.com",
            20,
            8.0,
            16384,
        )) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .cloud(cloud)
            .mode(RouterMode::LoadBalance)
            .load_balance_strategy(LoadBalanceStrategy::RoundRobin)
            .quota_enforcement_enabled(false)
            .build();

        // Execute multiple commands
        for i in 0..4 {
            let request = CodeExecutionRequest::new(format!("test-{}", i), "python");
            let result = router.execute(request).await.unwrap();
            assert!(result.is_success());
        }

        // Verify requests were distributed
        let status = router.get_status().await;
        assert!(status.requests_total >= 4);
    }

    #[tokio::test]
    async fn test_unified_router_execute_parallel() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .mode(RouterMode::LocalOnly)
            .max_parallel_executions(5)
            .quota_enforcement_enabled(false)
            .build();

        let requests: Vec<_> = (0..10)
            .map(|i| CodeExecutionRequest::new(format!("code-{}", i), "python"))
            .collect();

        let results = router.execute_parallel(requests).await;

        assert_eq!(results.len(), 10);
        for result in results {
            assert!(result.is_ok());
            assert!(result.unwrap().is_success());
        }
    }

    #[tokio::test]
    async fn test_unified_router_get_status() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;
        let cloud = Arc::new(CloudExecutionStrategy::new(
            "https://api.example.com",
            20,
            8.0,
            16384,
        )) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .cloud(cloud)
            .mode(RouterMode::PreferLocal)
            .build();

        let status = router.get_status().await;

        assert_eq!(status.mode, RouterMode::PreferLocal);
        assert!(status.fallback_enabled);
        assert!(status.local.available);
        assert!(status.cloud.available);
        assert!(status.any_available);
    }

    #[tokio::test]
    async fn test_unified_router_health_check() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;
        let cloud = Arc::new(CloudExecutionStrategy::new(
            "https://api.example.com",
            20,
            8.0,
            16384,
        )) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .cloud(cloud)
            .build();

        let (local_health, cloud_health) = router.health_check().await;

        assert!(local_health.available);
        assert!(cloud_health.available);
    }

    #[tokio::test]
    async fn test_unified_router_set_quota() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .mode(RouterMode::LocalOnly)
            .build();

        let quota = ResourceQuota::new(2.0, 1024, 5, 30000);
        router.set_quota(Some("user-1".to_string()), quota).await;

        let retrieved = router
            .resource_tracker()
            .get_quota(&Some("user-1".to_string()))
            .await;
        assert_eq!(retrieved.max_cpu_cores, 2.0);
        assert_eq!(retrieved.max_memory_mb, 1024);
    }

    #[tokio::test]
    async fn test_unified_router_execute_local_only_no_executor() {
        let router = UnifiedCodeActRouter::builder()
            .mode(RouterMode::LocalOnly)
            .quota_enforcement_enabled(false)
            .build();

        let request = CodeExecutionRequest::new("test", "python");
        let result = router.execute(request).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unified_router_execute_cloud_only_no_executor() {
        let router = UnifiedCodeActRouter::builder()
            .mode(RouterMode::CloudOnly)
            .quota_enforcement_enabled(false)
            .build();

        let request = CodeExecutionRequest::new("test", "python");
        let result = router.execute(request).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unified_router_metrics_after_requests() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .mode(RouterMode::LocalOnly)
            .quota_enforcement_enabled(false)
            .build();

        // Execute some commands
        for i in 0..5 {
            let request = CodeExecutionRequest::new(format!("test-{}", i), "python");
            let _ = router.execute(request).await;
        }

        assert_eq!(router.metrics().requests_total(), 5);
        assert_eq!(router.metrics().failures_total(), 0);
    }

    #[tokio::test]
    async fn test_unified_router_cancel_execution() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .mode(RouterMode::LocalOnly)
            .build();

        let result = router.cancel("nonexistent-id").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_health_monitor_lifecycle() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .health_check_interval(Duration::from_millis(100))
            .build();

        // Start health monitor
        router.start_health_monitor().await;

        // Wait a bit for health checks
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Stop health monitor
        router.stop_health_monitor().await;

        // Should not panic
    }

    // ========== ExecutorHealth Tests ==========

    #[test]
    fn test_executor_health_default() {
        let health = ExecutorHealth::default();

        assert!(!health.available);
        assert!(!health.healthy);
        assert_eq!(health.latency_ms, 0);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.active_executions, 0);
        assert!(health.last_check.is_none());
    }

    // ========== UnifiedRouterStatus Tests ==========

    #[test]
    fn test_unified_router_status_serialization() {
        let status = UnifiedRouterStatus {
            mode: RouterMode::PreferLocal,
            fallback_enabled: true,
            load_balance_strategy: LoadBalanceStrategy::RoundRobin,
            local: ExecutorHealth::default(),
            cloud: ExecutorHealth::default(),
            requests_total: 100,
            failures_total: 5,
            any_available: true,
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("prefer_local"));
        assert!(json.contains("round_robin"));

        let deserialized: UnifiedRouterStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.mode, RouterMode::PreferLocal);
        assert_eq!(deserialized.requests_total, 100);
    }

    // ========== Integration Tests ==========

    #[tokio::test]
    async fn test_full_execution_flow_with_quota() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .mode(RouterMode::LocalOnly)
            .quota_enforcement_enabled(true)
            .build();

        // Set a quota
        let quota = ResourceQuota::new(4.0, 8192, 10, 60000);
        router.set_quota(None, quota).await;

        // Execute within quota
        let request = CodeExecutionRequest::new("test", "python");
        let result = router.execute(request).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_success());
    }

    #[tokio::test]
    async fn test_quota_enforcement_blocks_over_limit() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .mode(RouterMode::LocalOnly)
            .quota_enforcement_enabled(true)
            .build();

        // Set a very restrictive quota
        let quota = ResourceQuota::new(0.5, 256, 1, 1000);
        router.set_quota(None, quota).await;

        // Request that exceeds quota
        let request = CodeExecutionRequest::new("test", "python").with_resources(2.0, 1024);
        let result = router.execute(request).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_response_time_strategy() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;
        let cloud = Arc::new(CloudExecutionStrategy::new(
            "https://api.example.com",
            20,
            8.0,
            16384,
        )) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .cloud(cloud)
            .mode(RouterMode::LoadBalance)
            .load_balance_strategy(LoadBalanceStrategy::ResponseTime)
            .quota_enforcement_enabled(false)
            .build();

        // First request establishes baseline
        let request = CodeExecutionRequest::new("test-1", "python");
        let _ = router.execute(request).await;

        // Execute more requests
        for i in 2..5 {
            let request = CodeExecutionRequest::new(format!("test-{}", i), "python");
            let _ = router.execute(request).await;
        }

        let status = router.get_status().await;
        assert!(status.requests_total >= 4);
    }

    #[tokio::test]
    async fn test_least_connections_strategy() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;
        let cloud = Arc::new(CloudExecutionStrategy::new(
            "https://api.example.com",
            20,
            8.0,
            16384,
        )) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .cloud(cloud)
            .mode(RouterMode::LoadBalance)
            .load_balance_strategy(LoadBalanceStrategy::LeastConnections)
            .quota_enforcement_enabled(false)
            .build();

        let requests: Vec<_> = (0..5)
            .map(|i| CodeExecutionRequest::new(format!("test-{}", i), "python"))
            .collect();

        let results = router.execute_parallel(requests).await;

        for result in results {
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_resource_aware_strategy() {
        let local =
            Arc::new(LocalExecutionStrategy::new(10, 4.0, 8192)) as Arc<dyn ExecutionStrategy>;
        let cloud = Arc::new(CloudExecutionStrategy::new(
            "https://api.example.com",
            20,
            8.0,
            16384,
        )) as Arc<dyn ExecutionStrategy>;

        let router = UnifiedCodeActRouter::builder()
            .local(local)
            .cloud(cloud)
            .mode(RouterMode::LoadBalance)
            .load_balance_strategy(LoadBalanceStrategy::ResourceAware)
            .quota_enforcement_enabled(false)
            .build();

        // Request that should fit on either
        let request = CodeExecutionRequest::new("test", "python").with_resources(1.0, 512);
        let result = router.execute(request).await;

        assert!(result.is_ok());
    }
}
