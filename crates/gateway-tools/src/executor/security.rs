//! CodeAct Code Security Validation Module
//!
//! This module provides comprehensive security validation for code execution in CodeAct.
//! It performs static analysis to detect dangerous patterns, blocked imports, and
//! potential security vulnerabilities before code execution.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        SecurityValidator                                 │
//! │  ┌───────────────────┐  ┌───────────────────┐  ┌────────────────────┐  │
//! │  │  ImportChecker    │  │  FunctionChecker  │  │  PatternMatcher    │  │
//! │  └───────────────────┘  └───────────────────┘  └────────────────────┘  │
//! │           │                      │                       │              │
//! │           └──────────────────────┼───────────────────────┘              │
//! │                                  │                                       │
//! │                      ┌───────────┴───────────┐                          │
//! │                      │   ValidationResult    │                          │
//! │                      └───────────────────────┘                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Security Features
//!
//! - Import whitelist/blacklist validation
//! - Dangerous function call detection
//! - Code injection pattern detection
//! - Network access detection
//! - File system access validation
//! - Risk scoring for code analysis
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::executor::security::{SecurityValidator, SecurityConfig};
//!
//! let config = SecurityConfig::default();
//! let validator = SecurityValidator::new(config);
//!
//! let code = r#"
//! import numpy as np
//! result = np.array([1, 2, 3])
//! print(result)
//! "#;
//!
//! match validator.validate_code(code) {
//!     Ok(result) if result.is_safe => println!("Code is safe to execute"),
//!     Ok(result) => {
//!         for issue in result.issues {
//!             println!("Security issue: {:?}", issue);
//!         }
//!     }
//!     Err(e) => println!("Validation error: {}", e),
//! }
//! ```

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

use crate::error::{ServiceError as Error, ServiceResult as Result};

// ============================================================================
// Security Configuration
// ============================================================================

/// Security configuration for code validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Allow network access (socket, requests, urllib, etc.)
    #[serde(default)]
    pub allow_network: bool,

    /// Allow file system access (open with write mode, etc.)
    #[serde(default)]
    pub allow_filesystem: bool,

    /// Allowed imports (if empty, all non-blocked imports are allowed)
    #[serde(default)]
    pub allowed_imports: HashSet<String>,

    /// Blocked function patterns
    #[serde(default = "default_blocked_functions")]
    pub blocked_functions: Vec<String>,

    /// Maximum code size in bytes
    #[serde(default = "default_max_code_size")]
    pub max_code_size: usize,

    /// Risk score threshold (0-100, issues above this are critical)
    #[serde(default = "default_risk_threshold")]
    pub risk_threshold: u32,

    /// Enable strict mode (fail on any issue)
    #[serde(default)]
    pub strict_mode: bool,

    /// Custom dangerous patterns
    #[serde(default)]
    pub custom_patterns: Vec<DangerousPatternConfig>,
}

fn default_blocked_functions() -> Vec<String> {
    vec![
        "eval".to_string(),
        "exec".to_string(),
        "compile".to_string(),
        "__import__".to_string(),
        "open".to_string(),
        "file".to_string(),
        "input".to_string(),
        "raw_input".to_string(),
        "execfile".to_string(),
        "globals".to_string(),
        "locals".to_string(),
        "vars".to_string(),
        "dir".to_string(),
        "getattr".to_string(),
        "setattr".to_string(),
        "delattr".to_string(),
        "hasattr".to_string(),
    ]
}

fn default_max_code_size() -> usize {
    1_048_576 // 1MB
}

fn default_risk_threshold() -> u32 {
    50
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allow_network: false,
            allow_filesystem: false,
            allowed_imports: default_allowed_imports(),
            blocked_functions: default_blocked_functions(),
            max_code_size: default_max_code_size(),
            risk_threshold: default_risk_threshold(),
            strict_mode: false,
            custom_patterns: Vec::new(),
        }
    }
}

/// Default allowed imports (safe standard library and data science modules)
fn default_allowed_imports() -> HashSet<String> {
    [
        // Data Science
        "numpy",
        "pandas",
        "matplotlib",
        "scipy",
        "sklearn",
        "seaborn",
        "plotly",
        "statsmodels",
        // Standard Library (safe subset)
        "json",
        "csv",
        "datetime",
        "re",
        "math",
        "random",
        "collections",
        "itertools",
        "functools",
        "operator",
        "string",
        "textwrap",
        "unicodedata",
        "decimal",
        "fractions",
        "statistics",
        "copy",
        "pprint",
        "enum",
        "dataclasses",
        "typing",
        "abc",
        "contextlib",
        "warnings",
        "traceback",
        "types",
        "inspect",
        "io",
        "base64",
        "hashlib",
        "hmac",
        "zlib",
        "gzip",
        "bz2",
        "lzma",
        "zipfile",
        "tarfile",
        "struct",
        "codecs",
        "html",
        "xml",
        // Testing
        "unittest",
        "doctest",
        // Typing
        "typing_extensions",
        // Utilities
        "heapq",
        "bisect",
        "array",
        "weakref",
        "numbers",
        "cmath",
        "logging",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Custom dangerous pattern configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DangerousPatternConfig {
    /// Pattern name
    pub name: String,
    /// Regex pattern
    pub pattern: String,
    /// Severity level
    pub severity: Severity,
    /// Description of what this pattern detects
    pub description: String,
}

// ============================================================================
// Severity and Issue Types
// ============================================================================

/// Severity level of a security issue
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Critical security issue - immediate threat
    Critical = 4,
    /// High severity - likely dangerous
    High = 3,
    /// Medium severity - potentially dangerous
    Medium = 2,
    /// Low severity - minor concern
    Low = 1,
    /// Informational - for awareness only
    Info = 0,
}

impl Severity {
    /// Get the risk score for this severity
    pub fn risk_score(&self) -> u32 {
        match self {
            Self::Critical => 40,
            Self::High => 25,
            Self::Medium => 15,
            Self::Low => 5,
            Self::Info => 1,
        }
    }

    /// Get a human-readable label
    pub fn label(&self) -> &'static str {
        match self {
            Self::Critical => "CRITICAL",
            Self::High => "HIGH",
            Self::Medium => "MEDIUM",
            Self::Low => "LOW",
            Self::Info => "INFO",
        }
    }
}

/// Type of security issue detected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueType {
    /// Dangerous import detected
    DangerousImport,
    /// Dangerous function call detected
    DangerousFunction,
    /// Network access attempt
    NetworkAccess,
    /// File system access attempt
    FileAccess,
    /// Code injection pattern detected
    CodeInjection,
    /// Privilege escalation attempt
    PrivilegeEscalation,
    /// System command execution
    SystemCommand,
    /// Obfuscation technique detected
    Obfuscation,
    /// Resource exhaustion potential
    ResourceExhaustion,
    /// Code size exceeded
    CodeSizeExceeded,
}

impl IssueType {
    /// Get a human-readable label
    pub fn label(&self) -> &'static str {
        match self {
            Self::DangerousImport => "Dangerous Import",
            Self::DangerousFunction => "Dangerous Function",
            Self::NetworkAccess => "Network Access",
            Self::FileAccess => "File Access",
            Self::CodeInjection => "Code Injection",
            Self::PrivilegeEscalation => "Privilege Escalation",
            Self::SystemCommand => "System Command",
            Self::Obfuscation => "Obfuscation",
            Self::ResourceExhaustion => "Resource Exhaustion",
            Self::CodeSizeExceeded => "Code Size Exceeded",
        }
    }
}

// ============================================================================
// Code Location
// ============================================================================

