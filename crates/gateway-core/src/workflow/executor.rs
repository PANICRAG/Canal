//! Enhanced Workflow Executor
//!
//! Provides parallel execution, pause/resume, and checkpoint support.

use super::checkpoint::CheckpointStore;
use super::dag::DagExecutor;
use super::engine::{
    ExecutionStatus, WorkflowDefinition, WorkflowEngine, WorkflowExecution, WorkflowStep,
};
use crate::error::{Error, Result};
use crate::llm::LlmRouter;
use crate::mcp::McpGateway;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Execution context passed to step handlers
#[derive(Debug, Clone)]
pub struct StepContext {
    pub execution_id: String,
    pub workflow_id: String,
    pub step_id: String,
    pub input: serde_json::Value,
    pub previous_results: HashMap<String, serde_json::Value>,
}

/// Step execution result
#[derive(Debug, Clone)]
pub struct StepResult {
    pub success: bool,
    pub output: serde_json::Value,
    pub error: Option<String>,
}

/// Enhanced workflow executor with parallel execution support
pub struct WorkflowExecutor {
    executions: Arc<RwLock<HashMap<String, WorkflowExecution>>>,
    checkpoint_store: Arc<CheckpointStore>,
    paused_executions: Arc<RwLock<HashSet<String>>>,
    /// Optional LLM router for executing LLM steps
    llm_router: Option<Arc<RwLock<LlmRouter>>>,
    /// Optional MCP gateway for executing tool call steps
    mcp_gateway: Option<Arc<McpGateway>>,
}

impl WorkflowExecutor {
    /// Create a new workflow executor
    pub fn new(checkpoint_store: Arc<CheckpointStore>) -> Self {
        Self {
            executions: Arc::new(RwLock::new(HashMap::new())),
            checkpoint_store,
            paused_executions: Arc::new(RwLock::new(HashSet::new())),
            llm_router: None,
            mcp_gateway: None,
        }
    }

    /// Create a new workflow executor with LLM and MCP services.
    pub fn with_services(
        checkpoint_store: Arc<CheckpointStore>,
        llm_router: Arc<RwLock<LlmRouter>>,
        mcp_gateway: Arc<McpGateway>,
    ) -> Self {
        Self {
            executions: Arc::new(RwLock::new(HashMap::new())),
            checkpoint_store,
            paused_executions: Arc::new(RwLock::new(HashSet::new())),
            llm_router: Some(llm_router),
            mcp_gateway: Some(mcp_gateway),
        }
    }

    /// Set the LLM router after construction.
    pub fn set_llm_router(&mut self, router: Arc<RwLock<LlmRouter>>) {
        self.llm_router = Some(router);
    }

    /// Set the MCP gateway after construction.
    pub fn set_mcp_gateway(&mut self, gateway: Arc<McpGateway>) {
        self.mcp_gateway = Some(gateway);
    }

    /// Start a new workflow execution
    pub async fn start(
        &self,
        workflow: &WorkflowDefinition,
        input: serde_json::Value,
    ) -> Result<WorkflowExecution> {
        let execution = WorkflowExecution {
            id: Uuid::new_v4().to_string(),
            workflow_id: workflow.id.clone(),
            status: ExecutionStatus::Running,
            current_step: 0,
            input: input.clone(),
            output: None,
            step_results: HashMap::new(),
            error: None,
            started_at: Some(chrono::Utc::now()),
            completed_at: None,
        };

        let mut executions = self.executions.write().await;
        executions.insert(execution.id.clone(), execution.clone());

        tracing::info!(
            execution_id = %execution.id,
            workflow_id = %workflow.id,
            "Workflow execution started"
        );

        Ok(execution)
    }

