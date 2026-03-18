//! Filesystem API endpoints
//!
//! Provides API routes for file system operations including reading, writing,
//! listing directories, and searching file content.

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use gateway_core::filesystem::{DirectoryListing, FileContent, SearchResult, WriteResult};
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, state::AppState};

/// Create the filesystem routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/read", post(read_file))
        .route("/write", post(write_file))
        .route("/list", post(list_directory))
        .route("/search", post(search_files))
        .route("/directories", get(list_allowed_directories))
}

/// Read file request
#[derive(Debug, Deserialize)]
pub struct ReadFileRequest {
    /// File path to read
    pub path: String,
    /// Encoding (default: utf-8)
    #[serde(default)]
    #[allow(dead_code)]
    pub encoding: Option<String>,
    /// Maximum bytes to read (default: config limit)
    #[serde(default)]
    #[allow(dead_code)]
    pub max_bytes: Option<u64>,
}

/// Read file response
#[derive(Debug, Serialize)]
pub struct ReadFileResponse {
    /// File path
    pub path: String,
    /// File content
    pub content: String,
    /// File size in bytes
    pub size: u64,
    /// Detected encoding
    pub encoding: String,
    /// Whether content was truncated
    pub truncated: bool,
}

impl From<FileContent> for ReadFileResponse {
    fn from(fc: FileContent) -> Self {
        Self {
            path: fc.path,
            content: fc.content,
            size: fc.size,
            encoding: fc.encoding,
            truncated: fc.truncated,
        }
    }
}

/// Read a file
pub async fn read_file(
    State(state): State<AppState>,
    Json(request): Json<ReadFileRequest>,
) -> Result<Json<ReadFileResponse>, ApiError> {
    let fs_service = state
        .filesystem_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("Filesystem service not available"))?;

    tracing::debug!(path = %request.path, "Reading file");

    let content = fs_service.read_file(&request.path).await.map_err(|e| {
        tracing::warn!(path = %request.path, error = %e, "Failed to read file");
        ApiError::from(e)
    })?;

    Ok(Json(content.into()))
}

/// Write file request
#[derive(Debug, Deserialize)]
pub struct WriteFileRequest {
    /// File path to write
    pub path: String,
    /// Content to write
    pub content: String,
    /// Create parent directories if needed
    #[serde(default)]
    pub create_dirs: bool,
    /// Overwrite if file exists
    #[serde(default)]
    pub overwrite: bool,
}

/// Write file response
#[derive(Debug, Serialize)]
pub struct WriteFileResponse {
    /// File path
    pub path: String,
    /// Bytes written
    pub bytes_written: u64,
    /// Whether file was created (vs overwritten)
    pub created: bool,
}

impl From<WriteResult> for WriteFileResponse {
    fn from(wr: WriteResult) -> Self {
        Self {
            path: wr.path,
            bytes_written: wr.bytes_written,
            created: wr.created,
        }
    }
}

/// Write a file
pub async fn write_file(
    State(state): State<AppState>,
    Json(request): Json<WriteFileRequest>,
) -> Result<Json<WriteFileResponse>, ApiError> {
    // R4-M: Enforce file content size limit to prevent DoS
    const MAX_FILE_CONTENT: usize = 10_000_000; // 10MB
    if request.content.len() > MAX_FILE_CONTENT {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "File content exceeds maximum size of {} bytes",
                MAX_FILE_CONTENT
            ),
        ));
    }

    let fs_service = state
        .filesystem_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("Filesystem service not available"))?;

    tracing::debug!(path = %request.path, "Writing file");

    let result = fs_service
        .write_file(
            &request.path,
            &request.content,
            request.create_dirs,
            request.overwrite,
        )
        .await
        .map_err(|e| {
            tracing::warn!(path = %request.path, error = %e, "Failed to write file");
            ApiError::from(e)
        })?;

    Ok(Json(result.into()))
}

/// List directory request
#[derive(Debug, Deserialize)]
pub struct ListDirectoryRequest {
    /// Directory path to list
    pub path: String,
    /// Recursively list subdirectories
    #[serde(default)]
    pub recursive: bool,
    /// Include hidden files (starting with .)
    #[serde(default)]
    pub include_hidden: bool,
}

/// List directory response
#[derive(Debug, Serialize)]
pub struct ListDirectoryResponse {
    /// Directory path
    pub path: String,
    /// Directory entries
    pub entries: Vec<DirectoryEntryResponse>,
    /// Total entry count
    pub total_count: usize,
}

/// Directory entry in response
#[derive(Debug, Serialize)]
pub struct DirectoryEntryResponse {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub size: Option<u64>,
    pub hidden: bool,
}

impl From<DirectoryListing> for ListDirectoryResponse {
    fn from(dl: DirectoryListing) -> Self {
        Self {
            path: dl.path,
            entries: dl
                .entries
                .into_iter()
                .map(|e| DirectoryEntryResponse {
                    name: e.name,
                    path: e.path,
                    entry_type: e.entry_type.to_string(),
                    size: e.size,
                    hidden: e.hidden,
                })
                .collect(),
            total_count: dl.total_count,
        }
    }
}

