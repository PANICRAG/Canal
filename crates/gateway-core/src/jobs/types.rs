//! Core job data types for the async job system.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Status of a job in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "job_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Submitted,
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Submitted => write!(f, "submitted"),
            Self::Queued => write!(f, "queued"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Type of job execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "job_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum JobType {
    Chat,
    Collaboration,
    Workflow,
}

/// A persistent async job record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub user_id: Uuid,
    pub session_id: Option<Uuid>,
    pub job_type: JobType,
    pub status: JobStatus,
    pub input: JobInput,
    pub result: Option<JobResult>,
    pub error: Option<String>,
    pub checkpoint_id: Option<String>,
    pub execution_id: Option<String>,
    pub progress_pct: Option<f32>,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    pub notify_webhook: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Input parameters for a job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInput {
    /// The user message / task description.
    pub message: String,
    /// Collaboration mode override (e.g. "auto", "direct", "swarm", "expert", "plan_execute").
    pub collaboration_mode: Option<String>,
    /// Model override.
    pub model: Option<String>,
    /// Maximum token budget for the job.
    pub budget_tokens: Option<u32>,
    /// Client capabilities for RTE delegation.
    pub client_capabilities: Option<serde_json::Value>,
}

/// Result of a completed job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    /// The final response text.
    pub response: String,
    /// Messages exchanged during execution.
    pub messages: Vec<serde_json::Value>,
    /// Total tokens consumed.
    pub total_tokens: u64,
    /// Total execution duration in milliseconds.
    pub total_duration_ms: u64,
    /// Collaboration mode that was actually used.
    pub collaboration_mode_used: Option<String>,
}

/// Compact summary for list endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: Uuid,
    pub job_type: JobType,
    pub status: JobStatus,
    pub input_preview: String,
    pub execution_id: Option<String>,
    pub progress_pct: Option<f32>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Result included for completed/failed jobs so list view can show preview.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JobResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Job {
    /// Create a compact summary of this job.
    pub fn to_summary(&self) -> JobSummary {
        let preview = if self.input.message.chars().count() > 100 {
            let truncated: String = self.input.message.chars().take(97).collect();
            format!("{}...", truncated)
        } else {
            self.input.message.clone()
        };
        JobSummary {
            id: self.id,
            job_type: self.job_type,
            status: self.status,
            input_preview: preview,
            execution_id: self.execution_id.clone(),
            progress_pct: self.progress_pct,
            tags: self.tags.clone(),
            created_at: self.created_at,
            started_at: self.started_at,
            completed_at: self.completed_at,
            result: self.result.clone(),
            error: self.error.clone(),
        }
    }
}

/// API request to submit a new job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitJobRequest {
    pub message: String,
    pub collaboration_mode: Option<String>,
    pub model: Option<String>,
    pub budget_tokens: Option<u32>,
    pub notify_webhook: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// API response after submitting a job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitJobResponse {
    pub job_id: Uuid,
    pub status: JobStatus,
    pub stream_url: String,
}

/// API response for job listings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobListResponse {
    pub jobs: Vec<JobSummary>,
    pub total: i64,
}

/// Signal sent to a running job to control its execution.
///
/// Used via a `watch` channel so the job's `tokio::select!` loop
/// can react to pause or cancel requests from the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobSignal {
    /// Default state: keep executing.
    Continue,
    /// Pause the job at the next safe point, save a checkpoint, and return.
    Pause,
    /// Cancel the job immediately and report an error.
    Cancel,
}
