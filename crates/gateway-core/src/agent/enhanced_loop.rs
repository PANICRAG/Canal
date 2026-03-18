//! Enhanced Agent Loop
//!
//! Integrates all the new agent components:
//! - Execution strategies (Parallel/Serial/Hybrid)
//! - Checkpoint and rollback system
//! - Hierarchical memory (WorkingMemory, SessionMemory)
//! - Error classification and recovery
//! - Workflow recording and learning
//! - Creative tool abstraction

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::memory::{
    ContextManager, ContextMemory, TaskStatus, ToolCallRecord,
    WorkingMemory, Verification, VerificationStatus,
};
use super::types::{
    AgentError, ErrorClassification, RecoveryDecision,
    ExecutionHints, ExecutionStrategy, RetryPolicy, ToolCategory,
    CheckpointConfig, CheckpointManager, Checkpoint,
};
use crate::creative::{CreativeToolManager, UnifiedApi};
use crate::error::Result;
use crate::llm::LlmRouter;
use crate::mcp::McpGateway;
use crate::workflow::recorder::WorkflowRecorder;

/// Enhanced agent loop configuration
#[derive(Debug, Clone)]
pub struct EnhancedLoopConfig {
    /// Maximum iterations before stopping
    pub max_iterations: u32,
    /// Whether to auto-execute safe operations
    pub auto_execute_safe: bool,
    /// Whether to auto-execute reversible operations
    pub auto_execute_reversible: bool,
    /// Always confirm sensitive operations
    pub always_confirm_sensitive: bool,
    /// Enable workflow learning
    pub enable_learning: bool,
    /// Checkpoint configuration
    pub checkpoint_config: CheckpointConfig,
}

impl Default for EnhancedLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            auto_execute_safe: true,
            auto_execute_reversible: true,
            always_confirm_sensitive: true,
            enable_learning: true,
            checkpoint_config: CheckpointConfig::default(),
        }
    }
}

/// Result of a single loop iteration
#[derive(Debug)]
pub enum IterationResult {
    /// Continue to next iteration
    Continue,
    /// Task completed successfully
    Complete { summary: String },
    /// Needs user input
    NeedsInput { question: String, options: Vec<String> },
    /// Error occurred
    Error { error: AgentError },
}

/// Enhanced agent loop with all integrated components
pub struct EnhancedAgentLoop {
    // Core components
    llm_router: Arc<LlmRouter>,
    mcp_gateway: Arc<McpGateway>,
    /// Unified Tool System (preferred over mcp_gateway when available)
    tool_system: Option<Arc<crate::tool_system::ToolSystem>>,

    // Memory system
    context_memory: Arc<RwLock<ContextMemory>>,
    context_manager: ContextManager,

    // Checkpoint system
    checkpoint_manager: Arc<RwLock<CheckpointManager>>,

    // Workflow system
    workflow_recorder: Arc<RwLock<WorkflowRecorder>>,

    // Creative tools (optional)
    creative_manager: Option<Arc<RwLock<CreativeToolManager>>>,

    // Configuration
    config: EnhancedLoopConfig,

    // State
    session_id: String,
    user_id: String,
    iteration_count: u32,
}

impl EnhancedAgentLoop {
    /// Create a new enhanced agent loop
    pub fn new(
        llm_router: Arc<LlmRouter>,
        mcp_gateway: Arc<McpGateway>,
        session_id: impl Into<String>,
        user_id: impl Into<String>,
    ) -> Self {
        let session_id = session_id.into();
        let user_id = user_id.into();

        Self {
            llm_router,
            mcp_gateway,
            tool_system: None,
            context_memory: Arc::new(RwLock::new(
                ContextMemory::new(&session_id, &user_id)
            )),
            context_manager: ContextManager::new(),
            checkpoint_manager: Arc::new(RwLock::new(CheckpointManager::new())),
            workflow_recorder: Arc::new(RwLock::new(WorkflowRecorder::new())),
            creative_manager: None,
            config: EnhancedLoopConfig::default(),
            session_id,
            user_id,
            iteration_count: 0,
        }
    }

    /// Enable creative tool support
    pub fn with_creative_tools(mut self) -> Self {
        self.creative_manager = Some(Arc::new(RwLock::new(CreativeToolManager::new())));
        self
    }

    /// Set configuration
    pub fn with_config(mut self, config: EnhancedLoopConfig) -> Self {
        self.config = config;
        self
    }

