//! Filesystem Service Module
//!
//! Provides secure file system access capabilities with configurable directory permissions.
//! Supports file reading, writing, directory listing, and content searching (via ripgrep).
//!
//! # Security Features
//!
//! - Configurable directory whitelist with read/write permissions
//! - Path traversal attack prevention
//! - Sensitive file pattern blocking (.env, credentials, etc.)
//! - File size limits
//! - Symlink resolution with boundary checking

mod config;
mod permissions;
mod reader;
mod search;
mod writer;

pub use config::{DirectoryConfig, DirectoryMode, FilesystemConfig};
pub use permissions::PermissionGuard;
pub use reader::{FileContent, FileReader};
pub use search::{FileSearcher, SearchMatch, SearchResult};
pub use writer::{FileWriter, WriteResult};

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Directory entry information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryEntry {
    /// Entry name (file or directory name)
    pub name: String,
    /// Full path to the entry
    pub path: String,
    /// Entry type
    pub entry_type: EntryType,
    /// File size in bytes (None for directories)
    pub size: Option<u64>,
    /// Last modified timestamp
    pub modified: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether the entry is hidden (starts with .)
    pub hidden: bool,
}

/// Type of directory entry
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
    Symlink,
    Unknown,
}

impl std::fmt::Display for EntryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryType::File => write!(f, "file"),
            EntryType::Directory => write!(f, "directory"),
            EntryType::Symlink => write!(f, "symlink"),
            EntryType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Directory listing result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryListing {
    /// Directory path
    pub path: String,
    /// Entries in the directory
    pub entries: Vec<DirectoryEntry>,
    /// Total entry count
    pub total_count: usize,
}

/// Main filesystem service
pub struct FilesystemService {
    config: FilesystemConfig,
    permission_guard: Arc<PermissionGuard>,
    reader: Arc<FileReader>,
    writer: Arc<FileWriter>,
    searcher: Arc<FileSearcher>,
}

impl FilesystemService {
    /// Create a new filesystem service with the given configuration
    pub fn new(config: FilesystemConfig) -> Self {
        let permission_guard = Arc::new(PermissionGuard::new(config.clone()));

        Self {
            reader: Arc::new(FileReader::new(permission_guard.clone(), config.clone())),
            writer: Arc::new(FileWriter::new(permission_guard.clone(), config.clone())),
            searcher: Arc::new(FileSearcher::new(permission_guard.clone())),
            permission_guard,
            config,
        }
    }

    /// Check if a path is readable
    pub fn can_read(&self, path: &str) -> bool {
        self.permission_guard.can_read(path)
    }

    /// Check if a path is writable
    pub fn can_write(&self, path: &str) -> bool {
        self.permission_guard.can_write(path)
    }

    /// Read file content
    pub async fn read_file(&self, path: &str) -> Result<FileContent> {
        self.reader.read(path).await
    }

    /// Read file as string
    pub async fn read_file_string(&self, path: &str) -> Result<String> {
        self.reader.read_string(path).await
    }

    /// Write file content
    pub async fn write_file(
        &self,
        path: &str,
        content: &str,
        create_dirs: bool,
        overwrite: bool,
    ) -> Result<WriteResult> {
        self.writer
            .write(path, content, create_dirs, overwrite)
            .await
    }

    /// List directory contents
    pub async fn list_directory(
        &self,
        path: &str,
        recursive: bool,
        include_hidden: bool,
    ) -> Result<DirectoryListing> {
        if !self.permission_guard.can_read(path) {
            return Err(Error::NotFound(format!("Path not accessible: {}", path)));
        }

        let canonical = self.permission_guard.resolve_path(path)?;

        let mut entries = Vec::new();
        self.list_directory_internal(&canonical, recursive, include_hidden, &mut entries)
            .await?;

        Ok(DirectoryListing {
            path: path.to_string(),
            entries: entries.clone(),
            total_count: entries.len(),
        })
    }

    /// Internal directory listing implementation
    #[async_recursion::async_recursion]
    async fn list_directory_internal(
        &self,
        path: &PathBuf,
        recursive: bool,
        include_hidden: bool,
        entries: &mut Vec<DirectoryEntry>,
    ) -> Result<()> {
        let mut read_dir = tokio::fs::read_dir(path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read directory: {}", e)))?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| Error::Internal(format!("Failed to read directory entry: {}", e)))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_hidden = name.starts_with('.');

            if is_hidden && !include_hidden {
                continue;
            }

            let path_str = entry.path().to_string_lossy().to_string();

            let metadata = entry.metadata().await.ok();
            let file_type = entry.file_type().await.ok();

            let entry_type = match file_type {
                Some(ft) if ft.is_file() => EntryType::File,
                Some(ft) if ft.is_dir() => EntryType::Directory,
                Some(ft) if ft.is_symlink() => EntryType::Symlink,
                _ => EntryType::Unknown,
            };

            let size = metadata
                .as_ref()
                .and_then(|m| if m.is_file() { Some(m.len()) } else { None });

            let modified = metadata.and_then(|m| {
                m.modified()
                    .ok()
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t))
            });

            entries.push(DirectoryEntry {
                name,
                path: path_str.clone(),
                entry_type,
                size,
                modified,
                hidden: is_hidden,
            });

            if recursive && entry_type == EntryType::Directory {
                let sub_path = entry.path();
                if self.permission_guard.can_read(&path_str) {
                    let _ = self
                        .list_directory_internal(&sub_path, recursive, include_hidden, entries)
                        .await;
                }
            }
        }

        Ok(())
    }

    /// Search for content in files
    pub async fn search(
        &self,
        path: &str,
        pattern: &str,
        file_pattern: Option<&str>,
        max_results: usize,
    ) -> Result<SearchResult> {
        self.searcher
            .search(path, pattern, file_pattern, max_results)
            .await
    }

    /// Get the allowed directories
    pub fn allowed_directories(&self) -> &[DirectoryConfig] {
        &self.config.allowed_directories
    }

    /// Get allowed directory paths as strings
    pub fn get_allowed_directories(&self) -> Vec<String> {
        self.config
            .allowed_directories
            .iter()
            .map(|d| d.path.clone())
            .collect()
    }

    /// Check if filesystem service is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_type_display() {
        assert_eq!(EntryType::File.to_string(), "file");
        assert_eq!(EntryType::Directory.to_string(), "directory");
        assert_eq!(EntryType::Symlink.to_string(), "symlink");
    }

    #[test]
    fn test_directory_entry_serialization() {
        let entry = DirectoryEntry {
            name: "test.txt".to_string(),
            path: "/data/test.txt".to_string(),
            entry_type: EntryType::File,
            size: Some(1024),
            modified: None,
            hidden: false,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"name\":\"test.txt\""));
        assert!(json.contains("\"entry_type\":\"file\""));
    }
}
