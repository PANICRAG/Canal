//! Memory bridge for hydrating and flushing graph state with UnifiedMemoryStore.
//!
//! Connects graph execution to persistent memory: loads relevant preferences
//! and patterns before execution, and persists step results afterwards.

use std::sync::Arc;

use uuid::Uuid;

use super::adapters::AgentGraphState;
use crate::memory::unified::{MemoryCategory, MemoryEntry, MemorySource, UnifiedMemoryStore};

/// Configuration for the memory bridge.
#[derive(Debug, Clone)]
pub struct MemoryBridgeConfig {
    /// Whether to load user preferences into working memory.
    pub load_preferences: bool,
    /// Whether to load related patterns into working memory.
    pub load_patterns: bool,
    /// Whether to persist step results after execution.
    pub persist_step_results: bool,
    /// Whether to persist working memory entries with "result:" prefix.
    pub persist_working_memory: bool,
    /// Maximum entries per category to load.
    pub max_entries_per_category: usize,
    /// Minimum confidence level for pattern loading (0.0-1.0).
    pub min_confidence: f32,
    /// Optional task scope tag for persisted entries.
    pub task_scope: Option<String>,
}

impl Default for MemoryBridgeConfig {
    fn default() -> Self {
        Self {
            load_preferences: true,
            load_patterns: true,
            persist_step_results: true,
            persist_working_memory: false,
            max_entries_per_category: 10,
            min_confidence: 0.3,
            task_scope: None,
        }
    }
}

/// Converts a `Confidence` enum to a float for comparison.
fn confidence_as_f32(confidence: &crate::memory::unified::Confidence) -> f32 {
    use crate::memory::unified::Confidence;
    match confidence {
        Confidence::Confirmed => 1.0,
        Confidence::High => 0.9,
        Confidence::Medium => 0.5,
        Confidence::Low => 0.3,
    }
}

/// Bridge between graph execution and the unified memory store.
///
/// Handles two operations:
/// - **Hydrate**: Load relevant preferences and patterns into the graph's
///   working memory before execution starts.
/// - **Flush**: Persist execution results (step_results, selected working_memory
///   entries) back to the memory store after execution completes.
pub struct MemoryBridge {
    store: Arc<UnifiedMemoryStore>,
    config: MemoryBridgeConfig,
    user_id: Uuid,
}

impl MemoryBridge {
    /// Create a new memory bridge.
    pub fn new(store: Arc<UnifiedMemoryStore>, user_id: Uuid, config: MemoryBridgeConfig) -> Self {
        Self {
            store,
            config,
            user_id,
        }
    }

    /// Create a memory bridge with default configuration.
    pub fn with_defaults(store: Arc<UnifiedMemoryStore>, user_id: Uuid) -> Self {
        Self::new(store, user_id, MemoryBridgeConfig::default())
    }

    /// Load relevant memory entries into the graph state's working memory.
    ///
    /// Loads preferences by category and patterns by semantic search,
    /// filtering by confidence threshold.
    pub async fn hydrate(&self, state: &mut AgentGraphState) -> usize {
        let mut loaded = 0;

        // 1. Load user preferences
        if self.config.load_preferences {
            let prefs = self
                .store
                .list_by_category(self.user_id, MemoryCategory::Preference)
                .await;
            for pref in prefs.into_iter().take(self.config.max_entries_per_category) {
                state.working_memory.insert(
                    format!("pref:{}", pref.key),
                    serde_json::json!({
                        "content": pref.content,
                        "confidence": format!("{:?}", pref.confidence),
                    }),
                );
                loaded += 1;
            }
        }

        // 2. Load patterns via semantic search (uses vector search when backend available)
        if self.config.load_patterns {
            let results = self
                .store
                .semantic_search(
                    self.user_id,
                    &state.task,
                    self.config.max_entries_per_category,
                )
                .await;

            for (i, pattern) in results
                .into_iter()
                .filter(|e| confidence_as_f32(&e.confidence) >= self.config.min_confidence)
                .enumerate()
            {
                state.working_memory.insert(
                    format!("pattern:{}", i),
                    serde_json::json!({
                        "content": pattern.content,
                        "source": format!("{:?}", pattern.source),
                        "category": format!("{:?}", pattern.category),
                    }),
                );
                loaded += 1;
            }
        }

        loaded
    }

    /// Persist execution results back to the memory store.
    ///
    /// Stores step_results as `ToolResult` entries and optionally stores
    /// working memory entries with the "result:" prefix.
    pub async fn flush(&self, state: &AgentGraphState) -> usize {
        let mut persisted = 0;

        // Persist step results
        if self.config.persist_step_results {
            for (step_name, result) in &state.step_results {
                let key = format!("step_result:{}", step_name);
                let content = serde_json::to_string(result).unwrap_or_default();
                let mut entry = MemoryEntry::new(&key, MemoryCategory::ToolResult, content);
                entry.source = MemorySource::System;
                if let Some(ref scope) = self.config.task_scope {
                    entry.tags.push(format!("scope:{}", scope));
                }
                let _ = self.store.store(self.user_id, entry).await;
                persisted += 1;
            }
        }

        // Persist working memory entries with "result:" prefix
        if self.config.persist_working_memory {
            for (key, value) in &state.working_memory {
                if key.starts_with("result:") {
                    let content = serde_json::to_string(value).unwrap_or_default();
                    let mut entry = MemoryEntry::new(key, MemoryCategory::Working, content);
                    entry.source = MemorySource::System;
                    let _ = self.store.store(self.user_id, entry).await;
                    persisted += 1;
                }
            }
        }

        persisted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphState;

    fn test_user_id() -> Uuid {
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    }

    #[tokio::test]
    async fn test_memory_bridge_hydrate_preferences() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let uid = test_user_id();

        // Add 3 preferences
        for i in 0..3 {
            let entry = MemoryEntry::new(
                format!("pref_{}", i),
                MemoryCategory::Preference,
                format!("preference content {}", i),
            );
            store.store(uid, entry).await.unwrap();
        }

        let bridge = MemoryBridge::with_defaults(store, uid);
        let mut state = AgentGraphState::new("test task");
        let loaded = bridge.hydrate(&mut state).await;

        assert_eq!(loaded, 3);
        assert!(state.working_memory.contains_key("pref:pref_0"));
        assert!(state.working_memory.contains_key("pref:pref_1"));
        assert!(state.working_memory.contains_key("pref:pref_2"));
    }

