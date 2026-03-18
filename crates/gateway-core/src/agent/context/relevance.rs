//! Relevance scoring for dynamic context filtering.
//!
//! Provides a unified scoring system for skills, knowledge entries,
//! and memory items. Items are scored based on keyword overlap,
//! confidence, and recency, then dynamically filtered to fit within
//! the token budget.
//!
//! # Scoring Formula
//!
//! `score = keyword_overlap * 0.6 + confidence * 0.3 + recency * 0.1 + skill_bonus`
//!
//! Where:
//! - `keyword_overlap`: fraction of task keywords found in item (0.0-1.0)
//! - `confidence`: item's confidence score (0.0-1.0)
//! - `recency`: decayed score based on last access time (0.0-1.0)
//! - `skill_bonus`: 0.1 for Skill items, 0.0 otherwise

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Source type for a scored item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemSource {
    /// A skill from the registry.
    Skill(String),
    /// A knowledge entry from the learning system.
    Knowledge(String),
    /// A memory item from the unified memory store.
    Memory(String),
}

impl std::fmt::Display for ItemSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skill(name) => write!(f, "[Skill] {}", name),
            Self::Knowledge(cat) => write!(f, "[Knowledge] {}", cat),
            Self::Memory(cat) => write!(f, "[Memory] {}", cat),
        }
    }
}

/// A content item with its relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredItem {
    /// The content text to inject into the prompt.
    pub content: String,
    /// Estimated token count for this item.
    pub tokens: usize,
    /// Computed relevance score (0.0-1.0).
    pub score: f64,
    /// Source of this item.
    pub source: ItemSource,
}

/// Trait for items that can be scored for relevance.
pub trait Scorable {
    /// Tags or keywords associated with this item.
    fn tags(&self) -> &[String];
    /// A short preview of the content.
    fn content_preview(&self) -> &str;
    /// Confidence score (0.0-1.0).
    fn confidence(&self) -> f64;
    /// When this item was last accessed.
    fn last_accessed(&self) -> DateTime<Utc>;
}

/// Configuration for the relevance scorer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevanceScorerConfig {
    /// Minimum score threshold (items below this are filtered out).
    pub min_threshold: f64,
    /// Weight for keyword overlap.
    pub keyword_weight: f64,
    /// Weight for confidence.
    pub confidence_weight: f64,
    /// Weight for recency.
    pub recency_weight: f64,
    /// Bonus score for Skill items.
    pub skill_bonus: f64,
    /// Maximum percentage of token budget for knowledge items.
    pub knowledge_max_pct: f64,
}

impl Default for RelevanceScorerConfig {
    fn default() -> Self {
        Self {
            min_threshold: 0.3,
            keyword_weight: 0.6,
            confidence_weight: 0.3,
            recency_weight: 0.1,
            skill_bonus: 0.1,
            knowledge_max_pct: 0.4,
        }
    }
}

/// Relevance scorer for dynamic context filtering.
///
/// Scores items based on keyword overlap with the task, confidence,
/// and recency. Then selects the best items that fit within the token budget.
///
/// # Example
///
/// ```rust,ignore
/// use gateway_core::agent::context::relevance::{RelevanceScorer, ScoredItem, ItemSource};
///
/// let scorer = RelevanceScorer::with_defaults();
/// let keywords = RelevanceScorer::extract_keywords("send email via gmail");
///
/// // Score and select items within a token budget
/// let items = vec![/* scored items */];
/// let selected = scorer.select(items, 2000);
/// ```
pub struct RelevanceScorer {
    config: RelevanceScorerConfig,
}

impl RelevanceScorer {
    /// Create a new scorer with the given configuration.
    pub fn new(config: RelevanceScorerConfig) -> Self {
        Self { config }
    }

