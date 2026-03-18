//! Token bucket rate limiter for API endpoints.
//!
//! Provides per-user, per-endpoint-category, per-tier rate limiting.
//! Uses a DashMap for concurrent access without locks.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Endpoint categories for rate limiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointCategory {
    /// /api/chat/* endpoints
    Chat,
    /// /api/tools/* endpoints
    ToolResult,
    /// /api/plugins/* + /api/connectors/* + /api/bundles/*
    Plugin,
    /// /api/debug/* endpoints
    Debug,
    /// /api/auth/* endpoints
    Auth,
    /// Admin-only endpoints (reload, etc.)
    Admin,
    /// All other endpoints
    Other,
}

impl EndpointCategory {
    /// Classify a request path into an endpoint category.
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/api/chat") {
            Self::Chat
        } else if path.starts_with("/api/tools") {
            Self::ToolResult
        } else if path.starts_with("/api/plugins")
            || path.starts_with("/api/connectors")
            || path.starts_with("/api/bundles")
        {
            Self::Plugin
        } else if path.starts_with("/api/debug") {
            Self::Debug
        } else if path.starts_with("/api/auth") {
            Self::Auth
        } else {
            Self::Other
        }
    }
}

/// User tier determines rate limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateLimitTier {
    Free,
    Pro,
    Enterprise,
}

/// Rate limit configuration per tier per category.
#[derive(Debug, Clone, Copy)]
pub struct TierLimits {
    /// Maximum requests per window
    pub max_requests: u32,
    /// Window duration
    pub window: Duration,
    /// Burst allowance above max_requests (consumed instantly)
    pub burst: u32,
}

/// Rate limiter configuration.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Limits per tier per category
    limits: std::collections::HashMap<(RateLimitTier, EndpointCategory), TierLimits>,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        use EndpointCategory::*;
        use RateLimitTier::*;

        let mut limits = std::collections::HashMap::new();

        // Free tier
        limits.insert(
            (Free, Chat),
            TierLimits {
                max_requests: 30,
                window: Duration::from_secs(60),
                burst: 5,
            },
        );
        limits.insert(
            (Free, ToolResult),
            TierLimits {
                max_requests: 10,
                window: Duration::from_secs(60),
                burst: 2,
            },
        );
        limits.insert(
            (Free, Plugin),
            TierLimits {
                max_requests: 20,
                window: Duration::from_secs(60),
                burst: 3,
            },
        );
        limits.insert(
            (Free, Debug),
            TierLimits {
                max_requests: 10,
                window: Duration::from_secs(60),
                burst: 2,
            },
        );
        limits.insert(
            (Free, Auth),
            TierLimits {
                max_requests: 5,
                window: Duration::from_secs(60),
                burst: 1,
            },
        );
        limits.insert(
            (Free, Admin),
            TierLimits {
                max_requests: 0,
                window: Duration::from_secs(60),
                burst: 0,
            },
        );
        limits.insert(
            (Free, Other),
            TierLimits {
                max_requests: 60,
                window: Duration::from_secs(60),
                burst: 10,
            },
        );

        // Pro tier
        limits.insert(
            (Pro, Chat),
            TierLimits {
                max_requests: 120,
                window: Duration::from_secs(60),
                burst: 20,
            },
        );
        limits.insert(
            (Pro, ToolResult),
            TierLimits {
                max_requests: 60,
                window: Duration::from_secs(60),
                burst: 10,
            },
        );
        limits.insert(
            (Pro, Plugin),
            TierLimits {
                max_requests: 60,
                window: Duration::from_secs(60),
                burst: 10,
            },
        );
        limits.insert(
            (Pro, Debug),
            TierLimits {
                max_requests: 30,
                window: Duration::from_secs(60),
                burst: 5,
            },
        );
        limits.insert(
            (Pro, Auth),
            TierLimits {
                max_requests: 10,
                window: Duration::from_secs(60),
                burst: 2,
            },
        );
        limits.insert(
            (Pro, Admin),
            TierLimits {
                max_requests: 0,
                window: Duration::from_secs(60),
                burst: 0,
            },
        );
        limits.insert(
            (Pro, Other),
            TierLimits {
                max_requests: 120,
                window: Duration::from_secs(60),
                burst: 20,
            },
        );

        // Enterprise tier — unlimited (very high limits)
        let unlimited = TierLimits {
            max_requests: 10_000,
            window: Duration::from_secs(60),
            burst: 1_000,
        };
        limits.insert((Enterprise, Chat), unlimited);
        limits.insert((Enterprise, ToolResult), unlimited);
        limits.insert((Enterprise, Plugin), unlimited);
        limits.insert((Enterprise, Debug), unlimited);
        limits.insert((Enterprise, Auth), unlimited);
        limits.insert((Enterprise, Admin), unlimited);
        limits.insert((Enterprise, Other), unlimited);

        Self { limits }
    }
}

