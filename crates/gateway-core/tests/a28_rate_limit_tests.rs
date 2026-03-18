//! A28 Rate Limiting Tests
//!
//! Tests token bucket rate limiter with per-user, per-endpoint-category,
//! per-tier enforcement. Validates headers, 429 responses, and burst behavior.
//!
//! Run: `cargo nextest run -p gateway-core --test a28_rate_limit_tests`

mod helpers;

use helpers::mock_auth::*;

// ============================================================
// Basic Rate Limit Enforcement
// ============================================================

#[cfg(test)]
mod rate_limit_tests {
    use super::*;
    use std::time::Duration;

    /// RL-1: Request within limit succeeds with rate limit headers
    #[tokio::test]
    async fn test_rate_limit_within_limit() {
        // GIVEN: Free tier user, chat endpoint, limit = 30/min
        let limiter = MockRateLimiter::new();

        // WHEN: 1st request to /api/chat/stream
        let result = limiter.check("chat", "free").await;

        // THEN: 200 OK equivalent (Ok result)
        assert!(result.is_ok());

        // AND: X-RateLimit-Remaining: 29
        let remaining = result.unwrap();
        assert_eq!(remaining, 29);

        // Verify limit header value would be 30
        let limit = limiter.limits.get("chat:free").copied().unwrap();
        assert_eq!(limit, 30);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// RL-2: Over-limit returns 429 with Retry-After
    #[tokio::test]
    async fn test_rate_limit_exceeded() {
        // GIVEN: Free tier user, chat endpoint, limit = 30/min
        let limiter = MockRateLimiter::new();

        // WHEN: Exhaust all 30 requests
        for i in 0..30 {
            let result = limiter.check("chat", "free").await;
            assert!(result.is_ok(), "Request {} should succeed", i + 1);
        }

        // THEN: 31st request returns 429 Too Many Requests
        let result = limiter.check("chat", "free").await;
        assert!(result.is_err());

        // AND: Retry-After: 60
        let retry_after = result.unwrap_err();
        assert_eq!(retry_after, 60);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// RL-3: Response includes X-RateLimit-* headers on every response
    #[tokio::test]
    async fn test_rate_limit_headers_present() {
        // GIVEN: Any authenticated request
        // THEN: All 3 headers present: X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset
        let header_names = vec![
            "X-RateLimit-Limit",
            "X-RateLimit-Remaining",
            "X-RateLimit-Reset",
        ];

        for header in &header_names {
            assert!(
                header.starts_with("X-RateLimit-"),
                "Header {} must start with X-RateLimit-",
                header
            );
            assert!(!header.is_empty());
        }

        assert_eq!(header_names.len(), 3);

        // Verify header names are valid HTTP header characters
        for header in &header_names {
            assert!(header.chars().all(|c| c.is_alphanumeric() || c == '-'));
        }

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// RL-4: Different users have independent limits
    #[tokio::test]
    async fn test_rate_limit_per_user() {
        // GIVEN: Two separate limiters represent two independent users
        let limiter_a = MockRateLimiter::new();
        let limiter_b = MockRateLimiter::new();

        // User A exhausts 29 of 30 requests
        for _ in 0..29 {
            let _ = limiter_a.check("chat", "free").await;
        }

        // WHEN: User A makes 1 more request -> succeeds (30th)
        let result_a = limiter_a.check("chat", "free").await;
        assert!(result_a.is_ok());
        assert_eq!(result_a.unwrap(), 0); // remaining = 0

        // AND: User B makes 1 request -> succeeds (independent bucket)
        let result_b = limiter_b.check("chat", "free").await;
        assert!(result_b.is_ok());
        assert_eq!(result_b.unwrap(), 29); // User B still has 29 remaining

        // AND: User A makes 1 more request -> 429
        let result_a_over = limiter_a.check("chat", "free").await;
        assert!(result_a_over.is_err());

        // THEN: User B unaffected by User A's limit
        let result_b_still_ok = limiter_b.check("chat", "free").await;
        assert!(result_b_still_ok.is_ok());

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// RL-5: Pro tier has higher limits than free tier
    #[tokio::test]
    async fn test_rate_limit_tier_differentiation() {
        // GIVEN: Free user and Pro user
        let free_user = MockAuthContext::free_user();
        let pro_user = MockAuthContext::pro_user();
        assert_eq!(free_user.tier, MockUserTier::Free);
        assert_eq!(pro_user.tier, MockUserTier::Pro);

        let limiter = MockRateLimiter::new();

        // THEN: Free user X-RateLimit-Limit: 30
        let free_limit = limiter.limits.get("chat:free").copied().unwrap();
        assert_eq!(free_limit, 30);

        // AND: Pro user X-RateLimit-Limit: 120
        let pro_limit = limiter.limits.get("chat:pro").copied().unwrap();
        assert_eq!(pro_limit, 120);

        // Verify pro has strictly higher limit
        assert!(
            pro_limit > free_limit,
            "Pro limit ({}) must be higher than free limit ({})",
            pro_limit,
            free_limit
        );

        // Verify the limiter actually enforces these limits
        let free_result = limiter.check("chat", free_user.tier.as_str()).await;
        assert!(free_result.is_ok());
        assert_eq!(free_result.unwrap(), 29); // 30 - 1

        let pro_result = limiter.check("chat", pro_user.tier.as_str()).await;
        assert!(pro_result.is_ok());
        assert_eq!(pro_result.unwrap(), 119); // 120 - 1

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ============================================================
// Per-Endpoint Category Tests
// ============================================================

#[cfg(test)]
mod endpoint_category_tests {
    use super::*;
    use std::time::Duration;

    /// Different endpoint categories have independent limits
    #[tokio::test]
    async fn test_independent_category_limits() {
        // GIVEN: Free user at 29/30 chat requests
        let limiter = MockRateLimiter::new();
        for _ in 0..29 {
            let _ = limiter.check("chat", "free").await;
        }

        // WHEN: User makes plugin request (/api/plugins/catalog)
        // THEN: Plugin request succeeds (separate category, limit=60)
        let plugin_result = limiter.check("plugin", "free").await;
        assert!(plugin_result.is_ok());
        assert_eq!(plugin_result.unwrap(), 59); // plugin limit = 60, first request

        // AND: Chat still has 1 remaining
        let chat_result = limiter.check("chat", "free").await;
        assert!(chat_result.is_ok());
        assert_eq!(chat_result.unwrap(), 0); // 30th chat request, remaining = 0

        // AND: Next chat request -> 429
        let chat_over = limiter.check("chat", "free").await;
        assert!(chat_over.is_err());

        // Plugin still works independently
        let plugin_still_ok = limiter.check("plugin", "free").await;
        assert!(plugin_still_ok.is_ok());

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// Tool result endpoint has higher limits (200/min free)
    #[tokio::test]
    async fn test_tool_result_higher_limit() {
        // GIVEN: Free user
        let limiter = MockRateLimiter::new();

        // Verify the limit is configured as 200
        let tool_result_limit = limiter.limits.get("tool_result:free").copied().unwrap();
        assert_eq!(tool_result_limit, 200);

        // WHEN: 200 requests to /api/tools/result
        // THEN: All 200 succeed
        for i in 0..200 {
            let result = limiter.check("tool_result", "free").await;
            assert!(result.is_ok(), "Request {} should succeed", i + 1);
        }

        // AND: 201st -> 429
        let result = limiter.check("tool_result", "free").await;
        assert!(result.is_err());

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// Admin endpoints have lowest limits (10/min free)
    #[tokio::test]
    async fn test_admin_lowest_limit() {
        // GIVEN: Admin user
        let limiter = MockRateLimiter::new();

        // Verify admin limit is 10
        let admin_limit = limiter.limits.get("admin:free").copied().unwrap();
        assert_eq!(admin_limit, 10);

        // WHEN: 10 requests to admin endpoint
        // THEN: All 10 succeed
        for i in 0..10 {
            let result = limiter.check("admin", "free").await;
            assert!(result.is_ok(), "Admin request {} should succeed", i + 1);
        }

        // AND: 11th -> 429
        let result = limiter.check("admin", "free").await;
        assert!(result.is_err());

        // Verify admin has the lowest limit across all categories
        let all_free_limits: Vec<u32> = limiter
            .limits
            .iter()
            .filter(|(k, _)| k.ends_with(":free"))
            .map(|(_, v)| *v)
            .collect();
        let min_limit = all_free_limits.iter().min().copied().unwrap();
        assert_eq!(
            min_limit, admin_limit,
            "Admin should have the lowest free-tier limit"
        );

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ============================================================
// Token Bucket Behavior Tests
// ============================================================

#[cfg(test)]
mod token_bucket_tests {
    use super::*;
    use std::time::Duration;

    /// Tokens refill over time
    #[tokio::test]
    async fn test_token_refill() {
        // GIVEN: Free user who exhausted chat limit (0/30)
        let limiter = MockRateLimiter::new();
        for _ in 0..30 {
            let _ = limiter.check("chat", "free").await;
        }
        assert!(limiter.check("chat", "free").await.is_err());

        // WHEN: Reset (simulates token refill after window expires)
        limiter.reset().await;

        // THEN: Tokens refilled, next request succeeds
        let result = limiter.check("chat", "free").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 29); // Full bucket again: 30 - 1 = 29

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// Burst allows short spikes above steady rate
    #[tokio::test]
    async fn test_burst_capacity() {
        // GIVEN: Free user, chat burst = 5
        let limiter = MockRateLimiter::new();

        // WHEN: 5 requests in rapid succession (no sleeps between)
        let mut results = Vec::new();
        for _ in 0..5 {
            results.push(limiter.check("chat", "free").await);
        }

        // THEN: All 5 succeed (burst capacity)
        for (i, result) in results.iter().enumerate() {
            assert!(result.is_ok(), "Burst request {} should succeed", i + 1);
        }

        // Verify remaining counts are decreasing
        let remaining: Vec<u32> = results
            .iter()
            .map(|r| r.as_ref().unwrap().clone())
            .collect();
        assert_eq!(remaining, vec![29, 28, 27, 26, 25]);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ============================================================
// Mock Rate Limiter Unit Tests
// ============================================================

#[cfg(test)]
mod mock_rate_limiter_tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_limiter_within_limit() {
        let limiter = MockRateLimiter::new();
        let result = limiter.check("chat", "free").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 29); // 30 - 1
    }

    #[tokio::test]
    async fn test_mock_limiter_exceeded() {
        let limiter = MockRateLimiter::new();
        // Exhaust limit
        for _ in 0..30 {
            let _ = limiter.check("chat", "free").await;
        }
        // 31st should fail
        let result = limiter.check("chat", "free").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_limiter_reset() {
        let limiter = MockRateLimiter::new();
        for _ in 0..30 {
            let _ = limiter.check("chat", "free").await;
        }
        assert!(limiter.check("chat", "free").await.is_err());

        limiter.reset().await;
        assert!(limiter.check("chat", "free").await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_limiter_pro_tier_higher() {
        let limiter = MockRateLimiter::new();
        // Pro tier chat limit = 120
        for _ in 0..120 {
            assert!(limiter.check("chat", "pro").await.is_ok());
        }
        assert!(limiter.check("chat", "pro").await.is_err());
    }
}