    /// Create a scorer with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(RelevanceScorerConfig::default())
    }

    /// Score a single item against task keywords.
    ///
    /// Returns a score between 0.0 and 1.0 (plus optional skill bonus).
    ///
    /// # Arguments
    ///
    /// * `item` - The item to score (must implement `Scorable`).
    /// * `task_keywords` - Keywords extracted from the current task.
    /// * `is_skill` - Whether the item is a skill (adds bonus).
    pub fn score(
        &self,
        item: &dyn Scorable,
        task_keywords: &HashSet<String>,
        is_skill: bool,
    ) -> f64 {
        let keyword_score =
            self.keyword_overlap(item.tags(), item.content_preview(), task_keywords);
        let confidence_score = item.confidence().clamp(0.0, 1.0);
        let recency_score = self.recency_score(item.last_accessed());

        let mut score = keyword_score * self.config.keyword_weight
            + confidence_score * self.config.confidence_weight
            + recency_score * self.config.recency_weight;

        if is_skill {
            score += self.config.skill_bonus;
        }

        score.clamp(0.0, 1.0)
    }

    /// Select items that fit within the token budget.
    ///
    /// Items are sorted by score (descending) and added until the budget
    /// is exhausted. Items below the minimum threshold are filtered out.
    /// Knowledge items are capped at `knowledge_max_pct` of the budget.
    ///
    /// # Arguments
    ///
    /// * `items` - Candidate items with pre-computed scores.
    /// * `token_budget` - Maximum total tokens for selected items.
    pub fn select(&self, mut items: Vec<ScoredItem>, token_budget: usize) -> Vec<ScoredItem> {
        // Filter by threshold
        items.retain(|item| item.score >= self.config.min_threshold);

        // Sort by score descending
        items.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let knowledge_budget = (token_budget as f64 * self.config.knowledge_max_pct) as usize;
        let mut total_tokens = 0usize;
        let mut knowledge_tokens = 0usize;
        let mut selected = Vec::new();

        for item in items {
            // Check total budget
            if total_tokens + item.tokens > token_budget {
                continue;
            }

            // Check knowledge budget
            if matches!(item.source, ItemSource::Knowledge(_)) {
                if knowledge_tokens + item.tokens > knowledge_budget {
                    continue;
                }
                knowledge_tokens += item.tokens;
            }

            total_tokens += item.tokens;
            selected.push(item);
        }

        selected
    }

    /// Compute keyword overlap between item tags/content and task keywords.
    fn keyword_overlap(
        &self,
        tags: &[String],
        content_preview: &str,
        task_keywords: &HashSet<String>,
    ) -> f64 {
        if task_keywords.is_empty() {
            return 0.0;
        }

        let item_words: HashSet<String> = tags
            .iter()
            .map(|t| t.to_lowercase())
            .chain(
                content_preview
                    .to_lowercase()
                    .split_whitespace()
                    .map(|s| s.to_string()),
            )
            .collect();

        let overlap = task_keywords
            .iter()
            .filter(|kw| item_words.contains(kw.as_str()))
            .count();

        (overlap as f64 / task_keywords.len() as f64).clamp(0.0, 1.0)
    }

    /// Compute recency score (1.0 for recent, decaying toward 0.0).
    ///
    /// Uses exponential decay with a half-life of 30 days.
    fn recency_score(&self, last_accessed: DateTime<Utc>) -> f64 {
        let days_ago = (Utc::now() - last_accessed).num_days().max(0) as f64;
        // Exponential decay: halves every 30 days
        (-(days_ago / 30.0) * 0.693).exp().clamp(0.0, 1.0)
    }

    /// Extract keywords from a task description.
    ///
    /// Filters out common English stop words and short tokens (2 chars or less).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gateway_core::agent::context::relevance::RelevanceScorer;
    ///
    /// let keywords = RelevanceScorer::extract_keywords("send an email to the user");
    /// assert!(keywords.contains("send"));
    /// assert!(keywords.contains("email"));
    /// assert!(!keywords.contains("the"));
    /// ```
    pub fn extract_keywords(task: &str) -> HashSet<String> {
        // Common stop words to filter out
        let stop_words: HashSet<&str> = [
            "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has",
            "had", "do", "does", "did", "will", "would", "could", "should", "may", "might", "can",
            "shall", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into",
            "through", "during", "before", "after", "and", "but", "or", "not", "no", "if", "then",
            "else", "when", "this", "that", "these", "those", "it", "its", "i", "me", "my", "we",
            "our", "you", "your", "he", "she", "they",
        ]
        .into_iter()
        .collect();

        task.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2 && !stop_words.contains(w))
            .map(|w| w.to_string())
            .collect()
    }

    /// Get the scorer configuration.
    pub fn config(&self) -> &RelevanceScorerConfig {
        &self.config
    }
}

