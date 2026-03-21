//! Browser MCP Client
//!
//! This module provides an MCP client specifically designed to communicate with
//! the browser extension's MCP server. It supports SSE (Server-Sent Events)
//! transport for real-time communication with the Chrome extension.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────┐         SSE/HTTP          ┌──────────────────────┐
//! │   BrowserMcpClient  │ ◄───────────────────────► │ Chrome Extension     │
//! │   (Rust)            │                           │ MCP Server (TS)      │
//! └─────────────────────┘                           └──────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use gateway_core::mcp::BrowserMcpClient;
//!
//! let client = BrowserMcpClient::connect("http://localhost:3456").await?;
//!
//! // Navigate to a URL
//! client.navigate("https://example.com", None).await?;
//!
//! // Click an element
//! client.click("#submit-btn", None).await?;
//!
//! // Take a screenshot
//! let screenshot = client.screenshot(false, None).await?;
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::error::{Error, Result};

// ============================================================================
// Types
// ============================================================================

/// Browser MCP client configuration
#[derive(Debug, Clone)]
pub struct BrowserMcpConfig {
    /// MCP server URL (e.g., "http://localhost:3456")
    pub url: String,
    /// Connection timeout in seconds
    pub connect_timeout_secs: u64,
    /// Request timeout in seconds
    pub request_timeout_secs: u64,
    /// Enable debug logging
    pub debug: bool,
}

impl Default for BrowserMcpConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:3456".to_string(),
            connect_timeout_secs: 10,
            request_timeout_secs: 60,
            debug: false,
        }
    }
}

/// Tab information from the browser
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TabInfo {
    pub id: i32,
    pub url: String,
    pub title: String,
    pub active: bool,
    #[serde(rename = "windowId")]
    pub window_id: i32,
    pub index: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(rename = "favIconUrl", skip_serializing_if = "Option::is_none")]
    pub fav_icon_url: Option<String>,
}

/// Screenshot result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScreenshotResult {
    /// Base64-encoded image data
    pub data: String,
    /// MIME type (e.g., "image/png")
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

/// MCP tool call result content
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum McpContentItem {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// MCP tool call result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpToolCallResult {
    pub content: Vec<McpContentItem>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

impl McpToolCallResult {
    /// Get text content from result
    pub fn text_content(&self) -> Option<String> {
        for item in &self.content {
            if let McpContentItem::Text { text } = item {
                return Some(text.clone());
            }
        }
        None
    }

    /// Get image content from result
    pub fn image_content(&self) -> Option<(String, String)> {
        for item in &self.content {
            if let McpContentItem::Image { data, mime_type } = item {
                return Some((data.clone(), mime_type.clone()));
            }
        }
        None
    }
}

/// JSON-RPC request
#[derive(Debug, serde::Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// JSON-RPC response
#[derive(Debug, serde::Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: u64,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, serde::Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i32,
    message: String,
}

// ============================================================================
// Browser MCP Client
// ============================================================================

/// MCP client for browser extension communication
///
/// This client connects to the browser extension's MCP server and provides
/// methods for browser automation through the MCP protocol.
pub struct BrowserMcpClient {
    config: BrowserMcpConfig,
    http_client: reqwest::Client,
    request_id: AtomicU64,
    connected: Arc<RwLock<bool>>,
}

impl BrowserMcpClient {
    /// Create a new browser MCP client with default configuration
    pub fn new() -> Self {
        Self::with_config(BrowserMcpConfig::default())
    }

    /// Create a new browser MCP client with custom configuration
    pub fn with_config(config: BrowserMcpConfig) -> Self {
        // R3-H15: Use unwrap_or_default instead of expect to avoid panic on client build failure
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
            .build()
            .unwrap_or_default();

        Self {
            config,
            http_client,
            request_id: AtomicU64::new(1),
            connected: Arc::new(RwLock::new(false)),
        }
    }

    /// Connect to the browser extension's MCP server
    pub async fn connect(url: &str) -> Result<Self> {
        let config = BrowserMcpConfig {
            url: url.to_string(),
            ..Default::default()
        };
        let mut client = Self::with_config(config);
        client.initialize().await?;
        Ok(client)
    }