/// List directory contents
pub async fn list_directory(
    State(state): State<AppState>,
    Json(request): Json<ListDirectoryRequest>,
) -> Result<Json<ListDirectoryResponse>, ApiError> {
    let fs_service = state
        .filesystem_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("Filesystem service not available"))?;

    tracing::debug!(path = %request.path, "Listing directory");

    let listing = fs_service
        .list_directory(&request.path, request.recursive, request.include_hidden)
        .await
        .map_err(|e| {
            tracing::warn!(path = %request.path, error = %e, "Failed to list directory");
            ApiError::from(e)
        })?;

    Ok(Json(listing.into()))
}

/// Search files request
#[derive(Debug, Deserialize)]
pub struct SearchFilesRequest {
    /// Directory path to search
    pub path: String,
    /// Search pattern (regex)
    pub pattern: String,
    /// File pattern filter (e.g., "*.rs")
    #[serde(default)]
    pub file_pattern: Option<String>,
    /// Maximum results to return
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_max_results() -> usize {
    100
}

/// Search files response
#[derive(Debug, Serialize)]
pub struct SearchFilesResponse {
    /// Search matches
    pub matches: Vec<SearchMatchResponse>,
    /// Total matches found
    pub total_matches: usize,
    /// Files searched
    pub files_searched: usize,
    /// Whether results were truncated
    pub truncated: bool,
}

/// Search match in response
#[derive(Debug, Serialize)]
pub struct SearchMatchResponse {
    pub path: String,
    pub line_number: usize,
    pub line_content: String,
    pub match_start: usize,
    pub match_end: usize,
}

impl From<SearchResult> for SearchFilesResponse {
    fn from(sr: SearchResult) -> Self {
        Self {
            matches: sr
                .matches
                .into_iter()
                .map(|m| SearchMatchResponse {
                    path: m.path,
                    line_number: m.line_number,
                    line_content: m.line_content,
                    match_start: m.match_start,
                    match_end: m.match_end,
                })
                .collect(),
            total_matches: sr.total_matches,
            files_searched: sr.files_searched,
            truncated: sr.truncated,
        }
    }
}

/// Search file contents
pub async fn search_files(
    State(state): State<AppState>,
    Json(request): Json<SearchFilesRequest>,
) -> Result<Json<SearchFilesResponse>, ApiError> {
    let fs_service = state
        .filesystem_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("Filesystem service not available"))?;

    tracing::debug!(path = %request.path, pattern = %request.pattern, "Searching files");

    let result = fs_service
        .search(
            &request.path,
            &request.pattern,
            request.file_pattern.as_deref(),
            request.max_results,
        )
        .await
        .map_err(|e| {
            tracing::warn!(path = %request.path, error = %e, "Search failed");
            ApiError::from(e)
        })?;

    Ok(Json(result.into()))
}

/// Allowed directory info
#[derive(Debug, Serialize)]
pub struct AllowedDirectoryResponse {
    pub path: String,
    pub mode: String,
    pub description: Option<String>,
}

/// List allowed directories response
#[derive(Debug, Serialize)]
pub struct AllowedDirectoriesResponse {
    pub directories: Vec<AllowedDirectoryResponse>,
}

/// List allowed directories
pub async fn list_allowed_directories(
    State(state): State<AppState>,
) -> Result<Json<AllowedDirectoriesResponse>, ApiError> {
    let fs_service = state
        .filesystem_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("Filesystem service not available"))?;

    let directories: Vec<AllowedDirectoryResponse> = fs_service
        .allowed_directories()
        .iter()
        .map(|d| AllowedDirectoryResponse {
            path: d.path.clone(),
            mode: d.mode.to_string(),
            description: d.description.clone(),
        })
        .collect();

    Ok(Json(AllowedDirectoriesResponse { directories }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::filesystem::{FileContent, SearchMatch};

    #[test]
    fn test_read_file_response_conversion() {
        let fc = FileContent {
            path: "/tmp/test.txt".to_string(),
            content: "Hello".to_string(),
            size: 5,
            encoding: "utf-8".to_string(),
            truncated: false,
        };

        let response: ReadFileResponse = fc.into();
        assert_eq!(response.path, "/tmp/test.txt");
        assert_eq!(response.size, 5);
    }

    #[test]
    fn test_search_response_conversion() {
        let sr = SearchResult {
            matches: vec![SearchMatch {
                path: "/tmp/test.rs".to_string(),
                line_number: 10,
                line_content: "fn main()".to_string(),
                match_start: 0,
                match_end: 2,
            }],
            total_matches: 1,
            files_searched: 1,
            truncated: false,
        };

        let response: SearchFilesResponse = sr.into();
        assert_eq!(response.matches.len(), 1);
        assert_eq!(response.matches[0].line_number, 10);
    }
}
