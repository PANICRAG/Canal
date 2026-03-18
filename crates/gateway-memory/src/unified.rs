//! Unified Memory System
//!
//! A single source of truth for all memory operations across:
//! - LLM interactions (Memory Tool)
//! - Frontend API (/api/memory)
//! - Agent context (task state, working memory)
//!
//! ## Design Principles
//! 1. Single storage layer, multiple views
//! 2. Everything is a MemoryEntry with rich metadata
//! 3. File-based view for LLMs, structured API for frontend
//! 4. Full-text and vector search support
//! 5. Cross-model compatibility (Claude, Qwen, GPT, etc.)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::Result;

// ============================================
// Core Types
// ============================================

/// Memory entry category
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    /// User preferences and settings
    Preference,
    /// Learned patterns and behaviors
    Pattern,
    /// Project context and information
    Project,
    /// Task progress and state
    Task,
    /// Conversation history/summary
    Conversation,
    /// Knowledge and facts
    Knowledge,
    /// Tool execution results
    ToolResult,
    /// Working memory (temporary, session-scoped)
    Working,
    /// Custom category
    Custom,
    /// Custom instructions for prompt generation
    CustomInstruction,
    /// Few-shot examples for prompt generation
    FewShotExample,
    /// Tool preferences (preferred/blocked tools)
    ToolPreference,
}

impl MemoryCategory {
    /// Get the file path prefix for this category
    pub fn path_prefix(&self) -> &'static str {
        match self {
            Self::Preference => "/memories/preferences",
            Self::Pattern => "/memories/patterns",
            Self::Project => "/memories/projects",
            Self::Task => "/memories/tasks",
            Self::Conversation => "/memories/conversations",
            Self::Knowledge => "/memories/knowledge",
            Self::ToolResult => "/memories/tool_results",
            Self::Working => "/memories/working",
            Self::Custom => "/memories/custom",
            Self::CustomInstruction => "/memories/custom_instructions",
            Self::FewShotExample => "/memories/few_shot_examples",
            Self::ToolPreference => "/memories/tool_preferences",
        }
    }

    /// Infer category from file path
    pub fn from_path(path: &str) -> Self {
        if path.contains("/custom_instructions") {
            Self::CustomInstruction
        } else if path.contains("/few_shot_examples") {
            Self::FewShotExample
        } else if path.contains("/tool_preferences") {
            Self::ToolPreference
        } else if path.contains("/preferences") {
            Self::Preference
        } else if path.contains("/patterns") {
            Self::Pattern
        } else if path.contains("/projects") {
            Self::Project
        } else if path.contains("/tasks") {
            Self::Task
        } else if path.contains("/conversations") {
            Self::Conversation
        } else if path.contains("/knowledge") {
            Self::Knowledge
        } else if path.contains("/tool_results") {
            Self::ToolResult
        } else if path.contains("/working") {
            Self::Working
        } else {
            Self::Custom
        }
    }
}

/// Confidence level for memory entries
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Low confidence, may need verification
    Low = 1,
    /// Medium confidence
    Medium = 2,
    /// High confidence
    High = 3,
    /// Explicitly confirmed by user
    Confirmed = 4,
}

impl Default for Confidence {
    fn default() -> Self {
        Self::Medium
    }
}

/// Source of the memory entry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    /// User explicitly stated
    User,
    /// Inferred from conversation
    Inferred,
    /// System generated
    System,
    /// Imported from external source
    Imported,
    /// LLM created via Memory Tool
    LlmCreated,
}

impl Default for MemorySource {
    fn default() -> Self {
        Self::System
    }
}

/// A single memory entry - the atomic unit of storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier
    pub id: Uuid,
    /// Human-readable key (also used as filename)
    pub key: String,
    /// Category for organization
    pub category: MemoryCategory,
    /// Title or summary
    pub title: Option<String>,
    /// Main content (text)
    pub content: String,
    /// Structured data (JSON)
    pub structured_data: Option<Value>,
    /// Tags for filtering
    pub tags: Vec<String>,
    /// Confidence level
    pub confidence: Confidence,
    /// Source of this entry
    pub source: MemorySource,
    /// Additional metadata
    pub metadata: HashMap<String, Value>,
    /// Session ID if session-scoped
    pub session_id: Option<Uuid>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
    /// Access count for LRU
    pub access_count: u64,
    /// Last accessed timestamp
    pub last_accessed: DateTime<Utc>,
    /// Version for optimistic locking
    pub version: u64,
}

