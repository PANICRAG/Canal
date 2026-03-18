//! Hook Matcher - Pattern matching for hook routing

use crate::agent::types::HookEvent;
use regex::Regex;
use std::collections::HashMap;

/// Hook matcher for routing events to handlers based on patterns
#[derive(Default)]
pub struct HookMatcher {
    /// Tool patterns by event
    tool_patterns: HashMap<HookEvent, Vec<MatchPattern>>,
    /// Path patterns by event
    path_patterns: HashMap<HookEvent, Vec<MatchPattern>>,
    /// Command patterns by event
    command_patterns: HashMap<HookEvent, Vec<MatchPattern>>,
}

/// A match pattern with optional regex
#[derive(Clone)]
pub struct MatchPattern {
    /// Original pattern string
    pub pattern: String,
    /// Compiled regex (if pattern contains special chars)
    regex: Option<Regex>,
    /// Associated handler ID
    pub handler_id: String,
}

impl MatchPattern {
    /// Create a new match pattern
    pub fn new(pattern: impl Into<String>, handler_id: impl Into<String>) -> Self {
        let pattern = pattern.into();
        let regex = Self::compile_pattern(&pattern);
        Self {
            pattern,
            regex,
            handler_id: handler_id.into(),
        }
    }

    /// Compile a glob-like pattern to regex
    fn compile_pattern(pattern: &str) -> Option<Regex> {
        // If pattern contains glob special chars, compile as regex
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            // R1-M113: Don't escape [ and ] — they're valid glob character classes
            let regex_str = pattern
                .replace('.', r"\.")
                .replace('*', ".*")
                .replace('?', ".");
            Regex::new(&format!("^{}$", regex_str)).ok()
        } else {
            None
        }
    }

    /// Check if this pattern matches a string
    pub fn matches(&self, text: &str) -> bool {
        if let Some(regex) = &self.regex {
            regex.is_match(text)
        } else {
            self.pattern == text
        }
    }
}

impl HookMatcher {
    /// Create a new hook matcher
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a tool pattern for an event
    pub fn add_tool_pattern(&mut self, event: HookEvent, pattern: MatchPattern) {
        self.tool_patterns.entry(event).or_default().push(pattern);
    }

    /// Add a path pattern for an event
    pub fn add_path_pattern(&mut self, event: HookEvent, pattern: MatchPattern) {
        self.path_patterns.entry(event).or_default().push(pattern);
    }

    /// Add a command pattern for an event
    pub fn add_command_pattern(&mut self, event: HookEvent, pattern: MatchPattern) {
        self.command_patterns
            .entry(event)
            .or_default()
            .push(pattern);
    }

