//! File Writer
//!
//! Provides secure file writing capabilities with size limits and permission checking.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use super::config::FilesystemConfig;
use super::permissions::PermissionGuard;

/// Result of a write operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteResult {
    /// File path
    pub path: String,
    /// Bytes written
    pub bytes_written: u64,
    /// Whether the file was created (vs overwritten)
    pub created: bool,
}

/// File writer with permission checking
pub struct FileWriter {
    permission_guard: Arc<PermissionGuard>,
    config: FilesystemConfig,
}

impl FileWriter {
    /// Create a new file writer
    pub fn new(permission_guard: Arc<PermissionGuard>, config: FilesystemConfig) -> Self {
        Self {
            permission_guard,
            config,
        }
    }

    /// Write content to a file
    pub async fn write(
        &self,
        path: &str,
        content: &str,
        create_dirs: bool,
        overwrite: bool,
    ) -> Result<WriteResult> {
        // Check permission
        if !self.permission_guard.can_write(path) {
            return Err(Error::Internal(format!(
                "Write permission denied for path: {}",
                path
            )));
        }

        // Check content size
        let content_bytes = content.as_bytes();
        if content_bytes.len() as u64 > self.config.max_write_bytes {
            return Err(Error::Internal(format!(
                "Content size ({} bytes) exceeds maximum allowed ({} bytes)",
                content_bytes.len(),
                self.config.max_write_bytes
            )));
        }

        let file_path = Path::new(path);

        // Check if file exists
        let file_exists = file_path.exists();

        if file_exists && !overwrite {
            return Err(Error::Internal(format!(
                "File already exists and overwrite=false: {}",
                path
            )));
        }

        // Create parent directories if requested
        if create_dirs {
            if let Some(parent) = file_path.parent() {
                if !parent.exists() {
                    // Check if we can write to the parent directory
                    let parent_str = parent.to_string_lossy().to_string();
                    if !self.permission_guard.can_write(&parent_str) {
                        return Err(Error::Internal(format!(
                            "Cannot create directory in: {}",
                            parent_str
                        )));
                    }

                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        Error::Internal(format!("Failed to create directories: {}", e))
                    })?;
                }
            }
        }

        // Write the file
        tokio::fs::write(file_path, content_bytes)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write file: {}", e)))?;

        Ok(WriteResult {
            path: path.to_string(),
            bytes_written: content_bytes.len() as u64,
            created: !file_exists,
        })
    }

    /// Append content to a file
    pub async fn append(&self, path: &str, content: &str) -> Result<WriteResult> {
        // Check permission
        if !self.permission_guard.can_write(path) {
            return Err(Error::Internal(format!(
                "Write permission denied for path: {}",
                path
            )));
        }

        let file_path = Path::new(path);

        // Check if file exists
        if !file_path.exists() {
            return Err(Error::NotFound(format!("File not found: {}", path)));
        }

        // Get current file size
        let metadata = tokio::fs::metadata(file_path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to get file metadata: {}", e)))?;

        let content_bytes = content.as_bytes();
        let new_size = metadata.len() + content_bytes.len() as u64;

        // Check size limit
        if new_size > self.config.max_write_bytes {
            return Err(Error::Internal(format!(
                "Appended content would exceed maximum file size ({} bytes)",
                self.config.max_write_bytes
            )));
        }

        // Append to file
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(file_path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to open file for append: {}", e)))?;

        file.write_all(content_bytes)
            .await
            .map_err(|e| Error::Internal(format!("Failed to append to file: {}", e)))?;

        Ok(WriteResult {
            path: path.to_string(),
            bytes_written: content_bytes.len() as u64,
            created: false,
        })
    }

    /// Delete a file
    pub async fn delete(&self, path: &str) -> Result<()> {
        // Check permission
        if !self.permission_guard.can_write(path) {
            return Err(Error::Internal(format!(
                "Write permission denied for path: {}",
                path
            )));
        }

        let canonical = self.permission_guard.resolve_path(path)?;

        // Check if it's a file
        let metadata = tokio::fs::metadata(&canonical)
            .await
            .map_err(|e| Error::NotFound(format!("File not found: {} ({})", path, e)))?;

        if !metadata.is_file() {
            return Err(Error::Internal(format!("Path is not a file: {}", path)));
        }

        tokio::fs::remove_file(&canonical)
            .await
            .map_err(|e| Error::Internal(format!("Failed to delete file: {}", e)))?;

        Ok(())
    }

    /// Create a directory
    pub async fn create_directory(&self, path: &str) -> Result<()> {
        // Check permission
        if !self.permission_guard.can_write(path) {
            return Err(Error::Internal(format!(
                "Write permission denied for path: {}",
                path
            )));
        }

        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create directory: {}", e)))?;

        Ok(())
    }

    /// Rename/move a file
    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        // Check permissions for both paths
        if !self.permission_guard.can_write(from) {
            return Err(Error::Internal(format!(
                "Write permission denied for source path: {}",
                from
            )));
        }
        if !self.permission_guard.can_write(to) {
            return Err(Error::Internal(format!(
                "Write permission denied for destination path: {}",
                to
            )));
        }

        let from_canonical = self.permission_guard.resolve_path(from)?;

        tokio::fs::rename(&from_canonical, to)
            .await
            .map_err(|e| Error::Internal(format!("Failed to rename file: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_result_serialization() {
        let result = WriteResult {
            path: "/tmp/test.txt".to_string(),
            bytes_written: 1024,
            created: true,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"bytes_written\":1024"));
        assert!(json.contains("\"created\":true"));
    }
}
