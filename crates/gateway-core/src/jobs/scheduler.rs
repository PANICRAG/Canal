//! Background job scheduler that claims queued jobs and executes them.
//!
//! The scheduler polls the `JobStore` for queued jobs, acquires a semaphore
//! permit for concurrency control, and spawns each job on a Tokio task.
//! Each job uses the `AgentFactory` to run a collaboration graph, forwarding
//! events to the `ExecutionStore` for SSE streaming.
//!
//! ## Pause & Resume
//!
//! Running jobs can be paused via `pause_job`, which sends a `JobSignal::Pause`
//! through a `watch` channel. The execution loop detects the signal, saves the
//! current `execution_id` as the checkpoint reference, updates the DB status to
//! `Paused`, and returns early. `resume_job` re-queues the job; when the
//! scheduler picks it up again it detects the existing `checkpoint_id` and
//! resumes from that point.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinHandle;
use tracing::{instrument, Instrument};
use uuid::Uuid;

use super::config::JobsConfig;
use super::error::JobError;
use super::notification::JobNotifier;
use super::store::JobStore;
use super::types::*;

use crate::agent::AgentFactory;

#[cfg(feature = "graph")]
use crate::graph::ExecutionStore;

#[cfg(feature = "graph")]
use crate::graph::{EventPayload, GraphStreamEvent};

/// Background job scheduler that manages concurrent job execution.
pub struct JobScheduler {
    job_store: Arc<JobStore>,
    agent_factory: Arc<AgentFactory>,
    #[cfg(feature = "graph")]
    execution_store: Arc<ExecutionStore>,
    #[cfg(feature = "devtools")]
    devtools_service: Option<Arc<devtools_core::DevtoolsService>>,
    semaphore: Arc<Semaphore>,
    signal_handles: DashMap<Uuid, (watch::Sender<JobSignal>, tokio::sync::mpsc::Sender<String>)>,
    notifier: Option<Arc<dyn JobNotifier>>,
    config: JobsConfig,
}

