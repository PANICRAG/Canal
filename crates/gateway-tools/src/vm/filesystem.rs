//! VM Filesystem Operations
//!
//! Provides file operations for VMs running with rootfs mounted.
//! Communicates with the VM API server over HTTP to perform file I/O,
//! directory operations, and workspace management.
//!
//! # Architecture
//!
//! ```text
//! +------------------+          HTTP/REST           +------------------+
//! |  VmFileSystem    |  ------------------------->  |   VM API Server  |
//! | (Rust Client)    |                              |   (Python)       |
//! +------------------+                              +------------------+
//!         |                                                 |
//!         | read/write/list/etc                             | fs operations
//!         v                                                 v
//!    Gateway Core                                     VM Rootfs
//! ```
//!
//! # Example
//!
//! ```ignore
//! use gateway_core::vm::VmFileSystem;
//!
//! let fs = VmFileSystem::new("http://172.16.0.2:8080", "/workspace");
//!
//! // Write a file
//! fs.write_file("/workspace/code.py", b"print('hello')").await?;
//!
//! // Read it back
//! let content = fs.read_file("/workspace/code.py").await?;
//!
//! // List directory
//! let entries = fs.list_dir("/workspace").await?;
//!
//! // Create workspace
//! let ws = fs.create_workspace("project-1").await?;
//! ```

use crate::error::{ServiceError as Error, ServiceResult as Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, instrument};

/// Default timeout for file operations in milliseconds
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Default workspace root directory
const DEFAULT_WORKSPACE_ROOT: &str = "/workspace";

/// File information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    /// Full path to the file
    pub path: String,
    /// File size in bytes
    pub size: u64,
    /// Last modified time
    pub modified: DateTime<Utc>,
    /// Whether this is a directory
    pub is_dir: bool,
    /// Unix file permissions
    pub permissions: u32,
}

/// Directory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    /// Entry name (without path)
    pub name: String,
    /// Full path to the entry
    pub path: String,
    /// Whether this is a directory
    pub is_dir: bool,
    /// Size in bytes (0 for directories)
    pub size: u64,
}

/// Workspace information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    /// Unique workspace identifier
    pub id: String,
    /// Full path to workspace directory
    pub path: String,
    /// When the workspace was created
    pub created_at: DateTime<Utc>,
}

/// Configuration for VmFileSystem
#[derive(Debug, Clone)]
pub struct VmFileSystemConfig {
    /// Base URL for the VM API server
    pub vm_url: String,
    /// Root directory for workspaces
    pub workspace_root: PathBuf,
    /// Timeout for file operations in milliseconds
    pub timeout_ms: u64,
    /// Maximum file size for uploads (in bytes)
    pub max_upload_size: u64,
}

impl Default for VmFileSystemConfig {
    fn default() -> Self {
        Self {
            vm_url: "http://localhost:8080".to_string(),
            workspace_root: PathBuf::from(DEFAULT_WORKSPACE_ROOT),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_upload_size: 100 * 1024 * 1024, // 100MB
        }
    }
}

/// VM Filesystem client for file operations
///
/// This client communicates with the VM API server to perform
/// file operations on the VM's rootfs.
pub struct VmFileSystem {
    vm_url: String,
    client: reqwest::Client,
    workspace_root: PathBuf,
    config: VmFileSystemConfig,
}

