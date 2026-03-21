//! Container Type Definitions
//!
//! This module defines the core types for Docker container lifecycle management,
//! including configuration, state, and network settings.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Network mode for containers
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    /// No network access (most secure)
    None,
    /// Bridge network (default Docker network)
    Bridge,
    /// Host network (least secure, full host access)
    Host,
    /// Custom network by name
    Custom(String),
}

impl Default for NetworkMode {
    fn default() -> Self {
        Self::None
    }
}

impl std::fmt::Display for NetworkMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkMode::None => write!(f, "none"),
            NetworkMode::Bridge => write!(f, "bridge"),
            NetworkMode::Host => write!(f, "host"),
            NetworkMode::Custom(name) => write!(f, "{}", name),
        }
    }
}

/// Mount type for containers
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MountType {
    /// Bind mount from host filesystem
    Bind,
    /// Docker volume
    Volume,
    /// Temporary filesystem (in-memory)
    Tmpfs,
}

impl Default for MountType {
    fn default() -> Self {
        Self::Bind
    }
}

/// Mount configuration for containers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mount {
    /// Mount type
    #[serde(rename = "type")]
    pub mount_type: MountType,
    /// Source path (host path for bind, volume name for volume)
    pub source: String,
    /// Target path inside container
    pub target: String,
    /// Whether the mount is read-only
    #[serde(default)]
    pub readonly: bool,
    /// Tmpfs size limit (e.g., "100m")
    #[serde(default)]
    pub tmpfs_size: Option<String>,
}

impl Mount {
    /// Create a new bind mount
    pub fn bind(source: impl Into<String>, target: impl Into<String>, readonly: bool) -> Self {
        Self {
            mount_type: MountType::Bind,
            source: source.into(),
            target: target.into(),
            readonly,
            tmpfs_size: None,
        }
    }

    /// Create a new volume mount
    pub fn volume(name: impl Into<String>, target: impl Into<String>, readonly: bool) -> Self {
        Self {
            mount_type: MountType::Volume,
            source: name.into(),
            target: target.into(),
            readonly,
            tmpfs_size: None,
        }
    }

    /// Create a new tmpfs mount
    pub fn tmpfs(target: impl Into<String>, size: impl Into<String>) -> Self {
        Self {
            mount_type: MountType::Tmpfs,
            source: String::new(),
            target: target.into(),
            readonly: false,
            tmpfs_size: Some(size.into()),
        }
    }
}

/// Container configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    /// Unique container identifier (used internally)
    pub id: String,
    /// Docker image to use
    pub image: String,
    /// CPU limit in cores (e.g., 1.0, 0.5, 2.0)
    #[serde(default = "default_cpu_limit")]
    pub cpu_limit: f64,
    /// Memory limit in megabytes
    #[serde(default = "default_memory_limit_mb")]
    pub memory_limit_mb: u64,
    /// Network mode
    #[serde(default)]
    pub network_mode: NetworkMode,
    /// Volume mounts
    #[serde(default)]
    pub mounts: Vec<Mount>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// User to run as (format: "uid" or "uid:gid")
    #[serde(default = "default_user")]
    pub user: String,
    /// Working directory inside container
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Command to run
    #[serde(default)]
    pub command: Vec<String>,
    /// Entrypoint override
    #[serde(default)]
    pub entrypoint: Option<Vec<String>>,
    /// Labels for the container
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// Whether to use read-only root filesystem
    #[serde(default = "default_read_only")]
    pub read_only_rootfs: bool,
    /// Drop all capabilities
    #[serde(default = "default_drop_caps")]
    pub drop_all_caps: bool,
    /// No new privileges security option
    #[serde(default = "default_no_new_privileges")]
    pub no_new_privileges: bool,
    /// Execution timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_cpu_limit() -> f64 {
    1.0
}

fn default_memory_limit_mb() -> u64 {
    512
}

fn default_user() -> String {
    "1000:1000".into()
}

fn default_read_only() -> bool {
    true
}

fn default_drop_caps() -> bool {
    true
}

fn default_no_new_privileges() -> bool {
    true
}

