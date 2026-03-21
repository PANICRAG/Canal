//! Memory service trait.
//!
//! Defines the boundary for memory storage and retrieval.
//! - Local impl wraps `UnifiedMemoryStore` directly
//! - Remote impl sends requests via gRPC to memory-service

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ServiceResult;

/// A memory entry for storage/retrieval across the service boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Unique identifier
    pub id: String,
    /// Category of the memory entry
    pub category: String,
    /// The content/value
    pub content: String,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// Source of this memory
    pub source: String,
    /// When this was created (RFC 3339)
    pub created_at: String,
}

/// Service boundary for memory operations.
///
/// # Example
///
/// ```rust,ignore
/// let memory: Arc<dyn MemoryService> = Arc::new(LocalMemoryService::new(store));
/// memory.store(item).await?;
/// let results = memory.query("category", "search term", 10).await?;
/// ```
#[async_trait]
pub trait MemoryService: Send + Sync {
    /// Store a memory entry.
    async fn store(&self, item: MemoryItem) -> ServiceResult<()>;

    /// Query memory entries by category and search term.
    async fn query(
        &self,
        category: &str,
        query: &str,
        limit: usize,
    ) -> ServiceResult<Vec<MemoryItem>>;

    /// List all entries in a category.
    async fn list_by_category(
        &self,
        category: &str,
        limit: usize,
    ) -> ServiceResult<Vec<MemoryItem>>;

    /// Delete a memory entry by ID.
    async fn delete(&self, id: &str) -> ServiceResult<()>;

    /// Get the total number of stored entries.
    async fn count(&self) -> ServiceResult<usize>;

    /// Health check for this service.
    async fn health(&self) -> ServiceResult<bool>;
}
