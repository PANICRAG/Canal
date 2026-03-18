//! Artifact Extraction - Extract artifacts from assistant responses
//!
//! Parses assistant messages to detect and extract:
//! - Code blocks (fenced with ```)
//! - Special artifact markers
//! - Markdown documents

use regex::Regex;
use uuid::Uuid;

use super::artifact::{
    ArtifactContent, ArtifactType, CodeBlockContent, DocumentContent, DocumentFormat,
    StoredArtifact,
};

/// Artifact extractor configuration
#[derive(Debug, Clone)]
pub struct ArtifactExtractorConfig {
    /// Minimum code block lines to create artifact
    pub min_code_lines: usize,
    /// Minimum document length to create artifact
    pub min_document_length: usize,
    /// Whether to extract code blocks
    pub extract_code_blocks: bool,
    /// Whether to extract documents
    pub extract_documents: bool,
    /// Whether to parse artifact markers
    pub parse_artifact_markers: bool,
}

impl Default for ArtifactExtractorConfig {
    fn default() -> Self {
        Self {
            min_code_lines: 3,
            min_document_length: 100,
            extract_code_blocks: true,
            extract_documents: true,
            parse_artifact_markers: true,
        }
    }
}

/// Extracted artifact with position information
#[derive(Debug, Clone)]
pub struct ExtractedArtifact {
    pub artifact: StoredArtifact,
    pub start_offset: usize,
    pub end_offset: usize,
}

/// Artifact extractor
pub struct ArtifactExtractor {
    config: ArtifactExtractorConfig,
    code_block_regex: Regex,
    artifact_marker_regex: Regex,
}