impl VmFileSystem {
    /// Create a new VmFileSystem client
    pub fn new(vm_url: impl Into<String>, workspace_root: impl Into<PathBuf>) -> Self {
        let vm_url = vm_url.into();
        let workspace_root = workspace_root.into();

        let config = VmFileSystemConfig {
            vm_url: vm_url.clone(),
            workspace_root: workspace_root.clone(),
            ..Default::default()
        };

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            vm_url,
            client,
            workspace_root,
            config,
        }
    }

    /// Create a VmFileSystem with custom configuration
    pub fn with_config(config: VmFileSystemConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            vm_url: config.vm_url.clone(),
            workspace_root: config.workspace_root.clone(),
            client,
            config,
        }
    }

    /// Get the VM URL
    pub fn vm_url(&self) -> &str {
        &self.vm_url
    }

    /// Get the workspace root
    pub fn workspace_root(&self) -> &PathBuf {
        &self.workspace_root
    }

    // ==========================================
    // File Operations
    // ==========================================

    /// Read a file from the VM
    ///
    /// # Arguments
    /// * `path` - Path to the file in the VM filesystem
    ///
    /// # Returns
    /// File contents as bytes
    #[instrument(skip(self))]
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        debug!(path = path, "Reading file from VM");

        #[derive(Serialize)]
        struct ReadRequest<'a> {
            path: &'a str,
            encoding: &'a str,
        }

        #[derive(Deserialize)]
        struct ReadResponse {
            success: bool,
            data: Option<ReadData>,
            error: Option<String>,
        }

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct ReadData {
            content: String,
            encoding: String,
            size: u64,
        }

        let url = format!("{}/api/files/read", self.vm_url);
        let response = self
            .client
            .post(&url)
            .json(&ReadRequest {
                path,
                encoding: "base64",
            })
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 404 {
                return Err(Error::NotFound(format!("File not found: {}", path)));
            }
            if status.as_u16() == 403 {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }

            return Err(Error::Internal(format!(
                "Failed to read file: {} - {}",
                status, body
            )));
        }

        let read_response: ReadResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse read response: {}", e)))?;

        if !read_response.success {
            let error = read_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            if error.contains("not found") || error.contains("No such file") {
                return Err(Error::NotFound(format!("File not found: {}", path)));
            }
            if error.contains("permission") || error.contains("Permission denied") {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }
            return Err(Error::Internal(format!("Failed to read file: {}", error)));
        }

        let data = read_response
            .data
            .ok_or_else(|| Error::Internal("No data in read response".to_string()))?;

        // Decode base64 content
        let content =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &data.content)
                .map_err(|e| Error::Internal(format!("Failed to decode file content: {}", e)))?;

        debug!(path = path, size = content.len(), "File read successfully");
        Ok(content)
    }

    /// Write a file to the VM
    ///
    /// # Arguments
    /// * `path` - Path to the file in the VM filesystem
    /// * `content` - File contents as bytes
    #[instrument(skip(self, content))]
    pub async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        debug!(path = path, size = content.len(), "Writing file to VM");

        // Check size limit
        if content.len() as u64 > self.config.max_upload_size {
            return Err(Error::FileTooLarge {
                size: content.len() as u64,
                limit: self.config.max_upload_size,
            });
        }

        #[derive(Serialize)]
        struct WriteRequest<'a> {
            path: &'a str,
            content: String,
            encoding: &'a str,
            create_dirs: bool,
        }

        #[derive(Deserialize)]
        struct WriteResponse {
            success: bool,
            error: Option<String>,
        }

        // Encode content as base64
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, content);

        let url = format!("{}/api/files/write", self.vm_url);
        let response = self
            .client
            .post(&url)
            .json(&WriteRequest {
                path,
                content: encoded,
                encoding: "base64",
                create_dirs: true,
            })
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 403 {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }

            return Err(Error::Internal(format!(
                "Failed to write file: {} - {}",
                status, body
            )));
        }

        let write_response: WriteResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse write response: {}", e)))?;

        if !write_response.success {
            let error = write_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            if error.contains("permission") || error.contains("Permission denied") {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }
            return Err(Error::Internal(format!("Failed to write file: {}", error)));
        }

        debug!(path = path, "File written successfully");
        Ok(())
    }

    /// Delete a file from the VM
    ///
    /// # Arguments
    /// * `path` - Path to the file to delete
    #[instrument(skip(self))]
    pub async fn delete_file(&self, path: &str) -> Result<()> {
        debug!(path = path, "Deleting file from VM");

        #[derive(Serialize)]
        struct DeleteRequest<'a> {
            path: &'a str,
        }

        #[derive(Deserialize)]
        struct DeleteResponse {
            success: bool,
            error: Option<String>,
        }

        let url = format!("{}/api/files/delete", self.vm_url);
        let response = self
            .client
            .post(&url)
            .json(&DeleteRequest { path })
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 404 {
                return Err(Error::NotFound(format!("File not found: {}", path)));
            }
            if status.as_u16() == 403 {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }

            return Err(Error::Internal(format!(
                "Failed to delete file: {} - {}",
                status, body
            )));
        }

        let delete_response: DeleteResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse delete response: {}", e)))?;

        if !delete_response.success {
            let error = delete_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            if error.contains("not found") || error.contains("No such file") {
                return Err(Error::NotFound(format!("File not found: {}", path)));
            }
            if error.contains("permission") || error.contains("Permission denied") {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }
            return Err(Error::Internal(format!("Failed to delete file: {}", error)));
        }

        debug!(path = path, "File deleted successfully");
        Ok(())
    }

    /// Check if a file exists in the VM
    ///
    /// # Arguments
    /// * `path` - Path to check
    ///
    /// # Returns
    /// `true` if the file exists, `false` otherwise
    #[instrument(skip(self))]
    pub async fn file_exists(&self, path: &str) -> Result<bool> {
        debug!(path = path, "Checking if file exists");

        match self.file_info(path).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Get information about a file
    ///
    /// # Arguments
    /// * `path` - Path to the file
    ///
    /// # Returns
    /// FileInfo containing metadata about the file
    #[instrument(skip(self))]
    pub async fn file_info(&self, path: &str) -> Result<FileInfo> {
        debug!(path = path, "Getting file info");

        #[derive(Serialize)]
        struct InfoRequest<'a> {
            path: &'a str,
        }

        #[derive(Deserialize)]
        struct InfoResponse {
            success: bool,
            data: Option<InfoData>,
            error: Option<String>,
        }

        #[derive(Deserialize)]
        struct InfoData {
            path: String,
            size: u64,
            modified: String,
            is_dir: bool,
            permissions: u32,
        }

        let url = format!("{}/api/files/info", self.vm_url);
        let response = self
            .client
            .post(&url)
            .json(&InfoRequest { path })
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 404 {
                return Err(Error::NotFound(format!("File not found: {}", path)));
            }

            return Err(Error::Internal(format!(
                "Failed to get file info: {} - {}",
                status, body
            )));
        }

        let info_response: InfoResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse info response: {}", e)))?;

        if !info_response.success {
            let error = info_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            if error.contains("not found") || error.contains("No such file") {
                return Err(Error::NotFound(format!("File not found: {}", path)));
            }
            return Err(Error::Internal(format!(
                "Failed to get file info: {}",
                error
            )));
        }

        let data = info_response
            .data
            .ok_or_else(|| Error::Internal("No data in info response".to_string()))?;

        let modified = DateTime::parse_from_rfc3339(&data.modified)
            .map_err(|e| Error::Internal(format!("Failed to parse modified time: {}", e)))?
            .with_timezone(&Utc);

        Ok(FileInfo {
            path: data.path,
            size: data.size,
            modified,
            is_dir: data.is_dir,
            permissions: data.permissions,
        })
    }

    // ==========================================
    // Directory Operations
    // ==========================================

    /// List contents of a directory
    ///
    /// # Arguments
    /// * `path` - Path to the directory
    ///
    /// # Returns
    /// Vector of directory entries
    #[instrument(skip(self))]
    pub async fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        debug!(path = path, "Listing directory");

        #[derive(Serialize)]
        struct ListRequest<'a> {
            path: &'a str,
        }

        #[derive(Deserialize)]
        struct ListResponse {
            success: bool,
            data: Option<ListData>,
            error: Option<String>,
        }

        #[derive(Deserialize)]
        struct ListData {
            entries: Vec<EntryData>,
        }

        #[derive(Deserialize)]
        struct EntryData {
            name: String,
            path: String,
            is_dir: bool,
            size: u64,
        }

        let url = format!("{}/api/files/list", self.vm_url);
        let response = self
            .client
            .post(&url)
            .json(&ListRequest { path })
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 404 {
                return Err(Error::NotFound(format!("Directory not found: {}", path)));
            }

            return Err(Error::Internal(format!(
                "Failed to list directory: {} - {}",
                status, body
            )));
        }

        let list_response: ListResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse list response: {}", e)))?;

        if !list_response.success {
            let error = list_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            if error.contains("not found") || error.contains("No such file") {
                return Err(Error::NotFound(format!("Directory not found: {}", path)));
            }
            return Err(Error::Internal(format!(
                "Failed to list directory: {}",
                error
            )));
        }

        let data = list_response
            .data
            .ok_or_else(|| Error::Internal("No data in list response".to_string()))?;

        let count = data.entries.len();
        let entries = data
            .entries
            .into_iter()
            .map(|e| DirEntry {
                name: e.name,
                path: e.path,
                is_dir: e.is_dir,
                size: e.size,
            })
            .collect();

        debug!(path = path, count = count, "Directory listed");
        Ok(entries)
    }

    /// Create a directory
    ///
    /// # Arguments
    /// * `path` - Path to the directory to create
    #[instrument(skip(self))]
    pub async fn create_dir(&self, path: &str) -> Result<()> {
        debug!(path = path, "Creating directory");

        #[derive(Serialize)]
        struct MkdirRequest<'a> {
            path: &'a str,
            recursive: bool,
        }

        #[derive(Deserialize)]
        struct MkdirResponse {
            success: bool,
            error: Option<String>,
        }

        let url = format!("{}/api/files/mkdir", self.vm_url);
        let response = self
            .client
            .post(&url)
            .json(&MkdirRequest {
                path,
                recursive: true,
            })
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 403 {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }

            return Err(Error::Internal(format!(
                "Failed to create directory: {} - {}",
                status, body
            )));
        }

        let mkdir_response: MkdirResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse mkdir response: {}", e)))?;

        if !mkdir_response.success {
            let error = mkdir_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            if error.contains("permission") || error.contains("Permission denied") {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }
            return Err(Error::Internal(format!(
                "Failed to create directory: {}",
                error
            )));
        }

        debug!(path = path, "Directory created");
        Ok(())
    }

    /// Remove a directory
    ///
    /// # Arguments
    /// * `path` - Path to the directory to remove
    /// * `recursive` - If true, remove directory and all contents
    #[instrument(skip(self))]
    pub async fn remove_dir(&self, path: &str, recursive: bool) -> Result<()> {
        debug!(path = path, recursive = recursive, "Removing directory");

        #[derive(Serialize)]
        struct RmdirRequest<'a> {
            path: &'a str,
            recursive: bool,
        }

        #[derive(Deserialize)]
        struct RmdirResponse {
            success: bool,
            error: Option<String>,
        }

        let url = format!("{}/api/files/rmdir", self.vm_url);
        let response = self
            .client
            .post(&url)
            .json(&RmdirRequest { path, recursive })
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 404 {
                return Err(Error::NotFound(format!("Directory not found: {}", path)));
            }
            if status.as_u16() == 403 {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }

            return Err(Error::Internal(format!(
                "Failed to remove directory: {} - {}",
                status, body
            )));
        }

        let rmdir_response: RmdirResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse rmdir response: {}", e)))?;

        if !rmdir_response.success {
            let error = rmdir_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            if error.contains("not found") || error.contains("No such file") {
                return Err(Error::NotFound(format!("Directory not found: {}", path)));
            }
            if error.contains("not empty") {
                return Err(Error::InvalidInput(format!(
                    "Directory not empty: {}. Use recursive=true to remove.",
                    path
                )));
            }
            if error.contains("permission") || error.contains("Permission denied") {
                return Err(Error::PermissionDenied(format!(
                    "Permission denied: {}",
                    path
                )));
            }
            return Err(Error::Internal(format!(
                "Failed to remove directory: {}",
                error
            )));
        }

        debug!(path = path, "Directory removed");
        Ok(())
    }

    // ==========================================
    // Workspace Operations
    // ==========================================

    /// Create a new workspace
    ///
    /// A workspace is an isolated directory for a task or session.
    ///
    /// # Arguments
    /// * `name` - Name for the workspace (used to generate ID)
    ///
    /// # Returns
    /// Workspace information including the assigned path
    #[instrument(skip(self))]
    pub async fn create_workspace(&self, name: &str) -> Result<Workspace> {
        debug!(name = name, "Creating workspace");

        #[derive(Serialize)]
        struct CreateWorkspaceRequest<'a> {
            name: &'a str,
            root: &'a str,
        }

        #[derive(Deserialize)]
        struct CreateWorkspaceResponse {
            success: bool,
            data: Option<WorkspaceData>,
            error: Option<String>,
        }

        #[derive(Deserialize)]
        struct WorkspaceData {
            id: String,
            path: String,
            created_at: String,
        }

        let url = format!("{}/api/workspace/create", self.vm_url);
        let response = self
            .client
            .post(&url)
            .json(&CreateWorkspaceRequest {
                name,
                root: self
                    .workspace_root
                    .to_str()
                    .unwrap_or(DEFAULT_WORKSPACE_ROOT),
            })
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            return Err(Error::Internal(format!(
                "Failed to create workspace: {} - {}",
                status, body
            )));
        }

        let create_response: CreateWorkspaceResponse = response.json().await.map_err(|e| {
            Error::Internal(format!("Failed to parse create workspace response: {}", e))
        })?;

        if !create_response.success {
            let error = create_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            return Err(Error::Internal(format!(
                "Failed to create workspace: {}",
                error
            )));
        }

        let data = create_response
            .data
            .ok_or_else(|| Error::Internal("No data in create workspace response".to_string()))?;

        let created_at = DateTime::parse_from_rfc3339(&data.created_at)
            .map_err(|e| Error::Internal(format!("Failed to parse created_at: {}", e)))?
            .with_timezone(&Utc);

        let workspace = Workspace {
            id: data.id,
            path: data.path,
            created_at,
        };

        debug!(
            workspace_id = %workspace.id,
            workspace_path = %workspace.path,
            "Workspace created"
        );

        Ok(workspace)
    }

    /// Cleanup a workspace
    ///
    /// Removes the workspace directory and all its contents.
    ///
    /// # Arguments
    /// * `workspace_id` - ID of the workspace to cleanup
    #[instrument(skip(self))]
    pub async fn cleanup_workspace(&self, workspace_id: &str) -> Result<()> {
        debug!(workspace_id = workspace_id, "Cleaning up workspace");

        #[derive(Serialize)]
        struct CleanupWorkspaceRequest<'a> {
            workspace_id: &'a str,
            root: &'a str,
        }

        #[derive(Deserialize)]
        struct CleanupWorkspaceResponse {
            success: bool,
            error: Option<String>,
        }

        let url = format!("{}/api/workspace/cleanup", self.vm_url);
        let response = self
            .client
            .post(&url)
            .json(&CleanupWorkspaceRequest {
                workspace_id,
                root: self
                    .workspace_root
                    .to_str()
                    .unwrap_or(DEFAULT_WORKSPACE_ROOT),
            })
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();

            if status.as_u16() == 404 {
                return Err(Error::NotFound(format!(
                    "Workspace not found: {}",
                    workspace_id
                )));
            }

            return Err(Error::Internal(format!(
                "Failed to cleanup workspace: {} - {}",
                status, body
            )));
        }

        let cleanup_response: CleanupWorkspaceResponse = response.json().await.map_err(|e| {
            Error::Internal(format!("Failed to parse cleanup workspace response: {}", e))
        })?;

        if !cleanup_response.success {
            let error = cleanup_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            if error.contains("not found") {
                return Err(Error::NotFound(format!(
                    "Workspace not found: {}",
                    workspace_id
                )));
            }
            return Err(Error::Internal(format!(
                "Failed to cleanup workspace: {}",
                error
            )));
        }

        debug!(workspace_id = workspace_id, "Workspace cleaned up");
        Ok(())
    }

    /// Get workspace path for a workspace ID
    pub fn workspace_path(&self, workspace_id: &str) -> PathBuf {
        self.workspace_root.join(workspace_id)
    }

    // ==========================================
    // Helper Methods
    // ==========================================

    /// Copy a file within the VM
    ///
    /// # Arguments
    /// * `src` - Source file path
    /// * `dst` - Destination file path
    #[instrument(skip(self))]
    pub async fn copy_file(&self, src: &str, dst: &str) -> Result<()> {
        debug!(src = src, dst = dst, "Copying file");

        // Read source file
        let content = self.read_file(src).await?;

        // Write to destination
        self.write_file(dst, &content).await?;

        debug!(src = src, dst = dst, "File copied");
        Ok(())
    }

    /// Move/rename a file within the VM
    ///
    /// # Arguments
    /// * `src` - Source file path
    /// * `dst` - Destination file path
    #[instrument(skip(self))]
    pub async fn move_file(&self, src: &str, dst: &str) -> Result<()> {
        debug!(src = src, dst = dst, "Moving file");

        // Copy then delete
        self.copy_file(src, dst).await?;
        self.delete_file(src).await?;

        debug!(src = src, dst = dst, "File moved");
        Ok(())
    }

    /// Read a file as UTF-8 string
    ///
    /// # Arguments
    /// * `path` - Path to the file
    ///
    /// # Returns
    /// File contents as string
    pub async fn read_file_string(&self, path: &str) -> Result<String> {
        let bytes = self.read_file(path).await?;
        String::from_utf8(bytes)
            .map_err(|e| Error::Internal(format!("File is not valid UTF-8: {}", e)))
    }

    /// Write a string to a file
    ///
    /// # Arguments
    /// * `path` - Path to the file
    /// * `content` - String content to write
    pub async fn write_file_string(&self, path: &str, content: &str) -> Result<()> {
        self.write_file(path, content.as_bytes()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_file_info_serialization() {
        let info = FileInfo {
            path: "/workspace/test.py".to_string(),
            size: 1024,
            modified: Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
            is_dir: false,
            permissions: 0o644,
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("/workspace/test.py"));
        assert!(json.contains("1024"));

        let deserialized: FileInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.path, info.path);
        assert_eq!(deserialized.size, info.size);
        assert!(!deserialized.is_dir);
    }

    #[test]
    fn test_dir_entry_serialization() {
        let entry = DirEntry {
            name: "test.py".to_string(),
            path: "/workspace/test.py".to_string(),
            is_dir: false,
            size: 512,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("test.py"));

        let deserialized: DirEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, entry.name);
        assert_eq!(deserialized.path, entry.path);
    }

    #[test]
    fn test_workspace_serialization() {
        let workspace = Workspace {
            id: "ws-12345".to_string(),
            path: "/workspace/ws-12345".to_string(),
            created_at: Utc.with_ymd_and_hms(2024, 1, 15, 12, 0, 0).unwrap(),
        };

        let json = serde_json::to_string(&workspace).unwrap();
        assert!(json.contains("ws-12345"));

        let deserialized: Workspace = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, workspace.id);
        assert_eq!(deserialized.path, workspace.path);
    }

    #[test]
    fn test_vm_filesystem_config_default() {
        let config = VmFileSystemConfig::default();
        assert_eq!(config.vm_url, "http://localhost:8080");
        assert_eq!(config.workspace_root, PathBuf::from("/workspace"));
        assert_eq!(config.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(config.max_upload_size, 100 * 1024 * 1024);
    }

    #[test]
    fn test_vm_filesystem_new() {
        let fs = VmFileSystem::new("http://172.16.0.2:8080", "/workspace");
        assert_eq!(fs.vm_url(), "http://172.16.0.2:8080");
        assert_eq!(fs.workspace_root(), &PathBuf::from("/workspace"));
    }

    #[test]
    fn test_vm_filesystem_with_config() {
        let config = VmFileSystemConfig {
            vm_url: "http://10.0.0.1:9090".to_string(),
            workspace_root: PathBuf::from("/data"),
            timeout_ms: 60000,
            max_upload_size: 50 * 1024 * 1024,
        };

        let fs = VmFileSystem::with_config(config);
        assert_eq!(fs.vm_url(), "http://10.0.0.1:9090");
        assert_eq!(fs.workspace_root(), &PathBuf::from("/data"));
    }

    #[test]
    fn test_workspace_path() {
        let fs = VmFileSystem::new("http://localhost:8080", "/workspace");
        let path = fs.workspace_path("ws-123");
        assert_eq!(path, PathBuf::from("/workspace/ws-123"));
    }

    // Mock-based tests would go here
    // For actual HTTP testing, we would use wiremock or similar

    #[tokio::test]
    async fn test_file_operations_error_handling() {
        // Verify error construction (classification is tested in gateway-core)
        let not_found = Error::NotFound("File not found: /test".to_string());
        assert!(matches!(not_found, Error::NotFound(_)));

        let permission = Error::PermissionDenied("Permission denied: /test".to_string());
        assert!(matches!(permission, Error::PermissionDenied(_)));

        let file_too_large = Error::FileTooLarge {
            size: 200 * 1024 * 1024,
            limit: 100 * 1024 * 1024,
        };
        assert!(matches!(file_too_large, Error::FileTooLarge { .. }));
    }

    #[test]
    fn test_dir_entry_for_directory() {
        let entry = DirEntry {
            name: "subdir".to_string(),
            path: "/workspace/subdir".to_string(),
            is_dir: true,
            size: 0,
        };

        assert!(entry.is_dir);
        assert_eq!(entry.size, 0);
    }

    #[test]
    fn test_file_info_permissions() {
        let info = FileInfo {
            path: "/workspace/script.sh".to_string(),
            size: 256,
            modified: Utc::now(),
            is_dir: false,
            permissions: 0o755,
        };

        // Check executable permission
        assert_eq!(info.permissions & 0o111, 0o111); // User, group, other execute
    }
}
