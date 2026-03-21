//! CDP-backed ScreenController implementation.
//!
//! Implements `canal_cv::ScreenController` over Chrome DevTools Protocol,
//! replacing the legacy `browser::CdpClient` + `BrowserRouter` stack with a
//! thin adapter that maps ScreenController methods to CDP commands.

use async_trait::async_trait;
// base64 decoding not needed — CDP returns base64 strings directly
use canal_cv::{
    ComputerUseError, ContextInfo, Modifier, MouseButton, ScreenCapture, ScreenController,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info};

/// Default CDP port for Chrome remote debugging.
pub const DEFAULT_CDP_PORT: u16 = 9222;

/// Default command timeout (30 seconds).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// CDP message ID counter.
static MESSAGE_ID: AtomicU64 = AtomicU64::new(1);

/// Configuration for the CDP screen controller.
#[derive(Debug, Clone)]
pub struct CdpConfig {
    /// Chrome remote debugging host.
    pub host: String,
    /// Chrome remote debugging port.
    pub port: u16,
    /// Default command timeout in milliseconds.
    pub timeout_ms: u64,
    /// Auto-create a new page on connect.
    pub auto_create_page: bool,
}

impl Default for CdpConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: DEFAULT_CDP_PORT,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            auto_create_page: true,
        }
    }
}

/// CDP response from Chrome.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CdpResponse {
    id: u64,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<CdpError>,
}

/// CDP error from Chrome.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CdpError {
    code: i64,
    message: String,
}

/// CDP message (response or event).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum CdpMessage {
    Response(CdpResponse),
    Event(CdpEvent),
}

/// CDP event from Chrome.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CdpEvent {
    method: String,
    #[serde(default)]
    params: Value,
}

/// A ScreenController backed by Chrome DevTools Protocol.
///
/// Connects to a Chrome/Chromium browser over WebSocket and maps
/// `ScreenController` trait methods to CDP domain commands.
pub struct CdpScreenController {
    ws_tx: mpsc::Sender<String>,
    pending: Arc<RwLock<HashMap<u64, oneshot::Sender<CdpResponse>>>>,
    connected: Arc<RwLock<bool>>,
    endpoint_url: String,
    session_id: Arc<RwLock<Option<String>>>,
    timeout_ms: u64,
    /// Cached viewport dimensions (width, height) in CSS pixels.
    display_size: Arc<RwLock<(u32, u32)>>,
    /// Cached device pixel ratio.
    pixel_ratio: Arc<RwLock<f32>>,
}

impl CdpScreenController {
    /// Connect to Chrome via CDP.
    pub async fn connect(config: CdpConfig) -> Result<Self, ComputerUseError> {
        let endpoint_url = format!("ws://{}:{}/devtools/browser", config.host, config.port);
        Self::connect_to_url(&endpoint_url, config.timeout_ms, config.auto_create_page).await
    }

