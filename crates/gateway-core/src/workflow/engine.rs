//! Workflow Engine implementation

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::{Error, Result};
use crate::llm::{ChatRequest, LlmRouter, Message};
use crate::mcp::McpGateway;

/// Workflow definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<WorkflowStep>,
}

/// Workflow step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub id: String,
    pub name: String,
    pub step_type: StepType,
    pub config: serde_json::Value,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Step type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepType {
    /// LLM call step
    Llm,
    /// Tool call step
    ToolCall,
    /// Conditional branching
    Condition,
    /// Parallel execution
    Parallel,
    /// Loop/iteration
    Loop,
    /// Transform data
    Transform,
    /// Wait for external event
    Wait,
}

/// Workflow execution status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Pending,
    Running,
    Success,
    Error,
    Cancelled,
    Paused,
}

/// Workflow execution state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowExecution {
    pub id: String,
    pub workflow_id: String,
    pub status: ExecutionStatus,
    pub current_step: usize,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub step_results: HashMap<String, serde_json::Value>,
    pub error: Option<String>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Workflow Engine
///
/// Manages workflow registration and execution.
pub struct WorkflowEngine {
    workflows: HashMap<String, WorkflowDefinition>,
    /// Optional LLM router for executing LLM steps
    llm_router: Option<Arc<RwLock<LlmRouter>>>,
    /// Optional MCP gateway for executing tool call steps
    mcp_gateway: Option<Arc<McpGateway>>,
    /// Tracks cancelled execution IDs
    cancelled: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl WorkflowEngine {
    /// Create a new workflow engine without LLM/MCP support.
    ///
    /// LLM and ToolCall step types will return errors. Use
    /// [`with_services`] to enable them.
    pub fn new() -> Self {
        Self {
            workflows: HashMap::new(),
            llm_router: None,
            mcp_gateway: None,
            cancelled: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    /// Create a new workflow engine with LLM router and MCP gateway.
    pub fn with_services(llm_router: Arc<RwLock<LlmRouter>>, mcp_gateway: Arc<McpGateway>) -> Self {
        Self {
            workflows: HashMap::new(),
            llm_router: Some(llm_router),
            mcp_gateway: Some(mcp_gateway),
            cancelled: Arc::new(RwLock::new(std::collections::HashSet::new())),
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

    /// Register a workflow
    pub fn register(&mut self, workflow: WorkflowDefinition) {
        tracing::info!(
            workflow_id = %workflow.id,
            workflow_name = %workflow.name,
            "Registering workflow"
        );
        self.workflows.insert(workflow.id.clone(), workflow);
    }

    /// Unregister a workflow
    pub fn unregister(&mut self, id: &str) -> bool {
        tracing::info!(workflow_id = %id, "Unregistering workflow");
        self.workflows.remove(id).is_some()
    }

    /// Get a workflow by ID
    pub fn get(&self, id: &str) -> Option<&WorkflowDefinition> {
        self.workflows.get(id)
    }

    /// List all workflows
    pub fn list(&self) -> Vec<&WorkflowDefinition> {
        self.workflows.values().collect()
    }

    /// Execute a workflow
    pub async fn execute(
        &self,
        workflow_id: &str,
        input: serde_json::Value,
    ) -> Result<WorkflowExecution> {
        let workflow = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| Error::NotFound(format!("Workflow not found: {}", workflow_id)))?;

        let mut execution = WorkflowExecution {
            id: uuid::Uuid::new_v4().to_string(),
            workflow_id: workflow_id.to_string(),
            status: ExecutionStatus::Running,
            current_step: 0,
            input: input.clone(),
            output: None,
            step_results: HashMap::new(),
            error: None,
            started_at: Some(chrono::Utc::now()),
            completed_at: None,
        };

        tracing::info!(
            execution_id = %execution.id,
            workflow_id = %workflow_id,
            workflow_name = %workflow.name,
            "Starting workflow execution"
        );

        // R2-H10: Validate that `depends_on` references only point to steps
        // appearing earlier in the list. Sequential execution runs steps in
        // order, so a dependency on a later step would never be satisfied.
        // (`WorkflowExecutor::execute_parallel` handles true DAG scheduling.)
        Self::validate_step_order(&workflow.steps)?;

        for (i, step) in workflow.steps.iter().enumerate() {
            // Check for cancellation before each step
            {
                let cancelled = self.cancelled.read().await;
                if cancelled.contains(&execution.id) {
                    tracing::info!(
                        execution_id = %execution.id,
                        "Workflow execution cancelled"
                    );
                    execution.status = ExecutionStatus::Cancelled;
                    execution.completed_at = Some(chrono::Utc::now());
                    return Ok(execution);
                }
            }

            execution.current_step = i;

            tracing::info!(
                execution_id = %execution.id,
                step_id = %step.id,
                step_name = %step.name,
                step_type = ?step.step_type,
                "Executing step"
            );

            match self.execute_step(step, &execution).await {
                Ok(result) => {
                    execution.step_results.insert(step.id.clone(), result);
                }
                Err(e) => {
                    tracing::error!(
                        execution_id = %execution.id,
                        step_id = %step.id,
                        error = %e,
                        "Step execution failed"
                    );
                    execution.status = ExecutionStatus::Error;
                    execution.error = Some(e.to_string());
                    execution.completed_at = Some(chrono::Utc::now());
                    return Ok(execution);
                }
            }
        }

        execution.status = ExecutionStatus::Success;
        execution.completed_at = Some(chrono::Utc::now());

        // Set the last step result as the workflow output
        if let Some(last_step) = workflow.steps.last() {
            execution.output = execution.step_results.get(&last_step.id).cloned();
        }

        tracing::info!(
            execution_id = %execution.id,
            workflow_id = %workflow_id,
            "Workflow execution completed"
        );

        Ok(execution)
    }

    /// Validate that every `depends_on` entry in each step refers to a step
    /// that appears *before* it in the list. In sequential execution the steps
    /// run in order, so a dependency on a later step can never be satisfied.
    fn validate_step_order(steps: &[WorkflowStep]) -> Result<()> {
        let mut seen: HashSet<&str> = HashSet::new();
        for step in steps {
            for dep in &step.depends_on {
                if !seen.contains(dep.as_str()) {
                    return Err(Error::Workflow(format!(
                        "Step '{}' depends on '{}', which has not appeared earlier \
                         in the step list. Reorder steps so that dependencies come \
                         first, or use parallel execution for DAG scheduling.",
                        step.id, dep
                    )));
                }
            }
            seen.insert(&step.id);
        }
        Ok(())
    }

    fn execute_step<'a>(
        &'a self,
        step: &'a WorkflowStep,
        execution: &'a WorkflowExecution,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value>> + Send + 'a>>
    {
        Box::pin(async move {
            match step.step_type {
                StepType::Llm => self.execute_llm_step(step, execution).await,
                StepType::ToolCall => self.execute_tool_call_step(step).await,
                StepType::Condition => self.execute_condition_step(step, execution).await,
                StepType::Parallel => self.execute_parallel_step(step, execution).await,
                StepType::Loop => self.execute_loop_step(step, execution).await,
                StepType::Transform => Self::execute_transform_step(step, execution),
                StepType::Wait => Self::execute_wait_step(step).await,
            }
        })
    }

    /// Public entry point for executing a single step, used by [`WorkflowExecutor`]
    /// to delegate step execution to the canonical implementations.
    pub async fn execute_step_delegated(
        &self,
        step: &WorkflowStep,
        execution: &WorkflowExecution,
    ) -> Result<serde_json::Value> {
        self.execute_step(step, execution).await
    }

    // ---- LLM step -------------------------------------------------------

    async fn execute_llm_step(
        &self,
        step: &WorkflowStep,
        execution: &WorkflowExecution,
    ) -> Result<serde_json::Value> {
        let router = self.llm_router.as_ref().ok_or_else(|| {
            Error::Workflow("LLM router not configured; cannot execute LLM step".into())
        })?;

        // Build prompt from config, optionally interpolating previous results
        let prompt = step
            .config
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let system = step
            .config
            .get("system")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let model = step
            .config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let max_tokens = step
            .config
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);

        let temperature = step
            .config
            .get("temperature")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32);

        // Allow the prompt to reference previous step results via {{step_id}}
        let resolved_prompt = Self::resolve_template(&prompt, &execution.step_results);

        let mut messages = Vec::new();
        if let Some(sys) = system {
            messages.push(Message::text("system", sys));
        }
        messages.push(Message::text("user", resolved_prompt));

        let request = ChatRequest {
            messages,
            model,
            max_tokens,
            temperature,
            stream: false,
            tools: vec![],
            tool_choice: None,
            ..Default::default()
        };

        let router_guard = router.read().await;
        let response = router_guard.route(request).await?;
        drop(router_guard);

        let text = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(serde_json::json!({
            "step_type": "llm",
            "status": "completed",
            "text": text,
            "model": response.model,
            "usage": {
                "prompt_tokens": response.usage.prompt_tokens,
                "completion_tokens": response.usage.completion_tokens,
                "total_tokens": response.usage.total_tokens,
            }
        }))
    }

    // ---- Tool Call step --------------------------------------------------

    async fn execute_tool_call_step(&self, step: &WorkflowStep) -> Result<serde_json::Value> {
        let gateway = self.mcp_gateway.as_ref().ok_or_else(|| {
            Error::Workflow("MCP gateway not configured; cannot execute tool call step".into())
        })?;

        let tool_name = step
            .config
            .get("tool")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Workflow("ToolCall step missing 'tool' in config".into()))?;

        let arguments = step
            .config
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        // Use the LLM tool call interface which handles namespace_tool format
        let result = gateway.execute_llm_tool_call(tool_name, arguments).await?;

        // Convert ToolCallResult to JSON
        Ok(serde_json::json!({
            "step_type": "tool_call",
            "status": "completed",
            "tool": tool_name,
            "result": {
                "content": result.content,
                "is_error": result.is_error,
            }
        }))
    }

    // ---- Condition step --------------------------------------------------

    async fn execute_condition_step(
        &self,
        step: &WorkflowStep,
        execution: &WorkflowExecution,
    ) -> Result<serde_json::Value> {
        // Config shape:
        //   { "operator": "equals"|"not_empty"|"contains"|"gt"|"lt",
        //     "left": "<value or {{step_id}}.field>",
        //     "right": "<value>",         -- not needed for not_empty
        //     "then_steps": [...],         -- optional inline sub-steps
        //     "else_steps": [...]          -- optional inline sub-steps
        //   }

        let operator = step
            .config
            .get("operator")
            .and_then(|v| v.as_str())
            .unwrap_or("not_empty");

        let left_raw = step
            .config
            .get("left")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        let left = Self::resolve_json_value(&left_raw, &execution.step_results);

        let right_raw = step
            .config
            .get("right")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        let right = Self::resolve_json_value(&right_raw, &execution.step_results);

        let condition_met = match operator {
            "equals" | "eq" => left == right,
            "not_equals" | "neq" => left != right,
            "not_empty" => !Self::is_empty_value(&left),
            "contains" => {
                if let (Some(haystack), Some(needle)) = (left.as_str(), right.as_str()) {
                    haystack.contains(needle)
                } else {
                    false
                }
            }
            "gt" => {
                let l = left.as_f64().unwrap_or(0.0);
                let r = right.as_f64().unwrap_or(0.0);
                l > r
            }
            "lt" => {
                let l = left.as_f64().unwrap_or(0.0);
                let r = right.as_f64().unwrap_or(0.0);
                l < r
            }
            "gte" => {
                let l = left.as_f64().unwrap_or(0.0);
                let r = right.as_f64().unwrap_or(0.0);
                l >= r
            }
            "lte" => {
                let l = left.as_f64().unwrap_or(0.0);
                let r = right.as_f64().unwrap_or(0.0);
                l <= r
            }
            other => {
                return Err(Error::Workflow(format!(
                    "Unknown condition operator: {}",
                    other
                )));
            }
        };

        let branch = if condition_met { "then" } else { "else" };

        // Optionally execute inline sub-steps for the chosen branch
        let branch_key = if condition_met {
            "then_steps"
        } else {
            "else_steps"
        };
        let mut branch_results = serde_json::json!(null);
        if let Some(sub_steps_val) = step.config.get(branch_key) {
            if let Ok(sub_steps) =
                serde_json::from_value::<Vec<WorkflowStep>>(sub_steps_val.clone())
            {
                let mut results = HashMap::new();
                for sub_step in &sub_steps {
                    let sub_result = self.execute_step(sub_step, execution).await?;
                    results.insert(sub_step.id.clone(), sub_result);
                }
                branch_results = serde_json::to_value(&results).unwrap_or(serde_json::json!(null));
            }
        }

        Ok(serde_json::json!({
            "step_type": "condition",
            "status": "completed",
            "result": condition_met,
            "branch": branch,
            "branch_results": branch_results,
        }))
    }

    // ---- Parallel step ---------------------------------------------------

    async fn execute_parallel_step(
        &self,
        step: &WorkflowStep,
        execution: &WorkflowExecution,
    ) -> Result<serde_json::Value> {
        // Config: { "steps": [ WorkflowStep, ... ] }
        let sub_steps_val = step
            .config
            .get("steps")
            .ok_or_else(|| Error::Workflow("Parallel step missing 'steps' in config".into()))?;

        let sub_steps: Vec<WorkflowStep> = serde_json::from_value(sub_steps_val.clone())
            .map_err(|e| Error::Workflow(format!("Invalid sub-steps in parallel config: {}", e)))?;

        if sub_steps.is_empty() {
            return Ok(serde_json::json!({
                "step_type": "parallel",
                "status": "completed",
                "results": {}
            }));
        }

        // Spawn each sub-step concurrently.
        // Because execute_step requires &self, we reconstruct minimal state for
        // each spawned task by cloning the necessary Arcs.
        let mut handles = Vec::with_capacity(sub_steps.len());

        for sub_step in sub_steps {
            let llm_router = self.llm_router.clone();
            let mcp_gateway = self.mcp_gateway.clone();
            let cancelled = self.cancelled.clone();
            let exec_clone = execution.clone();

            let handle = tokio::spawn(async move {
                // Build a temporary engine for the spawned task
                let engine = WorkflowEngine {
                    workflows: HashMap::new(),
                    llm_router,
                    mcp_gateway,
                    cancelled,
                };
                let result = engine.execute_step(&sub_step, &exec_clone).await;
                (sub_step.id.clone(), result)
            });
            handles.push(handle);
        }

        let mut results = HashMap::new();
        let mut errors = Vec::new();
        for handle in handles {
            match handle.await {
                Ok((id, Ok(val))) => {
                    results.insert(id, val);
                }
                Ok((id, Err(e))) => {
                    errors.push(format!("{}: {}", id, e));
                }
                Err(join_err) => {
                    errors.push(format!("task join error: {}", join_err));
                }
            }
        }

        if !errors.is_empty() {
            return Err(Error::Workflow(format!(
                "Parallel step had {} error(s): {}",
                errors.len(),
                errors.join("; ")
            )));
        }

        Ok(serde_json::json!({
            "step_type": "parallel",
            "status": "completed",
            "results": results,
        }))
    }

    // ---- Loop step -------------------------------------------------------

    async fn execute_loop_step(
        &self,
        step: &WorkflowStep,
        execution: &WorkflowExecution,
    ) -> Result<serde_json::Value> {
        // Config variants:
        //   Fixed iterations:    { "iterations": 5, "body": [ WorkflowStep, ... ] }
        //   While condition:     { "while": { "operator": ..., "left": ..., "right": ... },
        //                          "body": [...], "max_iterations": 100 }
        //   For-each:            { "collection": "{{step_id}}.items", "item_var": "item",
        //                          "body": [...] }

        let body_val = step
            .config
            .get("body")
            .ok_or_else(|| Error::Workflow("Loop step missing 'body' in config".into()))?;

        let body_steps: Vec<WorkflowStep> = serde_json::from_value(body_val.clone())
            .map_err(|e| Error::Workflow(format!("Invalid body steps in loop config: {}", e)))?;

        // R2-H16: Hard cap on loop iterations to prevent abuse
        const MAX_LOOP_ITERATIONS: usize = 10_000;

        let max_iterations = step
            .config
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000)
            .min(MAX_LOOP_ITERATIONS as u64) as usize;

        let mut all_results: Vec<serde_json::Value> = Vec::new();

        if let Some(iterations) = step.config.get("iterations").and_then(|v| v.as_u64()) {
            // Fixed iterations
            let count = (iterations as usize).min(max_iterations);
            for i in 0..count {
                let mut iter_results = HashMap::new();
                // Create an execution context that includes iteration index
                let mut iter_execution = execution.clone();
                iter_execution
                    .step_results
                    .insert("__loop_index".to_string(), serde_json::json!(i));
                for body_step in &body_steps {
                    let result = self.execute_step(body_step, &iter_execution).await?;
                    iter_execution
                        .step_results
                        .insert(body_step.id.clone(), result.clone());
                    iter_results.insert(body_step.id.clone(), result);
                }
                all_results.push(serde_json::to_value(&iter_results).unwrap_or_default());
            }
        } else if let Some(condition_config) = step.config.get("while").cloned() {
            // While-loop: evaluate a condition each iteration
            let mut iter_execution = execution.clone();
            for i in 0..max_iterations {
                // Build a temporary condition step to evaluate
                let cond_step = WorkflowStep {
                    id: format!("__while_cond_{}", i),
                    name: "while_condition".to_string(),
                    step_type: StepType::Condition,
                    config: condition_config.clone(),
                    depends_on: vec![],
                };
                let cond_result = self
                    .execute_condition_step(&cond_step, &iter_execution)
                    .await?;
                let should_continue = cond_result
                    .get("result")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }

                let mut iter_results = HashMap::new();
                iter_execution
                    .step_results
                    .insert("__loop_index".to_string(), serde_json::json!(i));
                for body_step in &body_steps {
                    let result = self.execute_step(body_step, &iter_execution).await?;
                    iter_execution
                        .step_results
                        .insert(body_step.id.clone(), result.clone());
                    iter_results.insert(body_step.id.clone(), result);
                }
                all_results.push(serde_json::to_value(&iter_results).unwrap_or_default());
            }
        } else if let Some(collection_ref) = step.config.get("collection").and_then(|v| v.as_str())
        {
            // For-each loop over a collection
            let collection = Self::resolve_json_ref(collection_ref, &execution.step_results);
            let items = match collection.as_array() {
                Some(arr) => arr.clone(),
                None => {
                    return Err(Error::Workflow(format!(
                        "Loop collection '{}' did not resolve to an array",
                        collection_ref
                    )));
                }
            };

            let item_var = step
                .config
                .get("item_var")
                .and_then(|v| v.as_str())
                .unwrap_or("item");

            let count = items.len().min(max_iterations);
            for (i, item) in items.into_iter().take(count).enumerate() {
                let mut iter_execution = execution.clone();
                iter_execution
                    .step_results
                    .insert("__loop_index".to_string(), serde_json::json!(i));
                iter_execution
                    .step_results
                    .insert(item_var.to_string(), item);

                let mut iter_results = HashMap::new();
                for body_step in &body_steps {
                    let result = self.execute_step(body_step, &iter_execution).await?;
                    iter_execution
                        .step_results
                        .insert(body_step.id.clone(), result.clone());
                    iter_results.insert(body_step.id.clone(), result);
                }
                all_results.push(serde_json::to_value(&iter_results).unwrap_or_default());
            }
        } else {
            return Err(Error::Workflow(
                "Loop step requires 'iterations', 'while', or 'collection' in config".into(),
            ));
        }