/// Location in the source code where an issue was found
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeLocation {
    /// Line number (1-indexed)
    pub line: usize,
    /// Column number (1-indexed)
    pub column: usize,
    /// End line number (for multi-line spans)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    /// End column number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<usize>,
    /// The source text that triggered the issue
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_text: Option<String>,
}

impl CodeLocation {
    /// Create a new code location
    pub fn new(line: usize, column: usize) -> Self {
        Self {
            line,
            column,
            end_line: None,
            end_column: None,
            source_text: None,
        }
    }

    /// Create a location with source text
    pub fn with_source(line: usize, column: usize, source: impl Into<String>) -> Self {
        Self {
            line,
            column,
            end_line: None,
            end_column: None,
            source_text: Some(source.into()),
        }
    }

    /// Create a location spanning multiple lines
    pub fn span(line: usize, column: usize, end_line: usize, end_column: usize) -> Self {
        Self {
            line,
            column,
            end_line: Some(end_line),
            end_column: Some(end_column),
            source_text: None,
        }
    }
}

impl std::fmt::Display for CodeLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(end_line) = self.end_line {
            write!(
                f,
                "{}:{}-{}:{}",
                self.line,
                self.column,
                end_line,
                self.end_column.unwrap_or(0)
            )
        } else {
            write!(f, "{}:{}", self.line, self.column)
        }
    }
}

// ============================================================================
// Security Issue
// ============================================================================

/// A security issue found during validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityIssue {
    /// Severity of the issue
    pub severity: Severity,
    /// Type of issue
    pub issue_type: IssueType,
    /// Location in source code
    pub location: CodeLocation,
    /// Description of the issue
    pub description: String,
    /// Suggested fix or alternative
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// Unique issue identifier
    pub issue_id: String,
}

impl SecurityIssue {
    /// Create a new security issue
    pub fn new(
        severity: Severity,
        issue_type: IssueType,
        location: CodeLocation,
        description: impl Into<String>,
    ) -> Self {
        let desc = description.into();
        let issue_id = format!(
            "{:?}-{}-{}",
            issue_type,
            location.line,
            &desc[..desc.len().min(20)]
        );
        Self {
            severity,
            issue_type,
            location,
            description: desc,
            suggestion: None,
            issue_id,
        }
    }

    /// Add a suggestion
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Get the risk score for this issue
    pub fn risk_score(&self) -> u32 {
        self.severity.risk_score()
    }
}

impl std::fmt::Display for SecurityIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} at {}: {}",
            self.severity.label(),
            self.issue_type.label(),
            self.location,
            self.description
        )
    }
}

// ============================================================================
// Validation Result
// ============================================================================

/// Result of code validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether the code is considered safe to execute
    pub is_safe: bool,
    /// List of issues found
    pub issues: Vec<SecurityIssue>,
    /// Overall risk score (0-100)
    pub risk_score: u32,
    /// Recommendations for improving security
    pub recommendations: Vec<String>,
    /// Validation metadata
    pub metadata: ValidationMetadata,
}

/// Metadata about the validation process
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidationMetadata {
    /// Total lines of code analyzed
    pub lines_analyzed: usize,
    /// Number of imports found
    pub imports_found: usize,
    /// Number of function calls analyzed
    pub function_calls_analyzed: usize,
    /// Validation duration in milliseconds
    pub validation_time_ms: u64,
    /// Code hash for caching
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_hash: Option<String>,
}

impl ValidationResult {
    /// Create a safe validation result
    pub fn safe() -> Self {
        Self {
            is_safe: true,
            issues: Vec::new(),
            risk_score: 0,
            recommendations: Vec::new(),
            metadata: ValidationMetadata::default(),
        }
    }

    /// Create an unsafe validation result
    pub fn unsafe_with_issues(issues: Vec<SecurityIssue>) -> Self {
        let risk_score: u32 = issues.iter().map(|i| i.risk_score()).sum::<u32>().min(100);
        Self {
            is_safe: false,
            issues,
            risk_score,
            recommendations: Vec::new(),
            metadata: ValidationMetadata::default(),
        }
    }

    /// Add a recommendation
    pub fn with_recommendation(mut self, recommendation: impl Into<String>) -> Self {
        self.recommendations.push(recommendation.into());
        self
    }

    /// Set metadata
    pub fn with_metadata(mut self, metadata: ValidationMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Get issues by severity
    pub fn issues_by_severity(&self, severity: Severity) -> Vec<&SecurityIssue> {
        self.issues
            .iter()
            .filter(|i| i.severity == severity)
            .collect()
    }

    /// Get critical issues
    pub fn critical_issues(&self) -> Vec<&SecurityIssue> {
        self.issues_by_severity(Severity::Critical)
    }

    /// Check if there are any critical issues
    pub fn has_critical_issues(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Critical)
    }
}

// ============================================================================
// Dangerous Pattern
// ============================================================================

/// A dangerous pattern to match against code
#[derive(Debug, Clone)]
pub struct DangerousPattern {
    /// Pattern name
    pub name: String,
    /// Compiled regex pattern
    pub pattern: Regex,
    /// Severity level
    pub severity: Severity,
    /// Description of what this pattern detects
    pub description: String,
    /// Issue type
    pub issue_type: IssueType,
}

impl DangerousPattern {
    /// Create a new dangerous pattern
    pub fn new(
        name: impl Into<String>,
        pattern: &str,
        severity: Severity,
        issue_type: IssueType,
        description: impl Into<String>,
    ) -> Result<Self> {
        let regex = Regex::new(pattern)
            .map_err(|e| Error::InvalidInput(format!("Invalid regex pattern: {}", e)))?;
        Ok(Self {
            name: name.into(),
            pattern: regex,
            severity,
            description: description.into(),
            issue_type,
        })
    }

    /// Check if the pattern matches the code
    pub fn matches(&self, code: &str) -> Vec<(usize, usize, String)> {
        let mut matches = Vec::new();
        for (line_idx, line) in code.lines().enumerate() {
            for m in self.pattern.find_iter(line) {
                matches.push((line_idx + 1, m.start() + 1, m.as_str().to_string()));
            }
        }
        matches
    }
}

// ============================================================================
// Import Whitelist
// ============================================================================

/// Import whitelist manager
#[derive(Debug, Clone)]
pub struct ImportWhitelist {
    allowed: HashSet<String>,
}

impl ImportWhitelist {
    /// Create a new import whitelist
    pub fn new(allowed: HashSet<String>) -> Self {
        Self { allowed }
    }

    /// Create with default safe imports
    pub fn default_safe() -> Self {
        Self::new(default_allowed_imports())
    }

    /// Check if an import is allowed
    pub fn is_allowed(&self, import: &str) -> bool {
        // R5-M: Fail-closed — empty whitelist blocks all imports (was fail-open)
        if self.allowed.is_empty() {
            return false;
        }
        // Check exact match first
        if self.allowed.contains(import) {
            return true;
        }
        // Check if base module is allowed (e.g., "numpy.random" should be allowed if "numpy" is)
        if let Some(base) = import.split('.').next() {
            return self.allowed.contains(base);
        }
        false
    }

    /// Add an allowed import
    pub fn allow(&mut self, import: impl Into<String>) {
        self.allowed.insert(import.into());
    }

    /// Remove an allowed import
    pub fn disallow(&mut self, import: &str) {
        self.allowed.remove(import);
    }

    /// Get all allowed imports
    pub fn allowed_imports(&self) -> &HashSet<String> {
        &self.allowed
    }
}

impl Default for ImportWhitelist {
    fn default() -> Self {
        Self::default_safe()
    }
}

// ============================================================================
// Security Validator
// ============================================================================

/// Main security validator for code analysis
pub struct SecurityValidator {
    config: SecurityConfig,
    patterns: Vec<DangerousPattern>,
    import_whitelist: ImportWhitelist,
    blocked_imports: HashSet<String>,
}

