//! Firecracker VM Snapshot Management
//!
//! Provides snapshot functionality for fast VM restoration using Firecracker's snapshot API.
//!
//! # Architecture
//!
//! ```text
//! +-------------------+
//! |  SnapshotManager  |
//! |  +-------------+  |
//! |  | storage_path|  |---> Snapshot Storage
//! |  +-------------+  |
//! |  +-------------+  |     +------------------+
//! |  | config      |  |---->| SnapshotConfig   |
//! |  +-------------+  |     +------------------+
//! +-------------------+
//!         |
//!         v
//! +-------------------+
//! |  Firecracker API  |
//! |  - PUT /snapshot  |
//! |  - PUT /snapshot  |
//! |    /load          |
//! +-------------------+
//! ```
//!
//! # Features
//!
//! - Create VM snapshots (memory + state)
//! - Restore VMs from snapshots
//! - Incremental snapshot support
//! - Automatic cleanup of old snapshots

use crate::error::{ServiceError as Error, ServiceResult as Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Unique identifier for a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SnapshotId(pub String);

impl SnapshotId {
    /// Create a new snapshot ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Create a snapshot ID from a string.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Get the string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SnapshotId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a VM (used when restoring).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VmId(pub String);

impl VmId {
    /// Create a new VM ID.
    pub fn new() -> Self {
        Self(format!("vm-{}", Uuid::new_v4()))
    }

    /// Create a VM ID from a string.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Get the string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for VmId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for VmId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// State of a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotState {
    /// Snapshot is being created.
    Creating,
    /// Snapshot is complete and ready.
    Ready,
    /// Snapshot is being restored.
    Restoring,
    /// Snapshot is corrupted or invalid.
    Invalid,
    /// Snapshot is being deleted.
    Deleting,
}

impl Default for SnapshotState {
    fn default() -> Self {
        Self::Creating
    }
}

impl std::fmt::Display for SnapshotState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotState::Creating => write!(f, "creating"),
            SnapshotState::Ready => write!(f, "ready"),
            SnapshotState::Restoring => write!(f, "restoring"),
            SnapshotState::Invalid => write!(f, "invalid"),
            SnapshotState::Deleting => write!(f, "deleting"),
        }
    }
}

/// Type of snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotType {
    /// Full snapshot containing all memory and state.
    Full,
    /// Incremental/differential snapshot (only dirty pages).
    Diff,
}

impl Default for SnapshotType {
    fn default() -> Self {
        Self::Full
    }
}

/// Information about a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Unique snapshot identifier.
    pub id: String,
    /// ID of the VM this snapshot was created from.
    pub vm_id: String,
    /// When the snapshot was created.
    pub created_at: DateTime<Utc>,
    /// Total size of the snapshot in bytes.
    pub size_bytes: u64,
    /// Current state of the snapshot.
    pub state: SnapshotState,
    /// Type of snapshot (full or differential).
    pub snapshot_type: SnapshotType,
    /// Path to the memory file.
    pub memory_file: PathBuf,
    /// Path to the snapshot state file.
    pub snapshot_file: PathBuf,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Parent snapshot ID (for incremental snapshots).
    #[serde(default)]
    pub parent_snapshot_id: Option<String>,
    /// VM configuration at snapshot time.
    #[serde(default)]
    pub vm_config: Option<SnapshotVmConfig>,
}

impl SnapshotInfo {
    /// Create new snapshot info.
    pub fn new(id: SnapshotId, vm_id: &str, memory_file: PathBuf, snapshot_file: PathBuf) -> Self {
        Self {
            id: id.0,
            vm_id: vm_id.to_string(),
            created_at: Utc::now(),
            size_bytes: 0,
            state: SnapshotState::Creating,
            snapshot_type: SnapshotType::Full,
            memory_file,
            snapshot_file,
            description: None,
            parent_snapshot_id: None,
            vm_config: None,
        }
    }