impl ArtifactExtractor {
    /// Create a new artifact extractor
    pub fn new(config: ArtifactExtractorConfig) -> Self {
        Self {
            config,
            // Match fenced code blocks with optional language
            code_block_regex: Regex::new(r"(?s)```(\w*)\n(.*?)```").unwrap(),
            // Match artifact markers: <artifact type="..." title="...">content</artifact>
            artifact_marker_regex: Regex::new(
                r#"(?s)<artifact\s+type="([^"]+)"\s+title="([^"]+)"(?:\s+[^>]*)?>(.+?)</artifact>"#,
            )
            .unwrap(),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(ArtifactExtractorConfig::default())
    }

    /// Extract all artifacts from content
    pub fn extract(
        &self,
        content: &str,
        session_id: Option<Uuid>,
        message_id: Option<Uuid>,
    ) -> Vec<ExtractedArtifact> {
        let mut artifacts = Vec::new();

        // Extract artifact markers first (they take precedence)
        if self.config.parse_artifact_markers {
            artifacts.extend(self.extract_artifact_markers(content, session_id, message_id));
        }

        // Extract code blocks (skip if already extracted via marker)
        if self.config.extract_code_blocks {
            let code_artifacts = self.extract_code_blocks(content, session_id, message_id);
            for code_artifact in code_artifacts {
                // Check if this region overlaps with an existing artifact
                let overlaps = artifacts.iter().any(|a| {
                    (code_artifact.start_offset >= a.start_offset
                        && code_artifact.start_offset < a.end_offset)
                        || (code_artifact.end_offset > a.start_offset
                            && code_artifact.end_offset <= a.end_offset)
                });
                if !overlaps {
                    artifacts.push(code_artifact);
                }
            }
        }

        // Sort by position
        artifacts.sort_by_key(|a| a.start_offset);
        artifacts
    }

    /// Extract code blocks from content
    fn extract_code_blocks(
        &self,
        content: &str,
        session_id: Option<Uuid>,
        message_id: Option<Uuid>,
    ) -> Vec<ExtractedArtifact> {
        let mut artifacts = Vec::new();

        for cap in self.code_block_regex.captures_iter(content) {
            let full_match = cap.get(0).unwrap();
            let language = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let code = cap.get(2).map(|m| m.as_str()).unwrap_or("");

            // Check minimum line count
            let line_count = code.lines().count();
            if line_count < self.config.min_code_lines {
                continue;
            }

            // Determine language and title
            let language = if language.is_empty() {
                "text".to_string()
            } else {
                language.to_string()
            };

            let title = Self::generate_code_title(&language, code);

            let mut artifact = StoredArtifact::code_block(&title, &language, code.trim());
            if let Some(sid) = session_id {
                artifact = artifact.with_session_id(sid);
            }
            if let Some(mid) = message_id {
                artifact = artifact.with_message_id(mid);
            }

            artifacts.push(ExtractedArtifact {
                artifact,
                start_offset: full_match.start(),
                end_offset: full_match.end(),
            });
        }

        artifacts
    }

    /// Extract artifact markers from content
    fn extract_artifact_markers(
        &self,
        content: &str,
        session_id: Option<Uuid>,
        message_id: Option<Uuid>,
    ) -> Vec<ExtractedArtifact> {
        let mut artifacts = Vec::new();

        for cap in self.artifact_marker_regex.captures_iter(content) {
            let full_match = cap.get(0).unwrap();
            let type_str = cap.get(1).map(|m| m.as_str()).unwrap_or("document");
            let title = cap.get(2).map(|m| m.as_str()).unwrap_or("Untitled");
            let inner_content = cap.get(3).map(|m| m.as_str()).unwrap_or("");

            let artifact_type = Self::parse_artifact_type(type_str);
            let content = Self::create_artifact_content(&artifact_type, inner_content.trim());

            let mut artifact = StoredArtifact::new(artifact_type, title, content);
            if let Some(sid) = session_id {
                artifact = artifact.with_session_id(sid);
            }
            if let Some(mid) = message_id {
                artifact = artifact.with_message_id(mid);
            }

            artifacts.push(ExtractedArtifact {
                artifact,
                start_offset: full_match.start(),
                end_offset: full_match.end(),
            });
        }

        artifacts
    }

    /// Generate a title for a code block
    fn generate_code_title(language: &str, code: &str) -> String {
        // Try to extract function/class name for common languages
        let title = match language {
            "rust" => Self::extract_rust_name(code),
            "python" | "py" => Self::extract_python_name(code),
            "javascript" | "js" | "typescript" | "ts" => Self::extract_js_name(code),
            "go" => Self::extract_go_name(code),
            _ => None,
        };

        title.unwrap_or_else(|| format!("{} code", Self::language_display_name(language)))
    }

    /// Get display name for language
    fn language_display_name(language: &str) -> &str {
        match language {
            "rust" | "rs" => "Rust",
            "python" | "py" => "Python",
            "javascript" | "js" => "JavaScript",
            "typescript" | "ts" => "TypeScript",
            "go" => "Go",
            "java" => "Java",
            "cpp" | "c++" => "C++",
            "c" => "C",
            "bash" | "sh" => "Shell",
            "sql" => "SQL",
            "html" => "HTML",
            "css" => "CSS",
            "json" => "JSON",
            "yaml" | "yml" => "YAML",
            "toml" => "TOML",
            "markdown" | "md" => "Markdown",
            "text" => "Text",
            _ => language,
        }
    }

    // R3-H9: Compile regexes once via OnceLock instead of per-call
    fn match_first_capture(regexes: &[Regex], code: &str) -> Option<String> {
        for re in regexes {
            if let Some(cap) = re.captures(code) {
                if let Some(name) = cap.get(1) {
                    return Some(name.as_str().to_string());
                }
            }
        }
        None
    }

    /// Extract function/struct name from Rust code
    fn extract_rust_name(code: &str) -> Option<String> {
        use std::sync::OnceLock;
        static RUST_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
        let regexes = RUST_REGEXES.get_or_init(|| {
            vec![
                Regex::new(r"(?m)^(?:pub\s+)?fn\s+(\w+)").unwrap(),
                Regex::new(r"(?m)^(?:pub\s+)?struct\s+(\w+)").unwrap(),
                Regex::new(r"(?m)^(?:pub\s+)?enum\s+(\w+)").unwrap(),
                Regex::new(r"(?m)^(?:pub\s+)?trait\s+(\w+)").unwrap(),
                Regex::new(r"(?m)^impl\s+(?:\w+\s+for\s+)?(\w+)").unwrap(),
            ]
        });
        Self::match_first_capture(regexes, code)
    }

    /// Extract function/class name from Python code
    fn extract_python_name(code: &str) -> Option<String> {
        use std::sync::OnceLock;
        static PYTHON_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
        let regexes = PYTHON_REGEXES.get_or_init(|| {
            vec![
                Regex::new(r"(?m)^def\s+(\w+)").unwrap(),
                Regex::new(r"(?m)^class\s+(\w+)").unwrap(),
            ]
        });
        Self::match_first_capture(regexes, code)
    }

    /// Extract function/class name from JavaScript/TypeScript code
    fn extract_js_name(code: &str) -> Option<String> {
        use std::sync::OnceLock;
        static JS_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
        let regexes = JS_REGEXES.get_or_init(|| {
            vec![
                Regex::new(r"(?m)^(?:export\s+)?(?:async\s+)?function\s+(\w+)").unwrap(),
                Regex::new(r"(?m)^(?:export\s+)?class\s+(\w+)").unwrap(),
                Regex::new(r"(?m)^(?:export\s+)?const\s+(\w+)\s*=").unwrap(),
                Regex::new(r"(?m)^(?:export\s+)?interface\s+(\w+)").unwrap(),
            ]
        });
        Self::match_first_capture(regexes, code)
    }

    /// Extract function/struct name from Go code
    fn extract_go_name(code: &str) -> Option<String> {
        use std::sync::OnceLock;
        static GO_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
        let regexes = GO_REGEXES.get_or_init(|| {
            vec![
                Regex::new(r"(?m)^func\s+(?:\(\w+\s+\*?\w+\)\s+)?(\w+)").unwrap(),
                Regex::new(r"(?m)^type\s+(\w+)\s+struct").unwrap(),
                Regex::new(r"(?m)^type\s+(\w+)\s+interface").unwrap(),
            ]
        });
        Self::match_first_capture(regexes, code)
    }

    /// Parse artifact type from string
    fn parse_artifact_type(type_str: &str) -> ArtifactType {
        match type_str.to_lowercase().as_str() {
            "document" | "doc" | "markdown" | "md" => ArtifactType::Document,
            "code" | "code_block" | "codeblock" => ArtifactType::CodeBlock,
            "chart" | "visualization" => ArtifactType::Chart,
            "table" => ArtifactType::Table,
            "image" | "img" => ArtifactType::Image,
            "timeline" => ArtifactType::Timeline,
            "gallery" | "image_gallery" => ArtifactType::ImageGallery,
            "video" | "video_preview" => ArtifactType::VideoPreview,
            _ => ArtifactType::Document,
        }
    }

    /// Create artifact content from type and raw content
    fn create_artifact_content(artifact_type: &ArtifactType, content: &str) -> ArtifactContent {
        match artifact_type {
            ArtifactType::Document => ArtifactContent::Document(DocumentContent {
                format: DocumentFormat::Markdown,
                content: content.to_string(),
                sections: Vec::new(),
            }),
            ArtifactType::CodeBlock => {
                // Try to detect language from first line or default to text
                let (language, code) = if content.starts_with("```") {
                    let lines: Vec<&str> = content.lines().collect();
                    if lines.len() >= 2 {
                        let lang = lines[0].trim_start_matches("```").trim();
                        let code = lines[1..lines.len() - 1].join("\n");
                        (if lang.is_empty() { "text" } else { lang }, code)
                    } else {
                        ("text", content.to_string())
                    }
                } else {
                    ("text", content.to_string())
                };

                ArtifactContent::CodeBlock(CodeBlockContent {
                    language: language.to_string(),
                    code,
                    filename: None,
                    highlights: Vec::new(),
                })
            }
            // For other types, default to document content
            _ => ArtifactContent::Document(DocumentContent {
                format: DocumentFormat::Markdown,
                content: content.to_string(),
                sections: Vec::new(),
            }),
        }
    }
}

impl Default for ArtifactExtractor {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_code_block() {
        let extractor = ArtifactExtractor::with_defaults();
        let content = r#"Here's some code:

```rust
fn main() {
    println!("Hello, world!");
}
```

And that's it."#;

