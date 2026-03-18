//! Virtual Machine Management Module
//!
//! Provides Firecracker microVM management for secure code execution and browser automation.
//!
//! # Architecture
//!
//! ```text
//! +------------------+     +------------------+
//! |    VmManager     |     |   VmExecutor     |
//! |  +------------+  |     |  +------------+  |
//! |  | VmPool     |  |---->|  | HTTP Client|  |---> VM API Server
//! |  +------------+  |     |  +------------+  |
//! |  +------------+  |     +------------------+
//! |  | Firecracker|  |              |
//! |  | Client     |  |              v
//! |  +------------+  |     +------------------+
//! +------------------+     |    VmSession     |
//!                          |  - Variables     |
//!                          |  - Browser State |
//!                          |  - History       |
//!                          +------------------+
//! ```
//!
//! # Features
//!
//! - VM lifecycle management (create, start, stop, destroy)
//! - Pre-warmed VM pool for fast acquisition
//! - Health checking via HTTP ping
//! - Command execution within VMs
//! - Automatic cleanup and resource management
//! - Python code execution with sandboxing
//! - Browser automation via Playwright
//! - Session management with variable persistence
//!
//! # Error Handling
//!
//! The module provides comprehensive error handling through `VmError`:
//! - Lifecycle errors (start, stop, state management)
//! - Communication errors (connection, timeout, network)
//! - Resource errors (pool exhaustion, health checks)
//! - Execution errors (code execution failures)
//!
//! # Result Handling
//!
//! The `result` module provides comprehensive result handling for VM code execution:
//! - `VmExecutionResult` - comprehensive result data with stdout/stderr, return values, artifacts
//! - `ResultCollector` - aggregates partial results and supports streaming
//! - `ArtifactCollector` - collects screenshots, files, and other execution artifacts
//! - Streaming support via `StreamChunk` for real-time output delivery

mod client;
pub mod config;
pub mod error;
pub mod executor;
pub mod filesystem;
pub mod firecracker;
mod manager;
mod pool;
pub mod result;
pub mod session;
pub mod snapshot;
pub mod vnc;
pub mod vnc_permission;

// Re-exports from client module (legacy API)
pub use client::{
    BootSource, DriveConfig, FirecrackerClient, FirecrackerConfig, MachineConfig, NetworkInterface,
    VmConfig,
};

// Re-exports from manager module
pub use manager::{ExecResult, VmInstance, VmManager, VmManagerConfig, VmStatus};

// Re-exports from pool module
pub use pool::{VmPool, VmPoolConfig, VmPoolStats};

// Re-exports from config module (new API)
pub use config::{
    ConfigError, DriveConfig as DriveConfiguration, NetworkConfig, PoolConfig,
    VmConfig as VmConfiguration,
};

// Re-exports from firecracker module (new API)
pub use firecracker::{
    ApiResponse, Drive, FirecrackerClient as FirecrackerApiClient, InstanceAction,
    InstanceActionInfo, MachineConfig as MachineConfiguration, NetworkInterface as NetworkIface,
    RateLimiter, TokenBucket, VmState,
};

// Re-exports from filesystem module
pub use filesystem::{DirEntry, FileInfo, VmFileSystem, VmFileSystemConfig, Workspace};

// Re-exports from executor module
pub use executor::{
    BrowserAction, BrowserResult, ExecutionContext, ExecutionResult, ExecutionStatus, OutputChunk,
    VmExecutor,
};

// Re-exports from session module
pub use session::{
    BrowserState, ExecutionEntry, SessionConfig, SessionInfo, SessionManager, SessionState,
    ViewportDimensions, VmSession,
};

// Re-exports from error module
pub use error::{VmError, VmErrorContext, VmResult};

// Re-exports from snapshot module
pub use snapshot::{
    CompressionAlgorithm, CompressionConfig, SnapshotConfig, SnapshotId, SnapshotInfo,
    SnapshotManager, SnapshotNetworkConfig, SnapshotState, SnapshotType, SnapshotVmConfig, VmId,
};

// Re-exports from result module
pub use result::{
    ApiArtifact, Artifact, ArtifactCollector, ArtifactType, ErrorDetails, ErrorType,
    ExecutionMetadata, ExecutionStatus as VmExecutionStatus, ResultCollector, StreamChunk,
    StreamConfig, VmExecutionResponse, VmExecutionResult,
};

// Re-exports from vnc module
pub use vnc::{
    VncConfig, VncConfigError, VncInfo, VncResizeRequest, VncResizeResponse, VncState, VncStatus,
};

// Re-exports from vnc_permission module
pub use vnc_permission::{
    TokenValidationResult, VncAccessConfig, VncAccessError, VncAccessManager, VncAccessStats,
    VncAccessToken, VncPermissionType, VncPermissions,
};
