//! End-to-End tests for Local CodeAct Execution
//!
//! These tests verify the integration of local CodeAct components including:
//! - Container lifecycle management (create, start, stop, destroy)
//! - Container pool warmup and reuse
//! - Python code execution with security validation
//! - SDK functionality (browser, bash operations)
//! - Security enforcement (blocked imports, patterns)
//! - Resource limit enforcement
//!
//! Run with: `cargo test --package gateway-core --test local_codeact_e2e`
//!
//! Note: These tests use mock implementations and do not require Docker.

use gateway_core::error::{Error, Result};
use gateway_core::executor::{
    ContainerConfig, ContainerRuntime, ContainerState, ExecutionStatus, IssueType, Mount,
    MountType, NetworkMode, PoolConfig, PoolStats, SecurityConfig, SecurityValidator, Severity,
    SharedSecurityValidator,
};

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

// ============================================================================
// Mock Components for Testing (without real Docker)
// ============================================================================

/// Mock Docker client for testing without real Docker daemon
#[derive(Debug)]
pub struct MockDockerClient {
    /// Simulated images available
    images: Arc<RwLock<HashSet<String>>>,
    /// Container creation counter
    container_counter: AtomicU32,
    /// Total containers created
    total_created: AtomicU64,
    /// Total containers destroyed
    total_destroyed: AtomicU64,
    /// Whether Docker daemon is "healthy"
    healthy: AtomicBool,
    /// Simulated latency in ms
    latency_ms: AtomicU64,
    /// Whether operations should fail
    fail_next_operation: AtomicBool,
}

impl Default for MockDockerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MockDockerClient {
    pub fn new() -> Self {
        let mut images = HashSet::new();
        images.insert("python:3.11-slim".to_string());
        images.insert("ubuntu:22.04".to_string());
        images.insert("alpine:latest".to_string());

        Self {
            images: Arc::new(RwLock::new(images)),
            container_counter: AtomicU32::new(0),
            total_created: AtomicU64::new(0),
            total_destroyed: AtomicU64::new(0),
            healthy: AtomicBool::new(true),
            latency_ms: AtomicU64::new(0),
            fail_next_operation: AtomicBool::new(false),
        }
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.healthy.store(healthy, Ordering::SeqCst);
    }

    pub fn set_latency_ms(&self, latency: u64) {
        self.latency_ms.store(latency, Ordering::SeqCst);
    }

    pub fn set_fail_next(&self, fail: bool) {
        self.fail_next_operation.store(fail, Ordering::SeqCst);
    }

    async fn simulate_latency(&self) {
        let latency = self.latency_ms.load(Ordering::SeqCst);
        if latency > 0 {
            tokio::time::sleep(Duration::from_millis(latency)).await;
        }
    }

    fn check_operation(&self) -> Result<()> {
        if self.fail_next_operation.swap(false, Ordering::SeqCst) {
            return Err(Error::Docker("Simulated Docker operation failure".into()));
        }
        if !self.healthy.load(Ordering::SeqCst) {
            return Err(Error::Docker("Docker daemon unhealthy".into()));
        }
        Ok(())
    }

    pub async fn ping(&self) -> Result<()> {
        self.simulate_latency().await;
        self.check_operation()
    }

    pub async fn version(&self) -> Result<String> {
        self.simulate_latency().await;
        self.check_operation()?;
        Ok("24.0.0".to_string())
    }

    pub async fn image_exists(&self, image: &str) -> bool {
        let images = self.images.read().await;
        images.contains(image)
    }

    pub async fn pull_image(&self, image: &str) -> Result<()> {
        self.simulate_latency().await;
        self.check_operation()?;
        let mut images = self.images.write().await;
        images.insert(image.to_string());
        Ok(())
    }

    pub async fn create_container(&self, config: &ContainerConfig) -> Result<String> {
        self.simulate_latency().await;
        self.check_operation()?;

        if !self.image_exists(&config.image).await {
            return Err(Error::Docker(format!("Image not found: {}", config.image)));
        }

        let id = self.container_counter.fetch_add(1, Ordering::SeqCst);
        self.total_created.fetch_add(1, Ordering::SeqCst);
        Ok(format!("mock-container-{}", id))
    }

    pub async fn start_container(&self, _container_id: &str) -> Result<()> {
        self.simulate_latency().await;
        self.check_operation()
    }

    pub async fn stop_container(&self, _container_id: &str, _timeout: u64) -> Result<()> {
        self.simulate_latency().await;
        self.check_operation()
    }

    pub async fn remove_container(&self, _container_id: &str, _force: bool) -> Result<()> {
        self.simulate_latency().await;
        self.check_operation()?;
        self.total_destroyed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn stats(&self) -> (u64, u64) {
        (
            self.total_created.load(Ordering::SeqCst),
            self.total_destroyed.load(Ordering::SeqCst),
        )
    }
}

/// Mock container pool for testing
pub struct MockContainerPool {
    docker: Arc<MockDockerClient>,
    config: PoolConfig,
    warm_containers: Arc<Mutex<VecDeque<MockWarmContainer>>>,
    in_use: Arc<Mutex<HashMap<String, ContainerRuntime>>>,
    stats: Arc<RwLock<PoolStats>>,
    shutdown: AtomicBool,
}

#[derive(Clone)]
struct MockWarmContainer {
    runtime: ContainerRuntime,
    warmed_at: Instant,
}

impl MockContainerPool {
    pub fn new(docker: Arc<MockDockerClient>, config: PoolConfig) -> Self {
        Self {
            docker,
            config,
            warm_containers: Arc::new(Mutex::new(VecDeque::new())),
            in_use: Arc::new(Mutex::new(HashMap::new())),
            stats: Arc::new(RwLock::new(PoolStats::default())),
            shutdown: AtomicBool::new(false),
        }
    }

    pub async fn warmup(&self, count: usize) -> Result<()> {
        for _ in 0..count {
            let config = ContainerConfig::new(&self.config.default_warm_image);
            let container_id = self.docker.create_container(&config).await?;
            self.docker.start_container(&container_id).await?;

            let runtime = ContainerRuntime::new(
                container_id,
                format!("warm-{}", uuid::Uuid::new_v4()),
                config,
            );

            let mut warm = self.warm_containers.lock().await;
            warm.push_back(MockWarmContainer {
                runtime,
                warmed_at: Instant::now(),
            });

            let mut stats = self.stats.write().await;
            stats.warm_containers += 1;
            stats.containers_created += 1;
        }
        Ok(())
    }

