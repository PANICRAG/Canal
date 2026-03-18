//! Integration tests for the Unified Tool System
//!
//! Tests cross-module interactions:
//! - Registry + Executor integration
//! - ToolSystem + McpGateway shared state
//! - ToolSystem + AgentFactory wiring
//! - External MCP tool registration flow
//! - Permission enforcement end-to-end
//! - Regression: old/new system consistency

use gateway_core::agent::types::{PermissionBehavior, PermissionMode, PermissionRule};
use gateway_core::mcp::gateway::{McpServerConfig, McpTransport};
use gateway_core::mcp::protocol::McpToolDef;
use gateway_core::tool_system::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Test helpers
// ============================================================================

fn make_entry(ns: &str, name: &str, source: ToolSource) -> ToolEntry {
    ToolEntry {
        id: ToolId::new(ns, name),
        description: format!("Test tool {}.{}", ns, name),
        input_schema: serde_json::json!({"type": "object"}),
        source,
        meta: ToolMeta {
            transport_type: "test".to_string(),
            location: "local".to_string(),
            server_name: "test".to_string(),
        },
    }
}

async fn setup_tool_system_with_builtins() -> ToolSystem {
    let ts = ToolSystem::new();
    {
        let mut registry = ts.registry.write().await;
        ToolSystem::register_all_builtin_tools(&mut registry);
    }
    ts
}

async fn setup_full_tool_system() -> ToolSystem {
    let ts = ToolSystem::new();
    {
        let mut registry = ts.registry.write().await;

        // Register 7 agent core tools
        for name in &["Read", "Write", "Edit", "Bash", "Glob", "Grep", "Computer"] {
            registry.register(make_entry("agent", name, ToolSource::Agent));
        }

        // Register 27 MCP builtins
        ToolSystem::register_all_builtin_tools(&mut registry);
    }
    ts
}

// ============================================================================
// Registry + Executor integration
// ============================================================================

#[tokio::test]
async fn registry_and_executor_share_tool_resolution() {
    let ts = setup_full_tool_system().await;

    // Verify registry has 34 tools
    assert_eq!(ts.tool_count().await, 34);

    // Verify executor can find tools through the registry
    // (execute will fail because no real backend, but resolution should work)
    let result = ts.execute("agent", "Read", serde_json::json!({})).await;
    // Error should be about agent tool not found in executor's agent_tools map
    // (not about "tool not found in registry")
    assert!(result.is_err());
    // The error comes from executor, not from registry lookup
}

#[tokio::test]
async fn execute_by_namespace_resolves_through_registry() {
    let ts = setup_tool_system_with_builtins().await;

    // Execute by namespace - should resolve through registry then fail at executor
    let result = ts
        .execute(
            "filesystem",
            "read_file",
            serde_json::json!({"path": "/tmp/test"}),
        )
        .await;
    assert!(result.is_err());
    // Error should be about builtin executor not being initialized with real services
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not initialized") || err.contains("not found"),
        "Expected initialization error, got: {}",
        err
    );
}

#[tokio::test]
async fn execute_llm_tool_call_resolves_agent_name() {
    let ts = setup_full_tool_system().await;

    // "Read" should resolve through get_by_llm_name → agent.Read
    let result = ts
        .execute_llm_tool_call("Read", serde_json::json!({}))
        .await;
    // Will fail because no real DynamicTool registered in executor, but should resolve
    // In bypass mode, permission passes, then executor fails
    match result {
        Ok(r) => {
            // ToolCallResult returned as "not found" error
            assert!(r.is_error);
        }
        Err(e) => {
            // Or it could error at executor level
            assert!(
                !e.to_string().contains("PERMISSION"),
                "Should not be a permission error in bypass mode"
            );
        }
    }
}

#[tokio::test]
async fn execute_llm_tool_call_resolves_mcp_name() {
    let ts = setup_tool_system_with_builtins().await;

    // "filesystem_read_file" should resolve through from_llm_name
    let result = ts
        .execute_llm_tool_call(
            "filesystem_read_file",
            serde_json::json!({"path": "/tmp/test"}),
        )
        .await;
    // Default bypass permission, executor will fail on missing backend
    match result {
        Ok(r) => assert!(r.is_error),
        Err(e) => assert!(
            !e.to_string().contains("PERMISSION"),
            "Should not be a permission error"
        ),
    }
}

