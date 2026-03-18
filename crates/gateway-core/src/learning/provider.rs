//! Knowledge provider trait for the context system.
//!
//! The [`KnowledgeProvider`] trait abstracts access to learned knowledge,
//! allowing the context system to query relevant knowledge without being
//! coupled to a specific knowledge storage implementation.

use super::distiller::KnowledgeEntry;

/// Trait for providing learned knowledge to the context system.
///
/// Implementations should return knowledge entries relevant to a given
/// task, ordered by relevance. The `limit` parameter controls the
/// maximum number of entries to return.
///
/// # Example
///
/// ```ignore
/// use gateway_core::learning::{KnowledgeProvider, KnowledgeEntry};
///
/// struct MyProvider;
///
/// impl KnowledgeProvider for MyProvider {
///     fn query(&self, task: &str, limit: usize) -> Vec<KnowledgeEntry> {
///         // Return relevant knowledge for the task
///         vec![]
///     }
/// }
/// ```
pub trait KnowledgeProvider: Send + Sync {
    /// Query knowledge relevant to a task.
    ///
    /// Returns up to `limit` knowledge entries relevant to the given task,
    /// ordered by relevance (most relevant first).
    fn query(&self, task: &str, limit: usize) -> Vec<KnowledgeEntry>;
}

/// Wrapper that provides knowledge from a LearningEngine.
///
/// This adapter implements `KnowledgeProvider` by delegating to
/// the `LearningEngine::query_knowledge` method.
pub struct LearningEngineProvider {
    engine: std::sync::Arc<super::LearningEngine>,
}

impl LearningEngineProvider {
    /// Create a new provider from a LearningEngine.
    pub fn new(engine: std::sync::Arc<super::LearningEngine>) -> Self {
        Self { engine }
    }
}

impl KnowledgeProvider for LearningEngineProvider {
    fn query(&self, task: &str, limit: usize) -> Vec<KnowledgeEntry> {
        let mut results = self.engine.query_knowledge(task);
        results.truncate(limit);
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learning::distiller::KnowledgeDistiller;
    use crate::learning::pattern::{MinedPattern, PatternType};

    /// Verify that a trivial implementation compiles and works.
    struct EmptyProvider;

    impl KnowledgeProvider for EmptyProvider {
        fn query(&self, _task: &str, _limit: usize) -> Vec<KnowledgeEntry> {
            vec![]
        }
    }

    #[test]
    fn test_empty_provider() {
        let provider = EmptyProvider;
        let results = provider.query("anything", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_knowledge_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EmptyProvider>();
    }

    #[test]
    fn test_distiller_as_provider() {
        let distiller = KnowledgeDistiller::new();

        // Store some knowledge
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

        // Query via the trait
        let provider: &dyn KnowledgeProvider = &distiller;
        let results = provider.query("take a screenshot and click", 5);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_distiller_provider_respects_limit() {
        let distiller = KnowledgeDistiller::new();

        // Store many patterns
        let patterns: Vec<MinedPattern> = (0..10)
            .map(|i| MinedPattern {
                pattern_type: PatternType::ToolSequence,
                description: format!("pattern {} with keyword browser", i),
                frequency: 5,
                confidence: 0.9,
                examples: vec![],
                tool_sequence: None,
                avg_duration_ms: 0,
            })
            .collect();
        distiller.store_patterns(&patterns);

        let provider: &dyn KnowledgeProvider = &distiller;
        let results = provider.query("browser", 3);
        assert!(results.len() <= 3);
    }

    #[test]
    fn test_distiller_provider_empty_store() {
        let distiller = KnowledgeDistiller::new();
        let provider: &dyn KnowledgeProvider = &distiller;
        let results = provider.query("anything", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_distiller_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<KnowledgeDistiller>();
    }
}
