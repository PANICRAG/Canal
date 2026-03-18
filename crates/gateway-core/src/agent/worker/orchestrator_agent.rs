//! Orchestrator Agent - Lead agent wrapper for the Orchestrator-Worker pattern
//!
//! Encapsulates the lead agent (typically Opus) that decomposes complex tasks
//! into worker specifications, delegates execution to WorkerManager, and
//! optionally synthesizes the combined results.

use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

use super::manager::WorkerManager;
use super::types::{OrchestratedResult, OrchestratorConfig, WorkerSpec, WorkerSpecJson};
use crate::chat::StreamEvent;
use crate::error::Result;
use crate::llm::{ChatRequest, LlmRouter, Message};

/// Orchestrator Agent - the lead agent in the Orchestrator-Worker pattern
///
/// Responsible for:
/// 1. Analyzing the task and decomposing it into worker specifications
/// 2. Dispatching workers via WorkerManager
/// 3. Optionally synthesizing results from all workers
pub struct OrchestratorAgent {
    llm_router: Arc<LlmRouter>,
    worker_manager: Arc<WorkerManager>,
    config: OrchestratorConfig,
}

impl OrchestratorAgent {
    /// Create a new OrchestratorAgent
    pub fn new(
        llm_router: Arc<LlmRouter>,
        worker_manager: Arc<WorkerManager>,
        config: OrchestratorConfig,
    ) -> Self {
        Self {
            llm_router,
            worker_manager,
            config,
        }
    }

    /// Execute a task using the Orchestrator-Worker pattern
    ///
    /// The lead agent analyzes the task, creates worker specifications,
    /// dispatches them for parallel execution, and synthesizes the results.
    pub async fn execute(
        &self,
        task: &str,
        stream_tx: Option<broadcast::Sender<StreamEvent>>,
    ) -> Result<OrchestratedResult> {
        // Step 1: Ask the lead agent to decompose the task
        if let Some(ref tx) = stream_tx {
            let _ = tx.send(StreamEvent::thinking(
                "Analyzing task and creating worker plan...",
            ));
        }

        let worker_specs_json = self.decompose_task(task).await?;

        // Step 2: Convert JSON specs to WorkerSpecs
        let mut name_to_id = std::collections::HashMap::new();
        // First pass: assign IDs
        for spec in &worker_specs_json {
            name_to_id.insert(spec.name.clone(), Uuid::new_v4());
        }

        let worker_specs: Vec<WorkerSpec> = worker_specs_json
            .iter()
            .map(|json_spec| {
                let mut spec = json_spec.to_worker_spec(&name_to_id);
                // Override the ID with the one we pre-assigned
                if let Some(&id) = name_to_id.get(&json_spec.name) {
                    spec.id = id;
                }
                spec
            })
            .collect();

        if let Some(ref tx) = stream_tx {
            let _ = tx.send(StreamEvent::Custom {
                event_type: "worker_progress".to_string(),
                data: serde_json::json!({
                    "phase": "dispatching",
                    "worker_count": worker_specs.len(),
                    "workers": worker_specs.iter().map(|s| &s.name).collect::<Vec<_>>(),
                }),
            });
        }

        // Step 3: Dispatch workers
        let mut result = self
            .worker_manager
            .execute_workers(worker_specs, stream_tx.clone())
            .await?;

        // Step 4: Optionally synthesize results
        if self.config.synthesize_results && !result.worker_results.is_empty() {
            if let Some(ref tx) = stream_tx {
                let _ = tx.send(StreamEvent::thinking("Synthesizing worker results..."));
            }

            let synthesized = self
                .worker_manager
                .synthesize_results(&result.worker_results, None)
                .await?;

            result.synthesized_output = Some(synthesized);
        }

        Ok(result)
    }

