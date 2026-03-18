//! File operations module

use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs;
use tracing::info;
use walkdir::WalkDir;

/// File operation errors
#[derive(Error, Debug)]
pub enum FileError {
    #[error("File not found: {0}")]
    NotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Path outside workspace: {0}")]
    PathOutsideWorkspace(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// File content result
#[derive(Debug, Clone)]
pub struct FileContentResult {
    pub path: String,
    pub content: Vec<u8>,
    pub encoding: String,
    pub size: i64,
    pub mime_type: String,
    pub is_binary: bool,
    pub truncated: bool,
}

/// Write result
#[derive(Debug, Clone)]
pub struct WriteResult {
    pub path: String,
    pub bytes_written: i64,
    pub created: bool,
}

/// Delete result
#[derive(Debug, Clone)]
pub struct DeleteResult {
    pub path: String,
    pub deleted: bool,
}

/// File entry
#[derive(Debug, Clone)]
pub struct FileEntryResult {
    pub name: String,
    pub path: String,
    pub entry_type: EntryTypeResult,
    pub size: i64,
    pub modified_at: i64,
}

/// Entry type
#[derive(Debug, Clone, Copy)]
pub enum EntryTypeResult {
    File,
    Directory,
    Symlink,
}

/// Directory listing
#[derive(Debug, Clone)]
pub struct DirectoryListingResult {
    pub path: String,
    pub entries: Vec<FileEntryResult>,
    pub total_count: i32,
    pub truncated: bool,
}

/// Search match
#[derive(Debug, Clone)]
pub struct SearchMatchResult {
    pub file: String,
    pub line_number: i32,
    pub line_content: String,
}

/// Search results
#[derive(Debug, Clone)]
pub struct SearchResultsResult {
    pub matches: Vec<SearchMatchResult>,
    pub total_matches: i32,
    pub files_searched: i32,
    pub truncated: bool,
}

/// File operations handler
#[derive(Clone)]
pub struct FileOperations {
    workspace_dir: PathBuf,
}

impl FileOperations {
    /// Create a new file operations handler
    pub fn new(workspace_dir: &str) -> Self {
        Self {
            workspace_dir: PathBuf::from(workspace_dir),
        }
    }

    /// Validate that a path is within the workspace
    fn validate_path(&self, path: &str) -> Result<PathBuf, FileError> {
        let full_path = if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            self.workspace_dir.join(path)
        };

        // Canonicalize to resolve any .. or symlinks
        // R8-H10: Do not fall back to uncanonicalized path — canonicalize parent instead
        let canonical = match full_path.canonicalize() {
            Ok(c) => c,
            Err(_) => {
                // File may not exist yet (write ops) — canonicalize parent + append filename
                if let (Some(parent), Some(file_name)) = (full_path.parent(), full_path.file_name())
                {
                    let canon_parent = parent
                        .canonicalize()
                        .map_err(|_| FileError::PathOutsideWorkspace(path.to_string()))?;
                    canon_parent.join(file_name)
                } else {
                    return Err(FileError::PathOutsideWorkspace(path.to_string()));
                }
            }
        };

        // Check if path is within workspace
        if !canonical.starts_with(&self.workspace_dir) {
            return Err(FileError::PathOutsideWorkspace(path.to_string()));
        }

        Ok(canonical)
    }

    /// Read a file
    pub async fn read_file(
        &self,
        path: &str,
        offset: i64,
        limit: i64,
    ) -> Result<FileContentResult, FileError> {
        let full_path = self.validate_path(path)?;

        info!(path = %full_path.display(), "Reading file");

        // R8-H9: Check if target is a symlink — reject to prevent symlink escape
        match fs::symlink_metadata(&full_path).await {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(FileError::PermissionDenied(
                    "Cannot read symlink target — potential path escape".to_string(),
                ));
            }
            Err(_) => {
                return Err(FileError::NotFound(path.to_string()));
            }
            _ => {}
        }

        // Read file content
        let content = fs::read(&full_path).await?;

        // Get metadata
        let metadata = fs::metadata(&full_path).await?;
        let size = metadata.len() as i64;

        // Apply offset and limit
        let start = offset as usize;
        let end = if limit > 0 {
            std::cmp::min(start + limit as usize, content.len())
        } else {
            content.len()
        };

        let sliced_content = if start < content.len() {
            content[start..end].to_vec()
        } else {
            Vec::new()
        };

        // Detect if binary
        let is_binary = sliced_content.iter().any(|&b| b == 0);

        // Detect MIME type (simplified)
        let mime_type = Self::detect_mime_type(path);

        Ok(FileContentResult {
            path: path.to_string(),
            content: sliced_content,
            encoding: if is_binary {
                "binary".to_string()
            } else {
                "utf8".to_string()
            },
            size,
            mime_type,
            is_binary,
            truncated: end < content.len(),
        })
    }