impl Default for RelevanceScorer {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Estimate token count for a string (rough approximation: ~4 chars per token).
pub fn estimate_tokens(content: &str) -> usize {
    content.len() / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestScorable {
        tags: Vec<String>,
        content: String,
        confidence: f64,
        last_accessed: DateTime<Utc>,
    }

    impl Scorable for TestScorable {
        fn tags(&self) -> &[String] {
            &self.tags
        }
        fn content_preview(&self) -> &str {
            &self.content
        }
        fn confidence(&self) -> f64 {
            self.confidence
        }
        fn last_accessed(&self) -> DateTime<Utc> {
            self.last_accessed
        }
    }

    fn make_scorable(tags: &[&str], content: &str, confidence: f64) -> TestScorable {
        TestScorable {
            tags: tags.iter().map(|s| s.to_string()).collect(),
            content: content.to_string(),
            confidence,
            last_accessed: Utc::now(),
        }
    }

    #[test]
    fn test_extract_keywords() {
        let keywords = RelevanceScorer::extract_keywords("send an email to the user");
        assert!(keywords.contains("send"));
        assert!(keywords.contains("email"));
        assert!(keywords.contains("user"));
        assert!(!keywords.contains("the"));
        assert!(!keywords.contains("an"));
        assert!(!keywords.contains("to"));
    }

    #[test]
    fn test_score_with_keyword_match() {
        let scorer = RelevanceScorer::with_defaults();
        let item = make_scorable(&["email", "gmail"], "Send emails via Gmail", 0.8);
        let keywords = RelevanceScorer::extract_keywords("send email via gmail");

        let score = scorer.score(&item, &keywords, false);
        assert!(
            score > 0.5,
            "Score should be high for matching keywords: {}",
            score
        );
    }

    #[test]
    fn test_score_with_no_match() {
        let scorer = RelevanceScorer::with_defaults();
        let item = make_scorable(&["kubernetes", "deploy"], "Deploy to k8s cluster", 0.5);
        let keywords = RelevanceScorer::extract_keywords("send email via gmail");

        let score = scorer.score(&item, &keywords, false);
        assert!(
            score < 0.3,
            "Score should be low for non-matching: {}",
            score
        );
    }

    #[test]
    fn test_skill_bonus() {
        let scorer = RelevanceScorer::with_defaults();
        let item = make_scorable(&["email"], "Email tool", 0.5);
        let keywords = RelevanceScorer::extract_keywords("send email");

        let score_no_bonus = scorer.score(&item, &keywords, false);
        let score_with_bonus = scorer.score(&item, &keywords, true);

        assert!(score_with_bonus > score_no_bonus);
        assert!((score_with_bonus - score_no_bonus - 0.1).abs() < 0.01);
    }

    #[test]
    fn test_select_within_budget() {
        let scorer = RelevanceScorer::with_defaults();
        let items = vec![
            ScoredItem {
                content: "Item A".to_string(),
                tokens: 100,
                score: 0.9,
                source: ItemSource::Skill("skill-a".to_string()),
            },
            ScoredItem {
                content: "Item B".to_string(),
                tokens: 100,
                score: 0.8,
                source: ItemSource::Skill("skill-b".to_string()),
            },
            ScoredItem {
                content: "Item C".to_string(),
                tokens: 100,
                score: 0.7,
                source: ItemSource::Skill("skill-c".to_string()),
            },
        ];

        let selected = scorer.select(items, 250);
        assert_eq!(
            selected.len(),
            2,
            "Should select 2 items within 250 token budget"
        );
        assert_eq!(selected[0].score, 0.9);
        assert_eq!(selected[1].score, 0.8);
    }

    #[test]
    fn test_select_filters_below_threshold() {
        let scorer = RelevanceScorer::with_defaults();
        let items = vec![
            ScoredItem {
                content: "High".to_string(),
                tokens: 50,
                score: 0.9,
                source: ItemSource::Skill("high".to_string()),
            },
            ScoredItem {
                content: "Low".to_string(),
                tokens: 50,
                score: 0.1,
                source: ItemSource::Skill("low".to_string()),
            },
        ];

        let selected = scorer.select(items, 1000);
        assert_eq!(selected.len(), 1, "Low score item should be filtered");
        assert_eq!(selected[0].score, 0.9);
    }

    #[test]
    fn test_knowledge_budget_cap() {
        let scorer = RelevanceScorer::with_defaults(); // knowledge_max_pct = 0.4
        let items = vec![
            ScoredItem {
                content: "K1".to_string(),
                tokens: 300,
                score: 0.9,
                source: ItemSource::Knowledge("cat".to_string()),
            },
            ScoredItem {
                content: "K2".to_string(),
                tokens: 300,
                score: 0.8,
                source: ItemSource::Knowledge("cat".to_string()),
            },
            ScoredItem {
                content: "S1".to_string(),
                tokens: 200,
                score: 0.7,
                source: ItemSource::Skill("skill".to_string()),
            },
        ];

        // Total budget 1000, knowledge budget 400
        let selected = scorer.select(items, 1000);

        // K1 (300 tokens) fits in knowledge budget (400)
        // K2 (300 tokens) would exceed knowledge budget (300+300=600 > 400)
        // S1 (200 tokens) is a skill, not limited by knowledge budget
        let knowledge_count = selected
            .iter()
            .filter(|i| matches!(i.source, ItemSource::Knowledge(_)))
            .count();
        assert_eq!(
            knowledge_count, 1,
            "Only 1 knowledge item should fit in 40% budget"
        );
    }

    #[test]
    fn test_recency_score() {
        let scorer = RelevanceScorer::with_defaults();

        let recent = scorer.recency_score(Utc::now());
        let old = scorer.recency_score(Utc::now() - chrono::Duration::days(90));

        assert!(recent > old, "Recent items should score higher");
        assert!(recent > 0.9, "Very recent items should score close to 1.0");
    }

    #[test]
    fn test_select_empty() {
        let scorer = RelevanceScorer::with_defaults();
        let selected = scorer.select(vec![], 1000);
        assert!(selected.is_empty());
    }

    #[test]
    fn test_item_source_display() {
        assert_eq!(
            format!("{}", ItemSource::Skill("commit".to_string())),
            "[Skill] commit"
        );
        assert_eq!(
            format!("{}", ItemSource::Knowledge("tool_guidance".to_string())),
            "[Knowledge] tool_guidance"
        );
        assert_eq!(
            format!("{}", ItemSource::Memory("user_prefs".to_string())),
            "[Memory] user_prefs"
        );
    }

    #[test]
    fn test_scored_item_serialization() {
        let item = ScoredItem {
            content: "Test content".to_string(),
            tokens: 50,
            score: 0.85,
            source: ItemSource::Skill("test".to_string()),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: ScoredItem = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.score, 0.85);
    }

    #[test]
    fn test_scorer_config_serialization() {
        let config = RelevanceScorerConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RelevanceScorerConfig = serde_json::from_str(&json).unwrap();
        assert!((parsed.min_threshold - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello world!"), 3); // 12 chars / 4
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
    }

    #[test]
    fn test_extract_keywords_filters_short_words() {
        let keywords = RelevanceScorer::extract_keywords("go to my app");
        assert!(!keywords.contains("go"));
        assert!(!keywords.contains("to"));
        assert!(!keywords.contains("my"));
        assert!(keywords.contains("app"));
    }

    #[test]
    fn test_extract_keywords_handles_punctuation() {
        let keywords = RelevanceScorer::extract_keywords("send email, then check status.");
        assert!(keywords.contains("send"));
        assert!(keywords.contains("email"));
        assert!(keywords.contains("check"));
        assert!(keywords.contains("status"));
    }

    #[test]
    fn test_score_empty_keywords() {
        let scorer = RelevanceScorer::with_defaults();
        let item = make_scorable(&["email"], "Email tool", 0.8);
        let empty_keywords: HashSet<String> = HashSet::new();

        let score = scorer.score(&item, &empty_keywords, false);
        // keyword_overlap = 0.0, confidence = 0.8 * 0.3 = 0.24, recency ~= 0.1
        assert!(
            score > 0.0,
            "Score should still account for confidence and recency"
        );
        assert!(
            score < 0.5,
            "Score should be moderate without keyword match: {}",
            score
        );
    }

    #[test]
    fn test_custom_config() {
        let config = RelevanceScorerConfig {
            min_threshold: 0.5,
            keyword_weight: 0.4,
            confidence_weight: 0.4,
            recency_weight: 0.2,
            skill_bonus: 0.2,
            knowledge_max_pct: 0.6,
        };
        let scorer = RelevanceScorer::new(config);

        assert!((scorer.config().min_threshold - 0.5).abs() < f64::EPSILON);
        assert!((scorer.config().skill_bonus - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn test_default_scorer() {
        let scorer = RelevanceScorer::default();
        assert!((scorer.config().min_threshold - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_select_skips_items_exceeding_budget_individually() {
        let scorer = RelevanceScorer::with_defaults();
        let items = vec![
            ScoredItem {
                content: "Big item".to_string(),
                tokens: 500,
                score: 0.9,
                source: ItemSource::Skill("big".to_string()),
            },
            ScoredItem {
                content: "Small item".to_string(),
                tokens: 50,
                score: 0.8,
                source: ItemSource::Skill("small".to_string()),
            },
        ];

        // Budget only fits the small item when the big one is tried first
        let selected = scorer.select(items, 100);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].content, "Small item");
    }

    #[test]
    fn test_memory_source_in_select() {
        let scorer = RelevanceScorer::with_defaults();
        let items = vec![
            ScoredItem {
                content: "Memory item".to_string(),
                tokens: 100,
                score: 0.9,
                source: ItemSource::Memory("user_prefs".to_string()),
            },
            ScoredItem {
                content: "Knowledge item".to_string(),
                tokens: 100,
                score: 0.8,
                source: ItemSource::Knowledge("tips".to_string()),
            },
        ];

        let selected = scorer.select(items, 1000);
        assert_eq!(selected.len(), 2);
        // Memory items are not capped by knowledge budget
        assert!(matches!(selected[0].source, ItemSource::Memory(_)));
    }

    #[test]
    fn test_recency_decay_30_days() {
        let scorer = RelevanceScorer::with_defaults();
        let thirty_days_ago = Utc::now() - chrono::Duration::days(30);
        let score = scorer.recency_score(thirty_days_ago);
        // After 30 days, score should be approximately 0.5 (half-life)
        assert!(
            (score - 0.5).abs() < 0.05,
            "Score after 30 days should be ~0.5, got {}",
            score
        );
    }

    #[test]
    fn test_item_source_equality() {
        assert_eq!(
            ItemSource::Skill("a".to_string()),
            ItemSource::Skill("a".to_string())
        );
        assert_ne!(
            ItemSource::Skill("a".to_string()),
            ItemSource::Knowledge("a".to_string())
        );
        assert_ne!(
            ItemSource::Skill("a".to_string()),
            ItemSource::Skill("b".to_string())
        );
    }

    #[test]
    fn test_score_clamped_to_one() {
        // Even with skill bonus, score should not exceed 1.0
        let config = RelevanceScorerConfig {
            min_threshold: 0.0,
            keyword_weight: 0.6,
            confidence_weight: 0.3,
            recency_weight: 0.1,
            skill_bonus: 0.5, // Large bonus
            knowledge_max_pct: 0.4,
        };
        let scorer = RelevanceScorer::new(config);
        let item = make_scorable(&["email", "gmail"], "Send emails via Gmail", 1.0);
        let keywords = RelevanceScorer::extract_keywords("email gmail");

        let score = scorer.score(&item, &keywords, true);
        assert!(
            score <= 1.0,
            "Score should be clamped to 1.0, got {}",
            score
        );
    }
}
