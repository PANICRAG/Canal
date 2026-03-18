//! Integration tests for the Gateway API endpoints
//!
//! These tests use axum_test to test the HTTP API endpoints.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use gateway_core::filesystem::{
    DirectoryConfig, DirectoryMode, FilesystemConfig, FilesystemService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

/// Create a minimal test router for filesystem endpoints
fn create_filesystem_test_router(temp_dir: &TempDir) -> Router {
    use axum::{
        extract::State,
        routing::{get, post},
        Json,
    };
    use serde::{Deserialize, Serialize};

    #[derive(Clone)]
    struct TestState {
        filesystem_service: Arc<FilesystemService>,
    }

    #[derive(Deserialize)]
    struct ReadFileRequest {
        path: String,
    }

    #[derive(Serialize)]
    struct ReadFileResponse {
        path: String,
        content: String,
        size: u64,
    }

    async fn read_file(
        State(state): State<TestState>,
        Json(request): Json<ReadFileRequest>,
    ) -> Result<Json<ReadFileResponse>, StatusCode> {
        let content = state
            .filesystem_service
            .read_file(&request.path)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;

        Ok(Json(ReadFileResponse {
            path: content.path,
            content: content.content,
            size: content.size,
        }))
    }

    #[derive(Deserialize)]
    struct WriteFileRequest {
        path: String,
        content: String,
        #[serde(default)]
        create_dirs: bool,
        #[serde(default)]
        overwrite: bool,
    }

    #[derive(Serialize)]
    struct WriteFileResponse {
        path: String,
        bytes_written: u64,
        created: bool,
    }

    async fn write_file(
        State(state): State<TestState>,
        Json(request): Json<WriteFileRequest>,
    ) -> Result<Json<WriteFileResponse>, StatusCode> {
        let result = state
            .filesystem_service
            .write_file(
                &request.path,
                &request.content,
                request.create_dirs,
                request.overwrite,
            )
            .await
            .map_err(|_| StatusCode::FORBIDDEN)?;

        Ok(Json(WriteFileResponse {
            path: result.path,
            bytes_written: result.bytes_written,
            created: result.created,
        }))
    }

    #[derive(Deserialize)]
    struct ListDirectoryRequest {
        path: String,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        include_hidden: bool,
    }

    #[derive(Serialize)]
    struct ListDirectoryResponse {
        path: String,
        total_count: usize,
        entries: Vec<DirectoryEntry>,
    }

    #[derive(Serialize)]
    struct DirectoryEntry {
        name: String,
        path: String,
        entry_type: String,
    }

    async fn list_directory(
        State(state): State<TestState>,
        Json(request): Json<ListDirectoryRequest>,
    ) -> Result<Json<ListDirectoryResponse>, StatusCode> {
        let listing = state
            .filesystem_service
            .list_directory(&request.path, request.recursive, request.include_hidden)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;

        Ok(Json(ListDirectoryResponse {
            path: listing.path,
            total_count: listing.total_count,
            entries: listing
                .entries
                .into_iter()
                .map(|e| DirectoryEntry {
                    name: e.name,
                    path: e.path,
                    entry_type: e.entry_type.to_string(),
                })
                .collect(),
        }))
    }

    #[derive(Deserialize)]
    struct SearchFilesRequest {
        path: String,
        pattern: String,
        #[serde(default)]
        file_pattern: Option<String>,
        #[serde(default = "default_max_results")]
        max_results: usize,
    }

    fn default_max_results() -> usize {
        100
    }

    #[derive(Serialize)]
    struct SearchFilesResponse {
        total_matches: usize,
        truncated: bool,
        matches: Vec<SearchMatch>,
    }

    #[derive(Serialize)]
    struct SearchMatch {
        path: String,
        line_number: usize,
        line_content: String,
    }

    async fn search_files(
        State(state): State<TestState>,
        Json(request): Json<SearchFilesRequest>,
    ) -> Result<Json<SearchFilesResponse>, StatusCode> {
        let result = state
            .filesystem_service
            .search(
                &request.path,
                &request.pattern,
                request.file_pattern.as_deref(),
                request.max_results,
            )
            .await
            .map_err(|_| StatusCode::BAD_REQUEST)?;

        Ok(Json(SearchFilesResponse {
            total_matches: result.total_matches,
            truncated: result.truncated,
            matches: result
                .matches
                .into_iter()
                .map(|m| SearchMatch {
                    path: m.path,
                    line_number: m.line_number,
                    line_content: m.line_content,
                })
                .collect(),
        }))
    }

    #[derive(Serialize)]
    struct AllowedDirectory {
        path: String,
        mode: String,
    }

    #[derive(Serialize)]
    struct AllowedDirectoriesResponse {
        directories: Vec<AllowedDirectory>,
    }

    async fn list_allowed_directories(
        State(state): State<TestState>,
    ) -> Json<AllowedDirectoriesResponse> {
        let directories = state
            .filesystem_service
            .allowed_directories()
            .iter()
            .map(|d| AllowedDirectory {
                path: d.path.clone(),
                mode: d.mode.to_string(),
            })
            .collect();

        Json(AllowedDirectoriesResponse { directories })
    }

    // Canonicalize the temp_dir path to handle symlinks (e.g., /tmp -> /private/tmp on macOS)
    let canonical_path = temp_dir.path().canonicalize().unwrap();
    let config = FilesystemConfig {
        enabled: true,
        allowed_directories: vec![DirectoryConfig {
            path: canonical_path.to_string_lossy().to_string(),
            mode: DirectoryMode::ReadWrite,
            description: Some("Test directory".to_string()),
            docker_mount_path: None,
        }],
        blocked_patterns: vec![".env".to_string(), "*.key".to_string()],
        max_read_bytes: 1024 * 1024,
        max_write_bytes: 512 * 1024,
        follow_symlinks: false,
        default_encoding: "utf-8".to_string(),
    };

    let state = TestState {
        filesystem_service: Arc::new(FilesystemService::new(config)),
    };

    Router::new()
        .route("/filesystem/read", post(read_file))
        .route("/filesystem/write", post(write_file))
        .route("/filesystem/list", post(list_directory))
        .route("/filesystem/search", post(search_files))
        .route("/filesystem/directories", get(list_allowed_directories))
        .with_state(state)
}

