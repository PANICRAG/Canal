//! Memory Types - User memory and preference storage
//!
//! This module defines types for storing user memories and preferences
//! that persist across conversations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Source of a memory entry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    /// Explicitly stated by the user
    UserStated,
    /// Inferred from conversation
    Inferred,
    /// Set by the system
    System,
    /// Imported from external source
    Imported,
}

impl Default for MemorySource {
    fn default() -> Self {
        Self::Inferred
    }
}

/// Memory category for organization
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    /// Personal information (name, location, etc.)
    Personal,
    /// Preferences (coding style, communication style, etc.)
    Preferences,
    /// Technical context (languages, frameworks, etc.)
    Technical,
    /// Project-specific information
    Project,
    /// Custom category
    Custom(String),
}

impl Default for MemoryCategory {
    fn default() -> Self {
        Self::Preferences
    }
}

/// Confidence level for inferred memories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryConfidence {
    /// Low confidence - may need confirmation
    Low,
    /// Medium confidence
    Medium,
    /// High confidence
    High,
    /// Confirmed by user
    Confirmed,
}

impl Default for MemoryConfidence {
    fn default() -> Self {
        Self::Medium
    }
}

impl MemoryConfidence {
    /// Get numeric value for comparison
    pub fn value(&self) -> u8 {
        match self {
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::Confirmed => 4,
        }
    }
}

/// A single memory entry with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// The memory key (e.g., "preferred_language", "name")
    pub key: String,
    /// The memory value
    pub value: serde_json::Value,
    /// When this memory was created
    pub created_at: DateTime<Utc>,
    /// When this memory was last updated
    pub updated_at: DateTime<Utc>,
    /// Source of this memory
    #[serde(default)]
    pub source: MemorySource,
    /// Category of this memory
    #[serde(default)]
    pub category: MemoryCategory,
    /// Confidence level
    #[serde(default)]
    pub confidence: MemoryConfidence,
    /// Session ID where this memory was created/updated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Additional metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl MemoryEntry {
    /// Create a new memory entry
    pub fn new(key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        let now = Utc::now();
        Self {
            key: key.into(),
            value: value.into(),
            created_at: now,
            updated_at: now,
            source: MemorySource::default(),
            category: MemoryCategory::default(),
            confidence: MemoryConfidence::default(),
            session_id: None,
            metadata: HashMap::new(),
        }
    }

    /// Create a memory entry with a string value
    pub fn text(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(key, serde_json::Value::String(value.into()))
    }

    /// Create a memory entry with a boolean value
    pub fn bool(key: impl Into<String>, value: bool) -> Self {
        Self::new(key, serde_json::Value::Bool(value))
    }

    /// Set the source
    pub fn with_source(mut self, source: MemorySource) -> Self {
        self.source = source;
        self
    }

    /// Set the category
    pub fn with_category(mut self, category: MemoryCategory) -> Self {
        self.category = category;
        self
    }

    /// Set the confidence level
    pub fn with_confidence(mut self, confidence: MemoryConfidence) -> Self {
        self.confidence = confidence;
        self
    }

    /// Set the session ID
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Update the value and timestamp
    pub fn update(&mut self, value: serde_json::Value) {
        self.value = value;
        self.updated_at = Utc::now();
    }

    /// Get value as string if it's a string
    pub fn as_str(&self) -> Option<&str> {
        self.value.as_str()
    }

    /// Get value as bool if it's a boolean
    pub fn as_bool(&self) -> Option<bool> {
        self.value.as_bool()
    }

    /// Get value as i64 if it's a number
    pub fn as_i64(&self) -> Option<i64> {
        self.value.as_i64()
    }
}

/// User memory collection
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserMemory {
    /// User ID this memory belongs to
    pub user_id: String,
    /// Memory entries indexed by key
    pub entries: HashMap<String, MemoryEntry>,
    /// When this memory collection was created
    pub created_at: DateTime<Utc>,
    /// When this memory collection was last updated
    pub updated_at: DateTime<Utc>,
    /// Version for optimistic locking
    #[serde(default)]
    pub version: u64,
}

