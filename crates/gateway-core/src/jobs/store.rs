//! PostgreSQL-backed job store for persistent job lifecycle management.

use chrono::Utc;
use sqlx::PgPool;
use tracing::instrument;
use uuid::Uuid;

use super::error::JobError;
use super::types::*;

/// PostgreSQL-backed store for async job records.
#[derive(Debug, Clone)]
pub struct JobStore {
    pool: PgPool,
}

impl JobStore {
    /// Create a new job store backed by the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a new job and return it. Status starts as Queued.
    #[instrument(skip(self, input, metadata), fields(user_id = %user_id))]
    pub async fn create_job(
        &self,
        user_id: Uuid,
        job_type: JobType,
        input: &JobInput,
        metadata: serde_json::Value,
        notify_webhook: Option<String>,
    ) -> Result<Job, JobError> {
        let input_json = serde_json::to_value(input)?;
        let now = Utc::now();
        let id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO jobs (id, user_id, job_type, status, input, metadata, notify_webhook, created_at, updated_at)
            VALUES ($1, $2, $3, 'queued', $4, $5, $6, $7, $7)
            "#,
        )
        .bind(id)
        .bind(user_id)
        .bind(job_type)
        .bind(&input_json)
        .bind(&metadata)
        .bind(&notify_webhook)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(Job {
            id,
            user_id,
            session_id: None,
            job_type,
            status: JobStatus::Queued,
            input: input.clone(),
            result: None,
            error: None,
            checkpoint_id: None,
            execution_id: None,
            progress_pct: None,
            tags: vec![],
            metadata,
            notify_webhook,
            created_at: now,
            started_at: None,
            completed_at: None,
            updated_at: now,
        })
    }

    /// Atomically claim the next queued job for execution.
    /// Uses `SELECT FOR UPDATE SKIP LOCKED` to support concurrent schedulers.
    #[instrument(skip(self))]
    pub async fn claim_next_job(&self) -> Result<Option<Job>, JobError> {
        let now = Utc::now();

        let row = sqlx::query_as::<_, JobRow>(
            r#"
            UPDATE jobs
            SET status = 'running', started_at = $1, updated_at = $1
            WHERE id = (
                SELECT id FROM jobs
                WHERE status = 'queued'
                ORDER BY created_at ASC
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            )
            RETURNING *
            "#,
        )
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row.into_job()?)),
            None => Ok(None),
        }
    }

    /// Update the status of a job, validating the transition.
    ///
    /// R2-M: Added state machine validation — terminal states (Completed/Failed/Cancelled)
    /// cannot transition further, and only valid forward transitions are allowed.
    #[instrument(skip(self), fields(job_id = %job_id, new_status = %new_status))]
    pub async fn update_status(&self, job_id: Uuid, new_status: JobStatus) -> Result<(), JobError> {
        let now = Utc::now();
        let completed_at = match new_status {
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => Some(now),
            _ => None,
        };

        // Validate state transition: only update if current status allows this transition
        let valid_from = match new_status {
            JobStatus::Submitted => {
                return Err(JobError::InvalidTransition {
                    from: "any".into(),
                    to: "submitted".into(),
                })
            }
            JobStatus::Queued => "'submitted'",
            JobStatus::Running => "'submitted', 'queued', 'paused'",
            JobStatus::Paused => "'running'",
            JobStatus::Completed => "'running'",
            JobStatus::Failed => "'running'",
            JobStatus::Cancelled => "'submitted', 'queued', 'running', 'paused'",
        };

        let sql = format!(
            "UPDATE jobs SET status = $2, updated_at = $3, completed_at = COALESCE($4, completed_at) \
             WHERE id = $1 AND status::text IN ({valid_from})"
        );

        let result = sqlx::query(&sql)
            .bind(job_id)
            .bind(new_status)
            .bind(now)
            .bind(completed_at)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            // Check if job exists to distinguish not-found from invalid transition
            let exists =
                sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM jobs WHERE id = $1)")
                    .bind(job_id)
                    .fetch_one(&self.pool)
                    .await
                    .unwrap_or(false);
            if exists {
                return Err(JobError::InvalidTransition {
                    from: "current".into(),
                    to: new_status.to_string(),
                });
            }
            return Err(JobError::NotFound(job_id));
        }
        Ok(())
    }

    /// Set the result of a completed job.
    #[instrument(skip(self, result), fields(job_id = %job_id))]
    pub async fn set_result(&self, job_id: Uuid, result: &JobResult) -> Result<(), JobError> {
        let now = Utc::now();
        let result_json = serde_json::to_value(result)?;

        sqlx::query(
            r#"
            UPDATE jobs SET status = 'completed', result = $2, completed_at = $3, updated_at = $3
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(&result_json)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set the error of a failed job.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn set_error(&self, job_id: Uuid, error: &str) -> Result<(), JobError> {
        let now = Utc::now();

        sqlx::query(
            r#"
            UPDATE jobs SET status = 'failed', error = $2, completed_at = $3, updated_at = $3
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(error)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set the checkpoint ID for a paused job (for resume).
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn set_checkpoint(&self, job_id: Uuid, checkpoint_id: &str) -> Result<(), JobError> {
        let now = Utc::now();

        sqlx::query(
            r#"
            UPDATE jobs SET checkpoint_id = $2, updated_at = $3
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(checkpoint_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Link a job to an execution ID for SSE streaming.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn set_execution_id(&self, job_id: Uuid, execution_id: &str) -> Result<(), JobError> {
        let now = Utc::now();

        sqlx::query(
            r#"
            UPDATE jobs SET execution_id = $2, updated_at = $3
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(execution_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update job progress percentage.
    #[instrument(skip(self), fields(job_id = %job_id, progress = %progress_pct))]
    pub async fn set_progress(&self, job_id: Uuid, progress_pct: f32) -> Result<(), JobError> {
        let now = Utc::now();

        sqlx::query(
            r#"
            UPDATE jobs SET progress_pct = $2, updated_at = $3
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(progress_pct)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set tags on a job.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn set_tags(&self, job_id: Uuid, tags: &[String]) -> Result<(), JobError> {
        let now = Utc::now();

        sqlx::query(
            r#"
            UPDATE jobs SET tags = $2, updated_at = $3
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(tags)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Append a value to an array in the job's metadata JSON.
    ///
    /// If the key does not exist, creates a new array with the value.
    #[instrument(skip(self, value), fields(job_id = %job_id, key = %key))]
    pub async fn append_metadata(
        &self,
        job_id: Uuid,
        key: &str,
        value: &str,
    ) -> Result<(), JobError> {
        let now = Utc::now();

        sqlx::query(
            r#"
            UPDATE jobs
            SET metadata = jsonb_set(
                COALESCE(metadata, '{}'::jsonb),
                ARRAY[$2],
                COALESCE(metadata->$2, '[]'::jsonb) || to_jsonb($3::text)
            ),
            updated_at = $4
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(key)
        .bind(value)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get a single job by ID.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn get_job(&self, job_id: Uuid) -> Result<Option<Job>, JobError> {
        let row = sqlx::query_as::<_, JobRow>(r#"SELECT * FROM jobs WHERE id = $1"#)
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(row) => Ok(Some(row.into_job()?)),
            None => Ok(None),
        }
    }

    /// List jobs for a user with optional status filter and pagination.
    #[instrument(skip(self), fields(user_id = %user_id))]
    pub async fn list_jobs(
        &self,
        user_id: Uuid,
        status_filter: Option<JobStatus>,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<JobSummary>, i64), JobError> {
        let (rows, total) = match status_filter {
            Some(status) => {
                let rows = sqlx::query_as::<_, JobRow>(
                    r#"
                    SELECT * FROM jobs
                    WHERE user_id = $1 AND status = $2
                    ORDER BY created_at DESC
                    LIMIT $3 OFFSET $4
                    "#,
                )
                .bind(user_id)
                .bind(status)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await?;

                let total: (i64,) = sqlx::query_as(
                    r#"SELECT COUNT(*) FROM jobs WHERE user_id = $1 AND status = $2"#,
                )
                .bind(user_id)
                .bind(status)
                .fetch_one(&self.pool)
                .await?;

                (rows, total.0)
            }
            None => {
                let rows = sqlx::query_as::<_, JobRow>(
                    r#"
                    SELECT * FROM jobs
                    WHERE user_id = $1
                    ORDER BY created_at DESC
                    LIMIT $2 OFFSET $3
                    "#,
                )
                .bind(user_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await?;

                let total: (i64,) =
                    sqlx::query_as(r#"SELECT COUNT(*) FROM jobs WHERE user_id = $1"#)
                        .bind(user_id)
                        .fetch_one(&self.pool)
                        .await?;

                (rows, total.0)
            }
        };

        let summaries = rows
            .into_iter()
            .filter_map(|r| r.into_job().ok().map(|j| j.to_summary()))
            .collect();
        Ok((summaries, total))
    }

    /// List all currently running jobs (for recovery on startup).
    #[instrument(skip(self))]
    pub async fn list_active(&self) -> Result<Vec<Job>, JobError> {
        let rows = sqlx::query_as::<_, JobRow>(
            r#"SELECT * FROM jobs WHERE status = 'running' ORDER BY started_at ASC"#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|r| r.into_job()).collect()
    }

    /// Cancel a running, queued, or paused job.
    #[instrument(skip(self), fields(job_id = %job_id))]
    pub async fn cancel_job(&self, job_id: Uuid) -> Result<(), JobError> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE jobs
            SET status = 'cancelled', completed_at = $2, updated_at = $2
            WHERE id = $1 AND status IN ('running', 'queued', 'paused', 'submitted')
            "#,
        )
        .bind(job_id)
        .bind(now)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            // Check if the job exists but is already in a terminal state
            let exists =
                sqlx::query_as::<_, (JobStatus,)>(r#"SELECT status FROM jobs WHERE id = $1"#)
                    .bind(job_id)
                    .fetch_optional(&self.pool)
                    .await?;

            match exists {
                None => return Err(JobError::NotFound(job_id)),
                Some((status,)) if status == JobStatus::Cancelled => {
                    return Err(JobError::AlreadyCancelled(job_id));
                }
                Some((status,)) => {
                    return Err(JobError::InvalidTransition {
                        from: status.to_string(),
                        to: "cancelled".to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Requeue all running jobs back to queued (for recovery after restart).
    #[instrument(skip(self))]
    pub async fn requeue_running(&self) -> Result<u64, JobError> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE jobs
            SET status = 'queued', started_at = NULL, execution_id = NULL, updated_at = $1
            WHERE status = 'running'
            "#,
        )
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

/// Internal row type for sqlx mapping.
#[derive(Debug, sqlx::FromRow)]
struct JobRow {
    id: Uuid,
    user_id: Uuid,
    session_id: Option<Uuid>,
    job_type: JobType,
    status: JobStatus,
    input: serde_json::Value,
    result: Option<serde_json::Value>,
    error: Option<String>,
    checkpoint_id: Option<String>,
    execution_id: Option<String>,
    progress_pct: Option<f32>,
    tags: Vec<String>,
    metadata: serde_json::Value,
    notify_webhook: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl JobRow {
    fn into_job(self) -> Result<Job, JobError> {
        let input: JobInput = serde_json::from_value(self.input)?;
        let result: Option<JobResult> = self.result.map(serde_json::from_value).transpose()?;

        Ok(Job {
            id: self.id,
            user_id: self.user_id,
            session_id: self.session_id,
            job_type: self.job_type,
            status: self.status,
            input,
            result,
            error: self.error,
            checkpoint_id: self.checkpoint_id,
            execution_id: self.execution_id,
            progress_pct: self.progress_pct,
            tags: self.tags,
            metadata: self.metadata,
            notify_webhook: self.notify_webhook,
            created_at: self.created_at,
            started_at: self.started_at,
            completed_at: self.completed_at,
            updated_at: self.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_summary_truncation() {
        let long_message = "a".repeat(200);
        let job = Job {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            session_id: None,
            job_type: JobType::Chat,
            status: JobStatus::Queued,
            input: JobInput {
                message: long_message,
                collaboration_mode: None,
                model: None,
                budget_tokens: None,
                client_capabilities: None,
            },
            result: None,
            error: None,
            checkpoint_id: None,
            execution_id: None,
            progress_pct: None,
            tags: vec![],
            metadata: serde_json::json!({}),
            notify_webhook: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            updated_at: Utc::now(),
        };

        let summary = job.to_summary();
        assert_eq!(summary.input_preview.len(), 100);
        assert!(summary.input_preview.ends_with("..."));
    }
}
