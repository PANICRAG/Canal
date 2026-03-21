//! PostgreSQL storage implementation for devtools persistence.
//!
//! Observations stored as JSONB tagged union (serde-serialized `Observation` enum).
//! Sessions auto-created on trace ingest.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::DevtoolsError;
use crate::filter::{MetricsFilter, ObservationUpdate, TraceFilter, TraceUpdate};
use crate::traits::TraceStore;
use crate::types::*;

type Result<T> = std::result::Result<T, DevtoolsError>;

fn to_err(e: sqlx::Error) -> DevtoolsError {
    DevtoolsError::Internal(e.to_string())
}

/// PostgreSQL-backed trace store.
pub struct PgTraceStore {
    pool: PgPool,
}

impl PgTraceStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Ensure a session row exists when a trace references one.
    async fn ensure_session(&self, trace: &Trace) -> Result<()> {
        if let Some(ref session_id) = trace.session_id {
            sqlx::query(
                "INSERT INTO dt_sessions (id, project_id) VALUES ($1, $2)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(session_id)
            .bind(&trace.project_id)
            .execute(&self.pool)
            .await
            .map_err(to_err)?;
        }
        Ok(())
    }

    /// Parse a TraceStatus from the DB string.
    fn parse_status(s: &str) -> TraceStatus {
        match s {
            "completed" => TraceStatus::Completed,
            "error" => TraceStatus::Error,
            _ => TraceStatus::Running,
        }
    }

    fn status_str(s: &TraceStatus) -> &'static str {
        match s {
            TraceStatus::Running => "running",
            TraceStatus::Completed => "completed",
            TraceStatus::Error => "error",
        }
    }

    /// Build a Trace from a row tuple.
    fn row_to_trace(
        row: (
            String,                    // id
            String,                    // project_id
            Option<String>,            // session_id
            Option<String>,            // name
            Option<String>,            // user_id
            DateTime<Utc>,             // start_time
            Option<DateTime<Utc>>,     // end_time
            Option<serde_json::Value>, // input
            Option<serde_json::Value>, // output
            serde_json::Value,         // metadata
            serde_json::Value,         // tags
            String,                    // status
            i64,                       // total_tokens
            f64,                       // total_cost_usd
            i32,                       // observation_count
        ),
    ) -> Trace {
        let metadata: serde_json::Map<String, serde_json::Value> =
            row.9.as_object().cloned().unwrap_or_default();
        let tags: Vec<String> = row
            .10
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Trace {
            id: row.0,
            project_id: row.1,
            session_id: row.2,
            name: row.3,
            user_id: row.4,
            start_time: row.5,
            end_time: row.6,
            input: row.7,
            output: row.8,
            metadata,
            tags,
            status: Self::parse_status(&row.11),
            total_tokens: row.12,
            total_cost_usd: row.13,
            observation_count: row.14 as usize,
        }
    }
}

#[async_trait]
impl TraceStore for PgTraceStore {
    // ── Ingest ──────────────────────────────────────────────────────────

    async fn ingest_trace(&self, trace: Trace) -> Result<()> {
        self.ensure_session(&trace).await?;

        let tags_json = serde_json::to_value(&trace.tags).unwrap_or_default();
        let metadata_json = serde_json::to_value(&trace.metadata).unwrap_or_default();

        sqlx::query(
            "INSERT INTO dt_traces (id, project_id, session_id, name, user_id, start_time, end_time, input, output, metadata, tags, status, total_tokens, total_cost_usd, observation_count)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
             ON CONFLICT (id) DO UPDATE SET
                 status = EXCLUDED.status,
                 end_time = EXCLUDED.end_time,
                 output = EXCLUDED.output,
                 metadata = EXCLUDED.metadata,
                 tags = EXCLUDED.tags",
        )
        .bind(&trace.id)
        .bind(&trace.project_id)
        .bind(&trace.session_id)
        .bind(&trace.name)
        .bind(&trace.user_id)
        .bind(trace.start_time)
        .bind(trace.end_time)
        .bind(&trace.input)
        .bind(&trace.output)
        .bind(&metadata_json)
        .bind(&tags_json)
        .bind(Self::status_str(&trace.status))
        .bind(trace.total_tokens)
        .bind(trace.total_cost_usd)
        .bind(trace.observation_count as i32)
        .execute(&self.pool)
        .await
        .map_err(to_err)?;

        Ok(())
    }