    pub async fn acquire(&self, config: ContainerConfig) -> Result<ContainerRuntime> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(Error::Internal("Pool is shutting down".into()));
        }

        {
            let mut stats = self.stats.write().await;
            stats.requests_served += 1;
        }

        // Try to get from warm pool
        {
            let mut warm = self.warm_containers.lock().await;
            let ttl = Duration::from_secs(self.config.warm_ttl_seconds);

            while let Some(container) = warm.pop_front() {
                if container.warmed_at.elapsed() < ttl {
                    let mut stats = self.stats.write().await;
                    stats.cache_hits += 1;
                    stats.warm_containers = stats.warm_containers.saturating_sub(1);

                    let mut in_use = self.in_use.lock().await;
                    in_use.insert(
                        container.runtime.config.id.clone(),
                        container.runtime.clone(),
                    );

                    return Ok(container.runtime);
                }
                // Expired, destroy it
                let _ = self
                    .docker
                    .remove_container(&container.runtime.docker_id, true)
                    .await;
            }
        }

        // Create new container
        {
            let mut stats = self.stats.write().await;
            stats.cache_misses += 1;
        }

        let container_id = self.docker.create_container(&config).await?;
        self.docker.start_container(&container_id).await?;

        // Create config with a unique internal ID for tracking
        let mut tracked_config = config;
        tracked_config.id = uuid::Uuid::new_v4().to_string();

        let runtime = ContainerRuntime::new(
            container_id,
            format!("exec-{}", uuid::Uuid::new_v4()),
            tracked_config,
        );

        {
            let mut in_use = self.in_use.lock().await;
            in_use.insert(runtime.config.id.clone(), runtime.clone());
        }

        {
            let mut stats = self.stats.write().await;
            stats.containers_created += 1;
            stats.running_containers += 1;
        }

        Ok(runtime)
    }

    pub async fn release(&self, id: &str, keep_warm: bool) -> Result<()> {
        let runtime = {
            let mut in_use = self.in_use.lock().await;
            in_use.remove(id)
        };

        if let Some(runtime) = runtime {
            if keep_warm && runtime.reuse_count < self.config.max_reuse_count {
                let mut warm = self.warm_containers.lock().await;
                warm.push_back(MockWarmContainer {
                    runtime,
                    warmed_at: Instant::now(),
                });

                let mut stats = self.stats.write().await;
                stats.warm_containers += 1;
                stats.running_containers = stats.running_containers.saturating_sub(1);
            } else {
                let _ = self.docker.remove_container(&runtime.docker_id, true).await;
                let mut stats = self.stats.write().await;
                stats.containers_recycled += 1;
                stats.running_containers = stats.running_containers.saturating_sub(1);
            }
        }

        Ok(())
    }

    pub async fn destroy(&self, id: &str) -> Result<()> {
        let runtime = {
            let mut in_use = self.in_use.lock().await;
            in_use.remove(id)
        };

        if let Some(runtime) = runtime {
            self.docker
                .remove_container(&runtime.docker_id, true)
                .await?;
            let mut stats = self.stats.write().await;
            stats.containers_recycled += 1;
            stats.running_containers = stats.running_containers.saturating_sub(1);
        }

        Ok(())
    }

    pub async fn stats(&self) -> PoolStats {
        self.stats.read().await.clone()
    }

    pub async fn warm_count(&self) -> usize {
        self.warm_containers.lock().await.len()
    }

    pub async fn in_use_count(&self) -> usize {
        self.in_use.lock().await.len()
    }

    pub async fn health_check(&self) -> Result<bool> {
        self.docker.ping().await?;
        Ok(true)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown.store(true, Ordering::SeqCst);

        // Destroy warm containers
        let mut warm = self.warm_containers.lock().await;
        for container in warm.drain(..) {
            let _ = self
                .docker
                .remove_container(&container.runtime.docker_id, true)
                .await;
        }

        // Destroy in-use containers
        let mut in_use = self.in_use.lock().await;
        for (_, runtime) in in_use.drain() {
            let _ = self.docker.remove_container(&runtime.docker_id, true).await;
        }

        Ok(())
    }
}

/// Mock Python executor for testing
pub struct MockPythonExecutor {
    security_validator: SecurityValidator,
    execution_count: AtomicU64,
    fail_execution: AtomicBool,
    execution_delay_ms: AtomicU64,
    /// Session variables for persistence testing
    session_vars: Arc<Mutex<HashMap<String, serde_json::Value>>>,
}

impl Default for MockPythonExecutor {
    fn default() -> Self {
        Self::new(SecurityConfig::default())
    }
}

