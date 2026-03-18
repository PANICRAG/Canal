//! Async job API routes.
//!
//! Provides REST endpoints for submitting, listing, streaming, and
//! cancelling long-running background jobs.

use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::StreamExt;
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::auth::AuthContext;
use crate::state::AppState;

use gateway_core::jobs::{
    hitl::HITLResponse, JobListResponse, JobStatus, JobType, SubmitJobRequest, SubmitJobResponse,
};

/// Register job routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", post(submit_job))
        .route("/", get(list_jobs))
        .route("/active", get(list_active_jobs))
        .route("/{id}", get(get_job))
        .route("/{id}/stream", get(stream_job))
        .route("/{id}/pending-approval", get(get_pending_approval))
        .route("/{id}/cancel", post(cancel_job))
        .route("/{id}/pause", post(pause_job))
        .route("/{id}/resume", post(resume_job))
        .route("/{id}/instruct", post(instruct_job))
        .route("/{id}/input", post(submit_hitl_input))
        .route("/{id}/step-result", post(submit_step_result))
}

/// Submit a new async job.
///
/// `POST /api/jobs`
async fn submit_job(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(request): Json<SubmitJobRequest>,
) -> Result<Json<SubmitJobResponse>, ApiError> {
    let user_id = auth.user_id;

    // Determine job type from collaboration mode
    let job_type = match request.collaboration_mode.as_deref() {
        Some("direct") | None => JobType::Chat,
        _ => JobType::Collaboration,
    };

    let input = gateway_core::jobs::JobInput {
        message: request.message.clone(),
        collaboration_mode: request.collaboration_mode.clone(),
        model: request.model.clone(),
        budget_tokens: request.budget_tokens,
        client_capabilities: None,
    };

    let metadata = request.metadata.clone().unwrap_or(serde_json::json!({}));

    let job = state
        .job_store
        .create_job(
            user_id,
            job_type,
            &input,
            metadata,
            request.notify_webhook.clone(),
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let stream_url = format!("/api/jobs/{}/stream", job.id);

    tracing::info!(
        job_id = %job.id,
        user_id = %user_id,
        job_type = ?job_type,
        "Job submitted"
    );

    Ok(Json(SubmitJobResponse {
        job_id: job.id,
        status: job.status,
        stream_url,
    }))
}

/// Query parameters for job listing.
#[derive(Debug, Deserialize)]
struct ListJobsQuery {
    status: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// List jobs for the authenticated user.
///
/// `GET /api/jobs?status=running&limit=20&offset=0`
async fn list_jobs(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Query(query): Query<ListJobsQuery>,
) -> Result<Json<JobListResponse>, ApiError> {
    let user_id = auth.user_id;
    let limit = query.limit.unwrap_or(20).min(100);
    let offset = query.offset.unwrap_or(0);

    let status_filter = query.status.as_deref().and_then(parse_status);

    let (jobs, total) = state
        .job_store
        .list_jobs(user_id, status_filter, limit, offset)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(JobListResponse { jobs, total }))
}

/// Get a single job by ID.
///
/// `GET /api/jobs/:id`
async fn get_job(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let job = state
        .job_store
        .get_job(id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("Job {} not found", id)))?;

    if job.user_id != auth.user_id {
        return Err(ApiError::not_found(format!("Job {} not found", id)));
    }

    Ok(Json(serde_json::to_value(&job).unwrap_or_default()))
}

