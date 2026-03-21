//! Memory Storage Module
//!
//! Provides persistent storage for user memories and preferences.
//!
//! # Module Structure
//!
//! - `store` - MemoryStore trait definition
//! - `file_store` - File-based persistent storage
//! - `memory_store` - In-memory storage for testing
//! - `context` - Hierarchical context memory (WorkingMemory, SessionMemory, etc.)
//!
//! # Hierarchical Memory System
//!
//! The memory module implements a hierarchical memory pattern:
//!
//! - **WorkingMemory**: Immediate task context (current task tree, tool states, variables)
//! - **SessionMemory**: Conversation history within a session
//! - **LongTermMemory**: User preferences and learned patterns (via UserMemory)
//! - **TeamMemory**: Shared workflows and templates (optional)
//!
//! ```rust,ignore
//! use gateway_core::agent::memory::{ContextMemory, ContextManager, WorkingMemory};
//!
//! // Create hierarchical context
//! let mut context = ContextMemory::new("session-1", "user-1");
//!
//! // Start a task in working memory
//! let task_id = context.working.start_task("Process files");
//!
//! // Store variables between tool calls
//! context.working.set_variable("file_path", serde_json::json!("/tmp/input.txt"));
//!
//! // Use context manager for compression
//! let manager = ContextManager::new();
//! if manager.needs_compression(&context) {
//!     manager.compress(&mut context)?;
//! }
//! ```

pub mod context;
mod file_store;
mod memory_store;
mod store;

pub use file_store::FileMemoryStore;
pub use memory_store::InMemoryStore;
pub use store::{MemoryError, MemoryStore};

// Hierarchical context memory exports
pub use context::{
    CompressionResult,
    // Context manager
    ContextManager,
    // Context memory (combines all)
    ContextMemory,
    // Session memory
    SessionMemory,
    SessionMessage,
    TaskNode,
    TaskResult,
    TaskStatus,
    TaskTree,
    TeamMemory,
    ToolCallRecord,
    ToolState,
    ToolStateValue,
    Verification,
    VerificationStatus,
    // Working memory
    WorkingMemory,
};