impl RateLimiterConfig {
    /// Get the limits for a specific tier and category.
    pub fn get_limits(&self, tier: RateLimitTier, category: EndpointCategory) -> TierLimits {
        self.limits
            .get(&(tier, category))
            .copied()
            .unwrap_or(TierLimits {
                max_requests: 60,
                window: Duration::from_secs(60),
                burst: 10,
            })
    }
}

/// Token bucket state for a single user+category pair.
#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
    max_tokens: f64,
    refill_rate: f64, // tokens per second
}

impl Bucket {
    fn new(limits: &TierLimits) -> Self {
        let max_tokens = (limits.max_requests + limits.burst) as f64;
        let refill_rate = limits.max_requests as f64 / limits.window.as_secs_f64();
        Self {
            tokens: max_tokens,
            last_refill: Instant::now(),
            max_tokens,
            refill_rate,
        }
    }

    fn try_consume(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
    }

    fn remaining(&mut self) -> u32 {
        self.refill();
        self.tokens.max(0.0) as u32
    }

    fn retry_after_secs(&self) -> u32 {
        if self.refill_rate <= 0.0 {
            return 60;
        }
        ((1.0 - self.tokens) / self.refill_rate).ceil().max(1.0) as u32
    }
}

/// Composite key for the bucket map.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BucketKey {
    user_id: uuid::Uuid,
    category: EndpointCategory,
}

/// Rate limiter result.
#[derive(Debug)]
pub struct RateLimitResult {
    /// Whether the request is allowed.
    pub allowed: bool,
    /// Remaining requests in the current window.
    pub remaining: u32,
    /// Maximum requests per window.
    pub limit: u32,
    /// Seconds until the bucket refills (if rate limited).
    pub retry_after: Option<u32>,
}

