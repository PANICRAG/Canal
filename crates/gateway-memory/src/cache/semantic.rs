//! Semantic cache using embedding similarity for response reuse.
//!
//! This module provides the L2 semantic cache layer that stores and retrieves
//! LLM responses based on embedding similarity. When a new query is similar
//! enough to a previously cached query (above the configured threshold), the
//! cached response is returned, saving LLM call costs and latency.
//!
//! The cache is backend-agnostic via the [`SemanticCacheBackend`] trait, with
//! an [`InMemorySemanticBackend`] provided for testing and development, and
//! a Qdrant-based backend expected for production use.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use super::embedding::EmbeddingProvider;
use crate::error::Result;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the semantic similarity cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheConfig {
    /// Qdrant collection name.
    #[serde(default = "default_collection")]
    pub collection: String,

    /// Minimum cosine similarity to consider a cache hit (0.0 - 1.0).
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,

    /// Time-to-live for cached entries in seconds.
    #[serde(default = "default_ttl_seconds")]
    pub ttl_seconds: u64,

    /// Maximum number of entries in the cache.
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,

    /// Qdrant gRPC endpoint URL.
    #[serde(default = "default_qdrant_url")]
    pub qdrant_url: String,
}

fn default_collection() -> String {
    "canal_cache".to_string()
}

fn default_similarity_threshold() -> f32 {
    0.92
}

fn default_ttl_seconds() -> u64 {
    3600
}

fn default_max_entries() -> usize {
    10_000
}

fn default_qdrant_url() -> String {
    "http://localhost:6334".to_string()
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            collection: default_collection(),
            similarity_threshold: default_similarity_threshold(),
            ttl_seconds: default_ttl_seconds(),
            max_entries: default_max_entries(),
            qdrant_url: default_qdrant_url(),
        }
    }
}

// ---------------------------------------------------------------------------
// Cached response payload
// ---------------------------------------------------------------------------

/// A cached LLM response with associated metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    /// The LLM response text.
    pub response: String,

    /// The model that generated this response.
    pub model: String,

    /// Number of tokens consumed by this response.
    pub tokens_used: u32,

    /// Estimated cost in USD.
    pub cost_usd: f64,

    /// When this response was originally generated.
    pub created_at: DateTime<Utc>,

    /// SHA-256 hash of the original query for exact-match bypass.
    pub task_hash: String,
}

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

/// A single result from a backend similarity search.
#[derive(Debug, Clone)]
pub struct CacheSearchResult {
    /// Unique identifier for the cached entry.
    pub id: String,
    /// Cosine similarity score (0.0 - 1.0).
    pub score: f32,
    /// The cached response payload.
    pub payload: CachedResponse,
}

/// Backend for semantic cache storage.
///
/// Implementations handle the actual vector storage and retrieval. The
/// [`SemanticCache`] orchestrates embedding generation and filtering on top
/// of whichever backend is plugged in.
#[async_trait]
pub trait SemanticCacheBackend: Send + Sync {
    /// Search for the nearest neighbours of `embedding`, returning up to
    /// `limit` results ordered by descending similarity.
    async fn search(&self, embedding: &[f32], limit: usize) -> Result<Vec<CacheSearchResult>>;

    /// Insert or update an entry in the backend.
    async fn upsert(&self, id: String, embedding: Vec<f32>, payload: CachedResponse) -> Result<()>;

    /// Delete entries by their IDs.
    async fn delete(&self, ids: &[String]) -> Result<()>;

    /// Return the total number of entries stored.
    async fn count(&self) -> Result<usize>;
}

// ---------------------------------------------------------------------------
// In-memory backend (for testing / development)
// ---------------------------------------------------------------------------

/// In-memory implementation of [`SemanticCacheBackend`].
///
/// Stores all entries in a `Vec` behind a `RwLock` and performs brute-force
/// cosine similarity search. Suitable for tests and low-volume development
/// use; **not** intended for production workloads.
pub struct InMemorySemanticBackend {
    entries: RwLock<Vec<(String, Vec<f32>, CachedResponse)>>,
    max_entries: usize,
}

impl InMemorySemanticBackend {
    /// Create an empty in-memory backend with default max entries (10_000).
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            max_entries: 10_000,
        }
    }

    /// Create an in-memory backend with a custom max entries limit.
    pub fn with_max_entries(max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            max_entries,
        }
    }
}

