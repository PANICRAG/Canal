//! A27 MCP Remote Server Integration Tests
//!
//! Comprehensive tests for the MCP server lifecycle:
//! - McpRefTracker + McpConnectionTracker cross-module interactions
//! - BundleDefinition .mcp.json loading + ref tracking wiring
//! - RuntimeRegistry all_active_bundles → startup reconnect simulation
//! - Full activate → connect → deactivate → cleanup lifecycle
//! - Shared server reference counting across multiple bundles
//! - Concurrent bundle activation safety
//! - Auth token pass-through from bundle to MCP config
//! - McpConnectionStatus serialization for API responses

use gateway_core::connectors::{
    BundleLoader, BundleManager, McpConnectionStatus, McpConnectionTracker, McpRefTracker,
    McpServerDef, RuntimeRegistry,
};
use gateway_core::mcp::gateway::{McpGateway, McpServerConfig, McpTransport};
use std::collections::HashSet;
use std::sync::Arc;
use tempfile::TempDir;

// ============================================================================
// Test Helpers
// ============================================================================

/// Create a test bundle directory with optional .mcp.json.
fn create_bundle_with_mcp(
    dir: &std::path::Path,
    name: &str,
    mcp_servers: &[(&str, &str)], // (server_name, url)
) {
    let bundle_dir = dir.join(name);
    std::fs::create_dir_all(&bundle_dir).unwrap();

    let def = serde_json::json!({
        "name": name,
        "version": "1.0.0",
        "description": format!("{} bundle", name),
        "required_categories": [],
    });
    std::fs::write(
        bundle_dir.join("plugin.json"),
        serde_json::to_string_pretty(&def).unwrap(),
    )
    .unwrap();

    if !mcp_servers.is_empty() {
        let mut servers = serde_json::Map::new();
        for (sname, url) in mcp_servers {
            servers.insert(
                sname.to_string(),
                serde_json::json!({"type": "http", "url": url}),
            );
        }
        let mcp = serde_json::json!({"mcpServers": servers});
        std::fs::write(
            bundle_dir.join(".mcp.json"),
            serde_json::to_string_pretty(&mcp).unwrap(),
        )
        .unwrap();
    }
}

/// Create a test bundle with auth tokens on MCP servers.
fn create_bundle_with_auth_mcp(
    dir: &std::path::Path,
    name: &str,
    mcp_servers: &[(&str, &str, Option<&str>)], // (server_name, url, auth_token)
) {
    let bundle_dir = dir.join(name);
    std::fs::create_dir_all(&bundle_dir).unwrap();

    let def = serde_json::json!({
        "name": name,
        "version": "1.0.0",
        "description": format!("{} bundle", name),
        "required_categories": [],
    });
    std::fs::write(
        bundle_dir.join("plugin.json"),
        serde_json::to_string_pretty(&def).unwrap(),
    )
    .unwrap();

    let mut servers = serde_json::Map::new();
    for (sname, url, token) in mcp_servers {
        let mut entry = serde_json::json!({"type": "http", "url": url});
        if let Some(t) = token {
            entry["auth_token"] = serde_json::json!(t);
        }
        servers.insert(sname.to_string(), entry);
    }
    let mcp = serde_json::json!({"mcpServers": servers});
    std::fs::write(
        bundle_dir.join(".mcp.json"),
        serde_json::to_string_pretty(&mcp).unwrap(),
    )
    .unwrap();
}

// ============================================================================
// 1. Integration: RefTracker + ConnectionTracker cross-module
// ============================================================================

#[test]
fn test_ref_tracker_and_connection_tracker_lifecycle() {
    let ref_tracker = McpRefTracker::new();
    let conn_tracker = McpConnectionTracker::new();

    // Simulate: activate "productivity" bundle with slack + notion
    let servers = vec!["slack", "notion"];
    for server in &servers {
        let is_first = ref_tracker.add_reference(server, "productivity");
        assert!(is_first, "First reference should return true");
        conn_tracker.set_status(server, McpConnectionStatus::Connecting);
    }

    // Simulate connections completing
    conn_tracker.set_status("slack", McpConnectionStatus::Connected);
    conn_tracker.set_status("notion", McpConnectionStatus::Failed("timeout".to_string()));

    // Verify statuses
    assert_eq!(conn_tracker.get_status("slack").label(), "connected");
    assert_eq!(conn_tracker.get_status("notion").label(), "failed");
    assert!(conn_tracker.get_status("notion").error_message().is_some());

    // Simulate: deactivate "productivity"
    let orphaned = ref_tracker.remove_bundle("productivity");
    assert_eq!(orphaned.len(), 2);

    for server_name in &orphaned {
        conn_tracker.remove(server_name);
    }

    // Both should be disconnected now
    assert_eq!(conn_tracker.get_status("slack").label(), "disconnected");
    assert_eq!(conn_tracker.get_status("notion").label(), "disconnected");
    assert_eq!(conn_tracker.all_statuses().len(), 0);
}

