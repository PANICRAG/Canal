//! Reflection Store for structured step execution reflections (A39).
//!
//! Stores [`StepReflection`] entries keyed by step action hash, enabling
//! the Judge to reference past evaluations for similar steps.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Verdict from the StepJudge evaluating a step execution result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepVerdict {
    /// Step completed successfully with expected output.
    Pass,
    /// Step produced partial results — may be acceptable.
    PartialPass,
    /// Step clearly failed — needs retry or replan.
    Fail,
    /// Step produced identical output on retry — escalate to replan.
    Stalled,
}

impl std::fmt::Display for StepVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepVerdict::Pass => write!(f, "pass"),
            StepVerdict::PartialPass => write!(f, "partial_pass"),
            StepVerdict::Fail => write!(f, "fail"),
            StepVerdict::Stalled => write!(f, "stalled"),
        }
    }
}

/// A structured reflection on a single step execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReflection {
    /// The action that was attempted.
    pub step_action: String,
    /// Tool category used (browser, file, shell, code, llm, search).
    pub tool_category: String,
    /// What was expected.
    pub expected_output: Option<String>,
    /// What actually happened.
    pub actual_output: String,
    /// Judge's verdict.
    pub verdict: StepVerdict,
    /// Explanation of why this verdict was chosen.
    pub reasoning: String,
    /// Actionable suggestions for retry or replan.
    pub suggestions: Vec<String>,
}

/// In-memory store for structured step reflections.
///
/// Keyed by a normalized hash of the step action, allowing the Judge
/// to reference past evaluations for similar steps within the same session.
pub struct ReflectionStore {
    reflections: Arc<RwLock<HashMap<String, Vec<StepReflection>>>>,
    max_per_action: usize,
    max_total: usize,
}

impl ReflectionStore {
    /// Create a new reflection store with default limits.
    pub fn new() -> Self {
        Self {
            reflections: Arc::new(RwLock::new(HashMap::new())),
            max_per_action: 10,
            max_total: 5_000,
        }
    }

    /// Create a store with custom limits.
    pub fn with_limits(max_per_action: usize, max_total: usize) -> Self {
        Self {
            reflections: Arc::new(RwLock::new(HashMap::new())),
            max_per_action,
            max_total,
        }
    }

    /// Store a reflection, keyed by a normalized action hash.
    pub async fn store(&self, reflection: StepReflection) {
        let key = Self::action_key(&reflection.step_action);
        let mut map = self.reflections.write().await;

        // Check total limit
        let total: usize = map.values().map(|v| v.len()).sum();
        if total >= self.max_total {
            // Evict oldest entries from the largest bucket
            if let Some(largest_key) = map
                .iter()
                .max_by_key(|(_, v)| v.len())
                .map(|(k, _)| k.clone())
            {
                if let Some(bucket) = map.get_mut(&largest_key) {
                    bucket.drain(..bucket.len() / 2);
                }
            }
        }

        let entry = map.entry(key).or_insert_with(Vec::new);
        if entry.len() >= self.max_per_action {
            entry.remove(0); // Remove oldest
        }
        entry.push(reflection);
    }

    /// Query reflections for similar actions.
    ///
    /// Uses simple keyword matching on the action string.
    pub async fn query_similar(&self, action: &str, limit: usize) -> Vec<StepReflection> {
        let map = self.reflections.read().await;
        let action_lower = action.to_lowercase();
        let action_words: Vec<&str> = action_lower.split_whitespace().collect();

        let mut results: Vec<(usize, &StepReflection)> = Vec::new();

        for reflections in map.values() {
            for r in reflections {
                let r_lower = r.step_action.to_lowercase();
                let score: usize = action_words.iter().filter(|w| r_lower.contains(*w)).count();
                if score > 0 {
                    results.push((score, r));
                }
            }
        }

        results.sort_by(|a, b| b.0.cmp(&a.0));
        results
            .into_iter()
            .take(limit)
            .map(|(_, r)| r.clone())
            .collect()
    }

    /// Clear all stored reflections.
    pub async fn clear(&self) {
        let mut map = self.reflections.write().await;
        map.clear();
    }

    /// Get total number of stored reflections.
    pub async fn len(&self) -> usize {
        let map = self.reflections.read().await;
        map.values().map(|v| v.len()).sum()
    }

    /// Check if the store is empty.
    pub async fn is_empty(&self) -> bool {
        let map = self.reflections.read().await;
        map.is_empty()
    }

    /// Normalize an action string to a lookup key.
    fn action_key(action: &str) -> String {
        action
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("_")
    }
}

impl Default for ReflectionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_reflection(action: &str, verdict: StepVerdict) -> StepReflection {
        StepReflection {
            step_action: action.into(),
            tool_category: "browser".into(),
            expected_output: Some("expected".into()),
            actual_output: "actual".into(),
            verdict,
            reasoning: "test reasoning".into(),
            suggestions: vec!["try again".into()],
        }
    }

    #[tokio::test]
    async fn test_store_and_query() {
        let store = ReflectionStore::new();
        store
            .store(test_reflection("Click submit button", StepVerdict::Fail))
            .await;
        store
            .store(test_reflection("Click cancel button", StepVerdict::Pass))
            .await;

        let results = store.query_similar("Click submit", 5).await;
        assert!(!results.is_empty());
        // Both reflections contain "click" and "button"
        assert!(results.len() <= 2);
    }

    #[tokio::test]
    async fn test_max_per_action_eviction() {
        let store = ReflectionStore::with_limits(2, 100);
        let action = "Open Gmail";

        store
            .store(test_reflection(action, StepVerdict::Fail))
            .await;
        store
            .store(test_reflection(action, StepVerdict::Fail))
            .await;
        store
            .store(test_reflection(action, StepVerdict::Pass))
            .await;

        // Only 2 should remain (max_per_action = 2)
        assert_eq!(store.len().await, 2);
    }

    #[tokio::test]
    async fn test_clear() {
        let store = ReflectionStore::new();
        store
            .store(test_reflection("action", StepVerdict::Pass))
            .await;
        assert!(!store.is_empty().await);

        store.clear().await;
        assert!(store.is_empty().await);
    }

    #[tokio::test]
    async fn test_query_no_match() {
        let store = ReflectionStore::new();
        store
            .store(test_reflection("Open Gmail inbox", StepVerdict::Pass))
            .await;

        let results = store.query_similar("Deploy kubernetes", 5).await;
        assert!(results.is_empty());
    }

    #[test]
    fn test_verdict_display() {
        assert_eq!(StepVerdict::Pass.to_string(), "pass");
        assert_eq!(StepVerdict::Fail.to_string(), "fail");
        assert_eq!(StepVerdict::Stalled.to_string(), "stalled");
        assert_eq!(StepVerdict::PartialPass.to_string(), "partial_pass");
    }

    #[test]
    fn test_verdict_serde_roundtrip() {
        for verdict in [
            StepVerdict::Pass,
            StepVerdict::PartialPass,
            StepVerdict::Fail,
            StepVerdict::Stalled,
        ] {
            let json = serde_json::to_string(&verdict).unwrap();
            let parsed: StepVerdict = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, verdict);
        }
    }

    #[test]
    fn test_reflection_serde_roundtrip() {
        let r = test_reflection("test action", StepVerdict::Fail);
        let json = serde_json::to_string(&r).unwrap();
        let parsed: StepReflection = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.step_action, "test action");
        assert_eq!(parsed.verdict, StepVerdict::Fail);
        assert_eq!(parsed.suggestions.len(), 1);
    }
}