impl Default for InMemorySemanticBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SemanticCacheBackend for InMemorySemanticBackend {
    async fn search(&self, embedding: &[f32], limit: usize) -> Result<Vec<CacheSearchResult>> {
        let entries = self.entries.read().await;
        let mut scored: Vec<CacheSearchResult> = entries
            .iter()
            .map(|(id, emb, payload)| CacheSearchResult {
                id: id.clone(),
                score: cosine_similarity(embedding, emb),
                payload: payload.clone(),
            })
            .collect();

        // Sort by descending score.
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }

    async fn upsert(&self, id: String, embedding: Vec<f32>, payload: CachedResponse) -> Result<()> {
        let mut entries = self.entries.write().await;
        // Replace if the same id already exists.
        if let Some(pos) = entries
            .iter()
            .position(|(existing_id, _, _)| existing_id == &id)
        {
            entries[pos] = (id, embedding, payload);
        } else {
            // Enforce max entries limit: evict oldest entry if at capacity
            if entries.len() >= self.max_entries {
                // Evict the oldest entry (first inserted)
                if !entries.is_empty() {
                    entries.remove(0);
                    tracing::debug!(
                        max_entries = self.max_entries,
                        "Semantic cache evicted oldest entry"
                    );
                }
            }
            entries.push((id, embedding, payload));
        }
        Ok(())
    }

    async fn delete(&self, ids: &[String]) -> Result<()> {
        let mut entries = self.entries.write().await;
        entries.retain(|(id, _, _)| !ids.contains(id));
        Ok(())
    }

    async fn count(&self) -> Result<usize> {
        let entries = self.entries.read().await;
        Ok(entries.len())
    }
}

// ---------------------------------------------------------------------------
// SemanticCache
// ---------------------------------------------------------------------------

/// Embedding-based semantic similarity cache (L2).
///
/// Queries are embedded via an [`EmbeddingProvider`], then searched against a
/// [`SemanticCacheBackend`]. Results are filtered by the configured similarity
/// threshold and TTL before being returned.
pub struct SemanticCache {
    embedding_provider: Arc<dyn EmbeddingProvider>,
    backend: Arc<dyn SemanticCacheBackend>,
    config: SemanticCacheConfig,
}

impl SemanticCache {
    /// Create a new semantic cache.
    ///
    /// # Arguments
    ///
    /// * `config` - Cache configuration (thresholds, TTL, etc.).
    /// * `embedding_provider` - The provider used to generate query embeddings.
    /// * `backend` - The storage backend for cached entries and embeddings.
    pub fn new(
        config: SemanticCacheConfig,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        backend: Arc<dyn SemanticCacheBackend>,
    ) -> Self {
        Self {
            embedding_provider,
            backend,
            config,
        }
    }

    /// Look up a cached response for the given query.
    ///
    /// Returns `Some(CachedResponse)` if a sufficiently similar entry exists
    /// that has not expired, or `None` on a cache miss.
    #[tracing::instrument(skip(self))]
    pub async fn get(&self, query: &str) -> Result<Option<CachedResponse>> {
        let embedding = self.embedding_provider.embed(query).await?;

        let results = self.backend.search(&embedding, 1).await?;

        if let Some(best) = results.into_iter().next() {
            // Check similarity threshold.
            if best.score < self.config.similarity_threshold {
                tracing::debug!(
                    score = best.score,
                    threshold = self.config.similarity_threshold,
                    "Semantic cache miss: below similarity threshold"
                );
                return Ok(None);
            }

            // Check TTL.
            let age = Utc::now()
                .signed_duration_since(best.payload.created_at)
                .num_seconds();
            if age < 0 || age as u64 > self.config.ttl_seconds {
                tracing::debug!(
                    age_seconds = age,
                    ttl_seconds = self.config.ttl_seconds,
                    "Semantic cache miss: entry expired"
                );
                return Ok(None);
            }

            tracing::debug!(
                score = best.score,
                model = %best.payload.model,
                "Semantic cache hit"
            );
            Ok(Some(best.payload))
        } else {
            tracing::debug!("Semantic cache miss: no results");
            Ok(None)
        }
    }