    /// Find matching handler IDs for a tool
    pub fn match_tool(&self, event: HookEvent, tool_name: &str) -> Vec<String> {
        self.tool_patterns
            .get(&event)
            .map(|patterns| {
                patterns
                    .iter()
                    .filter(|p| p.matches(tool_name))
                    .map(|p| p.handler_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Find matching handler IDs for a path
    pub fn match_path(&self, event: HookEvent, path: &str) -> Vec<String> {
        self.path_patterns
            .get(&event)
            .map(|patterns| {
                patterns
                    .iter()
                    .filter(|p| p.matches(path))
                    .map(|p| p.handler_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Find matching handler IDs for a command
    pub fn match_command(&self, event: HookEvent, command: &str) -> Vec<String> {
        self.command_patterns
            .get(&event)
            .map(|patterns| {
                patterns
                    .iter()
                    .filter(|p| p.matches(command))
                    .map(|p| p.handler_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if any patterns match for an event
    pub fn has_match(
        &self,
        event: HookEvent,
        tool_name: Option<&str>,
        path: Option<&str>,
        command: Option<&str>,
    ) -> bool {
        if let Some(tool) = tool_name {
            if !self.match_tool(event, tool).is_empty() {
                return true;
            }
        }
        if let Some(path) = path {
            if !self.match_path(event, path).is_empty() {
                return true;
            }
        }
        if let Some(cmd) = command {
            if !self.match_command(event, cmd).is_empty() {
                return true;
            }
        }
        false
    }

    /// Get all handler IDs that match
    pub fn get_matching_handlers(
        &self,
        event: HookEvent,
        tool_name: Option<&str>,
        path: Option<&str>,
        command: Option<&str>,
    ) -> Vec<String> {
        let mut handlers = Vec::new();

        if let Some(tool) = tool_name {
            handlers.extend(self.match_tool(event, tool));
        }
        if let Some(path) = path {
            handlers.extend(self.match_path(event, path));
        }
        if let Some(cmd) = command {
            handlers.extend(self.match_command(event, cmd));
        }

        // Deduplicate
        handlers.sort();
        handlers.dedup();
        handlers
    }

    /// Clear all patterns for an event
    pub fn clear_event(&mut self, event: HookEvent) {
        self.tool_patterns.remove(&event);
        self.path_patterns.remove(&event);
        self.command_patterns.remove(&event);
    }

    /// Clear all patterns
    pub fn clear_all(&mut self) {
        self.tool_patterns.clear();
        self.path_patterns.clear();
        self.command_patterns.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_pattern_exact() {
        let pattern = MatchPattern::new("Bash", "handler1");
        assert!(pattern.matches("Bash"));
        assert!(!pattern.matches("BashTool"));
    }

    #[test]
    fn test_match_pattern_wildcard() {
        let pattern = MatchPattern::new("Bash*", "handler1");
        assert!(pattern.matches("Bash"));
        assert!(pattern.matches("BashTool"));
        assert!(!pattern.matches("Shell"));

        let pattern = MatchPattern::new("*Tool", "handler2");
        assert!(pattern.matches("BashTool"));
        assert!(pattern.matches("ReadTool"));
        assert!(pattern.matches("Tool")); // "*" matches empty string too

        let pattern = MatchPattern::new("*ash*", "handler3");
        assert!(pattern.matches("Bash"));
        assert!(pattern.matches("BashTool"));
        assert!(pattern.matches("SlashCommand"));
    }

    #[test]
    fn test_hook_matcher_tool() {
        let mut matcher = HookMatcher::new();
        matcher.add_tool_pattern(
            HookEvent::PreToolUse,
            MatchPattern::new("Bash*", "bash_handler"),
        );
        matcher.add_tool_pattern(
            HookEvent::PreToolUse,
            MatchPattern::new("*File*", "file_handler"),
        );

        let handlers = matcher.match_tool(HookEvent::PreToolUse, "BashExecute");
        assert_eq!(handlers, vec!["bash_handler"]);

        let handlers = matcher.match_tool(HookEvent::PreToolUse, "ReadFile");
        assert_eq!(handlers, vec!["file_handler"]);

        let handlers = matcher.match_tool(HookEvent::PreToolUse, "Unknown");
        assert!(handlers.is_empty());
    }

    #[test]
    fn test_hook_matcher_combined() {
        let mut matcher = HookMatcher::new();
        matcher.add_tool_pattern(
            HookEvent::PreToolUse,
            MatchPattern::new("Bash", "tool_handler"),
        );
        matcher.add_path_pattern(
            HookEvent::PreToolUse,
            MatchPattern::new("/etc/*", "path_handler"),
        );

        assert!(matcher.has_match(HookEvent::PreToolUse, Some("Bash"), None, None));
        assert!(matcher.has_match(HookEvent::PreToolUse, None, Some("/etc/passwd"), None));
        assert!(!matcher.has_match(HookEvent::PreToolUse, Some("Read"), Some("/tmp/test"), None));

        let handlers = matcher.get_matching_handlers(
            HookEvent::PreToolUse,
            Some("Bash"),
            Some("/etc/passwd"),
            None,
        );
        assert_eq!(handlers.len(), 2);
    }
}
