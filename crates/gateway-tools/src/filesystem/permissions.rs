//! Permission Guard
//!
//! Validates file system access permissions, handles path resolution,
//! and blocks access to sensitive files.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use std::path::{Path, PathBuf};

use super::config::{DirectoryMode, FilesystemConfig};

/// Permission guard for validating file system access
pub struct PermissionGuard {
    config: FilesystemConfig,
}

impl PermissionGuard {
    /// Create a new permission guard with the given configuration
    pub fn new(config: FilesystemConfig) -> Self {
        Self { config }
    }

    /// Check if a path is readable
    pub fn can_read(&self, path: &str) -> bool {
        if !self.config.enabled {
            return false;
        }

        // Check if path is blocked
        if self.is_blocked(path) {
            return false;
        }

        // Check if path is in an allowed directory
        self.get_directory_for_path(path).is_some()
    }

    /// Check if a path is writable
    pub fn can_write(&self, path: &str) -> bool {
        if !self.config.enabled {
            return false;
        }

        // Check if path is blocked
        if self.is_blocked(path) {
            return false;
        }

        // Check if path is in an allowed directory with write permission
        self.get_directory_for_path(path)
            .map(|dir| dir.mode.can_write())
            .unwrap_or(false)
    }

    /// Get the directory configuration for a path
    fn get_directory_for_path(&self, path: &str) -> Option<&super::config::DirectoryConfig> {
        let path = Path::new(path);

        // Canonicalize to resolve symlinks and relative paths.
        // If canonicalization fails (e.g., path doesn't exist yet for writes),
        // canonicalize the parent directory and append the filename to prevent traversal.
        let canonical = match path.canonicalize() {
            Ok(c) => c,
            Err(_) => {
                if let Some(parent) = path.parent() {
                    match parent.canonicalize() {
                        Ok(canon_parent) => {
                            if let Some(file_name) = path.file_name() {
                                canon_parent.join(file_name)
                            } else {
                                return None; // No filename component — reject
                            }
                        }
                        Err(_) => return None, // Cannot resolve parent — reject
                    }
                } else {
                    return None; // No parent — reject
                }
            }
        };

        for dir in &self.config.allowed_directories {
            let dir_path = Path::new(&dir.path);
            let dir_canonical = dir_path
                .canonicalize()
                .unwrap_or_else(|_| dir_path.to_path_buf());

            if canonical.starts_with(&dir_canonical) {
                return Some(dir);
            }
        }

        None
    }

    /// Check if a path matches any blocked pattern
    pub fn is_blocked(&self, path: &str) -> bool {
        let path = Path::new(path);
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        for pattern in &self.config.blocked_patterns {
            if self.matches_pattern(file_name, pattern) {
                return true;
            }
            // Also check against the full path
            if self.matches_pattern(path.to_str().unwrap_or(""), pattern) {
                return true;
            }
        }

        false
    }

    /// Simple glob-like pattern matching
    fn matches_pattern(&self, text: &str, pattern: &str) -> bool {
        // Handle exact match
        if pattern == text {
            return true;
        }

        // Handle * at start and end
        if pattern.starts_with('*') && pattern.ends_with('*') {
            let inner = &pattern[1..pattern.len() - 1];
            return text.contains(inner);
        }

        // Handle * at start
        if pattern.starts_with('*') {
            let suffix = &pattern[1..];
            return text.ends_with(suffix);
        }

        // Handle * at end
        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            return text.starts_with(prefix);
        }

        // Handle .* pattern (e.g., .env.*)
        if pattern.contains(".*") {
            let parts: Vec<&str> = pattern.split(".*").collect();
            if parts.len() == 2 {
                return text.starts_with(parts[0]) && !text.ends_with(parts[0]);
            }
        }

        // R5-M: Handle mid-pattern * (e.g., "secret*.json" matches "secret_key.json")
        if let Some(star_pos) = pattern.find('*') {
            let prefix = &pattern[..star_pos];
            let suffix = &pattern[star_pos + 1..];
            return text.starts_with(prefix)
                && text.ends_with(suffix)
                && text.len() >= prefix.len() + suffix.len();
        }

