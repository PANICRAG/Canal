//! Chat Engine module
//!
//! Provides conversation management, message handling, and streaming support.
//!
//! ## Session Persistence
//!
//! The chat engine supports session persistence through the Agent SDK's session
//! management system. This allows:
//!
//! - **Session Resume**: Load an existing session and continue the conversation
//! - **Session Fork**: Create a new branch from an existing session
//! - **Context Compaction**: Automatically summarize older messages to reduce context size
//!
//! ## Artifact Storage
//!
//! The chat engine supports artifact generation and storage:
//!
//! - **Artifact Extraction**: Automatically extract code blocks and documents from responses
//! - **Artifact Storage**: Persist artifacts with file-based or memory storage
//! - **Artifact Types**: Support for documents, code blocks, charts, tables, images, etc.
//!
//! ## Example
//!
//! ```ignore
//! use gateway_core::chat::{ChatEngine, ChatEngineConfig};
//! use gateway_core::agent::session::{DefaultSessionManager, MemorySessionStorage};
//!
//! let storage = Arc::new(MemorySessionStorage::new());
//! let session_manager = Arc::new(DefaultSessionManager::new(storage));
//!
//! let engine = ChatEngine::with_session_manager(
//!     llm_router,
//!     None,
//!     ChatEngineConfig::default(),
//!     session_manager,
//! );
//!
//! // Create a new persistent session
//! let session = engine.create_persistent_session("/workspace").await?;
//!
//! // Resume an existing session
//! let resumed = engine.resume_session(&session_id).await?;
//!
//! // Fork a session for alternative conversation paths
//! let forked = engine.fork_session(&session_id).await?;
//! ```

pub mod artifact;
pub mod artifact_extractor;
pub mod artifact_store;
pub mod engine;
pub mod message;
pub mod repository;
pub mod session;
pub mod streaming;
pub mod tool_use;

pub use artifact::{
    ArtifactContent, ArtifactMetadata, ArtifactType, ChartContent, ChartOptions, ChartType,
    CodeBlockContent, CodeHighlight, ColumnDataType, DocumentContent, DocumentFormat,
    DocumentSection, ImageContent, StoredArtifact, TableColumn, TableContent, TimelineContent,
    TimelineEvent, TimelineOrientation,
};
pub use artifact_extractor::{ArtifactExtractor, ArtifactExtractorConfig, ExtractedArtifact};
pub use artifact_store::{
    ArtifactQuery, ArtifactResult, ArtifactStore, ArtifactStoreError, FileArtifactStore,
    MemoryArtifactStore,
};
pub use engine::{ChatEngine, ChatEngineConfig, ChatResponse, ToolCallInfo};
pub use message::{Artifact, ArtifactAction, ChatMessage, MessageRole};
pub use repository::{
    ConversationRepository, MessageRepository, NewConversation, NewMessage, StoredConversation,
    StoredMessage,
};
pub use session::{ChatSession, SessionSummary};
pub use streaming::StreamEvent;
pub use tool_use::{ToolUseConfig, ToolUseEngine, ToolUseEvent};

// Re-export session persistence types from agent module for convenience
pub use crate::agent::session::{
    ContextCompactor, DefaultSessionManager, FileSessionStorage, MemorySessionStorage,
    Session as PersistentSession, SessionError, SessionManager, SessionMetadata, SessionSnapshot,
    SessionStorage,
};
