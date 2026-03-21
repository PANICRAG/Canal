//! Plan cache for reusing execution plans on repeated task patterns.
//!
//! Uses an in-memory DashMap with LRU-style eviction and TTL expiry.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Configuration for the plan cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanCacheConfig {
    /// Maximum number of cached plans.
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
    /// Time-to-live for cached plans in seconds.
    #[serde(default = "default_ttl_seconds")]
    pub ttl_seconds: u64,
}

fn default_max_entries() -> usize {
    1000
}

fn default_ttl_seconds() -> u64 {
    1800 // 30 minutes
}

impl Default for PlanCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: default_max_entries(),
            ttl_seconds: default_ttl_seconds(),
        }
    }
}

/// A cached execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPlan {
    /// The normalized task key.
    pub task_key: String,
    /// The plan content (JSON or structured text).
    pub plan: String,
    /// Number of successful executions using this plan.
    pub success_count: u32,
    /// Number of failed executions using this plan.
    pub failure_count: u32,
    /// When this plan was last used.
    #[serde(skip)]
    pub last_used: Option<Instant>,
    /// When this plan was created.
    #[serde(skip)]
    pub created_at: Option<Instant>,
}

impl CachedPlan {
    /// Create a new cached plan.
    pub fn new(task_key: String, plan: String) -> Self {
        let now = Instant::now();
        Self {
            task_key,
            plan,
            success_count: 0,
            failure_count: 0,
            last_used: Some(now),
            created_at: Some(now),
        }
    }

    /// Success rate (0.0 - 1.0). Returns 0.5 if no outcomes recorded.
    pub fn success_rate(&self) -> f32 {
        let total = self.success_count + self.failure_count;
        if total == 0 {
            0.5
        } else {
            self.success_count as f32 / total as f32
        }
    }

    /// Whether this plan has expired.
    pub fn is_expired(&self, ttl: Duration) -> bool {
        match self.created_at {
            Some(created) => created.elapsed() > ttl,
            None => false,
        }
    }
}

/// In-memory plan cache with LRU eviction and TTL.
pub struct PlanCache {
    store: DashMap<String, CachedPlan>,
    /// LRU order tracking (most recently used keys at the end).
    access_order: RwLock<Vec<String>>,
    config: PlanCacheConfig,
}

impl PlanCache {
    /// Create a new plan cache with the given configuration.
    pub fn new(config: PlanCacheConfig) -> Self {
        Self {
            store: DashMap::new(),
            access_order: RwLock::new(Vec::new()),
            config,
        }
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(PlanCacheConfig::default())
    }

    /// Look up a cached plan by normalized task key.
    ///
    /// Returns `None` if not found or expired.
    #[tracing::instrument(skip(self))]
    pub async fn get(&self, task_key: &str) -> Option<CachedPlan> {
        let ttl = Duration::from_secs(self.config.ttl_seconds);

        if let Some(mut entry) = self.store.get_mut(task_key) {
            // Check TTL
            if entry.is_expired(ttl) {
                drop(entry);
                self.store.remove(task_key);
                tracing::debug!(task_key, "Plan cache entry expired");
                return None;
            }

            // Update LRU
            entry.last_used = Some(Instant::now());
            let plan = entry.clone();
            drop(entry);

            // Move to end of access order
            self.touch_lru(task_key).await;

            tracing::debug!(task_key, "Plan cache hit");
            Some(plan)
        } else {
            tracing::debug!(task_key, "Plan cache miss");
            None
        }
    }

    /// Store or update a cached plan. Evicts LRU entry if at capacity.
    #[tracing::instrument(skip(self, plan), fields(task_key = %plan.task_key))]
    pub async fn put(&self, plan: CachedPlan) {
        let key = plan.task_key.clone();

        // Atomic eviction + insert: hold the LRU lock across both operations
        // to prevent concurrent puts from exceeding max_entries.
        let mut order = self.access_order.write().await;

        if !self.store.contains_key(&key) && self.store.len() >= self.config.max_entries {
            // Evict LRU entry while holding the lock
            if let Some(oldest_key) = order.first().cloned() {
                order.remove(0);
                self.store.remove(&oldest_key);
                tracing::debug!(evicted_key = %oldest_key, "LRU eviction");
            }
        }

        self.store.insert(key.clone(), plan);

        // Update access order while still holding the lock
        order.retain(|k| k != &key);
        order.push(key);

        drop(order);

        tracing::debug!(cache_size = self.store.len(), "Plan cached");
    }

    /// Record execution outcome for a cached plan.
    pub fn record_outcome(&self, task_key: &str, success: bool) {
        if let Some(mut entry) = self.store.get_mut(task_key) {
            if success {
                entry.success_count += 1;
            } else {
                entry.failure_count += 1;
            }
            tracing::debug!(
                task_key,
                success,
                success_rate = entry.success_rate(),
                "Plan outcome recorded"
            );
        }
    }

    /// Normalize a task description into a cache key.
    ///
    /// 1. Lowercase
    /// 2. Remove common stopwords
    /// 3. Sort remaining tokens alphabetically
    /// 4. Join with single space
    pub fn normalize_key(task: &str) -> String {
        const STOPWORDS: &[&str] = &[
            "a", "an", "the", "is", "are", "was", "were", "be", "been", "please", "help", "me",
            "i", "my", "can", "you", "do", "to", "for", "of", "in", "on", "at", "with", "and",
            "or", "that", "this", "it", "its", "but", "not", "from", "by",
        ];

        let mut tokens: Vec<String> = task
            .to_lowercase()
            .split_whitespace()
            .filter(|w| !STOPWORDS.contains(w))
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .filter(|w| !w.is_empty())
            .collect();

        tokens.sort();
        tokens.dedup();
        tokens.join(" ")
    }