// ============================================================================
// ToolSystem + McpGateway shared state
// ============================================================================

#[tokio::test]
async fn shared_connections_arc_between_systems() {
    let connections: Arc<RwLock<HashMap<String, gateway_core::mcp::connection::McpConnection>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let configs: Arc<RwLock<HashMap<String, McpServerConfig>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let ts = ToolSystem::new_with_shared(configs.clone(), connections.clone());

    // Verify they share the same Arc
    assert!(Arc::ptr_eq(ts.connections_ref(), &connections));
    assert!(Arc::ptr_eq(ts.configs_ref(), &configs));
}

#[tokio::test]
async fn configs_visible_through_shared_arc() {
    let configs: Arc<RwLock<HashMap<String, McpServerConfig>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let connections: Arc<RwLock<HashMap<String, gateway_core::mcp::connection::McpConnection>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let ts = ToolSystem::new_with_shared(configs.clone(), connections.clone());

    // Register through ToolSystem
    ts.register_mcp_server(McpServerConfig {
        name: "test-server".to_string(),
        namespace: "test".to_string(),
        enabled: true,
        transport: McpTransport::Http {
            url: "http://localhost:3000".to_string(),
        },
        startup_timeout_secs: 30,
        auto_restart: false,
        auth_token: None,
    })
    .await;

    // Verify visible through shared Arc
    let c = configs.read().await;
    assert!(c.contains_key("test-server"));
}

// ============================================================================
// External MCP tool registration flow
// ============================================================================

#[tokio::test]
async fn register_external_tools_from_discovery() {
    let ts = ToolSystem::new();

    // Simulate tool discovery from an MCP server
    let discovered_tools = vec![
        McpToolDef {
            name: "list_ideas".to_string(),
            description: Some("List video ideas".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "category": {"type": "string"}
                }
            }),
        },
        McpToolDef {
            name: "create_video".to_string(),
            description: Some("Create a new video".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["title"],
                "properties": {
                    "title": {"type": "string"},
                    "duration": {"type": "integer"}
                }
            }),
        },
    ];

    ts.register_external_tools(
        "videocli",
        "videocli-mcp",
        &McpTransport::Stdio {
            command: "npx".to_string(),
            args: vec!["videocli-mcp".to_string()],
            env: HashMap::new(),
        },
        &discovered_tools,
    )
    .await;

    // Verify registration
    assert_eq!(ts.tool_count().await, 2);

    let tools = ts.list_by_namespace("videocli").await;
    assert_eq!(tools.len(), 2);

    // Verify metadata
    for tool in &tools {
        assert_eq!(tool.meta.transport_type, "stdio");
        assert!(tool.meta.location.contains("npx"));
        assert_eq!(tool.meta.server_name, "videocli-mcp");
        assert!(matches!(
            &tool.source,
            ToolSource::McpExternal { server_name } if server_name == "videocli-mcp"
        ));
    }

    // Verify LLM name resolution
    let meta = ts.get_tool_meta("videocli_list_ideas").await;
    assert!(meta.is_some());

    // Verify schema output
    let schemas = ts.tools_for_llm().await;
    assert_eq!(schemas.len(), 2);
    let names: Vec<String> = schemas
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"videocli_list_ideas".to_string()));
    assert!(names.contains(&"videocli_create_video".to_string()));
}

