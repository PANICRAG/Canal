//! RTE (Remote Tool Execution) protocol types.
//!
//! Defines the SSE events and request/response types used between
//! the gateway backend and native clients (Windows, macOS).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Client capabilities sent in the initial StreamChatRequest.
///
/// Tells the backend which tools the client can execute locally,
/// enabling the RTE delegation path instead of cloud execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Protocol version (e.g., "1.0")
    pub protocol_version: String,
    /// Tools the client can execute locally
    pub supported_tools: Vec<String>,
    /// Client platform identifier
    pub platform: String,
    /// Whether RTE is enabled on this client
    pub rte_enabled: bool,
    /// Max concurrent tool executions the client supports
    pub max_concurrent_tools: u32,
}

impl ClientCapabilities {
    /// Check if the client supports a specific tool
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        self.rte_enabled && self.supported_tools.iter().any(|t| t == tool_name)
    }

    /// Check if client has RTE capability at all
    pub fn is_rte_capable(&self) -> bool {
        self.rte_enabled && !self.supported_tools.is_empty()
    }
}

/// Tool execution request sent from server to client via SSE.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecuteRequest {
    /// Unique request identifier
    pub request_id: Uuid,
    /// Name of the tool to execute
    pub tool_name: String,
    /// Tool input parameters
    pub tool_input: serde_json::Value,
    /// Timeout in milliseconds before fallback triggers
    pub timeout_ms: u64,
    /// Fallback strategy if client fails or times out
    pub fallback: FallbackStrategy,
    /// HMAC-SHA256 signature for request integrity
    pub hmac_signature: String,
}

/// Tool execution result sent from client to server via POST.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecuteResult {
    /// Request ID this result corresponds to
    pub request_id: Uuid,
    /// Tool execution output
    pub result: serde_json::Value,
    /// Whether execution succeeded
    pub success: bool,
    /// Error message if execution failed
    pub error: Option<String>,
    /// Actual execution time in milliseconds
    pub execution_time_ms: u64,
    /// HMAC-SHA256 signature for result integrity
    pub hmac_signature: String,
}

/// Fallback strategy when client cannot execute a tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FallbackStrategy {
    /// Execute the tool on the cloud backend
    CloudExecution,
    /// Return an error to the agent
    Error,
    /// Retry the request to the client
    Retry { max_attempts: u32 },
    /// Skip the tool and continue agent loop
    Skip,
}

impl Default for FallbackStrategy {
    fn default() -> Self {
        Self::CloudExecution
    }
}

/// SSE event types for the RTE protocol.
///
/// These events are sent from the server to the client during
/// a streaming chat session with RTE enabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum RteSseEvent {
    /// Session established — contains the HMAC session secret
    #[serde(rename = "session_start")]
    SessionStart {
        session_id: Uuid,
        /// Base64-encoded session secret for HMAC signing
        session_secret: String,
    },

    /// Request the client to execute a tool locally
    #[serde(rename = "tool_execute_request")]
    ToolExecuteRequest(ToolExecuteRequest),

    /// Notify client that auth token needs refresh
    #[serde(rename = "auth_refresh_required")]
    AuthRefreshRequired {
        /// When the current token expires
        expires_at: String,
        /// URL to refresh the token
        refresh_url: String,
    },

    /// Resume notification after client reconnect — lists pending requests
    #[serde(rename = "resume")]
    Resume {
        /// Pending tool execution request IDs
        pending_request_ids: Vec<Uuid>,
    },
}

/// Per-tool fallback configuration.
///
/// Maps tool names to their fallback strategies and timeouts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFallbackConfig {
    /// Default timeout in milliseconds for tool execution
    pub default_timeout_ms: u64,
    /// Default fallback strategy
    pub default_fallback: FallbackStrategy,
    /// Per-tool overrides
    #[serde(default)]
    pub overrides: std::collections::HashMap<String, ToolOverride>,
}

/// Per-tool timeout and fallback override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOverride {
    /// Custom timeout for this tool (overrides default)
    pub timeout_ms: Option<u64>,
    /// Custom fallback strategy for this tool
    pub fallback: Option<FallbackStrategy>,
}

impl Default for ToolFallbackConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 30_000, // 30 seconds
            default_fallback: FallbackStrategy::CloudExecution,
            overrides: std::collections::HashMap::new(),
        }
    }
}

impl ToolFallbackConfig {
    /// Get the timeout for a specific tool
    pub fn timeout_for(&self, tool_name: &str) -> u64 {
        self.overrides
            .get(tool_name)
            .and_then(|o| o.timeout_ms)
            .unwrap_or(self.default_timeout_ms)
    }

    /// Get the fallback strategy for a specific tool
    pub fn fallback_for(&self, tool_name: &str) -> FallbackStrategy {
        self.overrides
            .get(tool_name)
            .and_then(|o| o.fallback.clone())
            .unwrap_or_else(|| self.default_fallback.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_capabilities_supports_tool() {
        let caps = ClientCapabilities {
            protocol_version: "1.0".to_string(),
            supported_tools: vec!["code_execute".to_string(), "file_read".to_string()],
            platform: "windows".to_string(),
            rte_enabled: true,
            max_concurrent_tools: 3,
        };

        assert!(caps.supports_tool("code_execute"));
        assert!(caps.supports_tool("file_read"));
        assert!(!caps.supports_tool("browser_screenshot"));
        assert!(caps.is_rte_capable());
    }

    #[test]
    fn test_client_capabilities_rte_disabled() {
        let caps = ClientCapabilities {
            protocol_version: "1.0".to_string(),
            supported_tools: vec!["code_execute".to_string()],
            platform: "web".to_string(),
            rte_enabled: false,
            max_concurrent_tools: 0,
        };

        assert!(!caps.supports_tool("code_execute"));
        assert!(!caps.is_rte_capable());
    }

    #[test]
    fn test_fallback_config_defaults() {
        let config = ToolFallbackConfig::default();
        assert_eq!(config.timeout_for("any_tool"), 30_000);
        assert_eq!(
            config.fallback_for("any_tool"),
            FallbackStrategy::CloudExecution
        );
    }

    #[test]
    fn test_fallback_config_overrides() {
        let mut config = ToolFallbackConfig::default();
        config.overrides.insert(
            "code_execute".to_string(),
            ToolOverride {
                timeout_ms: Some(60_000),
                fallback: Some(FallbackStrategy::Error),
            },
        );

        assert_eq!(config.timeout_for("code_execute"), 60_000);
        assert_eq!(config.fallback_for("code_execute"), FallbackStrategy::Error);
        assert_eq!(config.timeout_for("file_read"), 30_000);
    }
}