#[test]
fn test_shared_server_lifecycle_across_bundles() {
    let ref_tracker = McpRefTracker::new();
    let conn_tracker = McpConnectionTracker::new();

    // Activate "productivity" (slack, notion)
    assert!(ref_tracker.add_reference("slack", "productivity"));
    assert!(ref_tracker.add_reference("notion", "productivity"));
    conn_tracker.set_status("slack", McpConnectionStatus::Connected);
    conn_tracker.set_status("notion", McpConnectionStatus::Connected);

    // Activate "sales" (slack, hubspot) — slack is shared
    assert!(
        !ref_tracker.add_reference("slack", "sales"),
        "Second ref to slack should return false"
    );
    assert!(ref_tracker.add_reference("hubspot", "sales"));
    conn_tracker.set_status("hubspot", McpConnectionStatus::Connected);

    // Verify ref counts
    assert_eq!(ref_tracker.ref_count("slack"), 2);
    assert_eq!(ref_tracker.ref_count("notion"), 1);
    assert_eq!(ref_tracker.ref_count("hubspot"), 1);

    // Deactivate "productivity" — notion orphaned, slack survives
    let orphaned = ref_tracker.remove_bundle("productivity");
    assert_eq!(orphaned.len(), 1);
    assert_eq!(orphaned[0], "notion");

    // Clean up orphaned
    for s in &orphaned {
        conn_tracker.remove(s);
    }

    // slack still connected, notion cleaned up
    assert_eq!(conn_tracker.get_status("slack").label(), "connected");
    assert_eq!(conn_tracker.get_status("notion").label(), "disconnected");
    assert_eq!(ref_tracker.ref_count("slack"), 1);

    // Deactivate "sales" — everything orphaned
    let orphaned = ref_tracker.remove_bundle("sales");
    assert_eq!(orphaned.len(), 2);
    let mut orphaned_sorted = orphaned.clone();
    orphaned_sorted.sort();
    assert_eq!(orphaned_sorted, vec!["hubspot", "slack"]);

    for s in &orphaned {
        conn_tracker.remove(s);
    }
    assert_eq!(conn_tracker.all_statuses().len(), 0);
}

// ============================================================================
// 2. Integration: BundleLoader + RefTracker (real filesystem)
// ============================================================================

#[test]
fn test_bundle_load_and_ref_track_from_filesystem() {
    let tmp = TempDir::new().unwrap();

    // Create productivity bundle (slack + notion)
    create_bundle_with_mcp(
        tmp.path(),
        "productivity",
        &[
            ("slack", "https://mcp.slack.com/mcp"),
            ("notion", "https://mcp.notion.com/mcp"),
        ],
    );

    // Create sales bundle (slack + hubspot) — slack is shared
    create_bundle_with_mcp(
        tmp.path(),
        "sales",
        &[
            ("slack", "https://mcp.slack.com/mcp"),
            ("hubspot", "https://mcp.hubspot.com/mcp"),
        ],
    );

    // Load bundles
    let mut manager = BundleManager::new(vec![tmp.path().to_path_buf()]);
    manager.discover();

    let productivity = manager.get("productivity").unwrap();
    assert_eq!(productivity.mcp_servers.len(), 2);

    let sales = manager.get("sales").unwrap();
    assert_eq!(sales.mcp_servers.len(), 2);

    // Simulate activation with ref tracker
    let ref_tracker = McpRefTracker::new();
    let mut first_connects: Vec<String> = Vec::new();

    // Activate productivity
    for server in &productivity.mcp_servers {
        if ref_tracker.add_reference(&server.name, "productivity") {
            first_connects.push(server.name.clone());
        }
    }
    first_connects.sort();
    assert_eq!(first_connects, vec!["notion", "slack"]);

    // Activate sales — only hubspot is new
    first_connects.clear();
    for server in &sales.mcp_servers {
        if ref_tracker.add_reference(&server.name, "sales") {
            first_connects.push(server.name.clone());
        }
    }
    assert_eq!(first_connects, vec!["hubspot"]);
}