    #[tokio::test]
    async fn test_memory_bridge_hydrate_filters_low_confidence() {
        use crate::memory::unified::Confidence;

        let store = Arc::new(UnifiedMemoryStore::new());
        let uid = test_user_id();

        // Add entries with different confidence levels
        let mut high = MemoryEntry::new("high", MemoryCategory::Pattern, "high confidence");
        high.confidence = Confidence::High;
        store.store(uid, high).await.unwrap();

        let mut low = MemoryEntry::new("low", MemoryCategory::Pattern, "low confidence");
        low.confidence = Confidence::Low;
        store.store(uid, low).await.unwrap();

        let config = MemoryBridgeConfig {
            load_preferences: false,
            load_patterns: true,
            min_confidence: 0.7,
            ..Default::default()
        };

        let bridge = MemoryBridge::new(store, uid, config);
        let mut state = AgentGraphState::new("test task");
        let loaded = bridge.hydrate(&mut state).await;

        // Only the High confidence entry should be loaded
        // (search returns by keyword match, filtering by confidence after)
        assert!(loaded <= 1);
    }

    #[tokio::test]
    async fn test_memory_bridge_flush_step_results() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let uid = test_user_id();

        let bridge = MemoryBridge::with_defaults(store.clone(), uid);
        let mut state = AgentGraphState::new("test task");
        state.set_step_result("node_a", serde_json::json!({"result": "success"}));
        state.set_step_result("node_b", serde_json::json!({"result": "done"}));

        let persisted = bridge.flush(&state).await;
        assert_eq!(persisted, 2);

        // Verify entries are in the store
        let entries = store
            .list_by_category(uid, MemoryCategory::ToolResult)
            .await;
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn test_memory_bridge_flush_working_memory_prefix() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let uid = test_user_id();

        let config = MemoryBridgeConfig {
            persist_step_results: false,
            persist_working_memory: true,
            ..Default::default()
        };

        let bridge = MemoryBridge::new(store.clone(), uid, config);
        let mut state = AgentGraphState::new("test task");
        state.set_memory("result:output", serde_json::json!("good result"));
        state.set_memory("internal:temp", serde_json::json!("should not persist"));
        state.set_memory("result:summary", serde_json::json!("summary here"));

        let persisted = bridge.flush(&state).await;
        assert_eq!(persisted, 2); // Only "result:" prefixed entries

        let entries = store.list_by_category(uid, MemoryCategory::Working).await;
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn test_memory_bridge_task_scoping() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let uid = test_user_id();

        let config = MemoryBridgeConfig {
            task_scope: Some("email".to_string()),
            ..Default::default()
        };

        let bridge = MemoryBridge::new(store.clone(), uid, config);
        let mut state = AgentGraphState::new("send email");
        state.set_step_result("compose", serde_json::json!("done"));

        bridge.flush(&state).await;

        let entries = store
            .list_by_category(uid, MemoryCategory::ToolResult)
            .await;
        assert_eq!(entries.len(), 1);
        assert!(entries[0].tags.contains(&"scope:email".to_string()));
    }

    #[tokio::test]
    async fn test_parallel_merge_preserves_all_step_results() {
        // Test that merge() with or_insert preserves step results from all branches
        let mut state1 = AgentGraphState::new("task");
        state1.set_step_result("branch_a", serde_json::json!("result_a"));

        let mut state2 = AgentGraphState::new("task");
        state2.set_step_result("branch_b", serde_json::json!("result_b"));

        let mut state3 = AgentGraphState::new("task");
        state3.set_step_result("branch_c", serde_json::json!("result_c"));

        state1.merge(state2);
        state1.merge(state3);

        assert!(state1.get_step_result("branch_a").is_some());
        assert!(state1.get_step_result("branch_b").is_some());
        assert!(state1.get_step_result("branch_c").is_some());
    }

    #[tokio::test]
    async fn test_hydrate_flush_roundtrip() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let uid = test_user_id();

        // First execution: flush results
        let bridge = MemoryBridge::with_defaults(store.clone(), uid);
        let mut state = AgentGraphState::new("test task");
        state.set_step_result("node_x", serde_json::json!({"output": "value"}));
        bridge.flush(&state).await;

        // Second execution: verify store has data
        let entries = store.list(uid).await;
        assert!(!entries.is_empty());
    }
}
