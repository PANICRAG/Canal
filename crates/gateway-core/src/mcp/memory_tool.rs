//! Universal Memory Tool
//!
//! A model-agnostic memory tool that enables ANY LLM (Claude, Qwen, GPT, etc.)
//! to have persistent memory across conversations.
//!
//! ## Design Philosophy
//! - Claude has native memory behaviors through training
//! - Other models learn memory behaviors through system prompt engineering
//! - The tool interface is identical for all models
//! - System prompts are customized per model family for best results
//!
//! ## Architecture
//! The Memory Tool is now backed by `UnifiedMemoryStore`, which provides:
//! - Single source of truth for all memory data
//! - File-based view for LLM consumption (JSON format, migrated from XML)
//! - Structured API for frontend access
//! - Full-text search across all entries
//!
//! ## Supported Models
//! - Claude (Anthropic) - native memory_20250818 format
//! - Qwen (Alibaba) - function calling with detailed instructions
//! - GPT (OpenAI) - function calling with detailed instructions
//! - Gemini (Google) - function calling with detailed instructions
//! - DeepSeek - function calling with detailed instructions
//! - Any model with function/tool calling capability

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

use crate::error::Result;
use crate::memory::{Confidence, MemoryCategory, MemoryEntry, MemorySource, UnifiedMemoryStore};

// ============================================
// Model Family Detection
// ============================================

/// Supported model families
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFamily {
    Claude,
    Qwen,
    Gpt,
    Gemini,
    DeepSeek,
    Llama,
    Mistral,
    Other,
}

impl ModelFamily {
    /// Detect model family from model name
    pub fn from_model_name(model: &str) -> Self {
        let model_lower = model.to_lowercase();

        if model_lower.contains("claude") {
            ModelFamily::Claude
        } else if model_lower.contains("qwen") {
            ModelFamily::Qwen
        } else if model_lower.contains("gpt")
            || model_lower.contains("o1")
            || model_lower.contains("o3")
        {
            ModelFamily::Gpt
        } else if model_lower.contains("gemini") {
            ModelFamily::Gemini
        } else if model_lower.contains("deepseek") {
            ModelFamily::DeepSeek
        } else if model_lower.contains("llama") {
            ModelFamily::Llama
        } else if model_lower.contains("mistral") || model_lower.contains("mixtral") {
            ModelFamily::Mistral
        } else {
            ModelFamily::Other
        }
    }

    /// Check if model has native memory tool support
    pub fn has_native_memory_support(&self) -> bool {
        matches!(self, ModelFamily::Claude)
    }
}

// ============================================
// Memory Tool Types
// ============================================

/// Input for memory tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryToolInput {
    pub command: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub view_range: Option<(usize, usize)>,
    #[serde(default)]
    pub file_text: Option<String>,
    #[serde(default)]
    pub old_str: Option<String>,
    #[serde(default)]
    pub new_str: Option<String>,
    #[serde(default)]
    pub insert_line: Option<usize>,
    #[serde(default)]
    pub insert_text: Option<String>,
    #[serde(default)]
    pub old_path: Option<String>,
    #[serde(default)]
    pub new_path: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Result of memory tool operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryToolResult {
    pub success: bool,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_data: Option<Value>,
}

impl MemoryToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            success: true,
            content: content.into(),
            structured_data: None,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            success: false,
            content: content.into(),
            structured_data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.structured_data = Some(data);
        self
    }
}

// ============================================
// Memory Tool Handler
// ============================================

/// Universal Memory Tool handler that works with any LLM
///
/// Now backed by `UnifiedMemoryStore` for consistent data access
/// across the file-based LLM view and structured API.
pub struct MemoryToolHandler {
    user_id: Uuid,
    store: Arc<UnifiedMemoryStore>,
    model_family: ModelFamily,
}

impl MemoryToolHandler {
    /// Create a new memory tool handler with unified store
    pub fn new(user_id: Uuid, store: Arc<UnifiedMemoryStore>) -> Self {
        Self {
            user_id,
            store,
            model_family: ModelFamily::Other,
        }
    }

    /// Create handler for a specific model
    pub fn for_model(user_id: Uuid, store: Arc<UnifiedMemoryStore>, model_name: &str) -> Self {
        Self {
            user_id,
            store,
            model_family: ModelFamily::from_model_name(model_name),
        }
    }

    /// Set the model family
    pub fn with_model_family(mut self, family: ModelFamily) -> Self {
        self.model_family = family;
        self
    }

