//! Knowledge distillation and context injection.
//!
//! The [`KnowledgeDistiller`] converts mined patterns into
//! [`KnowledgeEntry`] records and provides a query interface
//! for injecting relevant knowledge into agent context.
//!
//! Uses `std::sync::RwLock` (not `tokio::sync::RwLock`) because
//! locks are held only briefly and never across `.await` points,
//! and the `query_relevant` / `store_patterns` methods need to be
//! callable from both sync and async contexts.

use std::collections::HashSet;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::pattern::{MinedPattern, PatternType};

/// A knowledge entry derived from learned patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    /// Category of this knowledge.
    pub category: KnowledgeCategory,
    /// Human-readable content that can be injected into context.
    pub content: String,
    /// Confidence score between 0.0 and 1.0.
    pub confidence: f32,
    /// When this entry was created.
    #[serde(default = "default_now")]
    pub created_at: DateTime<Utc>,
    /// When this entry was last accessed (queried).
    #[serde(default = "default_now")]
    pub last_accessed: DateTime<Utc>,
}

fn default_now() -> DateTime<Utc> {
    Utc::now()
}

/// Category of knowledge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeCategory {
    /// Guidance on which tools to use.
    ToolGuidance,
    /// Which model to prefer for a task type.
    ModelPreference,
    /// Warnings about known failure modes.
    ErrorAvoidance,
    /// Hints about which workflow pattern to use.
    WorkflowHint,
}

/// Maximum number of knowledge entries before eviction.
const MAX_KNOWLEDGE_ENTRIES: usize = 5_000;

/// Compute the decayed confidence for a knowledge entry.
///
/// Entries have a 90-day grace period after their last access. After
/// that, confidence decays at 0.1 per month (30 days), bottoming out
/// at 0.0.
pub fn decayed_confidence(entry: &KnowledgeEntry) -> f64 {
    let days_idle = (Utc::now() - entry.last_accessed).num_days() as f64;
    let months_idle = days_idle / 30.0;
    let decay = (months_idle - 3.0).max(0.0) * 0.1; // 90 days grace period
    (entry.confidence as f64 - decay).max(0.0)
}

/// Stores and queries learned knowledge.
///
/// The distiller converts [`MinedPattern`]s into [`KnowledgeEntry`]
/// records and provides keyword-based relevance matching for context
/// injection.
pub struct KnowledgeDistiller {
    knowledge: RwLock<Vec<KnowledgeEntry>>,
}

impl KnowledgeDistiller {
    /// Create a new distiller.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let distiller = KnowledgeDistiller::new();
    /// ```
    pub fn new() -> Self {
        Self {
            knowledge: RwLock::new(Vec::new()),
        }
    }

    /// Store patterns as knowledge entries. Returns number stored.
    ///
    /// Patterns are converted to knowledge entries and appended to the
    /// store. Duplicate entries (by content) are removed. When the store
    /// exceeds `MAX_KNOWLEDGE_ENTRIES`, the lowest-confidence entries are
    /// evicted.
    pub fn store_patterns(&self, patterns: &[MinedPattern]) -> usize {
        let entries: Vec<KnowledgeEntry> = patterns
            .iter()
            .map(|p| self.pattern_to_knowledge(p))
            .collect();
        let count = entries.len();

        let mut k = self.knowledge.write().unwrap_or_else(|p| p.into_inner());
        k.extend(entries);

        // Dedup by content
        let mut seen = HashSet::new();
        k.retain(|e| seen.insert(e.content.clone()));

        // Evict lowest-confidence entries when over limit
        if k.len() > MAX_KNOWLEDGE_ENTRIES {
            k.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            k.truncate(MAX_KNOWLEDGE_ENTRIES);
        }

        count
    }