// ============================================================================
// Filesystem API Tests
// ============================================================================

#[tokio::test]
async fn test_api_read_file() {
    let temp_dir = TempDir::new().unwrap();
    let app = create_filesystem_test_router(&temp_dir);

    // Create test file
    let test_file = temp_dir.path().join("test.txt");
    std::fs::write(&test_file, "Hello, API!").unwrap();

    let request = Request::builder()
        .method("POST")
        .uri("/filesystem/read")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "path": test_file.to_string_lossy()
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["content"], "Hello, API!");
    assert_eq!(json["size"], 11);
}

#[tokio::test]
async fn test_api_read_file_not_found() {
    let temp_dir = TempDir::new().unwrap();
    let app = create_filesystem_test_router(&temp_dir);

    let request = Request::builder()
        .method("POST")
        .uri("/filesystem/read")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "path": temp_dir.path().join("nonexistent.txt").to_string_lossy()
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_api_write_file() {
    let temp_dir = TempDir::new().unwrap();
    let app = create_filesystem_test_router(&temp_dir);

    // Use canonicalized path to match the config
    let canonical_base = temp_dir.path().canonicalize().unwrap();
    let test_file = canonical_base.join("output.txt");

    let request = Request::builder()
        .method("POST")
        .uri("/filesystem/write")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "path": test_file.to_string_lossy(),
                "content": "Written via API",
                "create_dirs": false,
                "overwrite": false
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["created"].as_bool().unwrap());
    assert_eq!(json["bytes_written"], 15);

    // Verify file content
    let content = std::fs::read_to_string(&test_file).unwrap();
    assert_eq!(content, "Written via API");
}

#[tokio::test]
async fn test_api_write_file_blocked() {
    let temp_dir = TempDir::new().unwrap();
    let app = create_filesystem_test_router(&temp_dir);

    // Use canonicalized path to match the config
    let canonical_base = temp_dir.path().canonicalize().unwrap();
    let env_file = canonical_base.join(".env");

    let request = Request::builder()
        .method("POST")
        .uri("/filesystem/write")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "path": env_file.to_string_lossy(),
                "content": "SECRET=value"
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_api_list_directory() {
    let temp_dir = TempDir::new().unwrap();
    let app = create_filesystem_test_router(&temp_dir);

    // Create test files
    std::fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
    std::fs::write(temp_dir.path().join("file2.txt"), "content2").unwrap();
    std::fs::create_dir(temp_dir.path().join("subdir")).unwrap();

    let request = Request::builder()
        .method("POST")
        .uri("/filesystem/list")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "path": temp_dir.path().to_string_lossy(),
                "recursive": false,
                "include_hidden": false
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["total_count"], 3);
    let entries = json["entries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"file1.txt"));
    assert!(names.contains(&"file2.txt"));
    assert!(names.contains(&"subdir"));
}

#[tokio::test]
async fn test_api_search_files() {
    let temp_dir = TempDir::new().unwrap();
    let app = create_filesystem_test_router(&temp_dir);

    // Create test files
    std::fs::write(
        temp_dir.path().join("code.rs"),
        "fn main() {\n    println!(\"Hello\");\n}",
    )
    .unwrap();

    let request = Request::builder()
        .method("POST")
        .uri("/filesystem/search")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "path": temp_dir.path().to_string_lossy(),
                "pattern": "fn main",
                "max_results": 100
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["total_matches"].as_u64().unwrap() >= 1);
    let matches = json["matches"].as_array().unwrap();
    assert!(matches
        .iter()
        .any(|m| m["line_content"].as_str().unwrap().contains("fn main")));
}

