//! Task-type based routing strategy.
//!
//! Routes requests by matching an optional `task_type` field against a list of
//! rules defined in the model profile. Each rule maps a task type string
//! (e.g., "code", "analysis", "summarisation") to a preferred provider/model
//! pair. When no rule matches, the strategy falls back to the profile's
//! primary target.
//!
//! ## How task_type is resolved
//!
//! The `ChatRequest` will carry an optional `task_type: Option<String>` field.
//! Until that field is added to the struct, the strategy extracts the task type
//! from the request's `model` field when it is prefixed with `task:` (e.g.,
//! `model: Some("task:code")`). This is a transitional convention — once
//! `ChatRequest.task_type` lands, the strategy will prefer it.

use async_trait::async_trait;

use super::{ModelProfile, ModelRegistry, RouteDecision, RoutingStrategyHandler};
use crate::error::{Error, Result};
use crate::router::ChatRequest;

/// Strategy that routes based on the request's declared task type.
pub struct TaskTypeRulesStrategy;

impl TaskTypeRulesStrategy {
    /// Extract the task type from a `ChatRequest`.
    ///
    /// Priority order:
    /// 1. `request.task_type` field (preferred).
    /// 2. The `model` field when formatted as `"task:<type>"` (transitional).
    /// 3. `None` — no task type could be determined.
    fn extract_task_type(request: &ChatRequest) -> Option<String> {
        // Prefer the explicit task_type field
        if let Some(ref tt) = request.task_type {
            if !tt.is_empty() {
                return Some(tt.clone());
            }
        }
        // Transitional fallback: look for `task:` prefix in the model field.
        if let Some(ref model) = request.model {
            if let Some(task) = model.strip_prefix("task:") {
                if !task.is_empty() {
                    return Some(task.to_string());
                }
            }
        }
        None
    }
}

#[async_trait]
impl RoutingStrategyHandler for TaskTypeRulesStrategy {
    async fn resolve(
        &self,
        profile: &ModelProfile,
        request: &ChatRequest,
        _registry: &ModelRegistry,
    ) -> Result<RouteDecision> {
        let task_type = Self::extract_task_type(request);

        if let Some(ref tt) = task_type {
            // Search for a matching rule (case-insensitive).
            let matched = profile
                .routing
                .task_type_rules
                .iter()
                .find(|rule| rule.task_type.eq_ignore_ascii_case(tt));

            if let Some(rule) = matched {
                tracing::info!(
                    task_type = %tt,
                    provider = %rule.preferred.provider,
                    model = %rule.preferred.model,
                    "Task-type rule matched"
                );
                return Ok(RouteDecision {
                    target: rule.preferred.clone(),
                    reason: format!("task_type_rule:{}", tt),
                    fallback: rule.fallback.clone(),
                });
            }

            tracing::debug!(
                task_type = %tt,
                "No task-type rule matched — using primary"
            );
        }

        // No task type or no matching rule — fall through to primary.
        let primary = profile.routing.primary.clone().ok_or_else(|| {
            Error::Llm("No primary target configured and no task-type rule matched".into())
        })?;

        Ok(RouteDecision {
            target: primary,
            reason: "primary:no_task_type_match".into(),
            fallback: profile.routing.fallback.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{ChatRequest, Message};
    use crate::strategies::{
        ModelProfile, ModelRegistry, ModelTarget, RoutingConfig, TaskTypeRule,
    };

    fn code_target() -> ModelTarget {
        ModelTarget {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        }
    }

    fn analysis_target() -> ModelTarget {
        ModelTarget {
            provider: "openai".into(),
            model: "gpt-4o".into(),
        }
    }

    fn primary_target() -> ModelTarget {
        ModelTarget {
            provider: "anthropic".into(),
            model: "claude-haiku".into(),
        }
    }

    fn make_profile() -> ModelProfile {
        ModelProfile {
            name: "multi-task".into(),
            description: "Profile with task-type rules".into(),
            routing: RoutingConfig {
                primary: Some(primary_target()),
                fallback: None,
                task_type_rules: vec![
                    TaskTypeRule {
                        task_type: "code".into(),
                        preferred: code_target(),
                        fallback: None,
                    },
                    TaskTypeRule {
                        task_type: "analysis".into(),
                        preferred: analysis_target(),
                        fallback: Some(primary_target()),
                    },
                ],
                ..Default::default()
            },
        }
    }

    fn request_with_task(task: &str) -> ChatRequest {
        ChatRequest {
            messages: vec![Message::text("user", "test")],
            model: Some(format!("task:{}", task)),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_matches_code_task() {
        let profile = make_profile();
        let registry = ModelRegistry::new();

        let decision = TaskTypeRulesStrategy
            .resolve(&profile, &request_with_task("code"), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, code_target());
        assert_eq!(decision.reason, "task_type_rule:code");
    }

    #[tokio::test]
    async fn test_matches_analysis_with_fallback() {
        let profile = make_profile();
        let registry = ModelRegistry::new();

        let decision = TaskTypeRulesStrategy
            .resolve(&profile, &request_with_task("analysis"), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, analysis_target());
        assert_eq!(decision.fallback, Some(primary_target()));
        assert_eq!(decision.reason, "task_type_rule:analysis");
    }

    #[tokio::test]
    async fn test_case_insensitive_match() {
        let profile = make_profile();
        let registry = ModelRegistry::new();

        let decision = TaskTypeRulesStrategy
            .resolve(&profile, &request_with_task("CODE"), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, code_target());
    }

    #[tokio::test]
    async fn test_no_match_falls_to_primary() {
        let profile = make_profile();
        let registry = ModelRegistry::new();

        let decision = TaskTypeRulesStrategy
            .resolve(&profile, &request_with_task("unknown"), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, primary_target());
        assert_eq!(decision.reason, "primary:no_task_type_match");
    }

    #[tokio::test]
    async fn test_no_task_type_falls_to_primary() {
        let profile = make_profile();
        let registry = ModelRegistry::new();

        let request = ChatRequest {
            messages: vec![Message::text("user", "hello")],
            ..Default::default()
        };

        let decision = TaskTypeRulesStrategy
            .resolve(&profile, &request, &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, primary_target());
    }

    #[tokio::test]
    async fn test_error_when_no_primary_and_no_match() {
        let profile = ModelProfile {
            name: "empty".into(),
            description: String::new(),
            routing: RoutingConfig::default(),
        };
        let registry = ModelRegistry::new();

        let result = TaskTypeRulesStrategy
            .resolve(&profile, &request_with_task("anything"), &registry)
            .await;

        assert!(result.is_err());
    }
}
