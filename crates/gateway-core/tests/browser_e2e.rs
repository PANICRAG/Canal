// Browser module removed (CV8: replaced by canal-cv)
#![cfg(feature = "browser-legacy-tests")]

//! End-to-End tests for the Browser Control module
//!
//! These tests verify the integration of browser components including:
//! - LocalBrowserClient command routing
//! - ExtensionManager connection management
//! - Permission checking system
//! - Mock WebSocket server communication
//!
//! Run with: `cargo test --package gateway-core --test browser_e2e`

use gateway_core::browser::{
    BrowserAction, BrowserCommand, BrowserEvent, BrowserMessage, BrowserPermission,
    BrowserPermissionChecker, BrowserResponse, BrowserRouter, CommandType, ConnectionMode,
    ConnectionStatus, DomainRules, EventType, ExtensionAuthRequest, ExtensionManager,
    ExtensionManagerBuilder, ExtensionState, LocalBrowserClient, LocalBrowserClientBuilder,
    NoOpRouter, PermissionDecision,
};
use gateway_core::error::Result;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::time::Duration;

// ============================================================================
// Mock Components for Testing
// ============================================================================

/// Mock browser router for testing command routing without real WebSocket
pub struct MockBrowserRouter {
    /// Whether the router reports as connected
    connected: AtomicBool,
    /// Connection mode to report
    mode: ConnectionMode,
    /// Counter for commands received
    command_count: AtomicU64,
    /// Pending commands for response simulation
    pending_responses: Arc<RwLock<HashMap<String, BrowserResponse>>>,
    /// Whether to fail all commands
    fail_all: AtomicBool,
    /// Custom response handler
    response_handler:
        Arc<RwLock<Option<Box<dyn Fn(&BrowserCommand) -> BrowserResponse + Send + Sync>>>>,
    /// Event handlers for testing
    event_handlers: Arc<RwLock<Vec<Arc<dyn Fn(BrowserEvent) + Send + Sync>>>>,
}

impl MockBrowserRouter {
    /// Create a new mock router
    pub fn new() -> Self {
        Self {
            connected: AtomicBool::new(false),
            mode: ConnectionMode::WebSocket,
            command_count: AtomicU64::new(0),
            pending_responses: Arc::new(RwLock::new(HashMap::new())),
            fail_all: AtomicBool::new(false),
            response_handler: Arc::new(RwLock::new(None)),
            event_handlers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create a connected mock router
    pub fn connected() -> Self {
        let router = Self::new();
        router.connected.store(true, Ordering::SeqCst);
        router
    }

    /// Set connection state
    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::SeqCst);
    }

    /// Set fail all mode
    pub fn set_fail_all(&self, fail: bool) {
        self.fail_all.store(fail, Ordering::SeqCst);
    }

    /// Get command count
    pub fn command_count(&self) -> u64 {
        self.command_count.load(Ordering::SeqCst)
    }

    /// Set a pending response for a specific command ID
    pub async fn set_response(&self, command_id: impl Into<String>, response: BrowserResponse) {
        self.pending_responses
            .write()
            .await
            .insert(command_id.into(), response);
    }

    /// Set a custom response handler
    pub async fn set_response_handler<F>(&self, handler: F)
    where
        F: Fn(&BrowserCommand) -> BrowserResponse + Send + Sync + 'static,
    {
        *self.response_handler.write().await = Some(Box::new(handler));
    }

    /// Register an event handler
    pub async fn on_event<F>(&self, handler: F)
    where
        F: Fn(BrowserEvent) + Send + Sync + 'static,
    {
        self.event_handlers.write().await.push(Arc::new(handler));
    }

    /// Trigger an event to all handlers
    pub async fn trigger_event(&self, event: BrowserEvent) {
        let handlers = self.event_handlers.read().await;
        for handler in handlers.iter() {
            handler(event.clone());
        }
    }
}

impl Default for MockBrowserRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BrowserRouter for MockBrowserRouter {
    async fn execute(&self, command: BrowserCommand) -> Result<BrowserResponse> {
        // Increment command count
        self.command_count.fetch_add(1, Ordering::SeqCst);

        // Check if not connected
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(BrowserResponse::error(
                &command.id,
                "Browser extension not connected",
            ));
        }

        // Check fail all mode
        if self.fail_all.load(Ordering::SeqCst) {
            return Ok(BrowserResponse::error(&command.id, "Simulated failure"));
        }

        // Check for pre-set response
        if let Some(response) = self.pending_responses.write().await.remove(&command.id) {
            return Ok(response);
        }

        // Check for custom handler
        if let Some(handler) = self.response_handler.read().await.as_ref() {
            return Ok(handler(&command));
        }

        // Default: return success response based on command type
        let result = match command.command {
            CommandType::Navigate => {
                serde_json::json!({
                    "url": command.params.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                    "title": "Mock Page",
                    "status": 200
                })
            }
            CommandType::Click => {
                serde_json::json!({
                    "clicked": true,
                    "selector": command.params.get("selector").and_then(|v| v.as_str()).unwrap_or("")
                })
            }
            CommandType::Screenshot => {
                serde_json::json!({
                    "data": "base64_encoded_screenshot_data",
                    "width": 1920,
                    "height": 1080,
                    "format": "png"
                })
            }
            CommandType::GetContent => {
                serde_json::json!({
                    "html": "<html><body>Mock Content</body></html>",
                    "text": "Mock Content"
                })
            }
            CommandType::Fill => {
                serde_json::json!({
                    "filled": true,
                    "selector": command.params.get("selector").and_then(|v| v.as_str()).unwrap_or("")
                })
            }
            CommandType::ExecuteScript => {
                serde_json::json!({
                    "result": "script_result",
                    "executed": true
                })
            }
            CommandType::GetText => {
                serde_json::json!({
                    "text": "Mock element text"
                })
            }
            _ => serde_json::json!({"success": true}),
        };

