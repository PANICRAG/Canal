//! Tool filter context for dynamic tool schema filtering.

/// Context for filtering tools based on task requirements.
///
/// This enables dynamic tool filtering to reduce token consumption by only
/// including relevant tools in LLM requests.
#[derive(Debug, Default, Clone)]
pub struct ToolFilterContext {
    /// Whether the current task involves browser automation
    pub is_browser_task: bool,
    /// Whether worker orchestration is enabled
    pub workers_enabled: bool,
    /// Whether code orchestration is enabled
    pub code_orchestration_enabled: bool,
    /// Recent tool uses (for potential future optimization)
    pub recent_tool_uses: Vec<String>,
}

impl ToolFilterContext {
    /// Create a new filter context
    pub fn new() -> Self {
        Self::default()
    }

    /// Set browser task flag
    pub fn browser_task(mut self, enabled: bool) -> Self {
        self.is_browser_task = enabled;
        self
    }

    /// Set workers enabled flag
    pub fn workers_enabled(mut self, enabled: bool) -> Self {
        self.workers_enabled = enabled;
        self
    }

    /// Set code orchestration enabled flag
    pub fn code_orchestration_enabled(mut self, enabled: bool) -> Self {
        self.code_orchestration_enabled = enabled;
        self
    }

    /// Detect if a message indicates a browser task
    pub fn detect_browser_task(message: &str) -> bool {
        let keywords = [
            "browser",
            "navigate",
            "click",
            "fill",
            "screenshot",
            "gmail",
            "web",
            "page",
            "website",
            "url",
            "login",
            "form",
            "button",
            "link",
            "scroll",
            "http",
            "https",
        ];
        let lower = message.to_lowercase();
        keywords.iter().any(|k| lower.contains(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_filter_context_default() {
        let context = ToolFilterContext::new();
        assert!(!context.is_browser_task);
        assert!(!context.workers_enabled);
        assert!(!context.code_orchestration_enabled);
    }

    #[test]
    fn test_tool_filter_context_builder() {
        let context = ToolFilterContext::new()
            .browser_task(true)
            .workers_enabled(true)
            .code_orchestration_enabled(false);

        assert!(context.is_browser_task);
        assert!(context.workers_enabled);
        assert!(!context.code_orchestration_enabled);
    }

    #[test]
    fn test_detect_browser_task() {
        assert!(ToolFilterContext::detect_browser_task(
            "Navigate to the website"
        ));
        assert!(ToolFilterContext::detect_browser_task(
            "Click the login button"
        ));
        assert!(!ToolFilterContext::detect_browser_task("Write a function"));
        assert!(!ToolFilterContext::detect_browser_task("Read the file"));
    }
}
