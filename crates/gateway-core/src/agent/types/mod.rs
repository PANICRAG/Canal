//! Agent SDK Protocol Types
//!
//! This module contains types compatible with Claude Agent SDK protocol.
//!
//! # Permission System
//!
//! The permission system provides fine-grained control over tool execution:
//!
//! - `PermissionMode` - Global permission mode (Default, AcceptEdits, Plan, BypassPermissions)
//! - `PermissionRule` - Pattern-based rules for tool/path/command matching
//! - `PermissionChecker` - Trait for implementing custom permission logic
//! - `PermissionManager` - Runtime permission rule management with dynamic updates
//! - `PermissionHook` - Hook system integration for permission checking
//!
//! ## Example
//!
//! ```rust,ignore
//! use gateway_core::agent::types::{
//!     PermissionManager, PermissionMode, PermissionChecker,
//!     CompositePermissionChecker, PermissionHook,
//! };
//!
//! // Create a permission manager with default checkers
//! let manager = PermissionManager::new();
//!
//! // Set permission mode
//! manager.set_mode(PermissionMode::Default).await;
//!
//! // Add allowed directories
//! manager.add_allowed_directory("/home/user/project").await;
//!
//! // Check tool permission
//! let result = manager.check_tool(
//!     "Bash",
//!     &serde_json::json!({"command": "ls -la"}),
//!     Some("session-id"),
//!     Some("/home/user/project"),
//! ).await;
//! ```

pub mod content;
pub mod error;
pub mod execution;
pub mod hooks;
pub mod memory;
pub mod messages;
pub mod permissions;

pub use content::*;
pub use error::*;
pub use execution::*;
pub use hooks::*;
pub use memory::*;
pub use messages::*;
pub use permissions::*;