#[tokio::test]
async fn external_tools_coexist_with_builtins() {
    let ts = setup_full_tool_system().await;

    // Add external tools
    ts.register_external_tools(
        "videocli",
        "videocli-mcp",
        &McpTransport::Http {
            url: "http://localhost:5000".to_string(),
        },
        &[McpToolDef {
            name: "list_ideas".to_string(),
            description: Some("List ideas".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
        }],
    )
    .await;

    // Total should be 34 + 1 = 35
    assert_eq!(ts.tool_count().await, 35);

    // Builtin tools still accessible
    let fs_tools = ts.list_by_namespace("filesystem").await;
    assert_eq!(fs_tools.len(), 4);

    // External tools accessible
    let vc_tools = ts.list_by_namespace("videocli").await;
    assert_eq!(vc_tools.len(), 1);

    // All tool meta includes both
    let all_meta = ts.get_all_tool_meta().await;
    assert_eq!(all_meta.len(), 35);
    assert!(all_meta.contains_key("filesystem_read_file"));
    assert!(all_meta.contains_key("videocli_list_ideas"));
}

// ============================================================================
// Permission enforcement end-to-end
// ============================================================================

#[tokio::test]
async fn permission_bypass_allows_all_tool_calls() {
    let ts = setup_full_tool_system().await;
    ts.set_permission_mode(PermissionMode::BypassPermissions)
        .await;

    // Even dangerous tool calls should pass permission (fail on executor)
    let tools_to_test = vec![
        "Read",
        "Write",
        "Bash",
        "filesystem_read_file",
        "filesystem_write_file",
        "executor_bash",
    ];

    for tool in tools_to_test {
        let result = ts.execute_llm_tool_call(tool, serde_json::json!({})).await;
        // Should NOT get PermissionDenied
        match &result {
            Err(e) => assert!(
                !e.to_string().contains("Permission denied"),
                "Bypass should not deny '{}': {}",
                tool,
                e
            ),
            Ok(_) => {} // OK - might succeed or return error result
        }
    }
}

#[tokio::test]
async fn permission_plan_blocks_all_modifying_tools() {
    let ts = setup_full_tool_system().await;
    ts.set_permission_mode(PermissionMode::Plan).await;

    // Plan mode denies tools whose names contain Write/Edit/Bash/NotebookEdit
    let blocked_tools = vec!["Write", "Edit", "Bash", "NotebookEdit"];

    for tool in blocked_tools {
        let result = ts.execute_llm_tool_call(tool, serde_json::json!({})).await;
        assert!(
            result.is_err(),
            "Plan mode should deny '{}' but got Ok",
            tool
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Plan mode"),
            "Expected 'Plan mode' in error for '{}', got: {}",
            tool,
            err
        );
    }
}

#[tokio::test]
async fn permission_rules_apply_correctly() {
    let ts = setup_full_tool_system().await;
    ts.set_permission_mode(PermissionMode::Default).await;

    // Add rule: allow all filesystem tools
    ts.add_permission_rule(
        PermissionRule::tool("filesystem_*"),
        PermissionBehavior::Allow,
    )
    .await;

    // Add rule: deny executor_bash
    ts.add_permission_rule(
        PermissionRule::tool("executor_bash"),
        PermissionBehavior::Deny,
    )
    .await;

    // filesystem_read_file should be allowed (rule match)
    let result = ts
        .execute_llm_tool_call(
            "filesystem_read_file",
            serde_json::json!({"path": "/tmp/test"}),
        )
        .await;
    // Should not be a permission error
    match &result {
        Err(e) => assert!(
            !e.to_string().contains("Permission denied")
                && !e.to_string().contains("PERMISSION_REQUIRED"),
            "filesystem_read_file should be allowed by rule, got: {}",
            e
        ),
        Ok(_) => {}
    }

    // executor_bash should be denied
    let result = ts
        .execute_llm_tool_call("executor_bash", serde_json::json!({}))
        .await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Permission denied"));
}

#[tokio::test]
async fn permission_accept_edits_auto_approves_edit_tools() {
    let ts = setup_full_tool_system().await;
    ts.set_permission_mode(PermissionMode::AcceptEdits).await;

    // Write/Edit/NotebookEdit should be auto-approved (no PERMISSION_REQUIRED)
    let edit_tools = vec!["Write", "Edit"];

    for tool in edit_tools {
        let result = ts.execute_llm_tool_call(tool, serde_json::json!({})).await;
        match &result {
            Err(e) => assert!(
                !e.to_string().contains("PERMISSION_REQUIRED"),
                "AcceptEdits should auto-approve '{}', got: {}",
                tool,
                e
            ),
            Ok(_) => {}
        }
    }
}

#[tokio::test]
async fn permission_default_asks_for_unknown_tools() {
    let ts = setup_full_tool_system().await;
    ts.set_permission_mode(PermissionMode::Default).await;

    let result = ts
        .execute_llm_tool_call("executor_bash", serde_json::json!({"command": "ls"}))
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("PERMISSION_REQUIRED"),
        "Default mode should ask, got: {}",
        err
    );
}