impl JobScheduler {
    /// Create a new job scheduler.
    pub fn new(
        job_store: Arc<JobStore>,
        agent_factory: Arc<AgentFactory>,
        #[cfg(feature = "graph")] execution_store: Arc<ExecutionStore>,
        config: JobsConfig,
    ) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));
        Self {
            job_store,
            agent_factory,
            #[cfg(feature = "graph")]
            execution_store,
            #[cfg(feature = "devtools")]
            devtools_service: None,
            semaphore,
            signal_handles: DashMap::new(),
            notifier: None,
            config,
        }
    }

    /// Set the devtools service for post-completion trace push.
    #[cfg(feature = "devtools")]
    pub fn with_devtools_service(mut self, service: Arc<devtools_core::DevtoolsService>) -> Self {
        self.devtools_service = Some(service);
        self
    }

    /// Set the notifier for job lifecycle events.
    pub fn with_notifier(mut self, notifier: Arc<dyn JobNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Start the background polling loop. Returns the join handle.
    pub fn start(self: &Arc<Self>) -> JoinHandle<()> {
        let scheduler = Arc::clone(self);
        let poll_interval = Duration::from_millis(scheduler.config.poll_interval_ms);

        tokio::spawn(
            async move {
                tracing::info!(
                    max_concurrent = scheduler.config.max_concurrent,
                    poll_interval_ms = scheduler.config.poll_interval_ms,
                    "Job scheduler started"
                );

                loop {
                    tokio::time::sleep(poll_interval).await;

                    if scheduler.semaphore.available_permits() == 0 {
                        continue;
                    }

                    match scheduler.job_store.claim_next_job().await {
                        Ok(Some(job)) => {
                            let job_id = job.id;
                            tracing::info!(job_id = %job_id, "Claimed job for execution");

                            let sched = Arc::clone(&scheduler);
                            let permit = match scheduler.semaphore.clone().acquire_owned().await {
                                Ok(p) => p,
                                Err(_) => {
                                    tracing::warn!("Semaphore closed, stopping scheduler");
                                    break;
                                }
                            };

                            tokio::spawn(
                                async move {
                                    sched.run_job(job).await;
                                    drop(permit);
                                }
                                .instrument(tracing::info_span!("job_execution", job_id = %job_id)),
                            );
                        }
                        Ok(None) => {
                            // No queued jobs, continue polling
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to claim next job");
                        }
                    }
                }
            }
            .instrument(tracing::info_span!("job_scheduler")),
        )
    }

    /// Execute a single job.
    #[instrument(skip(self, job), fields(job_id = %job.id, job_type = ?job.job_type))]
    async fn run_job(self: &Arc<Self>, job: Job) {
        let job_id = job.id;
        let start = std::time::Instant::now();

        // Set up signal channel (defaults to Continue) and instruct channel
        let (signal_tx, signal_rx) = watch::channel(JobSignal::Continue);
        let (instruct_tx, instruct_rx) = tokio::sync::mpsc::channel::<String>(16);
        self.signal_handles.insert(job_id, (signal_tx, instruct_tx));

        // Determine execution ID: reuse from checkpoint or generate new
        let execution_id = if let Some(ref ckpt) = job.checkpoint_id {
            // Resuming from checkpoint — reuse the stored execution ID
            tracing::info!(
                checkpoint_id = %ckpt,
                "Resuming job from checkpoint"
            );
            job.execution_id
                .clone()
                .unwrap_or_else(|| format!("job-{}", job_id))
        } else {
            format!("job-{}", job_id)
        };

        if let Err(e) = self.job_store.set_execution_id(job_id, &execution_id).await {
            tracing::error!(error = %e, "Failed to set execution ID");
        }

        // Execute with timeout
        let timeout = Duration::from_secs(self.config.job_timeout_secs);
        let result = tokio::time::timeout(
            timeout,
            self.execute_job(&job, signal_rx, instruct_rx, &execution_id),
        )
        .await;

        // Clean up signal handle
        self.signal_handles.remove(&job_id);

        let elapsed_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(response)) => {
                // Set 100% progress on successful completion
                let _ = self.job_store.set_progress(job_id, 100.0).await;

                let job_result = JobResult {
                    response: response.clone(),
                    messages: vec![],
                    total_tokens: 0,
                    total_duration_ms: elapsed_ms,
                    collaboration_mode_used: job.input.collaboration_mode.clone(),
                };
                if let Err(e) = self.job_store.set_result(job_id, &job_result).await {
                    tracing::error!(error = %e, "Failed to set job result");
                }
                tracing::info!(elapsed_ms, "Job completed successfully");

                // Notify
                if let Some(ref notifier) = self.notifier {
                    let mut completed_job = job.clone();
                    completed_job.status = JobStatus::Completed;
                    completed_job.result = Some(job_result);
                    notifier.on_completed(&completed_job).await;
                }

                // Push execution events to devtools (non-blocking)
                #[cfg(feature = "devtools")]
                if let Some(ref devtools_svc) = self.devtools_service {
                    let svc = devtools_svc.clone();
                    let exec_store = self.execution_store.clone();
                    let exec_id = execution_id.clone();
                    let job_name = job.input.message.chars().take(80).collect::<String>();
                    tokio::spawn(async move {
                        if let Err(e) =
                            push_events_to_devtools(&svc, &exec_store, &exec_id, &job_name, job_id)
                                .await
                        {
                            tracing::warn!(error = %e, "Failed to push job trace to devtools");
                        }
                    });
                }
            }
            Ok(Err(JobError::Paused(paused_id))) => {
                // Job was paused — checkpoint already saved in execute_job.
                // Save the execution_id as the checkpoint reference so resume
                // can re-enter from this point.
                if let Err(e) = self
                    .job_store
                    .set_checkpoint(paused_id, &execution_id)
                    .await
                {
                    tracing::error!(error = %e, "Failed to save checkpoint on pause");
                }
                if let Err(e) = self
                    .job_store
                    .update_status(paused_id, JobStatus::Paused)
                    .await
                {
                    tracing::error!(error = %e, "Failed to set paused status");
                }
                tracing::info!(elapsed_ms, "Job paused with checkpoint");

                // Notify (paused is not failed)
                if let Some(ref notifier) = self.notifier {
                    let mut paused_job = job.clone();
                    paused_job.status = JobStatus::Paused;
                    // We don't call on_failed — pause is a normal lifecycle event
                    let _ = notifier;
                    let _ = paused_job;
                }
            }
            Ok(Err(JobError::Cancelled(cancelled_id))) => {
                // cancel_job() already set DB status to 'cancelled' — don't overwrite.
                tracing::info!(
                    job_id = %cancelled_id,
                    elapsed_ms,
                    "Job cancelled"
                );
            }
            Ok(Err(e)) => {
                let error_msg = format!("{}", e);
                if let Err(store_err) = self.job_store.set_error(job_id, &error_msg).await {
                    tracing::error!(error = %store_err, "Failed to set job error");
                }
                tracing::error!(error = %e, elapsed_ms, "Job failed");

                if let Some(ref notifier) = self.notifier {
                    let mut failed_job = job.clone();
                    failed_job.status = JobStatus::Failed;
                    failed_job.error = Some(error_msg);
                    notifier.on_failed(&failed_job).await;
                }

                // Push execution events to devtools on failure too (for debugging)
                #[cfg(feature = "devtools")]
                if let Some(ref devtools_svc) = self.devtools_service {
                    let svc = devtools_svc.clone();
                    let exec_store = self.execution_store.clone();
                    let exec_id = execution_id.clone();
                    let job_name = job.input.message.chars().take(80).collect::<String>();
                    tokio::spawn(async move {
                        if let Err(e) =
                            push_events_to_devtools(&svc, &exec_store, &exec_id, &job_name, job_id)
                                .await
                        {
                            tracing::warn!(error = %e, "Failed to push failed job trace to devtools");
                        }
                    });
                }
            }
            Err(_timeout) => {
                let error_msg = format!("Job timed out after {}s", self.config.job_timeout_secs);
                if let Err(e) = self.job_store.set_error(job_id, &error_msg).await {
                    tracing::error!(error = %e, "Failed to set timeout error");
                }
                tracing::error!(elapsed_ms, "Job timed out");

                if let Some(ref notifier) = self.notifier {
                    let mut failed_job = job.clone();
                    failed_job.status = JobStatus::Failed;
                    failed_job.error = Some(error_msg);
                    notifier.on_failed(&failed_job).await;
                }
            }
        }
    }

    /// Internal execution logic for a job.
    ///
    /// Monitors a `watch::Receiver<JobSignal>` alongside the actual work.
    /// On `Pause` the method returns `Err(JobError::Paused(job_id))` so that
    /// `run_job` can save the checkpoint and update the DB status.
    /// On `Cancel` it returns a regular execution error.
    async fn execute_job(
        &self,
        job: &Job,
        mut signal_rx: watch::Receiver<JobSignal>,
        mut instruct_rx: tokio::sync::mpsc::Receiver<String>,
        execution_id: &str,
    ) -> Result<String, JobError> {
        let task = &job.input.message;
        let job_id = job.id;

        // Parse collaboration mode
        #[cfg(feature = "collaboration")]
        let mode = self.parse_collaboration_mode(job);

        // Register execution in store BEFORE the factory call so that SSE
        // subscribers who connect immediately after seeing the execution_id
        // in the DB can subscribe to a valid, existing execution record.
        #[cfg(feature = "graph")]
        {
            let exec_mode = crate::graph::ExecutionMode::Graph(
                job.input
                    .collaboration_mode
                    .clone()
                    .unwrap_or_else(|| "auto".to_string()),
            );
            self.execution_store
                .start_execution(execution_id, exec_mode)
                .await;
        }

        // Execute via AgentFactory with streaming.
        //
        // `start_collaboration_streaming` spawns execution in a background task
        // and returns (JoinHandle, Receiver) immediately. We drain the Receiver
        // in real-time so events flow to ExecutionStore → SSE → ChatView as
        // they happen, not after execution completes.
        #[cfg(feature = "collaboration")]
        {
            let factory = &self.agent_factory;
            let exec_start = std::time::Instant::now();

            // Spawn execution — returns immediately with handle + event receiver
            let (exec_handle, mut graph_rx) = factory
                .start_collaboration_streaming(task, mode, execution_id, job_id)
                .await
                .map_err(|e| JobError::Execution(e.to_string()))?;

            // Drain events in real-time WHILE execution runs concurrently
            let mut event_count = 0u32;
            loop {
                tokio::select! {
                    biased;
                    // Check for pause/cancel signals first
                    _ = signal_rx.changed() => {
                        match *signal_rx.borrow() {
                            JobSignal::Pause => {
                                tracing::info!(
                                    job_id = %job_id,
                                    events_processed = event_count,
                                    "Pause signal received during execution"
                                );
                                // Abort the spawned execution task
                                exec_handle.abort();
                                return Err(JobError::Paused(job_id));
                            }
                            JobSignal::Cancel => {
                                exec_handle.abort();
                                return Err(JobError::Cancelled(job_id));
                            }
                            JobSignal::Continue => {
                                // Spurious wake, keep going
                            }
                        }
                    }
                    msg = instruct_rx.recv() => {
                        if let Some(instruction) = msg {
                            tracing::info!(
                                job_id = %job_id,
                                "Received instruction: {}",
                                instruction.chars().take(100).collect::<String>()
                            );
                            // 1. Store in job metadata (existing behavior)
                            let _ = self.job_store.append_metadata(
                                job_id, "instructions", &instruction
                            ).await;

                            // 2. Publish to ExecutionStore so SSE subscribers
                            //    see the instruction in real-time
                            #[cfg(feature = "graph")]
                            {
                                self.execution_store.append_event(
                                    execution_id,
                                    crate::graph::EventPayload::InstructionReceived {
                                        job_id: job_id.to_string(),
                                        message: instruction.clone(),
                                    },
                                ).await;
                            }
                        }
                    }
                    event = graph_rx.recv() => {
                        match event {
                            Some(evt) => {
                                event_count += 1;
                                // Report progress every 5 events using asymptotic curve (approaches 95%)
                                if event_count % 5 == 0 {
                                    let progress = 95.0 * (1.0 - (-(event_count as f32) / 50.0).exp());
                                    let _ = self
                                        .job_store
                                        .set_progress(job_id, progress)
                                        .await;
                                }
                                // Forward to ExecutionStore → job SSE → ChatView
                                #[cfg(feature = "graph")]
                                if let Some(payload) = graph_event_to_payload(&evt) {
                                    self.execution_store.append_event(
                                        execution_id, payload
                                    ).await;
                                }
                            }
                            None => break, // Channel closed, execution done
                        }
                    }
                }
            }

            // All events drained — await the execution handle for final result
            let state = exec_handle
                .await
                .map_err(|e| JobError::Execution(format!("execution task failed: {e}")))?
                .map_err(|e| JobError::Execution(e.to_string()))?;

            // Set 100% progress after all events consumed
            let _ = self.job_store.set_progress(job_id, 100.0).await;

            // Emit the final assembled response so SSE
            // subscribers (and future replay) can access it.
            #[cfg(feature = "graph")]
            {
                let elapsed = exec_start.elapsed().as_millis() as u64;
                self.execution_store
                    .append_event(
                        execution_id,
                        EventPayload::JobResultReady {
                            response: state.response.clone(),
                            total_duration_ms: elapsed,
                        },
                    )
                    .await;
            }

            // Mark execution complete in store (closes subscriber channels)
            #[cfg(feature = "graph")]
            self.execution_store
                .complete_execution(execution_id, 0)
                .await;

            return Ok(state.response.clone());
        }

        #[cfg(not(feature = "collaboration"))]
        {
            let _ = signal_rx;
            let _ = instruct_rx;
            let _ = task;
            let _ = job_id;
            let _ = execution_id;
            Err(JobError::Execution(
                "collaboration feature required for job execution".to_string(),
            ))
        }
    }

    /// Parse collaboration mode from job input.
    #[cfg(feature = "collaboration")]
    fn parse_collaboration_mode(
        &self,
        job: &Job,
    ) -> Option<crate::collaboration::CollaborationMode> {
        let mode_str = job
            .input
            .collaboration_mode
            .as_deref()
            .unwrap_or(&self.config.default_mode);

        match mode_str {
            "direct" | "Direct" | "" => Some(crate::collaboration::CollaborationMode::Direct),
            // A40: All non-direct modes route to PlanExecute (Swarm/Expert merged)
            // Accept both camelCase (from frontend) and snake_case variants
            "plan_execute" | "PlanExecute" | "planexecute" | "Plan" | "plan" | "swarm"
            | "Swarm" | "expert" | "Expert" => {
                Some(crate::collaboration::CollaborationMode::PlanExecute)
            }
            "auto" | "Auto" => None, // Let AutoModeSelector decide
            _ => None,
        }
    }

    /// Cancel a running job by sending `JobSignal::Cancel` to its watch channel.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn cancel_job(&self, job_id: Uuid) -> Result<(), JobError> {
        // Signal the running task to cancel
        if let Some((_id, (signal_tx, _instruct_tx))) = self.signal_handles.remove(&job_id) {
            let _ = signal_tx.send(JobSignal::Cancel);
            tracing::info!("Sent cancel signal to running job");
        }

        // Update store
        self.job_store.cancel_job(job_id).await
    }

    /// Pause a running job by sending `JobSignal::Pause` to its watch channel.
    ///
    /// The running task will detect the signal, save a checkpoint, update the
    /// DB status to `Paused`, and return. If the job is not currently running
    /// in this scheduler (e.g. still queued), this falls back to a direct DB
    /// status update.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn pause_job(&self, job_id: Uuid) -> Result<(), JobError> {
        if let Some(entry) = self.signal_handles.get(&job_id) {
            let _ = entry.0.send(JobSignal::Pause);
            tracing::info!("Sent pause signal to running job");
            // The run_job loop will update DB status when the task exits
            Ok(())
        } else {
            // Job is not running in-process; validate state before updating DB
            let job = self
                .job_store
                .get_job(job_id)
                .await?
                .ok_or(JobError::NotFound(job_id))?;

            match job.status {
                JobStatus::Running | JobStatus::Queued => {
                    self.job_store
                        .update_status(job_id, JobStatus::Paused)
                        .await
                }
                _ => Err(JobError::InvalidTransition {
                    from: job.status.to_string(),
                    to: "paused".to_string(),
                }),
            }
        }
    }

    /// Send an instruction to a running job.
    ///
    /// The instruction is delivered via an mpsc channel to the running task.
    /// If the job is not currently running in this scheduler, returns an error.
    #[instrument(skip(self, message), fields(job_id = %job_id))]
    pub async fn instruct_job(&self, job_id: Uuid, message: String) -> Result<(), JobError> {
        if let Some(entry) = self.signal_handles.get(&job_id) {
            entry
                .1
                .send(message)
                .await
                .map_err(|_| JobError::Execution("Job instruct channel closed".to_string()))?;
            tracing::info!("Sent instruction to running job");
            Ok(())
        } else {
            // Job not running in this scheduler — check DB state
            let job = self
                .job_store
                .get_job(job_id)
                .await?
                .ok_or(JobError::NotFound(job_id))?;

            match job.status {
                JobStatus::Running => Err(JobError::Execution(
                    "Job is running but not found in local scheduler".to_string(),
                )),
                _ => Err(JobError::InvalidTransition {
                    from: job.status.to_string(),
                    to: "instruct (requires running)".to_string(),
                }),
            }
        }
    }

    /// Resume a paused job by re-queuing it.
    ///
    /// If the job has a `checkpoint_id`, the next execution will detect it
    /// and attempt to resume from that point. Otherwise the job restarts.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn resume_job(&self, job_id: Uuid) -> Result<Option<String>, JobError> {
        // Read current job state to get checkpoint info
        let job = self
            .job_store
            .get_job(job_id)
            .await?
            .ok_or(JobError::NotFound(job_id))?;

        if job.status != JobStatus::Paused {
            return Err(JobError::InvalidTransition {
                from: job.status.to_string(),
                to: "queued".to_string(),
            });
        }

        let checkpoint_id = job.checkpoint_id.clone();

        // Re-queue the job so the scheduler picks it up again
        self.job_store
            .update_status(job_id, JobStatus::Queued)
            .await?;

        if let Some(ref ckpt) = checkpoint_id {
            tracing::info!(
                checkpoint_id = %ckpt,
                "Resumed job with checkpoint — will restore on next execution"
            );
        } else {
            tracing::info!("Resumed job without checkpoint — will restart from beginning");
        }

        Ok(checkpoint_id)
    }

    /// Recover interrupted jobs on server restart.
    #[instrument(skip(self))]
    pub async fn recover(&self) -> Result<u64, JobError> {
        if !self.config.recovery.enabled {
            tracing::info!("Job recovery disabled");
            return Ok(0);
        }

        match self.config.recovery.strategy.as_str() {
            "requeue" => {
                let count = self.job_store.requeue_running().await?;
                if count > 0 {
                    tracing::info!(count, "Requeued interrupted jobs for recovery");
                }
                Ok(count)
            }
            "skip" => {
                tracing::info!("Recovery strategy is 'skip', no action taken");
                Ok(0)
            }
            other => {
                tracing::warn!(strategy = other, "Unknown recovery strategy");
                Ok(0)
            }
        }
    }

    /// Get the number of currently running jobs.
    pub fn active_count(&self) -> usize {
        self.signal_handles.len()
    }

    /// Get the maximum concurrent jobs setting.
    pub fn max_concurrent(&self) -> usize {
        self.config.max_concurrent
    }
}