        Ok(BrowserResponse::success(&command.id, result))
    }

    async fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn connection_status(&self) -> ConnectionStatus {
        ConnectionStatus {
            connected: self.connected.load(Ordering::SeqCst),
            mode: self.mode,
            last_ping: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            ),
            pending_commands: 0,
        }
    }

    async fn disconnect(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn reconnect(&self) -> Result<()> {
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Mock extension connection for testing ExtensionManager interactions
pub struct MockExtensionConnection {
    /// Extension ID
    pub extension_id: String,
    /// Channel sender for receiving messages from manager
    pub rx: mpsc::Receiver<BrowserMessage>,
    /// Channel sender for the manager to use
    pub tx: mpsc::Sender<BrowserMessage>,
    /// Connection ID assigned by manager
    pub connection_id: Option<String>,
}

impl MockExtensionConnection {
    /// Create a new mock extension connection
    pub fn new(extension_id: impl Into<String>) -> Self {
        let (tx, rx) = mpsc::channel(100);
        Self {
            extension_id: extension_id.into(),
            rx,
            tx,
            connection_id: None,
        }
    }

    /// Get the sender for use with ExtensionManager
    pub fn sender(&self) -> mpsc::Sender<BrowserMessage> {
        self.tx.clone()
    }

    /// Receive a message (non-blocking check)
    pub async fn try_receive(&mut self) -> Option<BrowserMessage> {
        self.rx.try_recv().ok()
    }

    /// Receive a message with timeout
    pub async fn receive_timeout(&mut self, timeout_ms: u64) -> Option<BrowserMessage> {
        tokio::time::timeout(Duration::from_millis(timeout_ms), self.rx.recv())
            .await
            .ok()
            .flatten()
    }
}

// ============================================================================
// Local Browser Navigate Tests
// ============================================================================

#[tokio::test]
async fn test_local_browser_navigate() {
    // Test navigation command routing through mock router
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::navigate("nav-1", "https://example.com");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert_eq!(response.id, "nav-1");
    assert!(response.result.is_some());

    let result = response.result.unwrap();
    assert_eq!(result["url"], "https://example.com");
    assert_eq!(result["title"], "Mock Page");
}

#[tokio::test]
async fn test_local_browser_navigate_disconnected() {
    let router = MockBrowserRouter::new(); // Not connected

    let cmd = BrowserCommand::navigate("nav-2", "https://example.com");
    let response = router.execute(cmd).await.unwrap();

    assert!(!response.success);
    assert!(response.error.unwrap().contains("not connected"));
}

#[tokio::test]
async fn test_local_browser_navigate_with_tab_id() {
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::navigate("nav-3", "https://example.com").with_tab_id(42);
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert_eq!(response.id, "nav-3");
}

#[tokio::test]
async fn test_local_browser_navigate_with_timeout() {
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::navigate("nav-4", "https://example.com").with_timeout(5000);
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
}

// ============================================================================
// Local Browser Click Tests
// ============================================================================

#[tokio::test]
async fn test_local_browser_click() {
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::click("click-1", "#submit-button");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert_eq!(response.id, "click-1");

    let result = response.result.unwrap();
    assert_eq!(result["clicked"], true);
    assert_eq!(result["selector"], "#submit-button");
}

#[tokio::test]
async fn test_local_browser_click_with_custom_response() {
    let router = MockBrowserRouter::connected();

    // Set custom response for element not found
    router
        .set_response(
            "click-2",
            BrowserResponse::error_with_code(
                "click-2",
                "Element not found: #nonexistent",
                "ELEMENT_NOT_FOUND",
            ),
        )
        .await;

    let cmd = BrowserCommand::click("click-2", "#nonexistent");
    let response = router.execute(cmd).await.unwrap();

    assert!(!response.success);
    assert!(response.error.unwrap().contains("Element not found"));
    assert_eq!(response.error_code, Some("ELEMENT_NOT_FOUND".to_string()));
}

#[tokio::test]
async fn test_local_browser_click_multiple_commands() {
    let router = MockBrowserRouter::connected();

    // Execute multiple click commands
    for i in 1..=5 {
        let cmd = BrowserCommand::click(format!("click-multi-{}", i), format!("#btn-{}", i));
        let response = router.execute(cmd).await.unwrap();
        assert!(response.success);
    }

    assert_eq!(router.command_count(), 5);
}

// ============================================================================
// Local Browser Screenshot Tests
// ============================================================================

#[tokio::test]
async fn test_local_browser_screenshot() {
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::screenshot("screenshot-1");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert_eq!(response.id, "screenshot-1");

    let result = response.result.unwrap();
    assert!(result["data"].as_str().is_some());
    assert_eq!(result["width"], 1920);
    assert_eq!(result["height"], 1080);
    assert_eq!(result["format"], "png");
}

#[tokio::test]
async fn test_local_browser_screenshot_element() {
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::screenshot_element("screenshot-2", "#main-content");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert_eq!(response.id, "screenshot-2");
}

#[tokio::test]
async fn test_local_browser_screenshot_with_custom_handler() {
    let router = MockBrowserRouter::connected();

    // Set custom handler that returns specific screenshot data
    router
        .set_response_handler(|cmd| {
            if cmd.command == CommandType::Screenshot {
                BrowserResponse::success(
                    &cmd.id,
                    serde_json::json!({
                        "data": "custom_base64_data",
                        "width": 800,
                        "height": 600,
                        "format": "jpeg"
                    }),
                )
            } else {
                BrowserResponse::success(&cmd.id, serde_json::json!({}))
            }
        })
        .await;

    let cmd = BrowserCommand::screenshot("screenshot-3");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();
    assert_eq!(result["data"], "custom_base64_data");
    assert_eq!(result["format"], "jpeg");
}

// ============================================================================
// Extension Manager Connection Tests
// ============================================================================

#[tokio::test]
async fn test_extension_manager_connection() {
    let manager = ExtensionManagerBuilder::new()
        .max_connections(5)
        .heartbeat_timeout(5000)
        .command_timeout(1000)
        .build();

    // Initially no connections
    assert_eq!(manager.connection_count(), 0);
    assert!(!manager.is_connected().await);

    // Connect an extension
    let (tx, _rx) = mpsc::channel(10);
    let auth_request = ExtensionAuthRequest {
        extension_id: "test-ext-001".to_string(),
        name: Some("Test Extension".to_string()),
        version: Some("1.0.0".to_string()),
        auth_token: None,
    };

    let result = manager
        .authenticate(auth_request, tx, Some("127.0.0.1:12345".to_string()))
        .await;

    assert!(result.success);
    assert!(result.connection_id.is_some());
    assert_eq!(manager.connection_count(), 1);
    assert!(manager.is_connected().await);

    // Verify connection info
    let info = manager.get_extension("test-ext-001");
    assert!(info.is_some());
    let info = info.unwrap();
    assert_eq!(info.extension_id, "test-ext-001");
    assert_eq!(info.name, Some("Test Extension".to_string()));
    assert_eq!(info.state, ExtensionState::Connected);
    assert_eq!(info.remote_addr, Some("127.0.0.1:12345".to_string()));
}

#[tokio::test]
async fn test_extension_manager_multiple_connections() {
    let manager = ExtensionManagerBuilder::new().max_connections(10).build();

    // Connect multiple extensions
    for i in 1..=5 {
        let (tx, _rx) = mpsc::channel(10);
        let auth_request = ExtensionAuthRequest {
            extension_id: format!("ext-{}", i),
            name: Some(format!("Extension {}", i)),
            version: None,
            auth_token: None,
        };

        let result = manager.authenticate(auth_request, tx, None).await;
        assert!(result.success, "Extension {} should connect", i);
    }

    assert_eq!(manager.connection_count(), 5);

    // List all connections
    let connections = manager.list_connections();
    assert_eq!(connections.len(), 5);
}

#[tokio::test]
async fn test_extension_manager_connection_limit() {
    let manager = ExtensionManagerBuilder::new().max_connections(2).build();

    // Try to connect 3 extensions (should fail on 3rd)
    for i in 1..=3 {
        let (tx, _rx) = mpsc::channel(10);
        let auth_request = ExtensionAuthRequest {
            extension_id: format!("ext-{}", i),
            name: None,
            version: None,
            auth_token: None,
        };

        let result = manager.authenticate(auth_request, tx, None).await;

        if i <= 2 {
            assert!(result.success, "Extension {} should connect", i);
        } else {
            assert!(!result.success, "Extension {} should be rejected", i);
            assert!(result.error.unwrap().contains("Maximum connections"));
        }
    }
}

#[tokio::test]
async fn test_extension_manager_disconnect() {
    let manager = ExtensionManager::new();
    let (tx, _rx) = mpsc::channel(10);

    let auth_request = ExtensionAuthRequest {
        extension_id: "test-ext".to_string(),
        name: None,
        version: None,
        auth_token: None,
    };

    let result = manager.authenticate(auth_request, tx, None).await;
    assert!(result.success);
    assert_eq!(manager.connection_count(), 1);

    // Disconnect by connection ID
    let conn_id = result.connection_id.unwrap();
    manager.disconnect_connection(&conn_id).await;

    assert_eq!(manager.connection_count(), 0);
    assert!(!manager.is_connected().await);
}

#[tokio::test]
async fn test_extension_manager_reconnect_replaces_old() {
    let manager = ExtensionManager::new();

    // First connection
    let (tx1, _rx1) = mpsc::channel(10);
    let auth1 = ExtensionAuthRequest {
        extension_id: "same-ext".to_string(),
        name: Some("First".to_string()),
        version: None,
        auth_token: None,
    };
    let result1 = manager.authenticate(auth1, tx1, None).await;
    assert!(result1.success);

    // Second connection with same extension ID
    let (tx2, _rx2) = mpsc::channel(10);
    let auth2 = ExtensionAuthRequest {
        extension_id: "same-ext".to_string(),
        name: Some("Second".to_string()),
        version: None,
        auth_token: None,
    };
    let result2 = manager.authenticate(auth2, tx2, None).await;
    assert!(result2.success);

    // Should only have one connection
    assert_eq!(manager.connection_count(), 1);

    // Should be the new connection
    let info = manager.get_extension("same-ext").unwrap();
    assert_eq!(info.name, Some("Second".to_string()));
}

// ============================================================================
// Browser Permission Check Tests
// ============================================================================

#[tokio::test]
async fn test_browser_permission_check_allow() {
    let checker = BrowserPermissionChecker::new(DomainRules::default());

    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "example.com".to_string(),
        })
        .await;

    assert!(decision.is_allowed());
}