    /// Query knowledge relevant to a task.
    ///
    /// Performs keyword overlap scoring against the task string and
    /// returns the top 5 most relevant entries.  High-confidence
    /// entries (> 0.8, after time decay) are always included regardless
    /// of keyword overlap. Entries older than 90 days without access
    /// begin to decay in confidence at a rate of 0.1 per month.
    pub fn query_relevant(&self, task: &str) -> Vec<KnowledgeEntry> {
        let task_words: HashSet<String> = task
            .to_lowercase()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        // R2-M: Phase 1 — read-only scoring (no write lock needed)
        let selected_indices = {
            let k = self.knowledge.read().unwrap_or_else(|p| p.into_inner());
            let mut scored: Vec<(f32, usize)> = k
                .iter()
                .enumerate()
                .filter_map(|(idx, entry)| {
                    let decayed = decayed_confidence(entry);
                    let entry_words: HashSet<String> = entry
                        .content
                        .to_lowercase()
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect();
                    let overlap = task_words.intersection(&entry_words).count();
                    if overlap > 0 || decayed > 0.8 {
                        let score = overlap as f32 * decayed as f32;
                        Some((score, idx))
                    } else {
                        None
                    }
                })
                .collect();

            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored
                .into_iter()
                .take(5)
                .map(|(_, idx)| idx)
                .collect::<Vec<usize>>()
        };

        // Phase 2 — write lock only for updating last_accessed timestamps
        let now = Utc::now();
        let mut k = self.knowledge.write().unwrap_or_else(|p| p.into_inner());
        for &idx in &selected_indices {
            k[idx].last_accessed = now;
        }

        selected_indices
            .into_iter()
            .map(|idx| k[idx].clone())
            .collect()
    }

    /// Convert a mined pattern into a knowledge entry.
    pub(crate) fn pattern_to_knowledge(&self, pattern: &MinedPattern) -> KnowledgeEntry {
        let (category, content) = match &pattern.pattern_type {
            PatternType::ToolSequence => {
                (KnowledgeCategory::ToolGuidance, pattern.description.clone())
            }
            PatternType::ErrorRecovery => (
                KnowledgeCategory::ErrorAvoidance,
                pattern.description.clone(),
            ),
            PatternType::ModelSelection => (
                KnowledgeCategory::ModelPreference,
                pattern.description.clone(),
            ),
            PatternType::TaskDecomposition => {
                (KnowledgeCategory::WorkflowHint, pattern.description.clone())
            }
            PatternType::FailurePattern => (
                KnowledgeCategory::ErrorAvoidance,
                pattern.description.clone(),
            ),
        };

        let now = Utc::now();
        KnowledgeEntry {
            category,
            content,
            confidence: pattern.confidence.clamp(0.0, 1.0),
            created_at: now,
            last_accessed: now,
        }
    }

    /// Number of stored knowledge entries.
    pub fn knowledge_count(&self) -> usize {
        self.knowledge
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }

    /// Get all knowledge entries (cloned).
    pub fn all_knowledge(&self) -> Vec<KnowledgeEntry> {
        self.knowledge
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Clear all stored knowledge.
    pub fn clear(&self) {
        self.knowledge
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}

impl Default for KnowledgeDistiller {
    fn default() -> Self {
        Self::new()
    }
}

impl super::provider::KnowledgeProvider for KnowledgeDistiller {
    fn query(&self, task: &str, limit: usize) -> Vec<KnowledgeEntry> {
        let mut results = self.query_relevant(task);
        results.truncate(limit);
        results
    }
}

// SAFETY: std::sync::RwLock<Vec<T>> is Send+Sync when T: Send+Sync,
// and KnowledgeEntry is Send+Sync (composed of String, f32, DateTime, and an enum of Strings).
// The compiler can verify this automatically.

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool_pattern(desc: &str, confidence: f32) -> MinedPattern {
        MinedPattern {
            pattern_type: PatternType::ToolSequence,
            description: desc.into(),
            frequency: 5,
            confidence,
            examples: vec![],
            tool_sequence: None,
            avg_duration_ms: 0,
        }
    }

    #[test]
    fn test_store_and_query() {
        let distiller = KnowledgeDistiller::new();

        let patterns = vec![
            MinedPattern {
                pattern_type: PatternType::ToolSequence,
                description: "Tool chain: screenshot -> click -> type".into(),
                frequency: 10,
                confidence: 0.9,
                examples: vec!["browser task".into()],
                tool_sequence: Some(vec!["screenshot".into(), "click".into(), "type".into()]),
                avg_duration_ms: 5000,
            },
            MinedPattern {
                pattern_type: PatternType::ModelSelection,
                description: "For code tasks, claude-sonnet achieves 90% success".into(),
                frequency: 20,
                confidence: 0.9,
                examples: vec![],
                tool_sequence: None,
                avg_duration_ms: 0,
            },
        ];

        let stored = distiller.store_patterns(&patterns);
        assert_eq!(stored, 2);
        assert_eq!(distiller.knowledge_count(), 2);

        // Query for browser-related knowledge
        let results = distiller.query_relevant("take a screenshot of the browser");
        assert!(!results.is_empty());
    }

    #[test]
    fn test_dedup_patterns() {
        let distiller = KnowledgeDistiller::new();

        let pattern = make_tool_pattern("same pattern", 0.8);

        distiller.store_patterns(&[pattern.clone()]);
        distiller.store_patterns(&[pattern.clone()]);

        assert_eq!(distiller.knowledge_count(), 1); // Deduped
    }

    #[test]
    fn test_pattern_to_knowledge_categories() {
        let distiller = KnowledgeDistiller::new();

        let tool_pattern = MinedPattern {
            pattern_type: PatternType::ToolSequence,
            description: "tool chain".into(),
            frequency: 5,
            confidence: 0.8,
            examples: vec![],
            tool_sequence: None,
            avg_duration_ms: 0,
        };
        let k = distiller.pattern_to_knowledge(&tool_pattern);
        assert_eq!(k.category, KnowledgeCategory::ToolGuidance);

        let error_pattern = MinedPattern {
            pattern_type: PatternType::ErrorRecovery,
            description: "recovery".into(),
            frequency: 5,
            confidence: 0.8,
            examples: vec![],
            tool_sequence: None,
            avg_duration_ms: 0,
        };
        let k = distiller.pattern_to_knowledge(&error_pattern);
        assert_eq!(k.category, KnowledgeCategory::ErrorAvoidance);

        let model_pattern = MinedPattern {
            pattern_type: PatternType::ModelSelection,
            description: "model".into(),
            frequency: 5,
            confidence: 0.8,
            examples: vec![],
            tool_sequence: None,
            avg_duration_ms: 0,
        };
        let k = distiller.pattern_to_knowledge(&model_pattern);
        assert_eq!(k.category, KnowledgeCategory::ModelPreference);

        let decomp_pattern = MinedPattern {
            pattern_type: PatternType::TaskDecomposition,
            description: "decompose".into(),
            frequency: 5,
            confidence: 0.8,
            examples: vec![],
            tool_sequence: None,
            avg_duration_ms: 0,
        };
        let k = distiller.pattern_to_knowledge(&decomp_pattern);
        assert_eq!(k.category, KnowledgeCategory::WorkflowHint);
    }

    #[test]
    fn test_empty_query() {
        let distiller = KnowledgeDistiller::new();
        let results = distiller.query_relevant("anything");
        assert!(results.is_empty());
    }

    #[test]
    fn test_high_confidence_always_returned() {
        let distiller = KnowledgeDistiller::new();

        // Store a high-confidence entry with unrelated keywords
        let pattern = MinedPattern {
            pattern_type: PatternType::ErrorRecovery,
            description: "Always retry on network failure".into(),
            frequency: 50,
            confidence: 0.95,
            examples: vec![],
            tool_sequence: None,
            avg_duration_ms: 0,
        };
        distiller.store_patterns(&[pattern]);

        // Query with completely different words — should still return
        // because confidence > 0.8
        let results = distiller.query_relevant("xyz unrelated query");
        assert!(!results.is_empty());
    }

    #[test]
    fn test_query_returns_max_5() {
        let distiller = KnowledgeDistiller::new();

        // Store 10 high-confidence entries
        let patterns: Vec<MinedPattern> = (0..10)
            .map(|i| MinedPattern {
                pattern_type: PatternType::ToolSequence,
                description: format!("pattern {} with keyword test", i),
                frequency: 5,
                confidence: 0.9,
                examples: vec![],
                tool_sequence: None,
                avg_duration_ms: 0,
            })
            .collect();
        distiller.store_patterns(&patterns);

        let results = distiller.query_relevant("test query");
        assert!(results.len() <= 5);
    }

    #[test]
    fn test_clear() {
        let distiller = KnowledgeDistiller::new();
        distiller.store_patterns(&[make_tool_pattern("test", 0.8)]);
        assert_eq!(distiller.knowledge_count(), 1);

        distiller.clear();
        assert_eq!(distiller.knowledge_count(), 0);
    }

    #[test]
    fn test_all_knowledge() {
        let distiller = KnowledgeDistiller::new();
        distiller.store_patterns(&[
            make_tool_pattern("first", 0.8),
            make_tool_pattern("second", 0.9),
        ]);

        let all = distiller.all_knowledge();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_knowledge_entry_serialize() {
        let now = Utc::now();
        let entry = KnowledgeEntry {
            category: KnowledgeCategory::ToolGuidance,
            content: "Use screenshot before click".into(),
            confidence: 0.85,
            created_at: now,
            last_accessed: now,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: KnowledgeEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.category, KnowledgeCategory::ToolGuidance);
        assert_eq!(parsed.content, "Use screenshot before click");
        assert!(json.contains("created_at"));
        assert!(json.contains("last_accessed"));
    }

    #[test]
    fn test_knowledge_entry_deserialize_without_timestamps() {
        // Ensure backward compatibility: JSON without timestamps uses defaults
        let json = r#"{"category":"tool_guidance","content":"test","confidence":0.5}"#;
        let entry: KnowledgeEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.content, "test");
        // created_at and last_accessed should default to now
        assert!(entry.created_at <= Utc::now());
        assert!(entry.last_accessed <= Utc::now());
    }

    #[test]
    fn test_knowledge_category_serialize() {
        let cat = KnowledgeCategory::ErrorAvoidance;
        let json = serde_json::to_string(&cat).unwrap();
        assert_eq!(json, "\"error_avoidance\"");

        let parsed: KnowledgeCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, KnowledgeCategory::ErrorAvoidance);
    }