impl UserMemory {
    /// Create a new user memory collection
    pub fn new(user_id: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            user_id: user_id.into(),
            entries: HashMap::new(),
            created_at: now,
            updated_at: now,
            version: 0,
        }
    }

    /// Get a memory entry by key
    pub fn get(&self, key: &str) -> Option<&MemoryEntry> {
        self.entries.get(key)
    }

    /// Get a memory value as string
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.entries.get(key).and_then(|e| e.as_str())
    }

    /// Get a memory value as bool
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.entries.get(key).and_then(|e| e.as_bool())
    }

    /// Set or update a memory entry
    pub fn set(&mut self, entry: MemoryEntry) {
        self.entries.insert(entry.key.clone(), entry);
        self.updated_at = Utc::now();
        self.version += 1;
    }

    /// Set a text memory
    pub fn set_text(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.set(MemoryEntry::text(key, value));
    }

    /// Set a boolean memory
    pub fn set_bool(&mut self, key: impl Into<String>, value: bool) {
        self.set(MemoryEntry::bool(key, value));
    }

    /// Remove a memory entry
    pub fn remove(&mut self, key: &str) -> Option<MemoryEntry> {
        let removed = self.entries.remove(key);
        if removed.is_some() {
            self.updated_at = Utc::now();
            self.version += 1;
        }
        removed
    }

    /// Check if a memory key exists
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Get all memory entries
    pub fn all(&self) -> impl Iterator<Item = &MemoryEntry> {
        self.entries.values()
    }

    /// Get entries by category
    pub fn by_category(&self, category: &MemoryCategory) -> Vec<&MemoryEntry> {
        self.entries
            .values()
            .filter(|e| &e.category == category)
            .collect()
    }

    /// Get entries with minimum confidence
    pub fn by_min_confidence(&self, min_confidence: MemoryConfidence) -> Vec<&MemoryEntry> {
        self.entries
            .values()
            .filter(|e| e.confidence.value() >= min_confidence.value())
            .collect()
    }

    /// Get the number of memory entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if memory is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Merge another memory collection, keeping newer entries
    pub fn merge(&mut self, other: UserMemory) {
        for (key, entry) in other.entries {
            match self.entries.get(&key) {
                Some(existing) if existing.updated_at >= entry.updated_at => {
                    // Keep existing if it's newer
                }
                _ => {
                    self.entries.insert(key, entry);
                }
            }
        }
        self.updated_at = Utc::now();
        self.version += 1;
    }

    /// Format memories for inclusion in system prompt
    pub fn format_for_prompt(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }

        let mut sections: HashMap<&MemoryCategory, Vec<&MemoryEntry>> = HashMap::new();

        for entry in self.entries.values() {
            sections.entry(&entry.category).or_default().push(entry);
        }

        let mut output = String::from("## User Preferences and Memory\n\n");

        for (category, entries) in sections {
            let category_name = match category {
                MemoryCategory::Personal => "Personal Information",
                MemoryCategory::Preferences => "Preferences",
                MemoryCategory::Technical => "Technical Context",
                MemoryCategory::Project => "Project Information",
                MemoryCategory::Custom(name) => name,
            };

            output.push_str(&format!("### {}\n", category_name));
            for entry in entries {
                let value_str = match &entry.value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Array(arr) => arr
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    other => other.to_string(),
                };
                output.push_str(&format!("- {}: {}\n", entry.key, value_str));
            }
            output.push('\n');
        }

        output
    }
}

/// Memory update event for hooks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryUpdateEvent {
    /// User ID
    pub user_id: String,
    /// Memory key
    pub key: String,
    /// Previous value (if any)
    pub old_value: Option<serde_json::Value>,
    /// New value
    pub new_value: serde_json::Value,
    /// Source of the update
    pub source: MemorySource,
    /// Session ID where update occurred
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Memory extraction request - used to ask LLM to extract memories
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryExtractionRequest {
    /// Conversation messages to analyze
    pub messages: Vec<serde_json::Value>,
    /// Existing memories to consider
    pub existing_memories: Vec<String>,
    /// Categories to focus on
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<MemoryCategory>>,
}