    /// Connect to a specific CDP WebSocket URL.
    pub async fn connect_to_url(
        url: &str,
        timeout_ms: u64,
        auto_create_page: bool,
    ) -> Result<Self, ComputerUseError> {
        info!(url = %url, "Connecting to Chrome CDP...");

        let (ws_stream, _) = connect_async(url).await.map_err(|e| {
            ComputerUseError::CaptureFailed(format!("Failed to connect to CDP: {}", e))
        })?;

        let (mut ws_write, mut ws_read) = ws_stream.split();

        let (ws_tx, mut ws_rx) = mpsc::channel::<String>(100);

        let pending: Arc<RwLock<HashMap<u64, oneshot::Sender<CdpResponse>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let pending_clone = pending.clone();

        let connected = Arc::new(RwLock::new(true));
        let connected_clone = connected.clone();

        // Send messages to WebSocket
        tokio::spawn(async move {
            while let Some(msg) = ws_rx.recv().await {
                if ws_write.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
        });

        // Receive messages from WebSocket
        tokio::spawn(async move {
            while let Some(msg) = ws_read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(cdp_msg) = serde_json::from_str::<CdpMessage>(&text) {
                            match cdp_msg {
                                CdpMessage::Response(response) => {
                                    let mut pending = pending_clone.write().await;
                                    if let Some(tx) = pending.remove(&response.id) {
                                        let _ = tx.send(response);
                                    }
                                }
                                CdpMessage::Event(event) => {
                                    debug!(method = %event.method, "CDP event received");
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        info!("CDP connection closed");
                        *connected_clone.write().await = false;
                        break;
                    }
                    Err(e) => {
                        error!(error = %e, "CDP WebSocket error");
                        *connected_clone.write().await = false;
                        break;
                    }
                    _ => {}
                }
            }
        });

        let controller = Self {
            ws_tx,
            pending,
            connected,
            endpoint_url: url.to_string(),
            session_id: Arc::new(RwLock::new(None)),
            timeout_ms,
            display_size: Arc::new(RwLock::new((1920, 1080))),
            pixel_ratio: Arc::new(RwLock::new(1.0)),
        };

        if auto_create_page {
            controller.create_page(None).await?;
        }

        // Fetch initial viewport metrics
        controller.refresh_display_metrics().await;

        info!("CDP ScreenController connected successfully");
        Ok(controller)
    }

    /// Send a CDP command and wait for response.
    async fn send_command(&self, method: &str, params: Value) -> Result<Value, ComputerUseError> {
        if !*self.connected.read().await {
            return Err(ComputerUseError::NotConnected);
        }

        let id = MESSAGE_ID.fetch_add(1, Ordering::SeqCst);

        let mut command = json!({
            "id": id,
            "method": method,
            "params": params,
        });

        if let Some(session_id) = self.session_id.read().await.clone() {
            command["sessionId"] = json!(session_id);
        }

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.write().await;
            pending.insert(id, tx);
        }

        let command_str = serde_json::to_string(&command).map_err(|e| {
            ComputerUseError::Other(anyhow::anyhow!("Failed to serialize CDP command: {}", e))
        })?;

        debug!(method = %method, id = id, "Sending CDP command");

        self.ws_tx
            .send(command_str)
            .await
            .map_err(|_| ComputerUseError::CaptureFailed("CDP connection closed".to_string()))?;

        let response = match timeout(Duration::from_millis(self.timeout_ms), rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                return Err(ComputerUseError::CaptureFailed(
                    "CDP response channel closed".to_string(),
                ))
            }
            Err(_) => {
                return Err(ComputerUseError::Timeout(Duration::from_millis(
                    self.timeout_ms,
                )))
            }
        };

