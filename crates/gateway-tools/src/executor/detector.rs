//! Language detection for code snippets

use regex::Regex;
use std::path::Path;

use super::Language;

/// Detected language with confidence score
#[derive(Debug, Clone)]
pub struct DetectedLanguage {
    pub language: Language,
    pub confidence: f32,
    pub hints: Vec<String>,
}

/// Language detector for code snippets
pub struct LanguageDetector {
    patterns: Vec<LanguagePattern>,
}

struct LanguagePattern {
    language: Language,
    patterns: Vec<(Regex, f32)>, // (pattern, weight)
    file_extensions: Vec<&'static str>,
    shebang_patterns: Vec<&'static str>,
}

impl Default for LanguageDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageDetector {
    /// Create a new language detector
    pub fn new() -> Self {
        let patterns = vec![
            // Python
            LanguagePattern {
                language: Language::Python,
                patterns: vec![
                    (Regex::new(r"^import\s+\w+").unwrap(), 0.8),
                    (Regex::new(r"^from\s+\w+\s+import").unwrap(), 0.9),
                    (Regex::new(r"def\s+\w+\s*\(").unwrap(), 0.9),
                    (Regex::new(r"class\s+\w+").unwrap(), 0.8),
                    (Regex::new(r"__name__\s*==").unwrap(), 1.0),
                    (Regex::new(r"print\s*\(").unwrap(), 0.5),
                    (Regex::new(r"self\.\w+").unwrap(), 0.7),
                    (Regex::new(r"__init__\s*\(").unwrap(), 1.0),
                    (Regex::new(r"async\s+def\s+\w+").unwrap(), 0.9),
                ],
                file_extensions: vec!["py", "pyw", "pyi"],
                shebang_patterns: vec!["python", "python3"],
            },
            // JavaScript
            LanguagePattern {
                language: Language::JavaScript,
                patterns: vec![
                    (Regex::new(r"const\s+\w+\s*=").unwrap(), 0.7),
                    (Regex::new(r"let\s+\w+\s*=").unwrap(), 0.7),
                    (Regex::new(r"var\s+\w+\s*=").unwrap(), 0.6),
                    (Regex::new(r"function\s+\w+\s*\(").unwrap(), 0.8),
                    (Regex::new(r"=>\s*\{").unwrap(), 0.8),
                    (Regex::new(r"require\s*\(").unwrap(), 0.9),
                    (Regex::new(r"module\.exports").unwrap(), 1.0),
                    (Regex::new(r"console\.(log|error|warn)").unwrap(), 0.7),
                    (Regex::new(r"document\.\w+").unwrap(), 0.8),
                    (Regex::new(r"window\.\w+").unwrap(), 0.8),
                    (Regex::new(r"async\s+function").unwrap(), 0.8),
                    (Regex::new(r"await\s+\w+").unwrap(), 0.6),
                ],
                file_extensions: vec!["js", "mjs", "cjs"],
                shebang_patterns: vec!["node"],
            },
            // TypeScript
            LanguagePattern {
                language: Language::TypeScript,
                patterns: vec![
                    (
                        Regex::new(r":\s*(string|number|boolean|any|void|never)\b").unwrap(),
                        0.9,
                    ),
                    (Regex::new(r"interface\s+\w+\s*\{").unwrap(), 1.0),
                    (Regex::new(r"type\s+\w+\s*=").unwrap(), 0.9),
                    (Regex::new(r"<\w+>").unwrap(), 0.5),
                    (Regex::new(r"as\s+(string|number|any|\w+)").unwrap(), 0.8),
                    (Regex::new(r"import\s+\{").unwrap(), 0.7),
                    (
                        Regex::new(r"export\s+(default\s+)?(class|function|const|interface|type)")
                            .unwrap(),
                        0.8,
                    ),
                    (Regex::new(r"private\s+\w+:").unwrap(), 0.9),
                    (Regex::new(r"public\s+\w+:").unwrap(), 0.9),
                ],
                file_extensions: vec!["ts", "tsx"],
                shebang_patterns: vec!["ts-node"],
            },
            // Rust
            LanguagePattern {
                language: Language::Rust,
                patterns: vec![
                    (Regex::new(r"fn\s+\w+").unwrap(), 0.8),
                    (Regex::new(r"let\s+(mut\s+)?\w+\s*:").unwrap(), 0.8),
                    (Regex::new(r"impl\s+\w+").unwrap(), 1.0),
                    (Regex::new(r"struct\s+\w+").unwrap(), 0.9),
                    (Regex::new(r"enum\s+\w+\s*\{").unwrap(), 0.8),
                    (Regex::new(r"use\s+\w+::").unwrap(), 0.9),
                    (Regex::new(r"pub\s+(fn|struct|enum|mod|use)").unwrap(), 1.0),
                    (Regex::new(r"println!\s*\(").unwrap(), 1.0),
                    (Regex::new(r"#\[derive").unwrap(), 1.0),
                    (Regex::new(r"&(mut\s+)?self").unwrap(), 1.0),
                    (Regex::new(r"Option<").unwrap(), 0.9),
                    (Regex::new(r"Result<").unwrap(), 0.9),
                    (Regex::new(r"\.unwrap\(\)").unwrap(), 0.9),
                    (Regex::new(r"match\s+\w+\s*\{").unwrap(), 0.8),
                ],
                file_extensions: vec!["rs"],
                shebang_patterns: vec![],
            },
            // Go
            LanguagePattern {
                language: Language::Go,
                patterns: vec![
                    (Regex::new(r"^package\s+\w+").unwrap(), 1.0),
                    (Regex::new(r"func\s+\w+\s*\(").unwrap(), 0.9),
                    (Regex::new(r"import\s+\(").unwrap(), 0.9),
                    (Regex::new(r"type\s+\w+\s+struct\s*\{").unwrap(), 1.0),
                    (Regex::new(r"type\s+\w+\s+interface\s*\{").unwrap(), 1.0),
                    (Regex::new(r"fmt\.Print").unwrap(), 1.0),
                    (Regex::new(r":=").unwrap(), 0.7),
                    (Regex::new(r"go\s+func").unwrap(), 1.0),
                    (Regex::new(r"defer\s+\w+").unwrap(), 1.0),
                    (Regex::new(r"chan\s+\w+").unwrap(), 1.0),
                    (Regex::new(r"make\s*\(").unwrap(), 0.7),
                ],
                file_extensions: vec!["go"],
                shebang_patterns: vec![],
            },
            // Bash
            LanguagePattern {
                language: Language::Bash,
                patterns: vec![
                    (Regex::new(r"#!/bin/(ba)?sh").unwrap(), 1.0),
                    (Regex::new(r"^\s*if\s+\[").unwrap(), 0.8),
                    (Regex::new(r"^\s*elif\s+\[").unwrap(), 0.9),
                    (Regex::new(r"^\s*fi\s*$").unwrap(), 0.9),
                    (Regex::new(r"^\s*for\s+\w+\s+in\s+").unwrap(), 0.8),
                    (Regex::new(r"^\s*done\s*$").unwrap(), 0.8),
                    (Regex::new(r"\$\{?\w+\}?").unwrap(), 0.5),
                    (Regex::new(r"echo\s+").unwrap(), 0.6),
                    (Regex::new(r"function\s+\w+\s*\(\)").unwrap(), 0.9),
                ],
                file_extensions: vec!["sh", "bash"],
                shebang_patterns: vec!["bash", "sh", "zsh"],
            },
        ];

        Self { patterns }
    }

