//! Mock Authentication for A28 Tests
//!
//! Provides mock Supabase auth, JWT generation, and auth context
//! creation for testing security hardening and RTE protocol.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// Mock Supabase JWT claims
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MockSupabaseClaims {
    pub sub: String,
    pub email: String,
    pub role: String,
    pub tier: String,
    pub exp: u64,
    pub iat: u64,
    pub aud: String,
}

/// User tier for rate limiting tests
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MockUserTier {
    Free,
    Pro,
    Enterprise,
}

impl MockUserTier {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Free => "free",
            Self::Pro => "pro",
            Self::Enterprise => "enterprise",
        }
    }
}

/// Mock auth context for testing
#[derive(Debug, Clone)]
pub struct MockAuthContext {
    pub user_id: String,
    pub email: String,
    pub role: String,
    pub tier: MockUserTier,
    pub permissions: Vec<String>,
}

impl MockAuthContext {
    /// Create a free-tier user
    pub fn free_user() -> Self {
        Self {
            user_id: "user-free-001".to_string(),
            email: "free@test.com".to_string(),
            role: "user".to_string(),
            tier: MockUserTier::Free,
            permissions: vec!["chat".to_string(), "read".to_string(), "write".to_string()],
        }
    }

    /// Create a pro-tier user
    pub fn pro_user() -> Self {
        Self {
            user_id: "user-pro-001".to_string(),
            email: "pro@test.com".to_string(),
            role: "user".to_string(),
            tier: MockUserTier::Pro,
            permissions: vec![
                "chat".to_string(),
                "read".to_string(),
                "write".to_string(),
                "tool_execute".to_string(),
            ],
        }
    }

    /// Create an enterprise-tier user
    pub fn enterprise_user() -> Self {
        Self {
            user_id: "user-ent-001".to_string(),
            email: "enterprise@test.com".to_string(),
            role: "user".to_string(),
            tier: MockUserTier::Enterprise,
            permissions: vec![
                "chat".to_string(),
                "read".to_string(),
                "write".to_string(),
                "tool_execute".to_string(),
                "admin_read".to_string(),
            ],
        }
    }

    /// Create an admin user
    pub fn admin_user() -> Self {
        Self {
            user_id: "user-admin-001".to_string(),
            email: "admin@test.com".to_string(),
            role: "admin".to_string(),
            tier: MockUserTier::Enterprise,
            permissions: vec!["*".to_string()],
        }
    }

    /// Create a user with no permissions (for rejection tests)
    pub fn no_perms_user() -> Self {
        Self {
            user_id: "user-noperm-001".to_string(),
            email: "noperm@test.com".to_string(),
            role: "user".to_string(),
            tier: MockUserTier::Free,
            permissions: vec![],
        }
    }

    /// Generate a mock JWT token string (not cryptographically valid)
    pub fn to_mock_jwt(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = MockSupabaseClaims {
            sub: self.user_id.clone(),
            email: self.email.clone(),
            role: self.role.clone(),
            tier: self.tier.as_str().to_string(),
            exp: now + 3600,
            iat: now,
            aud: "authenticated".to_string(),
        };
        // Simple base64 encoding for test purposes (NOT valid JWT)
        let header = base64_encode(r#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = base64_encode(&serde_json::to_string(&claims).unwrap());
        let signature = base64_encode("mock-signature");
        format!("{}.{}.{}", header, payload, signature)
    }

    /// Generate an expired JWT for testing
    pub fn to_expired_jwt(&self) -> String {
        let claims = MockSupabaseClaims {
            sub: self.user_id.clone(),
            email: self.email.clone(),
            role: self.role.clone(),
            tier: self.tier.as_str().to_string(),
            exp: 1000000000, // year 2001 — expired
            iat: 999999000,
            aud: "authenticated".to_string(),
        };
        let header = base64_encode(r#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = base64_encode(&serde_json::to_string(&claims).unwrap());
        let signature = base64_encode("mock-signature");
        format!("{}.{}.{}", header, payload, signature)
    }
}

/// Mock rate limiter for testing rate limit behavior
#[derive(Debug)]
pub struct MockRateLimiter {
    pub call_count: Arc<RwLock<std::collections::HashMap<String, u32>>>,
    pub limits: std::collections::HashMap<String, u32>,
}

impl MockRateLimiter {
    pub fn new() -> Self {
        let mut limits = std::collections::HashMap::new();
        // Free tier defaults
        limits.insert("chat:free".to_string(), 30);
        limits.insert("tool_result:free".to_string(), 200);
        limits.insert("plugin:free".to_string(), 60);
        limits.insert("connector:free".to_string(), 60);
        limits.insert("debug:free".to_string(), 30);
        limits.insert("admin:free".to_string(), 10);
        // Pro tier
        limits.insert("chat:pro".to_string(), 120);
        limits.insert("tool_result:pro".to_string(), 1000);
        limits.insert("plugin:pro".to_string(), 300);
        limits.insert("connector:pro".to_string(), 300);
        limits.insert("debug:pro".to_string(), 120);
        limits.insert("admin:pro".to_string(), 30);

        Self {
            call_count: Arc::new(RwLock::new(std::collections::HashMap::new())),
            limits,
        }
    }

    /// Check rate limit, returns remaining count or error
    pub async fn check(&self, category: &str, tier: &str) -> Result<u32, u32> {
        let key = format!("{}:{}", category, tier);
        let limit = self.limits.get(&key).copied().unwrap_or(30);
        let mut counts = self.call_count.write().await;
        let count = counts.entry(key).or_insert(0);
        *count += 1;
        if *count > limit {
            Err(60) // retry_after seconds
        } else {
            Ok(limit - *count)
        }
    }

    /// Reset all counters
    pub async fn reset(&self) {
        self.call_count.write().await.clear();
    }
}

fn base64_encode(input: &str) -> String {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut encoder = base64_writer(&mut buf);
        encoder.write_all(input.as_bytes()).unwrap();
    }
    String::from_utf8(buf).unwrap_or_else(|_| "invalid".to_string())
}

fn base64_writer(w: &mut Vec<u8>) -> impl std::io::Write + '_ {
    // Simple base64 encoding for test JWTs
    struct B64Writer<'a>(&'a mut Vec<u8>);
    impl<'a> std::io::Write for B64Writer<'a> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            // URL-safe base64 without padding
            const CHARS: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            for chunk in buf.chunks(3) {
                let b0 = chunk[0] as usize;
                let b1 = if chunk.len() > 1 {
                    chunk[1] as usize
                } else {
                    0
                };
                let b2 = if chunk.len() > 2 {
                    chunk[2] as usize
                } else {
                    0
                };
                self.0.push(CHARS[(b0 >> 2) & 0x3f]);
                self.0.push(CHARS[((b0 << 4) | (b1 >> 4)) & 0x3f]);
                if chunk.len() > 1 {
                    self.0.push(CHARS[((b1 << 2) | (b2 >> 6)) & 0x3f]);
                }
                if chunk.len() > 2 {
                    self.0.push(CHARS[b2 & 0x3f]);
                }
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    B64Writer(w)
}