    /// Get the age of this snapshot.
    pub fn age(&self) -> Duration {
        let now = Utc::now();
        let diff = now.signed_duration_since(self.created_at);
        Duration::from_secs(diff.num_seconds().max(0) as u64)
    }

    /// Check if the snapshot is ready for use.
    pub fn is_ready(&self) -> bool {
        self.state == SnapshotState::Ready
    }
}

/// VM configuration stored with snapshot for restoration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotVmConfig {
    /// Number of vCPUs.
    pub vcpu_count: u8,
    /// Memory size in MiB.
    pub mem_size_mib: u32,
    /// Network configuration.
    #[serde(default)]
    pub network: Option<SnapshotNetworkConfig>,
}

impl Default for SnapshotVmConfig {
    fn default() -> Self {
        Self {
            vcpu_count: 1,
            mem_size_mib: 512,
            network: None,
        }
    }
}

/// Network configuration for snapshot restoration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotNetworkConfig {
    /// TAP device name.
    pub tap_device: String,
    /// Guest MAC address.
    #[serde(default)]
    pub guest_mac: Option<String>,
}

/// Configuration for the snapshot manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotConfig {
    /// Maximum number of snapshots to keep.
    #[serde(default = "default_max_snapshots")]
    pub max_snapshots: usize,
    /// Maximum age of snapshots before automatic cleanup (in seconds).
    #[serde(default = "default_max_age_secs")]
    pub max_age_secs: u64,
    /// Whether to enable dirty page tracking for incremental snapshots.
    #[serde(default)]
    pub enable_diff_snapshots: bool,
    /// Compression settings for snapshots.
    #[serde(default)]
    pub compression: CompressionConfig,
    /// Whether to validate snapshots on creation.
    #[serde(default = "default_true")]
    pub validate_on_create: bool,
}

fn default_max_snapshots() -> usize {
    100
}

fn default_max_age_secs() -> u64 {
    86400 // 24 hours
}

fn default_true() -> bool {
    true
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            max_snapshots: default_max_snapshots(),
            max_age_secs: default_max_age_secs(),
            enable_diff_snapshots: false,
            compression: CompressionConfig::default(),
            validate_on_create: true,
        }
    }
}

impl SnapshotConfig {
    /// Create a new snapshot configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of snapshots.
    pub fn with_max_snapshots(mut self, max: usize) -> Self {
        self.max_snapshots = max;
        self
    }

    /// Set the maximum age in seconds.
    pub fn with_max_age_secs(mut self, secs: u64) -> Self {
        self.max_age_secs = secs;
        self
    }

    /// Enable differential snapshots.
    pub fn with_diff_snapshots(mut self, enable: bool) -> Self {
        self.enable_diff_snapshots = enable;
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<()> {
        if self.max_snapshots == 0 {
            return Err(Error::InvalidInput(
                "max_snapshots must be greater than 0".into(),
            ));
        }
        if self.max_age_secs == 0 {
            return Err(Error::InvalidInput(
                "max_age_secs must be greater than 0".into(),
            ));
        }
        Ok(())
    }
}

/// Compression configuration for snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Whether to enable compression.
    #[serde(default)]
    pub enabled: bool,
    /// Compression algorithm.
    #[serde(default)]
    pub algorithm: CompressionAlgorithm,
    /// Compression level (1-9).
    #[serde(default = "default_compression_level")]
    pub level: u8,
}

fn default_compression_level() -> u8 {
    6
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            algorithm: CompressionAlgorithm::default(),
            level: default_compression_level(),
        }
    }
}

/// Compression algorithm for snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompressionAlgorithm {
    /// No compression.
    #[default]
    None,
    /// Gzip compression.
    Gzip,
    /// LZ4 compression (fast).
    Lz4,
    /// Zstd compression (balanced).
    Zstd,
}

