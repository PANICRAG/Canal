//! Browser Automation Tool for Agent
//!
//! Provides browser control capabilities to the AI agent via Firecracker VMs.
//! Supports navigation, screenshots, clicking, filling forms, and JS execution.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use super::context::ToolContext;
use super::traits::{AgentTool, ToolError, ToolResult};
use crate::vm::{BrowserAction, VmExecutor, VmManager};

/// Browser tool input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserInput {
    /// Action to perform: navigate, screenshot, click, fill, evaluate, snapshot, wait, content, back, forward, reload
    pub action: String,
    /// URL for navigation
    #[serde(default)]
    pub url: Option<String>,
    /// CSS selector for click/fill/wait actions
    #[serde(default)]
    pub selector: Option<String>,
    /// Value for fill actions
    #[serde(default)]
    pub value: Option<String>,
    /// JavaScript code for evaluate action
    #[serde(default)]
    pub script: Option<String>,
    /// Whether to capture full page screenshot
    #[serde(default)]
    pub full_page: Option<bool>,
    /// Timeout in milliseconds for the action
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Browser tool output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserOutput {
    /// Whether the action succeeded
    pub success: bool,
    /// Result data (varies by action)
    pub data: Option<serde_json::Value>,
    /// Error message if failed
    pub error: Option<String>,
    /// Action duration in milliseconds
    pub duration_ms: u64,
    /// The VM instance ID used
    pub vm_id: String,
}

/// Browser automation tool for the AI agent
pub struct BrowserTool {
    /// VM manager for acquiring browser VMs
    vm_manager: Arc<VmManager>,
    /// Default timeout for browser operations
    default_timeout: Duration,
}

impl BrowserTool {
    /// Create a new browser tool
    pub fn new(vm_manager: Arc<VmManager>) -> Self {
        Self {
            vm_manager,
            default_timeout: Duration::from_secs(30),
        }
    }

    /// Set default timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Convert BrowserInput to BrowserAction
    fn to_browser_action(input: &BrowserInput) -> ToolResult<BrowserAction> {
        match input.action.as_str() {
            "navigate" => {
                let url = input
                    .url
                    .as_ref()
                    .ok_or_else(|| {
                        ToolError::InvalidInput(
                            "'url' is required for navigate action".into(),
                        )
                    })?;
                Ok(BrowserAction::Navigate {
                    url: url.clone(),
                    wait_until: "load".to_string(),
                    timeout: input.timeout.unwrap_or(30000),
                })
            }
            "screenshot" => Ok(BrowserAction::Screenshot {
                full_page: input.full_page.unwrap_or(false),
                image_type: "png".to_string(),
                quality: None,
                selector: input.selector.clone(),
            }),
            "click" => {
                let selector = input.selector.as_ref().ok_or_else(|| {
                    ToolError::InvalidInput(
                        "'selector' is required for click action".into(),
                    )
                })?;
                Ok(BrowserAction::Click {
                    selector: selector.clone(),
                    button: "left".to_string(),
                    click_count: 1,
                    delay: 0,
                })
            }
            "fill" => {
                let selector = input.selector.as_ref().ok_or_else(|| {
                    ToolError::InvalidInput(
                        "'selector' is required for fill action".into(),
                    )
                })?;
                let value = input.value.as_ref().ok_or_else(|| {
                    ToolError::InvalidInput(
                        "'value' is required for fill action".into(),
                    )
                })?;
                Ok(BrowserAction::Fill {
                    selector: selector.clone(),
                    value: value.clone(),
                    timeout: input.timeout.unwrap_or(30000),
                })
            }
            "evaluate" | "execute" => {
                let script = input.script.as_ref().ok_or_else(|| {
                    ToolError::InvalidInput(
                        "'script' is required for evaluate action".into(),
                    )
                })?;
                Ok(BrowserAction::Execute {
                    script: script.clone(),
                    arg: None,
                })
            }
            "snapshot" => Ok(BrowserAction::Snapshot),
            "wait" => {
                let selector = input.selector.as_ref().ok_or_else(|| {
                    ToolError::InvalidInput(
                        "'selector' is required for wait action".into(),
                    )
                })?;
                Ok(BrowserAction::Wait {
                    selector: selector.clone(),
                    timeout: input.timeout.unwrap_or(5000),
                    state: "visible".to_string(),
                })
            }
            "content" => Ok(BrowserAction::Content),
            "back" => Ok(BrowserAction::Back),
            "forward" => Ok(BrowserAction::Forward),
            "reload" => Ok(BrowserAction::Reload {
                wait_until: "load".to_string(),
            }),
            other => Err(ToolError::InvalidInput(format!(
                "Unknown browser action: '{}'. Supported: navigate, screenshot, click, fill, evaluate, snapshot, wait, content, back, forward, reload",
                other
            ))),
        }
    }
}

#[async_trait]
impl AgentTool for BrowserTool {
    type Input = BrowserInput;
    type Output = BrowserOutput;

    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Control a web browser to navigate pages, take screenshots, click elements, fill forms, and execute JavaScript. The browser runs in an isolated Firecracker VM."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Browser action to perform",
                    "enum": ["navigate", "screenshot", "click", "fill", "evaluate", "snapshot", "wait", "content", "back", "forward", "reload"]
                },
                "url": {
                    "type": "string",
                    "description": "URL for navigate action"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for click/fill/wait actions"
                },
                "value": {
                    "type": "string",
                    "description": "Value for fill action"
                },
                "script": {
                    "type": "string",
                    "description": "JavaScript code for evaluate action"
                },
                "full_page": {
                    "type": "boolean",
                    "description": "Capture full page screenshot (default: false)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds"
                }
            },
            "required": ["action"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "browser"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let action = Self::to_browser_action(&input)?;

        // Acquire VM for this action
        let instance = self.vm_manager.acquire().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to acquire browser VM: {}", e))
        })?;

        let vm_id = instance.id.clone();
        let timeout = input
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(self.default_timeout);
        let executor = VmExecutor::new(&instance, timeout);

        // Execute browser action
        let result = executor
            .execute_browser(action)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Browser action failed: {}", e)));

        // Release VM back to pool
        if let Err(e) = self.vm_manager.release(instance).await {
            warn!(vm_id = %vm_id, error = %e, "Failed to release browser VM");
        }

        let result = result?;

        Ok(BrowserOutput {
            success: result.success,
            data: result.data,
            error: result.error,
            duration_ms: result.duration_ms,
            vm_id,
        })
    }
}