    /// Run the agent loop for a user request
    pub async fn run(&mut self, user_message: &str) -> Result<String> {
        // Initialize working memory with the task
        {
            let mut memory = self.context_memory.write().await;
            memory.working.start_task(user_message);
            memory.session.add_user_message(user_message.to_string());
        }

        // Main loop
        loop {
            self.iteration_count += 1;

            if self.iteration_count > self.config.max_iterations {
                return Err(crate::error::Error::Internal(
                    "Maximum iterations exceeded".to_string()
                ));
            }

            let result = self.iterate().await?;

            match result {
                IterationResult::Continue => continue,
                IterationResult::Complete { summary } => {
                    // Learn from successful completion
                    if self.config.enable_learning {
                        self.learn_from_success().await?;
                    }
                    return Ok(summary);
                }
                IterationResult::NeedsInput { question, .. } => {
                    // In a real implementation, this would pause and wait
                    // For now, return the question
                    return Ok(format!("Need input: {}", question));
                }
                IterationResult::Error { error } => {
                    // Attempt recovery
                    let recovery = self.attempt_recovery(&error).await?;
                    match recovery {
                        RecoveryDecision::Retry { delay_ms } => {
                            if let Some(delay) = delay_ms {
                                tokio::time::sleep(
                                    std::time::Duration::from_millis(delay)
                                ).await;
                            }
                            continue;
                        }
                        RecoveryDecision::Rollback { checkpoint_id } => {
                            self.rollback_to_checkpoint(&checkpoint_id).await?;
                            continue;
                        }
                        RecoveryDecision::AskUser { question, options } => {
                            return Ok(format!("Error recovery needed: {} Options: {:?}", question, options));
                        }
                        RecoveryDecision::Abort { reason } => {
                            return Err(crate::error::Error::Internal(reason));
                        }
                    }
                }
            }
        }
    }

    /// Single iteration of the agent loop (6-step Manus pattern)
    async fn iterate(&mut self) -> Result<IterationResult> {
        // Step 1: ANALYZE - Understand current state
        let analysis = self.analyze().await?;

        if analysis.task_complete {
            return Ok(IterationResult::Complete {
                summary: analysis.summary.unwrap_or_default(),
            });
        }

        // Step 2: PLAN - Determine next actions
        let plan = self.plan(&analysis).await?;

        // Step 3: EXECUTE - Run tool calls
        let execution_result = self.execute(&plan).await;

        // Step 4: VERIFY - Check results
        let verification = self.verify(&execution_result).await?;

        match verification.status {
            VerificationStatus::Success => {
                // Step 5: LEARN - Record successful pattern
                if self.config.enable_learning {
                    self.record_success(&plan).await?;
                }

                // Step 6: RESPOND - Update state
                self.update_state(&verification).await?;

                Ok(IterationResult::Continue)
            }
            VerificationStatus::Failed { reason } => {
                Ok(IterationResult::Error {
                    error: AgentError::execution_failed(reason),
                })
            }
            VerificationStatus::NeedsConfirmation { message } => {
                Ok(IterationResult::NeedsInput {
                    question: message,
                    options: vec!["Confirm".to_string(), "Cancel".to_string()],
                })
            }
            VerificationStatus::Pending => {
                Ok(IterationResult::Continue)
            }
        }
    }

    /// Step 1: Analyze current state
    async fn analyze(&self) -> Result<Analysis> {
        let memory = self.context_memory.read().await;

        // Check if current task is complete
        let current_task = memory.working.get_current_task();
        let task_complete = current_task
            .map(|t| t.status == TaskStatus::Completed)
            .unwrap_or(true);

        let summary = if task_complete {
            Some(self.generate_summary(&memory).await?)
        } else {
            None
        };

        Ok(Analysis {
            task_complete,
            summary,
            pending_tools: memory.working.get_pending_tool_calls(),
            context_needs_compression: self.context_manager.needs_compression(&memory),
        })
    }

    /// Step 2: Plan next actions
    async fn plan(&self, analysis: &Analysis) -> Result<ExecutionPlan> {
        // Check if context needs compression
        if analysis.context_needs_compression {
            let mut memory = self.context_memory.write().await;
            self.context_manager.compress(&mut memory)?;
        }

        // For now, return a simple plan
        // In production, this would use the LLM to generate a plan
        Ok(ExecutionPlan {
            steps: vec![],
            execution_strategy: ExecutionStrategy::Hybrid {
                parallel_threshold: 0.5,
                sensitive_tools: vec!["delete".to_string(), "write".to_string()],
            },
        })
    }