/// Token bucket rate limiter.
///
/// Thread-safe via DashMap. Supports per-user, per-endpoint-category limits.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    buckets: Arc<DashMap<BucketKey, Bucket>>,
    config: Arc<RateLimiterConfig>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            buckets: Arc::new(DashMap::new()),
            config: Arc::new(config),
        }
    }

    /// Check and consume a rate limit token.
    ///
    /// Returns the rate limit result with headers information.
    pub fn check(
        &self,
        user_id: uuid::Uuid,
        category: EndpointCategory,
        tier: RateLimitTier,
    ) -> RateLimitResult {
        let limits = self.config.get_limits(tier, category);
        let key = BucketKey { user_id, category };

        let mut entry = self
            .buckets
            .entry(key)
            .or_insert_with(|| Bucket::new(&limits));
        let bucket = entry.value_mut();

        if bucket.try_consume() {
            RateLimitResult {
                allowed: true,
                remaining: bucket.remaining(),
                limit: limits.max_requests,
                retry_after: None,
            }
        } else {
            RateLimitResult {
                allowed: false,
                remaining: 0,
                limit: limits.max_requests,
                retry_after: Some(bucket.retry_after_secs()),
            }
        }
    }

    /// Evict expired/idle buckets to prevent unbounded memory growth.
    /// Call periodically (e.g., every 5 minutes).
    pub fn evict_idle(&self, max_idle: Duration) -> usize {
        let now = Instant::now();
        let idle_keys: Vec<BucketKey> = self
            .buckets
            .iter()
            .filter(|entry| now.duration_since(entry.value().last_refill) > max_idle)
            .map(|entry| entry.key().clone())
            .collect();
        let count = idle_keys.len();
        for key in idle_keys {
            self.buckets.remove(&key);
        }
        count
    }

    /// Get the number of active buckets.
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RateLimiterConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_free_tier_chat_limit() {
        let limiter = RateLimiter::default();
        let user = Uuid::new_v4();

        // Free tier: 30/min + 5 burst = 35 total
        for i in 0..35 {
            let result = limiter.check(user, EndpointCategory::Chat, RateLimitTier::Free);
            assert!(result.allowed, "Request {} should be allowed", i);
        }

        // 36th request should be rejected
        let result = limiter.check(user, EndpointCategory::Chat, RateLimitTier::Free);
        assert!(!result.allowed);
        assert!(result.retry_after.is_some());
    }

    #[test]
    fn test_enterprise_unlimited() {
        let limiter = RateLimiter::default();
        let user = Uuid::new_v4();

        for _ in 0..1000 {
            let result = limiter.check(user, EndpointCategory::Chat, RateLimitTier::Enterprise);
            assert!(result.allowed);
        }
    }

    #[test]
    fn test_different_users_isolated() {
        let limiter = RateLimiter::default();
        let user1 = Uuid::new_v4();
        let user2 = Uuid::new_v4();

        // Exhaust user1's chat limit
        for _ in 0..35 {
            limiter.check(user1, EndpointCategory::Chat, RateLimitTier::Free);
        }
        let r1 = limiter.check(user1, EndpointCategory::Chat, RateLimitTier::Free);
        assert!(!r1.allowed);

        // user2 should still be allowed
        let r2 = limiter.check(user2, EndpointCategory::Chat, RateLimitTier::Free);
        assert!(r2.allowed);
    }

    #[test]
    fn test_different_categories_isolated() {
        let limiter = RateLimiter::default();
        let user = Uuid::new_v4();

        // Exhaust chat limit
        for _ in 0..35 {
            limiter.check(user, EndpointCategory::Chat, RateLimitTier::Free);
        }
        assert!(
            !limiter
                .check(user, EndpointCategory::Chat, RateLimitTier::Free)
                .allowed
        );

        // Tool endpoint should still work
        assert!(
            limiter
                .check(user, EndpointCategory::ToolResult, RateLimitTier::Free)
                .allowed
        );
    }

    #[test]
    fn test_endpoint_category_from_path() {
        assert_eq!(
            EndpointCategory::from_path("/api/chat/stream"),
            EndpointCategory::Chat
        );
        assert_eq!(
            EndpointCategory::from_path("/api/tools/result"),
            EndpointCategory::ToolResult
        );
        assert_eq!(
            EndpointCategory::from_path("/api/plugins/catalog"),
            EndpointCategory::Plugin
        );
        assert_eq!(
            EndpointCategory::from_path("/api/connectors/list"),
            EndpointCategory::Plugin
        );
        assert_eq!(
            EndpointCategory::from_path("/api/bundles/activate"),
            EndpointCategory::Plugin
        );
        assert_eq!(
            EndpointCategory::from_path("/api/debug/executions"),
            EndpointCategory::Debug
        );
        assert_eq!(
            EndpointCategory::from_path("/api/auth/login"),
            EndpointCategory::Auth
        );
        assert_eq!(
            EndpointCategory::from_path("/api/settings"),
            EndpointCategory::Other
        );
    }

    #[test]
    fn test_evict_idle() {
        let limiter = RateLimiter::default();
        let user = Uuid::new_v4();

        limiter.check(user, EndpointCategory::Chat, RateLimitTier::Free);
        assert_eq!(limiter.bucket_count(), 1);

        // Won't evict with long idle threshold
        let evicted = limiter.evict_idle(Duration::from_secs(3600));
        assert_eq!(evicted, 0);
    }

    #[test]
    fn test_rate_limit_result_headers() {
        let limiter = RateLimiter::default();
        let user = Uuid::new_v4();

        let result = limiter.check(user, EndpointCategory::Chat, RateLimitTier::Free);
        assert!(result.allowed);
        assert_eq!(result.limit, 30);
        assert!(result.remaining > 0);
        assert!(result.retry_after.is_none());
    }
}