    /// Initialize handler - creates index if needed
    pub async fn initialize(&self) -> Result<()> {
        // Create index entry if it doesn't exist
        if self.store.get(self.user_id, "index").await.is_none() {
            let index_content = self.generate_index_content();
            let entry = MemoryEntry::new("index", MemoryCategory::Custom, index_content)
                .with_title("Memory System Index")
                .with_source(MemorySource::System)
                .with_confidence(Confidence::Confirmed);
            self.store.store(self.user_id, entry).await?;
        }
        Ok(())
    }

    // ============================================
    // Tool Definitions (Model-Specific)
    // ============================================

    /// Get tool definition for Claude (native format)
    pub fn claude_tool_definition() -> Value {
        json!({
            "type": "memory_20250818",
            "name": "memory"
        })
    }

    /// Get tool definition for Qwen/GPT/other models (function calling format)
    pub fn universal_tool_definition() -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "memory",
                "description": "Persistent memory storage across conversations. IMPORTANT: Always check your memory at the start of every conversation using 'view' command. Save important information to remember it later.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "enum": ["view", "create", "str_replace", "insert", "delete", "rename", "search"],
                            "description": "The operation to perform:\n- view: List directory or read file contents\n- create: Create a new file\n- str_replace: Replace text in a file\n- insert: Insert text at a specific line\n- delete: Delete a file or directory\n- rename: Rename or move a file\n- search: Search across all memories"
                        },
                        "path": {
                            "type": "string",
                            "description": "File or directory path, must start with /memories/"
                        },
                        "file_text": {
                            "type": "string",
                            "description": "Content for create command"
                        },
                        "old_str": {
                            "type": "string",
                            "description": "Text to find and replace (must be unique in file)"
                        },
                        "new_str": {
                            "type": "string",
                            "description": "Replacement text"
                        },
                        "insert_line": {
                            "type": "integer",
                            "description": "Line number to insert at (0-indexed)"
                        },
                        "insert_text": {
                            "type": "string",
                            "description": "Text to insert"
                        },
                        "old_path": {
                            "type": "string",
                            "description": "Source path for rename"
                        },
                        "new_path": {
                            "type": "string",
                            "description": "Destination path for rename"
                        },
                        "query": {
                            "type": "string",
                            "description": "Search query for search command"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max search results (default 10)"
                        }
                    },
                    "required": ["command"]
                }
            }
        })
    }

    /// Get the appropriate tool definition based on model family
    pub fn get_tool_definition(&self) -> Value {
        match self.model_family {
            ModelFamily::Claude => Self::claude_tool_definition(),
            _ => Self::universal_tool_definition(),
        }
    }

    // ============================================
    // System Prompts (Model-Specific)
    // ============================================

    /// Get system prompt for Claude (minimal - relies on native behavior)
    pub fn claude_system_prompt() -> String {
        r#"
MEMORY PROTOCOL:
1. Always check /memories directory before starting any task
2. Save important progress and learnings to memory files
3. Use the search command to find relevant past information

Your memory persists across conversations. Use it wisely.
"#
        .to_string()
    }

    /// Get system prompt for Qwen (detailed instructions in Chinese + English)
    pub fn qwen_system_prompt() -> String {
        r#"
## 记忆系统使用协议 / Memory System Protocol

你拥有一个持久化记忆系统，可以跨对话保存和读取信息。

### 核心规则 / Core Rules

**每次对话开始时，必须首先检查记忆：**
```json
{"command": "view", "path": "/memories"}
```

**在执行任务过程中，记录重要进展：**
```json
{"command": "create", "path": "/memories/task_progress.xml", "file_text": "<progress>...</progress>"}
```

**任务完成时，保存学到的经验：**
```json
{"command": "create", "path": "/memories/lessons_learned.xml", "file_text": "<lessons>...</lessons>"}
```

### 可用命令 / Available Commands

| 命令 | 用途 | 示例 |
|------|------|------|
| view | 查看目录或文件 | `{"command": "view", "path": "/memories"}` |
| create | 创建新文件 | `{"command": "create", "path": "/memories/notes.xml", "file_text": "..."}` |
| str_replace | 替换文件中的文本 | `{"command": "str_replace", "path": "/memories/notes.xml", "old_str": "old", "new_str": "new"}` |
| insert | 在指定行插入文本 | `{"command": "insert", "path": "/memories/notes.xml", "insert_line": 5, "insert_text": "..."}` |
| delete | 删除文件 | `{"command": "delete", "path": "/memories/old_file.xml"}` |
| rename | 重命名文件 | `{"command": "rename", "old_path": "/memories/a.xml", "new_path": "/memories/b.xml"}` |
| search | 搜索记忆 | `{"command": "search", "query": "用户偏好", "limit": 5}` |

### 记忆最佳实践 / Best Practices

1. **结构化存储**：使用 XML 格式保存信息，便于后续解析
2. **定期整理**：删除过时的文件，重命名使文件名更清晰
3. **分类存储**：
   - `/memories/user_*.xml` - 用户相关信息
   - `/memories/project_*.xml` - 项目相关信息
   - `/memories/task_*.xml` - 任务进展
   - `/memories/lessons_*.xml` - 经验教训

### 重要提醒 / Important

- 你的上下文窗口可能随时被重置，未保存到记忆的进展会丢失
- 假设每次对话可能是新的开始，依赖记忆而不是上下文
- 记忆是你跨对话保持连续性的唯一方式
"#.to_string()
    }

    /// Get system prompt for GPT models (concise English)
    pub fn gpt_system_prompt() -> String {
        r#"
## Memory System

You have access to a persistent memory system that survives across conversations.

### CRITICAL: Always Start With Memory Check

At the START of EVERY conversation, before doing anything else, run:
```json
{"command": "view", "path": "/memories"}
```

This shows you what you remember from previous conversations.

### Memory Commands

1. **view** - List directory or read file
   `{"command": "view", "path": "/memories/notes.xml"}`

2. **create** - Create new file
   `{"command": "create", "path": "/memories/notes.xml", "file_text": "<notes>...</notes>"}`

3. **str_replace** - Replace text in file
   `{"command": "str_replace", "path": "/memories/file.xml", "old_str": "find this", "new_str": "replace with"}`

4. **insert** - Insert at line
   `{"command": "insert", "path": "/memories/file.xml", "insert_line": 3, "insert_text": "new line"}`

5. **delete** - Delete file
   `{"command": "delete", "path": "/memories/old.xml"}`

6. **rename** - Move/rename file
   `{"command": "rename", "old_path": "/memories/a.xml", "new_path": "/memories/b.xml"}`

7. **search** - Search memories
   `{"command": "search", "query": "user preferences"}`

### Best Practices

- Save progress frequently - your context may be cleared
- Use structured XML for easy parsing
- Keep filenames descriptive
- Delete outdated information
- Assume memory is your only continuity across conversations
"#.to_string()
    }

    /// Get system prompt for DeepSeek (technical, detailed)
    pub fn deepseek_system_prompt() -> String {
        r#"
## Persistent Memory System Protocol

### Overview
You have access to a persistent file-based memory system mounted at `/memories/`.
This memory persists across conversation sessions and context window resets.

### Initialization Protocol
1. At conversation start, execute: `{"command": "view", "path": "/memories"}`
2. Read relevant memory files based on the task context
3. Proceed with the user's request

### Command Reference

```
view(path: str, view_range?: [int, int])
  - View directory listing or file contents
  - view_range: optional line range for large files

create(path: str, file_text: str)
  - Create new file at path with content
  - Error if file already exists

str_replace(path: str, old_str: str, new_str: str)
  - Replace exact match of old_str with new_str
  - old_str must appear exactly once (uniqueness required)

insert(path: str, insert_line: int, insert_text: str)
  - Insert text at specified line number (0-indexed)

delete(path: str)
  - Delete file or directory recursively

rename(old_path: str, new_path: str)
  - Move/rename file or directory

search(query: str, limit?: int)
  - Full-text search across all memories
  - Returns up to limit results (default 10)
```

### Memory Organization Schema
```
/memories/
├── index.xml           # Auto-generated index
├── preferences.xml     # User preferences
├── context/           # Conversation context
│   └── session_*.xml
├── projects/          # Project-specific info
│   └── {project_name}.xml
└── knowledge/         # Learned information
    └── {topic}.xml
```

### Continuity Guarantee
- Memory is the ONLY state that persists
- Context window may be cleared at any time
- Always save critical information to memory
- Always check memory at conversation start
"#
        .to_string()
    }

    /// Get system prompt for generic models (universal format)
    pub fn generic_system_prompt() -> String {
        r#"
## Memory Tool Instructions

You have a persistent memory system. Use these commands:

1. CHECK MEMORY FIRST (every conversation):
   {"command": "view", "path": "/memories"}

2. SAVE information:
   {"command": "create", "path": "/memories/filename.xml", "file_text": "content"}

3. UPDATE information:
   {"command": "str_replace", "path": "/memories/filename.xml", "old_str": "old text", "new_str": "new text"}

4. DELETE outdated files:
   {"command": "delete", "path": "/memories/old_file.xml"}

5. SEARCH memories:
   {"command": "search", "query": "search terms"}

IMPORTANT: Your context may reset. Save important information to memory to remember it later.
"#.to_string()
    }

    /// Get the appropriate system prompt based on model family
    pub fn get_system_prompt(&self) -> String {
        match self.model_family {
            ModelFamily::Claude => Self::claude_system_prompt(),
            ModelFamily::Qwen => Self::qwen_system_prompt(),
            ModelFamily::Gpt => Self::gpt_system_prompt(),
            ModelFamily::DeepSeek => Self::deepseek_system_prompt(),
            ModelFamily::Gemini => Self::gpt_system_prompt(), // Similar to GPT
            ModelFamily::Llama => Self::generic_system_prompt(),
            ModelFamily::Mistral => Self::generic_system_prompt(),
            ModelFamily::Other => Self::generic_system_prompt(),
        }
    }

    // ============================================
    // Tool Execution
    // ============================================

    /// Handle a memory tool call
    pub async fn handle(&self, input: MemoryToolInput) -> MemoryToolResult {
        // Validate path security
        if let Some(ref path) = input.path {
            if let Err(e) = self.validate_path(path) {
                return MemoryToolResult::error(e);
            }
        }
        if let Some(ref path) = input.old_path {
            if let Err(e) = self.validate_path(path) {
                return MemoryToolResult::error(e);
            }
        }
        if let Some(ref path) = input.new_path {
            if let Err(e) = self.validate_path(path) {
                return MemoryToolResult::error(e);
            }
        }

        match input.command.as_str() {
            "view" => self.handle_view(input).await,
            "create" => self.handle_create(input).await,
            "str_replace" => self.handle_str_replace(input).await,
            "insert" => self.handle_insert(input).await,
            "delete" => self.handle_delete(input).await,
            "rename" => self.handle_rename(input).await,
            "search" => self.handle_search(input).await,
            _ => MemoryToolResult::error(format!("Unknown command: {}", input.command)),
        }
    }

    fn validate_path(&self, path: &str) -> std::result::Result<(), String> {
        if !path.starts_with("/memories") {
            return Err("Path must start with /memories".to_string());
        }
        if path.contains("..") || path.contains("//") {
            return Err("Invalid path: directory traversal not allowed".to_string());
        }
        // Check single-encoded, double-encoded, and mixed-case percent-encoded traversal
        let lower = path.to_lowercase();
        if lower.contains("%2e") || lower.contains("%252e") || lower.contains("%c0%ae") {
            return Err("Invalid path: encoded traversal not allowed".to_string());
        }
        // Reject null bytes (can truncate paths in some systems)
        if path.contains('\0') || lower.contains("%00") {
            return Err("Invalid path: null bytes not allowed".to_string());
        }
        // Only allow alphanumeric, slashes, hyphens, underscores, dots (not leading)
        // in each path segment — reject any other special characters
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            if segment.starts_with('.') {
                return Err("Invalid path: segments must not start with '.'".to_string());
            }
            if !segment
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
            {
                return Err("Invalid path: contains disallowed characters".to_string());
            }
        }
        Ok(())
    }

    async fn handle_view(&self, input: MemoryToolInput) -> MemoryToolResult {
        let path = input.path.unwrap_or_else(|| "/memories".to_string());

        if path == "/memories" || path == "/memories/" {
            // List directory using UnifiedMemoryStore
            let listing = self.store.list_directory(self.user_id, &path).await;
            MemoryToolResult::success(listing)
        } else {
            // Read file content
            match self.store.read_file(self.user_id, &path).await {
                Some(content) => {
                    let display_content = if let Some((start, end)) = input.view_range {
                        let lines: Vec<&str> = content.lines().collect();
                        let start = start.saturating_sub(1);
                        let end = end.min(lines.len());
                        lines[start..end].join("\n")
                    } else {
                        content
                    };

                    let formatted = self.format_with_line_numbers(&display_content);
                    MemoryToolResult::success(format!("File: {}\n{}", path, formatted))
                }
                None => MemoryToolResult::error(format!("Path {} does not exist", path)),
            }
        }
    }

    async fn handle_create(&self, input: MemoryToolInput) -> MemoryToolResult {
        let path = match input.path {
            Some(p) => p,
            None => return MemoryToolResult::error("path is required"),
        };
        let content = match input.file_text {
            Some(c) => c,
            None => return MemoryToolResult::error("file_text is required"),
        };

        // Check if file already exists
        if self.store.get_by_path(self.user_id, &path).await.is_some() {
            return MemoryToolResult::error(format!(
                "File {} already exists. Use str_replace to modify it.",
                path
            ));
        }

        // Create via UnifiedMemoryStore
        match self.store.write_file(self.user_id, &path, &content).await {
            Ok(()) => MemoryToolResult::success(format!("Created: {}", path)),
            Err(e) => MemoryToolResult::error(format!("Failed to create file: {}", e)),
        }
    }

    async fn handle_str_replace(&self, input: MemoryToolInput) -> MemoryToolResult {
        let path = match input.path {
            Some(p) => p,
            None => return MemoryToolResult::error("path is required"),
        };
        let old_str = match input.old_str {
            Some(s) => s,
            None => return MemoryToolResult::error("old_str is required"),
        };
        let new_str = match input.new_str {
            Some(s) => s,
            None => return MemoryToolResult::error("new_str is required"),
        };

        // First check the content
        let content = match self.store.read_file(self.user_id, &path).await {
            Some(c) => c,
            None => return MemoryToolResult::error(format!("File {} not found", path)),
        };

        let count = content.matches(&old_str).count();

        if count == 0 {
            return MemoryToolResult::error(format!(
                "Text '{}' not found in {}",
                old_str.chars().take(50).collect::<String>(),
                path
            ));
        }

        if count > 1 {
            let line_numbers: Vec<usize> = content
                .lines()
                .enumerate()
                .filter(|(_, line)| line.contains(&old_str))
                .map(|(i, _)| i + 1)
                .collect();

            return MemoryToolResult::error(format!(
                "Text appears {} times (lines {:?}). Make old_str more specific to be unique.",
                count, line_numbers
            ));
        }

        // Perform update
        match self
            .store
            .update_file(self.user_id, &path, &old_str, &new_str)
            .await
        {
            Ok(true) => MemoryToolResult::success(format!("Updated: {}", path)),
            Ok(false) => MemoryToolResult::error(format!("Failed to update: text not found")),
            Err(e) => MemoryToolResult::error(format!("Failed to update: {}", e)),
        }
    }

    async fn handle_insert(&self, input: MemoryToolInput) -> MemoryToolResult {
        let path = match input.path {
            Some(p) => p,
            None => return MemoryToolResult::error("path is required"),
        };
        let insert_line = match input.insert_line {
            Some(l) => l,
            None => return MemoryToolResult::error("insert_line is required"),
        };
        let insert_text = match input.insert_text {
            Some(t) => t,
            None => return MemoryToolResult::error("insert_text is required"),
        };

        // Get current content
        let content = match self.store.read_file(self.user_id, &path).await {
            Some(c) => c,
            None => return MemoryToolResult::error(format!("File {} not found", path)),
        };

        let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let n_lines = lines.len();

        if insert_line > n_lines {
            return MemoryToolResult::error(format!(
                "Line {} out of range (file has {} lines)",
                insert_line, n_lines
            ));
        }

        for (i, line) in insert_text.lines().enumerate() {
            lines.insert(insert_line + i, line.to_string());
        }

        let new_content = lines.join("\n");

        match self
            .store
            .write_file(self.user_id, &path, &new_content)
            .await
        {
            Ok(()) => {
                MemoryToolResult::success(format!("Inserted at line {} in {}", insert_line, path))
            }
            Err(e) => MemoryToolResult::error(format!("Failed to insert: {}", e)),
        }
    }

    async fn handle_delete(&self, input: MemoryToolInput) -> MemoryToolResult {
        let path = match input.path {
            Some(p) => p,
            None => return MemoryToolResult::error("path is required"),
        };

        if self.store.delete_file(self.user_id, &path).await {
            MemoryToolResult::success(format!("Deleted: {}", path))
        } else {
            MemoryToolResult::error(format!("Path {} not found", path))
        }
    }

    async fn handle_rename(&self, input: MemoryToolInput) -> MemoryToolResult {
        let old_path = match input.old_path {
            Some(p) => p,
            None => return MemoryToolResult::error("old_path is required"),
        };
        let new_path = match input.new_path {
            Some(p) => p,
            None => return MemoryToolResult::error("new_path is required"),
        };

        // Check source exists
        if self
            .store
            .get_by_path(self.user_id, &old_path)
            .await
            .is_none()
        {
            return MemoryToolResult::error(format!("Source {} not found", old_path));
        }

        // Check destination doesn't exist
        if self
            .store
            .get_by_path(self.user_id, &new_path)
            .await
            .is_some()
        {
            return MemoryToolResult::error(format!("Destination {} already exists", new_path));
        }

        match self
            .store
            .rename_file(self.user_id, &old_path, &new_path)
            .await
        {
            Ok(true) => MemoryToolResult::success(format!("Renamed: {} -> {}", old_path, new_path)),
            Ok(false) => MemoryToolResult::error("Rename failed"),
            Err(e) => MemoryToolResult::error(format!("Rename failed: {}", e)),
        }
    }

    async fn handle_search(&self, input: MemoryToolInput) -> MemoryToolResult {
        let query = match input.query {
            Some(q) => q,
            None => return MemoryToolResult::error("query is required"),
        };
        let limit = input.limit.unwrap_or(10);

        let results = self.store.search(self.user_id, &query, limit).await;

        // Return JSON-formatted search results
        let json_content = self.store.search_json(self.user_id, &query, limit).await;

        let structured = json!({
            "count": results.len(),
            "entries": results.iter().map(|e| json!({
                "key": e.key,
                "category": format!("{:?}", e.category),
                "title": e.title,
                "content_preview": e.content.chars().take(200).collect::<String>()
            })).collect::<Vec<_>>()
        });

        MemoryToolResult::success(json_content).with_data(structured)
    }

    // ============================================
    // Helper Methods
    // ============================================

    fn format_with_line_numbers(&self, content: &str) -> String {
        content
            .lines()
            .enumerate()
            .map(|(i, line)| format!("{:>4} | {}", i + 1, line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn generate_index_content(&self) -> String {
        serde_json::to_string_pretty(&json!({
            "description": "Canal Memory System",
            "user_id": self.user_id.to_string(),
            "usage_guide": [
                "1. Use \"view\" to check existing memories",
                "2. Use \"create\" to save new information",
                "3. Use \"str_replace\" or \"insert\" to update files",
                "4. Use \"delete\" to remove outdated files",
                "5. Use \"search\" to find specific memories"
            ],
            "file_organization": {
                "preferences": "User preferences and settings",
                "patterns": "Learned patterns and behaviors",
                "projects": "Project-related context",
                "tasks": "Task progress and status",
                "knowledge": "Learned information",
                "conversations": "Conversation summaries",
                "working": "Temporary working memory"
            }
        }))
        .unwrap_or_default()
    }

    /// Get model family
    pub fn model_family(&self) -> ModelFamily {
        self.model_family
    }

    /// Get the underlying unified store
    pub fn store(&self) -> &Arc<UnifiedMemoryStore> {
        &self.store
    }

    /// Get user ID
    pub fn user_id(&self) -> Uuid {
        self.user_id
    }
}

// ============================================
// Convenience Functions
// ============================================

/// Get system prompt for any model
pub fn get_memory_system_prompt(model_name: &str) -> String {
    let family = ModelFamily::from_model_name(model_name);
    match family {
        ModelFamily::Claude => MemoryToolHandler::claude_system_prompt(),
        ModelFamily::Qwen => MemoryToolHandler::qwen_system_prompt(),
        ModelFamily::Gpt => MemoryToolHandler::gpt_system_prompt(),
        ModelFamily::DeepSeek => MemoryToolHandler::deepseek_system_prompt(),
        _ => MemoryToolHandler::generic_system_prompt(),
    }
}

/// Get tool definition for any model
pub fn get_memory_tool_definition(model_name: &str) -> Value {
    let family = ModelFamily::from_model_name(model_name);
    match family {
        ModelFamily::Claude => MemoryToolHandler::claude_tool_definition(),
        _ => MemoryToolHandler::universal_tool_definition(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_family_detection() {
        assert_eq!(
            ModelFamily::from_model_name("claude-3-opus"),
            ModelFamily::Claude
        );
        assert_eq!(
            ModelFamily::from_model_name("claude-sonnet-4"),
            ModelFamily::Claude
        );
        assert_eq!(
            ModelFamily::from_model_name("qwen-72b-chat"),
            ModelFamily::Qwen
        );
        assert_eq!(
            ModelFamily::from_model_name("qwen2.5-coder"),
            ModelFamily::Qwen
        );
        assert_eq!(ModelFamily::from_model_name("gpt-4o"), ModelFamily::Gpt);
        assert_eq!(ModelFamily::from_model_name("o1-preview"), ModelFamily::Gpt);
        assert_eq!(
            ModelFamily::from_model_name("gemini-pro"),
            ModelFamily::Gemini
        );
        assert_eq!(
            ModelFamily::from_model_name("deepseek-chat"),
            ModelFamily::DeepSeek
        );
        assert_eq!(
            ModelFamily::from_model_name("llama-3.1-70b"),
            ModelFamily::Llama
        );
        assert_eq!(
            ModelFamily::from_model_name("mistral-large"),
            ModelFamily::Mistral
        );
        assert_eq!(
            ModelFamily::from_model_name("unknown-model"),
            ModelFamily::Other
        );
    }

    #[test]
    fn test_path_validation() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let handler = MemoryToolHandler::new(Uuid::new_v4(), store);

        assert!(handler.validate_path("/memories/test.xml").is_ok());
        assert!(handler.validate_path("/memories/subdir/file.txt").is_ok());
        assert!(handler.validate_path("/etc/passwd").is_err());
        assert!(handler.validate_path("/memories/../etc/passwd").is_err());
    }

    #[tokio::test]
    async fn test_create_and_view() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let handler = MemoryToolHandler::new(Uuid::new_v4(), store);

        // Create a file
        let result = handler
            .handle(MemoryToolInput {
                command: "create".to_string(),
                path: Some("/memories/test.xml".to_string()),
                file_text: Some("<test>hello</test>".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.success, "Create failed: {}", result.content);

        // View the file
        let result = handler
            .handle(MemoryToolInput {
                command: "view".to_string(),
                path: Some("/memories/test.xml".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.success, "View failed: {}", result.content);
        assert!(result.content.contains("hello") || result.content.contains("test"));
    }

    #[tokio::test]
    async fn test_str_replace() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let handler = MemoryToolHandler::new(Uuid::new_v4(), store);

        // Create a file
        handler
            .handle(MemoryToolInput {
                command: "create".to_string(),
                path: Some("/memories/replace_test.xml".to_string()),
                file_text: Some("<content>old value</content>".to_string()),
                ..Default::default()
            })
            .await;

        // Replace content
        let result = handler
            .handle(MemoryToolInput {
                command: "str_replace".to_string(),
                path: Some("/memories/replace_test.xml".to_string()),
                old_str: Some("old value".to_string()),
                new_str: Some("new value".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.success, "Replace failed: {}", result.content);

        // View and verify
        let result = handler
            .handle(MemoryToolInput {
                command: "view".to_string(),
                path: Some("/memories/replace_test.xml".to_string()),
                ..Default::default()
            })
            .await;
        assert!(result.content.contains("new value"));
    }

    #[tokio::test]
    async fn test_search() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let handler = MemoryToolHandler::new(Uuid::new_v4(), store);

        // Create some files
        handler
            .handle(MemoryToolInput {
                command: "create".to_string(),
                path: Some("/memories/search_test1.xml".to_string()),
                file_text: Some("Hello world from file one".to_string()),
                ..Default::default()
            })
            .await;

        handler
            .handle(MemoryToolInput {
                command: "create".to_string(),
                path: Some("/memories/search_test2.xml".to_string()),
                file_text: Some("Hello universe from file two".to_string()),
                ..Default::default()
            })
            .await;

        // Search
        let result = handler
            .handle(MemoryToolInput {
                command: "search".to_string(),
                query: Some("Hello".to_string()),
                limit: Some(10),
                ..Default::default()
            })
            .await;
        assert!(result.success, "Search failed: {}", result.content);
    }
}

impl Default for MemoryToolInput {
    fn default() -> Self {
        Self {
            command: String::new(),
            path: None,
            view_range: None,
            file_text: None,
            old_str: None,
            new_str: None,
            insert_line: None,
            insert_text: None,
            old_path: None,
            new_path: None,
            query: None,
            limit: None,
        }
    }
}