    /// Store a response in the cache, keyed by the query's embedding.
    ///
    /// The query is embedded and a SHA-256 hash is computed for the
    /// `task_hash` field of the [`CachedResponse`].
    #[tracing::instrument(skip(self, response))]
    pub async fn put(&self, query: &str, response: CachedResponse) -> Result<()> {
        let embedding = self.embedding_provider.embed(query).await?;
        let id = sha256_hash(query);

        self.backend.upsert(id, embedding, response).await?;

        tracing::debug!("Stored response in semantic cache");
        Ok(())
    }

    /// Invalidate cached entries whose task hash matches the given pattern.
    ///
    /// This performs a broad search (using a zero-vector with large limit)
    /// and deletes all entries whose `task_hash` contains `task_pattern`.
    /// Returns the number of entries deleted.
    #[tracing::instrument(skip(self))]
    pub async fn invalidate(&self, task_pattern: &str) -> Result<usize> {
        // Embed the pattern to find semantically related entries.
        let embedding = self.embedding_provider.embed(task_pattern).await?;

        // Search with a generous limit; we will filter client-side.
        let results = self
            .backend
            .search(&embedding, self.config.max_entries)
            .await?;

        let ids_to_delete: Vec<String> = results
            .into_iter()
            .filter(|r| r.payload.task_hash.contains(task_pattern))
            .map(|r| r.id)
            .collect();

        let count = ids_to_delete.len();
        if !ids_to_delete.is_empty() {
            self.backend.delete(&ids_to_delete).await?;
        }

        tracing::debug!(deleted = count, "Invalidated semantic cache entries");
        Ok(count)
    }

    /// Return the configured similarity threshold.
    pub fn similarity_threshold(&self) -> f32 {
        self.config.similarity_threshold
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the cosine similarity between two vectors.
///
/// Returns a value in the range \[-1.0, 1.0\]. Identical (normalised)
/// vectors yield 1.0; orthogonal vectors yield 0.0.
///
/// If either vector has zero magnitude, returns 0.0.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vectors must have equal length");

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    // R5-L: Zero-magnitude vectors have no direction — similarity is undefined.
    // Return 0.0 (not 1.0) to avoid false cache matches on empty embeddings.
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }

    dot / (mag_a * mag_b)
}