impl MemoryEntry {
    /// Create a new memory entry
    pub fn new(
        key: impl Into<String>,
        category: MemoryCategory,
        content: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            key: key.into(),
            category,
            title: None,
            content: content.into(),
            structured_data: None,
            tags: Vec::new(),
            confidence: Confidence::default(),
            source: MemorySource::default(),
            metadata: HashMap::new(),
            session_id: None,
            created_at: now,
            updated_at: now,
            access_count: 0,
            last_accessed: now,
            version: 1,
        }
    }

    /// Builder: set title
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Builder: set structured data
    pub fn with_data(mut self, data: Value) -> Self {
        self.structured_data = Some(data);
        self
    }

    /// Builder: set tags
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Builder: set confidence
    pub fn with_confidence(mut self, confidence: Confidence) -> Self {
        self.confidence = confidence;
        self
    }

    /// Builder: set source
    pub fn with_source(mut self, source: MemorySource) -> Self {
        self.source = source;
        self
    }

    /// Builder: set session
    pub fn with_session(mut self, session_id: Uuid) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Get the virtual file path for this entry
    pub fn file_path(&self) -> String {
        format!("{}/{}.json", self.category.path_prefix(), self.key)
    }

    /// Convert to JSON format for LLM consumption
    ///
    /// Returns a pretty-printed JSON string representation of this entry.
    /// This is the preferred format for new code.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Parse a `MemoryEntry` from a JSON string.
    ///
    /// Returns `None` if the JSON is invalid or does not match the expected schema.
    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    /// Convert to XML format for LLM consumption
    ///
    /// # Deprecated
    ///
    /// This method is deprecated. Use [`to_json()`](Self::to_json) instead.
    /// XML format is kept only for backward compatibility and will be removed
    /// in a future version.
    #[deprecated(
        since = "0.2.0",
        note = "Use to_json() instead. XML format is kept for backward compatibility only."
    )]
    pub fn to_xml(&self) -> String {
        let mut xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<memory id="{}" category="{:?}" confidence="{:?}">
  <key>{}</key>
"#,
            self.id, self.category, self.confidence, self.key
        );

        if let Some(ref title) = self.title {
            xml.push_str(&format!("  <title>{}</title>\n", escape_xml(title)));
        }

        xml.push_str(&format!(
            "  <content>\n{}\n  </content>\n",
            escape_xml(&self.content)
        ));

        if let Some(ref data) = self.structured_data {
            xml.push_str(&format!(
                "  <structured_data>\n    {}\n  </structured_data>\n",
                serde_json::to_string_pretty(data).unwrap_or_default()
            ));
        }

        if !self.tags.is_empty() {
            xml.push_str("  <tags>\n");
            for tag in &self.tags {
                xml.push_str(&format!("    <tag>{}</tag>\n", escape_xml(tag)));
            }
            xml.push_str("  </tags>\n");
        }

        xml.push_str(&format!(
            "  <metadata>\n    <created>{}</created>\n    <updated>{}</updated>\n    <source>{:?}</source>\n  </metadata>\n",
            self.created_at.to_rfc3339(),
            self.updated_at.to_rfc3339(),
            self.source
        ));

        xml.push_str("</memory>\n");
        xml
    }

    /// Parse from XML content (best effort)
    pub fn from_xml(key: &str, category: MemoryCategory, xml: &str) -> Self {
        // Simple extraction - in production use proper XML parser
        let content = extract_xml_content(xml, "content").unwrap_or_else(|| xml.to_string());
        let title = extract_xml_content(xml, "title");

        let mut entry = Self::new(key, category, content);
        entry.title = title;
        entry.source = MemorySource::LlmCreated;
        entry
    }

    /// Update content and bump version
    pub fn update_content(&mut self, content: String) {
        self.content = content;
        self.updated_at = Utc::now();
        self.version += 1;
    }

    /// Record an access
    pub fn record_access(&mut self) {
        self.access_count += 1;
        self.last_accessed = Utc::now();
    }
}

// ============================================
// User Memory Context
// ============================================

/// User's complete memory context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMemoryContext {
    pub user_id: Uuid,
    pub preferences: MemoryPreferences,
    pub patterns: Vec<MemoryPattern>,
    pub recent_entries: Vec<MemoryEntry>,
    pub stats: MemoryStats,
}

/// User preferences stored in memory
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryPreferences {
    pub language: Option<String>,
    pub timezone: Option<String>,
    pub default_model: Option<String>,
    pub communication_style: Option<String>,
    pub custom: HashMap<String, Value>,
}

impl MemoryPreferences {
    /// Convert to XML
    pub fn to_xml(&self) -> String {
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<preferences>\n");

        if let Some(ref lang) = self.language {
            xml.push_str(&format!("  <language>{}</language>\n", lang));
        }
        if let Some(ref tz) = self.timezone {
            xml.push_str(&format!("  <timezone>{}</timezone>\n", tz));
        }
        if let Some(ref model) = self.default_model {
            xml.push_str(&format!("  <default_model>{}</default_model>\n", model));
        }
        if let Some(ref style) = self.communication_style {
            xml.push_str(&format!(
                "  <communication_style>{}</communication_style>\n",
                style
            ));
        }

        for (key, value) in &self.custom {
            xml.push_str(&format!(
                "  <custom key=\"{}\">{}</custom>\n",
                key,
                serde_json::to_string(value).unwrap_or_default()
            ));
        }

        xml.push_str("</preferences>\n");
        xml
    }

    /// Parse from XML
    pub fn from_xml(xml: &str) -> Self {
        Self {
            language: extract_xml_content(xml, "language"),
            timezone: extract_xml_content(xml, "timezone"),
            default_model: extract_xml_content(xml, "default_model"),
            communication_style: extract_xml_content(xml, "communication_style"),
            custom: HashMap::new(),
        }
    }
}

/// A learned pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPattern {
    pub id: Uuid,
    pub pattern_type: PatternType,
    pub description: String,
    pub confidence: f32,
    pub examples: Vec<String>,
    pub occurrence_count: u32,
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternType {
    Workflow,
    Naming,
    Timing,
    Style,
    ToolUsage,
    Organization,
    Communication,
    Custom,
}

/// Memory statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryStats {
    pub total_entries: usize,
    pub by_category: HashMap<String, usize>,
    pub total_size_bytes: usize,
    pub last_cleanup: Option<DateTime<Utc>>,
}

