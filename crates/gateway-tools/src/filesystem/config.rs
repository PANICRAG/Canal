//! Filesystem Configuration
//!
//! Configuration types for the filesystem service including directory permissions,
//! file size limits, and sensitive file patterns.

use serde::{Deserialize, Serialize};

/// Main configuration for the filesystem service
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemConfig {
    /// Whether filesystem access is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Allowed directories with their permissions
    #[serde(default)]
    pub allowed_directories: Vec<DirectoryConfig>,

    /// Patterns for files that should be blocked
    #[serde(default = "default_blocked_patterns")]
    pub blocked_patterns: Vec<String>,

    /// Maximum file size for reading (in bytes)
    #[serde(default = "default_max_read_bytes")]
    pub max_read_bytes: u64,

    /// Maximum file size for writing (in bytes)
    #[serde(default = "default_max_write_bytes")]
    pub max_write_bytes: u64,

    /// Whether to follow symlinks
    #[serde(default = "default_follow_symlinks")]
    pub follow_symlinks: bool,

    /// Default encoding for text files
    #[serde(default = "default_encoding")]
    pub default_encoding: String,
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_directories: vec![],
            blocked_patterns: default_blocked_patterns(),
            max_read_bytes: default_max_read_bytes(),
            max_write_bytes: default_max_write_bytes(),
            follow_symlinks: true,
            default_encoding: default_encoding(),
        }
    }
}

/// Configuration for an allowed directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryConfig {
    /// Directory path (absolute)
    pub path: String,

    /// Access mode (read-only or read-write)
    #[serde(default)]
    pub mode: DirectoryMode,

    /// Optional description
    #[serde(default)]
    pub description: Option<String>,

    /// Path to mount in Docker container (if used with executor)
    #[serde(default)]
    pub docker_mount_path: Option<String>,
}

/// Directory access mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DirectoryMode {
    /// Read-only access
    #[default]
    #[serde(rename = "ro")]
    ReadOnly,

    /// Read-write access
    #[serde(rename = "rw")]
    ReadWrite,
}

impl DirectoryMode {
    /// Check if this mode allows reading
    pub fn can_read(&self) -> bool {
        true // Both modes allow reading
    }

    /// Check if this mode allows writing
    pub fn can_write(&self) -> bool {
        matches!(self, DirectoryMode::ReadWrite)
    }
}

impl std::fmt::Display for DirectoryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirectoryMode::ReadOnly => write!(f, "ro"),
            DirectoryMode::ReadWrite => write!(f, "rw"),
        }
    }
}

// Default value functions
fn default_enabled() -> bool {
    true
}

fn default_blocked_patterns() -> Vec<String> {
    vec![
        // Environment and secrets
        ".env".to_string(),
        ".env.*".to_string(),
        "*.env".to_string(),
        // Credentials
        "credentials*".to_string(),
        "*credentials*".to_string(),
        // Keys and certificates
        "*.key".to_string(),
        "*.pem".to_string(),
        "*.p12".to_string(),
        "*.pfx".to_string(),
        "*.crt".to_string(),
        "id_rsa*".to_string(),
        "id_ed25519*".to_string(),
        "id_ecdsa*".to_string(),
        "*.ppk".to_string(),
        // Password files
        "*password*".to_string(),
        "*secret*".to_string(),
        // Config files with sensitive data
        ".npmrc".to_string(),
        ".pypirc".to_string(),
        ".netrc".to_string(),
        ".gitconfig".to_string(),
        // AWS credentials
        ".aws/credentials".to_string(),
        ".aws/config".to_string(),
        // SSH
        ".ssh/*".to_string(),
        // GPG
        ".gnupg/*".to_string(),
        // Docker
        ".docker/config.json".to_string(),
        // Kubernetes
        ".kube/config".to_string(),
        // History files
        ".bash_history".to_string(),
        ".zsh_history".to_string(),
        ".python_history".to_string(),
        ".node_repl_history".to_string(),
    ]
}

fn default_max_read_bytes() -> u64 {
    10 * 1024 * 1024 // 10MB
}

fn default_max_write_bytes() -> u64 {
    5 * 1024 * 1024 // 5MB
}

fn default_follow_symlinks() -> bool {
    true
}

fn default_encoding() -> String {
    "utf-8".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FilesystemConfig::default();
        assert!(config.enabled);
        assert!(config.allowed_directories.is_empty());
        assert!(!config.blocked_patterns.is_empty());
    }

    #[test]
    fn test_directory_mode() {
        let ro = DirectoryMode::ReadOnly;
        let rw = DirectoryMode::ReadWrite;

        assert!(ro.can_read());
        assert!(!ro.can_write());
        assert!(rw.can_read());
        assert!(rw.can_write());
    }

    #[test]
    fn test_blocked_patterns_include_sensitive() {
        let patterns = default_blocked_patterns();
        assert!(patterns.contains(&".env".to_string()));
        assert!(patterns.contains(&"*.key".to_string()));
        assert!(patterns.contains(&"*password*".to_string()));
    }

    #[test]
    fn test_config_serialization() {
        let config = FilesystemConfig {
            enabled: true,
            allowed_directories: vec![DirectoryConfig {
                path: "/data/projects".to_string(),
                mode: DirectoryMode::ReadWrite,
                description: Some("Project files".to_string()),
                docker_mount_path: Some("/workspace".to_string()),
            }],
            ..Default::default()
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: FilesystemConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.allowed_directories.len(), 1);
        assert_eq!(parsed.allowed_directories[0].mode, DirectoryMode::ReadWrite);
    }
}
