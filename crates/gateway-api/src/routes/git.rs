//! Git API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::middleware::auth::AuthContext;
use crate::state::AppState;
use gateway_core::git::{GitDiff, GitOperations, GitStatus, RepositoryManager};

/// Create git routes
pub fn routes() -> Router<AppState> {
    Router::new()
        // Repository management
        .route("/clone", post(clone_repository))
        .route("/repositories/{session_id}", get(get_repository))
        // Git operations (require session context)
        .route("/status", get(get_status))
        .route("/diff", get(get_diff))
        .route("/commit", post(create_commit))
        .route("/checkout", post(checkout_branch))
        .route("/branch", post(create_branch))
        .route("/branches", get(list_branches))
        .route("/pull", post(pull_changes))
        .route("/push", post(push_changes))
}

// ============================================================================
// Request/Response types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CloneRequest {
    pub repo_url: String,
    pub target_path: String,
    pub branch: Option<String>,
    pub depth: Option<u32>,
    pub session_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct CloneResponse {
    pub id: Uuid,
    pub repo_url: String,
    pub local_path: String,
    pub branch: String,
    pub commit: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StatusQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct DiffQuery {
    pub path: String,
    #[serde(default)]
    pub staged: bool,
}

#[derive(Debug, Deserialize)]
pub struct CommitRequest {
    pub path: String,
    pub message: String,
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct CommitResponse {
    pub commit_hash: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct CheckoutRequest {
    pub path: String,
    pub branch: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateBranchRequest {
    pub path: String,
    pub branch: String,
}

#[derive(Debug, Deserialize)]
pub struct BranchesQuery {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct BranchesResponse {
    pub branches: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct PushRequest {
    pub path: String,
    #[serde(default)]
    pub set_upstream: bool,
}

#[derive(Debug, Serialize)]
pub struct GitErrorResponse {
    pub error: String,
    pub code: String,
}

// ============================================================================
// Route handlers
// ============================================================================

/// Clone a repository
pub async fn clone_repository(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(request): Json<CloneRequest>,
) -> Result<Json<CloneResponse>, (StatusCode, Json<GitErrorResponse>)> {
    // Create workspace base path (could be configurable)
    let workspace_base =
        std::env::var("WORKSPACE_BASE").unwrap_or_else(|_| "/tmp/canal/workspaces".to_string());

    let manager = RepositoryManager::new(state.db.clone(), &workspace_base);

    let options = gateway_core::git::repository::CloneOptions {
        repo_url: request.repo_url,
        target_path: request.target_path,
        branch: request.branch,
        depth: request.depth,
    };

    // R4-M: Use authenticated user_id instead of caller-supplied user_id
    match manager
        .clone_repository(request.session_id, auth.user_id, options)
        .await
    {
        Ok(repo) => Ok(Json(CloneResponse {
            id: repo.id,
            repo_url: repo.repo_url,
            local_path: repo.local_path,
            branch: repo.current_branch,
            commit: repo.last_commit_hash,
        })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "CLONE_FAILED".to_string(),
            }),
        )),
    }
}

/// Get repository info by session
pub async fn get_repository(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<CloneResponse>, (StatusCode, Json<GitErrorResponse>)> {
    let workspace_base =
        std::env::var("WORKSPACE_BASE").unwrap_or_else(|_| "/tmp/canal/workspaces".to_string());

    let manager = RepositoryManager::new(state.db.clone(), &workspace_base);

    match manager.get_repository(session_id).await {
        Ok(Some(repo)) => Ok(Json(CloneResponse {
            id: repo.id,
            repo_url: repo.repo_url,
            local_path: repo.local_path,
            branch: repo.current_branch,
            commit: repo.last_commit_hash,
        })),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(GitErrorResponse {
                error: "Repository not found".to_string(),
                code: "NOT_FOUND".to_string(),
            }),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "QUERY_FAILED".to_string(),
            }),
        )),
    }
}

/// Get repository status
pub async fn get_status(
    Query(query): Query<StatusQuery>,
) -> Result<Json<GitStatus>, (StatusCode, Json<GitErrorResponse>)> {
    let ops = GitOperations::new(&query.path);

    match ops.status().await {
        Ok(status) => Ok(Json(status)),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "STATUS_FAILED".to_string(),
            }),
        )),
    }
}

/// Get diff
pub async fn get_diff(
    Query(query): Query<DiffQuery>,
) -> Result<Json<GitDiff>, (StatusCode, Json<GitErrorResponse>)> {
    let ops = GitOperations::new(&query.path);

    match ops.diff(query.staged).await {
        Ok(diff) => Ok(Json(diff)),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "DIFF_FAILED".to_string(),
            }),
        )),
    }
}

/// Create a commit
pub async fn create_commit(
    Json(request): Json<CommitRequest>,
) -> Result<Json<CommitResponse>, (StatusCode, Json<GitErrorResponse>)> {
    let ops = GitOperations::new(&request.path);

    match ops.commit(&request.message, request.files).await {
        Ok(commit_hash) => Ok(Json(CommitResponse {
            commit_hash,
            message: request.message,
        })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "COMMIT_FAILED".to_string(),
            }),
        )),
    }
}

/// Checkout a branch
pub async fn checkout_branch(
    Json(request): Json<CheckoutRequest>,
) -> Result<StatusCode, (StatusCode, Json<GitErrorResponse>)> {
    let ops = GitOperations::new(&request.path);

    match ops.checkout(&request.branch).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "CHECKOUT_FAILED".to_string(),
            }),
        )),
    }
}

/// Create a new branch
pub async fn create_branch(
    Json(request): Json<CreateBranchRequest>,
) -> Result<StatusCode, (StatusCode, Json<GitErrorResponse>)> {
    let ops = GitOperations::new(&request.path);

    match ops.create_branch(&request.branch).await {
        Ok(()) => Ok(StatusCode::CREATED),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "CREATE_BRANCH_FAILED".to_string(),
            }),
        )),
    }
}

/// List branches
pub async fn list_branches(
    Query(query): Query<BranchesQuery>,
) -> Result<Json<BranchesResponse>, (StatusCode, Json<GitErrorResponse>)> {
    let ops = GitOperations::new(&query.path);

    match ops.list_branches().await {
        Ok(branches) => Ok(Json(BranchesResponse { branches })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "LIST_BRANCHES_FAILED".to_string(),
            }),
        )),
    }
}

/// Pull changes
pub async fn pull_changes(
    Json(request): Json<PullRequest>,
) -> Result<StatusCode, (StatusCode, Json<GitErrorResponse>)> {
    let ops = GitOperations::new(&request.path);

    match ops.pull().await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "PULL_FAILED".to_string(),
            }),
        )),
    }
}

/// Push changes
pub async fn push_changes(
    Json(request): Json<PushRequest>,
) -> Result<StatusCode, (StatusCode, Json<GitErrorResponse>)> {
    let ops = GitOperations::new(&request.path);

    match ops.push(request.set_upstream).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(GitErrorResponse {
                error: e.to_string(),
                code: "PUSH_FAILED".to_string(),
            }),
        )),
    }
}
