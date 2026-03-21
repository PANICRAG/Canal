//! Executor Configuration
//!
//! Configuration types for the code execution system including Docker settings,
//! language-specific options, and resource limits.

use serde::{Deserialize, Serialize};

/// Main configuration for the code executor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    /// Whether code execution is globally enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Docker configuration
    #[serde(default)]
    pub docker: DockerConfig,

    /// Python executor configuration
    #[serde(default)]
    pub python: PythonConfig,

    /// Bash executor configuration
    #[serde(default)]
    pub bash: BashConfig,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            docker: DockerConfig::default(),
            python: PythonConfig::default(),
            bash: BashConfig::default(),
        }
    }
}

/// Docker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Whether Docker isolation is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Docker socket path (default: /var/run/docker.sock)
    #[serde(default = "default_docker_socket")]
    pub socket_path: String,

    /// Container prefix for identifying executor containers
    #[serde(default = "default_container_prefix")]
    pub container_prefix: String,

    /// Network mode (default: none for isolation)
    #[serde(default = "default_network_mode")]
    pub network_mode: String,

    /// Default resource limits
    #[serde(default)]
    pub default_limits: ResourceLimits,

    /// Container auto-cleanup timeout in seconds
    #[serde(default = "default_cleanup_timeout")]
    pub cleanup_timeout_seconds: u64,

    /// Maximum concurrent containers
    #[serde(default = "default_max_containers")]
    pub max_concurrent_containers: usize,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            socket_path: default_docker_socket(),
            container_prefix: default_container_prefix(),
            network_mode: default_network_mode(),
            default_limits: ResourceLimits::default(),
            cleanup_timeout_seconds: default_cleanup_timeout(),
            max_concurrent_containers: default_max_containers(),
        }
    }
}

/// Resource limits for container execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Memory limit (e.g., "512m", "1g")
    #[serde(default = "default_memory_limit")]
    pub memory: String,

    /// CPU limit (e.g., "1.0", "0.5")
    #[serde(default = "default_cpu_limit")]
    pub cpu: String,

    /// Whether to use read-only root filesystem
    #[serde(default = "default_read_only")]
    pub read_only_rootfs: bool,

    /// Temporary filesystem mounts (e.g., /tmp)
    #[serde(default = "default_tmpfs")]
    pub tmpfs_mounts: Vec<TmpfsMount>,

    /// User ID to run as (non-root)
    #[serde(default = "default_user_id")]
    pub user_id: u32,

    /// Group ID to run as
    #[serde(default = "default_group_id")]
    pub group_id: u32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory: default_memory_limit(),
            cpu: default_cpu_limit(),
            read_only_rootfs: true,
            tmpfs_mounts: default_tmpfs(),
            user_id: default_user_id(),
            group_id: default_group_id(),
        }
    }
}

/// Tmpfs mount configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmpfsMount {
    /// Mount path in container
    pub path: String,
    /// Size limit (e.g., "100m")
    pub size: String,
}

/// Base trait for language configuration
pub trait LanguageConfig {
    fn enabled(&self) -> bool;
    fn timeout_ms(&self) -> u64;
    fn docker_image(&self) -> &str;
}

/// Python executor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonConfig {
    /// Whether Python execution is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Default execution timeout in milliseconds
    #[serde(default = "default_python_timeout")]
    pub timeout_ms: u64,

    /// Docker image for Python execution
    #[serde(default = "default_python_image")]
    pub docker_image: String,

    /// Pre-installed packages (pip packages)
    #[serde(default)]
    pub preinstalled_packages: Vec<String>,

    /// Resource limits (overrides defaults)
    #[serde(default)]
    pub limits: Option<ResourceLimits>,
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: default_python_timeout(),
            docker_image: default_python_image(),
            preinstalled_packages: vec![
                "numpy".into(),
                "pandas".into(),
                "matplotlib".into(),
                "requests".into(),
            ],
            limits: None,
        }
    }
}

impl LanguageConfig for PythonConfig {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    fn docker_image(&self) -> &str {
        &self.docker_image
    }
}