    /// Number of entries in the cache.
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// Remove all expired entries.
    pub async fn cleanup_expired(&self) -> usize {
        let ttl = Duration::from_secs(self.config.ttl_seconds);
        let mut expired_keys = Vec::new();

        for entry in self.store.iter() {
            if entry.value().is_expired(ttl) {
                expired_keys.push(entry.key().clone());
            }
        }

        let count = expired_keys.len();
        for key in &expired_keys {
            self.store.remove(key);
        }

        // Clean up access order
        if count > 0 {
            let mut order = self.access_order.write().await;
            order.retain(|k| !expired_keys.contains(k));
        }

        count
    }

    /// Evict entries with success rate below threshold.
    pub fn evict_low_quality(&self, min_success_rate: f32) -> usize {
        let mut to_remove = Vec::new();
        for entry in self.store.iter() {
            let total = entry.success_count + entry.failure_count;
            if total >= 3 && entry.success_rate() < min_success_rate {
                to_remove.push(entry.key().clone());
            }
        }
        let count = to_remove.len();
        for key in to_remove {
            self.store.remove(&key);
        }
        count
    }

    /// Move key to end of LRU access order.
    async fn touch_lru(&self, key: &str) {
        let mut order = self.access_order.write().await;
        order.retain(|k| k != key);
        order.push(key.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_key_basic() {
        assert_eq!(
            PlanCache::normalize_key("Please help me write a test"),
            "test write"
        );
    }

    #[test]
    fn test_normalize_key_sorts_tokens() {
        assert_eq!(
            PlanCache::normalize_key("write code then test"),
            "code test then write"
        );
    }

    #[test]
    fn test_normalize_key_deduplicates() {
        assert_eq!(PlanCache::normalize_key("test test test code"), "code test");
    }

    #[test]
    fn test_normalize_key_empty() {
        assert_eq!(PlanCache::normalize_key(""), "");
        assert_eq!(PlanCache::normalize_key("the a an"), "");
    }

    #[test]
    fn test_normalize_key_case_insensitive() {
        assert_eq!(
            PlanCache::normalize_key("Write Test"),
            PlanCache::normalize_key("write test")
        );
    }

    #[test]
    fn test_cached_plan_success_rate() {
        let mut plan = CachedPlan::new("test".into(), "plan".into());
        assert_eq!(plan.success_rate(), 0.5); // No outcomes yet

        plan.success_count = 8;
        plan.failure_count = 2;
        assert!((plan.success_rate() - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_cached_plan_expiry() {
        let plan = CachedPlan::new("test".into(), "plan".into());
        assert!(!plan.is_expired(Duration::from_secs(60)));
        assert!(!plan.is_expired(Duration::from_secs(1)));
        // Note: can't easily test true expiry without sleeping
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let cache = PlanCache::with_defaults();
        let plan = CachedPlan::new("test_key".into(), "my plan".into());
        cache.put(plan).await;

        let result = cache.get("test_key").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().plan, "my plan");
    }

    #[tokio::test]
    async fn test_get_miss() {
        let cache = PlanCache::with_defaults();
        assert!(cache.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_record_outcome() {
        let cache = PlanCache::with_defaults();
        cache.put(CachedPlan::new("k".into(), "p".into())).await;

        cache.record_outcome("k", true);
        cache.record_outcome("k", true);
        cache.record_outcome("k", false);

        let plan = cache.get("k").await.unwrap();
        assert_eq!(plan.success_count, 2);
        assert_eq!(plan.failure_count, 1);
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let config = PlanCacheConfig {
            max_entries: 2,
            ttl_seconds: 3600,
        };
        let cache = PlanCache::new(config);

        cache
            .put(CachedPlan::new("a".into(), "plan_a".into()))
            .await;
        cache
            .put(CachedPlan::new("b".into(), "plan_b".into()))
            .await;

        // Access "a" to make it recently used
        cache.get("a").await;

        // Insert "c" — should evict "b" (least recently used)
        cache
            .put(CachedPlan::new("c".into(), "plan_c".into()))
            .await;

        assert!(cache.get("a").await.is_some());
        assert!(cache.get("b").await.is_none()); // evicted
        assert!(cache.get("c").await.is_some());
    }

    #[tokio::test]
    async fn test_evict_low_quality() {
        let cache = PlanCache::with_defaults();

        let mut good = CachedPlan::new("good".into(), "p".into());
        good.success_count = 9;
        good.failure_count = 1;
        cache.put(good).await;

        let mut bad = CachedPlan::new("bad".into(), "p".into());
        bad.success_count = 1;
        bad.failure_count = 9;
        cache.put(bad).await;

        let evicted = cache.evict_low_quality(0.5);
        assert_eq!(evicted, 1);
        assert!(cache.get("good").await.is_some());
        assert!(cache.get("bad").await.is_none());
    }

    #[tokio::test]
    async fn test_len_and_is_empty() {
        let cache = PlanCache::with_defaults();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);

        cache.put(CachedPlan::new("k".into(), "p".into())).await;
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }
}