        if let Some(err) = response.error {
            Err(ComputerUseError::Other(anyhow::anyhow!(
                "CDP error: {}",
                err.message
            )))
        } else {
            Ok(response.result.unwrap_or(json!({})))
        }
    }

    /// Create a new page/tab.
    async fn create_page(&self, url: Option<&str>) -> Result<String, ComputerUseError> {
        let url = url.unwrap_or("about:blank");

        let result = self
            .send_command("Target.createTarget", json!({ "url": url }))
            .await?;

        let target_id = result["targetId"]
            .as_str()
            .ok_or_else(|| ComputerUseError::Other(anyhow::anyhow!("No targetId in response")))?
            .to_string();

        info!(target_id = %target_id, "Created new page");

        let attach_result = self
            .send_command(
                "Target.attachToTarget",
                json!({
                    "targetId": target_id,
                    "flatten": true,
                }),
            )
            .await?;

        if let Some(session_id) = attach_result["sessionId"].as_str() {
            info!(session_id = %session_id, "Attached to target");
            *self.session_id.write().await = Some(session_id.to_string());

            self.send_command("Page.enable", json!({})).await?;
            self.send_command("Runtime.enable", json!({})).await?;
            self.send_command("DOM.enable", json!({})).await?;
        }

        Ok(target_id)
    }

    /// Refresh cached display metrics from the browser.
    async fn refresh_display_metrics(&self) {
        if let Ok(metrics) = self.send_command("Page.getLayoutMetrics", json!({})).await {
            if let Some(visual_viewport) = metrics.get("visualViewport") {
                let width = visual_viewport["clientWidth"].as_f64().unwrap_or(1920.0) as u32;
                let height = visual_viewport["clientHeight"].as_f64().unwrap_or(1080.0) as u32;
                let scale = visual_viewport["scale"].as_f64().unwrap_or(1.0) as f32;

                *self.display_size.write().await = (width, height);
                *self.pixel_ratio.write().await = scale;
            }
        }
    }

    /// Navigate to a URL.
    ///
    /// This is not part of the `ScreenController` trait (which is screen-agnostic),
    /// but is essential for browser-based screen control.
    pub async fn navigate(&self, url: &str) -> Result<(), ComputerUseError> {
        info!(url = %url, "Navigating to URL");
        self.send_command("Page.navigate", json!({ "url": url }))
            .await?;
        // Wait for page to settle
        tokio::time::sleep(Duration::from_millis(500)).await;
        // Refresh metrics after navigation
        self.refresh_display_metrics().await;
        Ok(())
    }

    /// Evaluate JavaScript in the page context.
    pub async fn evaluate(&self, expression: &str) -> Result<Value, ComputerUseError> {
        let result = self
            .send_command(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                }),
            )
            .await?;

        if let Some(exception) = result.get("exceptionDetails") {
            return Err(ComputerUseError::Other(anyhow::anyhow!(
                "JS error: {}",
                exception["text"].as_str().unwrap_or("unknown")
            )));
        }

        Ok(result.get("result").cloned().unwrap_or(json!({})))
    }

    /// Check if connected to Chrome.
    pub async fn is_connected(&self) -> bool {
        *self.connected.read().await
    }

    /// Get the CDP endpoint URL.
    pub fn endpoint_url(&self) -> &str {
        &self.endpoint_url
    }

    /// Map a CDP key name from common names.
    fn map_key_name(key: &str) -> &str {
        match key.to_lowercase().as_str() {
            "enter" | "return" => "Enter",
            "tab" => "Tab",
            "escape" | "esc" => "Escape",
            "backspace" => "Backspace",
            "delete" => "Delete",
            "arrowup" | "up" => "ArrowUp",
            "arrowdown" | "down" => "ArrowDown",
            "arrowleft" | "left" => "ArrowLeft",
            "arrowright" | "right" => "ArrowRight",
            "space" | " " => " ",
            "home" => "Home",
            "end" => "End",
            "pageup" => "PageUp",
            "pagedown" => "PageDown",
            _ => key,
        }
    }

    /// Build modifier flags for CDP key events.
    fn modifier_flags(modifiers: &[Modifier]) -> i32 {
        let mut flags = 0;
        for m in modifiers {
            match m {
                Modifier::Alt => flags |= 1,
                Modifier::Control => flags |= 2,
                Modifier::Meta => flags |= 4,
                Modifier::Shift => flags |= 8,
            }
        }
        flags
    }
}

#[async_trait]
impl ScreenController for CdpScreenController {
    async fn capture(&self) -> Result<ScreenCapture, ComputerUseError> {
        // Refresh metrics
        self.refresh_display_metrics().await;

        let (display_width, display_height) = *self.display_size.read().await;
        let pixel_ratio = *self.pixel_ratio.read().await;

        // Capture as JPEG for consistency with ScreenCapture expectations
        let result = self
            .send_command(
                "Page.captureScreenshot",
                json!({ "format": "jpeg", "quality": 70 }),
            )
            .await?;

        let base64_data = result["data"]
            .as_str()
            .ok_or_else(|| {
                ComputerUseError::CaptureFailed("No screenshot data in CDP response".to_string())
            })?
            .to_string();

        // Physical dimensions = display × pixel_ratio
        let physical_width = (display_width as f32 * pixel_ratio) as u32;
        let physical_height = (display_height as f32 * pixel_ratio) as u32;

        Ok(ScreenCapture {
            base64: base64_data,
            physical_width,
            physical_height,
            display_width,
            display_height,
            pixel_ratio,
            timestamp: Instant::now(),
        })
    }

