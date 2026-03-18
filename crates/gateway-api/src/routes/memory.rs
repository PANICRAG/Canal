//! Memory endpoints
//!
//! Unified memory API that provides:
//! - User context (preferences, patterns, recent entries)
//! - Full-text search across all memory entries
//! - Preferences management
//! - Memory entry CRUD operations
//! - Memory Tool integration for LLM access

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
    Json, Router,
};
use gateway_core::memory::{Confidence, MemoryCategory, MemoryEntry, MemorySource};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{error::ApiError, state::AppState};

/// Validate that the path user_id matches the authenticated user (IDOR prevention).
/// Returns the validated user_id or rejects with 403 FORBIDDEN.
/// Admins can access any user's memory.
fn validate_user_ownership(
    path_user_id: Uuid,
    auth: &Option<canal_auth::AuthContext>,
) -> Result<Uuid, ApiError> {
    if let Some(auth) = auth {
        if auth.user_id != path_user_id && auth.role != "admin" {
            return Err(ApiError::forbidden("Cannot access another user's memory"));
        }
    }
    Ok(path_user_id)
}

/// Create the memory routes
pub fn routes() -> Router<AppState> {
    Router::new()
        // Context & Search
        .route("/context/{user_id}", get(get_context))
        .route("/search/{user_id}", post(search_memory))
        .route("/semantic-search/{user_id}", post(semantic_search_memory))
        // Preferences
        .route("/preferences/{user_id}", get(get_preferences))
        .route("/preferences/{user_id}", post(update_preferences))
        // Entry CRUD
        .route("/entries/{user_id}", get(list_entries))
        .route("/entries/{user_id}", post(create_entry))
        .route("/entries/{user_id}/{key}", get(get_entry))
        .route("/entries/{user_id}/{key}", put(update_entry))
        .route("/entries/{user_id}/{key}", delete(delete_entry))
        // Stats
        .route("/stats/{user_id}", get(get_stats))
}

/// Memory context response
#[derive(Debug, Serialize)]
pub struct ContextResponse {
    pub user_id: Uuid,
    pub preferences: PreferencesResponse,
    pub patterns: Vec<PatternInfo>,
    pub recent_entries: Vec<EntryInfo>,
    pub stats: StatsResponse,
}

/// Pattern information
#[derive(Debug, Serialize)]
pub struct PatternInfo {
    pub id: Uuid,
    pub pattern_type: String,
    pub description: String,
    pub confidence: f32,
    pub occurrence_count: u32,
}

/// Entry information (summary)
#[derive(Debug, Serialize)]
pub struct EntryInfo {
    pub id: Uuid,
    pub key: String,
    pub category: String,
    pub title: Option<String>,
    pub content_preview: String,
    pub updated_at: String,
}

/// Get user context (preferences, patterns, and recent entries)
pub async fn get_context(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    auth: Option<axum::Extension<canal_auth::AuthContext>>,
) -> Result<Json<ContextResponse>, ApiError> {
    let user_id = validate_user_ownership(user_id, &auth.map(|a| a.0))?;
    let memory = &state.unified_memory;

    let context = memory.get_context(user_id).await;

    Ok(Json(ContextResponse {
        user_id,
        preferences: PreferencesResponse {
            user_id,
            language: context.preferences.language.clone(),
            timezone: context.preferences.timezone.clone(),
            default_model: context.preferences.default_model.clone(),
            communication_style: context.preferences.communication_style.clone(),
        },
        patterns: context
            .patterns
            .iter()
            .map(|p| PatternInfo {
                id: p.id,
                pattern_type: format!("{:?}", p.pattern_type),
                description: p.description.clone(),
                confidence: p.confidence,
                occurrence_count: p.occurrence_count,
            })
            .collect(),
        recent_entries: context
            .recent_entries
            .iter()
            .map(|e| EntryInfo {
                id: e.id,
                key: e.key.clone(),
                category: format!("{:?}", e.category),
                title: e.title.clone(),
                content_preview: e.content.chars().take(200).collect(),
                updated_at: e.updated_at.to_rfc3339(),
            })
            .collect(),
        stats: StatsResponse {
            total_entries: context.stats.total_entries,
            by_category: context.stats.by_category.clone(),
            total_size_bytes: context.stats.total_size_bytes,
        },
    }))
}

/// Memory search request
#[derive(Debug, Deserialize)]
pub struct MemorySearchRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    10
}

/// Memory search result
#[derive(Debug, Serialize)]
pub struct MemorySearchResult {
    pub items: Vec<MemorySearchItem>,
    pub total_count: usize,
}

/// Memory search item
#[derive(Debug, Serialize)]
pub struct MemorySearchItem {
    pub id: Uuid,
    pub key: String,
    pub category: String,
    pub title: Option<String>,
    pub content_preview: String,
    pub tags: Vec<String>,
    pub updated_at: String,
}

