//! Session repository for persistence

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::error::Result;

/// Session status enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// Session is active and running
    Active,
    /// Session is paused (container stopped, state preserved)
    Paused,
    /// Session has expired due to inactivity
    Expired,
    /// Session was explicitly terminated
    Terminated,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Active => write!(f, "active"),
            SessionStatus::Paused => write!(f, "paused"),
            SessionStatus::Expired => write!(f, "expired"),
            SessionStatus::Terminated => write!(f, "terminated"),
        }
    }
}

/// Session state record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SessionState {
    pub id: Uuid,
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub status: SessionStatus,
    pub container_id: Option<Uuid>,
    pub workspace_path: Option<String>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub last_tool_call_at: Option<DateTime<Utc>>,
    pub last_file_change_at: Option<DateTime<Utc>>,
    pub max_idle_minutes: i32,
    pub max_duration_hours: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub paused_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl SessionState {
    /// Check if session is active
    pub fn is_active(&self) -> bool {
        self.status == SessionStatus::Active
    }

    /// Check if session can be resumed
    pub fn can_resume(&self) -> bool {
        self.status == SessionStatus::Paused
    }

    /// Calculate idle time in minutes
    pub fn idle_minutes(&self) -> i64 {
        (Utc::now() - self.updated_at).num_minutes()
    }

    /// Check if session should expire due to inactivity
    pub fn should_expire(&self) -> bool {
        self.idle_minutes() > self.max_idle_minutes as i64
    }
}

/// Session file change record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SessionFile {
    pub id: Uuid,
    pub session_id: Uuid,
    pub file_path: String,
    pub file_hash: Option<String>,
    pub file_size: i64,
    pub change_type: String,
    pub previous_path: Option<String>,
    pub previous_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Session repository for database operations
#[derive(Clone)]
pub struct SessionRepository {
    db: PgPool,
}

impl SessionRepository {
    /// Create a new session repository
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Get or create session state
    pub async fn get_or_create_state(
        &self,
        session_id: Uuid,
        user_id: Uuid,
    ) -> Result<SessionState> {
        // Try to get existing state
        if let Some(state) = self.get_state(session_id).await? {
            return Ok(state);
        }

        // Create new state
        let expires_at = Utc::now() + chrono::Duration::hours(24);

        let state = sqlx::query_as::<_, SessionState>(
            r#"
            INSERT INTO session_states (session_id, user_id, expires_at)
            VALUES ($1, $2, $3)
            RETURNING *
            "#,
        )
        .bind(session_id)
        .bind(user_id)
        .bind(expires_at)
        .fetch_one(&self.db)
        .await?;

        Ok(state)
    }

    /// Get session state by session ID
    pub async fn get_state(&self, session_id: Uuid) -> Result<Option<SessionState>> {
        Ok(
            sqlx::query_as::<_, SessionState>("SELECT * FROM session_states WHERE session_id = $1")
                .bind(session_id)
                .fetch_optional(&self.db)
                .await?,
        )
    }

    /// Get all active sessions for a user
    pub async fn get_user_sessions(&self, user_id: Uuid) -> Result<Vec<SessionState>> {
        Ok(sqlx::query_as::<_, SessionState>(
            r#"
            SELECT * FROM session_states
            WHERE user_id = $1
            AND status IN ('active', 'paused')
            ORDER BY updated_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await?)
    }

    /// Update session status
    pub async fn update_status(
        &self,
        session_id: Uuid,
        status: SessionStatus,
    ) -> Result<SessionState> {
        let paused_at = if status == SessionStatus::Paused {
            Some(Utc::now())
        } else {
            None
        };

        Ok(sqlx::query_as::<_, SessionState>(
            r#"
            UPDATE session_states
            SET status = $2, paused_at = COALESCE($3, paused_at)
            WHERE session_id = $1
            RETURNING *
            "#,
        )
        .bind(session_id)
        .bind(status)
        .bind(paused_at)
        .fetch_one(&self.db)
        .await?)
    }

    /// Bind container to session
    pub async fn bind_container(
        &self,
        session_id: Uuid,
        container_id: Uuid,
    ) -> Result<SessionState> {
        Ok(sqlx::query_as::<_, SessionState>(
            r#"
            UPDATE session_states
            SET container_id = $2
            WHERE session_id = $1
            RETURNING *
            "#,
        )
        .bind(session_id)
        .bind(container_id)
        .fetch_one(&self.db)
        .await?)
    }

    /// Update last message timestamp
    pub async fn touch_message(&self, session_id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE session_states
            SET last_message_at = NOW(), updated_at = NOW()
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Update last tool call timestamp
    pub async fn touch_tool_call(&self, session_id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE session_states
            SET last_tool_call_at = NOW(), updated_at = NOW()
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Record a file change
    pub async fn record_file_change(
        &self,
        session_id: Uuid,
        file_path: &str,
        change_type: &str,
        file_hash: Option<&str>,
        file_size: i64,
    ) -> Result<SessionFile> {
        // Update session state
        sqlx::query(
            r#"
            UPDATE session_states
            SET last_file_change_at = NOW(), updated_at = NOW()
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .execute(&self.db)
        .await?;

        // Record the file change
        Ok(sqlx::query_as::<_, SessionFile>(
            r#"
            INSERT INTO session_files (session_id, file_path, change_type, file_hash, file_size)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
            "#,
        )
        .bind(session_id)
        .bind(file_path)
        .bind(change_type)
        .bind(file_hash)
        .bind(file_size)
        .fetch_one(&self.db)
        .await?)
    }

    /// Get file changes for a session
    pub async fn get_file_changes(&self, session_id: Uuid) -> Result<Vec<SessionFile>> {
        Ok(sqlx::query_as::<_, SessionFile>(
            r#"
            SELECT * FROM session_files
            WHERE session_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.db)
        .await?)
    }

    /// Find sessions that should expire
    pub async fn find_expired_sessions(&self) -> Result<Vec<SessionState>> {
        Ok(sqlx::query_as::<_, SessionState>(
            r#"
            SELECT * FROM session_states
            WHERE status = 'active'
            AND (
                expires_at < NOW()
                OR updated_at < NOW() - (max_idle_minutes || ' minutes')::INTERVAL
            )
            "#,
        )
        .fetch_all(&self.db)
        .await?)
    }

    /// Expire a session
    pub async fn expire_session(&self, session_id: Uuid) -> Result<SessionState> {
        self.update_status(session_id, SessionStatus::Expired).await
    }

    /// Terminate a session
    pub async fn terminate_session(&self, session_id: Uuid) -> Result<SessionState> {
        self.update_status(session_id, SessionStatus::Terminated)
            .await
    }

    /// Resume a paused session
    pub async fn resume_session(&self, session_id: Uuid) -> Result<SessionState> {
        let expires_at = Utc::now() + chrono::Duration::hours(24);

        Ok(sqlx::query_as::<_, SessionState>(
            r#"
            UPDATE session_states
            SET status = 'active', paused_at = NULL, expires_at = $2, updated_at = NOW()
            WHERE session_id = $1
            RETURNING *
            "#,
        )
        .bind(session_id)
        .bind(expires_at)
        .fetch_one(&self.db)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_status_display() {
        assert_eq!(SessionStatus::Active.to_string(), "active");
        assert_eq!(SessionStatus::Paused.to_string(), "paused");
    }
}