/// Request to create a snapshot (Firecracker API format).
#[derive(Debug, Serialize, Deserialize)]
struct CreateSnapshotRequest {
    /// Path to the file that will contain the guest memory.
    mem_file_path: String,
    /// Path to the file that will contain the microVM state.
    snapshot_path: String,
    /// Type of snapshot to create.
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_type: Option<String>,
}

/// Request to load a snapshot (Firecracker API format).
#[derive(Debug, Serialize, Deserialize)]
struct LoadSnapshotRequest {
    /// Path to the file that contains the guest memory.
    mem_file_path: String,
    /// Path to the file that contains the microVM state.
    snapshot_path: String,
    /// Whether to enable dirty page tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_diff_snapshots: Option<bool>,
    /// Whether to resume the microVM after loading.
    #[serde(skip_serializing_if = "Option::is_none")]
    resume_vm: Option<bool>,
}

/// Snapshot manager for creating and restoring Firecracker VM snapshots.
pub struct SnapshotManager {
    /// Base storage path for snapshots.
    storage_path: PathBuf,
    /// Snapshot configuration.
    config: SnapshotConfig,
    /// In-memory index of snapshots.
    snapshots: Arc<RwLock<HashMap<String, SnapshotInfo>>>,
    /// Path to Firecracker socket base directory.
    socket_base_path: PathBuf,
}

impl SnapshotManager {
    /// Create a new snapshot manager.
    pub fn new(storage_path: impl Into<PathBuf>, config: SnapshotConfig) -> Self {
        Self {
            storage_path: storage_path.into(),
            config,
            snapshots: Arc::new(RwLock::new(HashMap::new())),
            socket_base_path: PathBuf::from("/tmp/firecracker"),
        }
    }