/// Search user memory
pub async fn search_memory(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Json(request): Json<MemorySearchRequest>,
) -> Result<Json<MemorySearchResult>, ApiError> {
    let memory = &state.unified_memory;

    let results = memory.search(user_id, &request.query, request.limit).await;

    let items: Vec<MemorySearchItem> = results
        .iter()
        .map(|entry| MemorySearchItem {
            id: entry.id,
            key: entry.key.clone(),
            category: format!("{:?}", entry.category),
            title: entry.title.clone(),
            content_preview: entry.content.chars().take(300).collect(),
            tags: entry.tags.clone(),
            updated_at: entry.updated_at.to_rfc3339(),
        })
        .collect();

    let count = items.len();
    Ok(Json(MemorySearchResult {
        items,
        total_count: count,
    }))
}

/// Semantic search request (A38)
#[derive(Debug, Deserialize)]
pub struct SemanticSearchRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Semantic search memory entries using vector similarity (A38).
///
/// When a backend with embeddings is configured, uses vector similarity.
/// Otherwise falls back to keyword-based search.
pub async fn semantic_search_memory(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Json(request): Json<SemanticSearchRequest>,
) -> Result<Json<MemorySearchResult>, ApiError> {
    let memory = &state.unified_memory;

    let results = memory
        .semantic_search(user_id, &request.query, request.limit)
        .await;

    let items: Vec<MemorySearchItem> = results
        .iter()
        .map(|entry| MemorySearchItem {
            id: entry.id,
            key: entry.key.clone(),
            category: format!("{:?}", entry.category),
            title: entry.title.clone(),
            content_preview: entry.content.chars().take(300).collect(),
            tags: entry.tags.clone(),
            updated_at: entry.updated_at.to_rfc3339(),
        })
        .collect();

    let count = items.len();
    Ok(Json(MemorySearchResult {
        items,
        total_count: count,
    }))
}

/// User preferences response
#[derive(Debug, Serialize)]
pub struct PreferencesResponse {
    pub user_id: Uuid,
    pub language: Option<String>,
    pub timezone: Option<String>,
    pub default_model: Option<String>,
    pub communication_style: Option<String>,
}

/// Stats response
#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub total_entries: usize,
    pub by_category: std::collections::HashMap<String, usize>,
    pub total_size_bytes: usize,
}