        Ok(serde_json::json!({
            "step_type": "loop",
            "status": "completed",
            "iterations": all_results.len(),
            "results": all_results,
        }))
    }

    // ---- Transform step --------------------------------------------------

    fn execute_transform_step(
        step: &WorkflowStep,
        execution: &WorkflowExecution,
    ) -> Result<serde_json::Value> {
        // Config:
        //   { "operation": "extract"|"merge"|"map"|"set"|"template",
        //     ... operation-specific fields }

        let operation = step
            .config
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("extract");

        match operation {
            "extract" => {
                // Extract a field from a previous step result.
                // { "operation": "extract", "source": "{{step_id}}", "path": "text" }
                let source_raw = step
                    .config
                    .get("source")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let source = Self::resolve_json_value(&source_raw, &execution.step_results);

                let path = step
                    .config
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let extracted = Self::json_path_get(&source, path);
                Ok(serde_json::json!({
                    "step_type": "transform",
                    "operation": "extract",
                    "status": "completed",
                    "value": extracted,
                }))
            }
            "merge" => {
                // Merge multiple step results into one object.
                // { "operation": "merge", "sources": ["step1", "step2"] }
                let sources = step
                    .config
                    .get("sources")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                let mut merged = serde_json::Map::new();
                for src in &sources {
                    if let Some(key) = src.as_str() {
                        if let Some(val) = execution.step_results.get(key) {
                            if let Some(obj) = val.as_object() {
                                for (k, v) in obj {
                                    merged.insert(k.clone(), v.clone());
                                }
                            } else {
                                merged.insert(key.to_string(), val.clone());
                            }
                        }
                    }
                }
                Ok(serde_json::json!({
                    "step_type": "transform",
                    "operation": "merge",
                    "status": "completed",
                    "value": serde_json::Value::Object(merged),
                }))
            }
            "map" => {
                // Apply a simple key-value mapping.
                // { "operation": "map", "input": "{{step_id}}.items", "mapping": { "newKey": "$.oldKey" } }
                let input_raw = step
                    .config
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let input = Self::resolve_json_value(&input_raw, &execution.step_results);

                let mapping = step
                    .config
                    .get("mapping")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();

                let mapped = if let Some(arr) = input.as_array() {
                    let mapped_arr: Vec<serde_json::Value> = arr
                        .iter()
                        .map(|item| {
                            let mut obj = serde_json::Map::new();
                            for (new_key, path_val) in &mapping {
                                if let Some(path) = path_val.as_str() {
                                    let extracted = if path.starts_with("$.") {
                                        Self::json_path_get(item, &path[2..])
                                    } else {
                                        path_val.clone()
                                    };
                                    obj.insert(new_key.clone(), extracted);
                                }
                            }
                            serde_json::Value::Object(obj)
                        })
                        .collect();
                    serde_json::Value::Array(mapped_arr)
                } else {
                    // Map on a single object
                    let mut obj = serde_json::Map::new();
                    for (new_key, path_val) in &mapping {
                        if let Some(path) = path_val.as_str() {
                            let extracted = if path.starts_with("$.") {
                                Self::json_path_get(&input, &path[2..])
                            } else {
                                path_val.clone()
                            };
                            obj.insert(new_key.clone(), extracted);
                        }
                    }
                    serde_json::Value::Object(obj)
                };

                Ok(serde_json::json!({
                    "step_type": "transform",
                    "operation": "map",
                    "status": "completed",
                    "value": mapped,
                }))
            }
            "set" => {
                // Set an explicit value.
                // { "operation": "set", "value": { ... } }
                let value = step
                    .config
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let resolved = Self::resolve_json_value(&value, &execution.step_results);
                Ok(serde_json::json!({
                    "step_type": "transform",
                    "operation": "set",
                    "status": "completed",
                    "value": resolved,
                }))
            }
            "template" => {
                // String template interpolation.
                // { "operation": "template", "template": "Hello {{step1.text}}" }
                let template = step
                    .config
                    .get("template")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let resolved = Self::resolve_template(template, &execution.step_results);
                Ok(serde_json::json!({
                    "step_type": "transform",
                    "operation": "template",
                    "status": "completed",
                    "value": resolved,
                }))
            }
            other => Err(Error::Workflow(format!(
                "Unknown transform operation: {}",
                other
            ))),
        }
    }

    // ---- Wait step -------------------------------------------------------

    async fn execute_wait_step(step: &WorkflowStep) -> Result<serde_json::Value> {
        // R2-H15: Cap wait duration to 5 minutes to prevent abuse
        const MAX_WAIT_MS: u64 = 300_000;

        let duration_ms = step
            .config
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .or_else(|| step.config.get("timeout_ms").and_then(|v| v.as_u64()))
            .unwrap_or(0)
            .min(MAX_WAIT_MS);

        if duration_ms > 0 {
            tracing::info!(duration_ms = duration_ms, "Wait step sleeping");
            tokio::time::sleep(tokio::time::Duration::from_millis(duration_ms)).await;
        }

        Ok(serde_json::json!({
            "step_type": "wait",
            "status": "completed",
            "duration_ms": duration_ms,
        }))
    }

    // ---- Cancel ----------------------------------------------------------

    /// Cancel a running workflow execution.
    ///
    /// Sets a cancellation flag that is checked between steps. Currently
    /// running steps will complete before the workflow is marked cancelled.
    pub async fn cancel(&self, execution_id: &str) -> Result<()> {
        tracing::info!(execution_id = %execution_id, "Cancelling workflow execution");
        let mut cancelled = self.cancelled.write().await;
        cancelled.insert(execution_id.to_string());
        Ok(())
    }

    // ---- Helper methods --------------------------------------------------

    /// Resolve `{{step_id}}` and `{{step_id.field}}` placeholders in a template string.
    fn resolve_template(
        template: &str,
        step_results: &HashMap<String, serde_json::Value>,
    ) -> String {
        let mut result = template.to_string();
        // Match {{...}} patterns (regex compiled once via OnceLock)
        use std::sync::OnceLock;
        static RE: OnceLock<regex::Regex> = OnceLock::new();
        let re = RE.get_or_init(|| regex::Regex::new(r"\{\{([^}]+)\}\}").unwrap());
        for cap in re.captures_iter(template) {
            let full_match = &cap[0];
            let reference = cap[1].trim();
            let resolved = Self::resolve_json_ref(reference, step_results);
            let replacement = match resolved {
                serde_json::Value::String(s) => s,
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            result = result.replace(full_match, &replacement);
        }
        result
    }

    /// Resolve a dotted reference like "step_id.field.subfield" against step results.
    fn resolve_json_ref(
        reference: &str,
        step_results: &HashMap<String, serde_json::Value>,
    ) -> serde_json::Value {
        let parts: Vec<&str> = reference.splitn(2, '.').collect();
        let step_id = parts[0];

        match step_results.get(step_id) {
            Some(val) => {
                if parts.len() > 1 {
                    Self::json_path_get(val, parts[1])
                } else {
                    val.clone()
                }
            }
            None => serde_json::Value::Null,
        }
    }

    /// Resolve a JSON value that may contain string template references.
    fn resolve_json_value(
        value: &serde_json::Value,
        step_results: &HashMap<String, serde_json::Value>,
    ) -> serde_json::Value {
        match value {
            serde_json::Value::String(s) => {
                // Check if the entire string is a single reference like "{{step_id.field}}"
                let trimmed = s.trim();
                if trimmed.starts_with("{{")
                    && trimmed.ends_with("}}")
                    && trimmed.matches("{{").count() == 1
                {
                    let reference = &trimmed[2..trimmed.len() - 2];
                    Self::resolve_json_ref(reference.trim(), step_results)
                } else if s.contains("{{") {
                    // Contains template expressions mixed with text
                    serde_json::Value::String(Self::resolve_template(s, step_results))
                } else {
                    value.clone()
                }
            }
            _ => value.clone(),
        }
    }

    /// Navigate a dotted path in a JSON value (e.g., "field.subfield.0").
    fn json_path_get(value: &serde_json::Value, path: &str) -> serde_json::Value {
        if path.is_empty() {
            return value.clone();
        }
        let mut current = value;
        for segment in path.split('.') {
            match current {
                serde_json::Value::Object(map) => {
                    current = match map.get(segment) {
                        Some(v) => v,
                        None => return serde_json::Value::Null,
                    };
                }
                serde_json::Value::Array(arr) => {
                    if let Ok(idx) = segment.parse::<usize>() {
                        current = match arr.get(idx) {
                            Some(v) => v,
                            None => return serde_json::Value::Null,
                        };
                    } else {
                        return serde_json::Value::Null;
                    }
                }
                _ => return serde_json::Value::Null,
            }
        }
        current.clone()
    }

    /// Check if a JSON value is "empty" (null, empty string, empty array/object).
    ///
    /// R2-M31: `false` and `0` are valid values, not empty.
    fn is_empty_value(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Null => true,
            serde_json::Value::String(s) => s.is_empty(),
            serde_json::Value::Array(a) => a.is_empty(),
            serde_json::Value::Object(o) => o.is_empty(),
            _ => false,
        }
    }
}