impl SecurityValidator {
    /// Create a new security validator with the given configuration
    pub fn new(config: SecurityConfig) -> Self {
        let patterns = Self::build_patterns(&config);
        let import_whitelist = ImportWhitelist::new(config.allowed_imports.clone());
        let blocked_imports = Self::get_blocked_imports();

        Self {
            config,
            patterns,
            import_whitelist,
            blocked_imports,
        }
    }

    /// Create a validator with default configuration
    pub fn default_validator() -> Self {
        Self::new(SecurityConfig::default())
    }

    /// Create a permissive validator (allows more operations)
    pub fn permissive() -> Self {
        Self::new(SecurityConfig {
            allow_network: true,
            allow_filesystem: true,
            strict_mode: false,
            ..Default::default()
        })
    }

    /// Create a strict validator (blocks most operations)
    pub fn strict() -> Self {
        Self::new(SecurityConfig {
            allow_network: false,
            allow_filesystem: false,
            strict_mode: true,
            risk_threshold: 20,
            ..Default::default()
        })
    }

    /// Get the configuration
    pub fn config(&self) -> &SecurityConfig {
        &self.config
    }

    /// Get blocked imports
    fn get_blocked_imports() -> HashSet<String> {
        [
            // System access
            "os",
            "sys",
            "subprocess",
            "shutil",
            "pathlib",
            "glob",
            // Network
            "socket",
            "http",
            "urllib",
            "requests",
            "httplib",
            "ftplib",
            "smtplib",
            "poplib",
            "imaplib",
            "telnetlib",
            "asyncio",
            "aiohttp",
            "websocket",
            "websockets",
            // Low-level
            "ctypes",
            "cffi",
            "mmap",
            // Process/Thread
            "multiprocessing",
            "threading",
            "_thread",
            "concurrent",
            // Code execution
            "code",
            "codeop",
            "dis",
            "importlib",
            "runpy",
            "compileall",
            "py_compile",
            // System info
            "platform",
            "getpass",
            "pwd",
            "grp",
            "spwd",
            "crypt",
            // Terminal
            "pty",
            "fcntl",
            "termios",
            "tty",
            "curses",
            // Signal
            "signal",
            "resource",
            // Pickle (deserialization attacks)
            "pickle",
            "cPickle",
            "shelve",
            "marshal",
            // Database
            "sqlite3",
            "dbm",
            // Other dangerous
            "builtins",
            "__builtin__",
            "gc",
            "atexit",
            "syslog",
            "nis",
            "posix",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// Build dangerous patterns from configuration
    fn build_patterns(config: &SecurityConfig) -> Vec<DangerousPattern> {
        let mut patterns = Vec::new();

        // System command execution patterns
        patterns.extend(Self::system_command_patterns());

        // Code injection patterns
        patterns.extend(Self::code_injection_patterns());

        // Network access patterns (if not allowed)
        if !config.allow_network {
            patterns.extend(Self::network_patterns());
        }

        // File access patterns (if not allowed)
        if !config.allow_filesystem {
            patterns.extend(Self::file_access_patterns());
        }

        // Obfuscation patterns
        patterns.extend(Self::obfuscation_patterns());

        // Resource exhaustion patterns
        patterns.extend(Self::resource_exhaustion_patterns());

        // Add custom patterns
        for custom in &config.custom_patterns {
            if let Ok(pattern) = DangerousPattern::new(
                &custom.name,
                &custom.pattern,
                custom.severity,
                IssueType::CodeInjection,
                &custom.description,
            ) {
                patterns.push(pattern);
            }
        }

        patterns
    }

    fn system_command_patterns() -> Vec<DangerousPattern> {
        vec![
            DangerousPattern::new(
                "os_system",
                r"os\s*\.\s*system\s*\(",
                Severity::Critical,
                IssueType::SystemCommand,
                "os.system() executes shell commands - potential command injection",
            )
            .unwrap(),
            DangerousPattern::new(
                "os_popen",
                r"os\s*\.\s*popen\s*\(",
                Severity::Critical,
                IssueType::SystemCommand,
                "os.popen() executes shell commands",
            )
            .unwrap(),
            DangerousPattern::new(
                "os_exec",
                r"os\s*\.\s*exec[lv]?[pe]?\s*\(",
                Severity::Critical,
                IssueType::SystemCommand,
                "os.exec*() family replaces the current process",
            )
            .unwrap(),
            DangerousPattern::new(
                "os_spawn",
                r"os\s*\.\s*spawn[lv]?[pe]?\s*\(",
                Severity::Critical,
                IssueType::SystemCommand,
                "os.spawn*() family spawns new processes",
            )
            .unwrap(),
            DangerousPattern::new(
                "subprocess_any",
                r"subprocess\s*\.\s*(run|call|Popen|check_output|check_call|getoutput|getstatusoutput)\s*\(",
                Severity::Critical,
                IssueType::SystemCommand,
                "subprocess module can execute arbitrary commands",
            )
            .unwrap(),
            DangerousPattern::new(
                "commands_module",
                r"commands\s*\.\s*(getoutput|getstatusoutput)\s*\(",
                Severity::Critical,
                IssueType::SystemCommand,
                "commands module executes shell commands (deprecated)",
            )
            .unwrap(),
        ]
    }

    fn code_injection_patterns() -> Vec<DangerousPattern> {
        vec![
            DangerousPattern::new(
                "eval_call",
                r"\beval\s*\(",
                Severity::Critical,
                IssueType::CodeInjection,
                "eval() executes arbitrary Python code - major security risk",
            )
            .unwrap(),
            DangerousPattern::new(
                "exec_call",
                r"\bexec\s*\(",
                Severity::Critical,
                IssueType::CodeInjection,
                "exec() executes arbitrary Python code - major security risk",
            )
            .unwrap(),
            DangerousPattern::new(
                "compile_call",
                r"\bcompile\s*\(",
                Severity::High,
                IssueType::CodeInjection,
                "compile() can be used to create executable code objects",
            )
            .unwrap(),
            DangerousPattern::new(
                "dunder_import",
                r"__import__\s*\(",
                Severity::Critical,
                IssueType::CodeInjection,
                "__import__() can dynamically import any module",
            )
            .unwrap(),
            DangerousPattern::new(
                "importlib_import",
                r"importlib\s*\.\s*import_module\s*\(",
                Severity::Critical,
                IssueType::DangerousImport,
                "importlib.import_module() dynamically imports modules",
            )
            .unwrap(),
            DangerousPattern::new(
                "getattr_exec",
                r#"getattr\s*\([^,]+,\s*['"]__(class|bases|subclasses|mro|globals|builtins|code|func)__['"]\s*\)"#,
                Severity::Critical,
                IssueType::CodeInjection,
                "Accessing dunder attributes via getattr is a common attack vector",
            )
            .unwrap(),
            DangerousPattern::new(
                "dunder_class",
                r"\.__class__\s*\.\s*__bases__",
                Severity::Critical,
                IssueType::PrivilegeEscalation,
                "Class introspection chain often used in sandbox escapes",
            )
            .unwrap(),
            DangerousPattern::new(
                "dunder_subclasses",
                r"\.__class__\s*\.\s*__mro__\s*\[.*\]\s*\.\s*__subclasses__",
                Severity::Critical,
                IssueType::PrivilegeEscalation,
                "MRO traversal with __subclasses__ is a sandbox escape technique",
            )
            .unwrap(),
            DangerousPattern::new(
                "dunder_globals",
                r"\.__globals__",
                Severity::Critical,
                IssueType::PrivilegeEscalation,
                "__globals__ access can leak sensitive data and bypass restrictions",
            )
            .unwrap(),
            DangerousPattern::new(
                "dunder_builtins",
                r"__builtins__",
                Severity::Critical,
                IssueType::PrivilegeEscalation,
                "__builtins__ access can bypass security restrictions",
            )
            .unwrap(),
        ]
    }

    fn network_patterns() -> Vec<DangerousPattern> {
        vec![
            DangerousPattern::new(
                "socket_create",
                r"socket\s*\.\s*socket\s*\(",
                Severity::High,
                IssueType::NetworkAccess,
                "Creating network sockets",
            )
            .unwrap(),
            DangerousPattern::new(
                "socket_connect",
                r#"\.connect\s*\(\s*\(['"][^'"]+['"]\s*,\s*\d+\s*\)\s*\)"#,
                Severity::High,
                IssueType::NetworkAccess,
                "Socket connection to remote host",
            )
            .unwrap(),
            DangerousPattern::new(
                "requests_get",
                r"requests\s*\.\s*(get|post|put|delete|patch|head|options)\s*\(",
                Severity::Medium,
                IssueType::NetworkAccess,
                "HTTP request using requests library",
            )
            .unwrap(),
            DangerousPattern::new(
                "urllib_open",
                r"urllib\s*\.\s*(request\s*\.)?\s*(urlopen|Request)\s*\(",
                Severity::Medium,
                IssueType::NetworkAccess,
                "URL access using urllib",
            )
            .unwrap(),
            DangerousPattern::new(
                "httplib_connect",
                r"(http\.client|httplib)\s*\.\s*HTTP(S)?Connection\s*\(",
                Severity::Medium,
                IssueType::NetworkAccess,
                "HTTP connection using http.client",
            )
            .unwrap(),
            DangerousPattern::new(
                "aiohttp_session",
                r"aiohttp\s*\.\s*ClientSession\s*\(",
                Severity::Medium,
                IssueType::NetworkAccess,
                "Async HTTP session using aiohttp",
            )
            .unwrap(),
        ]
    }

    fn file_access_patterns() -> Vec<DangerousPattern> {
        // File access patterns are Critical when filesystem access is not allowed
        vec![
            DangerousPattern::new(
                "open_write",
                r#"\bopen\s*\([^)]*['"][waxWAX][+bU]*['"][^)]*\)"#,
                Severity::Critical,
                IssueType::FileAccess,
                "File open with write mode - filesystem access not allowed",
            )
            .unwrap(),
            DangerousPattern::new(
                "pathlib_write",
                r#"Path\s*\([^)]*\)\s*\.\s*(write_text|write_bytes|open\s*\([^)]*['"][waxWAX])"#,
                Severity::Critical,
                IssueType::FileAccess,
                "File write using pathlib - filesystem access not allowed",
            )
            .unwrap(),
            DangerousPattern::new(
                "os_remove",
                r"os\s*\.\s*(remove|unlink|rmdir|removedirs)\s*\(",
                Severity::Critical,
                IssueType::FileAccess,
                "File/directory removal - filesystem access not allowed",
            )
            .unwrap(),
            DangerousPattern::new(
                "shutil_rm",
                r"shutil\s*\.\s*(rmtree|move|copy|copytree)\s*\(",
                Severity::Critical,
                IssueType::FileAccess,
                "File operations using shutil - filesystem access not allowed",
            )
            .unwrap(),
            DangerousPattern::new(
                "os_chmod",
                r"os\s*\.\s*(chmod|chown|chflags)\s*\(",
                Severity::Critical,
                IssueType::PrivilegeEscalation,
                "File permission changes - filesystem access not allowed",
            )
            .unwrap(),
            DangerousPattern::new(
                "os_link",
                r"os\s*\.\s*(link|symlink)\s*\(",
                Severity::High,
                IssueType::FileAccess,
                "Creating file links",
            )
            .unwrap(),
        ]
    }

    fn obfuscation_patterns() -> Vec<DangerousPattern> {
        vec![
            DangerousPattern::new(
                "base64_decode_exec",
                r"(exec|eval)\s*\(\s*(base64\.b64decode|codecs\.decode)\s*\(",
                Severity::Critical,
                IssueType::Obfuscation,
                "Executing base64 encoded code - likely obfuscated malicious code",
            )
            .unwrap(),
            DangerousPattern::new(
                "chr_join",
                r#"['"][\s\n]*\.join\s*\(\s*\[?\s*(chr\s*\(\s*\d+\s*\)\s*,?\s*)+\s*\]?\s*\)"#,
                Severity::High,
                IssueType::Obfuscation,
                "String construction from chr() calls - obfuscation technique",
            )
            .unwrap(),
            DangerousPattern::new(
                "hex_decode",
                r"(bytes\.fromhex|bytearray\.fromhex)\s*\([^)]+\)\s*\.\s*decode\s*\(",
                Severity::Medium,
                IssueType::Obfuscation,
                "Decoding hex-encoded strings - possible obfuscation",
            )
            .unwrap(),
            DangerousPattern::new(
                "rot13_decode",
                r#"codecs\s*\.\s*decode\s*\([^,]+,\s*['"]rot[_-]?13['"]\s*\)"#,
                Severity::Medium,
                IssueType::Obfuscation,
                "ROT13 decoding - simple obfuscation",
            )
            .unwrap(),
            DangerousPattern::new(
                "lambda_call",
                r"\(\s*lambda\s+[^:]+:\s*(eval|exec|compile|__import__)\s*\(",
                Severity::High,
                IssueType::Obfuscation,
                "Lambda with dangerous function - obfuscation technique",
            )
            .unwrap(),
        ]
    }

    fn resource_exhaustion_patterns() -> Vec<DangerousPattern> {
        vec![
            DangerousPattern::new(
                "while_true",
                r"while\s+(True|1)\s*:",
                Severity::Medium,
                IssueType::ResourceExhaustion,
                "Infinite loop pattern - may cause resource exhaustion",
            )
            .unwrap(),
            // Note: Backreference patterns not supported in Rust regex
            // Simple heuristic: function that calls itself by common name patterns
            DangerousPattern::new(
                "recursion_self",
                r"def\s+\w+\s*\([^)]*\)[^:]*:\s*[^\n]*\breturn\s+\w+\s*\(",
                Severity::Low,
                IssueType::ResourceExhaustion,
                "Possible recursive function - check for proper base case",
            )
            .unwrap(),
            DangerousPattern::new(
                "large_allocation",
                r"\[\s*0\s*\]\s*\*\s*(\d{8,}|\d+\s*\*\s*\d+\s*\*\s*\d+)",
                Severity::Medium,
                IssueType::ResourceExhaustion,
                "Large memory allocation",
            )
            .unwrap(),
            DangerousPattern::new(
                "range_huge",
                r"range\s*\(\s*\d{10,}\s*\)",
                Severity::Medium,
                IssueType::ResourceExhaustion,
                "Huge range iteration - memory/CPU exhaustion",
            )
            .unwrap(),
        ]
    }

    /// Validate code for security issues
    pub fn validate_code(&self, code: &str) -> Result<ValidationResult> {
        let start_time = std::time::Instant::now();
        let mut issues = Vec::new();

        // Check code size
        if code.len() > self.config.max_code_size {
            issues.push(SecurityIssue::new(
                Severity::Critical,
                IssueType::CodeSizeExceeded,
                CodeLocation::new(1, 1),
                format!(
                    "Code size ({} bytes) exceeds maximum allowed ({} bytes)",
                    code.len(),
                    self.config.max_code_size
                ),
            ));
        }

        // Check imports
        let import_issues = self.check_imports(code);
        issues.extend(import_issues);

        // Check function calls
        let function_issues = self.check_functions(code);
        issues.extend(function_issues);

        // Check patterns
        let pattern_issues = self.check_patterns(code);
        issues.extend(pattern_issues);

        // Calculate metadata
        let lines_analyzed = code.lines().count();
        let imports_found = count_imports(code);
        let function_calls_analyzed = count_function_calls(code);
        let validation_time_ms = start_time.elapsed().as_millis() as u64;

        let metadata = ValidationMetadata {
            lines_analyzed,
            imports_found,
            function_calls_analyzed,
            validation_time_ms,
            code_hash: Some(hash_code(code)),
        };

        // Calculate risk score
        let risk_score: u32 = issues.iter().map(|i| i.risk_score()).sum::<u32>().min(100);

        // Determine if safe
        let has_critical = issues.iter().any(|i| i.severity == Severity::Critical);
        let over_threshold = risk_score > self.config.risk_threshold;
        let is_safe = if self.config.strict_mode {
            issues.is_empty()
        } else {
            !has_critical && !over_threshold
        };

        // Generate recommendations
        let recommendations = self.generate_recommendations(&issues);

        Ok(ValidationResult {
            is_safe,
            issues,
            risk_score,
            recommendations,
            metadata,
        })
    }

    /// Check imports for security issues
    pub fn check_imports(&self, code: &str) -> Vec<SecurityIssue> {
        let mut issues = Vec::new();

        // Import patterns
        let import_regex = Regex::new(r"^\s*import\s+(\S+)").unwrap();
        let from_import_regex = Regex::new(r"^\s*from\s+(\S+)\s+import").unwrap();

        for (line_idx, line) in code.lines().enumerate() {
            let line_num = line_idx + 1;

            // Check "import X" style
            if let Some(caps) = import_regex.captures(line) {
                let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                self.check_single_import(module, line_num, line, &mut issues);
            }

            // Check "from X import Y" style
            if let Some(caps) = from_import_regex.captures(line) {
                let module = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                self.check_single_import(module, line_num, line, &mut issues);
            }
        }

        issues
    }

    fn check_single_import(
        &self,
        module: &str,
        line_num: usize,
        line: &str,
        issues: &mut Vec<SecurityIssue>,
    ) {
        // Get base module name (e.g., "os" from "os.path")
        let base_module = module.split('.').next().unwrap_or(module);

        // Check if module is blocked
        if self.blocked_imports.contains(base_module) {
            // Blocked imports are all critical security issues
            let severity = match base_module {
                // System access - always critical
                "os" | "subprocess" | "ctypes" | "cffi" | "sys" | "shutil" => Severity::Critical,
                // Network access - critical when not explicitly allowed
                "socket" | "requests" | "urllib" | "http" | "aiohttp" | "websocket"
                    if !self.config.allow_network =>
                {
                    Severity::Critical
                }
                // Deserialization - critical due to arbitrary code execution risk
                "pickle" | "marshal" => Severity::Critical,
                // Code execution - critical
                "importlib" | "code" | "runpy" => Severity::Critical,
                // Process/threading - critical
                "multiprocessing" | "threading" | "concurrent" => Severity::Critical,
                // Other blocked imports are high severity
                _ => Severity::High,
            };

            let issue_type = match base_module {
                "socket" | "requests" | "urllib" | "http" | "aiohttp" => IssueType::NetworkAccess,
                "os" | "subprocess" | "shutil" => IssueType::SystemCommand,
                "ctypes" | "cffi" => IssueType::PrivilegeEscalation,
                "pickle" | "marshal" => IssueType::CodeInjection,
                _ => IssueType::DangerousImport,
            };

            issues.push(
                SecurityIssue::new(
                    severity,
                    issue_type,
                    CodeLocation::with_source(line_num, 1, line.trim()),
                    format!("Import of blocked module '{}' detected", base_module),
                )
                .with_suggestion(format!(
                    "Consider using a safer alternative or request explicit permission for '{}'",
                    base_module
                )),
            );
        }
        // Check whitelist if not empty
        else if !self.config.allowed_imports.is_empty()
            && !self.import_whitelist.is_allowed(base_module)
        {
            issues.push(
                SecurityIssue::new(
                    Severity::Medium,
                    IssueType::DangerousImport,
                    CodeLocation::with_source(line_num, 1, line.trim()),
                    format!("Import '{}' is not in the allowed list", base_module),
                )
                .with_suggestion("Use only whitelisted imports or request permission"),
            );
        }
    }

    /// Check function calls for security issues
    pub fn check_functions(&self, code: &str) -> Vec<SecurityIssue> {
        let mut issues = Vec::new();

        for blocked in &self.config.blocked_functions {
            // Build regex for function call
            let pattern = format!(r"\b{}\s*\(", regex::escape(blocked));
            if let Ok(regex) = Regex::new(&pattern) {
                for (line_idx, line) in code.lines().enumerate() {
                    for m in regex.find_iter(line) {
                        let severity = match blocked.as_str() {
                            "eval" | "exec" | "compile" | "__import__" => Severity::Critical,
                            "open" | "file" => {
                                if self.config.allow_filesystem {
                                    continue; // Skip if filesystem is allowed
                                }
                                Severity::High
                            }
                            "globals" | "locals" | "vars" => Severity::Medium,
                            _ => Severity::Low,
                        };

                        let issue_type = match blocked.as_str() {
                            "eval" | "exec" | "compile" | "__import__" => IssueType::CodeInjection,
                            "open" | "file" => IssueType::FileAccess,
                            "globals" | "locals" | "vars" | "getattr" | "setattr" => {
                                IssueType::PrivilegeEscalation
                            }
                            _ => IssueType::DangerousFunction,
                        };

                        issues.push(
                            SecurityIssue::new(
                                severity,
                                issue_type,
                                CodeLocation::with_source(line_idx + 1, m.start() + 1, m.as_str()),
                                format!("Blocked function '{}' detected", blocked),
                            )
                            .with_suggestion(suggest_alternative(blocked)),
                        );
                    }
                }
            }
        }

        issues
    }

    /// Check dangerous patterns in code
    pub fn check_patterns(&self, code: &str) -> Vec<SecurityIssue> {
        let mut issues = Vec::new();

        for pattern in &self.patterns {
            for (line, col, source) in pattern.matches(code) {
                issues.push(
                    SecurityIssue::new(
                        pattern.severity,
                        pattern.issue_type,
                        CodeLocation::with_source(line, col, source),
                        pattern.description.clone(),
                    )
                    .with_suggestion(format!(
                        "Pattern '{}' detected - review and remove if not needed",
                        pattern.name
                    )),
                );
            }
        }

        issues
    }

    /// Generate recommendations based on issues found
    fn generate_recommendations(&self, issues: &[SecurityIssue]) -> Vec<String> {
        let mut recommendations = Vec::new();
        let mut seen_types: HashSet<IssueType> = HashSet::new();

        for issue in issues {
            if seen_types.contains(&issue.issue_type) {
                continue;
            }
            seen_types.insert(issue.issue_type);

            match issue.issue_type {
                IssueType::DangerousImport => {
                    recommendations.push(
                        "Use only whitelisted imports from the safe list: numpy, pandas, json, etc."
                            .to_string(),
                    );
                }
                IssueType::DangerousFunction => {
                    recommendations.push(
                        "Avoid using eval(), exec(), and compile(). Use safer alternatives like ast.literal_eval() for parsing."
                            .to_string(),
                    );
                }
                IssueType::NetworkAccess => {
                    recommendations.push(
                        "Network access is restricted. Process data locally instead of fetching from external sources."
                            .to_string(),
                    );
                }
                IssueType::FileAccess => {
                    recommendations.push(
                        "File system access is restricted. Use provided input/output mechanisms instead."
                            .to_string(),
                    );
                }
                IssueType::CodeInjection => {
                    recommendations.push(
                        "Avoid dynamic code execution. Use explicit function calls and data structures."
                            .to_string(),
                    );
                }
                IssueType::PrivilegeEscalation => {
                    recommendations.push(
                        "Avoid accessing internal Python attributes (__class__, __globals__, etc.)."
                            .to_string(),
                    );
                }
                IssueType::SystemCommand => {
                    recommendations.push(
                        "System command execution is not allowed. Use Python built-in functions instead."
                            .to_string(),
                    );
                }
                IssueType::Obfuscation => {
                    recommendations.push(
                        "Code should be clear and readable. Avoid encoding or obfuscating code."
                            .to_string(),
                    );
                }
                IssueType::ResourceExhaustion => {
                    recommendations.push(
                        "Be mindful of resource usage. Use bounded iterations and avoid large allocations."
                            .to_string(),
                    );
                }
                IssueType::CodeSizeExceeded => {
                    recommendations.push(
                        "Reduce code size by splitting into smaller modules or removing unnecessary code."
                            .to_string(),
                    );
                }
            }
        }

        recommendations
    }

    /// Quick check if code has any critical issues (faster than full validation)
    pub fn quick_check(&self, code: &str) -> bool {
        // Check size
        if code.len() > self.config.max_code_size {
            return false;
        }

        // Check for obviously dangerous patterns
        let critical_patterns = [
            r"\beval\s*\(",
            r"\bexec\s*\(",
            r"__import__\s*\(",
            r"os\s*\.\s*system\s*\(",
            r"subprocess\s*\.",
            r"\.__class__\s*\.\s*__bases__",
        ];

        for pattern in critical_patterns {
            if let Ok(regex) = Regex::new(pattern) {
                if regex.is_match(code) {
                    return false;
                }
            }
        }

        // Check blocked imports
        for blocked in &self.blocked_imports {
            let import_pattern =
                format!(r"(^|\s)(import\s+{}|from\s+{}\s+import)", blocked, blocked);
            if let Ok(regex) = Regex::new(&import_pattern) {
                if regex.is_match(code) {
                    return false;
                }
            }
        }

        true
    }
}

impl Default for SecurityValidator {
    fn default() -> Self {
        Self::default_validator()
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Suggest an alternative for a blocked function
fn suggest_alternative(func: &str) -> String {
    match func {
        "eval" => "Use ast.literal_eval() for parsing literals safely".to_string(),
        "exec" => "Define functions explicitly instead of executing code strings".to_string(),
        "compile" => "Use explicit function definitions".to_string(),
        "__import__" => "Use regular import statements".to_string(),
        "open" => "Use provided I/O interfaces instead of direct file access".to_string(),
        "globals" | "locals" | "vars" => {
            "Access variables directly by name instead of through introspection".to_string()
        }
        "getattr" | "setattr" => "Access attributes directly instead of dynamically".to_string(),
        "input" => "Use function parameters to receive input".to_string(),
        _ => format!("Consider safer alternatives to '{}'", func),
    }
}

/// Count import statements in code
fn count_imports(code: &str) -> usize {
    code.lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("import ") || trimmed.starts_with("from ")
        })
        .count()
}

/// Count function calls in code (approximate)
fn count_function_calls(code: &str) -> usize {
    let call_regex = Regex::new(r"\w+\s*\(").unwrap();
    call_regex.find_iter(code).count()
}

/// Hash code for caching
fn hash_code(code: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    code.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

// ============================================================================
// Thread-Safe Validator
// ============================================================================

/// Thread-safe wrapper for SecurityValidator
#[derive(Clone)]
pub struct SharedSecurityValidator {
    inner: Arc<SecurityValidator>,
}

impl SharedSecurityValidator {
    /// Create a new shared validator
    pub fn new(config: SecurityConfig) -> Self {
        Self {
            inner: Arc::new(SecurityValidator::new(config)),
        }
    }

    /// Validate code
    pub fn validate_code(&self, code: &str) -> Result<ValidationResult> {
        self.inner.validate_code(code)
    }

    /// Quick check
    pub fn quick_check(&self, code: &str) -> bool {
        self.inner.quick_check(code)
    }

    /// Get configuration
    pub fn config(&self) -> &SecurityConfig {
        self.inner.config()
    }
}

impl Default for SharedSecurityValidator {
    fn default() -> Self {
        Self::new(SecurityConfig::default())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Configuration Tests
    // ========================================================================

    #[test]
    fn test_security_config_defaults() {
        let config = SecurityConfig::default();
        assert!(!config.allow_network);
        assert!(!config.allow_filesystem);
        assert!(!config.strict_mode);
        assert_eq!(config.max_code_size, 1_048_576);
        assert_eq!(config.risk_threshold, 50);
    }

    #[test]
    fn test_security_config_serialization() {
        let config = SecurityConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SecurityConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.allow_network, config.allow_network);
        assert_eq!(parsed.max_code_size, config.max_code_size);
    }

    #[test]
    fn test_default_allowed_imports() {
        let allowed = default_allowed_imports();
        assert!(allowed.contains("numpy"));
        assert!(allowed.contains("pandas"));
        assert!(allowed.contains("json"));
        assert!(allowed.contains("math"));
        assert!(!allowed.contains("os"));
        assert!(!allowed.contains("subprocess"));
    }

    // ========================================================================
    // Severity Tests
    // ========================================================================

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical > Severity::High);
        assert!(Severity::High > Severity::Medium);
        assert!(Severity::Medium > Severity::Low);
        assert!(Severity::Low > Severity::Info);
    }

    #[test]
    fn test_severity_risk_scores() {
        assert_eq!(Severity::Critical.risk_score(), 40);
        assert_eq!(Severity::High.risk_score(), 25);
        assert_eq!(Severity::Medium.risk_score(), 15);
        assert_eq!(Severity::Low.risk_score(), 5);
        assert_eq!(Severity::Info.risk_score(), 1);
    }

    // ========================================================================
    // Code Location Tests
    // ========================================================================

    #[test]
    fn test_code_location_display() {
        let loc = CodeLocation::new(10, 5);
        assert_eq!(format!("{}", loc), "10:5");

        let span = CodeLocation::span(10, 5, 12, 20);
        assert_eq!(format!("{}", span), "10:5-12:20");
    }

    #[test]
    fn test_code_location_with_source() {
        let loc = CodeLocation::with_source(5, 10, "eval(code)");
        assert_eq!(loc.source_text, Some("eval(code)".to_string()));
    }

    // ========================================================================
    // Security Issue Tests
    // ========================================================================

    #[test]
    fn test_security_issue_display() {
        let issue = SecurityIssue::new(
            Severity::Critical,
            IssueType::CodeInjection,
            CodeLocation::new(5, 1),
            "eval() is dangerous",
        );
        let display = format!("{}", issue);
        assert!(display.contains("CRITICAL"));
        assert!(display.contains("Code Injection"));
        assert!(display.contains("5:1"));
    }

    #[test]
    fn test_security_issue_with_suggestion() {
        let issue = SecurityIssue::new(
            Severity::High,
            IssueType::DangerousFunction,
            CodeLocation::new(1, 1),
            "Problem",
        )
        .with_suggestion("Use alternative");
        assert_eq!(issue.suggestion, Some("Use alternative".to_string()));
    }

    // ========================================================================
    // Validation Result Tests
    // ========================================================================

    #[test]
    fn test_validation_result_safe() {
        let result = ValidationResult::safe();
        assert!(result.is_safe);
        assert!(result.issues.is_empty());
        assert_eq!(result.risk_score, 0);
    }

    #[test]
    fn test_validation_result_unsafe() {
        let issues = vec![SecurityIssue::new(
            Severity::Critical,
            IssueType::CodeInjection,
            CodeLocation::new(1, 1),
            "eval detected",
        )];
        let result = ValidationResult::unsafe_with_issues(issues);
        assert!(!result.is_safe);
        assert_eq!(result.issues.len(), 1);
        assert!(result.risk_score > 0);
    }

    #[test]
    fn test_validation_result_critical_issues() {
        let issues = vec![
            SecurityIssue::new(
                Severity::Critical,
                IssueType::CodeInjection,
                CodeLocation::new(1, 1),
                "Critical",
            ),
            SecurityIssue::new(
                Severity::Low,
                IssueType::FileAccess,
                CodeLocation::new(2, 1),
                "Low",
            ),
        ];
        let result = ValidationResult::unsafe_with_issues(issues);
        assert!(result.has_critical_issues());
        assert_eq!(result.critical_issues().len(), 1);
    }

    // ========================================================================
    // Import Whitelist Tests
    // ========================================================================

    #[test]
    fn test_import_whitelist_default() {
        let whitelist = ImportWhitelist::default_safe();
        assert!(whitelist.is_allowed("numpy"));
        assert!(whitelist.is_allowed("pandas"));
        assert!(whitelist.is_allowed("json"));
        assert!(!whitelist.is_allowed("os"));
    }

    #[test]
    fn test_import_whitelist_submodule() {
        let whitelist = ImportWhitelist::default_safe();
        assert!(whitelist.is_allowed("numpy.random"));
        assert!(whitelist.is_allowed("pandas.DataFrame"));
    }

    #[test]
    fn test_import_whitelist_empty() {
        let whitelist = ImportWhitelist::new(HashSet::new());
        // R5-M: Empty whitelist now blocks all (fail-closed)
        assert!(!whitelist.is_allowed("anything"));
    }

    #[test]
    fn test_import_whitelist_modify() {
        let mut whitelist = ImportWhitelist::default_safe();
        whitelist.allow("custom_module");
        assert!(whitelist.is_allowed("custom_module"));

        whitelist.disallow("numpy");
        assert!(!whitelist.is_allowed("numpy"));
    }

    // ========================================================================
    // Dangerous Pattern Tests
    // ========================================================================

    #[test]
    fn test_dangerous_pattern_matches() {
        let pattern = DangerousPattern::new(
            "eval_call",
            r"\beval\s*\(",
            Severity::Critical,
            IssueType::CodeInjection,
            "eval() detected",
        )
        .unwrap();

        let code = "result = eval(user_input)";
        let matches = pattern.matches(code);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0, 1); // Line 1
    }