    /// Execute pre-defined worker specs (from a plan step)
    pub async fn execute_specs(
        &self,
        worker_specs_json: Vec<WorkerSpecJson>,
        synthesize_results: bool,
        synthesis_prompt: Option<&str>,
        stream_tx: Option<broadcast::Sender<StreamEvent>>,
    ) -> Result<OrchestratedResult> {
        // Convert JSON specs to WorkerSpecs
        let mut name_to_id = std::collections::HashMap::new();
        for spec in &worker_specs_json {
            name_to_id.insert(spec.name.clone(), Uuid::new_v4());
        }

        let worker_specs: Vec<WorkerSpec> = worker_specs_json
            .iter()
            .map(|json_spec| {
                let mut spec = json_spec.to_worker_spec(&name_to_id);
                if let Some(&id) = name_to_id.get(&json_spec.name) {
                    spec.id = id;
                }
                spec
            })
            .collect();

        // Dispatch workers
        let mut result = self
            .worker_manager
            .execute_workers(worker_specs, stream_tx)
            .await?;

        // Optionally synthesize
        if synthesize_results && !result.worker_results.is_empty() {
            let synthesized = self
                .worker_manager
                .synthesize_results(&result.worker_results, synthesis_prompt)
                .await?;
            result.synthesized_output = Some(synthesized);
        }

        Ok(result)
    }

    /// Ask the lead agent to decompose a task into worker specifications
    async fn decompose_task(&self, task: &str) -> Result<Vec<WorkerSpecJson>> {
        let prompt = format!(
            r#"You are a lead orchestrator agent. Your job is to decompose the following task
into multiple worker subtasks that can be executed in parallel (where possible).

Task: "{task}"

Create a JSON array of worker specifications. Each worker should have:
- "name": A unique identifier for this worker
- "prompt": Detailed instructions for the worker
- "agent_type": The type of worker ("Explore", "Bash", "Code", "general-purpose")
- "model": Optional model override (default: sonnet)
- "max_turns": Optional max turns (default: 10)
- "allowed_tools": Optional list of tools the worker can use
- "depends_on": List of worker names this worker depends on (for ordering)
- "priority": Priority level (0=normal, higher=more important)

Return ONLY a valid JSON array of worker specifications.
Example:
[
  {{
    "name": "analyzer-1",
    "prompt": "Analyze files 1-5 for patterns",
    "agent_type": "Explore",
    "depends_on": [],
    "priority": 0
  }},
  {{
    "name": "analyzer-2",
    "prompt": "Analyze files 6-10 for patterns",
    "agent_type": "Explore",
    "depends_on": [],
    "priority": 0
  }},
  {{
    "name": "synthesizer",
    "prompt": "Combine analysis results",
    "agent_type": "general-purpose",
    "depends_on": ["analyzer-1", "analyzer-2"],
    "priority": 1
  }}
]"#
        );

        let request = ChatRequest {
            messages: vec![Message::text("user", prompt)],
            model: Some(self.config.lead_model.clone()),
            max_tokens: Some(4096),
            temperature: Some(0.3),
            stream: false,
            ..Default::default()
        };

        let response = self.llm_router.route(request).await?;

        let content = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        // Parse the JSON array
        let mut specs: Vec<WorkerSpecJson> = serde_json::from_str(&content).map_err(|e| {
            crate::error::Error::InvalidInput(format!(
                "Failed to parse worker specifications from LLM: {}. Response: {}",
                e,
                content.chars().take(500).collect::<String>()
            ))
        })?;

        // R1-H12: Validate and clamp every spec parsed from LLM output.
        // This prevents prompt-injection from escalating privileges via crafted
        // worker specs (unexpected models, excessive tools/turns, etc.).
        for spec in &mut specs {
            spec.validate_and_clamp();
        }

        // Log an aggregate summary so operators can spot anomalies in dashboards
        tracing::info!(
            target: "orchestrator.security",
            worker_count = specs.len(),
            models = ?specs.iter().filter_map(|s| s.model.as_deref()).collect::<Vec<_>>(),
            tools_requested = specs.iter().filter_map(|s| s.allowed_tools.as_ref().map(|t| t.len())).sum::<usize>(),
            "R1-H12: validated LLM-generated worker specs"
        );

        Ok(specs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmConfig;

    #[test]
    fn test_orchestrator_agent_creation() {
        let llm_router = Arc::new(LlmRouter::new(LlmConfig::default()));
        let config = OrchestratorConfig::default();
        let manager = Arc::new(WorkerManager::new(config.clone(), llm_router.clone()));

        let _agent = OrchestratorAgent::new(llm_router, manager, config);
        // Just verify it can be created without panic
        assert!(true);
    }
}
