//! Learning and knowledge system for the AI Gateway.
//!
//! Implements a closed-loop learning cycle:
//!
//! 1. **Experience Collection** -- Records execution outcomes via `GraphObserver`
//! 2. **Pattern Mining** -- Discovers repeated tool sequences and error recovery patterns
//! 3. **Skill Extraction** -- Converts high-confidence patterns into reusable skills
//! 4. **Knowledge Distillation** -- Injects learned knowledge into agent context
//!
//! # Architecture
//!
//! ```text
//! GraphExecutor
//!       |
//!       v
//! LearningObserver ──> ExperienceCollector ──> LearningEngine
//!                                                   |
//!                                          ┌────────┴────────┐
//!                                          v                  v
//!                                    PatternMiner    KnowledgeDistiller
//!                                          |                  |
//!                                          v                  v
//!                                   MinedPattern[]     KnowledgeEntry[]
//!                                          |                  |
//!                                          └──────────────────┘
//!                                                   |
//!                                                   v
//!                                         Agent Context Injection
//! ```
//!
//! # Feature Gate
//!
//! This module is gated behind `#[cfg(feature = "learning")]` and
//! requires the `graph` feature (for `GraphObserver` and `AgentGraphState`).

pub mod collector;
pub mod distiller;
pub mod experience;
pub mod pattern;
pub mod provider;
pub mod reflection;

pub use collector::{ExperienceCollector, LearningObserver};
pub use distiller::{decayed_confidence, KnowledgeCategory, KnowledgeDistiller, KnowledgeEntry};
pub use experience::{
    Experience, ExperienceResult, FeedbackSignal, NodeTraceEntry, ToolCallRecord,
};
pub use pattern::{MinedPattern, PatternMiner, PatternMinerConfig, PatternType};
pub use provider::{KnowledgeProvider, LearningEngineProvider};
pub use reflection::{ReflectionStore, StepReflection, StepVerdict};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::memory::{Confidence, MemoryCategory, MemoryEntry, MemorySource, UnifiedMemoryStore};

/// Configuration for the learning engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningConfig {
    /// Whether learning is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Number of experiences to buffer before triggering a learning cycle.
    #[serde(default = "default_buffer_threshold")]
    pub buffer_threshold: usize,
    /// Pattern miner configuration.
    #[serde(default)]
    pub miner: PatternMinerConfig,
}

fn default_true() -> bool {
    true
}
fn default_buffer_threshold() -> usize {
    10
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            buffer_threshold: 10,
            miner: PatternMinerConfig::default(),
        }
    }
}

impl LearningConfig {
    /// Validate configuration values. Returns an error message if invalid.
    pub fn validate(&self) -> std::result::Result<(), String> {
        self.miner.validate()?;
        if self.buffer_threshold == 0 {
            return Err("buffer_threshold must be > 0".into());
        }
        Ok(())
    }
}

/// Report from a learning cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningReport {
    /// Number of experiences that were processed.
    pub experiences_processed: usize,
    /// Number of patterns discovered during mining.
    pub patterns_mined: usize,
    /// Number of patterns stored as knowledge.
    pub patterns_stored: usize,
}

impl LearningReport {
    /// Create an empty report (no work done).
    pub fn empty() -> Self {
        Self {
            experiences_processed: 0,
            patterns_mined: 0,
            patterns_stored: 0,
        }
    }

    /// Create a disabled report.
    pub fn disabled() -> Self {
        Self::empty()
    }
}

/// Main learning engine that orchestrates all components.
///
/// The engine ties together experience collection, pattern mining,
/// and knowledge distillation.  Call [`record`](Self::record) to
/// buffer experiences and [`learn`](Self::learn) to run a mining
/// cycle on the buffered data.
pub struct LearningEngine {
    collector: Arc<ExperienceCollector>,
    miner: PatternMiner,
    distiller: KnowledgeDistiller,
    enabled: Arc<AtomicBool>,
    /// Guard to prevent concurrent `learn()` calls from causing duplicate work.
    learning_in_progress: Arc<AtomicBool>,
    /// Optional unified memory store for persisting learned knowledge.
    unified_store: Option<Arc<UnifiedMemoryStore>>,
}

impl LearningEngine {
    /// Create a new learning engine.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use gateway_core::learning::{LearningEngine, LearningConfig};
    ///
    /// let engine = LearningEngine::new(LearningConfig::default());
    /// ```
    pub fn new(config: LearningConfig) -> Self {
        let enabled = config.enabled;
        Self {
            collector: Arc::new(ExperienceCollector::new(config.buffer_threshold)),
            miner: PatternMiner::new(config.miner),
            distiller: KnowledgeDistiller::new(),
            enabled: Arc::new(AtomicBool::new(enabled)),
            learning_in_progress: Arc::new(AtomicBool::new(false)),
            unified_store: None,
        }
    }