/// Get user preferences
pub async fn get_preferences(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<PreferencesResponse>, ApiError> {
    let memory = &state.unified_memory;

    let prefs = memory.get_preferences(user_id).await;

    Ok(Json(PreferencesResponse {
        user_id,
        language: prefs.language,
        timezone: prefs.timezone,
        default_model: prefs.default_model,
        communication_style: prefs.communication_style,
    }))
}

/// Update preferences request
#[derive(Debug, Deserialize)]
pub struct UpdatePreferencesRequest {
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub communication_style: Option<String>,
}

/// Update user preferences
pub async fn update_preferences(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Json(request): Json<UpdatePreferencesRequest>,
) -> Result<Json<PreferencesResponse>, ApiError> {
    let memory = &state.unified_memory;

    // Get current preferences
    let mut prefs = memory.get_preferences(user_id).await;

    // Update fields if provided
    if let Some(lang) = request.language {
        prefs.language = Some(lang);
    }
    if let Some(tz) = request.timezone {
        prefs.timezone = Some(tz);
    }
    if let Some(model) = request.default_model {
        prefs.default_model = Some(model);
    }
    if let Some(style) = request.communication_style {
        prefs.communication_style = Some(style);
    }

    // Save updated preferences
    memory
        .update_preferences(user_id, prefs.clone())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(user_id = %user_id, "User preferences updated");

    Ok(Json(PreferencesResponse {
        user_id,
        language: prefs.language,
        timezone: prefs.timezone,
        default_model: prefs.default_model,
        communication_style: prefs.communication_style,
    }))
}

// ============================================
// Entry CRUD Operations
// ============================================

/// List entries query parameters
#[derive(Debug, Deserialize)]
pub struct ListEntriesQuery {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// List entries response
#[derive(Debug, Serialize)]
pub struct ListEntriesResponse {
    pub entries: Vec<EntryInfo>,
    pub total: usize,
}

/// List memory entries for a user
pub async fn list_entries(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Query(query): Query<ListEntriesQuery>,
) -> Result<Json<ListEntriesResponse>, ApiError> {
    let memory = &state.unified_memory;

    let entries = if let Some(category_str) = query.category {
        // Parse category
        let category = parse_category(&category_str)?;
        memory.list_by_category(user_id, category).await
    } else {
        memory.list(user_id).await
    };

    let total = entries.len();
    let entries: Vec<EntryInfo> = entries
        .into_iter()
        .take(query.limit)
        .map(|e| EntryInfo {
            id: e.id,
            key: e.key,
            category: format!("{:?}", e.category),
            title: e.title,
            content_preview: e.content.chars().take(200).collect(),
            updated_at: e.updated_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(ListEntriesResponse { entries, total }))
}

/// Create entry request
#[derive(Debug, Deserialize)]
pub struct CreateEntryRequest {
    pub key: String,
    pub category: String,
    pub content: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub structured_data: Option<Value>,
}

/// Full entry response
#[derive(Debug, Serialize)]
pub struct EntryResponse {
    pub id: Uuid,
    pub key: String,
    pub category: String,
    pub title: Option<String>,
    pub content: String,
    pub tags: Vec<String>,
    pub confidence: String,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Create a new memory entry
pub async fn create_entry(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Json(request): Json<CreateEntryRequest>,
) -> Result<Json<EntryResponse>, ApiError> {
    let memory = &state.unified_memory;

    let category = parse_category(&request.category)?;

    let mut entry = MemoryEntry::new(&request.key, category, &request.content)
        .with_source(MemorySource::User)
        .with_confidence(Confidence::High)
        .with_tags(request.tags);

    if let Some(title) = request.title {
        entry = entry.with_title(title);
    }
    if let Some(data) = request.structured_data {
        entry = entry.with_data(data);
    }

    memory
        .store(user_id, entry.clone())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(
        user_id = %user_id,
        key = %entry.key,
        category = ?category,
        "Memory entry created"
    );

    Ok(Json(entry_to_response(entry)))
}

/// Get a specific memory entry
pub async fn get_entry(
    State(state): State<AppState>,
    Path((user_id, key)): Path<(Uuid, String)>,
) -> Result<Json<EntryResponse>, ApiError> {
    let memory = &state.unified_memory;

    let entry = memory
        .get(user_id, &key)
        .await
        .ok_or_else(|| ApiError::not_found(format!("Entry '{}' not found", key)))?;

    Ok(Json(entry_to_response(entry)))
}

/// Update entry request
#[derive(Debug, Deserialize)]
pub struct UpdateEntryRequest {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub structured_data: Option<Value>,
}

/// Update a memory entry
pub async fn update_entry(
    State(state): State<AppState>,
    Path((user_id, key)): Path<(Uuid, String)>,
    Json(request): Json<UpdateEntryRequest>,
) -> Result<Json<EntryResponse>, ApiError> {
    let memory = &state.unified_memory;

    // Get existing entry
    let mut entry = memory
        .get(user_id, &key)
        .await
        .ok_or_else(|| ApiError::not_found(format!("Entry '{}' not found", key)))?;

    // Update fields
    if let Some(content) = request.content {
        entry.update_content(content);
    }
    if let Some(title) = request.title {
        entry.title = Some(title);
    }
    if let Some(tags) = request.tags {
        entry.tags = tags;
    }
    if let Some(data) = request.structured_data {
        entry.structured_data = Some(data);
    }

    // Save updated entry
    memory
        .store(user_id, entry.clone())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(user_id = %user_id, key = %key, "Memory entry updated");

    Ok(Json(entry_to_response(entry)))
}

/// Delete a memory entry
pub async fn delete_entry(
    State(state): State<AppState>,
    Path((user_id, key)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let memory = &state.unified_memory;

    let deleted = memory.delete(user_id, &key).await;

    if deleted.is_some() {
        tracing::info!(user_id = %user_id, key = %key, "Memory entry deleted");
        Ok(Json(serde_json::json!({
            "success": true,
            "message": format!("Entry '{}' deleted", key)
        })))
    } else {
        Err(ApiError::not_found(format!("Entry '{}' not found", key)))
    }
}

/// Get memory stats for a user
pub async fn get_stats(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<StatsResponse>, ApiError> {
    let memory = &state.unified_memory;
    let stats = memory.get_stats(user_id).await;

    Ok(Json(StatsResponse {
        total_entries: stats.total_entries,
        by_category: stats.by_category,
        total_size_bytes: stats.total_size_bytes,
    }))
}

// ============================================
// Helper Functions
// ============================================

fn parse_category(s: &str) -> Result<MemoryCategory, ApiError> {
    match s.to_lowercase().as_str() {
        "preference" | "preferences" => Ok(MemoryCategory::Preference),
        "pattern" | "patterns" => Ok(MemoryCategory::Pattern),
        "project" | "projects" => Ok(MemoryCategory::Project),
        "task" | "tasks" => Ok(MemoryCategory::Task),
        "conversation" | "conversations" => Ok(MemoryCategory::Conversation),
        "knowledge" => Ok(MemoryCategory::Knowledge),
        "tool_result" | "toolresult" => Ok(MemoryCategory::ToolResult),
        "working" => Ok(MemoryCategory::Working),
        "custom" => Ok(MemoryCategory::Custom),
        _ => Err(ApiError::bad_request(format!("Invalid category: {}", s))),
    }
}

fn entry_to_response(entry: MemoryEntry) -> EntryResponse {
    EntryResponse {
        id: entry.id,
        key: entry.key,
        category: format!("{:?}", entry.category),
        title: entry.title,
        content: entry.content,
        tags: entry.tags,
        confidence: format!("{:?}", entry.confidence),
        source: format!("{:?}", entry.source),
        created_at: entry.created_at.to_rfc3339(),
        updated_at: entry.updated_at.to_rfc3339(),
    }
}
