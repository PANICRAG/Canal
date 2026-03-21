//! LLM-based task classifier using Function Calling.
//!
//! Classifies user tasks into categories (simple, multi_step, planning, expert)
//! to determine the best execution mode. Uses `classify_task` tool with
//! `ToolChoice::Tool` for reliable structured output.
//!
//! # Architecture
//!
//! - Uses cheapest/fastest model (e.g., qwen-turbo) for low latency
//! - In-memory cache with LRU eviction for repeated queries
//! - Returns `Option<ClassificationResult>` — `None` triggers keyword fallback
//! - Maps categories to `CollaborationMode` via `to_collaboration_mode()`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::llm::router::{
    ChatRequest, ChatResponse, ContentBlock, Message, ToolChoice, ToolDefinition,
};
use crate::llm::LlmRouter;

// ============================================================================
// Types
// ============================================================================

/// Classification result from the LLM classifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResult {
    /// Task category.
    pub category: TaskCategory,
    /// Confidence in the classification (0.0 - 1.0).
    pub confidence: f64,
    /// Brief explanation of why this category was chosen.
    pub reasoning: String,
}

/// Task category for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCategory {
    /// Single action, greeting, Q&A.
    Simple,
    /// 2-5 sequential steps.
    MultiStep,
    /// Complex tasks needing decomposition.
    Planning,
    /// Specialist knowledge or review.
    Expert,
}

/// Configuration for the task classifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifierConfig {
    /// Model to use for classification.
    pub model: String,
    /// Temperature (low for consistency).
    pub temperature: f32,
    /// Max tokens (small — only need classification output).
    pub max_tokens: u32,
    /// System prompt for classifier.
    pub system_prompt: String,
    /// Low-confidence threshold — below this, fallback to keyword.
    pub confidence_threshold: f64,
    /// Cache size limit (LRU eviction beyond this).
    pub cache_size: usize,
}

impl Default for ClassifierConfig {
    fn default() -> Self {
        Self {
            model: "qwen-turbo".into(),
            temperature: 0.1,
            max_tokens: 200,
            system_prompt: DEFAULT_CLASSIFIER_PROMPT.into(),
            confidence_threshold: 0.6,
            cache_size: 500,
        }
    }
}

const DEFAULT_CLASSIFIER_PROMPT: &str = r#"You are a task classifier. Analyze the user's request and determine the best execution mode.

CATEGORIES:
- simple: Greeting, Q&A, translation, single-action tasks (e.g., "你好", "what is 2+2", "翻译这段话")
- multi_step: Tasks requiring 2-5 sequential steps (e.g., "打开Gmail然后写邮件", "先搜索再总结")
- planning: Complex tasks needing decomposition into many steps with dependencies (e.g., "设计并实现一个API", "分步骤完成以下任务...")
- expert: Tasks requiring specialist knowledge, review, or multi-perspective analysis (e.g., "审查这个方案", "需要专家分析")

RULES:
1. When in doubt between simple and multi_step, choose simple
2. planning requires 5+ steps or explicit planning keywords
3. expert requires review/audit/specialist analysis
4. Set confidence based on how clear the classification is

You MUST call the classify_task tool. Do not respond with text."#;

// ============================================================================
// TaskClassifier
// ============================================================================

/// LLM-based task classifier using Function Calling.
pub struct TaskClassifier {
    llm_router: Arc<LlmRouter>,
    config: ClassifierConfig,
    /// Cache: message_hash → ClassificationResult.
    cache: Arc<RwLock<HashMap<u64, ClassificationResult>>>,
}