        false
    }

    /// Resolve a path and validate it's within allowed boundaries
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);

        // Check for path traversal attempts
        if self.has_path_traversal(path) {
            return Err(Error::Internal("Path traversal not allowed".into()));
        }

        // Canonicalize the path
        let canonical = if path.exists() {
            path.canonicalize()
                .map_err(|e| Error::Internal(format!("Failed to resolve path: {}", e)))?
        } else {
            // For non-existent paths, resolve parent and append filename
            if let Some(parent) = path.parent() {
                if parent.exists() {
                    let parent_canonical = parent.canonicalize().map_err(|e| {
                        Error::Internal(format!("Failed to resolve parent path: {}", e))
                    })?;
                    if let Some(filename) = path.file_name() {
                        parent_canonical.join(filename)
                    } else {
                        return Err(Error::Internal("Invalid path".into()));
                    }
                } else {
                    // Parent doesn't exist, return the original path
                    path.to_path_buf()
                }
            } else {
                path.to_path_buf()
            }
        };

        // Verify the path is within an allowed directory
        let path_str = canonical.to_string_lossy().to_string();
        if !self.can_read(&path_str) {
            return Err(Error::NotFound(format!(
                "Path not accessible: {}",
                path_str
            )));
        }

        Ok(canonical)
    }

    /// Check if a path contains path traversal attempts
    fn has_path_traversal(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        // Check for obvious traversal patterns
        if path_str.contains("..") {
            // Allow .. only if it resolves within allowed directories
            if let Ok(canonical) = path.canonicalize() {
                let canonical_str = canonical.to_string_lossy().to_string();
                return !self.is_in_allowed_directory(&canonical_str);
            }
            return true;
        }

        false
    }

    /// Check if a path is within an allowed directory
    fn is_in_allowed_directory(&self, path: &str) -> bool {
        let path = Path::new(path);

        for dir in &self.config.allowed_directories {
            let dir_path = Path::new(&dir.path);
            if let Ok(dir_canonical) = dir_path.canonicalize() {
                if path.starts_with(&dir_canonical) {
                    return true;
                }
            }
        }

        false
    }

    /// Get the mode for a path
    pub fn get_mode(&self, path: &str) -> Option<DirectoryMode> {
        self.get_directory_for_path(path).map(|dir| dir.mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::config::DirectoryConfig;

    fn create_test_guard() -> PermissionGuard {
        let config = FilesystemConfig {
            enabled: true,
            allowed_directories: vec![
                DirectoryConfig {
                    path: "/tmp/test-data".to_string(),
                    mode: DirectoryMode::ReadWrite,
                    description: None,
                    docker_mount_path: None,
                },
                DirectoryConfig {
                    path: "/tmp/readonly".to_string(),
                    mode: DirectoryMode::ReadOnly,
                    description: None,
                    docker_mount_path: None,
                },
            ],
            ..Default::default()
        };
        PermissionGuard::new(config)
    }

    #[test]
    fn test_matches_pattern_exact() {
        let guard = create_test_guard();
        assert!(guard.matches_pattern(".env", ".env"));
        assert!(!guard.matches_pattern(".env", ".envrc"));
    }

    #[test]
    fn test_matches_pattern_wildcard_start() {
        let guard = create_test_guard();
        assert!(guard.matches_pattern("test.key", "*.key"));
        assert!(guard.matches_pattern("private.key", "*.key"));
        assert!(!guard.matches_pattern("test.pem", "*.key"));
    }

    #[test]
    fn test_matches_pattern_wildcard_end() {
        let guard = create_test_guard();
        assert!(guard.matches_pattern("credentials.json", "credentials*"));
        assert!(guard.matches_pattern("credentials", "credentials*"));
        assert!(!guard.matches_pattern("creds.json", "credentials*"));
    }

    #[test]
    fn test_matches_pattern_wildcard_both() {
        let guard = create_test_guard();
        assert!(guard.matches_pattern("mypassword.txt", "*password*"));
        assert!(guard.matches_pattern("password", "*password*"));
        assert!(guard.matches_pattern("super_secret_password_file", "*password*"));
    }

    #[test]
    fn test_is_blocked() {
        let guard = create_test_guard();
        assert!(guard.is_blocked(".env"));
        assert!(guard.is_blocked("test.key"));
        assert!(guard.is_blocked("my_password.txt"));
        assert!(!guard.is_blocked("readme.md"));
        assert!(!guard.is_blocked("main.rs"));
    }
}