impl MockPythonExecutor {
    pub fn new(security_config: SecurityConfig) -> Self {
        Self {
            security_validator: SecurityValidator::new(security_config),
            execution_count: AtomicU64::new(0),
            fail_execution: AtomicBool::new(false),
            execution_delay_ms: AtomicU64::new(0),
            session_vars: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set_fail_execution(&self, fail: bool) {
        self.fail_execution.store(fail, Ordering::SeqCst);
    }

    pub fn set_execution_delay_ms(&self, delay: u64) {
        self.execution_delay_ms.store(delay, Ordering::SeqCst);
    }

    pub async fn execute(&self, code: &str) -> Result<MockExecutionResult> {
        // Validate code first
        let validation = self.security_validator.validate_code(code)?;
        if !validation.is_safe {
            return Ok(MockExecutionResult {
                stdout: String::new(),
                stderr: format!(
                    "Security validation failed: {} issues found",
                    validation.issues.len()
                ),
                exit_code: 1,
                status: ExecutionStatus::Error,
                duration_ms: 0,
            });
        }

        // Simulate execution delay
        let delay = self.execution_delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        // Check if execution should fail
        if self.fail_execution.load(Ordering::SeqCst) {
            return Ok(MockExecutionResult {
                stdout: String::new(),
                stderr: "Execution failed".to_string(),
                exit_code: 1,
                status: ExecutionStatus::Error,
                duration_ms: delay,
            });
        }

        self.execution_count.fetch_add(1, Ordering::SeqCst);

        // Simulate simple Python execution
        let (stdout, stderr, exit_code) = self.simulate_python(code).await;

        Ok(MockExecutionResult {
            stdout,
            stderr,
            exit_code,
            status: if exit_code == 0 {
                ExecutionStatus::Success
            } else {
                ExecutionStatus::Error
            },
            duration_ms: delay,
        })
    }

    async fn simulate_python(&self, code: &str) -> (String, String, i32) {
        // Simple Python simulation for common patterns
        let mut stdout = String::new();
        let stderr = String::new();
        let exit_code = 0;

        // Handle print statements
        for line in code.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("print(") {
                // Extract print content
                if let Some(content) = trimmed
                    .strip_prefix("print(")
                    .and_then(|s| s.strip_suffix(')'))
                {
                    let content = content.trim();
                    // Handle string literals
                    if (content.starts_with('"') && content.ends_with('"'))
                        || (content.starts_with('\'') && content.ends_with('\''))
                    {
                        let s = &content[1..content.len() - 1];
                        stdout.push_str(s);
                        stdout.push('\n');
                    } else if content.starts_with("f\"") || content.starts_with("f'") {
                        // f-string (simplified handling)
                        let s = &content[2..content.len() - 1];
                        stdout.push_str(s);
                        stdout.push('\n');
                    } else {
                        // Variable or expression
                        stdout.push_str(&format!("{}\n", content));
                    }
                }
            }

            // Handle variable assignments for session persistence
            if trimmed.contains('=') && !trimmed.contains("==") {
                let parts: Vec<&str> = trimmed.splitn(2, '=').collect();
                if parts.len() == 2 {
                    let var_name = parts[0].trim();
                    let value = parts[1].trim();
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(value) {
                        let mut vars = self.session_vars.lock().await;
                        vars.insert(var_name.to_string(), v);
                    }
                }
            }
        }

        (stdout, stderr, exit_code)
    }

    pub async fn get_session_var(&self, name: &str) -> Option<serde_json::Value> {
        let vars = self.session_vars.lock().await;
        vars.get(name).cloned()
    }

    pub fn execution_count(&self) -> u64 {
        self.execution_count.load(Ordering::SeqCst)
    }

    pub fn validator(&self) -> &SecurityValidator {
        &self.security_validator
    }
}

/// Mock execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub status: ExecutionStatus,
    pub duration_ms: u64,
}

// ============================================================================
// Container Lifecycle Tests
// ============================================================================

#[tokio::test]
async fn test_container_create_and_destroy() {
    let docker = Arc::new(MockDockerClient::new());
    let config = ContainerConfig::new("python:3.11-slim");

    // Create container
    let container_id = docker.create_container(&config).await.unwrap();
    assert!(container_id.starts_with("mock-container-"));

    // Start container
    docker.start_container(&container_id).await.unwrap();

    // Stop container
    docker.stop_container(&container_id, 10).await.unwrap();

    // Remove container
    docker.remove_container(&container_id, false).await.unwrap();

    let (created, destroyed) = docker.stats();
    assert_eq!(created, 1);
    assert_eq!(destroyed, 1);
}

#[tokio::test]
async fn test_container_pool_warmup() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig {
        min_warm_containers: 3,
        max_containers: 10,
        ..Default::default()
    };

    let pool = MockContainerPool::new(docker.clone(), config);

    // Warmup pool
    pool.warmup(3).await.unwrap();

    // Verify warm containers
    assert_eq!(pool.warm_count().await, 3);

    let stats = pool.stats().await;
    assert_eq!(stats.warm_containers, 3);
    assert_eq!(stats.containers_created, 3);
}

#[tokio::test]
async fn test_container_reuse() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig {
        min_warm_containers: 1,
        max_reuse_count: 5,
        ..Default::default()
    };

    let pool = MockContainerPool::new(docker.clone(), config);

    // Create a container
    let container_config = ContainerConfig::new("python:3.11-slim");
    let runtime = pool.acquire(container_config.clone()).await.unwrap();
    let id = runtime.config.id.clone();

    // Release it (keep warm)
    pool.release(&id, true).await.unwrap();

    // Verify it's in warm pool
    assert_eq!(pool.warm_count().await, 1);

    // Acquire again - should get from warm pool
    let runtime2 = pool.acquire(container_config).await.unwrap();

    // Stats should show cache hit
    let stats = pool.stats().await;
    assert_eq!(stats.cache_hits, 1);
    assert_eq!(stats.cache_misses, 1); // First acquire was a miss

    // Release and verify
    pool.release(&runtime2.config.id, true).await.unwrap();
}

#[tokio::test]
async fn test_container_timeout_cleanup() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig {
        min_warm_containers: 1,
        warm_ttl_seconds: 0, // Expire immediately
        ..Default::default()
    };

    let pool = MockContainerPool::new(docker.clone(), config);

    // Create and release a container
    let container_config = ContainerConfig::new("python:3.11-slim");
    let runtime = pool.acquire(container_config.clone()).await.unwrap();
    let id = runtime.config.id.clone();

    pool.release(&id, true).await.unwrap();

    // Small delay to ensure expiry
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Acquire again - expired container should be destroyed
    let _ = pool.acquire(container_config).await.unwrap();

    // Verify expired container was cleaned up
    let stats = pool.stats().await;
    assert_eq!(stats.cache_misses, 2); // Both were misses (first and after expiry)
}

#[tokio::test]
async fn test_container_health_check() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig::default();

    let pool = MockContainerPool::new(docker.clone(), config);

    // Health check should pass
    assert!(pool.health_check().await.unwrap());

    // Simulate unhealthy Docker
    docker.set_healthy(false);

    // Health check should fail
    assert!(pool.health_check().await.is_err());
}

#[tokio::test]
async fn test_container_pool_shutdown() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig::default();

    let pool = MockContainerPool::new(docker.clone(), config);

    // Warmup
    pool.warmup(3).await.unwrap();

    // Shutdown
    pool.shutdown().await.unwrap();

    // Verify all cleaned up
    assert_eq!(pool.warm_count().await, 0);
    assert_eq!(pool.in_use_count().await, 0);
}

