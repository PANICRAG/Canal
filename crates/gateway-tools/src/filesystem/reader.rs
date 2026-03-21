//! File Reader
//!
//! Provides secure file reading capabilities with size limits and encoding detection.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::config::FilesystemConfig;
use super::permissions::PermissionGuard;

/// File content result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    /// File path
    pub path: String,
    /// File content (as string for text files)
    pub content: String,
    /// File size in bytes
    pub size: u64,
    /// Detected or specified encoding
    pub encoding: String,
    /// Whether the content was truncated
    pub truncated: bool,
}

/// File reader with permission checking
pub struct FileReader {
    permission_guard: Arc<PermissionGuard>,
    config: FilesystemConfig,
}

impl FileReader {
    /// Create a new file reader
    pub fn new(permission_guard: Arc<PermissionGuard>, config: FilesystemConfig) -> Self {
        Self {
            permission_guard,
            config,
        }
    }

    /// Read a file and return its content
    pub async fn read(&self, path: &str) -> Result<FileContent> {
        // Check permission
        if !self.permission_guard.can_read(path) {
            return Err(Error::NotFound(format!("File not accessible: {}", path)));
        }

        // Resolve and validate path
        let canonical = self.permission_guard.resolve_path(path)?;

        // Check file exists and is a file
        let metadata = tokio::fs::metadata(&canonical)
            .await
            .map_err(|e| Error::NotFound(format!("File not found: {} ({})", path, e)))?;

        if !metadata.is_file() {
            return Err(Error::Internal(format!("Path is not a file: {}", path)));
        }

        let file_size = metadata.len();

        // Check file size limit
        let mut truncated = false;
        let read_size = if file_size > self.config.max_read_bytes {
            truncated = true;
            self.config.max_read_bytes as usize
        } else {
            file_size as usize
        };

        // Read file content
        let bytes = if truncated {
            // Read only up to the limit
            use tokio::io::AsyncReadExt;
            let mut file = tokio::fs::File::open(&canonical)
                .await
                .map_err(|e| Error::Internal(format!("Failed to open file: {}", e)))?;
            let mut buffer = vec![0u8; read_size];
            file.read_exact(&mut buffer)
                .await
                .map_err(|e| Error::Internal(format!("Failed to read file: {}", e)))?;
            buffer
        } else {
            tokio::fs::read(&canonical)
                .await
                .map_err(|e| Error::Internal(format!("Failed to read file: {}", e)))?
        };

        // Detect encoding and convert to string
        let (content, encoding) = self.decode_content(&bytes)?;

        Ok(FileContent {
            path: path.to_string(),
            content,
            size: file_size,
            encoding,
            truncated,
        })
    }

    /// Read a file as a string (convenience method)
    pub async fn read_string(&self, path: &str) -> Result<String> {
        let content = self.read(path).await?;
        Ok(content.content)
    }

    /// Decode bytes to string with encoding detection
    fn decode_content(&self, bytes: &[u8]) -> Result<(String, String)> {
        // Try UTF-8 first
        if let Ok(content) = String::from_utf8(bytes.to_vec()) {
            return Ok((content, "utf-8".to_string()));
        }

        // Try to detect if it's ASCII
        if bytes.iter().all(|&b| b.is_ascii()) {
            let content = String::from_utf8_lossy(bytes).to_string();
            return Ok((content, "ascii".to_string()));
        }

        // For non-UTF-8, use lossy conversion
        let content = String::from_utf8_lossy(bytes).to_string();
        Ok((content, "utf-8-lossy".to_string()))
    }

    /// Check if a file exists and is readable
    pub async fn exists(&self, path: &str) -> bool {
        if !self.permission_guard.can_read(path) {
            return false;
        }

        match self.permission_guard.resolve_path(path) {
            Ok(canonical) => tokio::fs::metadata(&canonical)
                .await
                .map(|m| m.is_file())
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Get file metadata
    pub async fn metadata(&self, path: &str) -> Result<FileMetadata> {
        if !self.permission_guard.can_read(path) {
            return Err(Error::NotFound(format!("File not accessible: {}", path)));
        }

        let canonical = self.permission_guard.resolve_path(path)?;

        let metadata = tokio::fs::metadata(&canonical)
            .await
            .map_err(|e| Error::NotFound(format!("File not found: {} ({})", path, e)))?;

        Ok(FileMetadata {
            path: path.to_string(),
            size: metadata.len(),
            is_file: metadata.is_file(),
            is_directory: metadata.is_dir(),
            is_symlink: metadata.is_symlink(),
            modified: metadata
                .modified()
                .ok()
                .map(chrono::DateTime::<chrono::Utc>::from),
            created: metadata
                .created()
                .ok()
                .map(chrono::DateTime::<chrono::Utc>::from),
        })
    }
}

/// File metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub path: String,
    pub size: u64,
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub modified: Option<chrono::DateTime<chrono::Utc>>,
    pub created: Option<chrono::DateTime<chrono::Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_content_serialization() {
        let content = FileContent {
            path: "/tmp/test.txt".to_string(),
            content: "Hello, World!".to_string(),
            size: 13,
            encoding: "utf-8".to_string(),
            truncated: false,
        };

        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"path\":\"/tmp/test.txt\""));
        assert!(json.contains("\"content\":\"Hello, World!\""));
    }
}