    /// Create a new snapshot manager with custom socket path.
    pub fn with_socket_path(
        storage_path: impl Into<PathBuf>,
        config: SnapshotConfig,
        socket_base_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            storage_path: storage_path.into(),
            config,
            snapshots: Arc::new(RwLock::new(HashMap::new())),
            socket_base_path: socket_base_path.into(),
        }
    }

    /// Initialize the snapshot manager.
    pub async fn init(&self) -> Result<()> {
        // Create storage directory if it doesn't exist
        tokio::fs::create_dir_all(&self.storage_path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create snapshot storage: {}", e)))?;

        // Load existing snapshot index
        self.load_index().await?;

        info!("Snapshot manager initialized at {:?}", self.storage_path);
        Ok(())
    }

    /// Load the snapshot index from disk.
    async fn load_index(&self) -> Result<()> {
        let index_path = self.storage_path.join("index.json");

        if !index_path.exists() {
            debug!("No existing snapshot index found");
            return Ok(());
        }

        let data = tokio::fs::read_to_string(&index_path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read snapshot index: {}", e)))?;

        let loaded: HashMap<String, SnapshotInfo> = serde_json::from_str(&data)
            .map_err(|e| Error::Internal(format!("Failed to parse snapshot index: {}", e)))?;

        let mut snapshots = self.snapshots.write().await;
        *snapshots = loaded;

        info!("Loaded {} snapshots from index", snapshots.len());
        Ok(())
    }

    /// Save the snapshot index to disk.
    async fn save_index(&self) -> Result<()> {
        let snapshots = self.snapshots.read().await;
        let data = serde_json::to_string_pretty(&*snapshots)
            .map_err(|e| Error::Internal(format!("Failed to serialize snapshot index: {}", e)))?;

        let index_path = self.storage_path.join("index.json");
        tokio::fs::write(&index_path, data)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write snapshot index: {}", e)))?;

        Ok(())
    }

    /// Get the socket path for a VM.
    fn socket_path(&self, vm_id: &str) -> PathBuf {
        self.socket_base_path.join(format!("{}.sock", vm_id))
    }

    /// Get the snapshot directory for a specific snapshot.
    fn snapshot_dir(&self, snapshot_id: &SnapshotId) -> PathBuf {
        self.storage_path.join(snapshot_id.as_str())
    }

    /// Send an HTTP request to Firecracker via Unix socket.
    async fn send_request(
        &self,
        socket_path: &Path,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<(u16, String)> {
        let mut stream = UnixStream::connect(socket_path).await.map_err(|e| {
            Error::Internal(format!("Failed to connect to Firecracker socket: {}", e))
        })?;

        let content_length = body.map(|b| b.len()).unwrap_or(0);
        let request = if let Some(body) = body {
            format!(
                "{} {} HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Accept: application/json\r\n\
                 \r\n\
                 {}",
                method, path, content_length, body
            )
        } else {
            format!(
                "{} {} HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Accept: application/json\r\n\
                 \r\n",
                method, path
            )
        };

        debug!(
            "Sending snapshot request to Firecracker: {} {}",
            method, path
        );

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| Error::Internal(format!("Failed to send request: {}", e)))?;

        let mut response = vec![0u8; 8192];
        let n = stream
            .read(&mut response)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read response: {}", e)))?;

        let response_str = String::from_utf8_lossy(&response[..n]).to_string();

        // Parse HTTP response
        self.parse_http_response(&response_str)
    }

    /// Parse HTTP response to extract status code and body.
    fn parse_http_response(&self, response: &str) -> Result<(u16, String)> {
        let mut lines = response.lines();

        let status_line = lines
            .next()
            .ok_or_else(|| Error::Internal("Empty response".into()))?;

        let parts: Vec<&str> = status_line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(Error::Internal(format!(
                "Invalid status line: {}",
                status_line
            )));
        }

        let status_code: u16 = parts[1]
            .parse()
            .map_err(|_| Error::Internal(format!("Invalid status code: {}", parts[1])))?;

        let mut in_body = false;
        let mut body = String::new();
        for line in lines {
            if in_body {
                body.push_str(line);
                body.push('\n');
            } else if line.is_empty() {
                in_body = true;
            }
        }

        Ok((status_code, body.trim().to_string()))
    }

    /// Create a snapshot of a running VM.
    pub async fn create_snapshot(&self, vm_id: &str) -> Result<SnapshotId> {
        info!(vm_id = vm_id, "Creating snapshot");

        let snapshot_id = SnapshotId::new();
        let snapshot_dir = self.snapshot_dir(&snapshot_id);

        // Create snapshot directory
        tokio::fs::create_dir_all(&snapshot_dir)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create snapshot directory: {}", e)))?;

        let memory_file = snapshot_dir.join("memory.bin");
        let snapshot_file = snapshot_dir.join("state.bin");

        // Create snapshot info
        let mut info = SnapshotInfo::new(
            snapshot_id.clone(),
            vm_id,
            memory_file.clone(),
            snapshot_file.clone(),
        );

        // Add to index (as creating)
        {
            let mut snapshots = self.snapshots.write().await;
            snapshots.insert(snapshot_id.0.clone(), info.clone());
        }

        // Save index
        self.save_index().await?;

        // Create snapshot request
        let request = CreateSnapshotRequest {
            mem_file_path: memory_file.to_string_lossy().to_string(),
            snapshot_path: snapshot_file.to_string_lossy().to_string(),
            snapshot_type: if self.config.enable_diff_snapshots {
                Some("Diff".to_string())
            } else {
                Some("Full".to_string())
            },
        };

        let body =
            serde_json::to_string(&request).map_err(|e| Error::Serialization(e.to_string()))?;

        // Send snapshot request to Firecracker
        let socket_path = self.socket_path(vm_id);
        let (status, response) = self
            .send_request(&socket_path, "PUT", "/snapshot/create", Some(&body))
            .await?;

        if status >= 400 {
            // Cleanup on failure
            let _ = tokio::fs::remove_dir_all(&snapshot_dir).await;
            let mut snapshots = self.snapshots.write().await;
            snapshots.remove(&snapshot_id.0);
            self.save_index().await?;

            return Err(Error::Internal(format!(
                "Failed to create snapshot: {} - {}",
                status, response
            )));
        }

        // Calculate snapshot size
        let memory_size = tokio::fs::metadata(&memory_file)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let state_size = tokio::fs::metadata(&snapshot_file)
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        // Update snapshot info
        info.size_bytes = memory_size + state_size;
        info.state = SnapshotState::Ready;
        info.snapshot_type = if self.config.enable_diff_snapshots {
            SnapshotType::Diff
        } else {
            SnapshotType::Full
        };

        // Update index
        {
            let mut snapshots = self.snapshots.write().await;
            snapshots.insert(snapshot_id.0.clone(), info);
        }

        self.save_index().await?;

        info!(
            snapshot_id = %snapshot_id,
            size_bytes = memory_size + state_size,
            "Snapshot created successfully"
        );

        Ok(snapshot_id)
    }

    /// Restore a VM from a snapshot.
    pub async fn restore_snapshot(&self, snapshot_id: &SnapshotId) -> Result<VmId> {
        info!(snapshot_id = %snapshot_id, "Restoring snapshot");

        // Get snapshot info
        let info = {
            let snapshots = self.snapshots.read().await;
            snapshots
                .get(&snapshot_id.0)
                .cloned()
                .ok_or_else(|| Error::NotFound(format!("Snapshot not found: {}", snapshot_id)))?
        };

        if info.state != SnapshotState::Ready {
            return Err(Error::InvalidInput(format!(
                "Snapshot is not ready: state={}",
                info.state
            )));
        }

        // Verify snapshot files exist
        if !info.memory_file.exists() {
            return Err(Error::NotFound(format!(
                "Memory file not found: {:?}",
                info.memory_file
            )));
        }
        if !info.snapshot_file.exists() {
            return Err(Error::NotFound(format!(
                "Snapshot file not found: {:?}",
                info.snapshot_file
            )));
        }

        // Update state to restoring
        {
            let mut snapshots = self.snapshots.write().await;
            if let Some(snapshot) = snapshots.get_mut(&snapshot_id.0) {
                snapshot.state = SnapshotState::Restoring;
            }
        }

        // Generate new VM ID
        let new_vm_id = VmId::new();

        // Create load snapshot request
        let request = LoadSnapshotRequest {
            mem_file_path: info.memory_file.to_string_lossy().to_string(),
            snapshot_path: info.snapshot_file.to_string_lossy().to_string(),
            enable_diff_snapshots: Some(self.config.enable_diff_snapshots),
            resume_vm: Some(true),
        };

        let body =
            serde_json::to_string(&request).map_err(|e| Error::Serialization(e.to_string()))?;

        // For restoration, we assume a new Firecracker instance has been started
        // and is waiting for the load command. The socket path uses the new VM ID.
        let socket_path = self.socket_path(new_vm_id.as_str());

        let (status, response) = self
            .send_request(&socket_path, "PUT", "/snapshot/load", Some(&body))
            .await?;

        // Restore snapshot state back to ready
        {
            let mut snapshots = self.snapshots.write().await;
            if let Some(snapshot) = snapshots.get_mut(&snapshot_id.0) {
                snapshot.state = SnapshotState::Ready;
            }
        }

        if status >= 400 {
            return Err(Error::Internal(format!(
                "Failed to restore snapshot: {} - {}",
                status, response
            )));
        }

        info!(
            snapshot_id = %snapshot_id,
            new_vm_id = %new_vm_id,
            "Snapshot restored successfully"
        );

        Ok(new_vm_id)
    }

    /// List all snapshots.
    pub async fn list_snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        let snapshots = self.snapshots.read().await;
        let mut list: Vec<SnapshotInfo> = snapshots.values().cloned().collect();

        // Sort by creation time (newest first)
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(list)
    }

    /// Get a specific snapshot.
    pub async fn get_snapshot(&self, snapshot_id: &SnapshotId) -> Result<SnapshotInfo> {
        let snapshots = self.snapshots.read().await;
        snapshots
            .get(&snapshot_id.0)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("Snapshot not found: {}", snapshot_id)))
    }

    /// Delete a snapshot.
    pub async fn delete_snapshot(&self, snapshot_id: &SnapshotId) -> Result<()> {
        info!(snapshot_id = %snapshot_id, "Deleting snapshot");

        // Get and update snapshot state
        let snapshot_dir = {
            let mut snapshots = self.snapshots.write().await;
            let info = snapshots
                .get_mut(&snapshot_id.0)
                .ok_or_else(|| Error::NotFound(format!("Snapshot not found: {}", snapshot_id)))?;

            if info.state == SnapshotState::Restoring {
                return Err(Error::InvalidInput(
                    "Cannot delete snapshot while it is being restored".into(),
                ));
            }

            info.state = SnapshotState::Deleting;
            self.snapshot_dir(snapshot_id)
        };

        // Delete snapshot directory
        if snapshot_dir.exists() {
            tokio::fs::remove_dir_all(&snapshot_dir)
                .await
                .map_err(|e| {
                    // Restore state on error
                    warn!(snapshot_id = %snapshot_id, error = %e, "Failed to delete snapshot files");
                    Error::Internal(format!("Failed to delete snapshot files: {}", e))
                })?;
        }

        // Remove from index
        {
            let mut snapshots = self.snapshots.write().await;
            snapshots.remove(&snapshot_id.0);
        }

        self.save_index().await?;

        info!(snapshot_id = %snapshot_id, "Snapshot deleted");
        Ok(())
    }

    /// Cleanup old snapshots based on max_age configuration.
    pub async fn cleanup_old_snapshots(&self, max_age: Duration) -> Result<u32> {
        info!(
            max_age_secs = max_age.as_secs(),
            "Cleaning up old snapshots"
        );

        let mut deleted = 0;
        let now = Utc::now();

        // Find snapshots to delete
        let to_delete: Vec<SnapshotId> = {
            let snapshots = self.snapshots.read().await;
            snapshots
                .iter()
                .filter(|(_, info)| {
                    let age = now.signed_duration_since(info.created_at);
                    age.num_seconds() > max_age.as_secs() as i64
                        && info.state == SnapshotState::Ready
                })
                .map(|(id, _)| SnapshotId::from_string(id.clone()))
                .collect()
        };

        // Delete old snapshots
        for snapshot_id in to_delete {
            match self.delete_snapshot(&snapshot_id).await {
                Ok(_) => {
                    deleted += 1;
                }
                Err(e) => {
                    error!(
                        snapshot_id = %snapshot_id,
                        error = %e,
                        "Failed to delete old snapshot"
                    );
                }
            }
        }

        info!(deleted = deleted, "Snapshot cleanup complete");
        Ok(deleted)
    }

    /// Cleanup excess snapshots to stay under max_snapshots limit.
    pub async fn cleanup_excess_snapshots(&self) -> Result<u32> {
        let current_count = {
            let snapshots = self.snapshots.read().await;
            snapshots.len()
        };

        if current_count <= self.config.max_snapshots {
            return Ok(0);
        }

        let to_delete_count = current_count - self.config.max_snapshots;
        info!(
            current = current_count,
            max = self.config.max_snapshots,
            to_delete = to_delete_count,
            "Cleaning up excess snapshots"
        );

        // Get oldest snapshots to delete
        let mut all_snapshots = self.list_snapshots().await?;
        all_snapshots.reverse(); // Oldest first

        let mut deleted = 0;
        for info in all_snapshots.into_iter().take(to_delete_count) {
            let snapshot_id = SnapshotId::from_string(info.id);
            match self.delete_snapshot(&snapshot_id).await {
                Ok(_) => deleted += 1,
                Err(e) => {
                    warn!(
                        snapshot_id = %snapshot_id,
                        error = %e,
                        "Failed to delete excess snapshot"
                    );
                }
            }
        }

        Ok(deleted)
    }

    /// Run all cleanup tasks.
    pub async fn run_cleanup(&self) -> Result<u32> {
        let max_age = Duration::from_secs(self.config.max_age_secs);
        let age_deleted = self.cleanup_old_snapshots(max_age).await?;
        let excess_deleted = self.cleanup_excess_snapshots().await?;
        Ok(age_deleted + excess_deleted)
    }

    /// Get the storage path.
    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    /// Get the configuration.
    pub fn config(&self) -> &SnapshotConfig {
        &self.config
    }

    /// Get the number of snapshots.
    pub async fn count(&self) -> usize {
        let snapshots = self.snapshots.read().await;
        snapshots.len()
    }

    /// Get total storage used by all snapshots.
    pub async fn total_size_bytes(&self) -> u64 {
        let snapshots = self.snapshots.read().await;
        snapshots.values().map(|s| s.size_bytes).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_id_new() {
        let id1 = SnapshotId::new();
        let id2 = SnapshotId::new();
        assert_ne!(id1, id2);
        assert!(!id1.as_str().is_empty());
    }

    #[test]
    fn test_snapshot_id_from_string() {
        let id = SnapshotId::from_string("my-snapshot");
        assert_eq!(id.as_str(), "my-snapshot");
        assert_eq!(id.to_string(), "my-snapshot");
    }

    #[test]
    fn test_vm_id_new() {
        let id = VmId::new();
        assert!(id.as_str().starts_with("vm-"));
    }

    #[test]
    fn test_vm_id_from_string() {
        let id = VmId::from_string("my-vm");
        assert_eq!(id.as_str(), "my-vm");
    }

    #[test]
    fn test_snapshot_state_display() {
        assert_eq!(SnapshotState::Creating.to_string(), "creating");
        assert_eq!(SnapshotState::Ready.to_string(), "ready");
        assert_eq!(SnapshotState::Restoring.to_string(), "restoring");
        assert_eq!(SnapshotState::Invalid.to_string(), "invalid");
        assert_eq!(SnapshotState::Deleting.to_string(), "deleting");
    }

    #[test]
    fn test_snapshot_state_serialization() {
        let state = SnapshotState::Ready;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"ready\"");

        let deserialized: SnapshotState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, SnapshotState::Ready);
    }

    #[test]
    fn test_snapshot_info_new() {
        let id = SnapshotId::from_string("snap-1");
        let info = SnapshotInfo::new(
            id,
            "vm-1",
            PathBuf::from("/tmp/memory.bin"),
            PathBuf::from("/tmp/state.bin"),
        );

        assert_eq!(info.id, "snap-1");
        assert_eq!(info.vm_id, "vm-1");
        assert_eq!(info.state, SnapshotState::Creating);
        assert_eq!(info.size_bytes, 0);
        assert!(!info.is_ready());
    }

    #[test]
    fn test_snapshot_info_is_ready() {
        let mut info = SnapshotInfo::new(
            SnapshotId::from_string("test"),
            "vm-1",
            PathBuf::from("/tmp/mem"),
            PathBuf::from("/tmp/state"),
        );

        assert!(!info.is_ready());

        info.state = SnapshotState::Ready;
        assert!(info.is_ready());
    }

    #[test]
    fn test_snapshot_config_default() {
        let config = SnapshotConfig::default();
        assert_eq!(config.max_snapshots, 100);
        assert_eq!(config.max_age_secs, 86400);
        assert!(!config.enable_diff_snapshots);
        assert!(config.validate_on_create);
    }

    #[test]
    fn test_snapshot_config_builder() {
        let config = SnapshotConfig::new()
            .with_max_snapshots(50)
            .with_max_age_secs(3600)
            .with_diff_snapshots(true);

        assert_eq!(config.max_snapshots, 50);
        assert_eq!(config.max_age_secs, 3600);
        assert!(config.enable_diff_snapshots);
    }

    #[test]
    fn test_snapshot_config_validate() {
        let valid = SnapshotConfig::default();
        assert!(valid.validate().is_ok());

        let invalid = SnapshotConfig {
            max_snapshots: 0,
            ..Default::default()
        };
        assert!(invalid.validate().is_err());

        let invalid_age = SnapshotConfig {
            max_age_secs: 0,
            ..Default::default()
        };
        assert!(invalid_age.validate().is_err());
    }

    #[test]
    fn test_compression_config_default() {
        let config = CompressionConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.algorithm, CompressionAlgorithm::None);
        assert_eq!(config.level, 6);
    }

    #[test]
    fn test_snapshot_vm_config_default() {
        let config = SnapshotVmConfig::default();
        assert_eq!(config.vcpu_count, 1);
        assert_eq!(config.mem_size_mib, 512);
        assert!(config.network.is_none());
    }

    #[test]
    fn test_snapshot_info_serialization() {
        let info = SnapshotInfo::new(
            SnapshotId::from_string("snap-1"),
            "vm-1",
            PathBuf::from("/tmp/memory.bin"),
            PathBuf::from("/tmp/state.bin"),
        );

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("snap-1"));
        assert!(json.contains("vm-1"));

        let deserialized: SnapshotInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "snap-1");
        assert_eq!(deserialized.vm_id, "vm-1");
    }

    #[test]
    fn test_snapshot_type_serialization() {
        let full = SnapshotType::Full;
        assert_eq!(serde_json::to_string(&full).unwrap(), "\"full\"");

        let diff = SnapshotType::Diff;
        assert_eq!(serde_json::to_string(&diff).unwrap(), "\"diff\"");
    }

    #[tokio::test]
    async fn test_snapshot_manager_new() {
        let config = SnapshotConfig::default();
        let manager = SnapshotManager::new("/tmp/snapshots", config);

        assert_eq!(manager.storage_path(), Path::new("/tmp/snapshots"));
        assert_eq!(manager.count().await, 0);
        assert_eq!(manager.total_size_bytes().await, 0);
    }

    #[tokio::test]
    async fn test_snapshot_manager_with_socket_path() {
        let config = SnapshotConfig::default();
        let manager =
            SnapshotManager::with_socket_path("/tmp/snapshots", config, "/var/run/firecracker");

        assert_eq!(manager.storage_path(), Path::new("/tmp/snapshots"));
        assert_eq!(
            manager.socket_path("vm-1"),
            PathBuf::from("/var/run/firecracker/vm-1.sock")
        );
    }

    #[tokio::test]
    async fn test_snapshot_manager_list_empty() {
        let config = SnapshotConfig::default();
        let manager = SnapshotManager::new("/tmp/test-snapshots", config);

        let list = manager.list_snapshots().await.unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_parse_http_response() {
        let config = SnapshotConfig::default();
        let manager = SnapshotManager::new("/tmp/test", config);

        let response = "HTTP/1.1 204 No Content\r\n\r\n";
        let (status, body) = manager.parse_http_response(response).unwrap();
        assert_eq!(status, 204);
        assert!(body.is_empty());

        let response_with_body =
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}";
        let (status, body) = manager.parse_http_response(response_with_body).unwrap();
        assert_eq!(status, 200);
        assert!(body.contains("ok"));
    }

    #[test]
    fn test_snapshot_dir_generation() {
        let config = SnapshotConfig::default();
        let manager = SnapshotManager::new("/var/lib/snapshots", config);

        let snapshot_id = SnapshotId::from_string("abc123");
        let dir = manager.snapshot_dir(&snapshot_id);
        assert_eq!(dir, PathBuf::from("/var/lib/snapshots/abc123"));
    }
}