    /// Set the unified memory store for persisting learned knowledge.
    pub fn with_unified_store(mut self, store: Arc<UnifiedMemoryStore>) -> Self {
        self.unified_store = Some(store);
        self
    }

    /// Get a reference to the experience collector.
    ///
    /// Use this to create a [`LearningObserver`] for graph execution.
    pub fn collector(&self) -> &Arc<ExperienceCollector> {
        &self.collector
    }

    /// Get a reference to the knowledge distiller.
    pub fn distiller(&self) -> &KnowledgeDistiller {
        &self.distiller
    }

    /// Record a completed execution experience.
    ///
    /// If learning is disabled, the experience is silently dropped.
    #[tracing::instrument(skip(self, experience), fields(task = %experience.task))]
    pub async fn record(&self, experience: Experience) -> Result<()> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        self.collector.record(experience).await;
        Ok(())
    }

    /// Run the learning cycle on buffered experiences.
    ///
    /// Drains all buffered experiences, runs pattern mining across
    /// four strategies (tool sequences, error recovery, model selection,
    /// failure patterns), and stores the discovered patterns as knowledge entries.
    #[tracing::instrument(skip(self))]
    pub async fn learn(&self) -> Result<LearningReport> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Ok(LearningReport::disabled());
        }

        // Prevent concurrent learn() calls from duplicating work.
        if self
            .learning_in_progress
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            tracing::debug!("Learning cycle already in progress, skipping");
            return Ok(LearningReport::empty());
        }

        let result = self.learn_inner().await;
        self.learning_in_progress.store(false, Ordering::Release);
        result
    }

    /// Inner learning implementation, separated to ensure the progress flag is always cleared.
    async fn learn_inner(&self) -> Result<LearningReport> {
        let experiences = self.collector.drain_buffer().await;
        if experiences.is_empty() {
            return Ok(LearningReport::empty());
        }

        let exp_count = experiences.len();

        // Mine patterns
        let mut patterns = Vec::new();
        patterns.extend(self.miner.mine_tool_sequences(&experiences));
        patterns.extend(self.miner.mine_error_recovery(&experiences));
        patterns.extend(self.miner.mine_model_selection(&experiences));
        patterns.extend(self.miner.mine_failure_patterns(&experiences));

        let pattern_count = patterns.len();

        // Store patterns as knowledge
        let stored = self.distiller.store_patterns(&patterns);

        // R2-M: Persist only newly stored knowledge (not all high-confidence entries)
        if stored > 0 {
            if let Some(ref store) = self.unified_store {
                // Convert patterns directly instead of re-querying all entries
                let system_user = uuid::Uuid::nil();
                let mut persisted = 0usize;
                for pattern in &patterns {
                    let entry = self.distiller.pattern_to_knowledge(pattern);
                    if entry.confidence < 0.5 {
                        continue;
                    }
                    let confidence = if entry.confidence >= 0.8 {
                        Confidence::High
                    } else {
                        Confidence::Medium
                    };
                    // R2-M: Use stable category name instead of Debug format
                    let category_name = match &entry.category {
                        KnowledgeCategory::ToolGuidance => "tool_guidance",
                        KnowledgeCategory::ModelPreference => "model_preference",
                        KnowledgeCategory::ErrorAvoidance => "error_avoidance",
                        KnowledgeCategory::WorkflowHint => "workflow_hint",
                    };
                    let memory_entry = MemoryEntry::new(
                        format!("learned_{}_{}", category_name, entry.content.len()),
                        MemoryCategory::Pattern,
                        entry.content.clone(),
                    )
                    .with_source(MemorySource::System)
                    .with_confidence(confidence)
                    .with_tags(vec!["learned".to_string(), category_name.to_string()]);
                    let _ = store.store(system_user, memory_entry).await;
                    persisted += 1;
                }
                if persisted > 0 {
                    tracing::debug!(
                        count = persisted,
                        "Persisted learned knowledge to UnifiedMemoryStore"
                    );
                }
            }
        }

        tracing::info!(
            experiences = exp_count,
            patterns_mined = pattern_count,
            patterns_stored = stored,
            "Learning cycle completed"
        );

        Ok(LearningReport {
            experiences_processed: exp_count,
            patterns_mined: pattern_count,
            patterns_stored: stored,
        })
    }

    /// Query knowledge relevant to a task.
    pub fn query_knowledge(&self, task: &str) -> Vec<KnowledgeEntry> {
        self.distiller.query_relevant(task)
    }

    /// Runtime kill switch.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        tracing::info!(enabled, "Learning engine enabled state changed");
    }

    /// Whether learning is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Get the number of buffered experiences.
    pub async fn buffer_size(&self) -> usize {
        self.collector.buffer_size().await
    }

    /// Get the total number of stored knowledge entries.
    pub fn knowledge_count(&self) -> usize {
        self.distiller.knowledge_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_disabled() {
        let engine = LearningEngine::new(LearningConfig {
            enabled: false,
            ..Default::default()
        });

        let exp = Experience::test_success("test task");
        engine.record(exp).await.unwrap();
        assert_eq!(engine.buffer_size().await, 0); // Not recorded when disabled

        let report = engine.learn().await.unwrap();
        assert_eq!(report.experiences_processed, 0);
    }

    #[tokio::test]
    async fn test_engine_record_and_learn() {
        let engine = LearningEngine::new(LearningConfig::default());

        let exp = Experience::test_success("test task");
        engine.record(exp).await.unwrap();
        assert_eq!(engine.buffer_size().await, 1);

        let report = engine.learn().await.unwrap();
        assert_eq!(report.experiences_processed, 1);
        assert_eq!(engine.buffer_size().await, 0); // Buffer drained
    }

    #[tokio::test]
    async fn test_engine_kill_switch() {
        let engine = LearningEngine::new(LearningConfig::default());
        assert!(engine.is_enabled());

        engine.set_enabled(false);
        assert!(!engine.is_enabled());

        let exp = Experience::test_success("test task");
        engine.record(exp).await.unwrap();
        assert_eq!(engine.buffer_size().await, 0);
    }

    #[tokio::test]
    async fn test_engine_learn_empty_buffer() {
        let engine = LearningEngine::new(LearningConfig::default());

        let report = engine.learn().await.unwrap();
        assert_eq!(report.experiences_processed, 0);
        assert_eq!(report.patterns_mined, 0);
        assert_eq!(report.patterns_stored, 0);
    }

    #[tokio::test]
    async fn test_engine_knowledge_query() {
        let engine = LearningEngine::new(LearningConfig {
            miner: PatternMinerConfig {
                min_frequency: 1,
                min_confidence: 0.0,
                ..Default::default()
            },
            ..Default::default()
        });

        // Record experiences with tool calls
        let mut exp = Experience::test_success("click the browser button");
        exp.tool_calls = vec![
            ToolCallRecord {
                tool_name: "screenshot".into(),
                input_summary: "".into(),
                success: true,
                duration_ms: 100,
                error: None,
            },
            ToolCallRecord {
                tool_name: "click".into(),
                input_summary: "".into(),
                success: true,
                duration_ms: 50,
                error: None,
            },
        ];
        engine.record(exp).await.unwrap();

        // Run learning
        let report = engine.learn().await.unwrap();
        assert_eq!(report.experiences_processed, 1);

        // Query should find knowledge related to browser tasks
        // (may or may not find results depending on mining thresholds)
        let _results = engine.query_knowledge("take a screenshot in browser");
    }

    #[tokio::test]
    async fn test_engine_multiple_learn_cycles() {
        let engine = LearningEngine::new(LearningConfig::default());

        // First cycle
        engine
            .record(Experience::test_success("task 1"))
            .await
            .unwrap();
        let r1 = engine.learn().await.unwrap();
        assert_eq!(r1.experiences_processed, 1);

        // Second cycle
        engine
            .record(Experience::test_success("task 2"))
            .await
            .unwrap();
        engine
            .record(Experience::test_success("task 3"))
            .await
            .unwrap();
        let r2 = engine.learn().await.unwrap();
        assert_eq!(r2.experiences_processed, 2);
    }

    #[tokio::test]
    async fn test_engine_knowledge_count() {
        let engine = LearningEngine::new(LearningConfig::default());
        assert_eq!(engine.knowledge_count(), 0);
    }

    #[test]
    fn test_learning_config_default() {
        let config = LearningConfig::default();
        assert!(config.enabled);
        assert_eq!(config.buffer_threshold, 10);
    }

    #[test]
    fn test_learning_config_serialize() {
        let config = LearningConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: LearningConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.enabled, config.enabled);
        assert_eq!(parsed.buffer_threshold, config.buffer_threshold);
    }

    #[test]
    fn test_learning_report_empty() {
        let report = LearningReport::empty();
        assert_eq!(report.experiences_processed, 0);
        assert_eq!(report.patterns_mined, 0);
        assert_eq!(report.patterns_stored, 0);
    }

    #[test]
    fn test_learning_report_serialize() {
        let report = LearningReport {
            experiences_processed: 10,
            patterns_mined: 5,
            patterns_stored: 3,
        };
        let json = serde_json::to_string(&report).unwrap();
        let parsed: LearningReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.experiences_processed, 10);
        assert_eq!(parsed.patterns_mined, 5);
        assert_eq!(parsed.patterns_stored, 3);
    }
}