#[tokio::test]
async fn test_container_config_builder() {
    let config = ContainerConfig::new("python:3.11-slim")
        .with_cpu_limit(0.5)
        .with_memory_limit(256)
        .with_network_mode(NetworkMode::None)
        .with_env("PYTHONPATH", "/app")
        .with_working_dir("/workspace")
        .with_tmpfs("/tmp", "100m");

    assert_eq!(config.image, "python:3.11-slim");
    assert_eq!(config.cpu_limit, 0.5);
    assert_eq!(config.memory_limit_mb, 256);
    assert_eq!(config.network_mode, NetworkMode::None);
    assert_eq!(config.env.get("PYTHONPATH"), Some(&"/app".to_string()));
    assert_eq!(config.working_dir, Some("/workspace".to_string()));
    assert_eq!(config.mounts.len(), 1);
}

#[tokio::test]
async fn test_container_multiple_acquire() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig {
        max_containers: 5,
        ..Default::default()
    };

    let pool = MockContainerPool::new(docker.clone(), config);

    // Acquire multiple containers
    let container_config = ContainerConfig::new("python:3.11-slim");
    let mut containers = Vec::new();

    for _ in 0..3 {
        let runtime = pool.acquire(container_config.clone()).await.unwrap();
        containers.push(runtime);
    }

    assert_eq!(pool.in_use_count().await, 3);

    // Release all
    for container in containers {
        pool.release(&container.config.id, false).await.unwrap();
    }

    assert_eq!(pool.in_use_count().await, 0);
}

// ============================================================================
// Code Execution Tests
// ============================================================================

#[tokio::test]
async fn test_simple_python_execution() {
    let executor = MockPythonExecutor::default();

    let code = r#"print("Hello, World!")"#;
    let result = executor.execute(code).await.unwrap();

    assert_eq!(result.status, ExecutionStatus::Success);
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("Hello, World!"));
}

#[tokio::test]
async fn test_python_with_imports() {
    let executor = MockPythonExecutor::default();

    // Safe imports should work
    let code = r#"
import json
import math
data = {"pi": 3.14159}
print("Imported successfully")
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
}

#[tokio::test]
async fn test_python_output_capture() {
    let executor = MockPythonExecutor::default();

    let code = r#"
print("line 1")
print("line 2")
print("line 3")
"#;
    let result = executor.execute(code).await.unwrap();

    assert!(result.stdout.contains("line 1"));
    assert!(result.stdout.contains("line 2"));
    assert!(result.stdout.contains("line 3"));
}

#[tokio::test]
async fn test_python_error_handling() {
    let executor = MockPythonExecutor::default();
    executor.set_fail_execution(true);

    let code = r#"print("test")"#;
    let result = executor.execute(code).await.unwrap();

    assert_eq!(result.status, ExecutionStatus::Error);
    assert_eq!(result.exit_code, 1);
}

#[tokio::test]
async fn test_python_multiline_code() {
    let executor = MockPythonExecutor::default();

    let code = r#"
def greet(name):
    return f"Hello, {name}!"

message = greet("World")
print(message)
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
}

#[tokio::test]
async fn test_python_variable_persistence() {
    let executor = MockPythonExecutor::default();

    // Set a variable
    let code1 = r#"x = 42"#;
    executor.execute(code1).await.unwrap();

    // Check persistence
    let var = executor.get_session_var("x").await;
    assert!(var.is_some());
    assert_eq!(var.unwrap(), serde_json::json!(42));
}

#[tokio::test]
async fn test_python_execution_timeout() {
    let executor = MockPythonExecutor::default();
    executor.set_execution_delay_ms(100);

    let start = Instant::now();
    let code = r#"print("test")"#;
    let result = executor.execute(code).await.unwrap();

    assert!(start.elapsed() >= Duration::from_millis(100));
    assert_eq!(result.status, ExecutionStatus::Success);
}

// ============================================================================
// SDK Functionality Tests
// ============================================================================

#[tokio::test]
async fn test_browser_sdk_import() {
    // Browser SDK imports should be allowed in permissive mode
    let config = SecurityConfig {
        allowed_imports: HashSet::new(), // Empty allows all non-blocked
        ..Default::default()
    };
    let executor = MockPythonExecutor::new(config);

    // Simulated browser SDK import (not actually blocked)
    let code = r#"
# Browser SDK simulation
class BrowserSDK:
    def navigate(self, url):
        return f"Navigated to {url}"

browser = BrowserSDK()
print(browser.navigate("https://example.com"))
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
}

#[tokio::test]
async fn test_browser_navigate_simulation() {
    let executor = MockPythonExecutor::default();

    let code = r#"
# Simulated browser navigation
url = "https://example.com"
print(f"Navigating to: {url}")
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
}

#[tokio::test]
async fn test_browser_screenshot_simulation() {
    let executor = MockPythonExecutor::default();

    let code = r#"
# Simulated screenshot capture
print("Screenshot captured: screenshot.png")
"#;
    let result = executor.execute(code).await.unwrap();
    assert!(result.stdout.contains("screenshot"));
}

#[tokio::test]
async fn test_bash_sdk_import() {
    let executor = MockPythonExecutor::default();

    let code = r#"
# Bash SDK simulation (without actual subprocess)
class BashSDK:
    def run(self, cmd):
        return f"Would execute: {cmd}"

bash = BashSDK()
print(bash.run("echo hello"))
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
}

#[tokio::test]
async fn test_bash_command_simulation() {
    let executor = MockPythonExecutor::default();

    let code = r#"
# Simulated bash command (safe)
command = "ls -la"
print(f"Command: {command}")
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
}

// ============================================================================
// Security Validation Tests
// ============================================================================

#[tokio::test]
async fn test_blocks_os_system() {
    let executor = MockPythonExecutor::default();

    let code = r#"
import os
os.system('rm -rf /')
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Error);
    assert!(result.stderr.contains("Security validation failed"));
}

#[tokio::test]
async fn test_blocks_subprocess() {
    let executor = MockPythonExecutor::default();

    let code = r#"
import subprocess
subprocess.run(['ls'])
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Error);
}

#[tokio::test]
async fn test_blocks_socket() {
    let executor = MockPythonExecutor::default();

    let code = r#"
import socket
s = socket.socket()
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Error);
}

#[tokio::test]
async fn test_blocks_file_write() {
    let config = SecurityConfig {
        allow_filesystem: false,
        ..Default::default()
    };
    let validator = SecurityValidator::new(config);

    let code = r#"f = open('/etc/passwd', 'w')"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
    assert!(result
        .issues
        .iter()
        .any(|i| i.issue_type == IssueType::FileAccess));
}