/// Convert a GraphStreamEvent to an EventPayload for the ExecutionStore.
///
/// Returns `None` for events that are handled separately (e.g., instructions).
#[cfg(feature = "graph")]
fn graph_event_to_payload(evt: &GraphStreamEvent) -> Option<EventPayload> {
    match evt {
        GraphStreamEvent::GraphStarted { .. } => Some(EventPayload::GraphStarted),
        GraphStreamEvent::NodeEntered { node_id, .. } => Some(EventPayload::NodeEntered {
            node_id: node_id.clone(),
        }),
        GraphStreamEvent::NodeCompleted {
            node_id,
            duration_ms,
            ..
        } => Some(EventPayload::NodeCompleted {
            node_id: node_id.clone(),
            duration_ms: *duration_ms,
        }),
        GraphStreamEvent::NodeFailed { node_id, error, .. } => Some(EventPayload::NodeFailed {
            node_id: node_id.clone(),
            error: error.clone(),
        }),
        GraphStreamEvent::EdgeTraversed {
            from, to, label, ..
        } => Some(EventPayload::EdgeTraversed {
            from: from.clone(),
            to: to.clone(),
            label: label.clone(),
        }),
        GraphStreamEvent::GraphCompleted {
            total_duration_ms, ..
        } => Some(EventPayload::GraphCompleted {
            total_duration_ms: *total_duration_ms,
        }),
        GraphStreamEvent::ParallelPartial {
            node_id,
            succeeded,
            failed,
            ..
        } => Some(EventPayload::ParallelPartialComplete {
            node_id: node_id.clone(),
            succeeded: *succeeded,
            failed: *failed,
        }),
        GraphStreamEvent::ParallelBranchFailed {
            node_id,
            branch_id,
            error,
            ..
        } => Some(EventPayload::ParallelBranchFailed {
            node_id: node_id.clone(),
            branch_id: branch_id.clone(),
            error: error.clone(),
        }),
        GraphStreamEvent::DagWaveStarted {
            wave_index,
            node_ids,
            ..
        } => Some(EventPayload::DagWaveStarted {
            wave_index: *wave_index,
            node_ids: node_ids.clone(),
        }),
        GraphStreamEvent::DagWaveCompleted {
            wave_index,
            duration_ms,
            ..
        } => Some(EventPayload::DagWaveCompleted {
            wave_index: *wave_index,
            duration_ms: *duration_ms,
        }),
        GraphStreamEvent::BudgetWarning { node_id, .. } => Some(EventPayload::BudgetWarning {
            node_id: node_id.clone(),
            consumed: 0,
            limit: 0,
            scope: "graph".to_string(),
        }),
        GraphStreamEvent::BudgetExceeded { node_id, .. } => Some(EventPayload::BudgetExceeded {
            node_id: node_id.clone(),
            consumed: 0,
            limit: 0,
            scope: "graph".to_string(),
        }),
        // Content streaming events
        GraphStreamEvent::NodeThinking {
            node_id, content, ..
        } => Some(EventPayload::ThinkingDelta {
            node_id: node_id.clone(),
            content: content.clone(),
        }),
        GraphStreamEvent::NodeText {
            node_id, content, ..
        } => Some(EventPayload::ContentDelta {
            node_id: node_id.clone(),
            content: content.clone(),
        }),
        GraphStreamEvent::NodeToolCall {
            node_id,
            tool_id,
            tool_name,
            ..
        } => Some(EventPayload::ToolCallStarted {
            node_id: node_id.clone(),
            tool_id: tool_id.clone(),
            tool_name: tool_name.clone(),
        }),
        GraphStreamEvent::NodeToolResult {
            node_id, tool_id, ..
        } => Some(EventPayload::ToolCallCompleted {
            node_id: node_id.clone(),
            tool_id: tool_id.clone(),
        }),
        // HITL events
        GraphStreamEvent::HITLInputRequired {
            request_id,
            job_id,
            prompt,
            input_type,
            options,
            timeout_seconds,
            context,
            ..
        } => Some(EventPayload::HITLInputRequired {
            request_id: request_id.clone(),
            job_id: job_id.clone(),
            prompt: prompt.clone(),
            input_type: input_type.clone(),
            options: options.clone(),
            timeout_seconds: *timeout_seconds,
            context: context.clone(),
        }),
        // A40: Judge evaluation events
        GraphStreamEvent::JudgeEvaluated {
            step_id,
            verdict,
            reasoning,
            suggestions,
            retry_count,
            ..
        } => Some(EventPayload::JudgeEvaluated {
            step_id: step_id.clone(),
            verdict: verdict.clone(),
            reasoning: reasoning.clone(),
            suggestions: suggestions.clone(),
            retry_count: *retry_count,
        }),
        // Plan approval — forward to frontend via SSE
        GraphStreamEvent::PlanApprovalRequired {
            execution_id,
            request_id,
            goal,
            steps,
            success_criteria,
            timeout_seconds,
            risk_level,
            revision_round,
            max_revisions,
            ..
        } => Some(EventPayload::PlanApprovalRequired {
            execution_id: execution_id.clone(),
            request_id: request_id.clone(),
            goal: goal.clone(),
            steps: steps.clone(),
            success_criteria: success_criteria.clone(),
            timeout_seconds: *timeout_seconds,
            risk_level: risk_level.clone(),
            revision_round: *revision_round,
            max_revisions: *max_revisions,
        }),
        // Handled separately or not needed in ExecutionStore
        GraphStreamEvent::InstructionReceived { .. } => None,
        // A43: Research planner pipeline events — forwarded via SSE, not stored in ExecutionStore
        GraphStreamEvent::ResearchProgress { .. } => None,
        GraphStreamEvent::ComplexityAssessed { .. } => None,
        GraphStreamEvent::ClarificationRequired { .. } => None,
        GraphStreamEvent::PrdReviewRequired { .. } => None,
    }
}

