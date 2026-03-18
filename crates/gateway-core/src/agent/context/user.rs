//! User Context Loader
//!
//! Loads and manages user-level context including preferences, CLAUDE.md content,
//! and memory items from the database.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::resolver::{ContextLayer, ContextPriority, ResolvedContext};
use crate::error::{Error, Result};

// ============================================================================
// User Context Types
// ============================================================================

/// User-level context configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserContext {
    pub user_id: Uuid,
    pub preferences: UserPreferences,
    pub claude_md_content: Option<String>,
    pub memory_items: Vec<MemoryItem>,
}

/// User preferences for code generation and communication
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserPreferences {
    pub coding_style: Option<CodingStyle>,
    pub communication: Option<CommunicationPrefs>,
    #[serde(default)]
    pub custom: serde_json::Map<String, Value>,
}

/// Coding style preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingStyle {
    /// Whether to prefer explicit type annotations
    #[serde(default)]
    pub prefer_explicit_types: bool,

    /// Maximum line length preference
    pub max_line_length: Option<usize>,

    /// Whether to prefer functional programming patterns
    #[serde(default)]
    pub prefer_functional: bool,
}

impl Default for CodingStyle {
    fn default() -> Self {
        Self {
            prefer_explicit_types: false,
            max_line_length: None,
            prefer_functional: false,
        }
    }
}

/// Communication preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunicationPrefs {
    /// Verbosity level: "minimal", "normal", "detailed"
    #[serde(default = "default_verbosity")]
    pub verbosity: String,

    /// Whether to include explanations with code
    #[serde(default = "default_true")]
    pub include_explanations: bool,
}

fn default_verbosity() -> String {
    "normal".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for CommunicationPrefs {
    fn default() -> Self {
        Self {
            verbosity: default_verbosity(),
            include_explanations: true,
        }
    }
}

/// A memory item stored for user context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Category of the memory (e.g., "preference", "fact", "pattern")
    pub category: String,

    /// Key identifier for the memory
    pub key: String,

    /// The memory value
    pub value: Value,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,

    /// Source of the memory (e.g., "explicit", "inferred", "conversation")
    pub source: String,
}

impl Default for UserContext {
    fn default() -> Self {
        Self {
            user_id: Uuid::nil(),
            preferences: UserPreferences::default(),
            claude_md_content: None,
            memory_items: vec![],
        }
    }
}

// ============================================================================
// ContextLayer Implementation
// ============================================================================

impl ContextLayer for UserContext {
    fn layer_name(&self) -> &str {
        "user"
    }

    fn priority(&self) -> ContextPriority {
        ContextPriority::User
    }

    fn apply_to(&self, resolved: &mut ResolvedContext) {
        let mut prefs = String::new();
        prefs.push_str("## User Preferences\n\n");

        // Add coding style preferences
        if let Some(coding) = &self.preferences.coding_style {
            if coding.prefer_explicit_types {
                prefs.push_str("- Prefer explicit type annotations\n");
            }
            if let Some(max_len) = coding.max_line_length {
                prefs.push_str(&format!("- Max line length: {}\n", max_len));
            }
            if coding.prefer_functional {
                prefs.push_str("- Prefer functional programming patterns\n");
            }
        }

        // Add communication preferences
        if let Some(comm) = &self.preferences.communication {
            prefs.push_str(&format!("- Verbosity: {}\n", comm.verbosity));
            if !comm.include_explanations {
                prefs.push_str("- Skip explanations, show code only\n");
            }
        }

        // Add custom preferences
        if !self.preferences.custom.is_empty() {
            prefs.push_str("\n### Custom Preferences\n\n");
            for (key, value) in &self.preferences.custom {
                prefs.push_str(&format!("- {}: {}\n", key, value));
            }
        }

        // Add CLAUDE.md content if present
        if let Some(claude_md) = &self.claude_md_content {
            prefs.push_str("\n### From CLAUDE.md\n\n");
            prefs.push_str(claude_md);
            if !claude_md.ends_with('\n') {
                prefs.push('\n');
            }
        }

        // Add relevant memory items (only high-confidence ones)
        if !self.memory_items.is_empty() {
            let high_confidence: Vec<_> = self
                .memory_items
                .iter()
                .filter(|item| item.confidence >= 0.7)
                .collect();

            if !high_confidence.is_empty() {
                prefs.push_str("\n### User Memory\n\n");
                for item in high_confidence {
                    prefs.push_str(&format!("- {}: {}\n", item.key, item.value));
                }
            }
        }

        resolved.user_preferences = Some(prefs);
    }
}

