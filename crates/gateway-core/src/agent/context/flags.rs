//! Feature flags for Context Engineering v2 rollout.
//!
//! Provides per-user sticky canary flags to gradually roll out
//! new context pipeline features.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Feature flags for context engineering v2 rollout.
///
/// Uses per-user sticky canary: the same user_id always gets the
/// same flag result for a given rollout percentage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextResolverFlags {
    /// Percentage of users that get the new scoring pipeline (0-100).
    pub scoring_rollout_pct: u8,
    /// Whether to dual-write to both old and new memory backends.
    pub memory_dual_write: bool,
    /// Whether to inject knowledge from learning system into prompts.
    pub knowledge_injection: bool,
    /// Whether to enable prompt inspection API.
    pub prompt_inspection: bool,
    /// Whether to enable conversation tracing.
    pub conversation_tracing: bool,
    /// Additional boolean flags.
    #[serde(default)]
    pub flags: HashMap<String, bool>,
}

impl Default for ContextResolverFlags {
    fn default() -> Self {
        Self {
            scoring_rollout_pct: 0,
            memory_dual_write: false,
            knowledge_injection: true,
            prompt_inspection: true,
            conversation_tracing: false,
            flags: HashMap::new(),
        }
    }
}

impl ContextResolverFlags {
    /// Check if a user should use the new scoring pipeline.
    ///
    /// Uses a deterministic hash of the user_id to produce a sticky
    /// assignment: the same user_id always returns the same result
    /// for a given rollout percentage.
    ///
    /// # Example
    ///
    /// ```
    /// use gateway_core::agent::context::flags::ContextResolverFlags;
    /// use uuid::Uuid;
    ///
    /// let flags = ContextResolverFlags {
    ///     scoring_rollout_pct: 50,
    ///     ..Default::default()
    /// };
    ///
    /// let user = Uuid::new_v4();
    /// // Same user always gets the same result
    /// let result1 = flags.should_use_new_pipeline(&user);
    /// let result2 = flags.should_use_new_pipeline(&user);
    /// assert_eq!(result1, result2);
    /// ```
    pub fn should_use_new_pipeline(&self, user_id: &Uuid) -> bool {
        if self.scoring_rollout_pct == 0 {
            return false;
        }
        if self.scoring_rollout_pct >= 100 {
            return true;
        }
        let hash = user_id_hash(user_id);
        (hash % 100) < self.scoring_rollout_pct as u64
    }

    /// Check if a named flag is enabled.
    pub fn is_flag_enabled(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }

    /// Load flags from a YAML configuration file.
    pub fn from_yaml(path: &std::path::Path) -> Result<Self, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {}", e))?;
        let value: serde_yaml::Value =
            serde_yaml::from_str(&content).map_err(|e| format!("Failed to parse YAML: {}", e))?;

        // Extract feature_flags section
        let flags_value = value
            .get("feature_flags")
            .ok_or_else(|| "Missing 'feature_flags' section".to_string())?;

        serde_yaml::from_value(flags_value.clone())
            .map_err(|e| format!("Failed to parse feature_flags: {}", e))
    }
}

/// Deterministic hash of a user_id for sticky canary assignment.
fn user_id_hash(user_id: &Uuid) -> u64 {
    user_id
        .as_bytes()
        .iter()
        .fold(0u64, |acc, &b| acc.wrapping_mul(31).wrapping_add(b as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_flags() {
        let flags = ContextResolverFlags::default();
        assert_eq!(flags.scoring_rollout_pct, 0);
        assert!(!flags.memory_dual_write);
        assert!(flags.knowledge_injection);
        assert!(flags.prompt_inspection);
        assert!(!flags.conversation_tracing);
    }

    #[test]
    fn test_sticky_canary_zero() {
        let flags = ContextResolverFlags {
            scoring_rollout_pct: 0,
            ..Default::default()
        };
        // 0% means no one gets it
        for _ in 0..100 {
            assert!(!flags.should_use_new_pipeline(&Uuid::new_v4()));
        }
    }

    #[test]
    fn test_sticky_canary_hundred() {
        let flags = ContextResolverFlags {
            scoring_rollout_pct: 100,
            ..Default::default()
        };
        // 100% means everyone gets it
        for _ in 0..100 {
            assert!(flags.should_use_new_pipeline(&Uuid::new_v4()));
        }
    }

    #[test]
    fn test_sticky_canary_deterministic() {
        let flags = ContextResolverFlags {
            scoring_rollout_pct: 50,
            ..Default::default()
        };
        let user = Uuid::new_v4();
        let result1 = flags.should_use_new_pipeline(&user);
        let result2 = flags.should_use_new_pipeline(&user);
        assert_eq!(result1, result2, "Same user must always get same result");
    }

    #[test]
    fn test_sticky_canary_distribution() {
        let flags = ContextResolverFlags {
            scoring_rollout_pct: 50,
            ..Default::default()
        };

        let mut enabled_count = 0;
        let total = 1000;

        for _ in 0..total {
            if flags.should_use_new_pipeline(&Uuid::new_v4()) {
                enabled_count += 1;
            }
        }

        // Should be roughly 50% (with tolerance for randomness)
        let pct = (enabled_count as f64 / total as f64) * 100.0;
        assert!(pct > 30.0 && pct < 70.0, "Expected ~50%, got {:.1}%", pct);
    }

    #[test]
    fn test_is_flag_enabled() {
        let mut flags = ContextResolverFlags::default();
        flags.flags.insert("my_flag".to_string(), true);

        assert!(flags.is_flag_enabled("my_flag"));
        assert!(!flags.is_flag_enabled("nonexistent"));
    }

    #[test]
    fn test_user_id_hash_deterministic() {
        let id = Uuid::parse_str("a1a2a3a4-b1b2-c1c2-d1d2-e1e2e3e4e5e6").unwrap();
        let h1 = user_id_hash(&id);
        let h2 = user_id_hash(&id);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_serde_round_trip() {
        let flags = ContextResolverFlags {
            scoring_rollout_pct: 42,
            memory_dual_write: true,
            knowledge_injection: true,
            prompt_inspection: false,
            conversation_tracing: true,
            flags: {
                let mut m = HashMap::new();
                m.insert("custom".to_string(), true);
                m
            },
        };
        let json = serde_json::to_string(&flags).unwrap();
        let parsed: ContextResolverFlags = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.scoring_rollout_pct, 42);
        assert!(parsed.memory_dual_write);
        assert!(parsed.is_flag_enabled("custom"));
    }
}