#[test]
fn test_bundle_with_no_mcp_json() {
    let tmp = TempDir::new().unwrap();

    // Bundle without .mcp.json
    let bundle_dir = tmp.path().join("simple");
    std::fs::create_dir_all(&bundle_dir).unwrap();
    std::fs::write(
        bundle_dir.join("plugin.json"),
        serde_json::to_string(&serde_json::json!({
            "name": "simple",
            "version": "1.0.0",
            "description": "Simple bundle",
            "required_categories": [],
        }))
        .unwrap(),
    )
    .unwrap();

    let mut manager = BundleManager::new(vec![tmp.path().to_path_buf()]);
    manager.discover();

    let bundle = manager.get("simple").unwrap();
    assert!(bundle.mcp_servers.is_empty());
}

// ============================================================================
// 3. Integration: RuntimeRegistry + RefTracker (startup reconnect)
// ============================================================================

#[tokio::test]
async fn test_startup_reconnect_simulation() {
    let tmp = TempDir::new().unwrap();

    // Create bundles on disk
    create_bundle_with_mcp(
        tmp.path(),
        "productivity",
        &[
            ("slack", "https://mcp.slack.com/mcp"),
            ("notion", "https://mcp.notion.com/mcp"),
        ],
    );
    create_bundle_with_mcp(
        tmp.path(),
        "sales",
        &[
            ("slack", "https://mcp.slack.com/mcp"),
            ("hubspot", "https://mcp.hubspot.com/mcp"),
        ],
    );

    // Setup runtime registry with active bundles (simulates persisted state)
    let activation_path = tmp.path().join("data").join("activations.json");
    let mut runtime = RuntimeRegistry::new(activation_path.clone());
    runtime
        .activate_bundle("user1", "productivity", "1.0.0")
        .await
        .unwrap();
    runtime
        .activate_bundle("user1", "sales", "1.0.0")
        .await
        .unwrap();

    // Reload from disk (simulates server restart)
    let runtime = RuntimeRegistry::load(activation_path).unwrap();
    let all_active = runtime.all_active_bundles();
    assert_eq!(all_active.len(), 1); // 1 user
    assert_eq!(all_active[0].1.len(), 2); // 2 bundles

    // Load bundle definitions
    let mut bundle_mgr = BundleManager::new(vec![tmp.path().to_path_buf()]);
    bundle_mgr.discover();

    // Rebuild ref tracker from persisted activations (simulates startup reconnect)
    let ref_tracker = McpRefTracker::new();
    let conn_tracker = McpConnectionTracker::new();

    let mut unique_servers: std::collections::HashMap<String, McpServerDef> =
        std::collections::HashMap::new();
    let mut rebuild_data: Vec<(String, Vec<String>)> = Vec::new();

    for (_user_id, bundle_names) in &all_active {
        for bundle_name in bundle_names {
            if let Some(bundle) = bundle_mgr.get(bundle_name) {
                let server_names: Vec<String> =
                    bundle.mcp_servers.iter().map(|s| s.name.clone()).collect();
                rebuild_data.push((bundle_name.clone(), server_names));
                for server_def in &bundle.mcp_servers {
                    unique_servers
                        .entry(server_def.name.clone())
                        .or_insert_with(|| server_def.clone());
                }
            }
        }
    }

    ref_tracker.rebuild(&rebuild_data);

    // Verify: 3 unique servers (slack deduplicated)
    assert_eq!(unique_servers.len(), 3);
    assert!(unique_servers.contains_key("slack"));
    assert!(unique_servers.contains_key("notion"));
    assert!(unique_servers.contains_key("hubspot"));

    // Verify ref counts after rebuild
    assert_eq!(ref_tracker.ref_count("slack"), 2); // shared between productivity + sales
    assert_eq!(ref_tracker.ref_count("notion"), 1);
    assert_eq!(ref_tracker.ref_count("hubspot"), 1);

    // Simulate connection attempts
    for (name, _) in &unique_servers {
        conn_tracker.set_status(name, McpConnectionStatus::Connecting);
    }
    for (name, _) in &unique_servers {
        conn_tracker.set_status(name, McpConnectionStatus::Connected);
    }

    // All connected
    let all = conn_tracker.all_statuses();
    assert_eq!(all.len(), 3);
    for (_, status) in &all {
        assert_eq!(status.label(), "connected");
    }
}