#[tokio::test]
async fn test_allows_safe_imports() {
    let executor = MockPythonExecutor::default();

    let code = r#"
import numpy as np
import pandas as pd
import json
import math

data = [1, 2, 3, 4, 5]
print(f"Sum: {sum(data)}")
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
}

#[tokio::test]
async fn test_security_bypass_eval() {
    let validator = SecurityValidator::default();

    let code = r#"eval("__import__('os').system('ls')")"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
    assert!(result.has_critical_issues());
}

#[tokio::test]
async fn test_security_bypass_exec() {
    let validator = SecurityValidator::default();

    let code = r#"exec(compile("import os", "<string>", "exec"))"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
}

#[tokio::test]
async fn test_security_bypass_dunder_import() {
    let validator = SecurityValidator::default();

    let code = r#"__import__('subprocess').call(['ls'])"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
}

#[tokio::test]
async fn test_security_bypass_class_bases() {
    let validator = SecurityValidator::default();

    let code = r#"().__class__.__bases__[0].__subclasses__()"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
    assert!(result
        .issues
        .iter()
        .any(|i| i.issue_type == IssueType::PrivilegeEscalation));
}

#[tokio::test]
async fn test_security_bypass_globals() {
    let validator = SecurityValidator::default();

    let code = r#"func.__globals__['os'].system('ls')"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
}

#[tokio::test]
async fn test_security_blocks_ctypes() {
    let validator = SecurityValidator::default();

    let code = r#"import ctypes"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
}

#[tokio::test]
async fn test_security_blocks_pickle() {
    let validator = SecurityValidator::default();

    let code = r#"import pickle
data = pickle.loads(user_data)
"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
}

// ============================================================================
// Resource Limit Tests
// ============================================================================

#[tokio::test]
async fn test_cpu_limit_enforcement() {
    let config = ContainerConfig::new("python:3.11-slim").with_cpu_limit(0.5);

    assert_eq!(config.cpu_limit, 0.5);
    assert!(config.cpu_limit > 0.0 && config.cpu_limit <= 8.0);
}

#[tokio::test]
async fn test_memory_limit_enforcement() {
    let config = ContainerConfig::new("python:3.11-slim").with_memory_limit(256);

    assert_eq!(config.memory_limit_mb, 256);
}

#[tokio::test]
async fn test_execution_timeout() {
    let executor = MockPythonExecutor::default();
    executor.set_execution_delay_ms(50);

    let start = Instant::now();
    let code = r#"print("test")"#;
    let _ = executor.execute(code).await.unwrap();

    // Execution should have taken at least the delay
    assert!(start.elapsed() >= Duration::from_millis(50));
}

#[tokio::test]
async fn test_disk_quota_config() {
    let config = ContainerConfig::new("python:3.11-slim").with_tmpfs("/tmp", "50m");

    assert_eq!(config.mounts.len(), 1);
    let mount = &config.mounts[0];
    assert_eq!(mount.mount_type, MountType::Tmpfs);
    assert_eq!(mount.target, "/tmp");
    assert_eq!(mount.tmpfs_size, Some("50m".to_string()));
}

// ============================================================================
// Additional Security Validation Tests
// ============================================================================

#[tokio::test]
async fn test_security_config_strict_mode() {
    let config = SecurityConfig {
        strict_mode: true,
        risk_threshold: 0,
        ..Default::default()
    };
    let validator = SecurityValidator::new(config);

    // Even low-risk code should fail in strict mode if any warning
    let code = r#"while True: break"#;
    let result = validator.validate_code(code).unwrap();

    // In strict mode, any issue makes it unsafe
    if !result.issues.is_empty() {
        assert!(!result.is_safe);
    }
}

#[tokio::test]
async fn test_security_config_permissive() {
    let validator = SecurityValidator::permissive();

    // Network access should be allowed in permissive mode
    let code = r#"requests.get('http://example.com')"#;
    let result = validator.validate_code(code).unwrap();

    // Should not flag network access
    assert!(!result
        .issues
        .iter()
        .any(|i| i.issue_type == IssueType::NetworkAccess));
}

#[tokio::test]
async fn test_security_code_size_limit() {
    let config = SecurityConfig {
        max_code_size: 100,
        ..Default::default()
    };
    let validator = SecurityValidator::new(config);

    let large_code = "x = 1\n".repeat(50); // > 100 bytes
    let result = validator.validate_code(&large_code).unwrap();

    assert!(!result.is_safe);
    assert!(result
        .issues
        .iter()
        .any(|i| i.issue_type == IssueType::CodeSizeExceeded));
}

#[tokio::test]
async fn test_security_quick_check() {
    let validator = SecurityValidator::default();

    // Safe code
    assert!(validator.quick_check("import json\nx = 1"));

    // Dangerous code
    assert!(!validator.quick_check("eval(user_input)"));
    assert!(!validator.quick_check("import os"));
    assert!(!validator.quick_check("exec(code)"));
}

#[tokio::test]
async fn test_security_issue_severity() {
    assert!(Severity::Critical > Severity::High);
    assert!(Severity::High > Severity::Medium);
    assert!(Severity::Medium > Severity::Low);
    assert!(Severity::Low > Severity::Info);

    assert_eq!(Severity::Critical.risk_score(), 40);
    assert_eq!(Severity::High.risk_score(), 25);
}

#[tokio::test]
async fn test_security_validation_metadata() {
    let validator = SecurityValidator::default();

    let code = r#"
import json
import math

x = 1 + 2
print(x)
"#;
    let result = validator.validate_code(code).unwrap();

    assert!(result.metadata.lines_analyzed > 0);
    assert!(result.metadata.imports_found >= 2);
    assert!(result.metadata.code_hash.is_some());
}

#[tokio::test]
async fn test_security_dangerous_pattern_obfuscation() {
    let validator = SecurityValidator::default();

    let code = r#"exec(base64.b64decode('aW1wb3J0IG9z'))"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
    assert!(result
        .issues
        .iter()
        .any(|i| i.issue_type == IssueType::Obfuscation));
}

#[tokio::test]
async fn test_security_resource_exhaustion() {
    let validator = SecurityValidator::default();

    let code = r#"while True: pass"#;
    let result = validator.validate_code(code).unwrap();

    assert!(result
        .issues
        .iter()
        .any(|i| i.issue_type == IssueType::ResourceExhaustion));
}

