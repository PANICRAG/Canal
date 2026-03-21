// Browser module removed (CV8: replaced by canal-cv)
#![cfg(feature = "browser-legacy-tests")]

//! Integration tests for browser accessibility and connection management features
//!
//! Tests for:
//! - ISSUE-002 fix: ExtensionManager concurrent registration race condition
//! - Accessibility tree support in computer_screenshot
//! - ComputerClickRefTool for ref-based clicking
//!
//! Run with: `cargo test --package gateway-core --test browser_accessibility_integration`

use gateway_core::browser::computer_use::{
    AccessibilityNode, AccessibilityTreeContext, BoundingBox, ScreenshotContext,
    SharedAccessibilityContext, SharedScreenshotContext,
};
use gateway_core::browser::{
    BrowserCommand, BrowserResponse, BrowserRouter, CommandType, ConnectionMode, ConnectionStatus,
    ExtensionAuthRequest, ExtensionManager,
};
use gateway_core::error::Result;

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::time::Duration;

// ============================================================================
// Mock Components
// ============================================================================

/// Mock browser router that supports accessibility tree in screenshots
pub struct MockAccessibilityRouter {
    connected: AtomicBool,
    command_count: AtomicU64,
    /// Whether to include accessibility tree in screenshot responses
    include_accessibility: AtomicBool,
}

impl MockAccessibilityRouter {
    pub fn new() -> Self {
        Self {
            connected: AtomicBool::new(true),
            command_count: AtomicU64::new(0),
            include_accessibility: AtomicBool::new(false),
        }
    }

    pub fn set_include_accessibility(&self, include: bool) {
        self.include_accessibility.store(include, Ordering::SeqCst);
    }