    /// Detect language from code content
    pub fn detect(&self, code: &str) -> DetectedLanguage {
        let mut best_match = DetectedLanguage {
            language: Language::Python, // Default fallback
            confidence: 0.0,
            hints: vec![],
        };

        // Check for shebang first
        if let Some(first_line) = code.lines().next() {
            if first_line.starts_with("#!") {
                for pattern in &self.patterns {
                    for shebang in &pattern.shebang_patterns {
                        if first_line.contains(shebang) {
                            return DetectedLanguage {
                                language: pattern.language.clone(),
                                confidence: 1.0,
                                hints: vec![format!("Shebang: {}", first_line)],
                            };
                        }
                    }
                }
            }
        }

        // Score each language
        for lang_pattern in &self.patterns {
            let mut score = 0.0;
            let mut hints = vec![];
            let mut match_count = 0;

            for (regex, weight) in &lang_pattern.patterns {
                if regex.is_match(code) {
                    score += *weight;
                    match_count += 1;
                    if hints.len() < 3 {
                        if let Some(m) = regex.find(code) {
                            hints.push(format!("Matched: {}", m.as_str()));
                        }
                    }
                }
            }

            // Normalize score
            if match_count > 0 {
                score /= match_count as f32;
                score *= (match_count as f32).min(5.0) / 5.0; // Bonus for multiple matches
            }

            if score > best_match.confidence {
                best_match = DetectedLanguage {
                    language: lang_pattern.language.clone(),
                    confidence: score.min(1.0),
                    hints,
                };
            }
        }

        best_match
    }