    #[test]
    fn test_dangerous_pattern_no_match() {
        let pattern = DangerousPattern::new(
            "eval_call",
            r"\beval\s*\(",
            Severity::Critical,
            IssueType::CodeInjection,
            "eval() detected",
        )
        .unwrap();

        let code = "# This is safe\nx = 1 + 2";
        let matches = pattern.matches(code);
        assert!(matches.is_empty());
    }

    // ========================================================================
    // Security Validator - Safe Code Tests
    // ========================================================================

    #[test]
    fn test_validate_safe_code() {
        let validator = SecurityValidator::default_validator();
        let code = r#"
import numpy as np
import json

data = np.array([1, 2, 3])
result = json.dumps({"data": data.tolist()})
print(result)
"#;
        let result = validator.validate_code(code).unwrap();
        assert!(result.is_safe);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn test_validate_math_operations() {
        let validator = SecurityValidator::default_validator();
        let code = r#"
import math

result = math.sqrt(16) + math.pi
print(f"Result: {result}")
"#;
        let result = validator.validate_code(code).unwrap();
        assert!(result.is_safe);
    }

    #[test]
    fn test_validate_data_processing() {
        let validator = SecurityValidator::default_validator();
        let code = r#"
import pandas as pd
import numpy as np

df = pd.DataFrame({"a": [1, 2, 3], "b": [4, 5, 6]})
mean = df["a"].mean()
"#;
        let result = validator.validate_code(code).unwrap();
        assert!(result.is_safe);
    }

    // ========================================================================
    // Security Validator - Dangerous Import Tests
    // ========================================================================

    #[test]
    fn test_validate_blocks_os() {
        let validator = SecurityValidator::default_validator();
        let code = "import os\nos.system('ls')";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
        assert!(result.has_critical_issues());
    }

    #[test]
    fn test_validate_blocks_subprocess() {
        let validator = SecurityValidator::default_validator();
        let code = "import subprocess\nsubprocess.run(['ls'])";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_blocks_socket() {
        let validator = SecurityValidator::default_validator();
        let code = "import socket\ns = socket.socket()";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
        assert!(result
            .issues
            .iter()
            .any(|i| i.issue_type == IssueType::NetworkAccess));
    }

    #[test]
    fn test_validate_blocks_ctypes() {
        let validator = SecurityValidator::default_validator();
        let code = "import ctypes";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_blocks_pickle() {
        let validator = SecurityValidator::default_validator();
        let code = "import pickle\ndata = pickle.loads(user_data)";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_blocks_from_import() {
        let validator = SecurityValidator::default_validator();
        let code = "from os import system\nsystem('ls')";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    // ========================================================================
    // Security Validator - Code Injection Tests
    // ========================================================================

    #[test]
    fn test_validate_blocks_eval() {
        let validator = SecurityValidator::default_validator();
        let code = "result = eval(user_input)";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
        assert!(result.has_critical_issues());
        assert!(result
            .issues
            .iter()
            .any(|i| i.issue_type == IssueType::CodeInjection));
    }

    #[test]
    fn test_validate_blocks_exec() {
        let validator = SecurityValidator::default_validator();
        let code = "exec(code_string)";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_blocks_compile() {
        let validator = SecurityValidator::default_validator();
        let code = "code = compile(source, '<string>', 'exec')";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_blocks_dunder_import() {
        let validator = SecurityValidator::default_validator();
        let code = "__import__('os').system('ls')";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_blocks_importlib() {
        let validator = SecurityValidator::default_validator();
        let code = "import importlib\nmod = importlib.import_module('os')";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    // ========================================================================
    // Security Validator - Sandbox Escape Tests
    // ========================================================================

    #[test]
    fn test_validate_blocks_class_bases_escape() {
        let validator = SecurityValidator::default_validator();
        let code = r#"
().__class__.__bases__[0].__subclasses__()
"#;
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
        assert!(result
            .issues
            .iter()
            .any(|i| i.issue_type == IssueType::PrivilegeEscalation));
    }

    #[test]
    fn test_validate_blocks_globals_access() {
        let validator = SecurityValidator::default_validator();
        let code = "func.__globals__['os']";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_blocks_builtins_access() {
        let validator = SecurityValidator::default_validator();
        let code = "__builtins__['eval']('code')";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    // ========================================================================
    // Security Validator - Obfuscation Tests
    // ========================================================================

    #[test]
    fn test_validate_blocks_base64_exec() {
        let validator = SecurityValidator::default_validator();
        let code = "exec(base64.b64decode('aW1wb3J0IG9z'))";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
        assert!(result
            .issues
            .iter()
            .any(|i| i.issue_type == IssueType::Obfuscation));
    }

    #[test]
    fn test_validate_blocks_chr_obfuscation() {
        let validator = SecurityValidator::default_validator();
        let code = "''.join([chr(101), chr(118), chr(97), chr(108)])";
        let result = validator.validate_code(code).unwrap();
        // This is a potential obfuscation technique
        assert!(result
            .issues
            .iter()
            .any(|i| i.issue_type == IssueType::Obfuscation));
    }

    // ========================================================================
    // Security Validator - Network Access Tests
    // ========================================================================

    #[test]
    fn test_validate_blocks_requests() {
        let validator = SecurityValidator::default_validator();
        let code = "import requests\nresponse = requests.get('http://evil.com')";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_blocks_urllib() {
        let validator = SecurityValidator::default_validator();
        let code = "from urllib import request\ndata = request.urlopen('http://evil.com')";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_allows_network_when_enabled() {
        let validator = SecurityValidator::new(SecurityConfig {
            allow_network: true,
            ..Default::default()
        });
        let code = "requests.get('http://api.example.com')";
        let result = validator.validate_code(code).unwrap();
        // Network access patterns not flagged when allow_network is true
        assert!(result
            .issues
            .iter()
            .all(|i| i.issue_type != IssueType::NetworkAccess));
    }

    // ========================================================================
    // Security Validator - File Access Tests
    // ========================================================================

    #[test]
    fn test_validate_blocks_open_write() {
        let validator = SecurityValidator::default_validator();
        let code = "f = open('/etc/passwd', 'w')";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_validate_allows_filesystem_when_enabled() {
        let validator = SecurityValidator::new(SecurityConfig {
            allow_filesystem: true,
            ..Default::default()
        });
        let code = "f = open('data.txt', 'w')";
        let result = validator.validate_code(code).unwrap();
        // File access patterns not flagged when allow_filesystem is true
        assert!(result
            .issues
            .iter()
            .all(|i| i.issue_type != IssueType::FileAccess));
    }

    // ========================================================================
    // Security Validator - Resource Exhaustion Tests
    // ========================================================================

    #[test]
    fn test_validate_warns_infinite_loop() {
        let validator = SecurityValidator::default_validator();
        let code = "while True: pass";
        let result = validator.validate_code(code).unwrap();
        assert!(result
            .issues
            .iter()
            .any(|i| i.issue_type == IssueType::ResourceExhaustion));
    }

    #[test]
    fn test_validate_warns_huge_range() {
        let validator = SecurityValidator::default_validator();
        let code = "for i in range(10000000000): pass";
        let result = validator.validate_code(code).unwrap();
        assert!(result
            .issues
            .iter()
            .any(|i| i.issue_type == IssueType::ResourceExhaustion));
    }

    // ========================================================================
    // Security Validator - Code Size Tests
    // ========================================================================

    #[test]
    fn test_validate_blocks_large_code() {
        let config = SecurityConfig {
            max_code_size: 100,
            ..Default::default()
        };
        let validator = SecurityValidator::new(config);
        let code = "x = 1\n".repeat(100); // > 100 bytes
        let result = validator.validate_code(&code).unwrap();
        assert!(result
            .issues
            .iter()
            .any(|i| i.issue_type == IssueType::CodeSizeExceeded));
    }

    // ========================================================================
    // Security Validator - Quick Check Tests
    // ========================================================================

    #[test]
    fn test_quick_check_safe_code() {
        let validator = SecurityValidator::default_validator();
        assert!(validator.quick_check("import json\ndata = json.loads('{}')"));
    }

    #[test]
    fn test_quick_check_dangerous_code() {
        let validator = SecurityValidator::default_validator();
        assert!(!validator.quick_check("eval(user_input)"));
        assert!(!validator.quick_check("import os"));
        assert!(!validator.quick_check("exec(code)"));
    }

    // ========================================================================
    // Security Validator - Strict Mode Tests
    // ========================================================================

    #[test]
    fn test_strict_mode_any_issue_fails() {
        let config = SecurityConfig {
            strict_mode: true,
            ..Default::default()
        };
        let validator = SecurityValidator::new(config);
        // Even low severity issues fail in strict mode
        let code = "while True: break"; // Low severity warning
        let result = validator.validate_code(code).unwrap();
        // In strict mode, any issue makes it unsafe
        if !result.issues.is_empty() {
            assert!(!result.is_safe);
        }
    }

    // ========================================================================
    // Security Validator - Permissive Mode Tests
    // ========================================================================

    #[test]
    fn test_permissive_allows_more() {
        let validator = SecurityValidator::permissive();
        let config = validator.config();
        assert!(config.allow_network);
        assert!(config.allow_filesystem);
    }

    // ========================================================================
    // Security Validator - Recommendations Tests
    // ========================================================================

    #[test]
    fn test_generates_recommendations() {
        let validator = SecurityValidator::default_validator();
        let code = "eval(user_input)";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.recommendations.is_empty());
    }

    // ========================================================================
    // Security Validator - Metadata Tests
    // ========================================================================

    #[test]
    fn test_validation_metadata() {
        let validator = SecurityValidator::default_validator();
        let code = "import json\nx = 1";
        let result = validator.validate_code(code).unwrap();
        assert_eq!(result.metadata.lines_analyzed, 2);
        assert_eq!(result.metadata.imports_found, 1);
        assert!(result.metadata.validation_time_ms >= 0);
        assert!(result.metadata.code_hash.is_some());
    }

    // ========================================================================
    // Bypass Attempt Tests
    // ========================================================================

    #[test]
    fn test_bypass_attempt_getattr_class() {
        let validator = SecurityValidator::default_validator();
        let code = "getattr(obj, '__class__')";
        let result = validator.validate_code(code).unwrap();
        // Should be flagged as potential bypass
        assert!(!result.issues.is_empty());
    }

    #[test]
    fn test_bypass_attempt_string_concat_import() {
        let validator = SecurityValidator::default_validator();
        let code = "__import__('o' + 's')";
        let result = validator.validate_code(code).unwrap();
        // __import__ is always blocked
        assert!(!result.is_safe);
    }

    #[test]
    fn test_bypass_attempt_unicode_encoding() {
        let validator = SecurityValidator::default_validator();
        // Attempt to bypass by using unicode escape
        let code = r#"eval("\u0065val")"#;
        let result = validator.validate_code(code).unwrap();
        // eval() call is detected regardless
        assert!(!result.is_safe);
    }

    #[test]
    fn test_bypass_attempt_comment_trick() {
        let validator = SecurityValidator::default_validator();
        // Attempting to hide dangerous code after comment
        let code = "# safe comment\nimport os  # malicious";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    #[test]
    fn test_bypass_attempt_multiline_string() {
        let validator = SecurityValidator::default_validator();
        let code = r#"
code = '''
import os
os.system('ls')
'''
exec(code)
"#;
        let result = validator.validate_code(code).unwrap();
        // exec() is detected
        assert!(!result.is_safe);
    }

    #[test]
    fn test_bypass_attempt_nested_function() {
        let validator = SecurityValidator::default_validator();
        let code = r#"
def evil():
    return eval

func = evil()
func('__import__("os")')
"#;
        let result = validator.validate_code(code).unwrap();
        // eval and __import__ in strings are detected
        assert!(!result.is_safe);
    }

    #[test]
    fn test_bypass_attempt_lambda_exec() {
        let validator = SecurityValidator::default_validator();
        let code = "(lambda: exec('import os'))()";
        let result = validator.validate_code(code).unwrap();
        assert!(!result.is_safe);
    }

    // ========================================================================
    // Shared Validator Tests
    // ========================================================================

    #[test]
    fn test_shared_validator() {
        let shared = SharedSecurityValidator::default();
        let code = "import json\ndata = json.loads('{}')";
        let result = shared.validate_code(code).unwrap();
        assert!(result.is_safe);
    }

    #[test]
    fn test_shared_validator_clone() {
        let shared1 = SharedSecurityValidator::default();
        let shared2 = shared1.clone();
        // Both should work independently
        assert!(shared1.quick_check("x = 1"));
        assert!(shared2.quick_check("x = 2"));
    }
}