    /// Execute workflow using DAG-based parallel execution
    pub async fn execute_parallel(
        &self,
        workflow: &WorkflowDefinition,
        execution_id: &str,
    ) -> Result<WorkflowExecution> {
        // Build DAG from workflow steps
        let mut dag = DagExecutor::new();
        for step in &workflow.steps {
            dag.add_node(step.id.clone(), step.depends_on.clone());
        }

        // Get parallel execution levels
        let levels = dag.get_parallel_levels()?;

        let mut execution = {
            let executions = self.executions.read().await;
            executions
                .get(execution_id)
                .ok_or_else(|| Error::NotFound(format!("Execution not found: {}", execution_id)))?
                .clone()
        };

        let step_map: HashMap<String, &WorkflowStep> =
            workflow.steps.iter().map(|s| (s.id.clone(), s)).collect();

        // Execute each level (steps within a level run in parallel)
        for (level_idx, level) in levels.iter().enumerate() {
            tracing::info!(
                execution_id = %execution_id,
                level = level_idx,
                steps = ?level,
                "Executing parallel level"
            );

            // Check if paused
            if self.is_paused(execution_id).await {
                execution.status = ExecutionStatus::Paused;
                self.update_execution(&execution).await?;
                return Ok(execution);
            }

            // Execute all steps in this level concurrently
            let mut handles = Vec::new();
            for step_id in level {
                let step = step_map
                    .get(step_id)
                    .ok_or_else(|| Error::NotFound(format!("Step not found: {}", step_id)))?;

                let ctx = StepContext {
                    execution_id: execution_id.to_string(),
                    workflow_id: workflow.id.clone(),
                    step_id: step_id.clone(),
                    input: execution.input.clone(),
                    previous_results: execution.step_results.clone(),
                };

                let step_clone = (*step).clone();
                let llm_router = self.llm_router.clone();
                let mcp_gateway = self.mcp_gateway.clone();
                let handle = tokio::spawn(async move {
                    Self::execute_step_internal(&step_clone, &ctx, llm_router, mcp_gateway).await
                });
                handles.push((step_id.clone(), handle));
            }

            // Wait for all steps in this level to complete
            for (step_id, handle) in handles {
                match handle.await {
                    Ok(Ok(result)) => {
                        if result.success {
                            execution
                                .step_results
                                .insert(step_id.clone(), result.output);
                        } else {
                            execution.status = ExecutionStatus::Error;
                            execution.error = result.error;
                            self.update_execution(&execution).await?;
                            return Ok(execution);
                        }
                    }
                    Ok(Err(e)) => {
                        execution.status = ExecutionStatus::Error;
                        execution.error = Some(e.to_string());
                        self.update_execution(&execution).await?;
                        return Ok(execution);
                    }
                    Err(e) => {
                        execution.status = ExecutionStatus::Error;
                        execution.error = Some(format!("Task join error: {}", e));
                        self.update_execution(&execution).await?;
                        return Ok(execution);
                    }
                }
            }

            // Save checkpoint after each level
            if let Some(first_step_id) = level.first() {
                self.checkpoint_store
                    .save(&execution, first_step_id)
                    .await?;
            }

            execution.current_step = level_idx + 1;
            self.update_execution(&execution).await?;
        }

        // Mark as complete
        execution.status = ExecutionStatus::Success;
        execution.completed_at = Some(chrono::Utc::now());

        // Set output from last step
        if let Some(last_step) = workflow.steps.last() {
            execution.output = execution.step_results.get(&last_step.id).cloned();
        }

        self.update_execution(&execution).await?;

        tracing::info!(
            execution_id = %execution_id,
            "Workflow execution completed successfully"
        );

        Ok(execution)
    }

    /// Execute a single step using a temporary WorkflowEngine to delegate
    /// to the canonical step implementations.
    async fn execute_step_internal(
        step: &WorkflowStep,
        ctx: &StepContext,
        llm_router: Option<Arc<RwLock<LlmRouter>>>,
        mcp_gateway: Option<Arc<McpGateway>>,
    ) -> Result<StepResult> {
        tracing::info!(
            step_id = %ctx.step_id,
            step_type = ?step.step_type,
            "Executing step"
        );

        // Build a temporary WorkflowEngine that shares the same service Arcs.
        let engine = if let (Some(router), Some(gateway)) = (llm_router, mcp_gateway) {
            WorkflowEngine::with_services(router, gateway)
        } else {
            WorkflowEngine::new()
        };

        // Build a WorkflowExecution from the StepContext so the engine
        // step implementations can access previous results.
        let execution = WorkflowExecution {
            id: ctx.execution_id.clone(),
            workflow_id: ctx.workflow_id.clone(),
            status: ExecutionStatus::Running,
            current_step: 0,
            input: ctx.input.clone(),
            output: None,
            step_results: ctx.previous_results.clone(),
            error: None,
            started_at: Some(chrono::Utc::now()),
            completed_at: None,
        };

        // Delegate to the engine's step execution which contains all the
        // real logic for LLM, ToolCall, Condition, etc.
        match engine.execute_step_delegated(step, &execution).await {
            Ok(output) => Ok(StepResult {
                success: true,
                output,
                error: None,
            }),
            Err(e) => Ok(StepResult {
                success: false,
                output: serde_json::json!({"error": e.to_string()}),
                error: Some(e.to_string()),
            }),
        }
    }

    /// Pause a running execution
    pub async fn pause(&self, execution_id: &str) -> Result<()> {
        let mut paused = self.paused_executions.write().await;
        paused.insert(execution_id.to_string());

        tracing::info!(execution_id = %execution_id, "Execution pause requested");
        Ok(())
    }

    /// Resume a paused execution
    pub async fn resume(&self, execution_id: &str) -> Result<()> {
        let mut paused = self.paused_executions.write().await;
        paused.remove(execution_id);

        // Update status
        let mut executions = self.executions.write().await;
        if let Some(execution) = executions.get_mut(execution_id) {
            if execution.status == ExecutionStatus::Paused {
                execution.status = ExecutionStatus::Running;
            }
        }

        tracing::info!(execution_id = %execution_id, "Execution resumed");
        Ok(())
    }

