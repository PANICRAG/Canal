//! Orchestrate Tool - Enables the agentic loop to spawn worker agents
//!
//! This tool allows an AgentRunner (in agentic loop mode) to use the
//! Orchestrator-Worker pattern via tool calling, dispatching parallel
//! workers and collecting their results.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::context::ToolContext;
use super::traits::{AgentTool, ToolError, ToolResult};
use crate::agent::worker::manager::WorkerManager;
use crate::agent::worker::types::{WorkerSpec, WorkerSpecJson};

/// Input for the Orchestrate tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrateInput {
    /// Worker specifications to dispatch
    pub workers: Vec<WorkerSpecJson>,
    /// Whether to synthesize results using the lead agent
    #[serde(default = "default_synthesize")]
    pub synthesize_results: bool,
    /// Optional custom prompt for result synthesis
    #[serde(default)]
    pub synthesis_prompt: Option<String>,
}

fn default_synthesize() -> bool {
    true
}

/// Output from the Orchestrate tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrateOutput {
    /// Whether all workers succeeded
    pub all_succeeded: bool,
    /// Number of workers executed
    pub worker_count: usize,
    /// Number of successful workers
    pub success_count: usize,
    /// Synthesized output (if synthesis was requested)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthesized_output: Option<String>,
    /// Individual worker results
    pub worker_results: Vec<WorkerResultSummary>,
    /// Total execution time in milliseconds
    pub total_duration_ms: u64,
}

/// Summary of a single worker's result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerResultSummary {
    pub name: String,
    pub success: bool,
    /// Truncated content (first 2000 chars)
    pub content: String,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Tool for spawning and managing worker agents
pub struct OrchestrateTool {
    worker_manager: Arc<WorkerManager>,
}

impl OrchestrateTool {
    /// Create a new OrchestrateTool with a WorkerManager
    pub fn new(worker_manager: Arc<WorkerManager>) -> Self {
        Self { worker_manager }
    }
}

#[async_trait]
impl AgentTool for OrchestrateTool {
    type Input = OrchestrateInput;
    type Output = OrchestrateOutput;

    fn name(&self) -> &str {
        "Orchestrate"
    }

    fn description(&self) -> &str {
        "Spawn multiple worker agents to execute subtasks in parallel. \
         Workers can have dependencies (DAG ordering) and their results \
         can be synthesized by the lead agent. Use this for complex tasks \
         that benefit from parallel decomposition."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "workers": {
                    "type": "array",
                    "description": "Array of worker specifications",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Unique name for this worker"
                            },
                            "prompt": {
                                "type": "string",
                                "description": "Detailed instructions for the worker"
                            },
                            "agent_type": {
                                "type": "string",
                                "description": "Worker type: Explore, Bash, Code, general-purpose",
                                "default": "general-purpose"
                            },
                            "model": {
                                "type": "string",
                                "description": "Optional model override (default: sonnet)"
                            },
                            "max_turns": {
                                "type": "integer",
                                "description": "Optional max turns"
                            },
                            "allowed_tools": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional list of allowed tools"
                            },
                            "timeout_ms": {
                                "type": "integer",
                                "description": "Optional timeout in milliseconds"
                            },
                            "depends_on": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Names of workers this depends on"
                            },
                            "priority": {
                                "type": "integer",
                                "description": "Priority level (0=normal)",
                                "default": 0
                            }
                        },
                        "required": ["name", "prompt"]
                    }
                },
                "synthesize_results": {
                    "type": "boolean",
                    "description": "Whether to synthesize all results into a unified output",
                    "default": true
                },
                "synthesis_prompt": {
                    "type": "string",
                    "description": "Optional custom prompt for result synthesis"
                }
            },
            "required": ["workers"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        // R1-H23: Spawns worker agents that execute mutating tools (file writes, git, bash)
        true
    }

    fn namespace(&self) -> &str {
        "orchestration"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        // Convert JSON specs to WorkerSpecs
        let mut name_to_id = std::collections::HashMap::new();
        for spec in &input.workers {
            name_to_id.insert(spec.name.clone(), uuid::Uuid::new_v4());
        }

        let worker_specs: Vec<WorkerSpec> = input
            .workers
            .iter()
            .map(|json_spec| {
                let mut spec = json_spec.to_worker_spec(&name_to_id);
                if let Some(&id) = name_to_id.get(&json_spec.name) {
                    spec.id = id;
                }
                spec
            })
            .collect();

        // Build name lookup for results
        let id_to_name: std::collections::HashMap<uuid::Uuid, String> = worker_specs
            .iter()
            .map(|s| (s.id, s.name.clone()))
            .collect();

        // Execute workers
        let mut result = self
            .worker_manager
            .execute_workers(worker_specs, None)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        // Optionally synthesize
        if input.synthesize_results && !result.worker_results.is_empty() {
            let synthesized = self
                .worker_manager
                .synthesize_results(&result.worker_results, input.synthesis_prompt.as_deref())
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?;
            result.synthesized_output = Some(synthesized);
        }

        let worker_count = result.worker_results.len();
        let success_count = result.worker_results.iter().filter(|r| r.success).count();

        let worker_results: Vec<WorkerResultSummary> = result
            .worker_results
            .iter()
            .map(|r| WorkerResultSummary {
                name: id_to_name
                    .get(&r.worker_id)
                    .cloned()
                    .unwrap_or_else(|| r.worker_id.to_string()),
                success: r.success,
                content: r.content.chars().take(2000).collect(),
                error: r.error.clone(),
                duration_ms: r.duration_ms,
            })
            .collect();

        Ok(OrchestrateOutput {
            all_succeeded: result.all_succeeded,
            worker_count,
            success_count,
            synthesized_output: result.synthesized_output,
            worker_results,
            total_duration_ms: result.total_duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrate_input_deserialization() {
        let json = serde_json::json!({
            "workers": [
                {
                    "name": "worker-1",
                    "prompt": "Do task 1",
                    "agent_type": "Explore"
                },
                {
                    "name": "worker-2",
                    "prompt": "Do task 2",
                    "depends_on": ["worker-1"]
                }
            ],
            "synthesize_results": true
        });

        let input: OrchestrateInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.workers.len(), 2);
        assert!(input.synthesize_results);
        assert_eq!(input.workers[1].depends_on, vec!["worker-1"]);
    }

    #[test]
    fn test_orchestrate_output_serialization() {
        let output = OrchestrateOutput {
            all_succeeded: true,
            worker_count: 2,
            success_count: 2,
            synthesized_output: Some("Combined result".to_string()),
            worker_results: vec![WorkerResultSummary {
                name: "worker-1".to_string(),
                success: true,
                content: "Result 1".to_string(),
                error: None,
                duration_ms: 1000,
            }],
            total_duration_ms: 1500,
        };

        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["all_succeeded"], true);
        assert_eq!(json["worker_count"], 2);
    }
}