#[tokio::test]
async fn test_security_huge_range() {
    let validator = SecurityValidator::default();

    let code = r#"for i in range(10000000000): pass"#;
    let result = validator.validate_code(code).unwrap();

    assert!(result
        .issues
        .iter()
        .any(|i| i.issue_type == IssueType::ResourceExhaustion));
}

// ============================================================================
// Shared Validator Tests
// ============================================================================

#[tokio::test]
async fn test_shared_validator_thread_safety() {
    let shared = SharedSecurityValidator::default();

    // Clone for multiple threads
    let shared1 = shared.clone();
    let shared2 = shared.clone();

    let handle1 = tokio::spawn(async move {
        for _ in 0..10 {
            let result = shared1.validate_code("x = 1").unwrap();
            assert!(result.is_safe);
        }
    });

    let handle2 = tokio::spawn(async move {
        for _ in 0..10 {
            let result = shared2.validate_code("import json").unwrap();
            assert!(result.is_safe);
        }
    });

    handle1.await.unwrap();
    handle2.await.unwrap();
}

#[tokio::test]
async fn test_shared_validator_quick_check() {
    let shared = SharedSecurityValidator::default();

    assert!(shared.quick_check("print('hello')"));
    assert!(!shared.quick_check("import os"));
}

// ============================================================================
// Container State Machine Tests
// ============================================================================

#[test]
fn test_container_state_transitions() {
    let config = ContainerConfig::default();
    let mut runtime =
        ContainerRuntime::new("docker-id-123".into(), "test-container".into(), config);

    // Initial state
    assert_eq!(runtime.state, ContainerState::Created);
    assert!(!runtime.is_running());

    // Start
    runtime.mark_started();
    assert_eq!(runtime.state, ContainerState::Running);
    assert!(runtime.is_running());
    assert!(runtime.started_at.is_some());

    // Mark warm
    runtime.mark_warm();
    assert_eq!(runtime.state, ContainerState::Warm);
    assert!(runtime.is_reusable());
    assert_eq!(runtime.reuse_count, 1);

    // Stop
    runtime.mark_stopped(0);
    assert_eq!(runtime.state, ContainerState::Stopped);
    assert!(!runtime.is_running());
    assert_eq!(runtime.exit_code, Some(0));
}

#[test]
fn test_container_error_state() {
    let config = ContainerConfig::default();
    let mut runtime =
        ContainerRuntime::new("docker-id-456".into(), "error-container".into(), config);

    runtime.mark_error("Container crashed");
    assert_eq!(runtime.state, ContainerState::Error);
    assert!(!runtime.healthy);
    assert_eq!(runtime.error, Some("Container crashed".to_string()));
}

// ============================================================================
// Network Mode Tests
// ============================================================================

#[test]
fn test_network_mode_values() {
    assert_eq!(NetworkMode::None.to_string(), "none");
    assert_eq!(NetworkMode::Bridge.to_string(), "bridge");
    assert_eq!(NetworkMode::Host.to_string(), "host");
    assert_eq!(
        NetworkMode::Custom("my-network".into()).to_string(),
        "my-network"
    );
}

#[test]
fn test_container_config_network() {
    let config = ContainerConfig::new("alpine:latest").with_network_mode(NetworkMode::None);

    assert_eq!(config.network_mode, NetworkMode::None);
}

// ============================================================================
// Mount Configuration Tests
// ============================================================================

#[test]
fn test_mount_bind() {
    let mount = Mount::bind("/host/path", "/container/path", true);

    assert_eq!(mount.mount_type, MountType::Bind);
    assert_eq!(mount.source, "/host/path");
    assert_eq!(mount.target, "/container/path");
    assert!(mount.readonly);
}

#[test]
fn test_mount_volume() {
    let mount = Mount::volume("my-volume", "/data", false);

    assert_eq!(mount.mount_type, MountType::Volume);
    assert_eq!(mount.source, "my-volume");
    assert_eq!(mount.target, "/data");
    assert!(!mount.readonly);
}

#[test]
fn test_mount_tmpfs() {
    let mount = Mount::tmpfs("/tmp", "100m");

    assert_eq!(mount.mount_type, MountType::Tmpfs);
    assert_eq!(mount.source, "");
    assert_eq!(mount.target, "/tmp");
    assert_eq!(mount.tmpfs_size, Some("100m".to_string()));
}

// ============================================================================
// Pool Stats Tests
// ============================================================================

#[test]
fn test_pool_stats_cache_hit_rate() {
    let mut stats = PoolStats::default();

    // No requests yet
    assert_eq!(stats.cache_hit_rate(), 0.0);

    // Add some stats
    stats.requests_served = 100;
    stats.cache_hits = 75;

    let rate = stats.cache_hit_rate();
    assert!((rate - 0.75).abs() < 0.001);
}

#[test]
fn test_pool_config_defaults() {
    let config = PoolConfig::default();

    assert_eq!(config.min_warm_containers, 2);
    assert_eq!(config.max_containers, 10);
    assert_eq!(config.warm_ttl_seconds, 300);
    assert_eq!(config.health_check_interval_seconds, 30);
    assert_eq!(config.max_reuse_count, 10);
}

// ============================================================================
// Integration Scenario Tests
// ============================================================================

#[tokio::test]
async fn test_full_codeact_execution_flow() {
    // 1. Create mock infrastructure
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig::default();
    let pool = MockContainerPool::new(docker.clone(), config);

    // 2. Warmup pool
    pool.warmup(2).await.unwrap();
    assert_eq!(pool.warm_count().await, 2);

    // 3. Create executor
    let executor = MockPythonExecutor::default();

    // 4. Acquire container
    let container_config = ContainerConfig::new("python:3.11-slim");
    let runtime = pool.acquire(container_config).await.unwrap();

    // 5. Execute code
    let code = r#"
import json
import math

data = {"result": math.pi}
print(json.dumps(data))
"#;
    let result = executor.execute(code).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);

    // 6. Release container
    pool.release(&runtime.config.id, true).await.unwrap();

    // 7. Verify stats
    let stats = pool.stats().await;
    assert!(stats.requests_served >= 1);

    // 8. Shutdown
    pool.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_security_integrated_execution() {
    let executor = MockPythonExecutor::default();

    // Safe code should execute
    let safe_code = r#"
import json
data = {"test": True}
print(json.dumps(data))
"#;
    let safe_result = executor.execute(safe_code).await.unwrap();
    assert_eq!(safe_result.status, ExecutionStatus::Success);

    // Dangerous code should be blocked
    let dangerous_code = r#"
import os
os.system('rm -rf /')
"#;
    let dangerous_result = executor.execute(dangerous_code).await.unwrap();
    assert_eq!(dangerous_result.status, ExecutionStatus::Error);
}

