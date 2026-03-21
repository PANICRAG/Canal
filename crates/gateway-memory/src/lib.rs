//! Memory and caching crate
//!
//! Provides unified memory storage, semantic caching, plan caching,
//! and embedding support.
//!
//! Extracted from `gateway-core::memory` and `gateway-core::cache` as a
//! standalone crate for faster compilation and independent versioning.

pub mod cache;
pub mod error;
pub mod persistence;
pub mod unified;

pub use error::{Error, Result};

pub use persistence::MemoryBackend;
pub use unified::{
    Confidence, MemoryCategory, MemoryConfig, MemoryEntry, MemoryPattern, MemoryPreferences,
    MemorySource, MemoryStats, PatternType, UnifiedMemoryStore, UserMemoryContext,
};

// Re-export commonly used cache types
pub use cache::{
    CacheLevel, CacheStats, CacheStatsSnapshot, CachedPlan, CachedResponse, EmbeddingConfig,
    EmbeddingProvider, MockEmbeddingProvider, PlanCache, PlanCacheConfig, RemoteEmbeddingProvider,
    SemanticCache, SemanticCacheConfig,
};