    pub fn command_count(&self) -> u64 {
        self.command_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BrowserRouter for MockAccessibilityRouter {
    async fn execute(&self, command: BrowserCommand) -> Result<BrowserResponse> {
        self.command_count.fetch_add(1, Ordering::SeqCst);

        if !self.connected.load(Ordering::SeqCst) {
            return Ok(BrowserResponse::error(&command.id, "Not connected"));
        }

        match command.command {
            CommandType::Screenshot => {
                let include_acc = self.include_accessibility.load(Ordering::SeqCst);

                let mut result = serde_json::json!({
                    "data": "base64_encoded_screenshot_data",
                    "width": 1280,
                    "height": 800,
                    "format": "jpeg",
                    "viewportWidth": 1920,
                    "viewportHeight": 1080,
                    "devicePixelRatio": 2.0
                });

                if include_acc {
                    result["accessibilityTree"] = serde_json::json!([
                        {
                            "ref": "@e1",
                            "role": "button",
                            "name": "Submit",
                            "bbox": {"x": 100, "y": 200, "width": 80, "height": 32}
                        },
                        {
                            "ref": "@e2",
                            "role": "textbox",
                            "name": "Email",
                            "value": "",
                            "bbox": {"x": 100, "y": 100, "width": 200, "height": 32}
                        },
                        {
                            "ref": "@e3",
                            "role": "link",
                            "name": "Forgot Password",
                            "bbox": {"x": 100, "y": 250, "width": 120, "height": 20}
                        },
                        {
                            "ref": "@e4",
                            "role": "checkbox",
                            "name": "Remember me",
                            "checked": false,
                            "bbox": {"x": 100, "y": 280, "width": 16, "height": 16}
                        }
                    ]);
                }

                Ok(BrowserResponse::success(&command.id, result))
            }
            CommandType::Click => {
                let x = command
                    .params
                    .get("x")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let y = command
                    .params
                    .get("y")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                Ok(BrowserResponse::success(
                    &command.id,
                    serde_json::json!({
                        "clicked": true,
                        "x": x,
                        "y": y
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

// ============================================================================
// ISSUE-002: ExtensionManager Concurrent Registration Tests
// ============================================================================

/// Test that registration lock prevents race condition
#[tokio::test]
async fn test_extension_manager_registration_lock() {
    let manager = ExtensionManager::new();

    // Create 5 concurrent registration attempts for the same extension
    let mut handles = vec![];

    for i in 0..5 {
        let (tx, _rx) = mpsc::channel(10);
        let auth_request = ExtensionAuthRequest {
            extension_id: "same-extension".to_string(),
            name: Some(format!("Connection {}", i)),
            version: None,
            auth_token: None,
        };

        let manager_ref = &manager;
        // Spawn all registration attempts concurrently
        handles.push(async move { manager_ref.authenticate(auth_request, tx, None).await });
    }

    // Execute all concurrently
    let results = futures::future::join_all(handles).await;

    // All should succeed (the lock ensures proper sequencing)
    for result in &results {
        assert!(result.success, "Registration should succeed");
    }

    // Only ONE connection should exist (last one wins due to deduplication)
    assert_eq!(
        manager.connection_count(),
        1,
        "Should only have one connection for same extension_id"
    );
}

/// Test that execute() prefers the most recently connected extension
#[tokio::test]
async fn test_extension_manager_execute_prefers_newest() {
    let manager = ExtensionManager::new();

    // Register first extension
    let (tx1, mut rx1) = mpsc::channel(10);
    let auth1 = ExtensionAuthRequest {
        extension_id: "ext-1".to_string(),
        name: Some("First Extension".to_string()),
        version: None,
        auth_token: None,
    };
    let result1 = manager.authenticate(auth1, tx1, None).await;
    assert!(result1.success);
    let conn_id_1 = result1.connection_id.unwrap();

    // Wait a bit
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Register second extension (should be preferred)
    let (tx2, mut rx2) = mpsc::channel(10);
    let auth2 = ExtensionAuthRequest {
        extension_id: "ext-2".to_string(),
        name: Some("Second Extension".to_string()),
        version: None,
        auth_token: None,
    };
    let result2 = manager.authenticate(auth2, tx2, None).await;
    assert!(result2.success);
    let conn_id_2 = result2.connection_id.unwrap();

    // Update heartbeat for the first one to be older
    manager.handle_heartbeat(&conn_id_1, 1000);
    manager.handle_heartbeat(&conn_id_2, 2000); // newer

    // Execute should prefer conn_id_2 (newer heartbeat)
    let manager_arc = Arc::new(manager);
    let manager_for_exec = manager_arc.clone();

    let exec_handle = tokio::spawn(async move {
        let cmd = BrowserCommand::screenshot("test-cmd");
        manager_for_exec.execute(cmd).await
    });

    // Check which connection received the command
    tokio::time::sleep(Duration::from_millis(50)).await;

    let _rx1_result = rx1.try_recv();
    let rx2_result = rx2.try_recv();

    // ext-2 (newer) should receive the command, not ext-1
    assert!(
        rx2_result.is_ok(),
        "Newer connection should receive the command"
    );

    // Clean up
    exec_handle.abort();
}

/// Test that reconnect replaces old connection properly
#[tokio::test]
async fn test_extension_manager_reconnect_replaces() {
    let manager = ExtensionManager::new();

    // First connection
    let (tx1, _rx1) = mpsc::channel(10);
    let auth1 = ExtensionAuthRequest {
        extension_id: "reconnect-ext".to_string(),
        name: Some("First".to_string()),
        version: Some("1.0".to_string()),
        auth_token: None,
    };
    let result1 = manager.authenticate(auth1, tx1, None).await;
    assert!(result1.success);

    // Second connection with same extension ID
    let (tx2, _rx2) = mpsc::channel(10);
    let auth2 = ExtensionAuthRequest {
        extension_id: "reconnect-ext".to_string(),
        name: Some("Second".to_string()),
        version: Some("1.1".to_string()),
        auth_token: None,
    };
    let result2 = manager.authenticate(auth2, tx2, None).await;
    assert!(result2.success);

    // Should only have one connection
    assert_eq!(manager.connection_count(), 1);

    // The connection should have the new info
    let info = manager.get_extension("reconnect-ext").unwrap();
    assert_eq!(info.name, Some("Second".to_string()));
    assert_eq!(info.version, Some("1.1".to_string()));
}

// ============================================================================
// Accessibility Tree Context Tests
// ============================================================================

/// Test AccessibilityTreeContext creation and lookup
#[test]
fn test_accessibility_tree_context_creation() {
    let nodes = vec![
        AccessibilityNode {
            r#ref: "@e1".to_string(),
            role: "button".to_string(),
            name: Some("Submit".to_string()),
            value: None,
            checked: None,
            disabled: None,
            expanded: None,
            focused: None,
            level: None,
            bbox: Some(BoundingBox {
                x: 100,
                y: 200,
                width: 80,
                height: 32,
            }),
            selector: None,
            children: None,
        },
        AccessibilityNode {
            r#ref: "@e2".to_string(),
            role: "textbox".to_string(),
            name: Some("Email".to_string()),
            value: Some("".to_string()),
            checked: None,
            disabled: None,
            expanded: None,
            focused: None,
            level: None,
            bbox: Some(BoundingBox {
                x: 100,
                y: 100,
                width: 200,
                height: 32,
            }),
            selector: None,
            children: None,
        },
    ];

    let ctx = AccessibilityTreeContext::new(nodes, Some(1));

    assert_eq!(ctx.nodes.len(), 2);
    assert!(ctx.tab_id == Some(1));
    assert!(!ctx.is_stale());
}

/// Test finding element by ref
#[test]
fn test_accessibility_tree_find_by_ref() {
    let nodes = vec![
        AccessibilityNode {
            r#ref: "@e1".to_string(),
            role: "button".to_string(),
            name: Some("Submit".to_string()),
            value: None,
            checked: None,
            disabled: None,
            expanded: None,
            focused: None,
            level: None,
            bbox: Some(BoundingBox {
                x: 100,
                y: 200,
                width: 80,
                height: 32,
            }),
            selector: None,
            children: None,
        },
        AccessibilityNode {
            r#ref: "@e2".to_string(),
            role: "link".to_string(),
            name: Some("Help".to_string()),
            value: None,
            checked: None,
            disabled: None,
            expanded: None,
            focused: None,
            level: None,
            bbox: Some(BoundingBox {
                x: 300,
                y: 400,
                width: 50,
                height: 20,
            }),
            selector: None,
            children: None,
        },
    ];

    let ctx = AccessibilityTreeContext::new(nodes, None);

    // Find existing element
    let node = ctx.find_by_ref("@e1");
    assert!(node.is_some());
    let node = node.unwrap();
    assert_eq!(node.role, "button");
    assert_eq!(node.name, Some("Submit".to_string()));

    // Find non-existing element
    let node = ctx.find_by_ref("@e99");
    assert!(node.is_none());
}

/// Test bounding box center calculation
#[test]
fn test_bounding_box_center() {
    let bbox = BoundingBox {
        x: 100,
        y: 200,
        width: 80,
        height: 32,
    };

    let (cx, cy) = bbox.center();
    assert_eq!(cx, 140); // 100 + 80/2
    assert_eq!(cy, 216); // 200 + 32/2
}

/// Test flattening nested accessibility nodes
#[test]
fn test_accessibility_tree_flatten_nested() {
    let nodes = vec![AccessibilityNode {
        r#ref: "@e1".to_string(),
        role: "navigation".to_string(),
        name: Some("Main Nav".to_string()),
        value: None,
        checked: None,
        disabled: None,
        expanded: None,
        focused: None,
        level: None,
        bbox: None,
        selector: None,
        children: Some(vec![
            AccessibilityNode {
                r#ref: "@e2".to_string(),
                role: "link".to_string(),
                name: Some("Home".to_string()),
                value: None,
                checked: None,
                disabled: None,
                expanded: None,
                focused: None,
                level: None,
                bbox: Some(BoundingBox {
                    x: 10,
                    y: 10,
                    width: 50,
                    height: 20,
                }),
                selector: None,
                children: None,
            },
            AccessibilityNode {
                r#ref: "@e3".to_string(),
                role: "link".to_string(),
                name: Some("About".to_string()),
                value: None,
                checked: None,
                disabled: None,
                expanded: None,
                focused: None,
                level: None,
                bbox: Some(BoundingBox {
                    x: 70,
                    y: 10,
                    width: 50,
                    height: 20,
                }),
                selector: None,
                children: None,
            },
        ]),
    }];

    let ctx = AccessibilityTreeContext::new(nodes, None);

    // Should have all 3 nodes flattened
    assert_eq!(ctx.nodes.len(), 3);

    // All refs should be findable
    assert!(ctx.find_by_ref("@e1").is_some());
    assert!(ctx.find_by_ref("@e2").is_some());
    assert!(ctx.find_by_ref("@e3").is_some());
}

// ============================================================================
// Screenshot Context Coordinate Transformation Tests
// ============================================================================

/// Test coordinate transformation from image to CSS pixels
#[test]
fn test_screenshot_context_coordinate_transformation() {
    let ctx = ScreenshotContext::new(
        800,  // image_width (compressed)
        600,  // image_height (compressed)
        1600, // viewport_width (CSS pixels)
        1200, // viewport_height (CSS pixels)
        2.0,  // device_pixel_ratio
        Some(1),
    );

    // Image coordinate (400, 300) should map to CSS (800, 600)
    let (css_x, css_y) = ctx.image_to_css(400, 300);
    assert_eq!(css_x, 800);
    assert_eq!(css_y, 600);

    // Edge coordinate (0, 0) should stay (0, 0)
    let (css_x, css_y) = ctx.image_to_css(0, 0);
    assert_eq!(css_x, 0);
    assert_eq!(css_y, 0);

    // Max coordinate (800, 600) should map to (1600, 1200)
    let (css_x, css_y) = ctx.image_to_css(800, 600);
    assert_eq!(css_x, 1600);
    assert_eq!(css_y, 1200);
}

/// Test that context detects when transformation is needed
#[test]
fn test_screenshot_context_needs_transformation() {
    // Compressed screenshot - needs transformation
    let ctx1 = ScreenshotContext::new(800, 600, 1600, 1200, 2.0, None);
    assert!(ctx1.needs_transformation());

    // Same size - no transformation needed
    let ctx2 = ScreenshotContext::new(1600, 1200, 1600, 1200, 1.0, None);
    assert!(!ctx2.needs_transformation());
}

/// Test screenshot context staleness
#[tokio::test]
async fn test_screenshot_context_staleness() {
    let ctx = ScreenshotContext::default();

    // Fresh context should not be stale
    assert!(!ctx.is_stale());

    // Note: We can't easily test staleness timeout without mocking time
    // In a real scenario, contexts older than 5 minutes are considered stale
}

// ============================================================================
// Integration Tests: Screenshot with Accessibility Tree
// ============================================================================

/// Test screenshot response includes accessibility tree when requested
#[tokio::test]
async fn test_screenshot_with_accessibility_tree() {
    let router = MockAccessibilityRouter::new();
    router.set_include_accessibility(true);

    let cmd = BrowserCommand::new(
        "screenshot-with-acc",
        CommandType::Screenshot,
        serde_json::json!({
            "includeAccessibility": true,
            "interactiveOnly": true
        }),
    );

    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);

    let result = response.result.unwrap();

    // Should have accessibility tree
    assert!(result.get("accessibilityTree").is_some());
    let tree = result["accessibilityTree"].as_array().unwrap();
    assert!(!tree.is_empty());

    // Check first element
    let first = &tree[0];
    assert_eq!(first["ref"], "@e1");
    assert_eq!(first["role"], "button");
    assert_eq!(first["name"], "Submit");

    // Check bbox exists
    let bbox = first["bbox"].as_object().unwrap();
    assert_eq!(bbox["x"], 100);
    assert_eq!(bbox["y"], 200);
}

/// Test screenshot response without accessibility tree (default)
#[tokio::test]
async fn test_screenshot_without_accessibility_tree() {
    let router = MockAccessibilityRouter::new();
    // Don't set include_accessibility

    let cmd = BrowserCommand::screenshot("screenshot-no-acc");
    let response = router.execute(cmd).await.unwrap();

    assert!(response.success);
    let result = response.result.unwrap();

    // Should NOT have accessibility tree
    assert!(result.get("accessibilityTree").is_none());

    // Should still have screenshot data
    assert!(result.get("data").is_some());
    assert_eq!(result["width"], 1280);
    assert_eq!(result["height"], 800);
}

// ============================================================================
// Integration Tests: Ref-Based Clicking Flow
// ============================================================================

/// Test the full ref-based clicking workflow
#[tokio::test]
async fn test_ref_based_clicking_workflow() {
    let router = Arc::new(MockAccessibilityRouter::new());
    router.set_include_accessibility(true);

    // Step 1: Take screenshot with accessibility tree
    let cmd = BrowserCommand::new(
        "screenshot-1",
        CommandType::Screenshot,
        serde_json::json!({ "includeAccessibility": true }),
    );
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);

    let result = response.result.unwrap();
    let tree = result["accessibilityTree"].as_array().unwrap();

    // Step 2: Parse accessibility tree and create context
    let nodes: Vec<AccessibilityNode> = tree
        .iter()
        .map(|v| AccessibilityNode {
            r#ref: v["ref"].as_str().unwrap().to_string(),
            role: v["role"].as_str().unwrap().to_string(),
            name: v["name"].as_str().map(|s| s.to_string()),
            value: v
                .get("value")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            checked: v.get("checked").and_then(|v| v.as_bool()),
            disabled: None,
            expanded: None,
            focused: None,
            level: None,
            bbox: v.get("bbox").map(|b| BoundingBox {
                x: b["x"].as_i64().unwrap() as i32,
                y: b["y"].as_i64().unwrap() as i32,
                width: b["width"].as_i64().unwrap() as i32,
                height: b["height"].as_i64().unwrap() as i32,
            }),
            selector: None,
            children: None,
        })
        .collect();

    let ctx = AccessibilityTreeContext::new(nodes, None);

    // Step 3: Find element by ref and get click coordinates
    let button = ctx.find_by_ref("@e1").expect("Should find @e1");
    assert_eq!(button.role, "button");
    assert_eq!(button.name, Some("Submit".to_string()));

    let bbox = button.bbox.as_ref().expect("Should have bbox");
    let (click_x, click_y) = bbox.center();

    assert_eq!(click_x, 140); // 100 + 80/2
    assert_eq!(click_y, 216); // 200 + 32/2

    // Step 4: Execute click at bbox center
    let cmd = BrowserCommand::new(
        "click-1",
        CommandType::Click,
        serde_json::json!({
            "x": click_x,
            "y": click_y,
            "button": "left"
        }),
    );
    let response = router.execute(cmd).await.unwrap();
    assert!(response.success);

    let result = response.result.unwrap();
    assert!(result["clicked"].as_bool().unwrap());
    assert_eq!(result["x"], 140);
    assert_eq!(result["y"], 216);
}

/// Test clicking different element types
#[tokio::test]
async fn test_click_different_element_types() {
    let nodes = vec![
        AccessibilityNode {
            r#ref: "@e1".to_string(),
            role: "button".to_string(),
            name: Some("Submit".to_string()),
            value: None,
            checked: None,
            disabled: None,
            expanded: None,
            focused: None,
            level: None,
            bbox: Some(BoundingBox {
                x: 100,
                y: 200,
                width: 80,
                height: 32,
            }),
            selector: None,
            children: None,
        },
        AccessibilityNode {
            r#ref: "@e2".to_string(),
            role: "textbox".to_string(),
            name: Some("Email".to_string()),
            value: Some("".to_string()),
            checked: None,
            disabled: None,
            expanded: None,
            focused: None,
            level: None,
            bbox: Some(BoundingBox {
                x: 100,
                y: 100,
                width: 200,
                height: 32,
            }),
            selector: None,
            children: None,
        },
        AccessibilityNode {
            r#ref: "@e3".to_string(),
            role: "checkbox".to_string(),
            name: Some("Remember me".to_string()),
            value: None,
            checked: Some(false),
            disabled: None,
            expanded: None,
            focused: None,
            level: None,
            bbox: Some(BoundingBox {
                x: 100,
                y: 280,
                width: 16,
                height: 16,
            }),
            selector: None,
            children: None,
        },
    ];

    let ctx = AccessibilityTreeContext::new(nodes, None);

    // Test button click coordinates
    let button = ctx.find_by_ref("@e1").unwrap();
    let (bx, by) = button.bbox.as_ref().unwrap().center();
    assert_eq!(bx, 140);
    assert_eq!(by, 216);

    // Test textbox click coordinates
    let textbox = ctx.find_by_ref("@e2").unwrap();
    let (tx, ty) = textbox.bbox.as_ref().unwrap().center();
    assert_eq!(tx, 200); // 100 + 200/2
    assert_eq!(ty, 116); // 100 + 32/2

    // Test checkbox click coordinates
    let checkbox = ctx.find_by_ref("@e3").unwrap();
    assert_eq!(checkbox.checked, Some(false));
    let (cx, cy) = checkbox.bbox.as_ref().unwrap().center();
    assert_eq!(cx, 108); // 100 + 16/2
    assert_eq!(cy, 288); // 280 + 16/2
}

// ============================================================================
// Error Handling Tests
// ============================================================================

/// Test handling of missing bbox
#[test]
fn test_element_without_bbox() {
    let nodes = vec![AccessibilityNode {
        r#ref: "@e1".to_string(),
        role: "heading".to_string(),
        name: Some("Page Title".to_string()),
        value: None,
        checked: None,
        disabled: None,
        expanded: None,
        focused: None,
        level: Some(1),
        bbox: None, // No bbox for this element
        selector: None,
        children: None,
    }];

    let ctx = AccessibilityTreeContext::new(nodes, None);
    let node = ctx.find_by_ref("@e1").unwrap();

    assert!(node.bbox.is_none());
    // In real implementation, should return error "Element has no bounding box"
}

/// Test handling of invalid ref format
#[test]
fn test_invalid_ref_lookup() {
    let nodes = vec![AccessibilityNode {
        r#ref: "@e1".to_string(),
        role: "button".to_string(),
        name: Some("Submit".to_string()),
        value: None,
        checked: None,
        disabled: None,
        expanded: None,
        focused: None,
        level: None,
        bbox: Some(BoundingBox {
            x: 100,
            y: 200,
            width: 80,
            height: 32,
        }),
        selector: None,
        children: None,
    }];

    let ctx = AccessibilityTreeContext::new(nodes, None);

    // Invalid ref formats should return None
    assert!(ctx.find_by_ref("e1").is_none()); // Missing @
    assert!(ctx.find_by_ref("@e999").is_none()); // Non-existent
    assert!(ctx.find_by_ref("").is_none()); // Empty
    assert!(ctx.find_by_ref("button").is_none()); // Not a ref
}

// ============================================================================
// Token Efficiency Tests
// ============================================================================

/// Verify accessibility tree format is compact for LLM consumption
#[test]
fn test_accessibility_tree_compact_format() {
    let node = AccessibilityNode {
        r#ref: "@e1".to_string(),
        role: "button".to_string(),
        name: Some("Submit".to_string()),
        value: None,    // Should be skipped in serialization
        checked: None,  // Should be skipped
        disabled: None, // Should be skipped
        expanded: None, // Should be skipped
        focused: None,  // Should be skipped
        level: None,    // Should be skipped
        bbox: Some(BoundingBox {
            x: 100,
            y: 200,
            width: 80,
            height: 32,
        }),
        selector: None, // Should be skipped
        children: None, // Should be skipped
    };

    let json = serde_json::to_string(&node).unwrap();

    // Verify skipped fields are not in output
    assert!(!json.contains("\"value\""));
    assert!(!json.contains("\"checked\""));
    assert!(!json.contains("\"disabled\""));
    assert!(!json.contains("\"expanded\""));
    assert!(!json.contains("\"focused\""));
    assert!(!json.contains("\"level\""));
    assert!(!json.contains("\"children\""));

    // Verify required fields are present
    assert!(json.contains("\"ref\":\"@e1\""));
    assert!(json.contains("\"role\":\"button\""));
    assert!(json.contains("\"name\":\"Submit\""));
    assert!(json.contains("\"bbox\""));
}

// ============================================================================
// Concurrent Access Tests
// ============================================================================

/// Test thread-safe access to shared accessibility context
#[tokio::test]
async fn test_shared_accessibility_context_concurrent_access() {
    let ctx: SharedAccessibilityContext =
        Arc::new(RwLock::new(AccessibilityTreeContext::default()));

    // Multiple readers
    let mut read_handles = vec![];
    for _ in 0..10 {
        let ctx_clone = ctx.clone();
        read_handles.push(tokio::spawn(async move {
            let guard = ctx_clone.read().await;
            guard.is_stale()
        }));
    }

    let results: Vec<bool> = futures::future::join_all(read_handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // All reads should succeed
    assert_eq!(results.len(), 10);

    // Write new context
    {
        let mut guard = ctx.write().await;
        *guard = AccessibilityTreeContext::new(
            vec![AccessibilityNode {
                r#ref: "@e1".to_string(),
                role: "button".to_string(),
                name: Some("New".to_string()),
                value: None,
                checked: None,
                disabled: None,
                expanded: None,
                focused: None,
                level: None,
                bbox: None,
                selector: None,
                children: None,
            }],
            Some(42),
        );
    }

    // Verify write succeeded
    let guard = ctx.read().await;
    assert_eq!(guard.nodes.len(), 1);
    assert_eq!(guard.tab_id, Some(42));
}

/// Test thread-safe access to shared screenshot context
#[tokio::test]
async fn test_shared_screenshot_context_concurrent_access() {
    let ctx: SharedScreenshotContext = Arc::new(RwLock::new(ScreenshotContext::default()));

    // Multiple coordinate transformations concurrently
    let mut handles = vec![];
    for i in 0..10 {
        let ctx_clone = ctx.clone();
        handles.push(tokio::spawn(async move {
            let guard = ctx_clone.read().await;
            guard.image_to_css(i * 10, i * 10)
        }));
    }

    let results: Vec<(u32, u32)> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    // All transformations should complete
    assert_eq!(results.len(), 10);

    // Update context
    {
        let mut guard = ctx.write().await;
        *guard = ScreenshotContext::new(800, 600, 1600, 1200, 2.0, Some(1));
    }

    // Verify new transformation scale
    let guard = ctx.read().await;
    let (x, y) = guard.image_to_css(400, 300);
    assert_eq!(x, 800);
    assert_eq!(y, 600);
}