    /// Cancel a running execution
    pub async fn cancel(&self, execution_id: &str) -> Result<()> {
        let mut executions = self.executions.write().await;
        if let Some(execution) = executions.get_mut(execution_id) {
            execution.status = ExecutionStatus::Cancelled;
            execution.completed_at = Some(chrono::Utc::now());
        }

        // Also remove from paused set if present
        let mut paused = self.paused_executions.write().await;
        paused.remove(execution_id);

        tracing::info!(execution_id = %execution_id, "Execution cancelled");
        Ok(())
    }

    /// Check if an execution is paused
    async fn is_paused(&self, execution_id: &str) -> bool {
        let paused = self.paused_executions.read().await;
        paused.contains(execution_id)
    }

    /// Update execution state
    async fn update_execution(&self, execution: &WorkflowExecution) -> Result<()> {
        let mut executions = self.executions.write().await;
        executions.insert(execution.id.clone(), execution.clone());
        Ok(())
    }

    /// Get execution by ID
    pub async fn get_execution(&self, execution_id: &str) -> Option<WorkflowExecution> {
        let executions = self.executions.read().await;
        executions.get(execution_id).cloned()
    }

    /// List all executions
    pub async fn list_executions(&self) -> Vec<WorkflowExecution> {
        let executions = self.executions.read().await;
        executions.values().cloned().collect()
    }

    /// Restore and resume from checkpoint
    pub async fn restore_from_checkpoint(
        &self,
        execution_id: &str,
        workflow: &WorkflowDefinition,
    ) -> Result<Option<WorkflowExecution>> {
        let restored = self.checkpoint_store.restore(execution_id).await?;

        if let Some(mut execution) = restored {
            // Store the restored execution
            let mut executions = self.executions.write().await;
            executions.insert(execution.id.clone(), execution.clone());

            // Resume execution
            execution.status = ExecutionStatus::Running;

            tracing::info!(
                execution_id = %execution_id,
                "Restored and resuming execution from checkpoint"
            );

            // Continue execution from checkpoint
            drop(executions);
            return Ok(Some(self.execute_parallel(workflow, execution_id).await?));
        }

        Ok(None)
    }
}

impl Default for WorkflowExecutor {
    fn default() -> Self {
        Self::new(Arc::new(CheckpointStore::new()))
    }
}

impl WorkflowExecutor {
    /// Create a default executor with services attached.
    pub fn default_with_services(
        llm_router: Arc<RwLock<LlmRouter>>,
        mcp_gateway: Arc<McpGateway>,
    ) -> Self {
        Self::with_services(Arc::new(CheckpointStore::new()), llm_router, mcp_gateway)
    }
}

#[cfg(test)]
mod tests {
    use super::super::engine::StepType;
    use super::*;

    fn create_test_workflow() -> WorkflowDefinition {
        WorkflowDefinition {
            id: "test-wf".to_string(),
            name: "Test Workflow".to_string(),
            description: "A test workflow".to_string(),
            steps: vec![
                WorkflowStep {
                    id: "step-1".to_string(),
                    name: "Step 1".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({}),
                    depends_on: vec![],
                },
                WorkflowStep {
                    id: "step-2".to_string(),
                    name: "Step 2".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({}),
                    depends_on: vec!["step-1".to_string()],
                },
            ],
        }
    }

    #[tokio::test]
    async fn test_start_execution() {
        let executor = WorkflowExecutor::default();
        let workflow = create_test_workflow();

        let execution = executor
            .start(&workflow, serde_json::json!({"test": true}))
            .await
            .unwrap();

        assert_eq!(execution.status, ExecutionStatus::Running);
        assert_eq!(execution.workflow_id, "test-wf");
    }

    #[tokio::test]
    async fn test_execute_parallel() {
        let executor = WorkflowExecutor::default();
        let workflow = create_test_workflow();

        let execution = executor
            .start(&workflow, serde_json::json!({}))
            .await
            .unwrap();

        let result = executor
            .execute_parallel(&workflow, &execution.id)
            .await
            .unwrap();

        assert_eq!(result.status, ExecutionStatus::Success);
        assert!(result.step_results.contains_key("step-1"));
        assert!(result.step_results.contains_key("step-2"));
    }

    #[tokio::test]
    async fn test_pause_resume() {
        let executor = WorkflowExecutor::default();
        let workflow = create_test_workflow();

        let execution = executor
            .start(&workflow, serde_json::json!({}))
            .await
            .unwrap();

        executor.pause(&execution.id).await.unwrap();
        assert!(executor.is_paused(&execution.id).await);

        executor.resume(&execution.id).await.unwrap();
        assert!(!executor.is_paused(&execution.id).await);
    }

    #[tokio::test]
    async fn test_cancel_execution() {
        let executor = WorkflowExecutor::default();
        let workflow = create_test_workflow();

        let execution = executor
            .start(&workflow, serde_json::json!({}))
            .await
            .unwrap();

        executor.cancel(&execution.id).await.unwrap();

        let cancelled = executor.get_execution(&execution.id).await.unwrap();
        assert_eq!(cancelled.status, ExecutionStatus::Cancelled);
    }
}