#[tokio::test]
async fn test_startup_reconnect_partial_failure() {
    let ref_tracker = McpRefTracker::new();
    let conn_tracker = McpConnectionTracker::new();

    // Simulate 3 servers to reconnect, one fails
    let servers = vec!["slack", "notion", "hubspot"];
    for s in &servers {
        ref_tracker.add_reference(s, "test-bundle");
        conn_tracker.set_status(s, McpConnectionStatus::Connecting);
    }

    // Simulate results: 2 succeed, 1 fails
    conn_tracker.set_status("slack", McpConnectionStatus::Connected);
    conn_tracker.set_status("notion", McpConnectionStatus::Connected);
    conn_tracker.set_status(
        "hubspot",
        McpConnectionStatus::Failed("connection refused".to_string()),
    );

    // Verify: partial success doesn't affect other servers
    assert_eq!(conn_tracker.get_status("slack").label(), "connected");
    assert_eq!(conn_tracker.get_status("notion").label(), "connected");
    assert_eq!(conn_tracker.get_status("hubspot").label(), "failed");
    assert_eq!(
        conn_tracker.get_status("hubspot").error_message(),
        Some("connection refused")
    );

    // Ref counts unaffected by connection failure
    assert_eq!(ref_tracker.ref_count("slack"), 1);
    assert_eq!(ref_tracker.ref_count("hubspot"), 1);
}

// ============================================================================
// 4. Auth Token Pass-Through
// ============================================================================

#[test]
fn test_auth_token_parsed_from_mcp_json() {
    let tmp = TempDir::new().unwrap();

    create_bundle_with_auth_mcp(
        tmp.path(),
        "auth-test",
        &[
            (
                "slack",
                "https://mcp.slack.com/mcp",
                Some("slack-token-123"),
            ),
            ("notion", "https://mcp.notion.com/mcp", None),
            (
                "linear",
                "https://mcp.linear.app/mcp",
                Some("linear-token-456"),
            ),
        ],
    );

    let mut manager = BundleManager::new(vec![tmp.path().to_path_buf()]);
    manager.discover();

    let bundle = manager.get("auth-test").unwrap();
    assert_eq!(bundle.mcp_servers.len(), 3);

    let slack = bundle
        .mcp_servers
        .iter()
        .find(|s| s.name == "slack")
        .unwrap();
    assert_eq!(slack.auth_token.as_deref(), Some("slack-token-123"));

    let notion = bundle
        .mcp_servers
        .iter()
        .find(|s| s.name == "notion")
        .unwrap();
    assert!(notion.auth_token.is_none());

    let linear = bundle
        .mcp_servers
        .iter()
        .find(|s| s.name == "linear")
        .unwrap();
    assert_eq!(linear.auth_token.as_deref(), Some("linear-token-456"));
}

#[test]
fn test_auth_token_flows_to_mcp_server_config() {
    // Simulate what activate_bundle does: McpServerDef → McpServerConfig
    let server_def = McpServerDef {
        name: "slack".to_string(),
        transport_type: "http".to_string(),
        url: "https://mcp.slack.com/mcp".to_string(),
        auth_token: Some("my-oauth-token".to_string()),
    };

    let config = McpServerConfig {
        name: server_def.name.clone(),
        transport: McpTransport::Http {
            url: server_def.url.clone(),
        },
        enabled: true,
        namespace: server_def.name.clone(),
        startup_timeout_secs: 30,
        auto_restart: false,
        auth_token: server_def.auth_token.clone(),
    };

    assert_eq!(config.auth_token.as_deref(), Some("my-oauth-token"));
    assert_eq!(config.name, "slack");
    assert!(matches!(config.transport, McpTransport::Http { .. }));
}

// ============================================================================
// 5. McpConnectionStatus Serialization (API response)
// ============================================================================

#[test]
fn test_connection_status_serialization() {
    // Verify the serde(tag = "status") serialization for API responses
    let connected = McpConnectionStatus::Connected;
    let json = serde_json::to_value(&connected).unwrap();
    assert_eq!(json["status"], "Connected");

    let failed = McpConnectionStatus::Failed("timeout".to_string());
    let json = serde_json::to_value(&failed).unwrap();
    assert_eq!(json["status"], "Failed");
    assert_eq!(json["error"], "timeout");

    let pending = McpConnectionStatus::Pending;
    let json = serde_json::to_value(&pending).unwrap();
    assert_eq!(json["status"], "Pending");
}

#[test]
fn test_connection_status_labels() {
    assert_eq!(McpConnectionStatus::Pending.label(), "pending");
    assert_eq!(McpConnectionStatus::Connecting.label(), "connecting");
    assert_eq!(McpConnectionStatus::Connected.label(), "connected");
    assert_eq!(
        McpConnectionStatus::Failed("err".to_string()).label(),
        "failed"
    );
    assert_eq!(McpConnectionStatus::Disconnected.label(), "disconnected");
}

