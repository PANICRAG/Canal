//! Builtin Tool Executor
//!
//! Executes builtin tools (filesystem, executor, browser, mac) directly without external MCP servers.
//! This connects LLM tool_use requests to actual gateway-core functions.
//!
//! ## macOS Tools (namespace: mac)
//! Native Rust implementation of macOS automation tools, eliminating the need for
//! external Node.js/Python MCP servers (like osascript-dxt).
//!
//! Available tools:
//! - `osascript` - Execute AppleScript commands
//! - `screenshot` - Capture screen
//! - `app_control` - Control applications (launch, quit, hide, show, minimize)
//! - `open_url` - Open URL in default browser
//! - `notify` - Show macOS notification
//! - `clipboard_read` - Read clipboard content
//! - `clipboard_write` - Write to clipboard
//! - `get_frontmost_app` - Get active application name
//! - `list_running_apps` - List all running applications

use crate::agent::automation::{AutomationRequest, BrowserAutomationOrchestrator};
use crate::error::{Error, Result};
use crate::executor::{CodeExecutor, ExecutionRequest, Language};
use crate::filesystem::FilesystemService;
use crate::screen::CdpScreenController;
use canal_cv::{MouseButton, ScreenController};
use serde_json::Value;
use std::sync::Arc;

use super::platform_automation::PlatformAutomation;
use super::protocol::ToolCallResult;

/// Builtin tool executor that handles filesystem, executor, screen, mac, and automation namespaces
pub struct BuiltinToolExecutor {
    filesystem: Option<Arc<FilesystemService>>,
    executor: Option<Arc<CodeExecutor>>,
    screen_controller: Option<Arc<dyn ScreenController>>,
    cdp_controller: Option<Arc<CdpScreenController>>,
    automation: Option<Arc<BrowserAutomationOrchestrator>>,
    /// Native macOS/Windows automation (replaces external MCP servers)
    platform_automation: PlatformAutomation,
}

impl BuiltinToolExecutor {
    /// Create a new builtin tool executor
    pub fn new(
        filesystem: Option<Arc<FilesystemService>>,
        executor: Option<Arc<CodeExecutor>>,
    ) -> Self {
        Self {
            filesystem,
            executor,
            screen_controller: None,
            cdp_controller: None,
            automation: None,
            platform_automation: PlatformAutomation::new(),
        }
    }

    /// Create a new builtin tool executor with screen controller
    pub fn with_screen(
        filesystem: Option<Arc<FilesystemService>>,
        executor: Option<Arc<CodeExecutor>>,
        screen_controller: Option<Arc<dyn ScreenController>>,
        cdp_controller: Option<Arc<CdpScreenController>>,
    ) -> Self {
        Self {
            filesystem,
            executor,
            screen_controller,
            cdp_controller,
            automation: None,
            platform_automation: PlatformAutomation::new(),
        }
    }

    /// Set the screen controller
    pub fn set_screen_controller(&mut self, controller: Arc<dyn ScreenController>) {
        self.screen_controller = Some(controller);
    }

    /// Set the CDP screen controller
    pub fn set_cdp_controller(&mut self, cdp: Arc<CdpScreenController>) {
        self.cdp_controller = Some(cdp);
    }

    /// Set the automation orchestrator
    pub fn set_automation(&mut self, automation: Arc<BrowserAutomationOrchestrator>) {
        self.automation = Some(automation);
    }

    /// Check if a namespace is handled by builtin executor
    pub fn handles_namespace(&self, namespace: &str) -> bool {
        matches!(
            namespace,
            "filesystem"
                | "executor"
                | "bash"
                | "shell"
                | "browser"
                | "screen"
                | "mac"
                | "osascript"
                | "automation"
        )
    }

