//! Git repository management

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use uuid::Uuid;

use crate::error::{Error, Result};

/// Git repository record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct GitRepository {
    pub id: Uuid,
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub repo_url: String,
    pub local_path: String,
    pub current_branch: String,
    pub last_commit_hash: Option<String>,
    pub clone_status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Clone options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneOptions {
    pub repo_url: String,
    pub target_path: String,
    pub branch: Option<String>,
    pub depth: Option<u32>,
}

/// Repository manager
#[derive(Clone)]
pub struct RepositoryManager {
    db: PgPool,
    workspace_base: PathBuf,
}

impl RepositoryManager {
    /// Create a new repository manager
    pub fn new(db: PgPool, workspace_base: impl AsRef<Path>) -> Self {
        Self {
            db,
            workspace_base: workspace_base.as_ref().to_path_buf(),
        }
    }

    /// Clone a repository
    pub async fn clone_repository(
        &self,
        session_id: Uuid,
        user_id: Uuid,
        options: CloneOptions,
    ) -> Result<GitRepository> {
        let target_path = self.workspace_base.join(&options.target_path);

        // Build git clone command
        let mut cmd = Command::new("git");
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.arg("clone");

        if let Some(branch) = &options.branch {
            cmd.args(["--branch", branch]);
        }

        if let Some(depth) = options.depth {
            cmd.args(["--depth", &depth.to_string()]);
        }

        cmd.arg(&options.repo_url);
        cmd.arg(&target_path);

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        tracing::info!(
            repo_url = %options.repo_url,
            target_path = %target_path.display(),
            "Cloning repository"
        );

        let output = cmd
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to run git clone: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Git clone failed: {}", stderr)));
        }

        // Get current branch and commit
        let branch = self.get_current_branch(&target_path).await?;
        let commit = self.get_head_commit(&target_path).await?;

        // Store in database
        let repo = sqlx::query_as::<_, GitRepository>(
            r#"
            INSERT INTO git_repositories (
                session_id, user_id, repo_url, local_path,
                current_branch, last_commit_hash, clone_status
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'complete')
            RETURNING *
            "#,
        )
        .bind(session_id)
        .bind(user_id)
        .bind(&options.repo_url)
        .bind(&options.target_path)
        .bind(&branch)
        .bind(&commit)
        .fetch_one(&self.db)
        .await
        .map_err(Error::from)?;

        tracing::info!(
            repo_id = %repo.id,
            branch = %branch,
            commit = %commit.as_deref().unwrap_or("unknown"),
            "Repository cloned successfully"
        );

        Ok(repo)
    }

    /// Get repository by session
    pub async fn get_repository(&self, session_id: Uuid) -> Result<Option<GitRepository>> {
        sqlx::query_as::<_, GitRepository>(
            "SELECT * FROM git_repositories WHERE session_id = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(session_id)
        .fetch_optional(&self.db)
        .await
        .map_err(Error::from)
    }

    /// Get current branch
    async fn get_current_branch(&self, path: &Path) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get branch: {}", e)))?;

        if !output.status.success() {
            return Ok("main".to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Get HEAD commit hash
    async fn get_head_commit(&self, path: &Path) -> Result<Option<String>> {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get commit: {}", e)))?;

        if !output.status.success() {
            return Ok(None);
        }

        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    }

    /// Update repository record after operations
    pub async fn update_repository(
        &self,
        repo_id: Uuid,
        branch: &str,
        commit: Option<&str>,
    ) -> Result<GitRepository> {
        sqlx::query_as::<_, GitRepository>(
            r#"
            UPDATE git_repositories
            SET current_branch = $2, last_commit_hash = $3, updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(repo_id)
        .bind(branch)
        .bind(commit)
        .fetch_one(&self.db)
        .await
        .map_err(Error::from)
    }

    /// Delete repository record
    pub async fn delete_repository(&self, repo_id: Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM git_repositories WHERE id = $1")
            .bind(repo_id)
            .execute(&self.db)
            .await
            .map_err(Error::from)?;

        Ok(result.rows_affected() > 0)
    }
}