    /// Step 3: Execute planned actions
    async fn execute(&mut self, plan: &ExecutionPlan) -> ExecutionResult {
        let mut results = Vec::new();
        let start_time = Instant::now();

        for step in &plan.steps {
            // Determine if we need a checkpoint
            if step.hints.creates_checkpoint {
                let checkpoint = self.create_checkpoint(&step.tool_name).await;
                if let Ok(cp) = checkpoint {
                    let mut manager = self.checkpoint_manager.write().await;
                    manager.add_checkpoint(cp);
                }
            }

            // Check if confirmation is needed
            if step.hints.requires_confirmation && self.config.always_confirm_sensitive {
                return ExecutionResult::NeedsConfirmation {
                    step: step.clone(),
                    message: format!("Confirm execution of: {}", step.tool_name),
                };
            }

            // Execute the tool
            let tool_result = self.execute_tool(step).await;

            // Record the tool call
            {
                let mut memory = self.context_memory.write().await;
                memory.working.record_tool_call(ToolCallRecord {
                    id: Uuid::new_v4().to_string(),
                    tool_name: step.tool_name.clone(),
                    input: step.params.clone(),
                    output: tool_result.clone().ok(),
                    error: tool_result.clone().err().map(|e| e.to_string()),
                    duration_ms: start_time.elapsed().as_millis() as u64,
                    timestamp: chrono::Utc::now(),
                });
            }

            // Record in workflow recorder if recording
            {
                let mut recorder = self.workflow_recorder.write().await;
                if recorder.is_recording() {
                    recorder.record_action(
                        &step.tool_name,
                        step.params.clone(),
                        tool_result.clone().ok(),
                    );
                }
            }

            match tool_result {
                Ok(result) => results.push(result),
                Err(e) => {
                    return ExecutionResult::Error {
                        error: e,
                        partial_results: results,
                    };
                }
            }
        }

        ExecutionResult::Success { results }
    }

    /// Execute a single tool
    async fn execute_tool(&self, step: &ExecutionStep) -> std::result::Result<serde_json::Value, AgentError> {
        // Check if this is a creative tool
        if step.tool_name.starts_with("creative.") || step.tool_name.starts_with("davinci.")
           || step.tool_name.starts_with("premiere.") || step.tool_name.starts_with("finalcut.") {
            return self.execute_creative_tool(step).await;
        }

        // Execute via ToolSystem (preferred) or MCP gateway
        let result = if let Some(ref ts) = self.tool_system {
            ts.execute_llm_tool_call(&step.tool_name, step.params.clone())
                .await
                .map(|r| serde_json::to_value(&r).unwrap_or_default())
                .map_err(|e| AgentError::tool_failed(step.tool_name.clone(), e.to_string()))?
        } else {
            self.mcp_gateway
                .call_tool(&step.tool_name, step.params.clone())
                .await
                .map_err(|e| AgentError::tool_failed(step.tool_name.clone(), e.to_string()))?
        };

        Ok(result)
    }

    /// Execute a creative tool
    async fn execute_creative_tool(&self, step: &ExecutionStep) -> std::result::Result<serde_json::Value, AgentError> {
        let manager = self.creative_manager.as_ref()
            .ok_or_else(|| AgentError::tool_failed(
                step.tool_name.clone(),
                "Creative tools not enabled".to_string()
            ))?;

        let manager = manager.read().await;
        let api = manager.api().await
            .map_err(|e| AgentError::tool_failed(step.tool_name.clone(), e.to_string()))?;

        // Route to appropriate creative operation
        let result = api.execute(&step.tool_name, step.params.clone()).await
            .map_err(|e| AgentError::tool_failed(step.tool_name.clone(), e.to_string()))?;

        Ok(serde_json::to_value(result).unwrap_or_default())
    }

    /// Step 4: Verify execution results
    async fn verify(&self, result: &ExecutionResult) -> Result<Verification> {
        match result {
            ExecutionResult::Success { results } => {
                // Verify all results are valid
                let all_valid = results.iter().all(|r| !r.is_null());

                Ok(Verification {
                    id: Uuid::new_v4().to_string(),
                    description: "Execution verification".to_string(),
                    status: if all_valid {
                        VerificationStatus::Success
                    } else {
                        VerificationStatus::Failed {
                            reason: "Some results were null".to_string(),
                        }
                    },
                    checked_at: chrono::Utc::now(),
                })
            }
            ExecutionResult::Error { error, .. } => {
                Ok(Verification {
                    id: Uuid::new_v4().to_string(),
                    description: "Execution verification".to_string(),
                    status: VerificationStatus::Failed {
                        reason: error.to_string(),
                    },
                    checked_at: chrono::Utc::now(),
                })
            }
            ExecutionResult::NeedsConfirmation { message, .. } => {
                Ok(Verification {
                    id: Uuid::new_v4().to_string(),
                    description: "Awaiting confirmation".to_string(),
                    status: VerificationStatus::NeedsConfirmation {
                        message: message.clone(),
                    },
                    checked_at: chrono::Utc::now(),
                })
            }
        }
    }

    /// Step 5: Record successful pattern for learning
    async fn record_success(&self, _plan: &ExecutionPlan) -> Result<()> {
        // Record successful execution pattern
        // This would be used to build workflow templates
        Ok(())
    }