/// Bash executor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashConfig {
    /// Whether Bash execution is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Default execution timeout in milliseconds
    #[serde(default = "default_bash_timeout")]
    pub timeout_ms: u64,

    /// Docker image for Bash execution
    #[serde(default = "default_bash_image")]
    pub docker_image: String,

    /// Allowed commands whitelist
    #[serde(default = "default_allowed_commands")]
    pub allowed_commands: Vec<String>,

    /// Blocked command patterns
    #[serde(default = "default_blocked_patterns")]
    pub blocked_patterns: Vec<String>,

    /// Resource limits (overrides defaults)
    #[serde(default)]
    pub limits: Option<ResourceLimits>,
}

impl Default for BashConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: default_bash_timeout(),
            docker_image: default_bash_image(),
            allowed_commands: default_allowed_commands(),
            blocked_patterns: default_blocked_patterns(),
            limits: None,
        }
    }
}

impl LanguageConfig for BashConfig {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    fn docker_image(&self) -> &str {
        &self.docker_image
    }
}

// Default value functions
fn default_enabled() -> bool {
    true
}

fn default_docker_socket() -> String {
    "/var/run/docker.sock".into()
}

fn default_container_prefix() -> String {
    "canal-executor".into()
}

fn default_network_mode() -> String {
    "none".into()
}

fn default_cleanup_timeout() -> u64 {
    300 // 5 minutes
}

fn default_max_containers() -> usize {
    10
}

fn default_memory_limit() -> String {
    "512m".into()
}

fn default_cpu_limit() -> String {
    "1.0".into()
}

fn default_read_only() -> bool {
    true
}

fn default_tmpfs() -> Vec<TmpfsMount> {
    vec![TmpfsMount {
        path: "/tmp".into(),
        size: "100m".into(),
    }]
}

fn default_user_id() -> u32 {
    1000
}

fn default_group_id() -> u32 {
    1000
}

fn default_python_timeout() -> u64 {
    30000 // 30 seconds
}

fn default_python_image() -> String {
    "python:3.11-slim".into()
}

fn default_bash_timeout() -> u64 {
    10000 // 10 seconds
}

fn default_bash_image() -> String {
    "ubuntu:22.04".into()
}

fn default_allowed_commands() -> Vec<String> {
    vec![
        "ls".into(),
        "cat".into(),
        "head".into(),
        "tail".into(),
        "grep".into(),
        "find".into(),
        "wc".into(),
        "echo".into(),
        "pwd".into(),
        "mkdir".into(),
        "touch".into(),
        "cp".into(),
        "mv".into(),
        "sort".into(),
        "uniq".into(),
        "cut".into(),
        "awk".into(),
        "sed".into(),
        "tr".into(),
        "date".into(),
        "env".into(),
        "which".into(),
        "file".into(),
        "stat".into(),
        "du".into(),
        "df".into(),
    ]
}

fn default_blocked_patterns() -> Vec<String> {
    vec![
        "rm -rf /".into(),
        "rm -rf /*".into(),
        "sudo".into(),
        "> /dev".into(),
        ">> /dev".into(),
        "chmod 777".into(),
        "chown root".into(),
        "mkfs".into(),
        "dd if=".into(),
        ":(){ :|:& };:".into(), // Fork bomb
        "wget".into(),
        "curl".into(),
        "nc ".into(),
        "netcat".into(),
        "/etc/passwd".into(),
        "/etc/shadow".into(),
        "eval".into(),
        "exec".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ExecutorConfig::default();
        assert!(config.enabled);
        assert!(config.docker.enabled);
        assert!(config.python.enabled);
        assert!(config.bash.enabled);
        assert_eq!(config.docker.network_mode, "none");
    }

    #[test]
    fn test_resource_limits() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.memory, "512m");
        assert_eq!(limits.cpu, "1.0");
        assert!(limits.read_only_rootfs);
        assert_eq!(limits.user_id, 1000);
    }

    #[test]
    fn test_bash_blocked_patterns() {
        let config = BashConfig::default();
        assert!(config.blocked_patterns.contains(&"sudo".into()));
        assert!(config.blocked_patterns.contains(&"rm -rf /".into()));
    }

    #[test]
    fn test_config_serialization() {
        let config = ExecutorConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: ExecutorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.python.timeout_ms, config.python.timeout_ms);
    }
}