// ============================================
// Unified Memory Store
// ============================================

/// The unified memory store - single source of truth
pub struct UnifiedMemoryStore {
    /// All memory entries by user
    entries: Arc<RwLock<HashMap<Uuid, HashMap<String, MemoryEntry>>>>,
    /// User preferences cache
    preferences: Arc<RwLock<HashMap<Uuid, MemoryPreferences>>>,
    /// User patterns cache
    patterns: Arc<RwLock<HashMap<Uuid, Vec<MemoryPattern>>>>,
    /// Configuration
    config: MemoryConfig,
    /// Optional persistent backend (e.g., pgvector via Session Module).
    /// Uses OnceLock for set-once interior mutability: the backend can be
    /// attached after construction (e.g., when Session Module initializes).
    backend: OnceLock<Arc<dyn super::persistence::MemoryBackend>>,
    /// Optional embedding provider for semantic search
    embedding_provider: Option<Arc<dyn crate::cache::EmbeddingProvider>>,
}

/// Memory store configuration
#[derive(Debug, Clone)]
pub struct MemoryConfig {
    /// Maximum entries per user
    pub max_entries_per_user: usize,
    /// Maximum content size in bytes
    pub max_content_size: usize,
    /// Maximum working memory entries (auto-cleanup)
    pub max_working_entries: usize,
    /// Working memory TTL in seconds
    pub working_memory_ttl_secs: u64,
    /// Enable persistence
    pub persistence_enabled: bool,
    /// Persistence path
    pub persistence_path: Option<String>,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_entries_per_user: 10000,
            max_content_size: 1024 * 1024, // 1MB
            max_working_entries: 100,
            working_memory_ttl_secs: 3600, // 1 hour
            persistence_enabled: false,
            persistence_path: None,
        }
    }
}

impl UnifiedMemoryStore {
    /// Create a new unified memory store
    pub fn new() -> Self {
        Self::with_config(MemoryConfig::default())
    }