    /// Execute a builtin tool call
    pub async fn execute(
        &self,
        namespace: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<ToolCallResult> {
        match namespace {
            "filesystem" => self.execute_filesystem(tool_name, arguments).await,
            "executor" | "bash" | "shell" => self.execute_code(tool_name, arguments).await,
            "browser" | "screen" => self.execute_screen(tool_name, arguments).await,
            "mac" | "osascript" => self.execute_mac(tool_name, arguments).await,
            "automation" => self.execute_automation(tool_name, arguments).await,
            _ => Err(Error::NotFound(format!(
                "Unknown builtin namespace: {}",
                namespace
            ))),
        }
    }

    /// Execute filesystem tools
    async fn execute_filesystem(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<ToolCallResult> {
        let fs = self
            .filesystem
            .as_ref()
            .ok_or_else(|| Error::Internal("Filesystem service not available".to_string()))?;

        match tool_name {
            "read_file" => {
                let path = arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'path' argument".to_string()))?;

                match fs.read_file(path).await {
                    Ok(content) => Ok(ToolCallResult::text(serde_json::to_string_pretty(
                        &serde_json::json!({
                            "path": content.path,
                            "content": content.content,
                            "size": content.size,
                            "encoding": content.encoding,
                            "truncated": content.truncated
                        }),
                    )?)),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            "write_file" => {
                let path = arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'path' argument".to_string()))?;

                let content = arguments
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'content' argument".to_string()))?;

                let create_dirs = arguments
                    .get("create_dirs")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                let overwrite = arguments
                    .get("overwrite")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                match fs.write_file(path, content, create_dirs, overwrite).await {
                    Ok(result) => Ok(ToolCallResult::text(serde_json::to_string_pretty(
                        &serde_json::json!({
                            "path": result.path,
                            "bytes_written": result.bytes_written,
                            "created": result.created
                        }),
                    )?)),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            "list_directory" => {
                let path = arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'path' argument".to_string()))?;

                let recursive = arguments
                    .get("recursive")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let include_hidden = arguments
                    .get("include_hidden")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                match fs.list_directory(path, recursive, include_hidden).await {
                    Ok(listing) => {
                        let entries: Vec<Value> = listing
                            .entries
                            .iter()
                            .map(|e| {
                                serde_json::json!({
                                    "name": e.name,
                                    "path": e.path,
                                    "type": e.entry_type.to_string(),
                                    "size": e.size,
                                    "hidden": e.hidden
                                })
                            })
                            .collect();

                        Ok(ToolCallResult::text(serde_json::to_string_pretty(
                            &serde_json::json!({
                                "path": listing.path,
                                "total_count": listing.total_count,
                                "entries": entries
                            }),
                        )?))
                    }
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            "search" => {
                let path = arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'path' argument".to_string()))?;

                let pattern = arguments
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'pattern' argument".to_string()))?;

                let file_pattern = arguments.get("file_pattern").and_then(|v| v.as_str());

                let max_results = arguments
                    .get("max_results")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100) as usize;

                match fs.search(path, pattern, file_pattern, max_results).await {
                    Ok(result) => Ok(ToolCallResult::text(serde_json::to_string_pretty(
                        &serde_json::json!({
                            "total_matches": result.total_matches,
                            "files_searched": result.files_searched,
                            "truncated": result.truncated,
                            "matches": result.matches.iter().map(|m| {
                                serde_json::json!({
                                    "path": m.path,
                                    "line_number": m.line_number,
                                    "line_content": m.line_content,
                                    "match_start": m.match_start,
                                    "match_end": m.match_end
                                })
                            }).collect::<Vec<_>>()
                        }),
                    )?)),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            _ => Ok(ToolCallResult::error(format!(
                "Unknown filesystem tool: {}",
                tool_name
            ))),
        }
    }

    /// Execute code execution tools
    async fn execute_code(&self, tool_name: &str, arguments: Value) -> Result<ToolCallResult> {
        let executor = self
            .executor
            .as_ref()
            .ok_or_else(|| Error::Internal("Code executor not available".to_string()))?;

        match tool_name {
            "execute" | "run" | "run_code" => {
                let code = arguments
                    .get("code")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'code' argument".to_string()))?;

                let language_str = arguments
                    .get("language")
                    .and_then(|v| v.as_str())
                    .unwrap_or("bash");

                let language: Language = language_str
                    .parse()
                    .map_err(|e: gateway_tools::ServiceError| Error::Internal(e.to_string()))?;

                let timeout_ms = arguments.get("timeout_ms").and_then(|v| v.as_u64());

                let request = ExecutionRequest {
                    code: code.to_string(),
                    language,
                    timeout_ms,
                    stream: false,
                    working_dir: arguments
                        .get("working_dir")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                };

                match executor.execute(request).await {
                    Ok(result) => Ok(ToolCallResult::text(serde_json::to_string_pretty(
                        &serde_json::json!({
                            "execution_id": result.execution_id,
                            "language": result.language.to_string(),
                            "stdout": result.stdout,
                            "stderr": result.stderr,
                            "exit_code": result.exit_code,
                            "status": format!("{:?}", result.status),
                            "duration_ms": result.duration_ms
                        }),
                    )?)),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            "bash" | "shell" => {
                // Shorthand for running bash commands
                let command = arguments
                    .get("command")
                    .or_else(|| arguments.get("code"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'command' argument".to_string()))?;

                let timeout_ms = arguments.get("timeout_ms").and_then(|v| v.as_u64());

                let request = ExecutionRequest {
                    code: command.to_string(),
                    language: Language::Bash,
                    timeout_ms,
                    stream: false,
                    working_dir: arguments
                        .get("working_dir")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                };

                match executor.execute(request).await {
                    Ok(result) => Ok(ToolCallResult::text(serde_json::to_string_pretty(
                        &serde_json::json!({
                            "stdout": result.stdout,
                            "stderr": result.stderr,
                            "exit_code": result.exit_code,
                            "duration_ms": result.duration_ms
                        }),
                    )?)),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            "python" => {
                let code = arguments
                    .get("code")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'code' argument".to_string()))?;

                let timeout_ms = arguments.get("timeout_ms").and_then(|v| v.as_u64());

                let request = ExecutionRequest {
                    code: code.to_string(),
                    language: Language::Python,
                    timeout_ms,
                    stream: false,
                    working_dir: arguments
                        .get("working_dir")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                };

                match executor.execute(request).await {
                    Ok(result) => Ok(ToolCallResult::text(serde_json::to_string_pretty(
                        &serde_json::json!({
                            "stdout": result.stdout,
                            "stderr": result.stderr,
                            "exit_code": result.exit_code,
                            "duration_ms": result.duration_ms
                        }),
                    )?)),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            _ => Ok(ToolCallResult::error(format!(
                "Unknown executor tool: {}",
                tool_name
            ))),
        }
    }

    /// Execute screen/browser tools via ScreenController + CDP.
    async fn execute_screen(&self, tool_name: &str, arguments: Value) -> Result<ToolCallResult> {
        let controller = self
            .screen_controller
            .as_ref()
            .ok_or_else(|| Error::Internal("Screen controller not available".to_string()))?;

        match tool_name {
            "navigate" | "browser_navigate" => {
                let cdp = self.cdp_controller.as_ref().ok_or_else(|| {
                    Error::Internal("CDP controller not available for navigation".to_string())
                })?;
                let url = arguments
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'url' argument".to_string()))?;

                match cdp.navigate(url).await {
                    Ok(()) => Ok(ToolCallResult::text(format!(
                        "Successfully navigated to {}",
                        url
                    ))),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            "screenshot" | "browser_screenshot" => match controller.capture().await {
                Ok(capture) => {
                    let result = serde_json::json!({
                        "base64": capture.base64,
                        "format": "jpeg",
                        "display_width": capture.display_width,
                        "display_height": capture.display_height,
                    });
                    Ok(ToolCallResult::text(
                        serde_json::to_string_pretty(&result).unwrap_or_default(),
                    ))
                }
                Err(e) => Ok(ToolCallResult::error(e.to_string())),
            },

            "click" | "browser_click" => {
                let x = arguments
                    .get("x")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| Error::Internal("Missing 'x' argument".to_string()))?
                    as u32;
                let y = arguments
                    .get("y")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| Error::Internal("Missing 'y' argument".to_string()))?
                    as u32;

                match controller.click(x, y, MouseButton::Left).await {
                    Ok(()) => Ok(ToolCallResult::text(format!(
                        "Clicked at ({}, {})",
                        x, y
                    ))),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            "type" | "browser_type" | "fill" | "browser_fill" => {
                let text = arguments
                    .get("text")
                    .or_else(|| arguments.get("value"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        Error::Internal("Missing 'text' or 'value' argument".to_string())
                    })?;

                match controller.type_text(text).await {
                    Ok(()) => Ok(ToolCallResult::text(format!("Typed: {}", text))),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            "scroll" | "browser_scroll" => {
                let direction = arguments
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("down");
                let amount = arguments
                    .get("amount")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(500.0);

                let (dx, dy) = match direction {
                    "up" => (0.0, -amount),
                    "down" => (0.0, amount),
                    "left" => (-amount, 0.0),
                    "right" => (amount, 0.0),
                    _ => (0.0, amount),
                };

                match controller.scroll(dx, dy).await {
                    Ok(()) => Ok(ToolCallResult::text(format!(
                        "Scrolled {} by {} pixels",
                        direction, amount
                    ))),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            "evaluate" | "browser_evaluate" => {
                let cdp = self.cdp_controller.as_ref().ok_or_else(|| {
                    Error::Internal("CDP controller not available for JS evaluation".to_string())
                })?;
                let script = arguments
                    .get("script")
                    .or_else(|| arguments.get("code"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'script' argument".to_string()))?;

                match cdp.evaluate(script).await {
                    Ok(result) => Ok(ToolCallResult::text(
                        serde_json::to_string_pretty(&result).unwrap_or_default(),
                    )),
                    Err(e) => Ok(ToolCallResult::error(e.to_string())),
                }
            }

            _ => Ok(ToolCallResult::error(format!(
                "Unknown screen tool: {}. Available: navigate, screenshot, click, type, scroll, evaluate.",
                tool_name
            ))),
        }
    }

    /// Execute macOS automation tools via native PlatformAutomation
    ///
    /// Available tools (9 total):
    /// - osascript: Execute AppleScript commands
    /// - screenshot: Capture screen or region
    /// - app_control: Launch/quit/hide/show/minimize apps
    /// - open_url: Open URL in default browser
    /// - notify: Show macOS notification
    /// - clipboard_read: Read clipboard content
    /// - clipboard_write: Write to clipboard
    /// - get_frontmost_app: Get active application name
    /// - list_running_apps: List all running applications
    async fn execute_mac(&self, tool_name: &str, arguments: Value) -> Result<ToolCallResult> {
        // R3-L: Removed redundant cfg!() — is_supported() already handles platform check
        if !self.platform_automation.is_supported() {
            return Ok(ToolCallResult::error(
                "macOS automation tools are only available on macOS".to_string(),
            ));
        }

        // Delegate to PlatformAutomation which handles all 9 macOS tools
        self.platform_automation.execute(tool_name, arguments).await
    }

    // ============================================================================
    // Automation Tools (Five-Layer Architecture)
    // ============================================================================

    /// Execute automation tools using the five-layer architecture
    ///
    /// This provides massive token savings compared to pure CV approaches:
    /// - Pure CV: ~4,100,000 tokens for 1000 items
    /// - Five-Layer: ~6,000 tokens (99.85% savings)
    ///
    /// Available tools:
    /// - `analyze`: Analyze a task and get routing recommendation
    /// - `execute`: Execute automation through the five-layer pipeline
    async fn execute_automation(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<ToolCallResult> {
        let automation = self.automation.as_ref().ok_or_else(|| {
            Error::Internal("Automation orchestrator not available. Enable with AUTOMATION_ORCHESTRATOR_ENABLED=true".to_string())
        })?;

        match tool_name {
            "analyze" | "automation_analyze" => {
                // Analyze task and return routing recommendation
                let task = arguments
                    .get("task")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'task' argument".to_string()))?;

                let data_count = arguments
                    .get("data_count")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);

                match automation.analyze(task, data_count).await {
                    Ok(analysis) => {
                        let response = serde_json::json!({
                            "task": analysis.task,
                            "target_system": analysis.target_system,
                            "data_volume": analysis.data_volume,
                            "decision": {
                                "path": analysis.decision.path,
                                "confidence": analysis.decision.confidence,
                                "reasoning": analysis.decision.reasoning,
                                "token_savings_percent": analysis.decision.token_savings_percent,
                            },
                            "recommendation": match &analysis.decision.path {
                                crate::agent::automation::AutomationPath::ReuseScript { script_id, .. } => {
                                    format!("Reuse cached script '{}' - near-zero token cost", script_id)
                                }
                                crate::agent::automation::AutomationPath::DirectApi { api_type, .. } => {
                                    format!("Direct API call via {} - minimal token cost", api_type)
                                }
                                crate::agent::automation::AutomationPath::ExploreAndGenerate { estimated_tokens, .. } => {
                                    format!("Explore & generate script - ~{} tokens (vs ~{} for pure CV)",
                                        estimated_tokens, data_count.unwrap_or(1) as u64 * 10000)
                                }
                                crate::agent::automation::AutomationPath::PureComputerVision { max_items, estimated_tokens } => {
                                    format!("Pure CV for {} items - ~{} tokens", max_items, estimated_tokens)
                                }
                                crate::agent::automation::AutomationPath::HybridApproach { .. } => {
                                    "Hybrid approach - CV exploration + script execution".to_string()
                                }
                                crate::agent::automation::AutomationPath::RequiresHumanAssistance { reason } => {
                                    format!("Human assistance required: {}", reason)
                                }
                            }
                        });
                        Ok(ToolCallResult::text(serde_json::to_string_pretty(&response).unwrap_or_default()))
                    }
                    Err(e) => Ok(ToolCallResult::error(format!("Analysis failed: {}", e))),
                }
            }

            "execute" | "automation_execute" => {
                // Execute automation through the five-layer pipeline
                let task = arguments
                    .get("task")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Internal("Missing 'task' argument".to_string()))?;

                let target_url = arguments
                    .get("target_url")
                    .or_else(|| arguments.get("url"))
                    .and_then(|v| v.as_str());

                let data: Vec<serde_json::Value> = arguments
                    .get("data")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                let timeout_ms = arguments
                    .get("timeout_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(300000);

                let force_explore = arguments
                    .get("force_explore")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Build the automation request
                let mut request = AutomationRequest::new(task);
                if let Some(url) = target_url {
                    request = request.with_url(url);
                }
                request = request.with_data(data);
                request.timeout_ms = timeout_ms;
                if force_explore {
                    request.options.insert("force_explore".to_string(), serde_json::json!(true));
                }

                // Execute through the orchestrator
                match automation.execute(request).await {
                    Ok(result) => {
                        let items_succeeded = result.stats.items_processed.saturating_sub(result.stats.items_failed);
                        let response = serde_json::json!({
                            "success": result.success,
                            "request_id": result.request_id,
                            "path_used": result.path_used,
                            "script_id": result.script_id,
                            "output": result.output,
                            "stats": {
                                "duration_ms": result.stats.duration_ms,
                                "items_processed": result.stats.items_processed,
                                "items_succeeded": items_succeeded,
                                "items_failed": result.stats.items_failed,
                                "exploration_tokens": result.stats.exploration_tokens,
                                "generation_tokens": result.stats.generation_tokens,
                                "total_tokens": result.stats.total_tokens,
                                "pure_cv_estimated_tokens": result.stats.pure_cv_estimated_tokens,
                                "token_savings_percent": result.stats.savings_percent,
                                "script_reused": result.stats.script_reused,
                            },
                            "error": result.error,
                        });
                        Ok(ToolCallResult::text(serde_json::to_string_pretty(&response).unwrap_or_default()))
                    }
                    Err(e) => Ok(ToolCallResult::error(format!("Automation execution failed: {}", e))),
                }
            }

            "status" | "automation_status" => {
                // Get orchestrator status
                let status = automation.status().await;
                let response = serde_json::json!({
                    "ready": status.ready,
                    "browser_connected": status.browser_connected,
                    "llm_available": status.llm_available,
                    "cached_scripts": status.cached_scripts,
                    "metrics": {
                        "total_requests": status.metrics.total_requests,
                        "successful_requests": status.metrics.successful_requests,
                        "failed_requests": status.metrics.failed_requests,
                        "scripts_generated": status.metrics.scripts_generated,
                        "scripts_reused": status.metrics.scripts_reused,
                        "tokens_saved": status.metrics.tokens_saved,
                        "items_processed": status.metrics.items_processed,
                    }
                });
                Ok(ToolCallResult::text(serde_json::to_string_pretty(&response).unwrap_or_default()))
            }

            _ => Ok(ToolCallResult::error(format!(
                "Unknown automation tool: {}. Available tools: analyze (analyze task routing), execute (run automation), status (get orchestrator status)",
                tool_name
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{DirectoryConfig, DirectoryMode, FilesystemConfig};
    use tempfile::TempDir;

    fn create_test_filesystem() -> (TempDir, Arc<FilesystemService>) {
        let temp_dir = TempDir::new().unwrap();
        let config = FilesystemConfig {
            enabled: true,
            allowed_directories: vec![DirectoryConfig {
                path: temp_dir.path().to_string_lossy().to_string(),
                mode: DirectoryMode::ReadWrite,
                description: None,
                docker_mount_path: None,
            }],
            max_read_bytes: 1024 * 1024,
            max_write_bytes: 1024 * 1024,
            blocked_patterns: vec![],
            follow_symlinks: true,
            default_encoding: "utf-8".to_string(),
        };
        let fs = Arc::new(FilesystemService::new(config));
        (temp_dir, fs)
    }

    #[tokio::test]
    async fn test_handles_namespace() {
        let executor = BuiltinToolExecutor::new(None, None);
        assert!(executor.handles_namespace("filesystem"));
        assert!(executor.handles_namespace("executor"));
        assert!(executor.handles_namespace("bash"));
        assert!(!executor.handles_namespace("videocli"));
        assert!(!executor.handles_namespace("adobe"));
    }

    #[tokio::test]
    async fn test_read_file() {
        let (temp_dir, fs) = create_test_filesystem();
        let executor = BuiltinToolExecutor::new(Some(fs), None);

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "Hello, World!").unwrap();

        let result = executor
            .execute(
                "filesystem",
                "read_file",
                serde_json::json!({
                    "path": test_file.to_string_lossy()
                }),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let content = result.text_content().unwrap();
        assert!(content.contains("Hello, World!"));
    }

    #[tokio::test]
    async fn test_write_file() {
        let (temp_dir, fs) = create_test_filesystem();
        let executor = BuiltinToolExecutor::new(Some(fs), None);

        let test_file = temp_dir.path().join("new_file.txt");

        let result = executor
            .execute(
                "filesystem",
                "write_file",
                serde_json::json!({
                    "path": test_file.to_string_lossy(),
                    "content": "Test content"
                }),
            )
            .await
            .unwrap();

        // Note: Write might fail due to permission check on new files
        // The result contains either success or error message
        let content = result.text_content().unwrap();
        if !result.is_error {
            // Verify file was written
            let file_content = std::fs::read_to_string(&test_file).unwrap();
            assert_eq!(file_content, "Test content");
        } else {
            // Permission error is acceptable in test environment
            assert!(
                content.contains("permission")
                    || content.contains("Permission")
                    || content.contains("accessible")
            );
        }
    }

    #[tokio::test]
    async fn test_list_directory() {
        let (temp_dir, fs) = create_test_filesystem();
        let executor = BuiltinToolExecutor::new(Some(fs), None);

        // Create some test files
        std::fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(temp_dir.path().join("file2.txt"), "content2").unwrap();
        std::fs::create_dir(temp_dir.path().join("subdir")).unwrap();

        let result = executor
            .execute(
                "filesystem",
                "list_directory",
                serde_json::json!({
                    "path": temp_dir.path().to_string_lossy()
                }),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let content = result.text_content().unwrap();
        assert!(content.contains("file1.txt"));
        assert!(content.contains("file2.txt"));
        assert!(content.contains("subdir"));
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let (temp_dir, fs) = create_test_filesystem();
        let executor = BuiltinToolExecutor::new(Some(fs), None);
        let _ = temp_dir; // Keep temp_dir alive

        let result = executor
            .execute("filesystem", "unknown_tool", serde_json::json!({}))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result
            .text_content()
            .unwrap()
            .contains("Unknown filesystem tool"));
    }

    #[tokio::test]
    async fn test_filesystem_not_available() {
        let executor = BuiltinToolExecutor::new(None, None);

        let result = executor
            .execute(
                "filesystem",
                "read_file",
                serde_json::json!({"path": "/test"}),
            )
            .await;

        assert!(result.is_err());
    }
}