impl TaskClassifier {
    /// Create a new task classifier.
    pub fn new(llm_router: Arc<LlmRouter>, config: ClassifierConfig) -> Self {
        Self {
            llm_router,
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Classify a task using LLM Function Calling.
    ///
    /// Returns `None` if LLM call fails (caller should fallback to keyword-based).
    pub async fn classify(&self, task: &str) -> Option<ClassificationResult> {
        // 1. Check cache
        let hash = Self::compute_hash(task);
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&hash) {
                tracing::debug!(
                    category = ?cached.category,
                    confidence = cached.confidence,
                    "Task classification cache hit"
                );
                return Some(cached.clone());
            }
        }

        // 2. Call LLM
        let request = ChatRequest {
            messages: vec![
                Message::text("system", &self.config.system_prompt),
                Message::text("user", task),
            ],
            model: Some(self.config.model.clone()),
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            tools: vec![Self::classify_task_tool_def()],
            tool_choice: Some(ToolChoice::Tool {
                name: "classify_task".into(),
            }),
            ..Default::default()
        };

        let response = match self.llm_router.route(request).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "LLM classification failed, will use keyword fallback");
                return None;
            }
        };

        // 3. Parse tool_use response
        let result = Self::parse_classification_response(&response)?;

        // 4. Cache result (with eviction)
        {
            let mut cache = self.cache.write().await;
            if cache.len() >= self.config.cache_size {
                // Simple eviction: clear half the cache
                let keys: Vec<u64> = cache.keys().take(cache.len() / 2).copied().collect();
                for k in keys {
                    cache.remove(&k);
                }
            }
            cache.insert(hash, result.clone());
        }

        tracing::info!(
            category = ?result.category,
            confidence = result.confidence,
            reasoning = %result.reasoning,
            "Task classified via LLM"
        );

        Some(result)
    }

    /// Map ClassificationResult to CollaborationMode.
    #[cfg(feature = "collaboration")]
    pub fn to_collaboration_mode(
        result: &ClassificationResult,
    ) -> crate::collaboration::CollaborationMode {
        use crate::collaboration::CollaborationMode;

        match result.category {
            TaskCategory::Simple => CollaborationMode::Direct,
            TaskCategory::MultiStep => {
                if result.confidence >= 0.6 {
                    CollaborationMode::Swarm {
                        initial_agent: "primary".into(),
                        handoff_rules: vec![],
                        agent_models: std::collections::HashMap::new(),
                    }
                } else {
                    CollaborationMode::Direct // Low confidence → conservative
                }
            }
            TaskCategory::Planning => CollaborationMode::PlanExecute,
            TaskCategory::Expert => CollaborationMode::Expert {
                supervisor: "coordinator".into(),
                specialists: vec!["executor".into(), "reviewer".into()],
                supervisor_model: None,
                default_specialist_model: None,
                specialist_models: std::collections::HashMap::new(),
            },
        }
    }

    /// Compute a hash for cache key.
    pub fn compute_hash(text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    /// Build the classify_task tool definition.
    pub fn classify_task_tool_def() -> ToolDefinition {
        ToolDefinition {
            name: "classify_task".into(),
            description: "Classify a user task to determine the best execution mode".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "category": {
                        "type": "string",
                        "enum": ["simple", "multi_step", "planning", "expert"],
                        "description": "simple=single action, multi_step=sequential steps, planning=needs decomposition, expert=needs specialist review"
                    },
                    "confidence": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Confidence in the classification (0-1)"
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "Brief explanation of why this category was chosen (1 sentence)"
                    }
                },
                "required": ["category", "confidence", "reasoning"]
            }),
        }
    }

    /// Parse the LLM response to extract ClassificationResult.
    pub fn parse_classification_response(response: &ChatResponse) -> Option<ClassificationResult> {
        let choice = response.choices.first()?;

        // Try extracting from content_blocks (tool_use)
        for block in &choice.message.content_blocks {
            if let ContentBlock::ToolUse { name, input, .. } = block {
                if name == "classify_task" {
                    return serde_json::from_value::<ClassificationResult>(input.clone()).ok();
                }
            }
        }

        // Fallback: try parsing JSON from text content
        let text = &choice.message.content;
        let json_start = text.find('{')?;
        let json_end = text.rfind('}')?;
        if json_start < json_end {
            serde_json::from_str::<ClassificationResult>(&text[json_start..=json_end]).ok()
        } else {
            None
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::router::{Choice, Usage};

    fn mock_usage() -> Usage {
        Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }
    }

    #[test]
    fn test_classifier_config_default() {
        let config = ClassifierConfig::default();
        assert_eq!(config.model, "qwen-turbo");
        assert!((config.temperature - 0.1).abs() < f32::EPSILON);
        assert_eq!(config.max_tokens, 200);
        assert!((config.confidence_threshold - 0.6).abs() < f64::EPSILON);
        assert_eq!(config.cache_size, 500);
        assert!(!config.system_prompt.is_empty());
    }

    #[test]
    fn test_classification_result_serde() {
        let result = ClassificationResult {
            category: TaskCategory::Planning,
            confidence: 0.85,
            reasoning: "Complex task with multiple steps".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ClassificationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.category, TaskCategory::Planning);
        assert!((parsed.confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_task_category_serde_all_variants() {
        for (variant, expected_str) in [
            (TaskCategory::Simple, "\"simple\""),
            (TaskCategory::MultiStep, "\"multi_step\""),
            (TaskCategory::Planning, "\"planning\""),
            (TaskCategory::Expert, "\"expert\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_str);
            let parsed: TaskCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }

    #[cfg(feature = "collaboration")]
    #[test]
    fn test_to_collaboration_mode_simple() {
        use crate::collaboration::CollaborationMode;
        let result = ClassificationResult {
            category: TaskCategory::Simple,
            confidence: 0.9,
            reasoning: "".into(),
        };
        assert!(matches!(
            TaskClassifier::to_collaboration_mode(&result),
            CollaborationMode::Direct
        ));
    }

    #[cfg(feature = "collaboration")]
    #[test]
    fn test_to_collaboration_mode_multi_step_high_confidence() {
        use crate::collaboration::CollaborationMode;
        let result = ClassificationResult {
            category: TaskCategory::MultiStep,
            confidence: 0.8,
            reasoning: "".into(),
        };
        assert!(matches!(
            TaskClassifier::to_collaboration_mode(&result),
            CollaborationMode::Swarm { .. }
        ));
    }

    #[cfg(feature = "collaboration")]
    #[test]
    fn test_to_collaboration_mode_multi_step_low_confidence_fallback_direct() {
        use crate::collaboration::CollaborationMode;
        let result = ClassificationResult {
            category: TaskCategory::MultiStep,
            confidence: 0.4,
            reasoning: "".into(),
        };
        assert!(matches!(
            TaskClassifier::to_collaboration_mode(&result),
            CollaborationMode::Direct
        ));
    }

    #[cfg(feature = "collaboration")]
    #[test]
    fn test_to_collaboration_mode_planning() {
        use crate::collaboration::CollaborationMode;
        let result = ClassificationResult {
            category: TaskCategory::Planning,
            confidence: 0.7,
            reasoning: "".into(),
        };
        assert!(matches!(
            TaskClassifier::to_collaboration_mode(&result),
            CollaborationMode::PlanExecute
        ));
    }

    #[cfg(feature = "collaboration")]
    #[test]
    fn test_to_collaboration_mode_expert() {
        use crate::collaboration::CollaborationMode;
        let result = ClassificationResult {
            category: TaskCategory::Expert,
            confidence: 0.9,
            reasoning: "".into(),
        };
        assert!(matches!(
            TaskClassifier::to_collaboration_mode(&result),
            CollaborationMode::Expert { .. }
        ));
    }

    #[test]
    fn test_compute_hash_deterministic() {
        let h1 = TaskClassifier::compute_hash("hello world");
        let h2 = TaskClassifier::compute_hash("hello world");
        let h3 = TaskClassifier::compute_hash("hello world!");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_classify_task_tool_def_valid() {
        let tool = TaskClassifier::classify_task_tool_def();
        assert_eq!(tool.name, "classify_task");
        let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(schema_str.contains("category"));
        assert!(schema_str.contains("confidence"));
        assert!(schema_str.contains("reasoning"));
        assert!(schema_str.contains("simple"));
        assert!(schema_str.contains("multi_step"));
        assert!(schema_str.contains("planning"));
        assert!(schema_str.contains("expert"));
    }

    #[test]
    fn test_parse_classification_from_tool_use() {
        let response = ChatResponse {
            id: "test".into(),
            model: "test".into(),
            choices: vec![Choice {
                index: 0,
                message: Message::with_blocks(
                    "assistant",
                    vec![ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "classify_task".into(),
                        input: serde_json::json!({
                            "category": "planning",
                            "confidence": 0.85,
                            "reasoning": "Task requires multiple steps with dependencies"
                        }),
                    }],
                ),
                finish_reason: "tool_use".into(),
                stop_reason: None,
            }],
            usage: mock_usage(),
        };
        let result = TaskClassifier::parse_classification_response(&response).unwrap();
        assert_eq!(result.category, TaskCategory::Planning);
        assert!((result.confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_classification_from_text_fallback() {
        let response = ChatResponse {
            id: "test".into(),
            model: "test".into(),
            choices: vec![Choice {
                index: 0,
                message: Message::text(
                    "assistant",
                    r#"Based on analysis: {"category":"simple","confidence":0.95,"reasoning":"Just a greeting"}"#,
                ),
                finish_reason: "end_turn".into(),
                stop_reason: None,
            }],
            usage: mock_usage(),
        };
        let result = TaskClassifier::parse_classification_response(&response).unwrap();
        assert_eq!(result.category, TaskCategory::Simple);
    }

    #[test]
    fn test_parse_classification_invalid_returns_none() {
        let response = ChatResponse {
            id: "test".into(),
            model: "test".into(),
            choices: vec![Choice {
                index: 0,
                message: Message::text("assistant", "I can't classify this"),
                finish_reason: "end_turn".into(),
                stop_reason: None,
            }],
            usage: mock_usage(),
        };
        assert!(TaskClassifier::parse_classification_response(&response).is_none());
    }
}