    async fn ingest_observation(&self, obs: Observation) -> Result<()> {
        let obs_id = obs.id().to_string();
        let trace_id = obs.trace_id().to_string();
        let data =
            serde_json::to_value(&obs).map_err(|e| DevtoolsError::Internal(e.to_string()))?;

        // Insert observation
        sqlx::query(
            "INSERT INTO dt_observations (id, trace_id, data) VALUES ($1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET data = EXCLUDED.data",
        )
        .bind(&obs_id)
        .bind(&trace_id)
        .bind(&data)
        .execute(&self.pool)
        .await
        .map_err(to_err)?;

        // Update trace aggregation counters
        if let Observation::Generation(ref gen) = obs {
            let cost = gen.cost_usd.unwrap_or(0.0);
            sqlx::query(
                "UPDATE dt_traces SET
                     observation_count = observation_count + 1,
                     total_tokens = total_tokens + $2,
                     total_cost_usd = total_cost_usd + $3
                 WHERE id = $1",
            )
            .bind(&trace_id)
            .bind(gen.total_tokens as i64)
            .bind(cost)
            .execute(&self.pool)
            .await
            .map_err(to_err)?;
        } else {
            sqlx::query(
                "UPDATE dt_traces SET observation_count = observation_count + 1 WHERE id = $1",
            )
            .bind(&trace_id)
            .execute(&self.pool)
            .await
            .map_err(to_err)?;
        }

        Ok(())
    }