#[tokio::test]
async fn test_browser_permission_check_blocked_domain() {
    let rules = DomainRules::default()
        .with_blocked("*.bank.com")
        .with_blocked("paypal.com");

    let checker = BrowserPermissionChecker::new(rules);

    // Test blocked wildcard
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "my.bank.com".to_string(),
        })
        .await;
    assert!(decision.is_denied());

    // Test blocked exact
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "paypal.com".to_string(),
        })
        .await;
    assert!(decision.is_denied());

    // Test allowed domain
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "example.com".to_string(),
        })
        .await;
    assert!(decision.is_allowed());
}

#[tokio::test]
async fn test_browser_permission_check_confirm_required() {
    let rules = DomainRules::default()
        .with_confirm_required("github.com")
        .with_confirm_required("mail.google.com");

    let checker = BrowserPermissionChecker::new(rules);

    let decision = checker
        .check(BrowserPermission::Click {
            domain: "github.com".to_string(),
            element: "#submit".to_string(),
        })
        .await;

    assert!(decision.requires_confirmation());
}

#[tokio::test]
async fn test_browser_permission_check_sensitive_action() {
    let checker = BrowserPermissionChecker::new(DomainRules::default());

    // ExecuteScript is always sensitive
    let decision = checker
        .check(BrowserPermission::ExecuteScript {
            domain: "example.com".to_string(),
        })
        .await;

    assert!(decision.requires_confirmation());

    // WriteCookies is always sensitive
    let decision = checker
        .check(BrowserPermission::WriteCookies {
            domain: "example.com".to_string(),
        })
        .await;

    assert!(decision.requires_confirmation());
}

#[tokio::test]
async fn test_browser_permission_check_read_only_domain() {
    let rules = DomainRules::default().with_read_only("search.example.com");

    let checker = BrowserPermissionChecker::new(rules);

    // Read actions should be allowed
    let decision = checker
        .check(BrowserPermission::Screenshot {
            domain: "search.example.com".to_string(),
        })
        .await;
    assert!(decision.is_allowed());

    // Write actions should be denied
    let decision = checker
        .check(BrowserPermission::Click {
            domain: "search.example.com".to_string(),
            element: "#btn".to_string(),
        })
        .await;
    assert!(decision.is_denied());
}

#[tokio::test]
async fn test_browser_permission_check_bypass_mode() {
    let rules = DomainRules::default().with_blocked("*.bank.com");
    let checker = BrowserPermissionChecker::new(rules);

    // Should be blocked normally
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "my.bank.com".to_string(),
        })
        .await;
    assert!(decision.is_denied());

    // Enable bypass mode
    checker.set_bypass_mode(true).await;

    // Should now be allowed
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "my.bank.com".to_string(),
        })
        .await;
    assert!(decision.is_allowed());
}

#[tokio::test]
async fn test_browser_permission_check_url() {
    let checker = BrowserPermissionChecker::new(DomainRules::default());

    let decision = checker
        .check_url("https://example.com/path", BrowserAction::Navigate)
        .await;

    assert!(decision.is_allowed());
}

#[tokio::test]
async fn test_browser_permission_check_invalid_url() {
    let checker = BrowserPermissionChecker::new(DomainRules::default());

    let decision = checker
        .check_url("not a valid url", BrowserAction::Navigate)
        .await;

    assert!(decision.is_denied());
}

#[tokio::test]
async fn test_browser_permission_check_sensitive_fill() {
    let rules = DomainRules::default().with_sensitive_field_pattern("*password*");

    let checker = BrowserPermissionChecker::new(rules);

    // Fill password field should require confirmation
    let decision = checker
        .check_fill("https://example.com", "#password-input", false)
        .await;

    assert!(decision.requires_confirmation());

    // Fill normal field should be allowed
    let decision = checker
        .check_fill("https://example.com", "#username", false)
        .await;

    assert!(decision.is_allowed());
}

#[tokio::test]
async fn test_browser_permission_default_security() {
    let checker = BrowserPermissionChecker::with_default_security();

    // Banking domains should be blocked
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "www.chase.com".to_string(),
        })
        .await;
    assert!(decision.is_denied());

    // GitHub should require confirmation
    let decision = checker
        .check(BrowserPermission::Click {
            domain: "github.com".to_string(),
            element: "#delete-repo".to_string(),
        })
        .await;
    assert!(decision.requires_confirmation());

    // Regular domains should be allowed
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "docs.rust-lang.org".to_string(),
        })
        .await;
    assert!(decision.is_allowed());
}

// ============================================================================
// Command Routing Integration Tests
// ============================================================================

#[tokio::test]
async fn test_command_routing_navigate_success() {
    let router = MockBrowserRouter::connected();

    // Navigate command
    let cmd = BrowserCommand::navigate("route-1", "https://example.com");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert_eq!(router.command_count(), 1);
}

#[tokio::test]
async fn test_command_routing_fill_success() {
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::fill("route-2", "#email", "test@example.com");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();
    assert_eq!(result["filled"], true);
}

#[tokio::test]
async fn test_command_routing_execute_script() {
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::execute_script("route-3", "return document.title;");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();
    assert_eq!(result["executed"], true);
}

#[tokio::test]
async fn test_command_routing_get_content() {
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::get_content("route-4");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();
    assert!(result["html"].as_str().is_some());
}

#[tokio::test]
async fn test_command_routing_wait_for_element() {
    let router = MockBrowserRouter::connected();

    let cmd = BrowserCommand::wait_for_element("route-5", "#loading-complete");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
}

#[tokio::test]
async fn test_command_routing_fail_mode() {
    let router = MockBrowserRouter::connected();
    router.set_fail_all(true);

    let cmd = BrowserCommand::navigate("fail-1", "https://example.com");
    let response = router.execute(cmd).await.unwrap();

    assert!(!response.success);
    assert!(response.error.unwrap().contains("Simulated failure"));
}

// ============================================================================
// NoOp Router Tests
// ============================================================================

#[tokio::test]
async fn test_noop_router_not_connected() {
    let router = NoOpRouter;

    assert!(!router.is_connected().await);
}

#[tokio::test]
async fn test_noop_router_execute_returns_error() {
    let router = NoOpRouter;

    let cmd = BrowserCommand::navigate("noop-1", "https://example.com");
    let response = router.execute(cmd).await.unwrap();

    assert!(!response.success);
    assert!(response.error.unwrap().contains("not connected"));
}

#[tokio::test]
async fn test_noop_router_connection_status() {
    let router = NoOpRouter;

    let status = router.connection_status().await;
    assert!(!status.connected);
    assert_eq!(status.mode, ConnectionMode::Unknown);
}

// ============================================================================
// Event Handling Tests
// ============================================================================