// ============================================================================
// Database Row Types (for sqlx without compile-time checks)
// ============================================================================

#[cfg(feature = "database")]
/// Database row for user preferences
#[derive(Debug, sqlx::FromRow)]
struct UserPreferencesRow {
    preferences: Option<Value>,
}

#[cfg(feature = "database")]
/// Database row for memory items
#[derive(Debug, sqlx::FromRow)]
struct MemoryItemRow {
    category: String,
    key: String,
    value: Option<Value>,
    confidence: Option<f64>,
    source: String,
}

// ============================================================================
// User Context Loader
// ============================================================================

/// Loader for user context from database and filesystem
pub struct UserContextLoader {
    #[cfg(feature = "database")]
    pool: Option<sqlx::Pool<sqlx::Postgres>>,
    claude_md_dir: PathBuf,
}

impl UserContextLoader {
    /// Create a new user context loader with database pool
    #[cfg(feature = "database")]
    pub fn new(pool: Option<sqlx::Pool<sqlx::Postgres>>, claude_md_dir: impl AsRef<Path>) -> Self {
        Self {
            pool,
            claude_md_dir: claude_md_dir.as_ref().to_path_buf(),
        }
    }

    /// Create a new user context loader (no-op database mode)
    #[cfg(not(feature = "database"))]
    pub fn new(claude_md_dir: impl AsRef<Path>) -> Self {
        Self {
            claude_md_dir: claude_md_dir.as_ref().to_path_buf(),
        }
    }

    /// Load user context for the given user ID
    ///
    /// This loads from multiple sources:
    /// 1. CLAUDE.md file from the configured directory
    /// 2. User preferences from database (when database feature enabled)
    /// 3. Memory items from database (when database feature enabled)
    #[cfg(feature = "database")]
    pub async fn load(&self, user_id: &Uuid) -> Result<UserContext> {
        // Load CLAUDE.md if exists (errors are ignored)
        let claude_md = self.load_claude_md().ok();

        // Load preferences from database (falls back to default)
        let preferences = self
            .load_preferences_from_db(user_id)
            .await
            .unwrap_or_default();

        // Load memory items from database (falls back to empty)
        let memory_items = self.load_memory_from_db(user_id).await.unwrap_or_default();

        Ok(UserContext {
            user_id: *user_id,
            preferences,
            claude_md_content: claude_md,
            memory_items,
        })
    }

    /// Load user context for the given user ID (no database)
    #[cfg(not(feature = "database"))]
    pub async fn load(&self, user_id: &Uuid) -> Result<UserContext> {
        Ok(self.load_sync(user_id))
    }

    /// Load user context synchronously (without database queries)
    ///
    /// Only loads CLAUDE.md file, useful for offline contexts
    pub fn load_sync(&self, user_id: &Uuid) -> UserContext {
        let claude_md = self.load_claude_md().ok();

        UserContext {
            user_id: *user_id,
            preferences: UserPreferences::default(),
            claude_md_content: claude_md,
            memory_items: vec![],
        }
    }

    /// Load CLAUDE.md file from the configured directory
    fn load_claude_md(&self) -> Result<String> {
        let path = self.claude_md_dir.join("CLAUDE.md");
        std::fs::read_to_string(&path).map_err(|e| {
            Error::Config(format!(
                "Failed to read CLAUDE.md at {}: {}",
                path.display(),
                e
            ))
        })
    }