    /// Step 6: Update state after successful iteration
    async fn update_state(&self, _verification: &Verification) -> Result<()> {
        // Update working memory with results
        Ok(())
    }

    /// Attempt error recovery
    async fn attempt_recovery(&self, error: &AgentError) -> Result<RecoveryDecision> {
        let classification = error.classify();

        match classification {
            ErrorClassification::Transient => {
                Ok(RecoveryDecision::Retry { delay_ms: Some(1000) })
            }
            ErrorClassification::RateLimited { retry_after_ms } => {
                Ok(RecoveryDecision::Retry { delay_ms: Some(retry_after_ms) })
            }
            ErrorClassification::Recoverable { suggestion } => {
                // Check if we have a checkpoint to rollback to
                let manager = self.checkpoint_manager.read().await;
                if let Some(checkpoint) = manager.latest_checkpoint() {
                    Ok(RecoveryDecision::Rollback {
                        checkpoint_id: checkpoint.id.clone(),
                    })
                } else {
                    Ok(RecoveryDecision::AskUser {
                        question: format!("Error: {}. Suggested fix: {}", error, suggestion),
                        options: vec!["Retry".to_string(), "Skip".to_string(), "Abort".to_string()],
                    })
                }
            }
            ErrorClassification::Fatal { reason } => {
                Ok(RecoveryDecision::Abort { reason })
            }
        }
    }

    /// Rollback to a checkpoint
    async fn rollback_to_checkpoint(&self, checkpoint_id: &str) -> Result<()> {
        let mut manager = self.checkpoint_manager.write().await;
        manager.rollback(checkpoint_id)?;

        // Also rollback working memory
        let mut memory = self.context_memory.write().await;
        memory.working.rollback_to_checkpoint(checkpoint_id);

        Ok(())
    }

    /// Create a checkpoint before sensitive operation
    async fn create_checkpoint(&self, operation: &str) -> Result<Checkpoint> {
        let memory = self.context_memory.read().await;

        Ok(Checkpoint {
            id: Uuid::new_v4().to_string(),
            created_at: chrono::Utc::now(),
            operation: operation.to_string(),
            state_snapshot: serde_json::to_value(&memory.working).unwrap_or_default(),
            can_rollback: true,
        })
    }

    /// Learn from successful task completion
    async fn learn_from_success(&self) -> Result<()> {
        let memory = self.context_memory.read().await;
        let mut recorder = self.workflow_recorder.write().await;

        // If we were recording, stop and save the template
        if recorder.is_recording() {
            if let Some(template) = recorder.stop_recording() {
                // Save template to storage (would go to database)
                tracing::info!("Learned new workflow template: {}", template.name);
            }
        }

        // Analyze if current execution could become a template
        let tool_calls = memory.working.get_recent_tool_calls(10);
        if tool_calls.len() >= 3 {
            // Potential pattern detected
            tracing::debug!("Potential workflow pattern detected with {} steps", tool_calls.len());
        }

        Ok(())
    }

    /// Generate summary of completed task
    async fn generate_summary(&self, memory: &ContextMemory) -> Result<String> {
        let tool_calls = memory.working.get_recent_tool_calls(20);
        let success_count = tool_calls.iter().filter(|t| t.error.is_none()).count();

        Ok(format!(
            "Task completed. Executed {} tool calls ({} successful).",
            tool_calls.len(),
            success_count
        ))
    }

    // === Public API for workflow recording ===

    /// Start recording a workflow
    pub async fn start_recording(&self, name: &str) {
        let mut recorder = self.workflow_recorder.write().await;
        recorder.start_recording(name);
    }

    /// Stop recording and get the template
    pub async fn stop_recording(&self) -> Option<crate::workflow::recorder::WorkflowTemplate> {
        let mut recorder = self.workflow_recorder.write().await;
        recorder.stop_recording()
    }

    /// Check if currently recording
    pub async fn is_recording(&self) -> bool {
        let recorder = self.workflow_recorder.read().await;
        recorder.is_recording()
    }
}

// === Supporting Types ===

#[derive(Debug)]
struct Analysis {
    task_complete: bool,
    summary: Option<String>,
    pending_tools: Vec<String>,
    context_needs_compression: bool,
}

#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    pub steps: Vec<ExecutionStep>,
    pub execution_strategy: ExecutionStrategy,
}

#[derive(Debug, Clone)]
pub struct ExecutionStep {
    pub tool_name: String,
    pub params: serde_json::Value,
    pub hints: ExecutionHints,
}

#[derive(Debug)]
pub enum ExecutionResult {
    Success { results: Vec<serde_json::Value> },
    Error { error: AgentError, partial_results: Vec<serde_json::Value> },
    NeedsConfirmation { step: ExecutionStep, message: String },
}