    /// Initialize the MCP connection
    pub async fn initialize(&mut self) -> Result<()> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "roots": { "listChanged": true }
            },
            "clientInfo": {
                "name": "canal",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let response = self.send_request("initialize", Some(params)).await?;

        if response.error.is_some() {
            return Err(Error::Internal("MCP initialization failed".to_string()));
        }

        // Send initialized notification
        let _ = self.send_request("notifications/initialized", None).await;

        *self.connected.write().await = true;

        if self.config.debug {
            tracing::info!(url = %self.config.url, "Connected to browser MCP server");
        }

        Ok(())
    }

    /// Check if connected to the MCP server
    pub async fn is_connected(&self) -> bool {
        *self.connected.read().await
    }

    /// Get the server URL
    pub fn url(&self) -> &str {
        &self.config.url
    }

    // =========================================================================
    // Navigation Tools
    // =========================================================================

    /// Navigate to a URL
    pub async fn navigate(&self, url: &str, tab_id: Option<i32>) -> Result<()> {
        let mut args = serde_json::json!({ "url": url });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        self.call_tool("browser_navigate", args).await?;
        Ok(())
    }

    // =========================================================================
    // Interaction Tools
    // =========================================================================

    /// Click an element
    pub async fn click(&self, selector: &str, tab_id: Option<i32>) -> Result<()> {
        let mut args = serde_json::json!({ "selector": selector });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        self.call_tool("browser_click", args).await?;
        Ok(())
    }

    /// Fill text into an input field
    pub async fn fill(&self, selector: &str, text: &str, tab_id: Option<i32>) -> Result<()> {
        let mut args = serde_json::json!({
            "selector": selector,
            "text": text
        });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        self.call_tool("browser_fill", args).await?;
        Ok(())
    }

    /// Type text character by character
    pub async fn type_text(
        &self,
        selector: &str,
        text: &str,
        delay_ms: Option<u32>,
        tab_id: Option<i32>,
    ) -> Result<()> {
        let mut args = serde_json::json!({
            "selector": selector,
            "text": text
        });
        if let Some(delay) = delay_ms {
            args["delay"] = serde_json::Value::Number(delay.into());
        }
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        self.call_tool("browser_type", args).await?;
        Ok(())
    }

    /// Press a keyboard key
    pub async fn press(&self, key: &str, tab_id: Option<i32>) -> Result<()> {
        let mut args = serde_json::json!({ "key": key });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        self.call_tool("browser_press", args).await?;
        Ok(())
    }

    /// Scroll the page
    pub async fn scroll(
        &self,
        direction: &str,
        amount: Option<u32>,
        selector: Option<&str>,
        tab_id: Option<i32>,
    ) -> Result<()> {
        let mut args = serde_json::json!({ "direction": direction });
        if let Some(amt) = amount {
            args["amount"] = serde_json::Value::Number(amt.into());
        }
        if let Some(sel) = selector {
            args["selector"] = serde_json::Value::String(sel.to_string());
        }
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        self.call_tool("browser_scroll", args).await?;
        Ok(())
    }

    /// Hover over an element
    pub async fn hover(&self, selector: &str, tab_id: Option<i32>) -> Result<()> {
        let mut args = serde_json::json!({ "selector": selector });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        self.call_tool("browser_hover", args).await?;
        Ok(())
    }

    /// Select an option from a dropdown
    pub async fn select(&self, selector: &str, value: &str, tab_id: Option<i32>) -> Result<()> {
        let mut args = serde_json::json!({
            "selector": selector,
            "value": value
        });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        self.call_tool("browser_select", args).await?;
        Ok(())
    }

    // =========================================================================
    // Content Tools
    // =========================================================================

    /// Take a screenshot
    pub async fn screenshot(
        &self,
        full_page: bool,
        tab_id: Option<i32>,
    ) -> Result<ScreenshotResult> {
        let mut args = serde_json::json!({ "fullPage": full_page });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        let result = self.call_tool("browser_screenshot", args).await?;

        if let Some((data, mime_type)) = result.image_content() {
            return Ok(ScreenshotResult { data, mime_type });
        }

        Err(Error::Internal(
            "Screenshot failed - no image data".to_string(),
        ))
    }

    /// Get accessibility tree snapshot
    pub async fn snapshot(
        &self,
        interactive_only: bool,
        compact: bool,
        tab_id: Option<i32>,
    ) -> Result<serde_json::Value> {
        let mut args = serde_json::json!({
            "interactiveOnly": interactive_only,
            "compact": compact
        });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        let result = self.call_tool("browser_snapshot", args).await?;

        if let Some(text) = result.text_content() {
            return serde_json::from_str(&text)
                .map_err(|e| Error::Internal(format!("Failed to parse snapshot: {}", e)));
        }

        Err(Error::Internal("Snapshot failed".to_string()))
    }

    /// Get text content from page or element
    pub async fn get_text(&self, selector: Option<&str>, tab_id: Option<i32>) -> Result<String> {
        let mut args = serde_json::json!({});
        if let Some(sel) = selector {
            args["selector"] = serde_json::Value::String(sel.to_string());
        }
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        let result = self.call_tool("browser_get_text", args).await?;

        result
            .text_content()
            .ok_or_else(|| Error::Internal("Failed to get text content".to_string()))
    }

    /// Get HTML content from element
    pub async fn get_html(
        &self,
        selector: &str,
        outer: bool,
        tab_id: Option<i32>,
    ) -> Result<String> {
        let mut args = serde_json::json!({
            "selector": selector,
            "outer": outer
        });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        let result = self.call_tool("browser_get_html", args).await?;

        result
            .text_content()
            .ok_or_else(|| Error::Internal("Failed to get HTML content".to_string()))
    }

    // =========================================================================
    // Element State Tools
    // =========================================================================

    /// Check if element is visible
    pub async fn is_visible(&self, selector: &str, tab_id: Option<i32>) -> Result<bool> {
        let mut args = serde_json::json!({ "selector": selector });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        let result = self.call_tool("browser_is_visible", args).await?;

        // R3-H2: Use exact comparison instead of .contains("true") which matches
        // any string containing "true" (e.g. "attribute_true_value")
        if let Some(text) = result.text_content() {
            return Ok(text.trim().eq_ignore_ascii_case("true"));
        }

        Ok(false)
    }

    /// Check if element exists
    pub async fn exists(&self, selector: &str, tab_id: Option<i32>) -> Result<bool> {
        let mut args = serde_json::json!({ "selector": selector });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        let result = self.call_tool("browser_exists", args).await?;

        if let Some(text) = result.text_content() {
            return Ok(text.trim().eq_ignore_ascii_case("true"));
        }

        Ok(false)
    }

    // =========================================================================
    // Wait Tools
    // =========================================================================

    /// Wait for element to appear
    pub async fn wait_for_selector(
        &self,
        selector: &str,
        timeout_ms: Option<u32>,
        tab_id: Option<i32>,
    ) -> Result<bool> {
        let mut args = serde_json::json!({ "selector": selector });
        if let Some(timeout) = timeout_ms {
            args["timeout"] = serde_json::Value::Number(timeout.into());
        }
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        let result = self.call_tool("browser_wait_for_selector", args).await?;

        if let Some(text) = result.text_content() {
            return Ok(text.contains("found"));
        }

        Ok(false)
    }

    // =========================================================================
    // Tab Management Tools
    // =========================================================================

    /// Get list of all tabs
    pub async fn get_tabs(&self) -> Result<Vec<TabInfo>> {
        let result = self
            .call_tool("browser_get_tabs", serde_json::json!({}))
            .await?;

        if let Some(text) = result.text_content() {
            return serde_json::from_str(&text)
                .map_err(|e| Error::Internal(format!("Failed to parse tabs: {}", e)));
        }

        Ok(vec![])
    }

    /// Open a new tab
    pub async fn new_tab(&self, url: Option<&str>) -> Result<TabInfo> {
        let mut args = serde_json::json!({});
        if let Some(u) = url {
            args["url"] = serde_json::Value::String(u.to_string());
        }

        let result = self.call_tool("browser_new_tab", args).await?;

        if let Some(text) = result.text_content() {
            return serde_json::from_str(&text)
                .map_err(|e| Error::Internal(format!("Failed to parse new tab info: {}", e)));
        }

        Err(Error::Internal("Failed to create new tab".to_string()))
    }

    /// Switch to a tab
    pub async fn switch_tab(&self, tab_id: i32) -> Result<()> {
        self.call_tool("browser_switch_tab", serde_json::json!({ "tabId": tab_id }))
            .await?;
        Ok(())
    }

    /// Close a tab
    pub async fn close_tab(&self, tab_id: i32) -> Result<()> {
        self.call_tool("browser_close_tab", serde_json::json!({ "tabId": tab_id }))
            .await?;
        Ok(())
    }

    // =========================================================================
    // Cookie Tools
    // =========================================================================

    /// Get cookies for a URL
    pub async fn get_cookies(
        &self,
        url: Option<&str>,
        name: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut args = serde_json::json!({});
        if let Some(u) = url {
            args["url"] = serde_json::Value::String(u.to_string());
        }
        if let Some(n) = name {
            args["name"] = serde_json::Value::String(n.to_string());
        }

        let result = self.call_tool("browser_get_cookies", args).await?;

        if let Some(text) = result.text_content() {
            return serde_json::from_str(&text)
                .map_err(|e| Error::Internal(format!("Failed to parse cookies: {}", e)));
        }

        Ok(serde_json::json!([]))
    }

    // =========================================================================
    // JavaScript Execution
    // =========================================================================

    /// Execute JavaScript in the page
    pub async fn evaluate(&self, script: &str, tab_id: Option<i32>) -> Result<serde_json::Value> {
        let mut args = serde_json::json!({ "script": script });
        if let Some(id) = tab_id {
            args["tabId"] = serde_json::Value::Number(id.into());
        }

        let result = self.call_tool("browser_evaluate", args).await?;

        if let Some(text) = result.text_content() {
            return serde_json::from_str(&text)
                .map_err(|e| Error::Internal(format!("Failed to parse evaluate result: {}", e)));
        }

        Ok(serde_json::Value::Null)
    }

    // =========================================================================
    // Resource Reading
    // =========================================================================

    /// Read a resource from the MCP server
    pub async fn read_resource(&self, uri: &str) -> Result<serde_json::Value> {
        let params = serde_json::json!({ "uri": uri });
        let response = self.send_request("resources/read", Some(params)).await?;

        if let Some(error) = response.error {
            return Err(Error::Internal(format!(
                "Resource read failed: {}",
                error.message
            )));
        }

        if let Some(result) = response.result {
            if let Some(contents) = result.get("contents") {
                return Ok(contents.clone());
            }
        }

        Err(Error::Internal("No resource content returned".to_string()))
    }

    /// Get DOM accessibility tree for a tab
    pub async fn get_dom(&self, tab_id: i32) -> Result<serde_json::Value> {
        self.read_resource(&format!("browser://dom/{}", tab_id))
            .await
    }

    // =========================================================================
    // Internal Methods
    // =========================================================================

    /// Call an MCP tool
    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolCallResult> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments
        });

        let response = self.send_request("tools/call", Some(params)).await?;

        if let Some(error) = response.error {
            return Err(Error::Internal(format!(
                "Tool call failed: {}",
                error.message
            )));
        }

        if let Some(result) = response.result {
            return serde_json::from_value(result)
                .map_err(|e| Error::Internal(format!("Failed to parse tool result: {}", e)));
        }

        Err(Error::Internal("No result from tool call".to_string()))
    }

    /// Send a JSON-RPC request to the MCP server
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        if self.config.debug {
            tracing::debug!(method = %method, id = %id, "Sending MCP request");
        }

        let response = self
            .http_client
            .post(&self.config.url)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("MCP request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Internal(format!(
                "MCP server returned HTTP {}: {}",
                status, body
            )));
        }

        let json_response: JsonRpcResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse MCP response: {}", e)))?;

        if self.config.debug {
            tracing::debug!(method = %method, id = %id, "Received MCP response");
        }

        Ok(json_response)
    }
}

impl Default for BrowserMcpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = BrowserMcpConfig::default();
        assert_eq!(config.url, "http://localhost:3456");
        assert_eq!(config.connect_timeout_secs, 10);
        assert_eq!(config.request_timeout_secs, 60);
    }

    #[test]
    fn test_tool_result_text() {
        let result = McpToolCallResult {
            content: vec![McpContentItem::Text {
                text: "Hello".to_string(),
            }],
            is_error: false,
        };
        assert_eq!(result.text_content(), Some("Hello".to_string()));
        assert!(result.image_content().is_none());
    }

    #[test]
    fn test_tool_result_image() {
        let result = McpToolCallResult {
            content: vec![McpContentItem::Image {
                data: "base64data".to_string(),
                mime_type: "image/png".to_string(),
            }],
            is_error: false,
        };
        assert!(result.text_content().is_none());
        assert_eq!(
            result.image_content(),
            Some(("base64data".to_string(), "image/png".to_string()))
        );
    }
}