#[tokio::test]
async fn test_event_handling() {
    let router = MockBrowserRouter::connected();

    let event_received = Arc::new(AtomicBool::new(false));
    let event_received_clone = event_received.clone();

    router
        .on_event(move |event| {
            if event.event == EventType::PageLoaded {
                event_received_clone.store(true, Ordering::SeqCst);
            }
        })
        .await;

    // Trigger event
    let event = BrowserEvent::page_loaded("https://example.com", "Example Page");
    router.trigger_event(event).await;

    assert!(event_received.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_multiple_event_handlers() {
    let router = MockBrowserRouter::connected();

    let counter = Arc::new(AtomicU64::new(0));
    let counter1 = counter.clone();
    let counter2 = counter.clone();

    router
        .on_event(move |_event| {
            counter1.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    router
        .on_event(move |_event| {
            counter2.fetch_add(1, Ordering::SeqCst);
        })
        .await;

    // Trigger event
    let event = BrowserEvent::page_loaded("https://example.com", "Example");
    router.trigger_event(event).await;

    // Both handlers should be called
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

// ============================================================================
// Extension Manager Message Handling Tests
// ============================================================================

#[tokio::test]
async fn test_extension_manager_handle_response() {
    let manager = ExtensionManager::new();
    let (tx, mut rx) = mpsc::channel(10);

    // Connect extension
    let auth_request = ExtensionAuthRequest {
        extension_id: "test-ext".to_string(),
        name: None,
        version: None,
        auth_token: None,
    };
    let result = manager.authenticate(auth_request, tx, None).await;
    let conn_id = result.connection_id.unwrap();

    // Spawn task to handle the response
    let manager_clone = Arc::new(manager);
    let conn_id_clone = conn_id.clone();

    // Start command execution in background
    let manager_for_exec = manager_clone.clone();
    let exec_handle = tokio::spawn(async move {
        let cmd = BrowserCommand::navigate("test-cmd", "https://example.com");
        manager_for_exec.execute(cmd).await
    });

    // Wait for command to be sent
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Simulate receiving command and sending response
    if let Some(msg) = rx.try_recv().ok() {
        if let BrowserMessage::Command(cmd) = msg {
            // Send response back
            let response =
                BrowserResponse::success(&cmd.id, serde_json::json!({"navigated": true}));
            manager_clone
                .handle_message(&conn_id_clone, BrowserMessage::Response(response))
                .await;
        }
    }

    // Wait for execution to complete
    let exec_result = exec_handle.await.unwrap();
    assert!(exec_result.is_ok());
}

#[tokio::test]
async fn test_extension_manager_handle_event() {
    let manager = ExtensionManager::new();
    let (tx, _rx) = mpsc::channel(10);

    // Connect extension
    let auth_request = ExtensionAuthRequest {
        extension_id: "test-ext".to_string(),
        name: None,
        version: None,
        auth_token: None,
    };
    let result = manager.authenticate(auth_request, tx, None).await;
    let conn_id = result.connection_id.unwrap();

    // Set up event handler
    let event_received = Arc::new(AtomicBool::new(false));
    let event_received_clone = event_received.clone();

    manager
        .on_event(Arc::new(move |ext_id, _event| {
            if ext_id == "test-ext" {
                event_received_clone.store(true, Ordering::SeqCst);
            }
        }))
        .await;

    // Handle event
    let event = BrowserEvent::page_loaded("https://example.com", "Example");
    manager
        .handle_message(&conn_id, BrowserMessage::Event(event))
        .await;

    assert!(event_received.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_extension_manager_handle_heartbeat() {
    let manager = ExtensionManager::new();
    let (tx, _rx) = mpsc::channel(10);

    // Connect extension
    let auth_request = ExtensionAuthRequest {
        extension_id: "test-ext".to_string(),
        name: None,
        version: None,
        auth_token: None,
    };
    let result = manager.authenticate(auth_request, tx, None).await;
    let conn_id = result.connection_id.unwrap();

    // Send heartbeat
    let timestamp = 1234567890u64;
    manager.handle_heartbeat(&conn_id, timestamp);

    // Verify heartbeat was recorded
    let info = manager.get_connection(&conn_id).unwrap();
    assert_eq!(info.last_heartbeat, Some(timestamp));
}

// ============================================================================
// Authentication Tests
// ============================================================================

#[tokio::test]
async fn test_extension_auth_with_allowlist() {
    let manager = ExtensionManagerBuilder::new()
        .allowed_extension_ids(vec!["allowed-ext".to_string()])
        .build();

    // Try to connect non-allowed extension
    let (tx1, _rx1) = mpsc::channel(10);
    let auth1 = ExtensionAuthRequest {
        extension_id: "not-allowed".to_string(),
        name: None,
        version: None,
        auth_token: None,
    };
    let result1 = manager.authenticate(auth1, tx1, None).await;
    assert!(!result1.success);
    assert!(result1.error.unwrap().contains("not in allowlist"));

    // Connect allowed extension
    let (tx2, _rx2) = mpsc::channel(10);
    let auth2 = ExtensionAuthRequest {
        extension_id: "allowed-ext".to_string(),
        name: None,
        version: None,
        auth_token: None,
    };
    let result2 = manager.authenticate(auth2, tx2, None).await;
    assert!(result2.success);
}

#[tokio::test]
async fn test_extension_auth_with_token() {
    let manager = ExtensionManagerBuilder::new()
        .require_auth_token(true)
        .auth_tokens(vec!["valid-token-123".to_string()])
        .build();

    // Try without token
    let (tx1, _rx1) = mpsc::channel(10);
    let auth1 = ExtensionAuthRequest {
        extension_id: "ext-1".to_string(),
        name: None,
        version: None,
        auth_token: None,
    };
    let result1 = manager.authenticate(auth1, tx1, None).await;
    assert!(!result1.success);
    assert!(result1.error.unwrap().contains("token required"));

    // Try with invalid token
    let (tx2, _rx2) = mpsc::channel(10);
    let auth2 = ExtensionAuthRequest {
        extension_id: "ext-2".to_string(),
        name: None,
        version: None,
        auth_token: Some("invalid-token".to_string()),
    };
    let result2 = manager.authenticate(auth2, tx2, None).await;
    assert!(!result2.success);
    assert!(result2.error.unwrap().contains("Invalid"));

    // Try with valid token
    let (tx3, _rx3) = mpsc::channel(10);
    let auth3 = ExtensionAuthRequest {
        extension_id: "ext-3".to_string(),
        name: None,
        version: None,
        auth_token: Some("valid-token-123".to_string()),
    };
    let result3 = manager.authenticate(auth3, tx3, None).await;
    assert!(result3.success);
}

// ============================================================================
// LocalBrowserClient Tests
// ============================================================================

#[tokio::test]
async fn test_local_browser_client_creation() {
    let client = LocalBrowserClient::new("ws://127.0.0.1:9876".to_string());

    assert_eq!(client.url(), "ws://127.0.0.1:9876");
    assert!(!client.is_connected().await);
}

#[tokio::test]
async fn test_local_browser_client_builder() {
    let client = LocalBrowserClientBuilder::new("ws://localhost:9876")
        .timeout(5000)
        .max_pending(50)
        .auto_reconnect(false)
        .reconnect_interval(10000)
        .max_reconnect_attempts(3)
        .ping_interval(60000)
        .build();

    assert_eq!(client.url(), "ws://localhost:9876");
}

#[tokio::test]
async fn test_local_browser_client_request_id_generation() {
    let client = LocalBrowserClient::new("ws://127.0.0.1:9876".to_string());

    let id1 = client.next_request_id();
    let id2 = client.next_request_id();
    let id3 = client.next_request_id();

    assert!(id1.starts_with("req-"));
    assert!(id2.starts_with("req-"));
    assert!(id3.starts_with("req-"));
    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
}

#[tokio::test]
async fn test_local_browser_client_execute_when_disconnected() {
    let client = LocalBrowserClient::new("ws://127.0.0.1:9876".to_string());

    let cmd = BrowserCommand::navigate("test", "https://example.com");
    let response = client.execute(cmd).await.unwrap();

    assert!(!response.success);
    assert!(response.error.unwrap().contains("not connected"));
}

#[tokio::test]
async fn test_local_browser_client_connection_status() {
    let client = LocalBrowserClient::new("ws://127.0.0.1:9876".to_string());

    let status = client.connection_status().await;
    assert!(!status.connected);
    assert_eq!(status.mode, ConnectionMode::Unknown);
    assert_eq!(status.pending_commands, 0);
}

#[tokio::test]
async fn test_local_browser_client_disconnect() {
    let client = LocalBrowserClient::new("ws://127.0.0.1:9876".to_string());

    // Should not error when disconnecting from non-connected client
    let result = client.disconnect().await;
    assert!(result.is_ok());
}

// ============================================================================
// Domain Rules Tests
// ============================================================================

#[test]
fn test_domain_rules_builder() {
    let rules = DomainRules::new()
        .with_blocked("*.bank.com")
        .with_confirm_required("github.com")
        .with_read_only("search.google.com")
        .with_allowed("*.example.com")
        .with_sensitive_field_pattern("*password*");

    assert!(rules.is_blocked("my.bank.com"));
    assert!(rules.requires_confirmation("github.com"));
    assert!(rules.is_read_only("search.google.com"));
    assert!(rules.is_allowed("docs.example.com"));
    assert!(rules.is_sensitive_field("#password-input"));
}

#[test]
fn test_domain_rules_default_security() {
    let rules = DomainRules::with_default_security();

    // Check blocked domains
    assert!(rules.is_blocked("www.chase.com"));
    assert!(rules.is_blocked("accounts.google.com"));
    assert!(rules.is_blocked("www.paypal.com"));

    // Check confirmation required
    assert!(rules.requires_confirmation("mail.google.com"));
    assert!(rules.requires_confirmation("github.com"));

    // Check sensitive patterns
    assert!(rules.is_sensitive_field("#password"));
    assert!(rules.is_sensitive_field("input[name='cvv']"));
}

// ============================================================================
// Browser Permission Types Tests
// ============================================================================

#[test]
fn test_browser_permission_properties() {
    // Test is_read_only
    assert!(BrowserPermission::Screenshot {
        domain: "test.com".to_string()
    }
    .is_read_only());
    assert!(BrowserPermission::ReadCookies {
        domain: "test.com".to_string()
    }
    .is_read_only());
    assert!(BrowserPermission::ReadContent {
        domain: "test.com".to_string()
    }
    .is_read_only());
    assert!(!BrowserPermission::Click {
        domain: "test.com".to_string(),
        element: "#btn".to_string()
    }
    .is_read_only());

    // Test is_write
    assert!(BrowserPermission::Navigate {
        domain: "test.com".to_string()
    }
    .is_write());
    assert!(BrowserPermission::Click {
        domain: "test.com".to_string(),
        element: "#btn".to_string()
    }
    .is_write());

    // Test is_sensitive
    assert!(BrowserPermission::ExecuteScript {
        domain: "test.com".to_string()
    }
    .is_sensitive());
    assert!(BrowserPermission::WriteCookies {
        domain: "test.com".to_string()
    }
    .is_sensitive());
    assert!(BrowserPermission::Fill {
        domain: "test.com".to_string(),
        element: "#pass".to_string(),
        sensitive: true
    }
    .is_sensitive());
}

#[test]
fn test_browser_permission_description() {
    let perm = BrowserPermission::Navigate {
        domain: "example.com".to_string(),
    };
    assert!(perm.description().contains("Navigate"));
    assert!(perm.description().contains("example.com"));

    let perm = BrowserPermission::Click {
        domain: "test.com".to_string(),
        element: "#submit".to_string(),
    };
    assert!(perm.description().contains("Click"));
    assert!(perm.description().contains("#submit"));
}

#[test]
fn test_browser_permission_action_name() {
    assert_eq!(
        BrowserPermission::Navigate {
            domain: "t.com".to_string()
        }
        .action_name(),
        "navigate"
    );
    assert_eq!(
        BrowserPermission::Click {
            domain: "t.com".to_string(),
            element: "".to_string()
        }
        .action_name(),
        "click"
    );
    assert_eq!(
        BrowserPermission::Fill {
            domain: "t.com".to_string(),
            element: "".to_string(),
            sensitive: false
        }
        .action_name(),
        "fill"
    );
    assert_eq!(
        BrowserPermission::Screenshot {
            domain: "t.com".to_string()
        }
        .action_name(),
        "screenshot"
    );
}

// ============================================================================
// Permission Decision Tests
// ============================================================================

#[test]
fn test_permission_decision_factories() {
    let allow = PermissionDecision::allow();
    assert!(allow.is_allowed());
    assert!(!allow.is_denied());
    assert!(!allow.requires_confirmation());

    let deny = PermissionDecision::deny("Blocked domain");
    assert!(!deny.is_allowed());
    assert!(deny.is_denied());
    assert!(!deny.requires_confirmation());

    let confirm = PermissionDecision::require_confirmation("Please confirm this action");
    assert!(!confirm.is_allowed());
    assert!(!confirm.is_denied());
    assert!(confirm.requires_confirmation());
}

// ============================================================================
// Integration: Full Flow Tests
// ============================================================================

#[tokio::test]
async fn test_full_navigation_flow() {
    // Create mock router
    let router = MockBrowserRouter::connected();

    // Create permission checker
    let checker = BrowserPermissionChecker::new(DomainRules::default());

    // Check permission first
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "example.com".to_string(),
        })
        .await;
    assert!(decision.is_allowed());

    // Execute navigation
    let cmd = BrowserCommand::navigate("flow-1", "https://example.com");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);
}

#[tokio::test]
async fn test_full_blocked_flow() {
    // Create mock router (kept for future extension)
    let _router = MockBrowserRouter::connected();

    // Create permission checker with blocked domain
    let rules = DomainRules::default().with_blocked("*.bank.com");
    let checker = BrowserPermissionChecker::new(rules);

    // Check permission - should be denied
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "my.bank.com".to_string(),
        })
        .await;
    assert!(decision.is_denied());

    // Command should not be executed if permission denied
    // In real implementation, the caller would check permission first
}

#[tokio::test]
async fn test_full_click_flow_with_confirmation() {
    // Create mock router
    let router = MockBrowserRouter::connected();

    // Create permission checker requiring confirmation
    let rules = DomainRules::default().with_confirm_required("github.com");
    let checker = BrowserPermissionChecker::new(rules);

    // Check permission - should require confirmation
    let decision = checker
        .check(BrowserPermission::Click {
            domain: "github.com".to_string(),
            element: "#delete-repo".to_string(),
        })
        .await;
    assert!(decision.requires_confirmation());

    // In real implementation, UI would prompt user
    // If confirmed, execute the command
    let cmd = BrowserCommand::click("flow-2", "#delete-repo");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);
}

// ============================================================================
// Wave 6: Console and Network Monitoring Integration Tests
// ============================================================================

/// Mock browser router that supports Console/Network monitoring commands
pub struct MockMonitoringRouter {
    connected: AtomicBool,
    /// Console monitoring state
    console_monitoring: Arc<RwLock<HashMap<Option<i32>, bool>>>,
    /// Console messages by tab
    console_messages: Arc<RwLock<HashMap<Option<i32>, Vec<ConsoleMessage>>>>,
    /// Network monitoring state
    network_monitoring: Arc<AtomicBool>,
    /// Network requests
    network_requests: Arc<RwLock<Vec<NetworkRequest>>>,
}

impl MockMonitoringRouter {
    pub fn new() -> Self {
        Self {
            connected: AtomicBool::new(true),
            console_monitoring: Arc::new(RwLock::new(HashMap::new())),
            console_messages: Arc::new(RwLock::new(HashMap::new())),
            network_monitoring: Arc::new(AtomicBool::new(false)),
            network_requests: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn add_console_message(&self, tab_id: Option<i32>, message: ConsoleMessage) {
        let mut messages = self.console_messages.write().await;
        messages.entry(tab_id).or_default().push(message);
    }

    pub async fn add_network_request(&self, request: NetworkRequest) {
        let mut requests = self.network_requests.write().await;
        requests.push(request);
    }
}

#[async_trait]
impl BrowserRouter for MockMonitoringRouter {
    async fn execute(&self, command: BrowserCommand) -> Result<BrowserResponse> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(BrowserResponse::error(&command.id, "Not connected"));
        }

        match command.command {
            CommandType::ConsoleStart => {
                let tab_id = command
                    .params
                    .get("tab_id")
                    .and_then(|v| v.as_i64().map(|i| i as i32));
                let mut monitoring = self.console_monitoring.write().await;
                monitoring.insert(tab_id, true);
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "message": "Console monitoring started"
                    }),
                ))
            }
            CommandType::ConsoleStop => {
                let tab_id = command
                    .params
                    .get("tab_id")
                    .and_then(|v| v.as_i64().map(|i| i as i32));
                let mut monitoring = self.console_monitoring.write().await;
                monitoring.insert(tab_id, false);
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "message": "Console monitoring stopped"
                    }),
                ))
            }
            CommandType::ConsoleGet => {
                let tab_id = command
                    .params
                    .get("tab_id")
                    .and_then(|v| v.as_i64().map(|i| i as i32));
                let messages = self.console_messages.read().await;
                let msgs = messages.get(&tab_id).cloned().unwrap_or_default();
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "messages": msgs
                    }),
                ))
            }
            CommandType::ConsoleClear => {
                let tab_id = command
                    .params
                    .get("tab_id")
                    .and_then(|v| v.as_i64().map(|i| i as i32));
                let mut messages = self.console_messages.write().await;
                messages.remove(&tab_id);
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "cleared": true
                    }),
                ))
            }
            CommandType::ConsoleStatus => {
                let tab_id = command
                    .params
                    .get("tab_id")
                    .and_then(|v| v.as_i64().map(|i| i as i32));
                let monitoring = self.console_monitoring.read().await;
                let is_monitoring = *monitoring.get(&tab_id).unwrap_or(&false);
                let messages = self.console_messages.read().await;
                let count = messages.get(&tab_id).map(|v| v.len()).unwrap_or(0);
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "monitoring": is_monitoring,
                        "message_count": count
                    }),
                ))
            }
            CommandType::NetworkStart => {
                self.network_monitoring.store(true, Ordering::SeqCst);
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "message": "Network monitoring started"
                    }),
                ))
            }
            CommandType::NetworkStop => {
                self.network_monitoring.store(false, Ordering::SeqCst);
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "message": "Network monitoring stopped"
                    }),
                ))
            }
            CommandType::NetworkGet => {
                let requests = self.network_requests.read().await;
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "requests": requests.clone()
                    }),
                ))
            }
            CommandType::NetworkGetFailed => {
                let requests = self.network_requests.read().await;
                let failed: Vec<_> = requests
                    .iter()
                    .filter(|r| r.status_code >= 400 || r.error.is_some())
                    .cloned()
                    .collect();
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "requests": failed
                    }),
                ))
            }
            CommandType::NetworkClear => {
                let mut requests = self.network_requests.write().await;
                requests.clear();
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "cleared": true
                    }),
                ))
            }
            CommandType::NetworkStatus => {
                let is_monitoring = self.network_monitoring.load(Ordering::SeqCst);
                let requests = self.network_requests.read().await;
                let failed_count = requests.iter().filter(|r| r.status_code >= 400).count();
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "monitoring": is_monitoring,
                        "request_count": requests.len(),
                        "failed_count": failed_count
                    }),
                ))
            }
            _ => Ok(BrowserResponse::success(
                &command.id,
                serde_json::json!({"success": true}),
            )),
        }
    }

    async fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn connection_status(&self) -> ConnectionStatus {
        ConnectionStatus {
            connected: self.connected.load(Ordering::SeqCst),
            mode: ConnectionMode::WebSocket,
            last_ping: None,
            pending_commands: 0,
        }
    }

    async fn disconnect(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn reconnect(&self) -> Result<()> {
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Console message structure for testing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleMessage {
    pub level: String,
    pub text: String,
    pub timestamp: u64,
    pub source: Option<String>,
    pub line: Option<u32>,
}

/// Network request structure for testing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkRequest {
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub resource_type: String,
    pub status_code: u16,
    pub timestamp: u64,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
}

// ============================================================================
// Console Monitoring Tests
// ============================================================================

#[tokio::test]
async fn test_console_start_monitoring() {
    let router = MockMonitoringRouter::new();

    let cmd = BrowserCommand::new(
        "console-1",
        CommandType::ConsoleStart,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert!(response.result.unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("started"));
}

#[tokio::test]
async fn test_console_stop_monitoring() {
    let router = MockMonitoringRouter::new();

    // Start first
    let cmd = BrowserCommand::new(
        "console-1",
        CommandType::ConsoleStart,
        serde_json::json!({}),
    );
    router.execute(cmd).await.unwrap();

    // Stop
    let cmd = BrowserCommand::new("console-2", CommandType::ConsoleStop, serde_json::json!({}));
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert!(response.result.unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("stopped"));
}

#[tokio::test]
async fn test_console_get_messages() {
    let router = MockMonitoringRouter::new();

    // Add test messages
    router
        .add_console_message(
            None,
            ConsoleMessage {
                level: "log".to_string(),
                text: "Test message 1".to_string(),
                timestamp: 1234567890,
                source: Some("test.js".to_string()),
                line: Some(10),
            },
        )
        .await;

    router
        .add_console_message(
            None,
            ConsoleMessage {
                level: "error".to_string(),
                text: "Test error".to_string(),
                timestamp: 1234567891,
                source: Some("test.js".to_string()),
                line: Some(20),
            },
        )
        .await;

    let cmd = BrowserCommand::new(
        "console-get-1",
        CommandType::ConsoleGet,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn test_console_clear_messages() {
    let router = MockMonitoringRouter::new();

    // Add a message
    router
        .add_console_message(
            None,
            ConsoleMessage {
                level: "log".to_string(),
                text: "Test message".to_string(),
                timestamp: 1234567890,
                source: None,
                line: None,
            },
        )
        .await;

    // Clear
    let cmd = BrowserCommand::new(
        "console-clear-1",
        CommandType::ConsoleClear,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert!(response.result.unwrap()["cleared"].as_bool().unwrap());

    // Verify cleared
    let cmd = BrowserCommand::new(
        "console-get-2",
        CommandType::ConsoleGet,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();
    let result = response.result.unwrap();
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 0);
}

#[tokio::test]
async fn test_console_status() {
    let router = MockMonitoringRouter::new();

    // Start monitoring
    let cmd = BrowserCommand::new(
        "console-start",
        CommandType::ConsoleStart,
        serde_json::json!({}),
    );
    router.execute(cmd).await.unwrap();

    // Add a message
    router
        .add_console_message(
            None,
            ConsoleMessage {
                level: "warn".to_string(),
                text: "Warning message".to_string(),
                timestamp: 1234567890,
                source: None,
                line: None,
            },
        )
        .await;

    // Check status
    let cmd = BrowserCommand::new(
        "console-status-1",
        CommandType::ConsoleStatus,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();
    assert!(result["monitoring"].as_bool().unwrap());
    assert_eq!(result["message_count"].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn test_console_tab_specific_monitoring() {
    let router = MockMonitoringRouter::new();

    // Add messages for different tabs
    router
        .add_console_message(
            Some(1),
            ConsoleMessage {
                level: "log".to_string(),
                text: "Tab 1 message".to_string(),
                timestamp: 1234567890,
                source: None,
                line: None,
            },
        )
        .await;

    router
        .add_console_message(
            Some(2),
            ConsoleMessage {
                level: "log".to_string(),
                text: "Tab 2 message".to_string(),
                timestamp: 1234567891,
                source: None,
                line: None,
            },
        )
        .await;

    // Get messages for tab 1
    let cmd = BrowserCommand::new(
        "console-get-tab1",
        CommandType::ConsoleGet,
        serde_json::json!({"tab_id": 1}),
    );
    let response = router.execute(cmd).await.unwrap();

    let result = response.result.unwrap();
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["text"].as_str().unwrap(), "Tab 1 message");
}

// ============================================================================
// Network Monitoring Tests
// ============================================================================

#[tokio::test]
async fn test_network_start_monitoring() {
    let router = MockMonitoringRouter::new();

    let cmd = BrowserCommand::new(
        "network-1",
        CommandType::NetworkStart,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert!(response.result.unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("started"));
}

#[tokio::test]
async fn test_network_stop_monitoring() {
    let router = MockMonitoringRouter::new();

    // Start first
    let cmd = BrowserCommand::new(
        "network-1",
        CommandType::NetworkStart,
        serde_json::json!({}),
    );
    router.execute(cmd).await.unwrap();

    // Stop
    let cmd = BrowserCommand::new("network-2", CommandType::NetworkStop, serde_json::json!({}));
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert!(response.result.unwrap()["message"]
        .as_str()
        .unwrap()
        .contains("stopped"));
}

#[tokio::test]
async fn test_network_get_requests() {
    let router = MockMonitoringRouter::new();

    // Add test requests
    router
        .add_network_request(NetworkRequest {
            request_id: "req-1".to_string(),
            url: "https://api.example.com/data".to_string(),
            method: "GET".to_string(),
            resource_type: "xhr".to_string(),
            status_code: 200,
            timestamp: 1234567890,
            duration_ms: Some(150),
            error: None,
        })
        .await;

    router
        .add_network_request(NetworkRequest {
            request_id: "req-2".to_string(),
            url: "https://api.example.com/users".to_string(),
            method: "POST".to_string(),
            resource_type: "fetch".to_string(),
            status_code: 201,
            timestamp: 1234567891,
            duration_ms: Some(200),
            error: None,
        })
        .await;

    let cmd = BrowserCommand::new(
        "network-get-1",
        CommandType::NetworkGet,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();
    let requests = result["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn test_network_get_failed_requests() {
    let router = MockMonitoringRouter::new();

    // Add successful request
    router
        .add_network_request(NetworkRequest {
            request_id: "req-1".to_string(),
            url: "https://api.example.com/data".to_string(),
            method: "GET".to_string(),
            resource_type: "xhr".to_string(),
            status_code: 200,
            timestamp: 1234567890,
            duration_ms: Some(150),
            error: None,
        })
        .await;

    // Add failed requests
    router
        .add_network_request(NetworkRequest {
            request_id: "req-2".to_string(),
            url: "https://api.example.com/not-found".to_string(),
            method: "GET".to_string(),
            resource_type: "xhr".to_string(),
            status_code: 404,
            timestamp: 1234567891,
            duration_ms: Some(100),
            error: None,
        })
        .await;

    router
        .add_network_request(NetworkRequest {
            request_id: "req-3".to_string(),
            url: "https://api.example.com/error".to_string(),
            method: "GET".to_string(),
            resource_type: "xhr".to_string(),
            status_code: 500,
            timestamp: 1234567892,
            duration_ms: Some(50),
            error: Some("Internal Server Error".to_string()),
        })
        .await;

    let cmd = BrowserCommand::new(
        "network-failed-1",
        CommandType::NetworkGetFailed,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();
    let requests = result["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 2); // Only 404 and 500 requests
}

#[tokio::test]
async fn test_network_clear_requests() {
    let router = MockMonitoringRouter::new();

    // Add a request
    router
        .add_network_request(NetworkRequest {
            request_id: "req-1".to_string(),
            url: "https://example.com".to_string(),
            method: "GET".to_string(),
            resource_type: "document".to_string(),
            status_code: 200,
            timestamp: 1234567890,
            duration_ms: Some(100),
            error: None,
        })
        .await;

    // Clear
    let cmd = BrowserCommand::new(
        "network-clear-1",
        CommandType::NetworkClear,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    assert!(response.result.unwrap()["cleared"].as_bool().unwrap());

    // Verify cleared
    let cmd = BrowserCommand::new(
        "network-get-2",
        CommandType::NetworkGet,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();
    let result = response.result.unwrap();
    let requests = result["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 0);
}

#[tokio::test]
async fn test_network_status() {
    let router = MockMonitoringRouter::new();

    // Start monitoring
    let cmd = BrowserCommand::new(
        "network-start",
        CommandType::NetworkStart,
        serde_json::json!({}),
    );
    router.execute(cmd).await.unwrap();

    // Add requests
    router
        .add_network_request(NetworkRequest {
            request_id: "req-1".to_string(),
            url: "https://example.com/ok".to_string(),
            method: "GET".to_string(),
            resource_type: "xhr".to_string(),
            status_code: 200,
            timestamp: 1234567890,
            duration_ms: Some(100),
            error: None,
        })
        .await;

    router
        .add_network_request(NetworkRequest {
            request_id: "req-2".to_string(),
            url: "https://example.com/fail".to_string(),
            method: "GET".to_string(),
            resource_type: "xhr".to_string(),
            status_code: 500,
            timestamp: 1234567891,
            duration_ms: Some(50),
            error: None,
        })
        .await;

    // Check status
    let cmd = BrowserCommand::new(
        "network-status-1",
        CommandType::NetworkStatus,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();
    assert!(result["monitoring"].as_bool().unwrap());
    assert_eq!(result["request_count"].as_u64().unwrap(), 2);
    assert_eq!(result["failed_count"].as_u64().unwrap(), 1);
}

// ============================================================================
// Wave 7: E2E Browser Automation Tests
// ============================================================================

/// E2E test scenario: Full navigation and interaction flow
#[tokio::test]
async fn test_e2e_navigation_and_interaction_flow() {
    let router = MockBrowserRouter::connected();

    // Step 1: Navigate to page
    let cmd = BrowserCommand::navigate("e2e-1", "https://example.com/login");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);

    // Step 2: Fill username
    let cmd = BrowserCommand::fill("e2e-2", "#username", "testuser");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);

    // Step 3: Fill password
    let cmd = BrowserCommand::fill("e2e-3", "#password", "testpass");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);

    // Step 4: Click submit
    let cmd = BrowserCommand::click("e2e-4", "#login-button");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);

    // Verify all commands executed
    assert_eq!(router.command_count(), 4);
}

/// E2E test scenario: Form submission with validation
#[tokio::test]
async fn test_e2e_form_submission_flow() {
    let router = MockBrowserRouter::connected();
    let checker = BrowserPermissionChecker::new(DomainRules::default());

    // Check permission for form domain
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "forms.example.com".to_string(),
        })
        .await;
    assert!(decision.is_allowed());

    // Navigate to form page
    let cmd = BrowserCommand::navigate("form-1", "https://forms.example.com/contact");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);

    // Fill form fields
    let fields = vec![
        ("#name", "John Doe"),
        ("#email", "john@example.com"),
        ("#message", "Hello, this is a test message."),
    ];

    for (i, (selector, value)) in fields.iter().enumerate() {
        let cmd = BrowserCommand::fill(format!("fill-{}", i), *selector, *value);
        let response = router.execute(cmd).await.unwrap();
        assert!(response.success);
    }

    // Submit form
    let cmd = BrowserCommand::click("submit-1", "#submit-form");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);
}

/// E2E test scenario: Screenshot capture workflow
#[tokio::test]
async fn test_e2e_screenshot_workflow() {
    let router = MockBrowserRouter::connected();

    // Navigate to page
    let cmd = BrowserCommand::navigate("ss-1", "https://dashboard.example.com");
    router.execute(cmd).await.unwrap();

    // Wait for element to load
    let cmd = BrowserCommand::wait_for_element("ss-2", "#main-content");
    router.execute(cmd).await.unwrap();

    // Take full page screenshot
    let cmd = BrowserCommand::screenshot("ss-3");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);
    let result = response.result.unwrap();
    assert!(result["data"].as_str().is_some());

    // Take element screenshot
    let cmd = BrowserCommand::screenshot_element("ss-4", "#chart-container");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);
}

/// E2E test scenario: Multi-tab workflow
#[tokio::test]
async fn test_e2e_multi_tab_workflow() {
    let router = MockBrowserRouter::connected();

    // Navigate first tab
    let cmd = BrowserCommand::navigate("tab-1", "https://example.com/page1");
    router.execute(cmd).await.unwrap();

    // Navigate with new tab
    let cmd = BrowserCommand::navigate("tab-2", "https://example.com/page2").with_tab_id(2);
    router.execute(cmd).await.unwrap();

    // Interact with first tab
    let cmd = BrowserCommand::click("tab-1-click", "#btn-1").with_tab_id(1);
    router.execute(cmd).await.unwrap();

    // Interact with second tab
    let cmd = BrowserCommand::fill("tab-2-fill", "#input-1", "test").with_tab_id(2);
    router.execute(cmd).await.unwrap();
}

/// E2E test scenario: Error handling and recovery
#[tokio::test]
async fn test_e2e_error_handling() {
    let router = MockBrowserRouter::connected();

    // Set custom response for element not found
    router
        .set_response(
            "error-1",
            BrowserResponse::error_with_code("error-1", "Element not found", "ELEMENT_NOT_FOUND"),
        )
        .await;

    // Try to click non-existent element
    let cmd = BrowserCommand::click("error-1", "#non-existent");
    let response = router.execute(cmd).await.unwrap();

    assert!(!response.success);
    assert_eq!(response.error_code, Some("ELEMENT_NOT_FOUND".to_string()));

    // Verify router can still handle subsequent commands
    let cmd = BrowserCommand::navigate("recovery-1", "https://example.com");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);
}

/// E2E test scenario: Permission-aware browsing
#[tokio::test]
async fn test_e2e_permission_aware_browsing() {
    let router = MockBrowserRouter::connected();
    let rules = DomainRules::default()
        .with_blocked("*.bank.com")
        .with_confirm_required("github.com");
    let checker = BrowserPermissionChecker::new(rules);

    // Test 1: Allowed domain
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "docs.rust-lang.org".to_string(),
        })
        .await;
    assert!(decision.is_allowed());

    let cmd = BrowserCommand::navigate("perm-1", "https://docs.rust-lang.org");
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);

    // Test 2: Blocked domain - should not execute
    let decision = checker
        .check(BrowserPermission::Navigate {
            domain: "secure.bank.com".to_string(),
        })
        .await;
    assert!(decision.is_denied());
    // Don't execute command for blocked domain

    // Test 3: Confirmation required domain
    let decision = checker
        .check(BrowserPermission::Click {
            domain: "github.com".to_string(),
            element: "#merge-pr".to_string(),
        })
        .await;
    assert!(decision.requires_confirmation());
    // In real implementation, would prompt user before executing
}

/// E2E test scenario: Console and network monitoring during navigation
#[tokio::test]
async fn test_e2e_monitoring_during_navigation() {
    let router = MockMonitoringRouter::new();

    // Start console monitoring
    let cmd = BrowserCommand::new("mon-1", CommandType::ConsoleStart, serde_json::json!({}));
    router.execute(cmd).await.unwrap();

    // Start network monitoring
    let cmd = BrowserCommand::new("mon-2", CommandType::NetworkStart, serde_json::json!({}));
    router.execute(cmd).await.unwrap();

    // Simulate console messages during page load
    router
        .add_console_message(
            None,
            ConsoleMessage {
                level: "info".to_string(),
                text: "App initialized".to_string(),
                timestamp: 1234567890,
                source: Some("app.js".to_string()),
                line: Some(1),
            },
        )
        .await;

    router
        .add_console_message(
            None,
            ConsoleMessage {
                level: "warn".to_string(),
                text: "Deprecated API usage".to_string(),
                timestamp: 1234567891,
                source: Some("legacy.js".to_string()),
                line: Some(42),
            },
        )
        .await;

    // Simulate network requests
    router
        .add_network_request(NetworkRequest {
            request_id: "page-load".to_string(),
            url: "https://example.com/".to_string(),
            method: "GET".to_string(),
            resource_type: "document".to_string(),
            status_code: 200,
            timestamp: 1234567890,
            duration_ms: Some(250),
            error: None,
        })
        .await;

    router
        .add_network_request(NetworkRequest {
            request_id: "api-call".to_string(),
            url: "https://api.example.com/data".to_string(),
            method: "GET".to_string(),
            resource_type: "xhr".to_string(),
            status_code: 200,
            timestamp: 1234567891,
            duration_ms: Some(150),
            error: None,
        })
        .await;

    // Get console status
    let cmd = BrowserCommand::new(
        "status-1",
        CommandType::ConsoleStatus,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["message_count"].as_u64().unwrap(), 2);

    // Get network status
    let cmd = BrowserCommand::new(
        "status-2",
        CommandType::NetworkStatus,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["request_count"].as_u64().unwrap(), 2);

    // Clean up
    let cmd = BrowserCommand::new("stop-1", CommandType::ConsoleStop, serde_json::json!({}));
    router.execute(cmd).await.unwrap();

    let cmd = BrowserCommand::new("stop-2", CommandType::NetworkStop, serde_json::json!({}));
    router.execute(cmd).await.unwrap();
}

/// E2E test scenario: Debugging with network error detection
#[tokio::test]
async fn test_e2e_network_error_detection() {
    let router = MockMonitoringRouter::new();

    // Start network monitoring
    let cmd = BrowserCommand::new("debug-1", CommandType::NetworkStart, serde_json::json!({}));
    router.execute(cmd).await.unwrap();

    // Simulate mixed network activity
    router
        .add_network_request(NetworkRequest {
            request_id: "ok-1".to_string(),
            url: "https://api.example.com/users".to_string(),
            method: "GET".to_string(),
            resource_type: "xhr".to_string(),
            status_code: 200,
            timestamp: 1234567890,
            duration_ms: Some(100),
            error: None,
        })
        .await;

    router
        .add_network_request(NetworkRequest {
            request_id: "fail-1".to_string(),
            url: "https://api.example.com/auth".to_string(),
            method: "POST".to_string(),
            resource_type: "xhr".to_string(),
            status_code: 401,
            timestamp: 1234567891,
            duration_ms: Some(50),
            error: Some("Unauthorized".to_string()),
        })
        .await;

    router
        .add_network_request(NetworkRequest {
            request_id: "fail-2".to_string(),
            url: "https://api.example.com/resource".to_string(),
            method: "GET".to_string(),
            resource_type: "fetch".to_string(),
            status_code: 503,
            timestamp: 1234567892,
            duration_ms: Some(30),
            error: Some("Service Unavailable".to_string()),
        })
        .await;

    // Get failed requests for debugging
    let cmd = BrowserCommand::new(
        "debug-2",
        CommandType::NetworkGetFailed,
        serde_json::json!({}),
    );
    let response = router.execute(cmd).await.unwrap();

    let result = response.result.unwrap();
    let failed_requests = result["requests"].as_array().unwrap();
    assert_eq!(failed_requests.len(), 2);

    // Verify we can identify the issues
    let status_codes: Vec<u64> = failed_requests
        .iter()
        .map(|r| r["status_code"].as_u64().unwrap())
        .collect();
    assert!(status_codes.contains(&401));
    assert!(status_codes.contains(&503));
}
