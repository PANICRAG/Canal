//! File Operations Tools - Read, Write, Edit

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// ============================================================================
// Read Tool
// ============================================================================

/// Read tool input
#[derive(Debug, Clone, Deserialize)]
pub struct ReadInput {
    /// The file path to read
    pub file_path: String,
    /// Line offset to start reading from (1-indexed)
    #[serde(default)]
    pub offset: Option<u32>,
    /// Number of lines to read
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Read tool output
#[derive(Debug, Clone, Serialize)]
pub struct ReadOutput {
    /// File content with line numbers
    pub content: String,
    /// Total number of lines in the file
    pub total_lines: u32,
    /// Number of lines returned
    pub lines_returned: u32,
    /// Whether content was truncated
    pub truncated: bool,
}

/// Read file tool
pub struct ReadTool {
    /// Maximum lines to read by default
    pub max_lines: u32,
    /// Maximum characters per line
    pub max_chars_per_line: usize,
}

impl Default for ReadTool {
    fn default() -> Self {
        Self {
            max_lines: 2000,
            max_chars_per_line: 2000,
        }
    }
}

#[async_trait]
impl AgentTool for ReadTool {
    type Input = ReadInput;
    type Output = ReadOutput;

    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        r#"Reads a file from the local filesystem. You can access any file directly by using this tool.

Usage:
- The file_path parameter must be an absolute path, not a relative path
- By default, it reads up to 2000 lines starting from the beginning of the file
- You can optionally specify a line offset and limit (especially handy for long files), but it's recommended to read the whole file by not providing these parameters
- Any lines longer than 2000 characters will be truncated
- Results are returned using cat -n format, with line numbers starting at 1
- This tool allows reading images (eg PNG, JPG, etc). When reading an image file the contents are presented visually as this is a multimodal LLM.
- This tool can read PDF files (.pdf). PDFs are processed page by page, extracting both text and visual content for analysis.
- This tool can only read files, not directories. To read a directory, use the Bash tool with ls command.
- You can call multiple tools in a single response. It is always better to speculatively read multiple potentially useful files in parallel.

IMPORTANT: Prefer editing existing files over creating new ones. Always read a file before editing it."#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-indexed)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read"
                }
            },
            "required": ["file_path"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "filesystem"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        let path = context.resolve_path(&input.file_path);

        // Check if path is allowed
        if !context.is_path_allowed(&path) {
            return Err(ToolError::PermissionDenied(format!(
                "Path not in allowed directories: {}",
                path.display()
            )));
        }

        // Check if file exists
        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "File not found: {}",
                path.display()
            )));
        }

        // Read file
        let file = fs::File::open(&path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        let offset = input.offset.unwrap_or(1).max(1) as usize;
        let limit = input.limit.unwrap_or(self.max_lines).min(self.max_lines) as usize;

        let mut content = String::new();
        let mut line_number = 0u32;
        let mut lines_returned = 0u32;
        let mut total_lines = 0u32;

        while let Ok(Some(line)) = lines.next_line().await {
            total_lines += 1;
            line_number += 1;

            if line_number < offset as u32 {
                continue;
            }

            if lines_returned >= limit as u32 {
                continue; // Keep counting total lines
            }

            // Truncate long lines
            let truncated_line = if line.len() > self.max_chars_per_line {
                format!("{}...", &line[..self.max_chars_per_line])
            } else {
                line
            };

            // Format with line number (matching cat -n format)
            content.push_str(&format!("{:>6}\t{}\n", line_number, truncated_line));
            lines_returned += 1;
        }

        let truncated = lines_returned < (total_lines - (offset as u32 - 1)).min(limit as u32);

        Ok(ReadOutput {
            content,
            total_lines,
            lines_returned,
            truncated,
        })
    }
}

// ============================================================================
// Write Tool
// ============================================================================

/// Write tool input
#[derive(Debug, Clone, Deserialize)]
pub struct WriteInput {
    /// The file path to write to
    pub file_path: String,
    /// The content to write
    pub content: String,
}

/// Write tool output
#[derive(Debug, Clone, Serialize)]
pub struct WriteOutput {
    /// Success message
    pub message: String,
    /// Number of bytes written
    pub bytes_written: u64,
}

/// Write file tool
pub struct WriteTool;

#[async_trait]
impl AgentTool for WriteTool {
    type Input = WriteInput;
    type Output = WriteOutput;

    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self) -> &str {
        r#"Writes a file to the local filesystem.

Usage:
- This tool will overwrite the existing file if there is one at the provided path.
- If this is an existing file, you MUST use the Read tool first to read the file's contents. This tool will fail if you did not read the file first.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- NEVER proactively create documentation files (*.md) or README files. Only create documentation files if explicitly requested by the User.
- Only use emojis if the user explicitly requests it. Avoid writing emojis to files unless asked.

Security:
- Never write files containing secrets or credentials
- Never overwrite system files
- Always verify the target directory exists before writing"#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "filesystem"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        // Check if mutations are allowed
        if !context.allows_mutations() {
            return Err(ToolError::PermissionDenied(
                "Write operations not allowed in current permission mode".to_string(),
            ));
        }

        let path = context.resolve_path(&input.file_path);

        // Check if path is allowed
        if !context.is_path_allowed(&path) {
            return Err(ToolError::PermissionDenied(format!(
                "Path not in allowed directories: {}",
                path.display()
            )));
        }

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Write file
        let mut file = fs::File::create(&path).await?;
        file.write_all(input.content.as_bytes()).await?;
        file.flush().await?;

        let bytes_written = input.content.len() as u64;

        Ok(WriteOutput {
            message: format!("Successfully wrote to {}", path.display()),
            bytes_written,
        })
    }
}