    /// Load CLAUDE.md file asynchronously
    pub async fn load_claude_md_async(&self) -> Result<String> {
        let path = self.claude_md_dir.join("CLAUDE.md");
        tokio::fs::read_to_string(&path).await.map_err(|e| {
            Error::Config(format!(
                "Failed to read CLAUDE.md at {}: {}",
                path.display(),
                e
            ))
        })
    }

    /// Load preferences from database
    #[cfg(feature = "database")]
    async fn load_preferences_from_db(&self, user_id: &Uuid) -> Result<UserPreferences> {
        let Some(pool) = &self.pool else {
            return Ok(UserPreferences::default());
        };

        // Query user preferences using query_as to avoid compile-time checks
        let row: Option<UserPreferencesRow> =
            sqlx::query_as("SELECT preferences FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| Error::Database(e))?;

        match row {
            Some(r) => {
                let prefs_value = r.preferences.unwrap_or(Value::Object(Default::default()));
                serde_json::from_value(prefs_value)
                    .map_err(|e| Error::Config(format!("Failed to parse user preferences: {}", e)))
            }
            None => Ok(UserPreferences::default()),
        }
    }

    /// Load memory items from database
    #[cfg(feature = "database")]
    async fn load_memory_from_db(&self, user_id: &Uuid) -> Result<Vec<MemoryItem>> {
        let Some(pool) = &self.pool else {
            return Ok(vec![]);
        };

        // Query user memory using query_as to avoid compile-time checks
        let rows: Vec<MemoryItemRow> = sqlx::query_as(
            r#"
            SELECT category, key, value, confidence, source
            FROM user_memory
            WHERE user_id = $1
            ORDER BY confidence DESC
            LIMIT 50
            "#,
        )
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(|e| Error::Database(e))?;

        let items = rows
            .into_iter()
            .map(|r| MemoryItem {
                category: r.category,
                key: r.key,
                value: r.value.unwrap_or(Value::Null),
                confidence: r.confidence.unwrap_or(1.0) as f32,
                source: r.source,
            })
            .collect();

        Ok(items)
    }

    /// Check if CLAUDE.md file exists
    pub fn claude_md_exists(&self) -> bool {
        self.claude_md_dir.join("CLAUDE.md").exists()
    }

    /// Get the CLAUDE.md directory path
    pub fn claude_md_dir(&self) -> &Path {
        &self.claude_md_dir
    }
}

impl std::fmt::Debug for UserContextLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserContextLoader")
            .field("claude_md_dir", &self.claude_md_dir)
            .field("has_pool", &{
                #[cfg(feature = "database")]
                {
                    self.pool.is_some()
                }
                #[cfg(not(feature = "database"))]
                {
                    false
                }
            })
            .finish()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_context_default() {
        let ctx = UserContext::default();
        assert_eq!(ctx.user_id, Uuid::nil());
        assert!(ctx.claude_md_content.is_none());
        assert!(ctx.memory_items.is_empty());
    }

    #[test]
    fn test_user_preferences_default() {
        let prefs = UserPreferences::default();
        assert!(prefs.coding_style.is_none());
        assert!(prefs.communication.is_none());
        assert!(prefs.custom.is_empty());
    }

    #[test]
    fn test_coding_style_default() {
        let style = CodingStyle::default();
        assert!(!style.prefer_explicit_types);
        assert!(style.max_line_length.is_none());
        assert!(!style.prefer_functional);
    }

    #[test]
    fn test_communication_prefs_default() {
        let comm = CommunicationPrefs::default();
        assert_eq!(comm.verbosity, "normal");
        assert!(comm.include_explanations);
    }

    #[test]
    fn test_user_context_applies_preferences() {
        let user = UserContext {
            user_id: Uuid::new_v4(),
            preferences: UserPreferences {
                coding_style: Some(CodingStyle {
                    prefer_explicit_types: true,
                    max_line_length: Some(100),
                    prefer_functional: false,
                }),
                communication: Some(CommunicationPrefs {
                    verbosity: "minimal".to_string(),
                    include_explanations: true,
                }),
                custom: Default::default(),
            },
            claude_md_content: Some("Custom instructions here".to_string()),
            memory_items: vec![],
        };

        let mut resolved = ResolvedContext::default();
        user.apply_to(&mut resolved);

        assert!(resolved.user_preferences.is_some());
        let prefs = resolved.user_preferences.unwrap();
        assert!(prefs.contains("explicit type"));
        assert!(prefs.contains("100"));
        assert!(prefs.contains("minimal"));
        assert!(prefs.contains("Custom instructions"));
    }

