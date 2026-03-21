//! Two-level application cache for the AI Gateway.
//!
//! - **L2 Semantic Cache**: Embedding-based similarity cache (Phase 3c)
//! - **L3 Plan Cache**: In-memory LRU cache for execution plans
//!
//! L1 (prefix cache) is provider-managed and not implemented here.

pub mod embedding;
pub mod plan;
pub mod semantic;

pub use embedding::{
    EmbeddingConfig, EmbeddingProvider, MockEmbeddingProvider, RemoteEmbeddingProvider,
};
pub use plan::{CachedPlan, PlanCache, PlanCacheConfig};
pub use semantic::{
    CacheSearchResult, CachedResponse, InMemorySemanticBackend, SemanticCache,
    SemanticCacheBackend, SemanticCacheConfig,
};

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Cache level indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheLevel {
    /// Semantic similarity cache (L2).
    Semantic,
    /// Plan cache (L3).
    Plan,
}

/// Cache statistics for monitoring.
#[derive(Debug, Default)]
pub struct CacheStats {
    pub l2_hits: AtomicU64,
    pub l2_misses: AtomicU64,
    pub l3_hits: AtomicU64,
    pub l3_misses: AtomicU64,
}

impl CacheStats {
    /// Create new stats.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a plan cache hit.
    pub fn record_l3_hit(&self) {
        self.l3_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a plan cache miss.
    pub fn record_l3_miss(&self) {
        self.l3_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a semantic cache hit.
    pub fn record_l2_hit(&self) {
        self.l2_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a semantic cache miss.
    pub fn record_l2_miss(&self) {
        self.l2_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Get snapshot of stats.
    pub fn snapshot(&self) -> CacheStatsSnapshot {
        CacheStatsSnapshot {
            l2_hits: self.l2_hits.load(Ordering::Relaxed),
            l2_misses: self.l2_misses.load(Ordering::Relaxed),
            l3_hits: self.l3_hits.load(Ordering::Relaxed),
            l3_misses: self.l3_misses.load(Ordering::Relaxed),
        }
    }
}

/// Serializable snapshot of cache statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStatsSnapshot {
    pub l2_hits: u64,
    pub l2_misses: u64,
    pub l3_hits: u64,
    pub l3_misses: u64,
}

impl CacheStatsSnapshot {
    /// L2 hit rate (0.0 - 1.0).
    pub fn l2_hit_rate(&self) -> f64 {
        let total = self.l2_hits + self.l2_misses;
        if total == 0 {
            0.0
        } else {
            self.l2_hits as f64 / total as f64
        }
    }

    /// L3 hit rate (0.0 - 1.0).
    pub fn l3_hit_rate(&self) -> f64 {
        let total = self.l3_hits + self.l3_misses;
        if total == 0 {
            0.0
        } else {
            self.l3_hits as f64 / total as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_stats_increment() {
        let stats = CacheStats::new();
        stats.record_l3_hit();
        stats.record_l3_hit();
        stats.record_l3_miss();
        let snap = stats.snapshot();
        assert_eq!(snap.l3_hits, 2);
        assert_eq!(snap.l3_misses, 1);
    }

    #[test]
    fn test_hit_rate_calculation() {
        let snap = CacheStatsSnapshot {
            l2_hits: 80,
            l2_misses: 20,
            l3_hits: 0,
            l3_misses: 0,
        };
        assert!((snap.l2_hit_rate() - 0.8).abs() < f64::EPSILON);
        assert!((snap.l3_hit_rate() - 0.0).abs() < f64::EPSILON);
    }
}