// ============================================================================
// Tool hot-update (re-registration)
// ============================================================================

#[tokio::test]
async fn tool_reregistration_updates_in_place() {
    let ts = ToolSystem::new();

    // Register v1
    ts.register_external_tools(
        "myserver",
        "my-mcp",
        &McpTransport::Http {
            url: "http://localhost:3000".to_string(),
        },
        &[McpToolDef {
            name: "my_tool".to_string(),
            description: Some("v1".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
        }],
    )
    .await;

    let tools = ts.list_by_namespace("myserver").await;
    assert_eq!(tools[0].description, "v1");

    // Re-register v2 (same namespace.name)
    ts.register_external_tools(
        "myserver",
        "my-mcp",
        &McpTransport::Http {
            url: "http://localhost:3000".to_string(),
        },
        &[McpToolDef {
            name: "my_tool".to_string(),
            description: Some("v2".to_string()),
            input_schema: serde_json::json!({"type": "object", "properties": {"new_field": {"type": "string"}}}),
        }],
    )
    .await;

    // Should still be 1 tool, but updated
    assert_eq!(ts.tool_count().await, 1);
    let tools = ts.list_by_namespace("myserver").await;
    assert_eq!(tools[0].description, "v2");
    assert!(tools[0].input_schema["properties"]["new_field"].is_object());
}

// ============================================================================
// Schema generation consistency
// ============================================================================

#[tokio::test]
async fn schemas_for_llm_consistent_with_tools_for_llm() {
    let ts = setup_full_tool_system().await;

    let all_schemas = ts.tools_for_llm().await;
    let filtered = ts.schemas_for_llm(&ToolFilter::default()).await;

    // tools_for_llm returns ALL schemas
    assert_eq!(all_schemas.len(), 34);

    // schemas_for_llm with default filter should include:
    // - 7 agent core tools (but not Orchestrate/CodeOrchestration/browser)
    // - 27 MCP builtin tools
    assert_eq!(filtered.len(), 34);
}

#[tokio::test]
async fn schema_format_matches_expected_structure() {
    let ts = setup_full_tool_system().await;
    let schemas = ts.tools_for_llm().await;

    for schema in &schemas {
        // Every schema must have name, description, input_schema
        assert!(
            schema.get("name").is_some(),
            "Missing 'name' in schema: {:?}",
            schema
        );
        assert!(
            schema.get("description").is_some(),
            "Missing 'description' in schema: {:?}",
            schema
        );
        assert!(
            schema.get("input_schema").is_some(),
            "Missing 'input_schema' in schema: {:?}",
            schema
        );

        // name must be a non-empty string
        let name = schema["name"].as_str().unwrap();
        assert!(!name.is_empty());

        // input_schema must be an object
        assert!(schema["input_schema"].is_object());
    }
}

#[tokio::test]
async fn agent_tool_schemas_use_bare_names() {
    let ts = setup_full_tool_system().await;
    let schemas = ts.tools_for_llm().await;

    let agent_tool_names: Vec<String> = schemas
        .iter()
        .filter_map(|s| {
            let name = s["name"].as_str().unwrap();
            if !name.contains('_') {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();

    // All 7 core agent tools should have bare names
    assert!(agent_tool_names.contains(&"Read".to_string()));
    assert!(agent_tool_names.contains(&"Write".to_string()));
    assert!(agent_tool_names.contains(&"Edit".to_string()));
    assert!(agent_tool_names.contains(&"Bash".to_string()));
    assert!(agent_tool_names.contains(&"Glob".to_string()));
    assert!(agent_tool_names.contains(&"Grep".to_string()));
    assert!(agent_tool_names.contains(&"Computer".to_string()));
}

#[tokio::test]
async fn mcp_tool_schemas_use_namespace_names() {
    let ts = setup_full_tool_system().await;
    let schemas = ts.tools_for_llm().await;

    let mcp_tool_names: Vec<String> = schemas
        .iter()
        .filter_map(|s| {
            let name = s["name"].as_str().unwrap();
            if name.contains('_') {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();

    // All 27 MCP builtin tools should have namespace_name format
    assert_eq!(mcp_tool_names.len(), 27);
    assert!(mcp_tool_names.contains(&"filesystem_read_file".to_string()));
    assert!(mcp_tool_names.contains(&"mac_get_frontmost_app".to_string()));
    assert!(mcp_tool_names.contains(&"automation_execute".to_string()));
}

// ============================================================================
// Regression: Tool count verification
// ============================================================================

#[tokio::test]
async fn full_system_has_correct_tool_counts() {
    let ts = setup_full_tool_system().await;

    // Total: 7 agent + 27 builtin = 34
    assert_eq!(ts.tool_count().await, 34);

    // By namespace
    assert_eq!(ts.list_by_namespace("agent").await.len(), 7);
    assert_eq!(ts.list_by_namespace("filesystem").await.len(), 4);
    assert_eq!(ts.list_by_namespace("executor").await.len(), 3);
    assert_eq!(ts.list_by_namespace("browser").await.len(), 8);
    assert_eq!(ts.list_by_namespace("mac").await.len(), 9);
    assert_eq!(ts.list_by_namespace("automation").await.len(), 3);
}

#[tokio::test]
async fn all_builtin_tools_accessible_by_llm_name() {
    let ts = setup_full_tool_system().await;

    let all_llm_names = vec![
        // Agent tools
        "Read",
        "Write",
        "Edit",
        "Bash",
        "Glob",
        "Grep",
        "Computer",
        // Filesystem
        "filesystem_read_file",
        "filesystem_write_file",
        "filesystem_list_directory",
        "filesystem_search",
        // Executor
        "executor_bash",
        "executor_python",
        "executor_run_code",
        // Browser
        "browser_navigate",
        "browser_snapshot",
        "browser_click",
        "browser_fill",
        "browser_screenshot",
        "browser_scroll",
        "browser_wait",
        "browser_evaluate",
        // Mac
        "mac_osascript",
        "mac_screenshot",
        "mac_app_control",
        "mac_open_url",
        "mac_notify",
        "mac_clipboard_read",
        "mac_clipboard_write",
        "mac_get_frontmost_app",
        "mac_list_running_apps",
        // Automation
        "automation_analyze",
        "automation_execute",
        "automation_status",
    ];

    for name in &all_llm_names {
        let meta = ts.get_tool_meta(name).await;
        assert!(
            meta.is_some(),
            "Tool '{}' should be accessible by LLM name",
            name
        );
    }
}

// ============================================================================
// Degradation / fallback
// ============================================================================

#[tokio::test]
async fn empty_tool_system_returns_graceful_errors() {
    let ts = ToolSystem::new();

    // No tools registered - execute should fail gracefully
    let result = ts
        .execute("nonexistent", "tool", serde_json::json!({}))
        .await;
    assert!(result.is_err());

    // List operations return empty, not error
    assert_eq!(ts.list_tools().await.len(), 0);
    assert_eq!(ts.list_by_namespace("any").await.len(), 0);
    assert_eq!(ts.get_all_tool_meta().await.len(), 0);
    assert_eq!(ts.tools_for_llm().await.len(), 0);
}

#[tokio::test]
async fn permission_mode_switch_takes_effect_immediately() {
    let ts = setup_full_tool_system().await;

    // Start in bypass
    ts.set_permission_mode(PermissionMode::BypassPermissions)
        .await;
    let result = ts
        .execute_llm_tool_call("Write", serde_json::json!({}))
        .await;
    // Should not get permission denied
    match &result {
        Err(e) => assert!(
            !e.to_string().contains("Plan mode"),
            "Bypass should not deny: {}",
            e
        ),
        Ok(_) => {}
    }

    // Switch to Plan
    ts.set_permission_mode(PermissionMode::Plan).await;
    let result = ts
        .execute_llm_tool_call("Write", serde_json::json!({}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Plan mode"));

    // Switch back to bypass
    ts.set_permission_mode(PermissionMode::BypassPermissions)
        .await;
    let result = ts
        .execute_llm_tool_call("Write", serde_json::json!({}))
        .await;
    match &result {
        Err(e) => assert!(
            !e.to_string().contains("Plan mode"),
            "Back to bypass should not deny: {}",
            e
        ),
        Ok(_) => {}
    }
}

// ============================================================================
// Concurrency
// ============================================================================

#[tokio::test]
async fn concurrent_reads_are_safe() {
    let ts = Arc::new(setup_full_tool_system().await);

    let mut handles = vec![];
    for _ in 0..20 {
        let ts = ts.clone();
        handles.push(tokio::spawn(async move {
            let tools = ts.list_tools().await;
            assert_eq!(tools.len(), 34);
            let meta = ts.get_tool_meta("filesystem_read_file").await;
            assert!(meta.is_some());
            let schemas = ts.tools_for_llm().await;
            assert_eq!(schemas.len(), 34);
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn concurrent_register_and_read() {
    let ts = Arc::new(ToolSystem::new());

    // Spawn writers
    let mut handles = vec![];
    for i in 0..10 {
        let ts = ts.clone();
        handles.push(tokio::spawn(async move {
            ts.register_external_tools(
                &format!("ns{}", i),
                &format!("server{}", i),
                &McpTransport::Http {
                    url: format!("http://localhost:{}", 3000 + i),
                },
                &[McpToolDef {
                    name: format!("tool_{}", i),
                    description: Some(format!("Tool {}", i)),
                    input_schema: serde_json::json!({"type": "object"}),
                }],
            )
            .await;
        }));
    }

    // Spawn readers
    for _ in 0..10 {
        let ts = ts.clone();
        handles.push(tokio::spawn(async move {
            // These should not panic even during concurrent writes
            let _ = ts.list_tools().await;
            let _ = ts.tool_count().await;
            let _ = ts.tools_for_llm().await;
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // After all writes complete, should have 10 tools
    assert_eq!(ts.tool_count().await, 10);
}

// ============================================================================
// Health check integration
// ============================================================================

#[tokio::test]
async fn health_check_with_registered_servers() {
    let ts = ToolSystem::new();

    // Register 3 server configs
    for i in 0..3 {
        ts.register_mcp_server(McpServerConfig {
            name: format!("server-{}", i),
            namespace: format!("ns{}", i),
            enabled: true,
            transport: McpTransport::Http {
                url: format!("http://localhost:{}", 3000 + i),
            },
            startup_timeout_secs: 30,
            auto_restart: false,
            auth_token: None,
        })
        .await;
    }

    // All should show as disconnected
    let status = ts.health_check_map().await;
    assert_eq!(status.len(), 3);
    for (_, connected) in &status {
        assert!(!connected);
    }

    // Individual health checks
    for i in 0..3 {
        let result = ts.health_check(&format!("ns{}", i)).await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Not connected
    }

    // Unknown namespace
    let result = ts.health_check("unknown").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn list_servers_shows_config_info() {
    let ts = ToolSystem::new();

    ts.register_mcp_server(McpServerConfig {
        name: "test-server".to_string(),
        namespace: "test".to_string(),
        enabled: true,
        transport: McpTransport::Stdio {
            command: "npx".to_string(),
            args: vec!["test-mcp".to_string()],
            env: HashMap::new(),
        },
        startup_timeout_secs: 30,
        auto_restart: false,
        auth_token: None,
    })
    .await;

    // Register some tools for this namespace
    ts.register_external_tools(
        "test",
        "test-server",
        &McpTransport::Stdio {
            command: "npx".to_string(),
            args: vec![],
            env: HashMap::new(),
        },
        &[
            McpToolDef {
                name: "tool_a".to_string(),
                description: Some("Tool A".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            },
            McpToolDef {
                name: "tool_b".to_string(),
                description: Some("Tool B".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
            },
        ],
    )
    .await;

    let servers = ts.list_servers().await;
    assert_eq!(servers.len(), 1);

    let (namespace, info) = &servers[0];
    assert_eq!(namespace, "test");
    assert_eq!(info.tool_count, 2);
    assert!(!info.connected);
    assert_eq!(info.transport_type, "stdio");
    assert_eq!(info.server_name, "test-server");
}