    #[test]
    fn test_user_context_applies_memory_items() {
        let user = UserContext {
            user_id: Uuid::new_v4(),
            preferences: UserPreferences::default(),
            claude_md_content: None,
            memory_items: vec![
                MemoryItem {
                    category: "preference".to_string(),
                    key: "favorite_language".to_string(),
                    value: Value::String("Rust".to_string()),
                    confidence: 0.9,
                    source: "explicit".to_string(),
                },
                MemoryItem {
                    category: "fact".to_string(),
                    key: "low_confidence_item".to_string(),
                    value: Value::String("ignored".to_string()),
                    confidence: 0.5,
                    source: "inferred".to_string(),
                },
            ],
        };

        let mut resolved = ResolvedContext::default();
        user.apply_to(&mut resolved);

        let prefs = resolved.user_preferences.unwrap();
        assert!(prefs.contains("favorite_language"));
        assert!(prefs.contains("Rust"));
        // Low confidence item should not be included
        assert!(!prefs.contains("low_confidence_item"));
    }

    #[test]
    fn test_user_context_layer_name() {
        let user = UserContext::default();
        assert_eq!(user.layer_name(), "user");
    }

    #[test]
    fn test_user_context_priority() {
        let user = UserContext::default();
        assert_eq!(user.priority(), ContextPriority::User);
    }

    #[test]
    fn test_user_context_serde_round_trip() {
        let user = UserContext {
            user_id: Uuid::new_v4(),
            preferences: UserPreferences {
                coding_style: Some(CodingStyle {
                    prefer_explicit_types: true,
                    max_line_length: Some(80),
                    prefer_functional: true,
                }),
                communication: Some(CommunicationPrefs {
                    verbosity: "detailed".to_string(),
                    include_explanations: false,
                }),
                custom: {
                    let mut map = serde_json::Map::new();
                    map.insert("theme".to_string(), Value::String("dark".to_string()));
                    map
                },
            },
            claude_md_content: Some("# My Rules\n- Be concise".to_string()),
            memory_items: vec![MemoryItem {
                category: "preference".to_string(),
                key: "editor".to_string(),
                value: Value::String("neovim".to_string()),
                confidence: 0.85,
                source: "conversation".to_string(),
            }],
        };

        // Serialize
        let json = serde_json::to_string(&user).expect("serialize");

        // Deserialize
        let restored: UserContext = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(user.user_id, restored.user_id);
        assert_eq!(
            user.preferences
                .coding_style
                .as_ref()
                .unwrap()
                .max_line_length,
            restored
                .preferences
                .coding_style
                .as_ref()
                .unwrap()
                .max_line_length
        );
        assert_eq!(user.claude_md_content, restored.claude_md_content);
        assert_eq!(user.memory_items.len(), restored.memory_items.len());
    }

    #[test]
    fn test_loader_without_pool() {
        #[cfg(feature = "database")]
        let loader = UserContextLoader::new(None, "/tmp/test");
        #[cfg(not(feature = "database"))]
        let loader = UserContextLoader::new("/tmp/test");
        let ctx = loader.load_sync(&Uuid::new_v4());
        assert!(ctx.preferences.coding_style.is_none());
    }

    #[test]
    fn test_loader_debug() {
        #[cfg(feature = "database")]
        let loader = UserContextLoader::new(None, "/tmp/test");
        #[cfg(not(feature = "database"))]
        let loader = UserContextLoader::new("/tmp/test");
        let debug = format!("{:?}", loader);
        assert!(debug.contains("UserContextLoader"));
        assert!(debug.contains("/tmp/test"));
        assert!(debug.contains("has_pool"));
    }
}