    async fn click(&self, x: u32, y: u32, button: MouseButton) -> Result<(), ComputerUseError> {
        let btn = match button {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
        };

        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mousePressed",
                "x": x,
                "y": y,
                "button": btn,
                "clickCount": 1,
            }),
        )
        .await?;

        tokio::time::sleep(Duration::from_millis(50)).await;

        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseReleased",
                "x": x,
                "y": y,
                "button": btn,
                "clickCount": 1,
            }),
        )
        .await?;

        Ok(())
    }

    async fn type_text(&self, text: &str) -> Result<(), ComputerUseError> {
        for ch in text.chars() {
            self.send_command(
                "Input.dispatchKeyEvent",
                json!({
                    "type": "keyDown",
                    "text": ch.to_string(),
                }),
            )
            .await?;

            self.send_command(
                "Input.dispatchKeyEvent",
                json!({
                    "type": "keyUp",
                    "text": ch.to_string(),
                }),
            )
            .await?;
        }
        Ok(())
    }

    async fn key_press(&self, key: &str, modifiers: &[Modifier]) -> Result<(), ComputerUseError> {
        let mapped_key = Self::map_key_name(key);
        let flags = Self::modifier_flags(modifiers);

        self.send_command(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyDown",
                "key": mapped_key,
                "modifiers": flags,
            }),
        )
        .await?;

        self.send_command(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": mapped_key,
                "modifiers": flags,
            }),
        )
        .await?;

        Ok(())
    }

    async fn scroll(&self, delta_x: f64, delta_y: f64) -> Result<(), ComputerUseError> {
        let (w, h) = *self.display_size.read().await;
        // Scroll at viewport center
        let center_x = w / 2;
        let center_y = h / 2;

        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseWheel",
                "x": center_x,
                "y": center_y,
                "deltaX": delta_x,
                "deltaY": delta_y,
            }),
        )
        .await?;

        Ok(())
    }

    async fn drag(
        &self,
        from_x: u32,
        from_y: u32,
        to_x: u32,
        to_y: u32,
    ) -> Result<(), ComputerUseError> {
        // Press at start
        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mousePressed",
                "x": from_x,
                "y": from_y,
                "button": "left",
                "clickCount": 1,
            }),
        )
        .await?;

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Move to destination
        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved",
                "x": to_x,
                "y": to_y,
            }),
        )
        .await?;

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Release at destination
        self.send_command(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseReleased",
                "x": to_x,
                "y": to_y,
                "button": "left",
                "clickCount": 1,
            }),
        )
        .await?;

        Ok(())
    }

    fn display_size(&self) -> (u32, u32) {
        // Use blocking read — this is called from sync context
        // The value is cached and rarely changes
        // For async contexts, use refresh_display_metrics() first
        // tokio RwLock try_read is not available, so we return the cached default
        // This is safe because refresh_display_metrics() is called on connect and navigate
        if let Ok(size) = self.display_size.try_read() {
            *size
        } else {
            (1920, 1080) // Fallback
        }
    }

    fn context_info(&self) -> Option<ContextInfo> {
        // Context info requires async evaluation — return None for sync access.
        // Use get_context_info() for the async version.
        None
    }
}

impl CdpScreenController {
    /// Get context info asynchronously (title + URL from page).
    pub async fn get_context_info(&self) -> Option<ContextInfo> {
        let title = self
            .evaluate("document.title")
            .await
            .ok()
            .and_then(|v| v["value"].as_str().map(|s| s.to_string()));

        let url = self
            .evaluate("window.location.href")
            .await
            .ok()
            .and_then(|v| v["value"].as_str().map(|s| s.to_string()));

        Some(ContextInfo {
            url,
            title,
            app_name: Some("Chrome".to_string()),
            interactive_elements: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cdp_config_default() {
        let config = CdpConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9222);
        assert_eq!(config.timeout_ms, 30_000);
        assert!(config.auto_create_page);
    }

    #[test]
    fn test_key_mapping() {
        assert_eq!(CdpScreenController::map_key_name("enter"), "Enter");
        assert_eq!(CdpScreenController::map_key_name("Enter"), "Enter");
        assert_eq!(CdpScreenController::map_key_name("ESCAPE"), "Escape");
        assert_eq!(CdpScreenController::map_key_name("tab"), "Tab");
        assert_eq!(CdpScreenController::map_key_name("up"), "ArrowUp");
        assert_eq!(CdpScreenController::map_key_name("a"), "a");
    }

    #[test]
    fn test_modifier_flags() {
        assert_eq!(CdpScreenController::modifier_flags(&[]), 0);
        assert_eq!(CdpScreenController::modifier_flags(&[Modifier::Control]), 2);
        assert_eq!(CdpScreenController::modifier_flags(&[Modifier::Shift]), 8);
        assert_eq!(
            CdpScreenController::modifier_flags(&[Modifier::Control, Modifier::Shift]),
            10
        );
        assert_eq!(
            CdpScreenController::modifier_flags(&[Modifier::Alt, Modifier::Meta]),
            5
        );
    }
}