    async fn update_trace(&self, id: &str, update: TraceUpdate) -> Result<()> {
        // Build dynamic UPDATE — we always update at least one check
        let existing: Option<(String,)> = sqlx::query_as("SELECT id FROM dt_traces WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(to_err)?;

        if existing.is_none() {
            return Err(DevtoolsError::TraceNotFound { id: id.into() });
        }

        if let Some(status) = &update.status {
            sqlx::query("UPDATE dt_traces SET status = $2 WHERE id = $1")
                .bind(id)
                .bind(Self::status_str(status))
                .execute(&self.pool)
                .await
                .map_err(to_err)?;
        }
        if let Some(end_time) = update.end_time {
            sqlx::query("UPDATE dt_traces SET end_time = $2 WHERE id = $1")
                .bind(id)
                .bind(end_time)
                .execute(&self.pool)
                .await
                .map_err(to_err)?;
        }
        if let Some(output) = &update.output {
            sqlx::query("UPDATE dt_traces SET output = $2 WHERE id = $1")
                .bind(id)
                .bind(output)
                .execute(&self.pool)
                .await
                .map_err(to_err)?;
        }
        if let Some(name) = &update.name {
            sqlx::query("UPDATE dt_traces SET name = $2 WHERE id = $1")
                .bind(id)
                .bind(name)
                .execute(&self.pool)
                .await
                .map_err(to_err)?;
        }
        if let Some(tags) = &update.tags {
            let tags_json = serde_json::to_value(tags).unwrap_or_default();
            sqlx::query("UPDATE dt_traces SET tags = $2 WHERE id = $1")
                .bind(id)
                .bind(&tags_json)
                .execute(&self.pool)
                .await
                .map_err(to_err)?;
        }
        if let Some(metadata) = &update.metadata {
            let meta_json = serde_json::to_value(metadata).unwrap_or_default();
            sqlx::query("UPDATE dt_traces SET metadata = metadata || $2 WHERE id = $1")
                .bind(id)
                .bind(&meta_json)
                .execute(&self.pool)
                .await
                .map_err(to_err)?;
        }

        Ok(())
    }

    async fn update_observation(&self, id: &str, update: ObservationUpdate) -> Result<()> {
        // Read current observation JSONB
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT data FROM dt_observations WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(to_err)?;

        let (data,) = row.ok_or_else(|| DevtoolsError::ObservationNotFound { id: id.into() })?;

        let mut obs: Observation =
            serde_json::from_value(data).map_err(|e| DevtoolsError::Internal(e.to_string()))?;

        // Apply updates in-memory then write back
        match &mut obs {
            Observation::Span(ref mut span) => {
                if let Some(status) = &update.status {
                    span.status = status.clone();
                }
                if let Some(end_time) = update.end_time {
                    span.end_time = Some(end_time);
                }
                if let Some(output) = &update.output {
                    span.output = Some(output.clone());
                }
                if let Some(metadata) = &update.metadata {
                    for (k, v) in metadata {
                        span.metadata.insert(k.clone(), v.clone());
                    }
                }
            }
            Observation::Generation(ref mut gen) => {
                if let Some(status) = &update.status {
                    gen.status = status.clone();
                }
                if let Some(end_time) = update.end_time {
                    gen.end_time = Some(end_time);
                }
                if let Some(output) = &update.output {
                    gen.output = Some(output.clone());
                }
                if let Some(input_tokens) = update.input_tokens {
                    gen.input_tokens = input_tokens;
                }
                if let Some(output_tokens) = update.output_tokens {
                    gen.output_tokens = output_tokens;
                }
                if let Some(total_tokens) = update.total_tokens {
                    gen.total_tokens = total_tokens;
                }
                if let Some(cost_usd) = update.cost_usd {
                    gen.cost_usd = Some(cost_usd);
                }
                if let Some(metadata) = &update.metadata {
                    for (k, v) in metadata {
                        gen.metadata.insert(k.clone(), v.clone());
                    }
                }
            }
            Observation::Event(_) => {
                // Events are immutable
            }
        }

        let new_data =
            serde_json::to_value(&obs).map_err(|e| DevtoolsError::Internal(e.to_string()))?;

        sqlx::query("UPDATE dt_observations SET data = $2 WHERE id = $1")
            .bind(id)
            .bind(&new_data)
            .execute(&self.pool)
            .await
            .map_err(to_err)?;

        Ok(())
    }

    // ── Query: Traces ───────────────────────────────────────────────────

    async fn get_trace(&self, id: &str) -> Result<Option<Trace>> {
        let row: Option<(String, String, Option<String>, Option<String>, Option<String>, DateTime<Utc>, Option<DateTime<Utc>>, Option<serde_json::Value>, Option<serde_json::Value>, serde_json::Value, serde_json::Value, String, i64, f64, i32)> =
            sqlx::query_as(
                "SELECT id, project_id, session_id, name, user_id, start_time, end_time, input, output, metadata, tags, status, total_tokens, total_cost_usd, observation_count
                 FROM dt_traces WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(to_err)?;

        Ok(row.map(Self::row_to_trace))
    }

    async fn list_traces(&self, filter: TraceFilter) -> Result<Vec<Trace>> {
        // Build dynamic query with bind parameters
        // R5-C15: Fix off-by-one — idx must start at 0 so first filter gets $1
        let mut idx = 0u32;

        // We'll use a single query with all optional filters as COALESCE patterns
        // For simplicity, use conditional SQL
        let mut sql = String::from(
            "SELECT id, project_id, session_id, name, user_id, start_time, end_time, input, output, metadata, tags, status, total_tokens, total_cost_usd, observation_count
             FROM dt_traces WHERE TRUE",
        );

        if filter.project_id.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND project_id = ${}", idx));
        }
        if filter.session_id.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND session_id = ${}", idx));
        }
        if filter.status.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND status = ${}", idx));
        }
        if filter.user_id.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND user_id = ${}", idx));
        }
        if filter.tag.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND tags ? ${}", idx));
        }
        if filter.name.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND name ILIKE '%' || ${} || '%'", idx));
        }
        if filter.start_after.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND start_time >= ${}", idx));
        }
        if filter.start_before.is_some() {
            idx += 1;
            sql.push_str(&format!(" AND start_time <= ${}", idx));
        }

        sql.push_str(" ORDER BY start_time DESC");
        idx += 1;
        sql.push_str(&format!(" LIMIT ${}", idx));
        idx += 1;
        sql.push_str(&format!(" OFFSET ${}", idx));

        // Build the query with dynamic binds
        let mut query = sqlx::query_as::<
            _,
            (
                String,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
                DateTime<Utc>,
                Option<DateTime<Utc>>,
                Option<serde_json::Value>,
                Option<serde_json::Value>,
                serde_json::Value,
                serde_json::Value,
                String,
                i64,
                f64,
                i32,
            ),
        >(&sql);

        if let Some(ref v) = filter.project_id {
            query = query.bind(v);
        }
        if let Some(ref v) = filter.session_id {
            query = query.bind(v);
        }
        if let Some(ref v) = filter.status {
            query = query.bind(Self::status_str(v));
        }
        if let Some(ref v) = filter.user_id {
            query = query.bind(v);
        }
        if let Some(ref v) = filter.tag {
            query = query.bind(v);
        }
        if let Some(ref v) = filter.name {
            query = query.bind(v);
        }
        if let Some(v) = filter.start_after {
            query = query.bind(v);
        }
        if let Some(v) = filter.start_before {
            query = query.bind(v);
        }

        query = query.bind(filter.limit as i64);
        query = query.bind(filter.offset as i64);

        let rows = query.fetch_all(&self.pool).await.map_err(to_err)?;

        Ok(rows.into_iter().map(Self::row_to_trace).collect())
    }

    async fn get_trace_observations(&self, trace_id: &str) -> Result<Vec<Observation>> {
        let rows: Vec<(serde_json::Value,)> =
            sqlx::query_as("SELECT data FROM dt_observations WHERE trace_id = $1")
                .bind(trace_id)
                .fetch_all(&self.pool)
                .await
                .map_err(to_err)?;

        let mut observations = Vec::with_capacity(rows.len());
        for (data,) in rows {
            let obs: Observation =
                serde_json::from_value(data).map_err(|e| DevtoolsError::Internal(e.to_string()))?;
            observations.push(obs);
        }
        Ok(observations)
    }

    // ── Query: Sessions ─────────────────────────────────────────────────

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let row: Option<(String, String, DateTime<Utc>, serde_json::Value)> = sqlx::query_as(
            "SELECT id, project_id, created_at, metadata FROM dt_sessions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(to_err)?;

        Ok(row.map(|r| Session {
            id: r.0,
            project_id: r.1,
            created_at: r.2,
            metadata: r.3.as_object().cloned().unwrap_or_default(),
        }))
    }

    async fn list_sessions(&self, project_id: Option<&str>, limit: usize) -> Result<Vec<Session>> {
        let rows: Vec<(String, String, DateTime<Utc>, serde_json::Value)> =
            if let Some(pid) = project_id {
                sqlx::query_as(
                    "SELECT id, project_id, created_at, metadata FROM dt_sessions
                     WHERE project_id = $1 ORDER BY created_at DESC LIMIT $2",
                )
                .bind(pid)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(to_err)?
            } else {
                sqlx::query_as(
                    "SELECT id, project_id, created_at, metadata FROM dt_sessions
                     ORDER BY created_at DESC LIMIT $1",
                )
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(to_err)?
            };

        Ok(rows
            .into_iter()
            .map(|r| Session {
                id: r.0,
                project_id: r.1,
                created_at: r.2,
                metadata: r.3.as_object().cloned().unwrap_or_default(),
            })
            .collect())
    }

    async fn get_session_traces(&self, session_id: &str) -> Result<Vec<Trace>> {
        let rows: Vec<(String, String, Option<String>, Option<String>, Option<String>, DateTime<Utc>, Option<DateTime<Utc>>, Option<serde_json::Value>, Option<serde_json::Value>, serde_json::Value, serde_json::Value, String, i64, f64, i32)> =
            sqlx::query_as(
                "SELECT id, project_id, session_id, name, user_id, start_time, end_time, input, output, metadata, tags, status, total_tokens, total_cost_usd, observation_count
                 FROM dt_traces WHERE session_id = $1 ORDER BY start_time ASC",
            )
            .bind(session_id)
            .fetch_all(&self.pool)
            .await
            .map_err(to_err)?;

        Ok(rows.into_iter().map(Self::row_to_trace).collect())
    }

    // ── Query: Metrics ──────────────────────────────────────────────────

    async fn get_metrics_summary(&self, filter: MetricsFilter) -> Result<MetricsSummary> {
        // Trace-level aggregation
        let mut sql = String::from(
            "SELECT COUNT(*)::BIGINT, SUM(total_tokens)::BIGINT, SUM(total_cost_usd),
                    SUM(observation_count)::BIGINT,
                    COUNT(*) FILTER (WHERE status = 'running')::BIGINT,
                    COUNT(*) FILTER (WHERE status = 'completed')::BIGINT,
                    COUNT(*) FILTER (WHERE status = 'error')::BIGINT,
                    AVG(EXTRACT(EPOCH FROM (end_time - start_time)) * 1000) FILTER (WHERE end_time IS NOT NULL)
             FROM dt_traces WHERE TRUE",
        );

        let mut bind_idx = 0u32;
        if filter.project_id.is_some() {
            bind_idx += 1;
            sql.push_str(&format!(" AND project_id = ${}", bind_idx));
        }
        if filter.start_time.is_some() {
            bind_idx += 1;
            sql.push_str(&format!(" AND start_time >= ${}", bind_idx));
        }
        if filter.end_time.is_some() {
            bind_idx += 1;
            sql.push_str(&format!(" AND start_time <= ${}", bind_idx));
        }

        let mut query = sqlx::query_as::<
            _,
            (
                i64,
                Option<i64>,
                Option<f64>,
                Option<i64>,
                i64,
                i64,
                i64,
                Option<f64>,
            ),
        >(&sql);

        if let Some(ref v) = filter.project_id {
            query = query.bind(v);
        }
        if let Some(v) = filter.start_time {
            query = query.bind(v);
        }
        if let Some(v) = filter.end_time {
            query = query.bind(v);
        }

        let (total_traces, total_tokens, total_cost, total_obs, running, completed, error, avg_dur) =
            query.fetch_one(&self.pool).await.map_err(to_err)?;

        // Model usage from observations
        let mut model_sql = String::from(
            "SELECT o.data->>'model', COUNT(*)::BIGINT, SUM((o.data->>'total_tokens')::BIGINT)::BIGINT, SUM((o.data->>'cost_usd')::DOUBLE PRECISION)
             FROM dt_observations o
             JOIN dt_traces t ON o.trace_id = t.id
             WHERE o.data->>'observation_type' = 'generation'",
        );

        let mut midx = 0u32;
        if filter.project_id.is_some() {
            midx += 1;
            model_sql.push_str(&format!(" AND t.project_id = ${}", midx));
        }
        model_sql.push_str(" GROUP BY o.data->>'model'");

        let mut model_query =
            sqlx::query_as::<_, (Option<String>, i64, Option<i64>, Option<f64>)>(&model_sql);

        if let Some(ref v) = filter.project_id {
            model_query = model_query.bind(v);
        }

        let model_rows = model_query.fetch_all(&self.pool).await.map_err(to_err)?;

        let model_usage: Vec<ModelUsage> = model_rows
            .into_iter()
            .map(|(model, count, tokens, cost)| ModelUsage {
                model: model.unwrap_or_else(|| "unknown".to_string()),
                call_count: count as usize,
                total_tokens: tokens.unwrap_or(0),
                total_cost_usd: cost.unwrap_or(0.0),
            })
            .collect();

        Ok(MetricsSummary {
            total_traces: total_traces as usize,
            total_observations: total_obs.unwrap_or(0) as usize,
            total_tokens: total_tokens.unwrap_or(0),
            total_cost_usd: total_cost.unwrap_or(0.0),
            avg_trace_duration_ms: avg_dur.unwrap_or(0.0),
            model_usage,
            traces_by_status: TracesByStatus {
                running: running as usize,
                completed: completed as usize,
                error: error as usize,
            },
        })
    }

    // ── Projects ────────────────────────────────────────────────────────

    async fn create_project(&self, project: Project) -> Result<()> {
        let metadata_json = serde_json::to_value(&project.metadata).unwrap_or_default();

        sqlx::query(
            "INSERT INTO dt_projects (id, name, service_type, endpoint, api_key, created_at, metadata)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&project.id)
        .bind(&project.name)
        .bind(&project.service_type)
        .bind(&project.endpoint)
        .bind(&project.api_key)
        .bind(project.created_at)
        .bind(&metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.constraint() == Some("dt_projects_pkey") {
                    return DevtoolsError::ProjectAlreadyExists {
                        id: project.id.clone(),
                    };
                }
            }
            to_err(e)
        })?;

        Ok(())
    }

    async fn get_project(&self, id: &str) -> Result<Option<Project>> {
        let row: Option<(
            String,
            String,
            String,
            Option<String>,
            String,
            DateTime<Utc>,
            serde_json::Value,
        )> = sqlx::query_as(
            "SELECT id, name, service_type, endpoint, api_key, created_at, metadata
                 FROM dt_projects WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(to_err)?;

        Ok(row.map(|r| Project {
            id: r.0,
            name: r.1,
            service_type: r.2,
            endpoint: r.3,
            api_key: r.4,
            created_at: r.5,
            metadata: r.6.as_object().cloned().unwrap_or_default(),
        }))
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        let rows: Vec<(
            String,
            String,
            String,
            Option<String>,
            String,
            DateTime<Utc>,
            serde_json::Value,
        )> = sqlx::query_as(
            "SELECT id, name, service_type, endpoint, api_key, created_at, metadata
                 FROM dt_projects ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(to_err)?;

        Ok(rows
            .into_iter()
            .map(|r| Project {
                id: r.0,
                name: r.1,
                service_type: r.2,
                endpoint: r.3,
                api_key: r.4,
                created_at: r.5,
                metadata: r.6.as_object().cloned().unwrap_or_default(),
            })
            .collect())
    }

    async fn delete_project(&self, id: &str) -> Result<()> {
        // R5-M: Wrap in transaction to prevent orphaned records on partial failure
        let mut tx = self.pool.begin().await.map_err(to_err)?;

        // CASCADE on dt_observations handles cleanup.
        // Delete traces (which cascades observations), then sessions, then project.
        sqlx::query("DELETE FROM dt_traces WHERE project_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(to_err)?;

        sqlx::query("DELETE FROM dt_sessions WHERE project_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(to_err)?;

        sqlx::query("DELETE FROM dt_projects WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(to_err)?;

        tx.commit().await.map_err(to_err)?;
        Ok(())
    }

    async fn resolve_project_key(&self, api_key: &str) -> Result<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT id FROM dt_projects WHERE api_key = $1")
                .bind(api_key)
                .fetch_optional(&self.pool)
                .await
                .map_err(to_err)?;
        Ok(row.map(|r| r.0))
    }
}