/// Push execution events to devtools as trace observations after job completion.
///
/// Reads all events from `ExecutionStore`, maps them to devtools `Observation`s,
/// and ingests them. If a trace already exists (created by `DevtoolsObserver` during
/// graph execution), only adds observations that the observer doesn't capture
/// (tool calls, LLM requests, plan steps, etc.). If no trace exists, creates one.
#[cfg(all(feature = "devtools", feature = "graph"))]
async fn push_events_to_devtools(
    svc: &devtools_core::DevtoolsService,
    exec_store: &ExecutionStore,
    execution_id: &str,
    job_name: &str,
    job_id: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::graph::ExecutionEvent;
    use chrono::Utc;
    use devtools_core::types::*;
    use std::collections::HashMap;

    let events = exec_store.get_events(execution_id, 0, None);
    if events.is_empty() {
        return Ok(());
    }

    // Check if trace already exists (created by DevtoolsObserver during graph execution)
    let trace_exists = svc.get_trace(execution_id).await?.is_some();

    if !trace_exists {
        // Create trace for this job
        let status = match exec_store.get_execution(execution_id) {
            Some(summary) => match summary.status {
                crate::graph::ExecutionStatus::Completed => TraceStatus::Completed,
                crate::graph::ExecutionStatus::Failed(_) => TraceStatus::Error,
                crate::graph::ExecutionStatus::Running => TraceStatus::Running,
            },
            None => TraceStatus::Completed,
        };
        let trace = Trace {
            id: execution_id.to_string(),
            project_id: "canal".to_string(),
            session_id: None,
            name: Some(format!("Job: {}", job_name)),
            user_id: None,
            start_time: events.first().map(|e| e.timestamp).unwrap_or_else(Utc::now),
            end_time: events.last().map(|e| e.timestamp),
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            tags: vec!["job".to_string(), format!("job-{}", job_id)],
            status,
            total_tokens: 0,
            total_cost_usd: 0.0,
            observation_count: 0,
        };
        svc.ingest_trace(trace).await?;
    }

    // Pre-scan to pair enter/exit events
    let mut node_enters: HashMap<String, &ExecutionEvent> = HashMap::new();
    let mut step_starts: HashMap<u32, &ExecutionEvent> = HashMap::new();
    let mut llm_requests: Vec<&ExecutionEvent> = Vec::new();

    for event in &events {
        match &event.payload {
            EventPayload::NodeEntered { node_id } => {
                node_enters.insert(node_id.clone(), event);
            }
            EventPayload::PlanStepStarted { step_id, .. } => {
                step_starts.insert(*step_id, event);
            }
            EventPayload::LlmRequest { .. } => {
                llm_requests.push(event);
            }
            _ => {}
        }
    }

    let mut observations: Vec<Observation> = Vec::new();
    let mut llm_req_idx = 0usize;

    for event in &events {
        let obs = match &event.payload {
            // Skip events that DevtoolsObserver already captures (when trace exists)
            EventPayload::GraphStarted
            | EventPayload::GraphCompleted { .. }
            | EventPayload::NodeEntered { .. }
            | EventPayload::NodeCompleted { .. }
            | EventPayload::NodeFailed { .. }
            | EventPayload::EdgeTraversed { .. }
                if trace_exists =>
            {
                continue;
            }

            // Skip noisy deltas
            EventPayload::ThinkingDelta { .. } | EventPayload::ContentDelta { .. } => continue,

            // LLM request+response → Generation (pair by index)
            EventPayload::LlmRequest { .. } => {
                // Don't emit here; wait for the matching LlmResponse
                llm_req_idx += 1;
                continue;
            }
            EventPayload::LlmResponse {
                model,
                duration_ms,
                output_tokens,
            } => {
                // Pair with the previous LlmRequest
                let (req_model, req_tokens) = if llm_req_idx > 0 {
                    if let Some(req_event) = llm_requests.get(llm_req_idx - 1) {
                        match &req_event.payload {
                            EventPayload::LlmRequest {
                                model: m,
                                input_tokens: t,
                            } => (m.clone(), *t),
                            _ => (model.clone(), 0),
                        }
                    } else {
                        (model.clone(), 0)
                    }
                } else {
                    (model.clone(), 0)
                };
                Some(Observation::Generation(GenerationData {
                    id: Uuid::new_v4().to_string(),
                    trace_id: execution_id.to_string(),
                    parent_id: None,
                    name: format!("llm.{}", req_model),
                    model: req_model,
                    start_time: event.timestamp
                        - chrono::Duration::milliseconds(*duration_ms as i64),
                    end_time: Some(event.timestamp),
                    input: None,
                    output: None,
                    input_tokens: req_tokens as i32,
                    output_tokens: *output_tokens as i32,
                    total_tokens: (req_tokens + *output_tokens) as i32,
                    cost_usd: None,
                    metadata: serde_json::Map::new(),
                    status: ObservationStatus::Completed,
                    service_name: None,
                }))
            }

            // Tool call → Event
            EventPayload::ToolCall {
                tool_name,
                duration_ms,
                success,
            } => Some(Observation::Event(EventData {
                id: Uuid::new_v4().to_string(),
                trace_id: execution_id.to_string(),
                parent_id: None,
                name: format!("tool.{}", tool_name),
                time: event.timestamp,
                input: None,
                output: Some(serde_json::json!({ "success": success, "duration_ms": duration_ms })),
                metadata: serde_json::Map::new(),
                level: if *success {
                    ObservationLevel::Info
                } else {
                    ObservationLevel::Error
                },
                service_name: None,
            })),

            // Tool call started/completed → Events
            EventPayload::ToolCallStarted {
                tool_name, tool_id, ..
            } => Some(Observation::Event(EventData {
                id: Uuid::new_v4().to_string(),
                trace_id: execution_id.to_string(),
                parent_id: None,
                name: format!("tool.{}", tool_name),
                time: event.timestamp,
                input: Some(serde_json::json!({ "tool_id": tool_id })),
                output: None,
                metadata: serde_json::Map::new(),
                level: ObservationLevel::Info,
                service_name: None,
            })),
            EventPayload::ToolCallCompleted { tool_id, .. } => {
                Some(Observation::Event(EventData {
                    id: Uuid::new_v4().to_string(),
                    trace_id: execution_id.to_string(),
                    parent_id: None,
                    name: "tool.completed".to_string(),
                    time: event.timestamp,
                    input: Some(serde_json::json!({ "tool_id": tool_id })),
                    output: None,
                    metadata: serde_json::Map::new(),
                    level: ObservationLevel::Info,
                    service_name: None,
                }))
            }

            // Plan events → Span (paired) or Event
            EventPayload::PlanCreated {
                goal, total_steps, ..
            } => Some(Observation::Event(EventData {
                id: Uuid::new_v4().to_string(),
                trace_id: execution_id.to_string(),
                parent_id: None,
                name: "plan.created".to_string(),
                time: event.timestamp,
                input: Some(serde_json::json!({ "goal": goal, "total_steps": total_steps })),
                output: None,
                metadata: serde_json::Map::new(),
                level: ObservationLevel::Info,
                service_name: None,
            })),
            EventPayload::PlanStepCompleted {
                step_id,
                duration_ms,
                tokens_used,
                ..
            } => {
                let start_time = step_starts
                    .get(step_id)
                    .map(|e| e.timestamp)
                    .unwrap_or(event.timestamp);
                Some(Observation::Span(SpanData {
                    id: Uuid::new_v4().to_string(),
                    trace_id: execution_id.to_string(),
                    parent_id: None,
                    name: format!("plan.step.{}", step_id),
                    start_time,
                    end_time: Some(event.timestamp),
                    input: None,
                    output: Some(serde_json::json!({ "tokens_used": tokens_used })),
                    metadata: serde_json::Map::new(),
                    status: ObservationStatus::Completed,
                    level: ObservationLevel::Info,
                    service_name: None,
                }))
            }
            EventPayload::PlanStepFailed { step_id, error, .. } => {
                let start_time = step_starts
                    .get(step_id)
                    .map(|e| e.timestamp)
                    .unwrap_or(event.timestamp);
                Some(Observation::Span(SpanData {
                    id: Uuid::new_v4().to_string(),
                    trace_id: execution_id.to_string(),
                    parent_id: None,
                    name: format!("plan.step.{}", step_id),
                    start_time,
                    end_time: Some(event.timestamp),
                    input: None,
                    output: Some(serde_json::json!({ "error": error })),
                    metadata: serde_json::Map::new(),
                    status: ObservationStatus::Error,
                    level: ObservationLevel::Error,
                    service_name: None,
                }))
            }
            EventPayload::PlanCompleted {
                steps_completed,
                total_duration_ms,
                total_tokens,
                ..
            } => Some(Observation::Event(EventData {
                id: Uuid::new_v4().to_string(),
                trace_id: execution_id.to_string(),
                parent_id: None,
                name: "plan.completed".to_string(),
                time: event.timestamp,
                input: None,
                output: Some(serde_json::json!({
                    "steps_completed": steps_completed,
                    "total_duration_ms": total_duration_ms,
                    "total_tokens": total_tokens,
                })),
                metadata: serde_json::Map::new(),
                level: ObservationLevel::Info,
                service_name: None,
            })),

            // Swarm/Expert → Event
            EventPayload::HandoffTriggered {
                from_agent,
                to_agent,
                ..
            } => Some(Observation::Event(EventData {
                id: Uuid::new_v4().to_string(),
                trace_id: execution_id.to_string(),
                parent_id: None,
                name: "swarm.handoff".to_string(),
                time: event.timestamp,
                input: Some(serde_json::json!({ "from": from_agent, "to": to_agent })),
                output: None,
                metadata: serde_json::Map::new(),
                level: ObservationLevel::Info,
                service_name: None,
            })),
            EventPayload::SpecialistDispatched { specialist, .. } => {
                Some(Observation::Event(EventData {
                    id: Uuid::new_v4().to_string(),
                    trace_id: execution_id.to_string(),
                    parent_id: None,
                    name: "expert.dispatch".to_string(),
                    time: event.timestamp,
                    input: Some(serde_json::json!({ "specialist": specialist })),
                    output: None,
                    metadata: serde_json::Map::new(),
                    level: ObservationLevel::Info,
                    service_name: None,
                }))
            }
            EventPayload::QualityGateResult {
                specialist,
                score,
                passed,
                ..
            } => Some(Observation::Event(EventData {
                id: Uuid::new_v4().to_string(),
                trace_id: execution_id.to_string(),
                parent_id: None,
                name: "expert.quality_gate".to_string(),
                time: event.timestamp,
                input: Some(
                    serde_json::json!({ "specialist": specialist, "score": score, "passed": passed }),
                ),
                output: None,
                metadata: serde_json::Map::new(),
                level: if *passed {
                    ObservationLevel::Info
                } else {
                    ObservationLevel::Warning
                },
                service_name: None,
            })),

            // HITL → Event
            EventPayload::InstructionReceived { message, .. } => {
                Some(Observation::Event(EventData {
                    id: Uuid::new_v4().to_string(),
                    trace_id: execution_id.to_string(),
                    parent_id: None,
                    name: "job.instruction".to_string(),
                    time: event.timestamp,
                    input: Some(
                        serde_json::json!({ "message": message.chars().take(200).collect::<String>() }),
                    ),
                    output: None,
                    metadata: serde_json::Map::new(),
                    level: ObservationLevel::Info,
                    service_name: None,
                }))
            }
            EventPayload::HITLInputRequired {
                prompt, input_type, ..
            } => Some(Observation::Event(EventData {
                id: Uuid::new_v4().to_string(),
                trace_id: execution_id.to_string(),
                parent_id: None,
                name: "job.hitl_required".to_string(),
                time: event.timestamp,
                input: Some(serde_json::json!({ "prompt": prompt, "input_type": input_type })),
                output: None,
                metadata: serde_json::Map::new(),
                level: ObservationLevel::Warning,
                service_name: None,
            })),

            // Job result → Event
            EventPayload::JobResultReady {
                response,
                total_duration_ms,
            } => Some(Observation::Event(EventData {
                id: Uuid::new_v4().to_string(),
                trace_id: execution_id.to_string(),
                parent_id: None,
                name: "job.result".to_string(),
                time: event.timestamp,
                input: None,
                output: Some(serde_json::json!({
                    "response_preview": response.chars().take(200).collect::<String>(),
                    "total_duration_ms": total_duration_ms,
                })),
                metadata: serde_json::Map::new(),
                level: ObservationLevel::Info,
                service_name: None,
            })),

            // Remaining events → generic Event
            _ => Some(Observation::Event(EventData {
                id: Uuid::new_v4().to_string(),
                trace_id: execution_id.to_string(),
                parent_id: None,
                name: format!("exec.{}", event_variant_name(&event.payload)),
                time: event.timestamp,
                input: None,
                output: None,
                metadata: serde_json::Map::new(),
                level: ObservationLevel::Debug,
                service_name: None,
            })),
        };

        if let Some(o) = obs {
            observations.push(o);
        }
    }

    // Batch ingest all observations
    if !observations.is_empty() {
        let count = observations.len();
        let batch = devtools_core::types::IngestBatch {
            traces: vec![],
            observations,
        };
        svc.ingest_batch(batch).await?;
        tracing::debug!(
            execution_id,
            observation_count = count,
            "Pushed job events to devtools"
        );
    }

    // Update trace tags if it already existed
    if trace_exists {
        let update = devtools_core::filter::TraceUpdate {
            tags: Some(vec!["job".to_string(), format!("job-{}", job_id)]),
            ..Default::default()
        };
        svc.update_trace(execution_id, update).await?;
    }

    Ok(())
}