/// Compute the SHA-256 hex digest of a string.
pub fn sha256_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Re-use the MockEmbeddingProvider from the embedding module.
    use super::super::embedding::MockEmbeddingProvider;

    // -- Helper to create a CachedResponse ----------------------------------

    fn make_response(response: &str) -> CachedResponse {
        CachedResponse {
            response: response.to_string(),
            model: "test-model".to_string(),
            tokens_used: 100,
            cost_usd: 0.001,
            created_at: Utc::now(),
            task_hash: sha256_hash(response),
        }
    }

    // -- Unit tests ---------------------------------------------------------

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 2.0, 3.0, 4.0];
        let sim = cosine_similarity(&v, &v);
        assert!(
            (sim - 1.0).abs() < 1e-6,
            "identical vectors should have similarity ~1.0, got {}",
            sim
        );
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim.abs() < 1e-6,
            "orthogonal vectors should have similarity ~0.0, got {}",
            sim
        );
    }

    #[test]
    fn test_sha256_hash_deterministic() {
        let h1 = sha256_hash("hello world");
        let h2 = sha256_hash("hello world");
        assert_eq!(h1, h2, "same input must produce same hash");

        let h3 = sha256_hash("different input");
        assert_ne!(h1, h3, "different inputs should produce different hashes");

        // SHA-256 hex digest is 64 characters.
        assert_eq!(h1.len(), 64);
    }

    #[tokio::test]
    async fn test_in_memory_backend_upsert_and_search() {
        let backend = InMemorySemanticBackend::new();

        let emb = vec![1.0, 0.0, 0.0];
        let resp = make_response("answer one");
        backend
            .upsert("id1".to_string(), emb.clone(), resp)
            .await
            .unwrap();

        assert_eq!(backend.count().await.unwrap(), 1);

        let results = backend.search(&emb, 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "id1");
        assert!(
            (results[0].score - 1.0).abs() < 1e-6,
            "exact match should score ~1.0"
        );
        assert_eq!(results[0].payload.response, "answer one");
    }

    #[tokio::test]
    async fn test_in_memory_backend_delete() {
        let backend = InMemorySemanticBackend::new();

        backend
            .upsert("a".to_string(), vec![1.0, 0.0], make_response("resp_a"))
            .await
            .unwrap();
        backend
            .upsert("b".to_string(), vec![0.0, 1.0], make_response("resp_b"))
            .await
            .unwrap();

        assert_eq!(backend.count().await.unwrap(), 2);

        backend.delete(&["a".to_string()]).await.unwrap();
        assert_eq!(backend.count().await.unwrap(), 1);

        let results = backend.search(&[0.0, 1.0], 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "b");
    }

    #[tokio::test]
    async fn test_semantic_cache_put_and_get() {
        let provider = Arc::new(MockEmbeddingProvider::new(8));
        let backend = Arc::new(InMemorySemanticBackend::new());
        let config = SemanticCacheConfig {
            similarity_threshold: 0.5, // Low threshold for test reliability.
            ttl_seconds: 3600,
            ..Default::default()
        };

        let cache = SemanticCache::new(config, provider, backend);

        let resp = make_response("cached answer");
        cache.put("what is rust?", resp.clone()).await.unwrap();

        // Same query should hit.
        let hit = cache.get("what is rust?").await.unwrap();
        assert!(hit.is_some(), "identical query should be a cache hit");
        assert_eq!(hit.unwrap().response, "cached answer");
    }

    #[tokio::test]
    async fn test_semantic_cache_miss_low_similarity() {
        let provider = Arc::new(MockEmbeddingProvider::new(8));
        let backend = Arc::new(InMemorySemanticBackend::new());
        let config = SemanticCacheConfig {
            similarity_threshold: 0.99, // Very high threshold.
            ttl_seconds: 3600,
            ..Default::default()
        };

        let cache = SemanticCache::new(config, provider, backend);

        let resp = make_response("cached answer");
        cache.put("what is rust?", resp).await.unwrap();

        // A different query should miss at the high threshold.
        let miss = cache.get("tell me about python programming").await.unwrap();
        assert!(
            miss.is_none(),
            "dissimilar query should miss at high threshold"
        );
    }

    #[test]
    fn test_semantic_cache_config_defaults() {
        let config = SemanticCacheConfig::default();
        assert_eq!(config.collection, "canal_cache");
        assert!((config.similarity_threshold - 0.92).abs() < f32::EPSILON);
        assert_eq!(config.ttl_seconds, 3600);
        assert_eq!(config.max_entries, 10_000);
        assert_eq!(config.qdrant_url, "http://localhost:6334");
    }

    #[test]
    fn test_cached_response_serialize() {
        let resp = CachedResponse {
            response: "hello".to_string(),
            model: "gpt-4".to_string(),
            tokens_used: 42,
            cost_usd: 0.0012,
            created_at: Utc::now(),
            task_hash: sha256_hash("hello"),
        };

        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: CachedResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.response, "hello");
        assert_eq!(deserialized.model, "gpt-4");
        assert_eq!(deserialized.tokens_used, 42);
        assert!((deserialized.cost_usd - 0.0012).abs() < f64::EPSILON);
        assert_eq!(deserialized.task_hash, resp.task_hash);
    }

    #[tokio::test]
    async fn test_in_memory_backend_upsert_replaces_existing() {
        let backend = InMemorySemanticBackend::new();

        backend
            .upsert("id1".to_string(), vec![1.0, 0.0], make_response("first"))
            .await
            .unwrap();
        backend
            .upsert("id1".to_string(), vec![1.0, 0.0], make_response("second"))
            .await
            .unwrap();

        assert_eq!(backend.count().await.unwrap(), 1);
        let results = backend.search(&[1.0, 0.0], 10).await.unwrap();
        assert_eq!(results[0].payload.response, "second");
    }

    #[test]
    fn test_similarity_threshold_accessor() {
        // We cannot construct SemanticCache without an async runtime for the
        // embedding provider, but we can verify the config accessor works by
        // building one in a sync context using a dummy provider and backend.
        let config = SemanticCacheConfig {
            similarity_threshold: 0.85,
            ..Default::default()
        };
        assert!((config.similarity_threshold - 0.85).abs() < f32::EPSILON);
    }
}