/// Memory extraction response from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryExtractionResponse {
    /// New memories to add
    pub new_memories: Vec<ExtractedMemory>,
    /// Memories to update
    pub updated_memories: Vec<ExtractedMemory>,
    /// Memories to delete (keys only)
    pub deleted_memories: Vec<String>,
}

/// An extracted memory from conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedMemory {
    /// Memory key
    pub key: String,
    /// Memory value
    pub value: serde_json::Value,
    /// Category
    pub category: MemoryCategory,
    /// Confidence
    pub confidence: MemoryConfidence,
    /// Reasoning for this extraction
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_entry_creation() {
        let entry = MemoryEntry::text("name", "Alice")
            .with_source(MemorySource::UserStated)
            .with_category(MemoryCategory::Personal)
            .with_confidence(MemoryConfidence::Confirmed);

        assert_eq!(entry.key, "name");
        assert_eq!(entry.as_str(), Some("Alice"));
        assert_eq!(entry.source, MemorySource::UserStated);
        assert_eq!(entry.category, MemoryCategory::Personal);
        assert_eq!(entry.confidence, MemoryConfidence::Confirmed);
    }

    #[test]
    fn test_user_memory_operations() {
        let mut memory = UserMemory::new("user-1");

        // Set memories
        memory.set_text("name", "Alice");
        memory.set_bool("prefers_dark_mode", true);

        assert_eq!(memory.len(), 2);
        assert_eq!(memory.get_str("name"), Some("Alice"));
        assert_eq!(memory.get_bool("prefers_dark_mode"), Some(true));

        // Update memory
        memory.set_text("name", "Bob");
        assert_eq!(memory.get_str("name"), Some("Bob"));

        // Remove memory
        let removed = memory.remove("prefers_dark_mode");
        assert!(removed.is_some());
        assert_eq!(memory.len(), 1);
    }

    #[test]
    fn test_user_memory_by_category() {
        let mut memory = UserMemory::new("user-1");

        memory.set(MemoryEntry::text("name", "Alice").with_category(MemoryCategory::Personal));
        memory.set(MemoryEntry::text("language", "Rust").with_category(MemoryCategory::Technical));
        memory
            .set(MemoryEntry::text("framework", "Actix").with_category(MemoryCategory::Technical));

        let technical = memory.by_category(&MemoryCategory::Technical);
        assert_eq!(technical.len(), 2);

        let personal = memory.by_category(&MemoryCategory::Personal);
        assert_eq!(personal.len(), 1);
    }

    #[test]
    fn test_format_for_prompt() {
        let mut memory = UserMemory::new("user-1");

        memory.set(MemoryEntry::text("name", "Alice").with_category(MemoryCategory::Personal));
        memory.set(MemoryEntry::text("language", "Rust").with_category(MemoryCategory::Technical));

        let prompt = memory.format_for_prompt();
        assert!(prompt.contains("User Preferences"));
        assert!(prompt.contains("name: Alice"));
        assert!(prompt.contains("language: Rust"));
    }

    #[test]
    fn test_memory_merge() {
        let mut memory1 = UserMemory::new("user-1");
        memory1.set_text("key1", "value1");
        memory1.set_text("key2", "old_value");

        // Sleep to ensure different timestamps
        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut memory2 = UserMemory::new("user-1");
        memory2.set_text("key2", "new_value");
        memory2.set_text("key3", "value3");

        memory1.merge(memory2);

        assert_eq!(memory1.get_str("key1"), Some("value1"));
        assert_eq!(memory1.get_str("key2"), Some("new_value"));
        assert_eq!(memory1.get_str("key3"), Some("value3"));
    }

    #[test]
    fn test_memory_confidence_ordering() {
        assert!(MemoryConfidence::Low.value() < MemoryConfidence::Medium.value());
        assert!(MemoryConfidence::Medium.value() < MemoryConfidence::High.value());
        assert!(MemoryConfidence::High.value() < MemoryConfidence::Confirmed.value());
    }
}