        let artifacts = extractor.extract(content, None, None);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact.artifact_type, ArtifactType::CodeBlock);
        assert_eq!(artifacts[0].artifact.title, "main");

        if let ArtifactContent::CodeBlock(code) = &artifacts[0].artifact.content {
            assert_eq!(code.language, "rust");
            assert!(code.code.contains("println!"));
        } else {
            panic!("Expected CodeBlock content");
        }
    }

    #[test]
    fn test_extract_multiple_code_blocks() {
        let extractor = ArtifactExtractor::new(ArtifactExtractorConfig {
            min_code_lines: 2, // Lower the threshold for this test
            ..Default::default()
        });
        let content = r#"
```python
def hello():
    print("Hello")
```

```javascript
function greet() {
    console.log("Hi");
}
```
"#;

        let artifacts = extractor.extract(content, None, None);
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].artifact.title, "hello");
        assert_eq!(artifacts[1].artifact.title, "greet");
    }

    #[test]
    fn test_skip_small_code_blocks() {
        let extractor = ArtifactExtractor::new(ArtifactExtractorConfig {
            min_code_lines: 3,
            ..Default::default()
        });

        let content = r#"
Short code:
```bash
echo "hi"
```
"#;

        let artifacts = extractor.extract(content, None, None);
        assert_eq!(artifacts.len(), 0);
    }

    #[test]
    fn test_extract_artifact_marker() {
        let extractor = ArtifactExtractor::with_defaults();
        let content = r#"
Here's a document:

<artifact type="document" title="Project README">
# My Project

This is a great project.
</artifact>

Hope you like it!
"#;

        let artifacts = extractor.extract(content, None, None);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact.artifact_type, ArtifactType::Document);
        assert_eq!(artifacts[0].artifact.title, "Project README");
    }

    #[test]
    fn test_extract_with_session_and_message() {
        let extractor = ArtifactExtractor::with_defaults();
        let session_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();

        let content = r#"
```rust
fn test() {
    // Test function
}
```
"#;

        let artifacts = extractor.extract(content, Some(session_id), Some(message_id));
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact.session_id, Some(session_id));
        assert_eq!(artifacts[0].artifact.message_id, Some(message_id));
    }

    #[test]
    fn test_generate_code_title_rust() {
        assert_eq!(
            ArtifactExtractor::generate_code_title(
                "rust",
                "pub struct MyStruct {\n    field: i32\n}"
            ),
            "MyStruct"
        );
        assert_eq!(
            ArtifactExtractor::generate_code_title("rust", "fn calculate() -> i32 {\n    42\n}"),
            "calculate"
        );
        assert_eq!(
            ArtifactExtractor::generate_code_title("rust", "impl Display for MyType {}"),
            "MyType"
        );
    }

    #[test]
    fn test_generate_code_title_python() {
        assert_eq!(
            ArtifactExtractor::generate_code_title("python", "class MyClass:\n    pass"),
            "MyClass"
        );
        assert_eq!(
            ArtifactExtractor::generate_code_title("python", "def my_function():\n    pass"),
            "my_function"
        );
    }

    #[test]
    fn test_generate_code_title_javascript() {
        assert_eq!(
            ArtifactExtractor::generate_code_title(
                "javascript",
                "export function myFunc() {\n    return 42;\n}"
            ),
            "myFunc"
        );
        assert_eq!(
            ArtifactExtractor::generate_code_title(
                "typescript",
                "export interface Config {\n    name: string;\n}"
            ),
            "Config"
        );
    }

    #[test]
    fn test_generate_code_title_go() {
        assert_eq!(
            ArtifactExtractor::generate_code_title("go", "func (s *Server) HandleRequest() {\n}"),
            "HandleRequest"
        );
        assert_eq!(
            ArtifactExtractor::generate_code_title(
                "go",
                "type Config struct {\n    Name string\n}"
            ),
            "Config"
        );
    }

    #[test]
    fn test_parse_artifact_type() {
        assert_eq!(
            ArtifactExtractor::parse_artifact_type("document"),
            ArtifactType::Document
        );
        assert_eq!(
            ArtifactExtractor::parse_artifact_type("code_block"),
            ArtifactType::CodeBlock
        );
        assert_eq!(
            ArtifactExtractor::parse_artifact_type("chart"),
            ArtifactType::Chart
        );
        assert_eq!(
            ArtifactExtractor::parse_artifact_type("unknown"),
            ArtifactType::Document
        );
    }

    #[test]
    fn test_artifact_marker_takes_precedence() {
        let extractor = ArtifactExtractor::with_defaults();
        let content = r#"
<artifact type="code_block" title="Custom Title">
```rust
fn example() {
    // code here
}
```
</artifact>
"#;

        let artifacts = extractor.extract(content, None, None);
        // Should only extract one artifact (the marker), not the code block inside
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact.title, "Custom Title");
    }
}