    /// Create with custom config
    pub fn with_config(config: MemoryConfig) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            preferences: Arc::new(RwLock::new(HashMap::new())),
            patterns: Arc::new(RwLock::new(HashMap::new())),
            config,
            backend: OnceLock::new(),
            embedding_provider: None,
        }
    }

    /// Create with a persistent backend and embedding provider.
    ///
    /// When a backend is present, `store()` writes through to both in-memory
    /// and persistent storage, and `semantic_search()` uses vector search
    /// via the backend instead of keyword fallback.
    pub fn with_backend(
        config: MemoryConfig,
        backend: Arc<dyn super::persistence::MemoryBackend>,
        embedding_provider: Arc<dyn crate::cache::EmbeddingProvider>,
    ) -> Self {
        let store = Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            preferences: Arc::new(RwLock::new(HashMap::new())),
            patterns: Arc::new(RwLock::new(HashMap::new())),
            config,
            backend: OnceLock::new(),
            embedding_provider: Some(embedding_provider),
        };
        let _ = store.backend.set(backend);
        store
    }

    /// Create with a persistent backend only (no embedding provider).
    ///
    /// Write-through to backend is enabled, but semantic search falls back
    /// to keyword search since no embedding provider is available.
    pub fn with_backend_only(
        config: MemoryConfig,
        backend: Arc<dyn super::persistence::MemoryBackend>,
    ) -> Self {
        let store = Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            preferences: Arc::new(RwLock::new(HashMap::new())),
            patterns: Arc::new(RwLock::new(HashMap::new())),
            config,
            backend: OnceLock::new(),
            embedding_provider: None,
        };
        let _ = store.backend.set(backend);
        store
    }

    /// Returns `true` if a persistent backend is configured.
    pub fn has_backend(&self) -> bool {
        self.backend.get().is_some()
    }

    /// Get a reference to the backend, if configured.
    pub fn backend(&self) -> Option<&Arc<dyn super::persistence::MemoryBackend>> {
        self.backend.get()
    }

    /// Set the persistent backend after construction (set-once).
    ///
    /// Returns `Ok(())` if the backend was set, or `Err(backend)` if one was
    /// already configured. This allows the Session Module to attach a
    /// pgvector backend to a store that was created by the Engine Module.
    pub fn set_backend(
        &self,
        backend: Arc<dyn super::persistence::MemoryBackend>,
    ) -> std::result::Result<(), Arc<dyn super::persistence::MemoryBackend>> {
        self.backend.set(backend)
    }

    /// Get a reference to the embedding provider, if configured.
    pub fn embedding_provider(&self) -> Option<&Arc<dyn crate::cache::EmbeddingProvider>> {
        self.embedding_provider.as_ref()
    }

    /// Get a reference to the internal entries RwLock for synchronous access.
    ///
    /// This is used by `ContextIntegration` to read entries without blocking
    /// via `try_read()`. Prefer the async methods for normal usage.
    pub fn entries_lock(
        &self,
    ) -> &tokio::sync::RwLock<HashMap<Uuid, HashMap<String, MemoryEntry>>> {
        &self.entries
    }

    // ============================================
    // Entry Operations
    // ============================================

    /// Store a memory entry.
    ///
    /// When a persistent backend is configured, this performs a write-through:
    /// the entry is stored in both the in-memory cache and the backend.
    /// Backend failures are logged but do not fail the operation.
    pub async fn store(&self, user_id: Uuid, entry: MemoryEntry) -> Result<()> {
        // Write-through to backend (fire-and-forget on failure)
        if let Some(backend) = self.backend.get() {
            if let Err(e) = backend.store(user_id, &entry).await {
                tracing::warn!(
                    user_id = %user_id,
                    key = %entry.key,
                    error = %e,
                    "memory backend write-through failed, continuing with in-memory only"
                );
            }
        }

        let mut entries = self.entries.write().await;
        let user_entries = entries.entry(user_id).or_insert_with(HashMap::new);

        // Check limits
        if user_entries.len() >= self.config.max_entries_per_user {
            // Remove oldest working memory entries first
            self.cleanup_working_memory(user_entries).await;
        }

        user_entries.insert(entry.key.clone(), entry);
        Ok(())
    }

    /// Get a memory entry by key
    pub async fn get(&self, user_id: Uuid, key: &str) -> Option<MemoryEntry> {
        let mut entries = self.entries.write().await;
        if let Some(user_entries) = entries.get_mut(&user_id) {
            if let Some(entry) = user_entries.get_mut(key) {
                entry.record_access();
                return Some(entry.clone());
            }
        }
        None
    }

    /// Get entry by file path
    pub async fn get_by_path(&self, user_id: Uuid, path: &str) -> Option<MemoryEntry> {
        let key = self.path_to_key(path);
        self.get(user_id, &key).await
    }

    /// Delete a memory entry
    pub async fn delete(&self, user_id: Uuid, key: &str) -> Option<MemoryEntry> {
        let mut entries = self.entries.write().await;
        if let Some(user_entries) = entries.get_mut(&user_id) {
            return user_entries.remove(key);
        }
        None
    }

    /// List all entries for a user
    pub async fn list(&self, user_id: Uuid) -> Vec<MemoryEntry> {
        let entries = self.entries.read().await;
        entries
            .get(&user_id)
            .map(|e| e.values().cloned().collect())
            .unwrap_or_default()
    }

    /// List entries by category
    pub async fn list_by_category(
        &self,
        user_id: Uuid,
        category: MemoryCategory,
    ) -> Vec<MemoryEntry> {
        let entries = self.entries.read().await;
        entries
            .get(&user_id)
            .map(|e| {
                e.values()
                    .filter(|entry| entry.category == category)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Search entries
    pub async fn search(&self, user_id: Uuid, query: &str, limit: usize) -> Vec<MemoryEntry> {
        let entries = self.entries.read().await;
        let query_lower = query.to_lowercase();

        let mut results: Vec<(f32, MemoryEntry)> = entries
            .get(&user_id)
            .map(|e| {
                e.values()
                    .filter_map(|entry| {
                        let score = self.calculate_relevance(entry, &query_lower);
                        if score > 0.0 {
                            Some((score, entry.clone()))
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Sort by relevance
        results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Return top results
        results.into_iter().take(limit).map(|(_, e)| e).collect()
    }

    /// Search entries and return results as a JSON string.
    ///
    /// This is a convenience wrapper around [`search()`](Self::search) that
    /// serializes each matching entry to JSON format instead of returning
    /// Rust structs. Useful for API endpoints that need a JSON response.
    pub async fn search_json(&self, user_id: Uuid, query: &str, limit: usize) -> String {
        let results = self.search(user_id, query, limit).await;
        let json_entries: Vec<serde_json::Value> = results
            .iter()
            .map(|entry| serde_json::to_value(entry).unwrap_or_default())
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "query": query,
            "count": json_entries.len(),
            "results": json_entries,
        }))
        .unwrap_or_default()
    }

    /// Perform semantic search if a backend and embedding provider are available.
    ///
    /// Falls back to keyword-based `search()` when no backend is configured.
    /// Returns entries sorted by relevance (semantic similarity or keyword score).
    pub async fn semantic_search(
        &self,
        user_id: Uuid,
        query: &str,
        limit: usize,
    ) -> Vec<MemoryEntry> {
        {
            // Try semantic path if backend + embedding provider are available
            if let (Some(backend), Some(ref embedder)) =
                (self.backend.get(), &self.embedding_provider)
            {
                match embedder.embed(query).await {
                    Ok(query_embedding) => {
                        match backend
                            .semantic_search(user_id, &query_embedding, limit, 0.3)
                            .await
                        {
                            Ok(results) if !results.is_empty() => {
                                return results.into_iter().map(|(_, entry)| entry).collect();
                            }
                            Ok(_) => {
                                // Empty results — fall through to keyword
                                tracing::debug!(
                                    "semantic search returned no results, falling back to keyword"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "semantic search failed, falling back to keyword search"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "embedding generation failed, falling back to keyword search"
                        );
                    }
                }
            }
        }

        // Fallback: keyword search
        self.search(user_id, query, limit).await
    }

    fn calculate_relevance(&self, entry: &MemoryEntry, query: &str) -> f32 {
        let mut score = 0.0;

        // Title match (highest weight)
        if let Some(ref title) = entry.title {
            if title.to_lowercase().contains(query) {
                score += 3.0;
            }
        }

        // Key match
        if entry.key.to_lowercase().contains(query) {
            score += 2.0;
        }

        // Content match
        if entry.content.to_lowercase().contains(query) {
            score += 1.0;
        }

        // Tag match
        for tag in &entry.tags {
            if tag.to_lowercase().contains(query) {
                score += 1.5;
            }
        }

        // Recency boost
        let age_hours = (Utc::now() - entry.updated_at).num_hours() as f32;
        let recency_boost = 1.0 / (1.0 + age_hours / 24.0);
        score *= 1.0 + recency_boost * 0.5;

        // Confidence boost
        score *= match entry.confidence {
            Confidence::Confirmed => 1.5,
            Confidence::High => 1.3,
            Confidence::Medium => 1.0,
            Confidence::Low => 0.8,
        };

        score
    }

    // ============================================
    // Preferences Operations
    // ============================================

    /// Get user preferences
    pub async fn get_preferences(&self, user_id: Uuid) -> MemoryPreferences {
        let prefs = self.preferences.read().await;
        prefs.get(&user_id).cloned().unwrap_or_default()
    }

    /// Update user preferences
    pub async fn update_preferences(&self, user_id: Uuid, prefs: MemoryPreferences) -> Result<()> {
        let mut preferences = self.preferences.write().await;
        preferences.insert(user_id, prefs.clone());

        // Also store as entry for file-based access
        let entry = MemoryEntry::new("preferences", MemoryCategory::Preference, prefs.to_xml())
            .with_title("User Preferences")
            .with_source(MemorySource::System)
            .with_confidence(Confidence::Confirmed);

        drop(preferences);
        self.store(user_id, entry).await?;

        Ok(())
    }

    // ============================================
    // Pattern Operations
    // ============================================

    /// Get user patterns
    pub async fn get_patterns(&self, user_id: Uuid) -> Vec<MemoryPattern> {
        let patterns = self.patterns.read().await;
        patterns.get(&user_id).cloned().unwrap_or_default()
    }

    /// Add or update a pattern
    pub async fn record_pattern(&self, user_id: Uuid, pattern: MemoryPattern) -> Result<()> {
        let mut patterns = self.patterns.write().await;
        let user_patterns = patterns.entry(user_id).or_insert_with(Vec::new);

        // Check if pattern exists
        if let Some(existing) = user_patterns.iter_mut().find(|p| p.id == pattern.id) {
            existing.occurrence_count += 1;
            existing.confidence = (existing.confidence + pattern.confidence) / 2.0;
            existing.last_seen = Utc::now();
        } else {
            user_patterns.push(pattern);
        }

        Ok(())
    }

    // ============================================
    // File System View (for LLM)
    // ============================================

    /// List directory contents (LLM view)
    pub async fn list_directory(&self, user_id: Uuid, path: &str) -> String {
        let entries = self.entries.read().await;

        let mut listing = format!("Directory listing: {}\n\n", path);

        if let Some(user_entries) = entries.get(&user_id) {
            let matching: Vec<_> = user_entries
                .values()
                .filter(|e| e.file_path().starts_with(path) || path == "/memories")
                .collect();

            if matching.is_empty() {
                listing.push_str("(empty)\n");
            } else {
                for entry in matching {
                    let size = entry.content.len();
                    listing.push_str(&format!(
                        "{:>6}  {}  {}\n",
                        format_size(size),
                        entry.updated_at.format("%Y-%m-%d %H:%M"),
                        entry.file_path()
                    ));
                }
            }
        } else {
            listing.push_str("(no entries)\n");
        }

        // Add category summary
        listing.push_str("\nCategories:\n");
        for cat in &[
            MemoryCategory::Preference,
            MemoryCategory::Pattern,
            MemoryCategory::Project,
            MemoryCategory::Task,
            MemoryCategory::Knowledge,
        ] {
            listing.push_str(&format!("  {}\n", cat.path_prefix()));
        }

        listing
    }

    /// Read file contents (LLM view)
    ///
    /// Returns the memory entry as a JSON string.
    pub async fn read_file(&self, user_id: Uuid, path: &str) -> Option<String> {
        let entry = self.get_by_path(user_id, path).await?;
        Some(entry.to_json())
    }

    /// Write file (from LLM)
    pub async fn write_file(&self, user_id: Uuid, path: &str, content: &str) -> Result<()> {
        let key = self.path_to_key(path);
        let category = MemoryCategory::from_path(path);

        // Check if it's a preferences update
        if category == MemoryCategory::Preference && key == "preferences" {
            let prefs = MemoryPreferences::from_xml(content);
            return self.update_preferences(user_id, prefs).await;
        }

        let entry = MemoryEntry::from_xml(&key, category, content);
        self.store(user_id, entry).await
    }

    /// Update file content (str_replace)
    pub async fn update_file(
        &self,
        user_id: Uuid,
        path: &str,
        old_str: &str,
        new_str: &str,
    ) -> Result<bool> {
        let key = self.path_to_key(path);

        let mut entries = self.entries.write().await;
        if let Some(user_entries) = entries.get_mut(&user_id) {
            if let Some(entry) = user_entries.get_mut(&key) {
                if entry.content.contains(old_str) {
                    entry.content = entry.content.replace(old_str, new_str);
                    entry.updated_at = Utc::now();
                    entry.version += 1;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Delete file
    pub async fn delete_file(&self, user_id: Uuid, path: &str) -> bool {
        let key = self.path_to_key(path);
        self.delete(user_id, &key).await.is_some()
    }

    /// Rename file
    pub async fn rename_file(&self, user_id: Uuid, old_path: &str, new_path: &str) -> Result<bool> {
        let old_key = self.path_to_key(old_path);
        let new_key = self.path_to_key(new_path);
        let new_category = MemoryCategory::from_path(new_path);

        let mut entries = self.entries.write().await;
        if let Some(user_entries) = entries.get_mut(&user_id) {
            if let Some(mut entry) = user_entries.remove(&old_key) {
                entry.key = new_key.clone();
                entry.category = new_category;
                entry.updated_at = Utc::now();
                user_entries.insert(new_key, entry);
                return Ok(true);
            }
        }
        Ok(false)
    }

    // ============================================
    // Context Operations
    // ============================================

    /// Get full user context
    pub async fn get_context(&self, user_id: Uuid) -> UserMemoryContext {
        let entries = self.list(user_id).await;
        let preferences = self.get_preferences(user_id).await;
        let patterns = self.get_patterns(user_id).await;

        // Calculate stats
        let mut by_category: HashMap<String, usize> = HashMap::new();
        let mut total_size = 0;

        for entry in &entries {
            let cat_name = format!("{:?}", entry.category);
            *by_category.entry(cat_name).or_insert(0) += 1;
            total_size += entry.content.len();
        }

        let stats = MemoryStats {
            total_entries: entries.len(),
            by_category,
            total_size_bytes: total_size,
            last_cleanup: None,
        };

        // Get recent entries
        let mut recent = entries;
        recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        recent.truncate(20);

        UserMemoryContext {
            user_id,
            preferences,
            patterns,
            recent_entries: recent,
            stats,
        }
    }

    // ============================================
    // Helper Methods
    // ============================================

    fn path_to_key(&self, path: &str) -> String {
        path.trim_start_matches("/memories/")
            .trim_start_matches("preferences/")
            .trim_start_matches("patterns/")
            .trim_start_matches("projects/")
            .trim_start_matches("tasks/")
            .trim_start_matches("knowledge/")
            .trim_start_matches("conversations/")
            .trim_start_matches("working/")
            .trim_start_matches("custom/")
            .trim_end_matches(".json")
            .trim_end_matches(".xml")
            .trim_end_matches(".txt")
            .trim_end_matches(".md")
            .replace('/', "_")
    }

    async fn cleanup_working_memory(&self, entries: &mut HashMap<String, MemoryEntry>) {
        let now = Utc::now();
        let ttl = chrono::Duration::seconds(self.config.working_memory_ttl_secs as i64);

        // Remove expired working memory entries
        entries.retain(|_, entry| {
            if entry.category == MemoryCategory::Working {
                now - entry.updated_at < ttl
            } else {
                true
            }
        });

        // If still over limit, remove oldest by access
        if entries.len() >= self.config.max_entries_per_user {
            let mut items: Vec<_> = entries.iter().collect();
            items.sort_by(|a, b| a.1.last_accessed.cmp(&b.1.last_accessed));

            let to_remove: Vec<_> = items
                .iter()
                .take(entries.len() - self.config.max_entries_per_user + 100)
                .map(|(k, _)| k.to_string())
                .collect();

            for key in to_remove {
                entries.remove(&key);
            }
        }
    }

    /// Clear all entries for a user
    pub async fn clear(&self, user_id: Uuid) {
        let mut entries = self.entries.write().await;
        entries.remove(&user_id);

        let mut preferences = self.preferences.write().await;
        preferences.remove(&user_id);

        let mut patterns = self.patterns.write().await;
        patterns.remove(&user_id);
    }

    /// Get stats for a user
    pub async fn get_stats(&self, user_id: Uuid) -> MemoryStats {
        let entries = self.entries.read().await;

        if let Some(user_entries) = entries.get(&user_id) {
            let mut by_category: HashMap<String, usize> = HashMap::new();
            let mut total_size = 0;

            for entry in user_entries.values() {
                let cat_name = format!("{:?}", entry.category);
                *by_category.entry(cat_name).or_insert(0) += 1;
                total_size += entry.content.len();
            }

            MemoryStats {
                total_entries: user_entries.len(),
                by_category,
                total_size_bytes: total_size,
                last_cleanup: None,
            }
        } else {
            MemoryStats::default()
        }
    }

    // ============================================
    // Prompt Integration Methods
    // ============================================

    /// Get stored custom instructions for a user.
    pub async fn get_custom_instructions(&self, user_id: Uuid) -> Option<String> {
        let entries = self.entries.read().await;
        entries.get(&user_id).and_then(|user_entries| {
            user_entries
                .values()
                .find(|e| e.category == MemoryCategory::CustomInstruction)
                .map(|e| e.content.clone())
        })
    }

    /// Store custom instructions for a user.
    pub async fn set_custom_instructions(&self, user_id: Uuid, instructions: String) -> Result<()> {
        let entry = MemoryEntry::new(
            "custom_instructions",
            MemoryCategory::CustomInstruction,
            instructions,
        )
        .with_title("Custom Instructions")
        .with_source(MemorySource::User)
        .with_confidence(Confidence::Confirmed);
        self.store(user_id, entry).await
    }

    /// Get stored few-shot examples for a user.
    pub async fn get_few_shot_examples(&self, user_id: Uuid) -> Vec<MemoryEntry> {
        self.list_by_category(user_id, MemoryCategory::FewShotExample)
            .await
    }

    /// Get stored tool preferences for a user.
    pub async fn get_tool_preferences(&self, user_id: Uuid) -> Option<MemoryEntry> {
        let entries = self.entries.read().await;
        entries.get(&user_id).and_then(|user_entries| {
            user_entries
                .values()
                .find(|e| e.category == MemoryCategory::ToolPreference)
                .cloned()
        })
    }

    /// Build a prompt context string from stored preferences and learned patterns.
    ///
    /// This assembles a text block suitable for injection into the system prompt,
    /// combining user preferences, custom instructions, and optionally relevant
    /// learned patterns for the given task.
    pub async fn get_prompt_context(&self, user_id: Uuid, task_hint: Option<&str>) -> String {
        let mut sections = Vec::new();

        // 1. User preferences
        let prefs = self.get_preferences(user_id).await;
        let mut pref_lines = Vec::new();
        if let Some(ref lang) = prefs.language {
            pref_lines.push(format!("- Language: {}", lang));
        }
        if let Some(ref style) = prefs.communication_style {
            pref_lines.push(format!("- Communication style: {}", style));
        }
        if let Some(ref model) = prefs.default_model {
            pref_lines.push(format!("- Preferred model: {}", model));
        }
        if let Some(ref tz) = prefs.timezone {
            pref_lines.push(format!("- Timezone: {}", tz));
        }
        if !pref_lines.is_empty() {
            sections.push(format!("[User Preferences]\n{}", pref_lines.join("\n")));
        }

        // 2. Custom instructions
        if let Some(instructions) = self.get_custom_instructions(user_id).await {
            sections.push(format!("[Custom Instructions]\n{}", instructions));
        }

        // 3. Relevant learned patterns (if task hint provided)
        if let Some(hint) = task_hint {
            let patterns = self
                .search(user_id, hint, 5)
                .await
                .into_iter()
                .filter(|e| e.category == MemoryCategory::Pattern)
                .collect::<Vec<_>>();
            if !patterns.is_empty() {
                let pattern_text = patterns
                    .iter()
                    .map(|p| format!("- {}", p.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                sections.push(format!("[Learned Patterns]\n{}", pattern_text));
            }
        }

        sections.join("\n\n")
    }
}

impl Default for UnifiedMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================
// Utility Functions
// ============================================

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn extract_xml_content(xml: &str, tag: &str) -> Option<String> {
    let start_tag = format!("<{}>", tag);
    let end_tag = format!("</{}>", tag);

    let start = xml.find(&start_tag)? + start_tag.len();
    let end = xml.find(&end_tag)?;

    Some(xml[start..end].trim().to_string())
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    #[allow(deprecated)]
    use super::*;

    #[tokio::test]
    async fn test_store_and_get() {
        let store = UnifiedMemoryStore::new();
        let user_id = Uuid::new_v4();

        let entry = MemoryEntry::new("test_key", MemoryCategory::Knowledge, "Test content")
            .with_title("Test Entry");

        store.store(user_id, entry.clone()).await.unwrap();

        let retrieved = store.get(user_id, "test_key").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().content, "Test content");
    }

    #[tokio::test]
    async fn test_search() {
        let store = UnifiedMemoryStore::new();
        let user_id = Uuid::new_v4();

        store
            .store(
                user_id,
                MemoryEntry::new("key1", MemoryCategory::Knowledge, "Hello world"),
            )
            .await
            .unwrap();
        store
            .store(
                user_id,
                MemoryEntry::new("key2", MemoryCategory::Knowledge, "Goodbye world"),
            )
            .await
            .unwrap();
        store
            .store(
                user_id,
                MemoryEntry::new("key3", MemoryCategory::Knowledge, "Something else"),
            )
            .await
            .unwrap();

        let results = store.search(user_id, "world", 10).await;
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_file_operations() {
        let store = UnifiedMemoryStore::new();
        let user_id = Uuid::new_v4();

        // Write (both .xml and .json paths map to the same key)
        store
            .write_file(
                user_id,
                "/memories/knowledge/test.xml",
                "<content>Test</content>",
            )
            .await
            .unwrap();

        // Read via .xml path (backward compat)
        let content = store
            .read_file(user_id, "/memories/knowledge/test.xml")
            .await;
        assert!(content.is_some());
        assert!(content.unwrap().contains("Test"));

        // Read via .json path (new format)
        let content = store
            .read_file(user_id, "/memories/knowledge/test.json")
            .await;
        assert!(content.is_some());
        assert!(content.unwrap().contains("Test"));

        // Update
        store
            .write_file(
                user_id,
                "/memories/knowledge/test.xml",
                "<content>Updated</content>",
            )
            .await
            .unwrap();
        let content = store
            .read_file(user_id, "/memories/knowledge/test.xml")
            .await;
        assert!(content.unwrap().contains("Updated"));

        // Delete
        let deleted = store
            .delete_file(user_id, "/memories/knowledge/test.xml")
            .await;
        assert!(deleted);
    }

    #[test]
    fn test_category_from_path() {
        assert_eq!(
            MemoryCategory::from_path("/memories/preferences/user.xml"),
            MemoryCategory::Preference
        );
        assert_eq!(
            MemoryCategory::from_path("/memories/projects/my_project.xml"),
            MemoryCategory::Project
        );
        assert_eq!(
            MemoryCategory::from_path("/memories/unknown/file.xml"),
            MemoryCategory::Custom
        );
        // JSON paths also work
        assert_eq!(
            MemoryCategory::from_path("/memories/preferences/user.json"),
            MemoryCategory::Preference
        );
        assert_eq!(
            MemoryCategory::from_path("/memories/projects/my_project.json"),
            MemoryCategory::Project
        );
    }

    #[test]
    #[allow(deprecated)]
    fn test_entry_to_xml() {
        let entry = MemoryEntry::new("test", MemoryCategory::Knowledge, "Test content")
            .with_title("Test Title")
            .with_tags(vec!["tag1".to_string(), "tag2".to_string()]);

        let xml = entry.to_xml();
        assert!(xml.contains("<title>Test Title</title>"));
        assert!(xml.contains("<tag>tag1</tag>"));
        assert!(xml.contains("Test content"));
    }

    #[test]
    fn test_entry_to_json() {
        let entry = MemoryEntry::new("test", MemoryCategory::Knowledge, "Test content")
            .with_title("Test Title")
            .with_tags(vec!["tag1".to_string(), "tag2".to_string()]);

        let json_str = entry.to_json();

        // Verify it is valid JSON
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("should be valid JSON");
        assert_eq!(parsed["key"], "test");
        assert_eq!(parsed["category"], "knowledge");
        assert_eq!(parsed["content"], "Test content");
        assert_eq!(parsed["title"], "Test Title");
        assert_eq!(parsed["tags"][0], "tag1");
        assert_eq!(parsed["tags"][1], "tag2");
    }

    #[test]
    fn test_entry_from_json_roundtrip() {
        let entry = MemoryEntry::new("roundtrip_key", MemoryCategory::Project, "Project notes")
            .with_title("My Project")
            .with_tags(vec!["rust".to_string(), "gateway".to_string()])
            .with_confidence(Confidence::High)
            .with_source(MemorySource::User);

        let json_str = entry.to_json();
        let restored = MemoryEntry::from_json(&json_str).expect("should parse back from JSON");

        assert_eq!(restored.key, "roundtrip_key");
        assert_eq!(restored.category, MemoryCategory::Project);
        assert_eq!(restored.content, "Project notes");
        assert_eq!(restored.title.as_deref(), Some("My Project"));
        assert_eq!(
            restored.tags,
            vec!["rust".to_string(), "gateway".to_string()]
        );
        assert_eq!(restored.confidence, Confidence::High);
        assert_eq!(restored.source, MemorySource::User);
    }

    #[test]
    fn test_entry_from_json_invalid() {
        assert!(MemoryEntry::from_json("not valid json").is_none());
        assert!(MemoryEntry::from_json("{}").is_none());
        assert!(MemoryEntry::from_json("").is_none());
    }

    #[test]
    fn test_file_path_uses_json_extension() {
        let entry = MemoryEntry::new("my_notes", MemoryCategory::Knowledge, "Some notes");
        assert!(
            entry.file_path().ends_with(".json"),
            "file_path should use .json extension, got: {}",
            entry.file_path()
        );
        assert_eq!(entry.file_path(), "/memories/knowledge/my_notes.json");
    }

    #[tokio::test]
    async fn test_read_file_returns_json() {
        let store = UnifiedMemoryStore::new();
        let user_id = Uuid::new_v4();

        let entry = MemoryEntry::new("json_test", MemoryCategory::Knowledge, "JSON content test")
            .with_title("JSON Test");
        store.store(user_id, entry).await.unwrap();

        let content = store
            .read_file(user_id, "/memories/knowledge/json_test.json")
            .await;
        assert!(content.is_some());

        let json_str = content.unwrap();
        // Verify the content is valid JSON, not XML
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("read_file should return valid JSON");
        assert_eq!(parsed["key"], "json_test");
        assert_eq!(parsed["content"], "JSON content test");
        assert_eq!(parsed["title"], "JSON Test");
    }

    #[tokio::test]
    async fn test_search_json() {
        let store = UnifiedMemoryStore::new();
        let user_id = Uuid::new_v4();

        store
            .store(
                user_id,
                MemoryEntry::new("key1", MemoryCategory::Knowledge, "Hello world"),
            )
            .await
            .unwrap();
        store
            .store(
                user_id,
                MemoryEntry::new("key2", MemoryCategory::Knowledge, "Hello universe"),
            )
            .await
            .unwrap();
        store
            .store(
                user_id,
                MemoryEntry::new("key3", MemoryCategory::Knowledge, "Something else"),
            )
            .await
            .unwrap();

        let json_str = store.search_json(user_id, "Hello", 10).await;

        // Verify it is valid JSON with expected structure
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("search_json should return valid JSON");
        assert_eq!(parsed["query"], "Hello");
        assert_eq!(parsed["count"], 2);
        assert!(parsed["results"].is_array());
        assert_eq!(parsed["results"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_search_json_empty() {
        let store = UnifiedMemoryStore::new();
        let user_id = Uuid::new_v4();

        let json_str = store.search_json(user_id, "nonexistent", 10).await;

        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("should be valid JSON");
        assert_eq!(parsed["count"], 0);
        assert!(parsed["results"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_path_to_key_json_and_xml_equivalent() {
        let store = UnifiedMemoryStore::new();
        let user_id = Uuid::new_v4();

        // Store via .xml path
        store
            .write_file(user_id, "/memories/knowledge/compat.xml", "compat content")
            .await
            .unwrap();

        // Should be readable via .json path too (same underlying key)
        let content_via_json = store
            .read_file(user_id, "/memories/knowledge/compat.json")
            .await;
        assert!(
            content_via_json.is_some(),
            "Should read entry via .json path even if stored with .xml path"
        );

        // And via .xml path (backward compat)
        let content_via_xml = store
            .read_file(user_id, "/memories/knowledge/compat.xml")
            .await;
        assert!(content_via_xml.is_some(), "Should read entry via .xml path");
    }
}