// ============================================================================
// Edit Tool
// ============================================================================

/// Edit tool input
#[derive(Debug, Clone, Deserialize)]
pub struct EditInput {
    /// The file path to edit
    pub file_path: String,
    /// The text to replace
    pub old_string: String,
    /// The replacement text
    pub new_string: String,
    /// Whether to replace all occurrences
    #[serde(default)]
    pub replace_all: bool,
}

/// Edit tool output
#[derive(Debug, Clone, Serialize)]
pub struct EditOutput {
    /// Success message
    pub message: String,
    /// Number of replacements made
    pub replacements: u32,
}

/// Edit file tool
pub struct EditTool;

#[async_trait]
impl AgentTool for EditTool {
    type Input = EditInput;
    type Output = EditOutput;

    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> &str {
        r#"Performs exact string replacements in files.

Usage:
- You must use your Read tool at least once in the conversation before editing. This tool will error if you attempt an edit without reading the file.
- When editing text from Read tool output, ensure you preserve the exact indentation (tabs/spaces) as it appears AFTER the line number prefix. The line number prefix format is: spaces + line number + tab. Everything after that tab is the actual file content to match. Never include any part of the line number prefix in the old_string or new_string.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- The edit will FAIL if old_string is not unique in the file. Either provide a larger string with more surrounding context to make it unique or use replace_all to change every instance of old_string.
- Use replace_all for replacing and renaming strings across the file. This parameter is useful if you want to rename a variable for instance.

Best Practices:
- Include enough context in old_string to make the match unique
- Preserve exact whitespace and indentation
- For multi-line edits, include line breaks in the strings"#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Whether to replace all occurrences (default: false)"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "filesystem"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        // Check if mutations are allowed
        if !context.allows_mutations() {
            return Err(ToolError::PermissionDenied(
                "Edit operations not allowed in current permission mode".to_string(),
            ));
        }

        let path = context.resolve_path(&input.file_path);

        // Check if path is allowed
        if !context.is_path_allowed(&path) {
            return Err(ToolError::PermissionDenied(format!(
                "Path not in allowed directories: {}",
                path.display()
            )));
        }

        // Check if file exists
        if !path.exists() {
            return Err(ToolError::NotFound(format!(
                "File not found: {}",
                path.display()
            )));
        }

        // Read current content
        let content = fs::read_to_string(&path).await?;

        // Check for old_string
        let occurrences = content.matches(&input.old_string).count();

        if occurrences == 0 {
            return Err(ToolError::InvalidInput(format!(
                "old_string not found in file: {:?}",
                input.old_string
            )));
        }

        if occurrences > 1 && !input.replace_all {
            return Err(ToolError::InvalidInput(format!(
                "old_string found {} times. Use replace_all=true or provide more context to make it unique.",
                occurrences
            )));
        }

        // Perform replacement
        let new_content = if input.replace_all {
            content.replace(&input.old_string, &input.new_string)
        } else {
            content.replacen(&input.old_string, &input.new_string, 1)
        };

        // Write back
        fs::write(&path, &new_content).await?;

        let replacements = if input.replace_all {
            occurrences as u32
        } else {
            1
        };

        Ok(EditOutput {
            message: format!(
                "Successfully edited {} ({} replacement{})",
                path.display(),
                replacements,
                if replacements == 1 { "" } else { "s" }
            ),
            replacements,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_tool() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        std::fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let tool = ReadTool::default();
        let context =
            ToolContext::new("s1", temp_dir.path()).with_allowed_directory(temp_dir.path());

        let input = ReadInput {
            file_path: file_path.to_string_lossy().to_string(),
            offset: None,
            limit: None,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert_eq!(output.total_lines, 3);
        assert_eq!(output.lines_returned, 3);
        assert!(output.content.contains("line 1"));
    }

    #[tokio::test]
    async fn test_write_tool() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("new_file.txt");

        let tool = WriteTool;
        let context = ToolContext::new("s1", temp_dir.path())
            .with_allowed_directory(temp_dir.path())
            .with_permission_mode(crate::agent::types::PermissionMode::AcceptEdits);

        let input = WriteInput {
            file_path: file_path.to_string_lossy().to_string(),
            content: "Hello, World!".to_string(),
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert_eq!(output.bytes_written, 13);
        assert!(file_path.exists());
    }

    #[tokio::test]
    async fn test_edit_tool() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("edit_test.txt");
        std::fs::write(&file_path, "Hello, World!").unwrap();

        let tool = EditTool;
        let context = ToolContext::new("s1", temp_dir.path())
            .with_allowed_directory(temp_dir.path())
            .with_permission_mode(crate::agent::types::PermissionMode::AcceptEdits);

        let input = EditInput {
            file_path: file_path.to_string_lossy().to_string(),
            old_string: "World".to_string(),
            new_string: "Rust".to_string(),
            replace_all: false,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert_eq!(output.replacements, 1);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello, Rust!");
    }

    #[tokio::test]
    async fn test_edit_tool_multiple_not_allowed() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("multi.txt");
        std::fs::write(&file_path, "foo bar foo").unwrap();

        let tool = EditTool;
        let context = ToolContext::new("s1", temp_dir.path())
            .with_allowed_directory(temp_dir.path())
            .with_permission_mode(crate::agent::types::PermissionMode::AcceptEdits);

        let input = EditInput {
            file_path: file_path.to_string_lossy().to_string(),
            old_string: "foo".to_string(),
            new_string: "baz".to_string(),
            replace_all: false,
        };

        let result = tool.execute(input, &context).await;
        assert!(matches!(result, Err(ToolError::InvalidInput(_))));
    }
}