#[tokio::test]
async fn test_api_list_allowed_directories() {
    let temp_dir = TempDir::new().unwrap();
    let app = create_filesystem_test_router(&temp_dir);

    let request = Request::builder()
        .method("GET")
        .uri("/filesystem/directories")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let directories = json["directories"].as_array().unwrap();
    assert_eq!(directories.len(), 1);
    assert_eq!(directories[0]["mode"], "rw");
}

// ============================================================================
// Health Check API Tests (Simulated)
// ============================================================================

fn create_health_test_router() -> Router {
    use axum::{routing::get, Json};
    use serde::Serialize;

    #[derive(Serialize)]
    struct HealthResponse {
        status: String,
        version: String,
    }

    async fn health_check() -> Json<HealthResponse> {
        Json(HealthResponse {
            status: "healthy".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    #[derive(Serialize)]
    struct ReadinessResponse {
        ready: bool,
        checks: ReadinessChecks,
    }

    #[derive(Serialize)]
    struct ReadinessChecks {
        database: bool,
        llm_router: bool,
    }

    async fn readiness_check() -> Json<ReadinessResponse> {
        Json(ReadinessResponse {
            ready: true,
            checks: ReadinessChecks {
                database: true,
                llm_router: true,
            },
        })
    }

    Router::new()
        .route("/health", get(health_check))
        .route("/health/ready", get(readiness_check))
}

#[tokio::test]
async fn test_health_check() {
    let app = create_health_test_router();

    let request = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "healthy");
}

#[tokio::test]
async fn test_readiness_check() {
    let app = create_health_test_router();

    let request = Request::builder()
        .method("GET")
        .uri("/health/ready")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["ready"].as_bool().unwrap());
}

// ============================================================================
// Code Execution API Tests (Simulated)
// ============================================================================

fn create_code_test_router() -> Router {
    use axum::{routing::post, Json};
    use serde::{Deserialize, Serialize};

    #[derive(Deserialize)]
    struct ExecuteRequest {
        code: String,
        language: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    }

    #[derive(Serialize)]
    struct ExecuteResponse {
        success: bool,
        stdout: String,
        stderr: String,
        execution_time_ms: u64,
    }

    async fn execute_code(Json(request): Json<ExecuteRequest>) -> Json<ExecuteResponse> {
        // Simulated response for testing API structure
        let (success, stdout, stderr) = match request.language.as_str() {
            "python" => {
                if request.code.contains("error") {
                    (false, String::new(), "SyntaxError".to_string())
                } else {
                    (true, "Executed successfully".to_string(), String::new())
                }
            }
            "bash" => {
                if request.code.contains("rm -rf") {
                    (false, String::new(), "Command blocked".to_string())
                } else {
                    (true, "Command executed".to_string(), String::new())
                }
            }
            _ => (false, String::new(), "Unsupported language".to_string()),
        };

        Json(ExecuteResponse {
            success,
            stdout,
            stderr,
            execution_time_ms: 100,
        })
    }

    #[derive(Serialize)]
    struct LanguagesResponse {
        languages: Vec<LanguageInfo>,
    }

    #[derive(Serialize)]
    struct LanguageInfo {
        name: String,
        enabled: bool,
    }

    async fn list_languages() -> Json<LanguagesResponse> {
        Json(LanguagesResponse {
            languages: vec![
                LanguageInfo {
                    name: "python".to_string(),
                    enabled: true,
                },
                LanguageInfo {
                    name: "bash".to_string(),
                    enabled: true,
                },
            ],
        })
    }

    Router::new()
        .route("/code/execute", post(execute_code))
        .route("/code/languages", axum::routing::get(list_languages))
}

#[tokio::test]
async fn test_code_execute_python() {
    let app = create_code_test_router();

    let request = Request::builder()
        .method("POST")
        .uri("/code/execute")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "code": "print('hello')",
                "language": "python"
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(json["success"].as_bool().unwrap());
}

#[tokio::test]
async fn test_code_execute_blocked() {
    let app = create_code_test_router();

    let request = Request::builder()
        .method("POST")
        .uri("/code/execute")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({
                "code": "rm -rf /",
                "language": "bash"
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert!(!json["success"].as_bool().unwrap());
    assert!(json["stderr"].as_str().unwrap().contains("blocked"));
}

#[tokio::test]
async fn test_list_languages() {
    let app = create_code_test_router();

    let request = Request::builder()
        .method("GET")
        .uri("/code/languages")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let languages = json["languages"].as_array().unwrap();
    let names: Vec<&str> = languages
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"python"));
    assert!(names.contains(&"bash"));
}