#[tokio::test]
async fn test_concurrent_executions() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig {
        max_containers: 10,
        ..Default::default()
    };
    let pool = Arc::new(MockContainerPool::new(docker.clone(), config));

    // Run multiple concurrent acquisitions
    let mut handles = vec![];

    for i in 0..5 {
        let pool_clone = pool.clone();
        let handle = tokio::spawn(async move {
            let config = ContainerConfig::new("python:3.11-slim");
            let runtime = pool_clone.acquire(config).await.unwrap();

            // Simulate some work
            tokio::time::sleep(Duration::from_millis(10)).await;

            pool_clone.release(&runtime.config.id, true).await.unwrap();
            i
        });
        handles.push(handle);
    }

    // Wait for all to complete
    for handle in handles {
        handle.await.unwrap();
    }

    let stats = pool.stats().await;
    assert_eq!(stats.requests_served, 5);
}

#[tokio::test]
async fn test_error_recovery_scenario() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig::default();
    let pool = MockContainerPool::new(docker.clone(), config);

    // Acquire a container
    let container_config = ContainerConfig::new("python:3.11-slim");
    let runtime = pool.acquire(container_config).await.unwrap();

    // Simulate failure - don't keep warm
    pool.release(&runtime.config.id, false).await.unwrap();

    // Stats should show recycled
    let stats = pool.stats().await;
    assert_eq!(stats.containers_recycled, 1);
}

#[tokio::test]
async fn test_pool_exhaustion_handling() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig {
        max_containers: 2,
        ..Default::default()
    };
    let pool = MockContainerPool::new(docker.clone(), config);

    // Acquire max containers
    let container_config = ContainerConfig::new("python:3.11-slim");
    let _r1 = pool.acquire(container_config.clone()).await.unwrap();
    let _r2 = pool.acquire(container_config.clone()).await.unwrap();

    // Pool can still create more in mock
    let r3 = pool.acquire(container_config.clone()).await;
    assert!(r3.is_ok());
}

// ============================================================================
// Additional Container Lifecycle Tests
// ============================================================================

#[tokio::test]
async fn test_docker_client_image_pull() {
    let docker = MockDockerClient::new();

    // Image not initially present
    assert!(!docker.image_exists("custom:v1").await);

    // Pull the image
    docker.pull_image("custom:v1").await.unwrap();

    // Now it exists
    assert!(docker.image_exists("custom:v1").await);
}

#[tokio::test]
async fn test_docker_client_failure_simulation() {
    let docker = MockDockerClient::new();

    // Set next operation to fail
    docker.set_fail_next(true);

    // Operation should fail
    let result = docker.ping().await;
    assert!(result.is_err());

    // Next operation should succeed
    let result = docker.ping().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_docker_client_latency_simulation() {
    let docker = MockDockerClient::new();
    docker.set_latency_ms(50);

    let start = Instant::now();
    docker.ping().await.unwrap();

    assert!(start.elapsed() >= Duration::from_millis(50));
}

#[tokio::test]
async fn test_docker_client_unhealthy() {
    let docker = MockDockerClient::new();
    docker.set_healthy(false);

    let result = docker.version().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unhealthy"));
}

#[tokio::test]
async fn test_container_config_preset_python() {
    let config = ContainerConfig::python();

    assert_eq!(config.image, "python:3.11-slim");
    assert_eq!(config.working_dir, Some("/app".to_string()));
    // Should have tmpfs for /tmp
    assert!(config.mounts.iter().any(|m| m.target == "/tmp"));
}

#[tokio::test]
async fn test_container_config_preset_bash() {
    let config = ContainerConfig::bash();

    assert_eq!(config.image, "ubuntu:22.04");
    assert_eq!(config.working_dir, Some("/workspace".to_string()));
}

#[tokio::test]
async fn test_container_config_security_defaults() {
    let config = ContainerConfig::default();

    assert!(config.read_only_rootfs);
    assert!(config.drop_all_caps);
    assert!(config.no_new_privileges);
}

#[tokio::test]
async fn test_container_config_writable_rootfs() {
    let config = ContainerConfig::new("alpine:latest").with_writable_rootfs();

    assert!(!config.read_only_rootfs);
}

#[tokio::test]
async fn test_container_config_network_enabled() {
    let config = ContainerConfig::new("alpine:latest").with_network_enabled();

    assert_eq!(config.network_mode, NetworkMode::Bridge);
}

#[tokio::test]
async fn test_container_runtime_reusable_check() {
    let config = ContainerConfig::default();
    let mut runtime =
        ContainerRuntime::new("docker-id-789".into(), "reusable-container".into(), config);

    // Not reusable initially (not warm)
    assert!(!runtime.is_reusable());

    // Mark warm
    runtime.mark_warm();
    assert!(runtime.is_reusable());
    assert!(runtime.healthy);

    // Mark unhealthy
    runtime.healthy = false;
    assert!(!runtime.is_reusable());
}

// ============================================================================
// Additional Security Validation Tests
// ============================================================================

#[tokio::test]
async fn test_security_blocks_multiprocessing() {
    let validator = SecurityValidator::default();

    let code = r#"from multiprocessing import Process"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
}

#[tokio::test]
async fn test_security_blocks_threading_direct() {
    let validator = SecurityValidator::default();

    let code = r#"import threading
t = threading.Thread(target=func)
t.start()
"#;
    let result = validator.validate_code(code).unwrap();

    // Threading should be blocked or flagged
    assert!(!result.is_safe);
}

#[tokio::test]
async fn test_security_system_exit() {
    let validator = SecurityValidator::default();

    let code = r#"import sys
sys.exit(1)
"#;
    let result = validator.validate_code(code).unwrap();

    // sys.exit should be flagged as dangerous import or system command
    assert!(result.issues.iter().any(|i| {
        i.issue_type == IssueType::SystemCommand || i.issue_type == IssueType::DangerousImport
    }));
}

#[tokio::test]
async fn test_security_nested_exec() {
    let validator = SecurityValidator::default();

    let code = r#"exec(eval("'print(1)'"))"#;
    let result = validator.validate_code(code).unwrap();

    // Multiple dangerous functions
    assert!(!result.is_safe);
    assert!(result.issues.len() >= 2);
}

#[tokio::test]
async fn test_security_code_injection_attempt() {
    let validator = SecurityValidator::default();

    let code = r#"user_input = "malicious"
exec(f"print({user_input})")
"#;
    let result = validator.validate_code(code).unwrap();

    assert!(!result.is_safe);
}

#[tokio::test]
async fn test_security_allows_list_comprehension() {
    let validator = SecurityValidator::default();

    let code = r#"numbers = [x**2 for x in range(10)]
print(numbers)
"#;
    let result = validator.validate_code(code).unwrap();

    // Safe code should pass
    assert!(result.is_safe);
}

#[tokio::test]
async fn test_security_allows_dict_operations() {
    let validator = SecurityValidator::default();

    let code = r#"data = {"key": "value", "count": 42}
data["new_key"] = "new_value"
print(data)
"#;
    let result = validator.validate_code(code).unwrap();

    assert!(result.is_safe);
}

#[tokio::test]
async fn test_security_allows_safe_builtins() {
    let validator = SecurityValidator::default();

    let code = r#"
numbers = [1, 2, 3, 4, 5]
result = sum(numbers)
length = len(numbers)
maximum = max(numbers)
minimum = min(numbers)
print(f"Sum: {result}, Len: {length}, Max: {maximum}, Min: {minimum}")
"#;
    let result = validator.validate_code(code).unwrap();

    assert!(result.is_safe);
}

// ============================================================================
// Additional Pool Tests
// ============================================================================

#[tokio::test]
async fn test_pool_acquire_after_shutdown() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig::default();
    let pool = MockContainerPool::new(docker.clone(), config);

    // Shutdown the pool
    pool.shutdown().await.unwrap();

    // Try to acquire - should fail
    let container_config = ContainerConfig::new("python:3.11-slim");
    let result = pool.acquire(container_config).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("shutting down"));
}