    #[test]
    fn test_decayed_confidence_no_decay_within_grace() {
        let now = Utc::now();
        let entry = KnowledgeEntry {
            category: KnowledgeCategory::ToolGuidance,
            content: "recent entry".into(),
            confidence: 0.9,
            created_at: now,
            last_accessed: now,
        };
        let dc = decayed_confidence(&entry);
        assert!((dc - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_decayed_confidence_after_grace_period() {
        use chrono::Duration;
        let now = Utc::now();
        // 120 days idle => 4 months => 1 month past grace => 0.1 decay
        let entry = KnowledgeEntry {
            category: KnowledgeCategory::ToolGuidance,
            content: "old entry".into(),
            confidence: 0.8,
            created_at: now - Duration::days(120),
            last_accessed: now - Duration::days(120),
        };
        let dc = decayed_confidence(&entry);
        // 120 days = 4 months, grace = 3 months, decay = (4 - 3) * 0.1 = 0.1
        // 0.8 - 0.1 = 0.7
        assert!((dc - 0.7).abs() < 0.05);
    }

    #[test]
    fn test_decayed_confidence_floors_at_zero() {
        use chrono::Duration;
        let now = Utc::now();
        // 365 days idle => ~12.2 months => 9.2 months past grace => 0.92 decay
        // 0.5 - 0.92 = negative => clamped to 0.0
        let entry = KnowledgeEntry {
            category: KnowledgeCategory::ToolGuidance,
            content: "very old entry".into(),
            confidence: 0.5,
            created_at: now - Duration::days(365),
            last_accessed: now - Duration::days(365),
        };
        let dc = decayed_confidence(&entry);
        assert!((dc - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_query_updates_last_accessed() {
        let distiller = KnowledgeDistiller::new();

        let pattern = MinedPattern {
            pattern_type: PatternType::ToolSequence,
            description: "Tool chain: screenshot -> click".into(),
            frequency: 10,
            confidence: 0.9,
            examples: vec![],
            tool_sequence: None,
            avg_duration_ms: 0,
        };
        distiller.store_patterns(&[pattern]);

        // Query to trigger last_accessed update
        let results = distiller.query_relevant("screenshot click");
        assert!(!results.is_empty());

        // The returned entries should have last_accessed very close to now
        let entry = &results[0];
        let diff = (Utc::now() - entry.last_accessed).num_seconds();
        assert!(diff < 2);
    }

    #[test]
    fn test_stored_patterns_have_timestamps() {
        let distiller = KnowledgeDistiller::new();
        distiller.store_patterns(&[make_tool_pattern("timestamped", 0.7)]);

        let all = distiller.all_knowledge();
        assert_eq!(all.len(), 1);
        let entry = &all[0];
        let diff = (Utc::now() - entry.created_at).num_seconds();
        assert!(diff < 2);
    }

    #[test]
    fn test_failure_pattern_to_knowledge() {
        let distiller = KnowledgeDistiller::new();
        let pattern = MinedPattern {
            pattern_type: PatternType::FailurePattern,
            description: "Avoid: navigate -> click without screenshot".into(),
            frequency: 5,
            confidence: 0.7,
            examples: vec![],
            tool_sequence: Some(vec!["navigate".into(), "click".into()]),
            avg_duration_ms: 0,
        };
        let k = distiller.pattern_to_knowledge(&pattern);
        assert_eq!(k.category, KnowledgeCategory::ErrorAvoidance);
        assert!(k.content.contains("Avoid"));
    }
}