/// SSE stream for a job's execution events.
///
/// `GET /api/jobs/:id/stream`
///
/// Supports reconnection via `Last-Event-ID` header for replay.
#[cfg(feature = "jobs")]
async fn stream_job(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    // Look up the job, waiting for the scheduler to assign an execution_id.
    // The client typically calls /stream immediately after POST /api/jobs,
    // before the scheduler has picked up the queued job.  Poll briefly
    // instead of returning 404 right away.
    let (job, execution_id) = {
        let mut attempts = 0u32;
        loop {
            let job = state
                .job_store
                .get_job(id)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
                .ok_or_else(|| ApiError::not_found(format!("Job {} not found", id)))?;

            if job.user_id != auth.user_id {
                return Err(ApiError::not_found(format!("Job {} not found", id)));
            }

            if let Some(eid) = job.execution_id.clone() {
                break (job, eid);
            }

            attempts += 1;
            if attempts >= 50 {
                // ~10 seconds elapsed — give up
                return Err(ApiError::not_found(
                    "Job has no execution yet after waiting 10s".to_string(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    };

    // Parse Last-Event-ID for reconnection replay.
    // On fresh connect (no header), replay ALL events so the client gets
    // the full history including content_delta events that already fired.
    let replay_count = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .map(|last_id| last_id + 1) // Replay events after the last seen ID
        .unwrap_or(usize::MAX); // Default: replay ALL events

    // Subscribe to the execution store (reuses existing infrastructure)
    let (rx, replay_events) = state.execution_store.subscribe(&execution_id, replay_count);

    // Send initial job status event
    let status_event = serde_json::json!({
        "job_id": id.to_string(),
        "status": job.status,
        "progress_pct": job.progress_pct,
    });

    let initial = futures::stream::once(async move {
        Ok(Event::default()
            .event("job_status_changed")
            .data(status_event.to_string()))
    });

    // Capture before consuming for the terminal-job fallback check and live ID offset
    let replay_is_empty = replay_events.is_empty();
    let replay_len = replay_events.len();

    // Replay historical events with typed SSE event names
    let replay_stream =
        futures::stream::iter(replay_events.into_iter().enumerate()).map(|(i, event)| {
            let (event_type, data) = event_payload_to_sse(&event.payload);
            Ok(Event::default()
                .event(event_type)
                .id(i.to_string())
                .data(data.to_string()))
        });

    let keep_alive = KeepAlive::new()
        .interval(std::time::Duration::from_secs(15))
        .text("ping");

    // For already-terminal jobs, send replay + result + done.
    // For running jobs, chain the live stream from the subscriber channel.
    let job_is_terminal = matches!(
        job.status,
        gateway_core::jobs::JobStatus::Completed
            | gateway_core::jobs::JobStatus::Failed
            | gateway_core::jobs::JobStatus::Cancelled
    );

    // Live event IDs continue from where replay left off
    let start_id = replay_len;
    let tail: std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<Event, std::convert::Infallible>> + Send>,
    > = if job_is_terminal {
        // Terminal job: if replay events are empty (e.g. after server restart
        // and the in-memory ExecutionStore was lost), synthesise a result
        // event from the DB-stored result so the client still gets the
        // response text.
        let mut terminal_events: Vec<Result<Event, std::convert::Infallible>> = Vec::new();

        if replay_is_empty {
            // No events in ExecutionStore — fall back to DB result
            if let Some(ref result) = job.result {
                if !result.response.is_empty() {
                    terminal_events.push(Ok(Event::default().event("job_result").data(
                        serde_json::json!({
                            "response": result.response,
                            "total_tokens": result.total_tokens,
                            "total_duration_ms": result.total_duration_ms,
                            "collaboration_mode": result.collaboration_mode_used,
                        })
                        .to_string(),
                    )));
                }
            }
        }

        terminal_events.push(Ok(Event::default().event("done").data("{}")));
        Box::pin(futures::stream::iter(terminal_events))
    } else {
        // Running: live stream from subscriber channel, followed by done when it closes
        let live_stream = ReceiverStream::new(rx).enumerate().map(move |(i, event)| {
            let (event_type, data) = event_payload_to_sse(&event.payload);
            Ok(Event::default()
                .event(event_type)
                .id((start_id + i).to_string())
                .data(data.to_string()))
        });
        let done_event =
            futures::stream::once(async { Ok(Event::default().event("done").data("{}")) });
        Box::pin(live_stream.chain(done_event))
    };

    Ok(Sse::new(initial.chain(replay_stream).chain(tail)).keep_alive(keep_alive))
}

/// Fallback stream_job when graph feature is not enabled.
#[cfg(not(feature = "jobs"))]
async fn stream_job(
    State(_state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path(_id): Path<Uuid>,
    _headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::InternalError(
        "SSE streaming requires graph feature".to_string(),
    ))
}

/// Check for a pending plan approval on a job.
///
/// `GET /api/jobs/{id}/pending-approval`
///
/// Returns the plan approval data if the job's execution contains a
/// `PlanApprovalRequired` event.  The client polls this endpoint to
/// recover plan approvals that the SSE stream may not have delivered
/// in real-time.
#[cfg(feature = "jobs")]
async fn get_pending_approval(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let job = state
        .job_store
        .get_job(id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("Job {} not found", id)))?;

    let execution_id = match job.execution_id {
        Some(eid) => eid,
        None => {
            return Ok(Json(serde_json::json!({ "has_pending_approval": false })));
        }
    };

    let events = state.execution_store.get_events(&execution_id, 0, None);

    // Find the most recent PlanApprovalRequired event
    let approval_event = events.iter().rev().find(|e| {
        matches!(
            e.payload,
            gateway_core::graph::EventPayload::PlanApprovalRequired { .. }
        )
    });

    match approval_event {
        Some(event) => {
            let (_, data) = event_payload_to_sse(&event.payload);
            Ok(Json(serde_json::json!({
                "has_pending_approval": true,
                "approval_data": data,
            })))
        }
        None => Ok(Json(serde_json::json!({ "has_pending_approval": false }))),
    }
}

/// Fallback when graph feature is not enabled.
#[cfg(not(feature = "jobs"))]
async fn get_pending_approval(
    State(_state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path(_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({ "has_pending_approval": false })))
}

/// List currently active (running) jobs.
///
/// `GET /api/jobs/active`
async fn list_active_jobs(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let active = state
        .job_store
        .list_active()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let summaries: Vec<_> = active.iter().map(|j| j.to_summary()).collect();
    Ok(Json(serde_json::json!({
        "jobs": summaries,
        "total": summaries.len(),
    })))
}

/// Cancel a running or queued job.
///
/// `POST /api/jobs/:id/cancel`
async fn cancel_job(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Signal the scheduler to cancel
    state
        .job_scheduler
        .cancel_job(id)
        .await
        .map_err(|e| match &e {
            gateway_core::jobs::JobError::NotFound(_) => ApiError::not_found(e.to_string()),
            gateway_core::jobs::JobError::AlreadyCancelled(_) => ApiError::not_found(e.to_string()),
            gateway_core::jobs::JobError::InvalidTransition { .. } => {
                ApiError::bad_request(e.to_string())
            }
            _ => ApiError::internal(e.to_string()),
        })?;

    tracing::info!(job_id = %id, "Job cancelled");

    Ok(Json(serde_json::json!({
        "job_id": id.to_string(),
        "status": "cancelled",
    })))
}

/// Pause a running job at the next safe point.
///
/// Sends a `JobSignal::Pause` to the running task so it can save a
/// checkpoint before stopping. If the job is not actively running
/// (e.g. still queued), the status is updated directly in the DB.
///
/// `POST /api/jobs/:id/pause`
async fn pause_job(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .job_scheduler
        .pause_job(id)
        .await
        .map_err(|e| match &e {
            gateway_core::jobs::JobError::NotFound(_) => ApiError::not_found(e.to_string()),
            gateway_core::jobs::JobError::InvalidTransition { .. } => {
                ApiError::bad_request(e.to_string())
            }
            _ => ApiError::internal(e.to_string()),
        })?;

    tracing::info!(job_id = %id, "Job pause requested");

    Ok(Json(serde_json::json!({
        "job_id": id.to_string(),
        "status": "pausing",
    })))
}

/// Resume a paused job.
///
/// Reads the job's checkpoint (if any) and re-queues it. The scheduler
/// will detect the checkpoint on the next execution and resume from that
/// point instead of starting over.
///
/// `POST /api/jobs/:id/resume`
async fn resume_job(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let checkpoint_id = state
        .job_scheduler
        .resume_job(id)
        .await
        .map_err(|e| match &e {
            gateway_core::jobs::JobError::NotFound(_) => ApiError::not_found(e.to_string()),
            gateway_core::jobs::JobError::InvalidTransition { .. } => {
                ApiError::bad_request(e.to_string())
            }
            _ => ApiError::internal(e.to_string()),
        })?;

    let has_checkpoint = checkpoint_id.is_some();

    tracing::info!(
        job_id = %id,
        has_checkpoint = has_checkpoint,
        "Job resumed (re-queued)"
    );

    Ok(Json(serde_json::json!({
        "job_id": id.to_string(),
        "status": "queued",
        "resumed_from_checkpoint": has_checkpoint,
        "checkpoint_id": checkpoint_id,
    })))
}

/// Send an instruction to a running job.
///
/// `POST /api/jobs/:id/instruct`
async fn instruct_job(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(request): Json<InstructRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .job_scheduler
        .instruct_job(id, request.message.clone())
        .await
        .map_err(|e| match &e {
            gateway_core::jobs::JobError::NotFound(_) => ApiError::not_found(e.to_string()),
            gateway_core::jobs::JobError::InvalidTransition { .. } => {
                ApiError::bad_request(e.to_string())
            }
            _ => ApiError::internal(e.to_string()),
        })?;

    tracing::info!(job_id = %id, "Job instruction sent");

    Ok(Json(serde_json::json!({
        "job_id": id.to_string(),
        "status": "instruction_sent",
    })))
}

#[derive(Debug, Deserialize)]
struct InstructRequest {
    message: String,
}

/// Submit a user response to a HITL input request.
///
/// `POST /api/jobs/:id/input`
async fn submit_hitl_input(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path(_id): Path<Uuid>,
    Json(request): Json<HITLInputRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let request_id = request
        .request_id
        .parse::<Uuid>()
        .map_err(|_| ApiError::bad_request("Invalid request_id UUID".to_string()))?;

    state
        .pending_hitl_inputs
        .complete(
            &request_id,
            HITLResponse {
                value: request.value.clone(),
                metadata: request.metadata.clone(),
            },
        )
        .map_err(|e| ApiError::not_found(e))?;

    tracing::info!(request_id = %request_id, "HITL input submitted");

    Ok(Json(serde_json::json!({
        "request_id": request_id.to_string(),
        "status": "input_received",
    })))
}

#[derive(Debug, Deserialize)]
struct HITLInputRequest {
    request_id: String,
    value: String,
    metadata: Option<serde_json::Value>,
}

/// Submit a step execution result from the client (A43 Step Delegation).
///
/// `POST /api/jobs/{id}/step-result`
#[cfg(feature = "collaboration")]
async fn submit_step_result(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path(_id): Path<Uuid>,
    Json(request): Json<StepResultRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let request_id = request
        .request_id
        .parse::<Uuid>()
        .map_err(|_| ApiError::bad_request("Invalid request_id UUID".to_string()))?;

    let result = gateway_core::agent::step_delegate::StepExecuteResult {
        request_id,
        success: request.success,
        output: request.output.unwrap_or(serde_json::json!({})),
        error: request.error,
        execution_time_ms: request.execution_time_ms.unwrap_or(0),
    };

    state
        .pending_step_executions
        .complete(&request_id, result)
        .map_err(|e| ApiError::not_found(e.to_string()))?;

    tracing::info!(request_id = %request_id, "Step execution result submitted");

    Ok(Json(serde_json::json!({
        "request_id": request_id.to_string(),
        "status": "result_received",
    })))
}

#[cfg(not(feature = "collaboration"))]
async fn submit_step_result(Path(_id): Path<Uuid>) -> Result<Json<serde_json::Value>, ApiError> {
    Err(ApiError::not_found(
        "Step delegation not enabled".to_string(),
    ))
}

#[derive(Debug, Deserialize)]
struct StepResultRequest {
    request_id: String,
    success: bool,
    output: Option<serde_json::Value>,
    error: Option<String>,
    execution_time_ms: Option<u64>,
}

/// Map an EventPayload to an SSE event type name and data JSON.
///
/// The event type names match the `backendEventMap` in SseParser.swift
/// so the frontend can decode them correctly.
#[cfg(feature = "jobs")]
fn event_payload_to_sse(
    payload: &gateway_core::graph::EventPayload,
) -> (String, serde_json::Value) {
    let (event_type, mut data) = event_payload_to_sse_inner(payload);

    // A40: Add server timestamp to every event for accurate timeline
    if let serde_json::Value::Object(ref mut map) = data {
        map.insert(
            "server_ts".into(),
            serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
        );
    }

    (event_type, data)
}

/// Inner mapping from EventPayload to (event_type, data).
#[cfg(feature = "jobs")]
fn event_payload_to_sse_inner(
    payload: &gateway_core::graph::EventPayload,
) -> (String, serde_json::Value) {
    use gateway_core::graph::EventPayload;

    match payload {
        // Content streaming events → frontend SseParser format
        EventPayload::ThinkingDelta { content, .. } => (
            "thinking_delta".into(),
            serde_json::json!({ "delta": content }),
        ),
        EventPayload::ContentDelta { content, .. } => (
            "content_delta".into(),
            serde_json::json!({ "delta": content }),
        ),
        EventPayload::ToolCallStarted {
            tool_id, tool_name, ..
        } => (
            "tool_call_start".into(),
            serde_json::json!({ "tool_use_id": tool_id, "tool_name": tool_name }),
        ),
        EventPayload::ToolCallCompleted { tool_id, .. } => (
            "tool_call_complete".into(),
            serde_json::json!({ "tool_use_id": tool_id, "tool_name": "" }),
        ),

        // Graph events
        EventPayload::GraphStarted => (
            "graph_started".into(),
            serde_json::json!({ "execution_id": "", "graph_name": "", "node_count": 0, "edge_count": 0 }),
        ),
        EventPayload::NodeEntered { node_id } => (
            "graph_node_entered".into(),
            serde_json::json!({ "execution_id": "", "node_id": node_id }),
        ),
        EventPayload::NodeCompleted {
            node_id,
            duration_ms,
        } => (
            "graph_node_completed".into(),
            serde_json::json!({ "execution_id": "", "node_id": node_id, "duration_ms": duration_ms }),
        ),
        EventPayload::NodeFailed { node_id, error } => (
            "graph_node_completed".into(),
            serde_json::json!({ "execution_id": "", "node_id": node_id, "status": "failed", "error": error }),
        ),
        EventPayload::EdgeTraversed { from, to, label } => (
            "graph_edge_traversed".into(),
            serde_json::json!({ "execution_id": "", "from_node": from, "to_node": to, "label": label }),
        ),
        EventPayload::GraphCompleted { total_duration_ms } => (
            "graph_completed".into(),
            serde_json::json!({ "execution_id": "", "duration_ms": total_duration_ms }),
        ),

        // Plan events
        EventPayload::PlanCreated {
            goal,
            total_steps,
            steps_preview,
            ..
        } => (
            "plan_created".into(),
            serde_json::json!({
                "plan_id": "",
                "title": goal,
                "steps": steps_preview.iter().enumerate().map(|(i, s)| {
                    serde_json::json!({"id": i.to_string(), "title": s})
                }).collect::<Vec<_>>(),
            }),
        ),
        EventPayload::PlanStepStarted {
            step_id, action, ..
        } => (
            "plan_step_started".into(),
            serde_json::json!({ "plan_id": "", "step_id": step_id.to_string(), "title": action }),
        ),
        EventPayload::PlanStepCompleted {
            step_id,
            duration_ms,
            ..
        } => (
            "plan_step_completed".into(),
            serde_json::json!({ "plan_id": "", "step_id": step_id.to_string(), "duration_ms": duration_ms }),
        ),
        EventPayload::PlanStepFailed {
            step_id,
            error,
            duration_ms,
        } => (
            "plan_step_failed".into(),
            serde_json::json!({ "plan_id": "", "step_id": step_id.to_string(), "error": error, "duration_ms": duration_ms }),
        ),
        EventPayload::PlanCompleted {
            steps_completed,
            steps_failed,
            steps_skipped,
            ..
        } => (
            "plan_completed".into(),
            serde_json::json!({
                "plan_id": "",
                "status": "completed",
                "summary": format!("{} completed, {} failed, {} skipped", steps_completed, steps_failed, steps_skipped),
            }),
        ),
        EventPayload::ReplanTriggered { reason, .. } => (
            "replan_started".into(),
            serde_json::json!({ "plan_id": "", "reason": reason }),
        ),
        EventPayload::ReplanCompleted {
            new_steps_count, ..
        } => (
            "replan_completed".into(),
            serde_json::json!({ "plan_id": "", "new_plan_id": "" }),
        ),

        // Routing
        EventPayload::RoutingClassified {
            source,
            category,
            routed_to,
            confidence,
            reasoning,
            ..
        } => (
            "routing_decision".into(),
            serde_json::json!({
                "source": source,
                "category": category,
                "mode": routed_to,
                "confidence": confidence,
                "reasoning": reasoning,
            }),
        ),

        // HITL events
        EventPayload::HITLInputRequired {
            request_id,
            job_id,
            prompt,
            input_type,
            options,
            timeout_seconds,
            context,
        } => (
            "hitl_input_required".into(),
            serde_json::json!({
                "execution_id": "",
                "request_id": request_id,
                "job_id": job_id,
                "prompt": prompt,
                "input_type": input_type,
                "options": options,
                "timeout_seconds": timeout_seconds,
                "context": context,
            }),
        ),
        EventPayload::InstructionReceived { job_id, message } => (
            "instruction_received".into(),
            serde_json::json!({ "job_id": job_id, "message": message }),
        ),

        // Job lifecycle
        EventPayload::JobResultReady {
            response,
            total_duration_ms,
        } => (
            "job_result".into(),
            serde_json::json!({
                "response": response,
                "total_duration_ms": total_duration_ms,
            }),
        ),

        // A40: Judge evaluation events
        EventPayload::JudgeEvaluated {
            step_id,
            verdict,
            reasoning,
            suggestions,
            retry_count,
        } => (
            "judge_evaluated".into(),
            serde_json::json!({
                "execution_id": "",
                "step_id": step_id,
                "verdict": verdict,
                "reasoning": reasoning,
                "suggestions": suggestions,
                "retry_count": retry_count,
            }),
        ),

        // Plan approval — forward to frontend for human-in-the-loop
        EventPayload::PlanApprovalRequired {
            execution_id,
            request_id,
            goal,
            steps,
            success_criteria,
            timeout_seconds,
            risk_level,
            revision_round,
            max_revisions,
        } => (
            "plan_approval_required".into(),
            serde_json::json!({
                "execution_id": execution_id,
                "request_id": request_id,
                "goal": goal,
                "steps": steps,
                "success_criteria": success_criteria,
                "risk_level": risk_level,
                "timeout_seconds": timeout_seconds,
                "revision_round": revision_round,
                "max_revisions": max_revisions,
            }),
        ),

        // Default: serialize full payload
        other => {
            let data = serde_json::to_value(other).unwrap_or_default();
            ("execution_event".into(), data)
        }
    }
}

/// Parse a status string into a JobStatus.
fn parse_status(s: &str) -> Option<JobStatus> {
    match s {
        "submitted" => Some(JobStatus::Submitted),
        "queued" => Some(JobStatus::Queued),
        "running" => Some(JobStatus::Running),
        "paused" => Some(JobStatus::Paused),
        "completed" => Some(JobStatus::Completed),
        "failed" => Some(JobStatus::Failed),
        "cancelled" => Some(JobStatus::Cancelled),
        _ => None,
    }
}

#[cfg(all(test, feature = "jobs"))]
mod tests {
    use super::event_payload_to_sse;
    use gateway_core::graph::EventPayload;

    #[test]
    fn test_server_ts_present_in_all_events() {
        // A40: Every SSE event must include a server_ts field
        let payloads = vec![
            EventPayload::ContentDelta {
                content: "hi".into(),
                node_id: "n".into(),
            },
            EventPayload::GraphStarted,
            EventPayload::JobResultReady {
                response: "done".into(),
                total_duration_ms: 100,
            },
        ];
        for payload in &payloads {
            let (_event_type, data) = event_payload_to_sse(payload);
            assert!(
                data.get("server_ts").is_some(),
                "Missing server_ts for {:?}",
                payload
            );
            // Verify it's an RFC3339 timestamp string
            let ts = data["server_ts"].as_str().unwrap();
            assert!(ts.contains("T"), "server_ts should be ISO 8601: {}", ts);
        }
    }

    #[test]
    fn test_content_delta_to_sse() {
        let (event_type, data) = event_payload_to_sse(&EventPayload::ContentDelta {
            content: "hello".into(),
            node_id: "n1".into(),
        });
        assert_eq!(event_type, "content_delta");
        assert_eq!(data["delta"], "hello");
    }

    #[test]
    fn test_thinking_delta_to_sse() {
        let (event_type, data) = event_payload_to_sse(&EventPayload::ThinkingDelta {
            content: "analyzing...".into(),
            node_id: "n1".into(),
        });
        assert_eq!(event_type, "thinking_delta");
        assert_eq!(data["delta"], "analyzing...");
    }

    #[test]
    fn test_job_result_ready_to_sse() {
        let (event_type, data) = event_payload_to_sse(&EventPayload::JobResultReady {
            response: "The answer is 42.".into(),
            total_duration_ms: 1500,
        });
        assert_eq!(event_type, "job_result");
        assert_eq!(data["response"], "The answer is 42.");
        assert_eq!(data["total_duration_ms"], 1500);
    }

    #[test]
    fn test_hitl_input_required_to_sse() {
        let (event_type, data) = event_payload_to_sse(&EventPayload::HITLInputRequired {
            request_id: "req-1".into(),
            job_id: "job-1".into(),
            prompt: "Choose an option".into(),
            input_type: "choice".into(),
            options: Some(vec!["A".into(), "B".into()]),
            timeout_seconds: Some(60),
            context: Some("step 3".into()),
        });
        assert_eq!(event_type, "hitl_input_required");
        assert_eq!(data["request_id"], "req-1");
        assert_eq!(data["job_id"], "job-1");
        assert_eq!(data["prompt"], "Choose an option");
        assert_eq!(data["input_type"], "choice");
        assert_eq!(data["timeout_seconds"], 60);
        assert_eq!(data["context"], "step 3");
        let options = data["options"].as_array().unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0], "A");
    }

    #[test]
    fn test_graph_lifecycle_to_sse() {
        // GraphStarted
        let (event_type, _) = event_payload_to_sse(&EventPayload::GraphStarted);
        assert_eq!(event_type, "graph_started");

        // NodeEntered
        let (event_type, data) = event_payload_to_sse(&EventPayload::NodeEntered {
            node_id: "agent_1".into(),
        });
        assert_eq!(event_type, "graph_node_entered");
        assert_eq!(data["node_id"], "agent_1");

        // NodeCompleted
        let (event_type, data) = event_payload_to_sse(&EventPayload::NodeCompleted {
            node_id: "agent_1".into(),
            duration_ms: 350,
        });
        assert_eq!(event_type, "graph_node_completed");
        assert_eq!(data["node_id"], "agent_1");
        assert_eq!(data["duration_ms"], 350);

        // GraphCompleted
        let (event_type, data) = event_payload_to_sse(&EventPayload::GraphCompleted {
            total_duration_ms: 2000,
        });
        assert_eq!(event_type, "graph_completed");
        assert_eq!(data["duration_ms"], 2000);
    }

    #[test]
    fn test_judge_evaluated_to_sse() {
        let (event_type, data) = event_payload_to_sse(&EventPayload::JudgeEvaluated {
            step_id: Some("1".into()),
            verdict: "pass".into(),
            reasoning: "Output matches expected format".into(),
            suggestions: vec!["Consider adding error handling".into()],
            retry_count: 0,
        });
        assert_eq!(event_type, "judge_evaluated");
        assert_eq!(data["step_id"], "1");
        assert_eq!(data["verdict"], "pass");
        assert_eq!(data["reasoning"], "Output matches expected format");
        assert_eq!(data["suggestions"][0], "Consider adding error handling");
        assert_eq!(data["retry_count"], 0);
    }

    #[test]
    fn test_judge_evaluated_final_no_step_id() {
        let (event_type, data) = event_payload_to_sse(&EventPayload::JudgeEvaluated {
            step_id: None,
            verdict: "partial_pass".into(),
            reasoning: "Overall quality acceptable".into(),
            suggestions: vec![],
            retry_count: 1,
        });
        assert_eq!(event_type, "judge_evaluated");
        assert!(data["step_id"].is_null());
        assert_eq!(data["verdict"], "partial_pass");
        assert_eq!(data["retry_count"], 1);
    }

    #[test]
    fn test_instruction_received_to_sse() {
        let (event_type, data) = event_payload_to_sse(&EventPayload::InstructionReceived {
            job_id: "job-abc".into(),
            message: "focus on security".into(),
        });
        assert_eq!(event_type, "instruction_received");
        assert_eq!(data["job_id"], "job-abc");
        assert_eq!(data["message"], "focus on security");
    }
}