#[test]
fn test_connection_status_error_message() {
    assert!(McpConnectionStatus::Connected.error_message().is_none());
    assert!(McpConnectionStatus::Pending.error_message().is_none());
    assert_eq!(
        McpConnectionStatus::Failed("timeout".to_string()).error_message(),
        Some("timeout")
    );
}

// ============================================================================
// 6. Concurrent Bundle Activation Safety
// ============================================================================

#[test]
fn test_concurrent_add_reference_deterministic() {
    // DashMap guarantees: only one of two concurrent add_reference() calls
    // for the same server returns true (first=true, subsequent=false).
    // This test validates the logic sequentially but confirms the invariant.
    let tracker = McpRefTracker::new();

    let first = tracker.add_reference("slack", "bundle-a");
    let second = tracker.add_reference("slack", "bundle-b");

    assert!(first);
    assert!(!second);
    assert_eq!(tracker.ref_count("slack"), 2);
}

#[tokio::test]
async fn test_concurrent_activation_with_shared_servers() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let ref_tracker = Arc::new(McpRefTracker::new());
    let conn_tracker = Arc::new(McpConnectionTracker::new());
    let connect_count = Arc::new(AtomicUsize::new(0));

    // Simulate 10 bundles activating "slack" concurrently
    let mut handles = Vec::new();
    for i in 0..10 {
        let rt = ref_tracker.clone();
        let ct = conn_tracker.clone();
        let cc = connect_count.clone();
        handles.push(tokio::spawn(async move {
            let bundle_name = format!("bundle-{}", i);
            let is_first = rt.add_reference("slack", &bundle_name);
            if is_first {
                // Only the first activation should trigger a connect
                ct.set_status("slack", McpConnectionStatus::Connecting);
                cc.fetch_add(1, Ordering::SeqCst);
                // Simulate connect delay
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                ct.set_status("slack", McpConnectionStatus::Connected);
            }
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Exactly 1 connect should have happened
    assert_eq!(connect_count.load(Ordering::SeqCst), 1);
    assert_eq!(ref_tracker.ref_count("slack"), 10);
    assert_eq!(conn_tracker.get_status("slack").label(), "connected");
}

// ============================================================================
// 7. Full Lifecycle E2E (without real MCP server)
// ============================================================================

#[tokio::test]
async fn test_full_lifecycle_activate_deactivate() {
    let tmp = TempDir::new().unwrap();

    // Setup bundles on disk
    create_bundle_with_mcp(
        tmp.path(),
        "productivity",
        &[
            ("slack", "https://mcp.slack.com/mcp"),
            ("notion", "https://mcp.notion.com/mcp"),
        ],
    );

    // Load bundles
    let mut bundle_mgr = BundleManager::new(vec![tmp.path().to_path_buf()]);
    bundle_mgr.discover();

    // Setup trackers
    let ref_tracker = McpRefTracker::new();
    let conn_tracker = McpConnectionTracker::new();
    let activation_path = tmp.path().join("activations.json");
    let mut runtime = RuntimeRegistry::new(activation_path);

    // === ACTIVATE ===
    let bundle = bundle_mgr.get("productivity").unwrap();
    runtime
        .activate_bundle("user1", "productivity", &bundle.version)
        .await
        .unwrap();

    for server in &bundle.mcp_servers {
        let is_first = ref_tracker.add_reference(&server.name, "productivity");
        assert!(is_first);
        conn_tracker.set_status(&server.name, McpConnectionStatus::Connecting);
    }

    // Simulate connection success
    conn_tracker.set_status("slack", McpConnectionStatus::Connected);
    conn_tracker.set_status("notion", McpConnectionStatus::Connected);

    // Verify active state
    assert!(runtime.is_active("user1", "productivity"));
    assert_eq!(conn_tracker.get_status("slack").label(), "connected");
    assert_eq!(conn_tracker.get_status("notion").label(), "connected");
    assert_eq!(ref_tracker.ref_count("slack"), 1);

    // === DEACTIVATE ===
    runtime
        .deactivate_bundle("user1", "productivity")
        .await
        .unwrap();

    let orphaned = ref_tracker.remove_bundle("productivity");
    assert_eq!(orphaned.len(), 2);
    for s in &orphaned {
        conn_tracker.remove(s);
    }

    // Verify clean state
    assert!(!runtime.is_active("user1", "productivity"));
    assert_eq!(conn_tracker.all_statuses().len(), 0);
    assert_eq!(ref_tracker.ref_count("slack"), 0);
    assert_eq!(ref_tracker.ref_count("notion"), 0);
}

#[tokio::test]
async fn test_full_lifecycle_two_bundles_shared_server() {
    let tmp = TempDir::new().unwrap();

    create_bundle_with_mcp(
        tmp.path(),
        "productivity",
        &[
            ("slack", "https://mcp.slack.com/mcp"),
            ("notion", "https://mcp.notion.com/mcp"),
        ],
    );
    create_bundle_with_mcp(
        tmp.path(),
        "sales",
        &[
            ("slack", "https://mcp.slack.com/mcp"),
            ("hubspot", "https://mcp.hubspot.com/mcp"),
        ],
    );

    let mut bundle_mgr = BundleManager::new(vec![tmp.path().to_path_buf()]);
    bundle_mgr.discover();

    let ref_tracker = McpRefTracker::new();
    let conn_tracker = McpConnectionTracker::new();
    let mut runtime = RuntimeRegistry::new(tmp.path().join("activations.json"));

    // === ACTIVATE productivity ===
    let productivity = bundle_mgr.get("productivity").unwrap();
    runtime
        .activate_bundle("user1", "productivity", &productivity.version)
        .await
        .unwrap();

    let mut connects = Vec::new();
    for server in &productivity.mcp_servers {
        if ref_tracker.add_reference(&server.name, "productivity") {
            connects.push(server.name.clone());
            conn_tracker.set_status(&server.name, McpConnectionStatus::Connected);
        }
    }
    connects.sort();
    assert_eq!(connects, vec!["notion", "slack"]);

    // === ACTIVATE sales ===
    let sales = bundle_mgr.get("sales").unwrap();
    runtime
        .activate_bundle("user1", "sales", &sales.version)
        .await
        .unwrap();

    connects.clear();
    for server in &sales.mcp_servers {
        if ref_tracker.add_reference(&server.name, "sales") {
            connects.push(server.name.clone());
            conn_tracker.set_status(&server.name, McpConnectionStatus::Connected);
        }
    }
    assert_eq!(connects, vec!["hubspot"]); // slack already connected

    // Verify shared state
    assert_eq!(ref_tracker.ref_count("slack"), 2);
    assert_eq!(conn_tracker.all_statuses().len(), 3);

    // === DEACTIVATE productivity ===
    runtime
        .deactivate_bundle("user1", "productivity")
        .await
        .unwrap();

    let orphaned = ref_tracker.remove_bundle("productivity");
    assert_eq!(orphaned, vec!["notion"]); // slack survives (ref=1)
    for s in &orphaned {
        conn_tracker.remove(s);
    }

    assert_eq!(conn_tracker.get_status("slack").label(), "connected"); // still alive
    assert_eq!(conn_tracker.get_status("notion").label(), "disconnected"); // cleaned up
    assert_eq!(conn_tracker.get_status("hubspot").label(), "connected");
    assert_eq!(ref_tracker.ref_count("slack"), 1);

    // === DEACTIVATE sales ===
    runtime.deactivate_bundle("user1", "sales").await.unwrap();

    let orphaned = ref_tracker.remove_bundle("sales");
    let mut orphaned_sorted = orphaned.clone();
    orphaned_sorted.sort();
    assert_eq!(orphaned_sorted, vec!["hubspot", "slack"]);
    for s in &orphaned {
        conn_tracker.remove(s);
    }

    // Everything cleaned up
    assert_eq!(conn_tracker.all_statuses().len(), 0);
    assert_eq!(ref_tracker.all_servers().len(), 0);
}

// ============================================================================
// 8. McpGateway Integration (register_server_config + connect_server)
// ============================================================================

#[tokio::test]
async fn test_mcp_gateway_register_config_with_auth_token() {
    let gateway = McpGateway::new();

    let config = McpServerConfig {
        name: "slack".to_string(),
        transport: McpTransport::Http {
            url: "https://mcp.slack.com/mcp".to_string(),
        },
        enabled: true,
        namespace: "slack".to_string(),
        startup_timeout_secs: 30,
        auto_restart: false,
        auth_token: Some("test-token-abc".to_string()),
    };

    gateway.register_server_config(config).await.unwrap();

    let stored = gateway.get_server("slack").await.unwrap();
    assert_eq!(stored.auth_token.as_deref(), Some("test-token-abc"));
    assert_eq!(stored.name, "slack");
}

#[tokio::test]
async fn test_mcp_gateway_unregister_cleans_up() {
    let gateway = McpGateway::new();

    let config = McpServerConfig {
        name: "notion".to_string(),
        transport: McpTransport::Http {
            url: "https://mcp.notion.com/mcp".to_string(),
        },
        enabled: true,
        namespace: "notion".to_string(),
        startup_timeout_secs: 30,
        auto_restart: false,
        auth_token: None,
    };

    gateway.register_server_config(config).await.unwrap();
    assert!(gateway.get_server("notion").await.is_some());

    gateway.unregister_server_by_name("notion").await.unwrap();
    assert!(gateway.get_server("notion").await.is_none());
}

// ============================================================================
// 9. McpServerDef serialization roundtrip
// ============================================================================

#[test]
fn test_mcp_server_def_serde_roundtrip() {
    let def = McpServerDef {
        name: "slack".to_string(),
        transport_type: "http".to_string(),
        url: "https://mcp.slack.com/mcp".to_string(),
        auth_token: Some("tok123".to_string()),
    };

    let json = serde_json::to_string(&def).unwrap();
    let back: McpServerDef = serde_json::from_str(&json).unwrap();

    assert_eq!(back.name, "slack");
    assert_eq!(back.url, "https://mcp.slack.com/mcp");
    assert_eq!(back.auth_token.as_deref(), Some("tok123"));
}

#[test]
fn test_mcp_server_def_serde_no_token() {
    let json = r#"{"name":"notion","transport_type":"http","url":"https://mcp.notion.com/mcp"}"#;
    let def: McpServerDef = serde_json::from_str(json).unwrap();

    assert_eq!(def.name, "notion");
    assert!(def.auth_token.is_none());
}

// ============================================================================
// 10. Edge Cases
// ============================================================================

#[test]
fn test_deactivate_bundle_with_no_mcp_servers() {
    let ref_tracker = McpRefTracker::new();
    let orphaned = ref_tracker.remove_bundle("nonexistent");
    assert!(orphaned.is_empty());
}

#[test]
fn test_double_activate_same_bundle() {
    let ref_tracker = McpRefTracker::new();

    // Same bundle adds same server twice — should be idempotent
    assert!(ref_tracker.add_reference("slack", "productivity"));
    // Adding the same (server, bundle) pair again
    assert!(!ref_tracker.add_reference("slack", "productivity"));
    // ref count should still be 1 (HashSet dedup)
    assert_eq!(ref_tracker.ref_count("slack"), 1);
}

#[test]
fn test_ref_tracker_rebuild_replaces_old_state() {
    let tracker = McpRefTracker::new();

    // Add some initial state
    tracker.add_reference("old-server", "old-bundle");
    assert_eq!(tracker.ref_count("old-server"), 1);

    // Rebuild clears everything
    tracker.rebuild(&[("new-bundle".to_string(), vec!["new-server".to_string()])]);

    assert_eq!(tracker.ref_count("old-server"), 0);
    assert_eq!(tracker.ref_count("new-server"), 1);
}

#[tokio::test]
async fn test_all_active_bundles_empty_registry() {
    let tmp = TempDir::new().unwrap();
    let registry = RuntimeRegistry::new(tmp.path().join("activations.json"));
    let all = registry.all_active_bundles();
    assert!(all.is_empty());
}

#[tokio::test]
async fn test_all_active_bundles_multi_user() {
    let tmp = TempDir::new().unwrap();
    let mut registry = RuntimeRegistry::new(tmp.path().join("activations.json"));

    registry
        .activate_bundle("user1", "productivity", "1.0.0")
        .await
        .unwrap();
    registry
        .activate_bundle("user1", "sales", "1.0.0")
        .await
        .unwrap();
    registry
        .activate_bundle("user2", "data-science", "1.0.0")
        .await
        .unwrap();

    let all = registry.all_active_bundles();
    assert_eq!(all.len(), 2); // 2 users

    let user1 = all.iter().find(|(uid, _)| uid == "user1").unwrap();
    assert_eq!(user1.1.len(), 2);

    let user2 = all.iter().find(|(uid, _)| uid == "user2").unwrap();
    assert_eq!(user2.1.len(), 1);
}

// ============================================================================
// 11. Real .mcp.json format (from Cowork plugin bundles)
// ============================================================================

#[test]
fn test_real_cowork_mcp_json_format() {
    let tmp = TempDir::new().unwrap();
    let bundle_dir = tmp.path().join("productivity");
    std::fs::create_dir_all(&bundle_dir).unwrap();

    std::fs::write(
        bundle_dir.join("plugin.json"),
        serde_json::to_string(&serde_json::json!({
            "name": "productivity",
            "version": "1.0.0",
            "description": "Productivity suite"
        }))
        .unwrap(),
    )
    .unwrap();

    // Real Cowork .mcp.json format (from actual plugin bundles)
    std::fs::write(
        bundle_dir.join(".mcp.json"),
        r#"{
          "mcpServers": {
            "slack": {
              "type": "http",
              "url": "https://mcp.composio.dev/slack/abc123"
            },
            "notion": {
              "type": "http",
              "url": "https://mcp.notion.com/mcp"
            },
            "asana": {
              "type": "http",
              "url": "https://mcp.composio.dev/asana/def456"
            },
            "atlassian": {
              "type": "http",
              "url": "https://mcp.composio.dev/atlassian/ghi789"
            },
            "ms365": {
              "type": "http",
              "url": "https://mcp.composio.dev/microsoft365/jkl012"
            },
            "google-workspace": {
              "type": "http",
              "url": "https://mcp.composio.dev/googleworkspace/mno345"
            }
          }
        }"#,
    )
    .unwrap();

    let bundles = BundleLoader::discover(&[tmp.path().to_path_buf()]);
    assert_eq!(bundles.len(), 1);

    let bundle = &bundles[0];
    assert_eq!(bundle.mcp_servers.len(), 6);

    let server_names: HashSet<String> = bundle.mcp_servers.iter().map(|s| s.name.clone()).collect();
    assert!(server_names.contains("slack"));
    assert!(server_names.contains("notion"));
    assert!(server_names.contains("asana"));
    assert!(server_names.contains("atlassian"));
    assert!(server_names.contains("ms365"));
    assert!(server_names.contains("google-workspace"));

    // All should have HTTP transport
    for s in &bundle.mcp_servers {
        assert_eq!(s.transport_type, "http");
        assert!(!s.url.is_empty());
    }
}

#[test]
fn test_mcp_json_with_mixed_transports() {
    let tmp = TempDir::new().unwrap();
    let bundle_dir = tmp.path().join("mixed");
    std::fs::create_dir_all(&bundle_dir).unwrap();

    std::fs::write(
        bundle_dir.join("plugin.json"),
        r#"{"name":"mixed","version":"1.0.0","description":"Mixed transports"}"#,
    )
    .unwrap();

    std::fs::write(
        bundle_dir.join(".mcp.json"),
        r#"{
          "mcpServers": {
            "remote": {"type": "http", "url": "https://example.com/mcp"},
            "local": {"type": "stdio", "url": ""},
            "default-type": {"url": "https://example.com/mcp2"}
          }
        }"#,
    )
    .unwrap();

    let bundles = BundleLoader::discover(&[tmp.path().to_path_buf()]);
    assert_eq!(bundles.len(), 1);

    // "local" has empty URL, should be skipped
    // "default-type" has no explicit type, defaults to "http"
    let bundle = &bundles[0];
    assert_eq!(bundle.mcp_servers.len(), 2); // remote + default-type

    let remote = bundle
        .mcp_servers
        .iter()
        .find(|s| s.name == "remote")
        .unwrap();
    assert_eq!(remote.transport_type, "http");

    let default = bundle
        .mcp_servers
        .iter()
        .find(|s| s.name == "default-type")
        .unwrap();
    assert_eq!(default.transport_type, "http"); // defaults to http
}

// ============================================================================
// 12. Status tracking with build_mcp_server_statuses logic
// ============================================================================

#[test]
fn test_connection_tracker_status_transitions() {
    let tracker = McpConnectionTracker::new();

    // Full lifecycle: Pending → Connecting → Connected → Disconnected
    tracker.set_status("slack", McpConnectionStatus::Pending);
    assert_eq!(tracker.get_status("slack").label(), "pending");

    tracker.set_status("slack", McpConnectionStatus::Connecting);
    assert_eq!(tracker.get_status("slack").label(), "connecting");

    tracker.set_status("slack", McpConnectionStatus::Connected);
    assert_eq!(tracker.get_status("slack").label(), "connected");

    // Simulate deactivation
    tracker.remove("slack");
    assert_eq!(tracker.get_status("slack").label(), "disconnected");
}

#[test]
fn test_connection_tracker_failed_then_retry() {
    let tracker = McpConnectionTracker::new();

    // First attempt fails
    tracker.set_status("notion", McpConnectionStatus::Connecting);
    tracker.set_status(
        "notion",
        McpConnectionStatus::Failed("DNS resolution failed".to_string()),
    );
    assert_eq!(tracker.get_status("notion").label(), "failed");
    assert_eq!(
        tracker.get_status("notion").error_message(),
        Some("DNS resolution failed")
    );

    // Retry succeeds (status can be overwritten)
    tracker.set_status("notion", McpConnectionStatus::Connecting);
    tracker.set_status("notion", McpConnectionStatus::Connected);
    assert_eq!(tracker.get_status("notion").label(), "connected");
    assert!(tracker.get_status("notion").error_message().is_none());
}