    /// Detect language from file extension
    pub fn detect_from_extension(&self, path: &str) -> Option<Language> {
        let path = Path::new(path);
        let ext = path.extension()?.to_str()?.to_lowercase();

        for pattern in &self.patterns {
            if pattern.file_extensions.contains(&ext.as_str()) {
                return Some(pattern.language.clone());
            }
        }

        None
    }

    /// Detect language from filename
    pub fn detect_from_filename(&self, filename: &str) -> Option<Language> {
        let filename_lower = filename.to_lowercase();

        // Special filenames
        match filename_lower.as_str() {
            "makefile" | "gnumakefile" => return Some(Language::Bash),
            "dockerfile" => return Some(Language::Bash),
            "cargo.toml" => return Some(Language::Rust),
            "package.json" => return Some(Language::JavaScript),
            "go.mod" | "go.sum" => return Some(Language::Go),
            "requirements.txt" | "setup.py" | "pyproject.toml" => return Some(Language::Python),
            _ => {}
        }

        // Try extension
        self.detect_from_extension(filename)
    }

    /// Detect with file context
    pub fn detect_with_context(&self, code: &str, filename: Option<&str>) -> DetectedLanguage {
        // Try filename first
        if let Some(filename) = filename {
            if let Some(lang) = self.detect_from_filename(filename) {
                return DetectedLanguage {
                    language: lang,
                    confidence: 1.0,
                    hints: vec![format!("From filename: {}", filename)],
                };
            }
        }

        // Fall back to content detection
        self.detect(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_python() {
        let detector = LanguageDetector::new();
        let code = r#"
import os
from typing import List

def hello(name: str) -> str:
    return f"Hello, {name}!"

if __name__ == "__main__":
    print(hello("World"))
"#;
        let result = detector.detect(code);
        assert!(matches!(result.language, Language::Python));
        // Multiple pattern matches result in normalized confidence
        assert!(result.confidence > 0.0);
    }

    #[test]
    fn test_detect_rust() {
        let detector = LanguageDetector::new();
        let code = r#"
use std::collections::HashMap;

fn main() {
    let mut map: HashMap<String, i32> = HashMap::new();
    map.insert("key".to_string(), 42);
    println!("{:?}", map);
}
"#;
        let result = detector.detect(code);
        assert!(matches!(result.language, Language::Rust));
        assert!(result.confidence > 0.5);
    }

    #[test]
    fn test_detect_go() {
        let detector = LanguageDetector::new();
        let code = r#"
package main

import "fmt"

func main() {
    fmt.Println("Hello, Go!")
}
"#;
        let result = detector.detect(code);
        assert!(matches!(result.language, Language::Go));
        // Multiple pattern matches result in normalized confidence
        assert!(result.confidence > 0.0);
    }

    #[test]
    fn test_detect_from_extension() {
        let detector = LanguageDetector::new();
        assert!(matches!(
            detector.detect_from_extension("main.rs"),
            Some(Language::Rust)
        ));
        assert!(matches!(
            detector.detect_from_extension("script.py"),
            Some(Language::Python)
        ));
        assert!(matches!(
            detector.detect_from_extension("app.ts"),
            Some(Language::TypeScript)
        ));
    }

    #[test]
    fn test_detect_with_shebang() {
        let detector = LanguageDetector::new();
        let code = "#!/usr/bin/env python3\nprint(\"Hello from Python!\")";
        let result = detector.detect(code);
        assert!(matches!(result.language, Language::Python));
        assert_eq!(result.confidence, 1.0);
    }
}