    /// Write a file
    pub async fn write_file(
        &self,
        path: &str,
        content: &[u8],
        create_dirs: bool,
        overwrite: bool,
    ) -> Result<WriteResult, FileError> {
        // For new files, we can't canonicalize yet, so just join with workspace
        let full_path = if path.starts_with('/') {
            PathBuf::from(path)
        } else {
            self.workspace_dir.join(path)
        };

        // Check if path is within workspace (without canonicalize for new files)
        let normalized = full_path
            .components()
            .fold(PathBuf::new(), |mut acc, comp| {
                match comp {
                    std::path::Component::ParentDir => {
                        acc.pop();
                    }
                    std::path::Component::Normal(p) => acc.push(p),
                    std::path::Component::RootDir => acc.push("/"),
                    _ => {}
                }
                acc
            });

        if !normalized.starts_with(&self.workspace_dir) {
            return Err(FileError::PathOutsideWorkspace(path.to_string()));
        }

        info!(path = %full_path.display(), size = content.len(), "Writing file");

        // R8-H9: Check if target is a symlink — reject to prevent symlink escape
        let exists = match fs::symlink_metadata(&full_path).await {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(FileError::PermissionDenied(
                        "Cannot write to symlink target — potential path escape".to_string(),
                    ));
                }
                if !overwrite {
                    return Err(FileError::PermissionDenied(
                        "File exists and overwrite is false".to_string(),
                    ));
                }
                true
            }
            Err(_) => false,
        };

        // Create parent directories if needed
        if create_dirs {
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).await?;
            }
        }

        // Write the file
        fs::write(&full_path, content).await?;

        Ok(WriteResult {
            path: path.to_string(),
            bytes_written: content.len() as i64,
            created: !exists,
        })
    }

    /// Delete a file or directory
    pub async fn delete_file(
        &self,
        path: &str,
        recursive: bool,
    ) -> Result<DeleteResult, FileError> {
        let full_path = self.validate_path(path)?;

        info!(path = %full_path.display(), recursive = recursive, "Deleting file");

        if !full_path.exists() {
            return Ok(DeleteResult {
                path: path.to_string(),
                deleted: false,
            });
        }

        if full_path.is_dir() {
            if recursive {
                fs::remove_dir_all(&full_path).await?;
            } else {
                fs::remove_dir(&full_path).await?;
            }
        } else {
            fs::remove_file(&full_path).await?;
        }

        Ok(DeleteResult {
            path: path.to_string(),
            deleted: true,
        })
    }

    /// List directory contents
    pub async fn list_directory(
        &self,
        path: &str,
        recursive: bool,
        max_depth: i32,
    ) -> Result<DirectoryListingResult, FileError> {
        let full_path = self.validate_path(path)?;

        info!(path = %full_path.display(), recursive = recursive, "Listing directory");

        if !full_path.exists() {
            return Err(FileError::NotFound(path.to_string()));
        }

        if !full_path.is_dir() {
            return Err(FileError::NotFound(format!("{} is not a directory", path)));
        }

        let mut entries = Vec::new();
        let depth = if recursive {
            if max_depth > 0 {
                max_depth as usize
            } else {
                usize::MAX
            }
        } else {
            1
        };

        for entry in WalkDir::new(&full_path)
            .max_depth(depth)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            // Skip the root directory itself
            if entry.path() == full_path {
                continue;
            }

            let metadata = entry.metadata().ok();
            let file_type = if entry.file_type().is_dir() {
                EntryTypeResult::Directory
            } else if entry.file_type().is_symlink() {
                EntryTypeResult::Symlink
            } else {
                EntryTypeResult::File
            };

            entries.push(FileEntryResult {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry
                    .path()
                    .strip_prefix(&self.workspace_dir)
                    .unwrap_or(entry.path())
                    .to_string_lossy()
                    .to_string(),
                entry_type: file_type,
                size: metadata.as_ref().map(|m| m.len() as i64).unwrap_or(0),
                modified_at: metadata
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            });
        }

        let total_count = entries.len() as i32;

        Ok(DirectoryListingResult {
            path: path.to_string(),
            entries,
            total_count,
            truncated: false,
        })
    }

    /// Search files for a pattern
    pub async fn search_files(
        &self,
        path: &str,
        pattern: &str,
        is_regex: bool,
        max_results: i32,
    ) -> Result<SearchResultsResult, FileError> {
        let full_path = self.validate_path(path)?;

        info!(
            path = %full_path.display(),
            pattern = %pattern,
            is_regex = is_regex,
            "Searching files"
        );

        let mut matches = Vec::new();
        let mut files_searched = 0;
        let max = if max_results > 0 {
            max_results as usize
        } else {
            1000
        };

        // R8-M114: Compile regex once before the loop instead of per-line
        let compiled_regex = if is_regex {
            match regex::Regex::new(pattern) {
                Ok(re) => Some(re),
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Invalid regex pattern: {}", e),
                    )
                    .into())
                }
            }
        } else {
            None
        };

        for entry in WalkDir::new(&full_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            files_searched += 1;

            // Read file content
            if let Ok(content) = fs::read_to_string(entry.path()).await {
                for (line_num, line) in content.lines().enumerate() {
                    let is_match = if let Some(ref re) = compiled_regex {
                        re.is_match(line)
                    } else {
                        line.contains(pattern)
                    };

                    if is_match {
                        matches.push(SearchMatchResult {
                            file: entry
                                .path()
                                .strip_prefix(&self.workspace_dir)
                                .unwrap_or(entry.path())
                                .to_string_lossy()
                                .to_string(),
                            line_number: (line_num + 1) as i32,
                            line_content: line.to_string(),
                        });

                        if matches.len() >= max {
                            return Ok(SearchResultsResult {
                                matches,
                                total_matches: max as i32,
                                files_searched,
                                truncated: true,
                            });
                        }
                    }
                }
            }
        }

        let total = matches.len() as i32;
        Ok(SearchResultsResult {
            matches,
            total_matches: total,
            files_searched,
            truncated: false,
        })
    }

    /// Detect MIME type from file extension
    fn detect_mime_type(path: &str) -> String {
        let extension = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        match extension.to_lowercase().as_str() {
            "txt" => "text/plain",
            "html" | "htm" => "text/html",
            "css" => "text/css",
            "js" => "application/javascript",
            "json" => "application/json",
            "xml" => "application/xml",
            "py" => "text/x-python",
            "rs" => "text/x-rust",
            "go" => "text/x-go",
            "md" => "text/markdown",
            "yaml" | "yml" => "text/yaml",
            "toml" => "text/toml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "pdf" => "application/pdf",
            _ => "application/octet-stream",
        }
        .to_string()
    }
}