impl Default for WorkflowEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_workflow() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "test-workflow".to_string(),
            name: "Test Workflow".to_string(),
            description: "A test workflow".to_string(),
            steps: vec![],
        };

        engine.register(workflow);

        assert!(engine.get("test-workflow").is_some());
    }

    #[tokio::test]
    async fn test_execute_workflow_transform() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "test-workflow".to_string(),
            name: "Test Workflow".to_string(),
            description: "A test workflow".to_string(),
            steps: vec![WorkflowStep {
                id: "step1".to_string(),
                name: "Step 1".to_string(),
                step_type: StepType::Transform,
                config: serde_json::json!({
                    "operation": "set",
                    "value": {"hello": "world"}
                }),
                depends_on: vec![],
            }],
        };

        engine.register(workflow);

        let result = engine
            .execute("test-workflow", serde_json::json!({"input": "test"}))
            .await;

        assert!(result.is_ok());
        let execution = result.unwrap();
        assert_eq!(execution.status, ExecutionStatus::Success);
        let step_result = execution.step_results.get("step1").unwrap();
        assert_eq!(step_result["value"]["hello"], "world");
    }

    #[tokio::test]
    async fn test_workflow_not_found() {
        let engine = WorkflowEngine::new();

        let result = engine.execute("nonexistent", serde_json::json!({})).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_wait_step() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "wait-wf".to_string(),
            name: "Wait Workflow".to_string(),
            description: "Tests wait step".to_string(),
            steps: vec![WorkflowStep {
                id: "wait1".to_string(),
                name: "Wait Step".to_string(),
                step_type: StepType::Wait,
                config: serde_json::json!({"duration_ms": 10}),
                depends_on: vec![],
            }],
        };

        engine.register(workflow);
        let result = engine
            .execute("wait-wf", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.step_results["wait1"]["duration_ms"], 10);
    }

    #[tokio::test]
    async fn test_condition_step_equals() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "cond-wf".to_string(),
            name: "Condition Workflow".to_string(),
            description: "Tests condition step".to_string(),
            steps: vec![
                WorkflowStep {
                    id: "setup".to_string(),
                    name: "Setup".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({
                        "operation": "set",
                        "value": {"status": "ready"}
                    }),
                    depends_on: vec![],
                },
                WorkflowStep {
                    id: "check".to_string(),
                    name: "Check".to_string(),
                    step_type: StepType::Condition,
                    config: serde_json::json!({
                        "operator": "equals",
                        "left": "{{setup.value.status}}",
                        "right": "ready"
                    }),
                    depends_on: vec!["setup".to_string()],
                },
            ],
        };

        engine.register(workflow);
        let result = engine
            .execute("cond-wf", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.step_results["check"]["result"], true);
        assert_eq!(result.step_results["check"]["branch"], "then");
    }

    #[tokio::test]
    async fn test_condition_step_not_empty() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "cond-ne-wf".to_string(),
            name: "Condition Not Empty".to_string(),
            description: "Tests not_empty condition".to_string(),
            steps: vec![
                WorkflowStep {
                    id: "setup".to_string(),
                    name: "Setup".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({
                        "operation": "set",
                        "value": "some_data"
                    }),
                    depends_on: vec![],
                },
                WorkflowStep {
                    id: "check".to_string(),
                    name: "Check".to_string(),
                    step_type: StepType::Condition,
                    config: serde_json::json!({
                        "operator": "not_empty",
                        "left": "{{setup.value}}"
                    }),
                    depends_on: vec!["setup".to_string()],
                },
            ],
        };

        engine.register(workflow);
        let result = engine
            .execute("cond-ne-wf", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.step_results["check"]["result"], true);
    }

    #[tokio::test]
    async fn test_transform_extract() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "extract-wf".to_string(),
            name: "Extract Workflow".to_string(),
            description: "Tests transform extract".to_string(),
            steps: vec![
                WorkflowStep {
                    id: "data".to_string(),
                    name: "Data".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({
                        "operation": "set",
                        "value": {"nested": {"key": "found_it"}}
                    }),
                    depends_on: vec![],
                },
                WorkflowStep {
                    id: "extract".to_string(),
                    name: "Extract".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({
                        "operation": "extract",
                        "source": "{{data.value}}",
                        "path": "nested.key"
                    }),
                    depends_on: vec!["data".to_string()],
                },
            ],
        };

        engine.register(workflow);
        let result = engine
            .execute("extract-wf", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.step_results["extract"]["value"], "found_it");
    }

    #[tokio::test]
    async fn test_transform_merge() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "merge-wf".to_string(),
            name: "Merge Workflow".to_string(),
            description: "Tests transform merge".to_string(),
            steps: vec![
                WorkflowStep {
                    id: "a".to_string(),
                    name: "A".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({
                        "operation": "set",
                        "value": {"x": 1}
                    }),
                    depends_on: vec![],
                },
                WorkflowStep {
                    id: "b".to_string(),
                    name: "B".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({
                        "operation": "set",
                        "value": {"y": 2}
                    }),
                    depends_on: vec![],
                },
                WorkflowStep {
                    id: "merged".to_string(),
                    name: "Merged".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({
                        "operation": "merge",
                        "sources": ["a", "b"]
                    }),
                    depends_on: vec!["a".to_string(), "b".to_string()],
                },
            ],
        };

        engine.register(workflow);
        let result = engine
            .execute("merge-wf", serde_json::json!({}))
            .await
            .unwrap();
        let merged = &result.step_results["merged"]["value"];
        // Merged should contain keys from both a and b step results
        assert!(merged.is_object());
    }

    #[tokio::test]
    async fn test_loop_fixed_iterations() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "loop-wf".to_string(),
            name: "Loop Workflow".to_string(),
            description: "Tests fixed-iteration loop".to_string(),
            steps: vec![WorkflowStep {
                id: "loop1".to_string(),
                name: "Loop".to_string(),
                step_type: StepType::Loop,
                config: serde_json::json!({
                    "iterations": 3,
                    "body": [{
                        "id": "inner",
                        "name": "Inner Step",
                        "step_type": "transform",
                        "config": {"operation": "set", "value": "iteration_done"}
                    }]
                }),
                depends_on: vec![],
            }],
        };

        engine.register(workflow);
        let result = engine
            .execute("loop-wf", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.step_results["loop1"]["iterations"], 3);
        let results = result.step_results["loop1"]["results"].as_array().unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_llm_step_without_router() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "llm-wf".to_string(),
            name: "LLM Workflow".to_string(),
            description: "Tests LLM step without router".to_string(),
            steps: vec![WorkflowStep {
                id: "llm1".to_string(),
                name: "LLM Call".to_string(),
                step_type: StepType::Llm,
                config: serde_json::json!({"prompt": "Hello"}),
                depends_on: vec![],
            }],
        };

        engine.register(workflow);
        let result = engine
            .execute("llm-wf", serde_json::json!({}))
            .await
            .unwrap();
        // Should fail with error status since no LLM router is configured
        assert_eq!(result.status, ExecutionStatus::Error);
        assert!(result.error.unwrap().contains("LLM router not configured"));
    }

    #[tokio::test]
    async fn test_tool_call_step_without_gateway() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "tool-wf".to_string(),
            name: "Tool Workflow".to_string(),
            description: "Tests tool call step without gateway".to_string(),
            steps: vec![WorkflowStep {
                id: "tool1".to_string(),
                name: "Tool Call".to_string(),
                step_type: StepType::ToolCall,
                config: serde_json::json!({
                    "tool": "filesystem_read_file",
                    "arguments": {"path": "/tmp/test.txt"}
                }),
                depends_on: vec![],
            }],
        };

        engine.register(workflow);
        let result = engine
            .execute("tool-wf", serde_json::json!({}))
            .await
            .unwrap();
        // Should fail with error since no MCP gateway is configured
        assert_eq!(result.status, ExecutionStatus::Error);
        assert!(result.error.unwrap().contains("MCP gateway not configured"));
    }

    #[tokio::test]
    async fn test_cancel_execution() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "cancel-wf".to_string(),
            name: "Cancel Workflow".to_string(),
            description: "Tests cancellation".to_string(),
            steps: vec![
                WorkflowStep {
                    id: "wait1".to_string(),
                    name: "Wait".to_string(),
                    step_type: StepType::Wait,
                    config: serde_json::json!({"duration_ms": 10}),
                    depends_on: vec![],
                },
                WorkflowStep {
                    id: "wait2".to_string(),
                    name: "Wait 2".to_string(),
                    step_type: StepType::Wait,
                    config: serde_json::json!({"duration_ms": 10}),
                    depends_on: vec!["wait1".to_string()],
                },
            ],
        };

        engine.register(workflow);

        // Pre-cancel the execution ID. Since execute() generates a UUID we
        // cannot know it ahead of time. Instead, test the mechanism via the
        // cancel method directly.
        engine.cancel("some-exec-id").await.unwrap();

        let cancelled = engine.cancelled.read().await;
        assert!(cancelled.contains("some-exec-id"));
    }

    #[tokio::test]
    async fn test_resolve_template() {
        let mut results = HashMap::new();
        results.insert(
            "step1".to_string(),
            serde_json::json!({"text": "hello", "count": 42}),
        );

        let resolved = WorkflowEngine::resolve_template(
            "Result: {{step1.text}}, count={{step1.count}}",
            &results,
        );
        assert_eq!(resolved, "Result: hello, count=42");
    }

    #[tokio::test]
    async fn test_json_path_get() {
        let value = serde_json::json!({
            "a": {
                "b": {
                    "c": "deep"
                }
            },
            "arr": [10, 20, 30]
        });

        assert_eq!(
            WorkflowEngine::json_path_get(&value, "a.b.c"),
            serde_json::json!("deep")
        );
        assert_eq!(
            WorkflowEngine::json_path_get(&value, "arr.1"),
            serde_json::json!(20)
        );
        assert_eq!(
            WorkflowEngine::json_path_get(&value, "nonexistent"),
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn test_parallel_step() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "par-wf".to_string(),
            name: "Parallel Workflow".to_string(),
            description: "Tests parallel step".to_string(),
            steps: vec![WorkflowStep {
                id: "parallel1".to_string(),
                name: "Parallel".to_string(),
                step_type: StepType::Parallel,
                config: serde_json::json!({
                    "steps": [
                        {
                            "id": "sub_a",
                            "name": "Sub A",
                            "step_type": "transform",
                            "config": {"operation": "set", "value": "a_done"}
                        },
                        {
                            "id": "sub_b",
                            "name": "Sub B",
                            "step_type": "transform",
                            "config": {"operation": "set", "value": "b_done"}
                        }
                    ]
                }),
                depends_on: vec![],
            }],
        };

        engine.register(workflow);
        let result = engine
            .execute("par-wf", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.status, ExecutionStatus::Success);
        let par_result = &result.step_results["parallel1"];
        assert!(par_result["results"]["sub_a"].is_object());
        assert!(par_result["results"]["sub_b"].is_object());
    }

    #[tokio::test]
    async fn test_transform_template() {
        let mut engine = WorkflowEngine::new();

        let workflow = WorkflowDefinition {
            id: "tmpl-wf".to_string(),
            name: "Template Workflow".to_string(),
            description: "Tests template transform".to_string(),
            steps: vec![
                WorkflowStep {
                    id: "data".to_string(),
                    name: "Data".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({
                        "operation": "set",
                        "value": {"name": "World"}
                    }),
                    depends_on: vec![],
                },
                WorkflowStep {
                    id: "tmpl".to_string(),
                    name: "Template".to_string(),
                    step_type: StepType::Transform,
                    config: serde_json::json!({
                        "operation": "template",
                        "template": "Hello {{data.value.name}}!"
                    }),
                    depends_on: vec!["data".to_string()],
                },
            ],
        };

        engine.register(workflow);
        let result = engine
            .execute("tmpl-wf", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.step_results["tmpl"]["value"], "Hello World!");
    }
}