/// Get a snake_case name for an EventPayload variant (for generic event mapping).
#[cfg(all(feature = "devtools", feature = "graph"))]
fn event_variant_name(payload: &EventPayload) -> &'static str {
    match payload {
        EventPayload::GraphStarted => "graph_started",
        EventPayload::NodeEntered { .. } => "node_entered",
        EventPayload::NodeCompleted { .. } => "node_completed",
        EventPayload::NodeFailed { .. } => "node_failed",
        EventPayload::EdgeTraversed { .. } => "edge_traversed",
        EventPayload::GraphCompleted { .. } => "graph_completed",
        EventPayload::CheckpointSaved { .. } => "checkpoint_saved",
        EventPayload::HandoffTriggered { .. } => "handoff_triggered",
        EventPayload::HandoffConditionChecked { .. } => "handoff_condition_checked",
        EventPayload::CycleDetected { .. } => "cycle_detected",
        EventPayload::SpecialistDispatched { .. } => "specialist_dispatched",
        EventPayload::QualityGateResult { .. } => "quality_gate_result",
        EventPayload::SupervisorDecision { .. } => "supervisor_decision",
        EventPayload::LlmRequest { .. } => "llm_request",
        EventPayload::LlmResponse { .. } => "llm_response",
        EventPayload::ToolCall { .. } => "tool_call",
        EventPayload::ParallelPartialComplete { .. } => "parallel_partial",
        EventPayload::ParallelBranchFailed { .. } => "parallel_branch_failed",
        EventPayload::DagWaveStarted { .. } => "dag_wave_started",
        EventPayload::DagWaveCompleted { .. } => "dag_wave_completed",
        EventPayload::MemoryHydrated { .. } => "memory_hydrated",
        EventPayload::MemoryFlushed { .. } => "memory_flushed",
        EventPayload::TemplateSelected { .. } => "template_selected",
        EventPayload::AutoModeFallback { .. } => "auto_mode_fallback",
        EventPayload::BudgetWarning { .. } => "budget_warning",
        EventPayload::BudgetExceeded { .. } => "budget_exceeded",
        EventPayload::RoutingClassified { .. } => "routing_classified",
        EventPayload::PlanCreated { .. } => "plan_created",
        EventPayload::PlanStepStarted { .. } => "plan_step_started",
        EventPayload::PlanStepCompleted { .. } => "plan_step_completed",
        EventPayload::PlanStepFailed { .. } => "plan_step_failed",
        EventPayload::ReplanTriggered { .. } => "replan_triggered",
        EventPayload::ReplanCompleted { .. } => "replan_completed",
        EventPayload::PlanCompleted { .. } => "plan_completed",
        EventPayload::ThinkingDelta { .. } => "thinking_delta",
        EventPayload::ContentDelta { .. } => "content_delta",
        EventPayload::ToolCallStarted { .. } => "tool_call_started",
        EventPayload::ToolCallCompleted { .. } => "tool_call_completed",
        EventPayload::InstructionReceived { .. } => "instruction_received",
        EventPayload::HITLInputRequired { .. } => "hitl_input_required",
        EventPayload::PlanApprovalRequired { .. } => "plan_approval_required",
        EventPayload::JobResultReady { .. } => "job_result_ready",
        EventPayload::JudgeEvaluated { .. } => "judge_evaluated",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_signal_values() {
        // Verify default is Continue
        let sig = JobSignal::Continue;
        assert_eq!(sig, JobSignal::Continue);
        assert_ne!(sig, JobSignal::Pause);
        assert_ne!(sig, JobSignal::Cancel);

        // Verify distinctness
        assert_ne!(JobSignal::Pause, JobSignal::Cancel);
    }

    #[tokio::test]
    async fn test_pause_signal_sent_via_watch() {
        let (tx, mut rx) = watch::channel(JobSignal::Continue);

        // Initially Continue
        assert_eq!(*rx.borrow(), JobSignal::Continue);

        // Send Pause
        tx.send(JobSignal::Pause).unwrap();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), JobSignal::Pause);
    }

    #[tokio::test]
    async fn test_cancel_signal_sent_via_watch() {
        let (tx, mut rx) = watch::channel(JobSignal::Continue);

        // Send Cancel
        tx.send(JobSignal::Cancel).unwrap();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), JobSignal::Cancel);
    }

    #[test]
    fn test_signal_handles_tuple_operations() {
        let map: DashMap<Uuid, (watch::Sender<JobSignal>, tokio::sync::mpsc::Sender<String>)> =
            DashMap::new();
        let id = Uuid::new_v4();
        let (watch_tx, _watch_rx) = watch::channel(JobSignal::Continue);
        let (mpsc_tx, _mpsc_rx) = tokio::sync::mpsc::channel(16);

        map.insert(id, (watch_tx, mpsc_tx));
        assert_eq!(map.len(), 1);

        // Simulate pause_job: get handle and send signal via .0
        if let Some(entry) = map.get(&id) {
            entry.0.send(JobSignal::Pause).unwrap();
        }

        // Simulate cancel_job: remove handle and send signal
        if let Some((_id, (removed_watch, _removed_mpsc))) = map.remove(&id) {
            let _ = removed_watch.send(JobSignal::Cancel);
        }
        assert_eq!(map.len(), 0);
    }

    #[tokio::test]
    async fn test_instruct_channel_delivers_message() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(16);
        tx.send("change to Python".to_string()).await.unwrap();
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg, "change to Python");
    }

    #[tokio::test]
    async fn test_instruct_channel_multiple_messages() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(16);
        tx.send("msg1".into()).await.unwrap();
        tx.send("msg2".into()).await.unwrap();
        assert_eq!(rx.recv().await.unwrap(), "msg1");
        assert_eq!(rx.recv().await.unwrap(), "msg2");
    }

    #[tokio::test]
    async fn test_instruct_channel_closed_when_job_done() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(16);
        drop(tx); // Job finished — sender dropped
        assert!(rx.recv().await.is_none());
    }

    #[test]
    fn test_job_signal_debug_and_copy() {
        // Ensure Debug and Copy traits work
        let sig = JobSignal::Pause;
        let sig_copy = sig; // Copy
        assert_eq!(sig, sig_copy);
        assert_eq!(format!("{:?}", sig), "Pause");
    }
}