fn default_timeout_ms() -> u64 {
    30000 // 30 seconds
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            image: "ubuntu:22.04".into(),
            cpu_limit: default_cpu_limit(),
            memory_limit_mb: default_memory_limit_mb(),
            network_mode: NetworkMode::default(),
            mounts: Vec::new(),
            env: HashMap::new(),
            user: default_user(),
            working_dir: None,
            command: Vec::new(),
            entrypoint: None,
            labels: HashMap::new(),
            read_only_rootfs: default_read_only(),
            drop_all_caps: default_drop_caps(),
            no_new_privileges: default_no_new_privileges(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

impl ContainerConfig {
    /// Create a new container configuration with the given image
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            image: image.into(),
            ..Default::default()
        }
    }

    /// Create a configuration for Python execution
    pub fn python() -> Self {
        Self::new("python:3.11-slim")
            .with_tmpfs("/tmp", "100m")
            .with_working_dir("/app")
    }

    /// Create a configuration for Bash execution
    pub fn bash() -> Self {
        Self::new("ubuntu:22.04")
            .with_tmpfs("/tmp", "100m")
            .with_working_dir("/workspace")
    }

    /// Set CPU limit
    pub fn with_cpu_limit(mut self, limit: f64) -> Self {
        self.cpu_limit = limit;
        self
    }

    /// Set memory limit in MB
    pub fn with_memory_limit(mut self, limit_mb: u64) -> Self {
        self.memory_limit_mb = limit_mb;
        self
    }

    /// Set network mode
    pub fn with_network_mode(mut self, mode: NetworkMode) -> Self {
        self.network_mode = mode;
        self
    }

    /// Add a bind mount
    pub fn with_bind_mount(
        mut self,
        source: impl Into<String>,
        target: impl Into<String>,
        readonly: bool,
    ) -> Self {
        self.mounts.push(Mount::bind(source, target, readonly));
        self
    }

    /// Add a volume mount
    pub fn with_volume_mount(
        mut self,
        name: impl Into<String>,
        target: impl Into<String>,
        readonly: bool,
    ) -> Self {
        self.mounts.push(Mount::volume(name, target, readonly));
        self
    }

    /// Add a tmpfs mount
    pub fn with_tmpfs(mut self, target: impl Into<String>, size: impl Into<String>) -> Self {
        self.mounts.push(Mount::tmpfs(target, size));
        self
    }

    /// Set environment variable
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set multiple environment variables
    pub fn with_envs(mut self, envs: HashMap<String, String>) -> Self {
        self.env.extend(envs);
        self
    }

    /// Set user
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = user.into();
        self
    }

    /// Set working directory
    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Set command
    pub fn with_command(mut self, command: Vec<String>) -> Self {
        self.command = command;
        self
    }

    /// Set entrypoint
    pub fn with_entrypoint(mut self, entrypoint: Vec<String>) -> Self {
        self.entrypoint = Some(entrypoint);
        self
    }

    /// Add a label
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Enable network access (sets bridge mode)
    pub fn with_network_enabled(mut self) -> Self {
        self.network_mode = NetworkMode::Bridge;
        self
    }

    /// Disable read-only root filesystem
    pub fn with_writable_rootfs(mut self) -> Self {
        self.read_only_rootfs = false;
        self
    }
}

/// Container lifecycle state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerState {
    /// Container is being created
    Creating,
    /// Container is created but not started
    Created,
    /// Container is starting up
    Starting,
    /// Container is running
    Running,
    /// Container is paused
    Paused,
    /// Container is stopping
    Stopping,
    /// Container has stopped/exited
    Stopped,
    /// Container is being removed
    Removing,
    /// Container has been removed
    Removed,
    /// Container is in an error state
    Error,
    /// Container is warm and ready for reuse
    Warm,
}

impl Default for ContainerState {
    fn default() -> Self {
        Self::Created
    }
}

/// Container runtime information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerRuntime {
    /// Docker container ID (full 64-char ID)
    pub docker_id: String,
    /// Container name
    pub name: String,
    /// Current state
    pub state: ContainerState,
    /// Container configuration
    pub config: ContainerConfig,
    /// Creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Start timestamp (if started)
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Stop timestamp (if stopped)
    pub stopped_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Exit code (if exited)
    pub exit_code: Option<i32>,
    /// Error message (if in error state)
    pub error: Option<String>,
    /// Health check status
    pub healthy: bool,
    /// Number of times this container has been reused
    pub reuse_count: u32,
}