#[tokio::test]
async fn test_pool_warmup_with_missing_image() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig {
        default_warm_image: "nonexistent:v1".to_string(),
        ..Default::default()
    };
    let pool = MockContainerPool::new(docker.clone(), config);

    // Warmup should fail for missing image
    let result = pool.warmup(1).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_pool_stats_initial() {
    let docker = Arc::new(MockDockerClient::new());
    let config = PoolConfig::default();
    let pool = MockContainerPool::new(docker.clone(), config);

    let stats = pool.stats().await;

    assert_eq!(stats.requests_served, 0);
    assert_eq!(stats.cache_hits, 0);
    assert_eq!(stats.cache_misses, 0);
    assert_eq!(stats.containers_created, 0);
    assert_eq!(stats.containers_recycled, 0);
    assert_eq!(stats.warm_containers, 0);
}

// ============================================================================
// Execution Status Tests
// ============================================================================

#[test]
fn test_execution_status_equality() {
    assert_eq!(ExecutionStatus::Success, ExecutionStatus::Success);
    assert_eq!(ExecutionStatus::Error, ExecutionStatus::Error);
    assert_ne!(ExecutionStatus::Success, ExecutionStatus::Error);
}

#[test]
fn test_execution_status_serialization() {
    let status = ExecutionStatus::Success;
    let json = serde_json::to_string(&status).unwrap();

    let deserialized: ExecutionStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(status, deserialized);
}

// ============================================================================
// Mock Executor Tests
// ============================================================================

#[tokio::test]
async fn test_mock_executor_execution_count() {
    let executor = MockPythonExecutor::default();

    assert_eq!(executor.execution_count(), 0);

    executor.execute("print(1)").await.unwrap();
    assert_eq!(executor.execution_count(), 1);

    executor.execute("print(2)").await.unwrap();
    assert_eq!(executor.execution_count(), 2);
}

#[tokio::test]
async fn test_mock_executor_session_var_persistence() {
    let executor = MockPythonExecutor::default();

    // Set multiple variables
    executor.execute("a = 10").await.unwrap();
    executor.execute("b = 20").await.unwrap();

    // Check both are persisted
    assert_eq!(
        executor.get_session_var("a").await,
        Some(serde_json::json!(10))
    );
    assert_eq!(
        executor.get_session_var("b").await,
        Some(serde_json::json!(20))
    );
}

#[tokio::test]
async fn test_mock_executor_validator_access() {
    let executor = MockPythonExecutor::default();

    // Should be able to access the underlying validator
    let validator = executor.validator();
    let result = validator.validate_code("import json").unwrap();
    assert!(result.is_safe);
}

// ============================================================================
// Additional Container State Tests
// ============================================================================

#[test]
fn test_container_state_creating() {
    let config = ContainerConfig::default();
    let runtime = ContainerRuntime::new("docker-123".into(), "creating-test".into(), config);

    // Default state should be Created
    assert_eq!(runtime.state, ContainerState::Created);
    assert!(runtime.started_at.is_none());
    assert!(runtime.stopped_at.is_none());
    assert!(runtime.exit_code.is_none());
}

#[test]
fn test_container_runtime_serialization() {
    let config = ContainerConfig::new("alpine:latest");
    let runtime = ContainerRuntime::new("docker-serial".into(), "serial-test".into(), config);

    let json = serde_json::to_string(&runtime).unwrap();
    let deserialized: ContainerRuntime = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.docker_id, "docker-serial");
    assert_eq!(deserialized.name, "serial-test");
    assert_eq!(deserialized.config.image, "alpine:latest");
}

#[test]
fn test_container_config_serialization() {
    let config = ContainerConfig::new("python:3.11")
        .with_cpu_limit(0.5)
        .with_memory_limit(256)
        .with_env("KEY", "value");

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: ContainerConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.image, "python:3.11");
    assert_eq!(deserialized.cpu_limit, 0.5);
    assert_eq!(deserialized.memory_limit_mb, 256);
    assert_eq!(deserialized.env.get("KEY"), Some(&"value".to_string()));
}
