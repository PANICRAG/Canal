//! ActionChainExecutor — sequential CU tool execution with screen change detection.
//!
//! When the LLM returns multiple CU tool calls in a batch, they must execute
//! sequentially (not in parallel) because each action changes the screen state.
//!
//! Uses LOCAL baseline pHash (not shared ScreenChangeDetector) to avoid
//! race conditions with CV5 ScreenMonitor background polling.

use std::sync::Arc;

use crate::phash::{compute_phash, hash_similarity};
use crate::screen_controller::ScreenController;

/// All computer use tool names recognized by the pipeline.
pub const COMPUTER_USE_TOOLS: &[&str] = &[
    "computer_screenshot",
    "computer_click",
    "computer_click_at",
    "computer_type",
    "computer_key",
    "computer_scroll",
    "computer_drag",
    "computer_act",
    "computer_extract",
    "computer_observe",
];

/// Tools that mutate screen state (everything except read-only tools).
const MUTATING_TOOLS: &[&str] = &[
    "computer_click",
    "computer_click_at",
    "computer_type",
    "computer_key",
    "computer_scroll",
    "computer_drag",
    "computer_act",
];

/// Check if a tool name is a computer use tool.
pub fn is_computer_use_tool(tool_name: &str) -> bool {
    COMPUTER_USE_TOOLS.contains(&tool_name)
}

/// Check if a tool name mutates screen state.
pub fn is_mutating_tool(tool_name: &str) -> bool {
    MUTATING_TOOLS.contains(&tool_name)
}

/// Configuration for action chain execution.
#[derive(Debug, Clone)]
pub struct ChainConfig {
    /// pHash similarity threshold below which screen is "significantly changed".
    /// When a significant change is detected mid-chain, remaining tools are skipped.
    pub change_threshold: f32,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            change_threshold: 0.85,
        }
    }
}

/// Sequential executor for computer use tool chains.
///
/// When the agent returns multiple CU tools in one response, they execute
/// one-by-one with screen change detection between them. If a significant
/// screen change is detected (e.g., navigation), remaining tools are skipped
/// to let the agent re-evaluate the new screen state.
pub struct ActionChainExecutor {
    controller: Arc<dyn ScreenController>,
    config: ChainConfig,
}

impl ActionChainExecutor {
    /// Create a new chain executor.
    pub fn new(controller: Arc<dyn ScreenController>, config: ChainConfig) -> Self {
        Self { controller, config }
    }

    /// Check if any tool in the batch is a CU tool.
    pub fn has_cu_tools(&self, tool_names: &[&str]) -> bool {
        tool_names.iter().any(|name| is_computer_use_tool(name))
    }

    /// Capture current screen and compute pHash for baseline comparison.
    pub async fn capture_baseline(&self) -> Option<u64> {
        match self.controller.capture().await {
            Ok(cap) => Some(compute_phash(&cap.base64)),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to capture baseline for chain execution");
                None
            }
        }
    }

    /// Check if the screen has significantly changed since baseline.
    ///
    /// Returns `(changed, similarity)`.
    pub async fn check_change(&self, baseline_hash: u64) -> (bool, f32) {
        match self.controller.capture().await {
            Ok(cap) => {
                let current_hash = compute_phash(&cap.base64);
                let similarity = hash_similarity(baseline_hash, current_hash);
                (similarity < self.config.change_threshold, similarity)
            }
            Err(_) => (false, 1.0), // Can't capture = assume no change
        }
    }

    /// Get the chain config.
    pub fn config(&self) -> &ChainConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_computer_use_tool() {
        assert!(is_computer_use_tool("computer_click"));
        assert!(is_computer_use_tool("computer_act"));
        assert!(is_computer_use_tool("computer_screenshot"));
        assert!(is_computer_use_tool("computer_observe"));
        assert!(!is_computer_use_tool("read_file"));
        assert!(!is_computer_use_tool("bash"));
        assert!(!is_computer_use_tool("search"));
    }

    #[test]
    fn test_is_mutating_tool() {
        assert!(is_mutating_tool("computer_click"));
        assert!(is_mutating_tool("computer_type"));
        assert!(is_mutating_tool("computer_drag"));
        assert!(is_mutating_tool("computer_act"));
        assert!(!is_mutating_tool("computer_screenshot"));
        assert!(!is_mutating_tool("computer_extract"));
        assert!(!is_mutating_tool("computer_observe"));
    }

    #[test]
    fn test_chain_config_defaults() {
        let config = ChainConfig::default();
        assert!((config.change_threshold - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn test_has_cu_tools() {
        let controller = Arc::new(crate::NoopScreenController::new());
        let executor = ActionChainExecutor::new(controller, ChainConfig::default());

        assert!(executor.has_cu_tools(&["computer_click", "read_file"]));
        assert!(executor.has_cu_tools(&["computer_act"]));
        assert!(!executor.has_cu_tools(&["read_file", "bash"]));
        assert!(!executor.has_cu_tools(&[]));
    }

    #[tokio::test]
    async fn test_capture_baseline_noop() {
        let controller = Arc::new(crate::NoopScreenController::new());
        let executor = ActionChainExecutor::new(controller, ChainConfig::default());
        // NoopScreenController returns error, so baseline should be None
        let baseline = executor.capture_baseline().await;
        assert!(baseline.is_none());
    }
}