impl ContainerRuntime {
    /// Create a new container runtime info
    pub fn new(docker_id: String, name: String, config: ContainerConfig) -> Self {
        Self {
            docker_id,
            name,
            state: ContainerState::Created,
            config,
            created_at: chrono::Utc::now(),
            started_at: None,
            stopped_at: None,
            exit_code: None,
            error: None,
            healthy: true,
            reuse_count: 0,
        }
    }

    /// Check if container is in a running state
    pub fn is_running(&self) -> bool {
        matches!(self.state, ContainerState::Running | ContainerState::Warm)
    }

    /// Check if container can be reused
    pub fn is_reusable(&self) -> bool {
        self.state == ContainerState::Warm && self.healthy
    }

    /// Mark container as started
    pub fn mark_started(&mut self) {
        self.state = ContainerState::Running;
        self.started_at = Some(chrono::Utc::now());
    }

    /// Mark container as stopped
    pub fn mark_stopped(&mut self, exit_code: i32) {
        self.state = ContainerState::Stopped;
        self.stopped_at = Some(chrono::Utc::now());
        self.exit_code = Some(exit_code);
    }

    /// Mark container as warm (ready for reuse)
    pub fn mark_warm(&mut self) {
        self.state = ContainerState::Warm;
        self.reuse_count += 1;
    }

    /// Mark container as having an error
    pub fn mark_error(&mut self, error: impl Into<String>) {
        self.state = ContainerState::Error;
        self.error = Some(error.into());
        self.healthy = false;
    }
}

/// Container pool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Minimum number of warm containers to maintain
    #[serde(default = "default_min_warm")]
    pub min_warm_containers: usize,
    /// Maximum number of total containers
    #[serde(default = "default_max_containers")]
    pub max_containers: usize,
    /// How long to keep warm containers before recycling (seconds)
    #[serde(default = "default_warm_ttl")]
    pub warm_ttl_seconds: u64,
    /// Health check interval (seconds)
    #[serde(default = "default_health_check_interval")]
    pub health_check_interval_seconds: u64,
    /// Maximum reuse count before recycling a container
    #[serde(default = "default_max_reuse")]
    pub max_reuse_count: u32,
    /// Container prefix for identification
    #[serde(default = "default_prefix")]
    pub container_prefix: String,
    /// Default image for warm containers
    #[serde(default = "default_warm_image")]
    pub default_warm_image: String,
}

fn default_min_warm() -> usize {
    2
}

fn default_max_containers() -> usize {
    10
}

fn default_warm_ttl() -> u64 {
    300 // 5 minutes
}

fn default_health_check_interval() -> u64 {
    30
}

fn default_max_reuse() -> u32 {
    10
}

fn default_prefix() -> String {
    "canal-pool".into()
}

fn default_warm_image() -> String {
    "python:3.11-slim".into()
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min_warm_containers: default_min_warm(),
            max_containers: default_max_containers(),
            warm_ttl_seconds: default_warm_ttl(),
            health_check_interval_seconds: default_health_check_interval(),
            max_reuse_count: default_max_reuse(),
            container_prefix: default_prefix(),
            default_warm_image: default_warm_image(),
        }
    }
}

/// Container pool statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolStats {
    /// Total containers managed
    pub total_containers: usize,
    /// Running containers
    pub running_containers: usize,
    /// Warm (idle) containers
    pub warm_containers: usize,
    /// Total containers created
    pub containers_created: u64,
    /// Total containers recycled
    pub containers_recycled: u64,
    /// Total requests served
    pub requests_served: u64,
    /// Cache hits (reused warm containers)
    pub cache_hits: u64,
    /// Cache misses (had to create new container)
    pub cache_misses: u64,
    /// Health check failures
    pub health_check_failures: u64,
}

impl PoolStats {
    /// Calculate cache hit rate
    pub fn cache_hit_rate(&self) -> f64 {
        if self.requests_served == 0 {
            0.0
        } else {
            self.cache_hits as f64 / self.requests_served as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_mode_display() {
        assert_eq!(NetworkMode::None.to_string(), "none");
        assert_eq!(NetworkMode::Bridge.to_string(), "bridge");
        assert_eq!(NetworkMode::Host.to_string(), "host");
        assert_eq!(NetworkMode::Custom("mynet".into()).to_string(), "mynet");
    }

    #[test]
    fn test_mount_constructors() {
        let bind = Mount::bind("/host/path", "/container/path", true);
        assert_eq!(bind.mount_type, MountType::Bind);
        assert_eq!(bind.source, "/host/path");
        assert_eq!(bind.target, "/container/path");
        assert!(bind.readonly);

        let volume = Mount::volume("myvolume", "/data", false);
        assert_eq!(volume.mount_type, MountType::Volume);
        assert_eq!(volume.source, "myvolume");

        let tmpfs = Mount::tmpfs("/tmp", "100m");
        assert_eq!(tmpfs.mount_type, MountType::Tmpfs);
        assert_eq!(tmpfs.tmpfs_size, Some("100m".into()));
    }

    #[test]
    fn test_container_config_builder() {
        let config = ContainerConfig::new("python:3.11")
            .with_cpu_limit(0.5)
            .with_memory_limit(256)
            .with_env("PYTHONPATH", "/app")
            .with_tmpfs("/tmp", "50m")
            .with_working_dir("/app");

        assert_eq!(config.image, "python:3.11");
        assert_eq!(config.cpu_limit, 0.5);
        assert_eq!(config.memory_limit_mb, 256);
        assert_eq!(config.env.get("PYTHONPATH"), Some(&"/app".to_string()));
        assert_eq!(config.mounts.len(), 1);
        assert_eq!(config.working_dir, Some("/app".into()));
    }

    #[test]
    fn test_container_config_presets() {
        let python = ContainerConfig::python();
        assert_eq!(python.image, "python:3.11-slim");
        assert_eq!(python.working_dir, Some("/app".into()));

        let bash = ContainerConfig::bash();
        assert_eq!(bash.image, "ubuntu:22.04");
        assert_eq!(bash.working_dir, Some("/workspace".into()));
    }

    #[test]
    fn test_container_state_transitions() {
        let config = ContainerConfig::default();
        let mut runtime = ContainerRuntime::new("abc123".into(), "test-container".into(), config);

        assert_eq!(runtime.state, ContainerState::Created);
        assert!(!runtime.is_running());

        runtime.mark_started();
        assert_eq!(runtime.state, ContainerState::Running);
        assert!(runtime.is_running());
        assert!(runtime.started_at.is_some());

        runtime.mark_warm();
        assert_eq!(runtime.state, ContainerState::Warm);
        assert!(runtime.is_reusable());
        assert_eq!(runtime.reuse_count, 1);

        runtime.mark_stopped(0);
        assert_eq!(runtime.state, ContainerState::Stopped);
        assert!(!runtime.is_running());
        assert_eq!(runtime.exit_code, Some(0));
    }

    #[test]
    fn test_pool_stats_cache_hit_rate() {
        let mut stats = PoolStats::default();
        assert_eq!(stats.cache_hit_rate(), 0.0);

        stats.requests_served = 100;
        stats.cache_hits = 75;
        assert!((stats.cache_hit_rate() - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_container_config_serialization() {
        let config = ContainerConfig::new("alpine:latest")
            .with_cpu_limit(0.5)
            .with_env("FOO", "bar");

        let json = serde_json::to_string(&config).unwrap();
        let parsed: ContainerConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.image, "alpine:latest");
        assert_eq!(parsed.cpu_limit, 0.5);
        assert_eq!(parsed.env.get("FOO"), Some(&"bar".to_string()));
    }

    #[test]
    fn test_pool_config_defaults() {
        let config = PoolConfig::default();
        assert_eq!(config.min_warm_containers, 2);
        assert_eq!(config.max_containers, 10);
        assert_eq!(config.warm_ttl_seconds, 300);
        assert_eq!(config.max_reuse_count, 10);
    }
}
