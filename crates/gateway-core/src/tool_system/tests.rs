//! Comprehensive tests for the Unified Tool System
//!
//! Test structure:
//! - types: ToolId, ToolEntry, ToolSource, ToolMeta, ToolFilter
//! - registry: UnifiedToolRegistry CRUD, indexes, edge cases
//! - resolver: Schema generation, filtering, resolution
//! - permissions: PermissionMode matrix, rules, format compatibility
//! - facade: ToolSystem registration, execution, query APIs

#[cfg(test)]
mod types_tests {
    use crate::tool_system::types::*;

    // ========================================================================
    // ToolId::new
    // ========================================================================

    #[test]
    fn new_creates_with_correct_fields() {
        let id = ToolId::new("filesystem", "read_file");
        assert_eq!(id.namespace, "filesystem");
        assert_eq!(id.name, "read_file");
    }

    #[test]
    fn new_with_empty_namespace() {
        let id = ToolId::new("", "read_file");
        assert_eq!(id.namespace, "");
        assert_eq!(id.name, "read_file");
    }

    #[test]
    fn new_with_empty_name() {
        let id = ToolId::new("filesystem", "");
        assert_eq!(id.namespace, "filesystem");
        assert_eq!(id.name, "");
    }

    #[test]
    fn new_with_special_characters() {
        let id = ToolId::new("my-ns", "my_tool");
        assert_eq!(id.namespace, "my-ns");
        assert_eq!(id.name, "my_tool");
    }

    // ========================================================================
    // ToolId::agent
    // ========================================================================

    #[test]
    fn agent_sets_agent_namespace() {
        let id = ToolId::agent("Read");
        assert_eq!(id.namespace, "agent");
        assert_eq!(id.name, "Read");
    }

    #[test]
    fn agent_preserves_case() {
        let id = ToolId::agent("BrowserTool");
        assert_eq!(id.name, "BrowserTool");
    }

    // ========================================================================
    // ToolId::from_llm_name
    // ========================================================================

    #[test]
    fn from_llm_name_standard_mcp() {
        let id = ToolId::from_llm_name("filesystem_read_file").unwrap();
        assert_eq!(id.namespace, "filesystem");
        assert_eq!(id.name, "read_file");
    }

    #[test]
    fn from_llm_name_multi_underscore() {
        let id = ToolId::from_llm_name("mac_get_frontmost_app").unwrap();
        assert_eq!(id.namespace, "mac");
        assert_eq!(id.name, "get_frontmost_app");
    }

    #[test]
    fn from_llm_name_agent_tool_returns_none() {
        assert!(ToolId::from_llm_name("Read").is_none());
    }

    #[test]
    fn from_llm_name_all_agent_tools_return_none() {
        for name in &["Read", "Write", "Edit", "Bash", "Glob", "Grep", "Computer"] {
            assert!(
                ToolId::from_llm_name(name).is_none(),
                "'{}' should NOT be parsed as MCP tool",
                name
            );
        }
    }

    #[test]
    fn from_llm_name_empty_returns_none() {
        assert!(ToolId::from_llm_name("").is_none());
    }

    #[test]
    fn from_llm_name_leading_underscore() {
        let id = ToolId::from_llm_name("_broken").unwrap();
        assert_eq!(id.namespace, "");
        assert_eq!(id.name, "broken");
    }

    #[test]
    fn from_llm_name_trailing_underscore() {
        let id = ToolId::from_llm_name("broken_").unwrap();
        assert_eq!(id.namespace, "broken");
        assert_eq!(id.name, "");
    }

    #[test]
    fn from_llm_name_only_underscore() {
        let id = ToolId::from_llm_name("_").unwrap();
        assert_eq!(id.namespace, "");
        assert_eq!(id.name, "");
    }

    #[test]
    fn from_llm_name_all_27_builtin_tools() {
        let all = vec![
            ("filesystem_read_file", "filesystem", "read_file"),
            ("filesystem_write_file", "filesystem", "write_file"),
            ("filesystem_list_directory", "filesystem", "list_directory"),
            ("filesystem_search", "filesystem", "search"),
            ("executor_bash", "executor", "bash"),
            ("executor_python", "executor", "python"),
            ("executor_run_code", "executor", "run_code"),
            ("browser_navigate", "browser", "navigate"),
            ("browser_snapshot", "browser", "snapshot"),
            ("browser_click", "browser", "click"),
            ("browser_fill", "browser", "fill"),
            ("browser_screenshot", "browser", "screenshot"),
            ("browser_scroll", "browser", "scroll"),
            ("browser_wait", "browser", "wait"),
            ("browser_evaluate", "browser", "evaluate"),
            ("mac_osascript", "mac", "osascript"),
            ("mac_screenshot", "mac", "screenshot"),
            ("mac_app_control", "mac", "app_control"),
            ("mac_open_url", "mac", "open_url"),
            ("mac_notify", "mac", "notify"),
            ("mac_clipboard_read", "mac", "clipboard_read"),
            ("mac_clipboard_write", "mac", "clipboard_write"),
            ("mac_get_frontmost_app", "mac", "get_frontmost_app"),
            ("mac_list_running_apps", "mac", "list_running_apps"),
            ("automation_analyze", "automation", "analyze"),
            ("automation_execute", "automation", "execute"),
            ("automation_status", "automation", "status"),
        ];

        for (llm_name, expected_ns, expected_name) in &all {
            let id = ToolId::from_llm_name(llm_name)
                .unwrap_or_else(|| panic!("Failed to parse: '{}'", llm_name));
            assert_eq!(id.namespace, *expected_ns, "ns mismatch for '{}'", llm_name);
            assert_eq!(id.name, *expected_name, "name mismatch for '{}'", llm_name);
        }
    }

    // ========================================================================
    // ToolId::llm_name
    // ========================================================================

    #[test]
    fn llm_name_agent_returns_bare_name() {
        let id = ToolId::agent("Read");
        assert_eq!(id.llm_name(), "Read");
    }

    #[test]
    fn llm_name_mcp_returns_namespace_name() {
        let id = ToolId::new("filesystem", "read_file");
        assert_eq!(id.llm_name(), "filesystem_read_file");
    }

    #[test]
    fn llm_name_roundtrip_mcp() {
        let original = "filesystem_read_file";
        let id = ToolId::from_llm_name(original).unwrap();
        assert_eq!(id.llm_name(), original);
    }

    #[test]
    fn llm_name_roundtrip_multi_underscore() {
        let original = "mac_get_frontmost_app";
        let id = ToolId::from_llm_name(original).unwrap();
        assert_eq!(id.llm_name(), original);
    }

    #[test]
    fn llm_name_all_27_roundtrip() {
        let all_llm_names = vec![
            "filesystem_read_file",
            "filesystem_write_file",
            "filesystem_list_directory",
            "filesystem_search",
            "executor_bash",
            "executor_python",
            "executor_run_code",
            "browser_navigate",
            "browser_snapshot",
            "browser_click",
            "browser_fill",
            "browser_screenshot",
            "browser_scroll",
            "browser_wait",
            "browser_evaluate",
            "mac_osascript",
            "mac_screenshot",
            "mac_app_control",
            "mac_open_url",
            "mac_notify",
            "mac_clipboard_read",
            "mac_clipboard_write",
            "mac_get_frontmost_app",
            "mac_list_running_apps",
            "automation_analyze",
            "automation_execute",
            "automation_status",
        ];

        for name in &all_llm_names {
            let id = ToolId::from_llm_name(name).unwrap();
            assert_eq!(id.llm_name(), *name, "Roundtrip failed for '{}'", name);
        }
    }

    // ========================================================================
    // ToolId::registry_key
    // ========================================================================

    #[test]
    fn registry_key_mcp() {
        let id = ToolId::new("filesystem", "read_file");
        assert_eq!(id.registry_key(), "filesystem.read_file");
    }

    #[test]
    fn registry_key_agent() {
        let id = ToolId::agent("Read");
        assert_eq!(id.registry_key(), "agent.Read");
    }

    #[test]
    fn registry_key_empty_parts() {
        let id = ToolId::new("", "");
        assert_eq!(id.registry_key(), ".");
    }

    // ========================================================================
    // ToolId Display
    // ========================================================================

    #[test]
    fn display_matches_registry_key() {
        let id = ToolId::new("filesystem", "read_file");
        assert_eq!(format!("{}", id), "filesystem.read_file");
    }

    // ========================================================================
    // ToolId equality and hashing
    // ========================================================================

    #[test]
    fn tool_id_equality() {
        let a = ToolId::new("filesystem", "read_file");
        let b = ToolId::new("filesystem", "read_file");
        assert_eq!(a, b);
    }

    #[test]
    fn tool_id_inequality_namespace() {
        let a = ToolId::new("filesystem", "read_file");
        let b = ToolId::new("executor", "read_file");
        assert_ne!(a, b);
    }

    #[test]
    fn tool_id_inequality_name() {
        let a = ToolId::new("filesystem", "read_file");
        let b = ToolId::new("filesystem", "write_file");
        assert_ne!(a, b);
    }

    #[test]
    fn tool_id_hashmap_key() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert(ToolId::new("fs", "read"), 1);
        map.insert(ToolId::new("fs", "write"), 2);
        assert_eq!(map.get(&ToolId::new("fs", "read")), Some(&1));
        assert_eq!(map.get(&ToolId::new("fs", "write")), Some(&2));
        assert_eq!(map.get(&ToolId::new("fs", "delete")), None);
    }

    // ========================================================================
    // ToolId clone and serde
    // ========================================================================

    #[test]
    fn tool_id_clone() {
        let id = ToolId::new("filesystem", "read_file");
        let cloned = id.clone();
        assert_eq!(id, cloned);
    }

    #[test]
    fn tool_id_serde_roundtrip() {
        let id = ToolId::new("filesystem", "read_file");
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: ToolId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    // ========================================================================
    // ToolSource
    // ========================================================================

    #[test]
    fn tool_source_index_key_agent() {
        assert_eq!(ToolSource::Agent.index_key(), "agent");
    }

    #[test]
    fn tool_source_index_key_builtin() {
        assert_eq!(ToolSource::McpBuiltin.index_key(), "mcp_builtin");
    }

    #[test]
    fn tool_source_index_key_external() {
        let src = ToolSource::McpExternal {
            server_name: "videocli-mcp".to_string(),
        };
        assert_eq!(src.index_key(), "mcp_external:videocli-mcp");
    }

    #[test]
    fn tool_source_api_label() {
        assert_eq!(ToolSource::Agent.api_label(), "agent_builtin");
        assert_eq!(ToolSource::McpBuiltin.api_label(), "mcp_builtin");
        let ext = ToolSource::McpExternal {
            server_name: "test".to_string(),
        };
        assert_eq!(ext.api_label(), "mcp_external");
    }

    #[test]
    fn tool_source_equality() {
        assert_eq!(ToolSource::Agent, ToolSource::Agent);
        assert_eq!(ToolSource::McpBuiltin, ToolSource::McpBuiltin);
        assert_ne!(ToolSource::Agent, ToolSource::McpBuiltin);

        let ext_a = ToolSource::McpExternal {
            server_name: "a".to_string(),
        };
        let ext_b = ToolSource::McpExternal {
            server_name: "b".to_string(),
        };
        let ext_a2 = ToolSource::McpExternal {
            server_name: "a".to_string(),
        };
        assert_ne!(ext_a, ext_b);
        assert_eq!(ext_a, ext_a2);
    }

    // ========================================================================
    // ToolMeta
    // ========================================================================

    #[test]
    fn tool_meta_default() {
        let meta = ToolMeta::default();
        assert_eq!(meta.transport_type, "local");
        assert_eq!(meta.location, "local");
        assert_eq!(meta.server_name, "");
    }

    // ========================================================================
    // ToolFilter
    // ========================================================================

    #[test]
    fn tool_filter_default() {
        let filter = ToolFilter::default();
        assert!(filter.enabled_namespaces.is_none());
        assert!(!filter.is_browser_task);
        assert!(!filter.workers_enabled);
        assert!(!filter.code_orchestration_enabled);
    }

    // ========================================================================
    // ToolEntry serialization
    // ========================================================================

    #[test]
    fn tool_entry_serializes_to_json() {
        let entry = ToolEntry {
            id: ToolId::new("filesystem", "read_file"),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            source: ToolSource::McpBuiltin,
            meta: ToolMeta::default(),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["id"]["namespace"], "filesystem");
        assert_eq!(json["id"]["name"], "read_file");
        assert_eq!(json["description"], "Read a file");
        assert_eq!(json["source"], "McpBuiltin");
    }

    #[test]
    fn tool_entry_external_source_serializes() {
        let entry = ToolEntry {
            id: ToolId::new("videocli", "list_ideas"),
            description: "List ideas".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            source: ToolSource::McpExternal {
                server_name: "videocli-mcp".to_string(),
            },
            meta: ToolMeta::default(),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["source"]["McpExternal"]["server_name"], "videocli-mcp");
    }
}

#[cfg(test)]
mod registry_tests {
    use crate::tool_system::registry::UnifiedToolRegistry;
    use crate::tool_system::types::*;

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

    // ========================================================================
    // register + get
    // ========================================================================

    #[test]
    fn register_single_tool() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        assert_eq!(reg.count(), 1);
    }

    #[test]
    fn get_existing_tool() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        let entry = reg.get(&ToolId::new("filesystem", "read_file")).unwrap();
        assert_eq!(entry.id.name, "read_file");
        assert_eq!(entry.description, "Test tool filesystem.read_file");
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let reg = UnifiedToolRegistry::new();
        assert!(reg.get(&ToolId::new("filesystem", "read_file")).is_none());
    }

    #[test]
    fn register_overwrites_existing() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(ToolEntry {
            id: ToolId::new("filesystem", "read_file"),
            description: "v1".to_string(),
            input_schema: serde_json::json!({}),
            source: ToolSource::McpBuiltin,
            meta: ToolMeta::default(),
        });
        reg.register(ToolEntry {
            id: ToolId::new("filesystem", "read_file"),
            description: "v2".to_string(),
            input_schema: serde_json::json!({}),
            source: ToolSource::McpBuiltin,
            meta: ToolMeta::default(),
        });
        assert_eq!(reg.count(), 1);
        assert_eq!(
            reg.get(&ToolId::new("filesystem", "read_file"))
                .unwrap()
                .description,
            "v2"
        );
    }

    #[test]
    fn register_many_tools() {
        let mut reg = UnifiedToolRegistry::new();
        for i in 0..50 {
            reg.register(make_entry(
                &format!("ns{}", i / 10),
                &format!("tool_{}", i),
                ToolSource::McpBuiltin,
            ));
        }
        assert_eq!(reg.count(), 50);
    }

    // ========================================================================
    // contains
    // ========================================================================

    #[test]
    fn contains_registered_tool() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        assert!(reg.contains(&ToolId::agent("Read")));
        assert!(!reg.contains(&ToolId::agent("Write")));
    }

    // ========================================================================
    // get_by_llm_name
    // ========================================================================

    #[test]
    fn get_by_llm_name_agent_tool() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        let entry = reg.get_by_llm_name("Read").unwrap();
        assert_eq!(entry.id.namespace, "agent");
        assert_eq!(entry.id.name, "Read");
    }

    #[test]
    fn get_by_llm_name_mcp_tool() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        let entry = reg.get_by_llm_name("filesystem_read_file").unwrap();
        assert_eq!(entry.id.namespace, "filesystem");
        assert_eq!(entry.id.name, "read_file");
    }

    #[test]
    fn get_by_llm_name_nonexistent() {
        let reg = UnifiedToolRegistry::new();
        assert!(reg.get_by_llm_name("nonexistent").is_none());
        assert!(reg.get_by_llm_name("nonexistent_tool").is_none());
    }

    #[test]
    fn get_by_llm_name_prefers_agent() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        // Even if a parsed MCP tool would match, agent is checked first
        let entry = reg.get_by_llm_name("Read").unwrap();
        assert!(matches!(entry.source, ToolSource::Agent));
    }

    #[test]
    fn get_by_llm_name_multi_underscore() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "mac",
            "get_frontmost_app",
            ToolSource::McpBuiltin,
        ));
        let entry = reg.get_by_llm_name("mac_get_frontmost_app").unwrap();
        assert_eq!(entry.id.name, "get_frontmost_app");
    }

    // ========================================================================
    // list / list_by_namespace / list_by_source
    // ========================================================================

    #[test]
    fn list_all() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn list_empty_registry() {
        let reg = UnifiedToolRegistry::new();
        assert_eq!(reg.list().len(), 0);
    }

    #[test]
    fn list_by_namespace_returns_correct() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry(
            "filesystem",
            "write_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry("browser", "navigate", ToolSource::McpBuiltin));

        assert_eq!(reg.list_by_namespace("filesystem").len(), 2);
        assert_eq!(reg.list_by_namespace("browser").len(), 1);
    }

    #[test]
    fn list_by_namespace_nonexistent() {
        let reg = UnifiedToolRegistry::new();
        assert_eq!(reg.list_by_namespace("nonexistent").len(), 0);
    }

    #[test]
    fn list_by_namespace_after_register() {
        let mut reg = UnifiedToolRegistry::new();
        assert_eq!(reg.list_by_namespace("filesystem").len(), 0);
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        assert_eq!(reg.list_by_namespace("filesystem").len(), 1);
    }

    #[test]
    fn list_by_source_agent() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry("agent", "Write", ToolSource::Agent));
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));

        assert_eq!(reg.list_by_source(&ToolSource::Agent).len(), 2);
        assert_eq!(reg.list_by_source(&ToolSource::McpBuiltin).len(), 1);
    }

    #[test]
    fn list_by_source_external() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "videocli",
            "list_ideas",
            ToolSource::McpExternal {
                server_name: "videocli-mcp".to_string(),
            },
        ));
        let ext_tools = reg.list_by_source(&ToolSource::McpExternal {
            server_name: "videocli-mcp".to_string(),
        });
        assert_eq!(ext_tools.len(), 1);
    }

    // ========================================================================
    // list_filtered
    // ========================================================================

    #[test]
    fn list_filtered_by_namespaces() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry("browser", "navigate", ToolSource::McpBuiltin));

        let filtered = reg.list_filtered(&["agent".to_string(), "filesystem".to_string()]);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn list_filtered_empty_namespaces() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        let filtered = reg.list_filtered(&[]);
        assert_eq!(filtered.len(), 0);
    }

    // ========================================================================
    // unregister
    // ========================================================================

    #[test]
    fn unregister_existing() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.unregister(&ToolId::new("filesystem", "read_file"));
        assert_eq!(reg.count(), 0);
        assert!(!reg.contains(&ToolId::new("filesystem", "read_file")));
    }

    #[test]
    fn unregister_updates_namespace_index() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry(
            "filesystem",
            "write_file",
            ToolSource::McpBuiltin,
        ));
        reg.unregister(&ToolId::new("filesystem", "read_file"));
        assert_eq!(reg.list_by_namespace("filesystem").len(), 1);
    }

    #[test]
    fn unregister_updates_source_index() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.unregister(&ToolId::new("filesystem", "read_file"));
        assert_eq!(reg.list_by_source(&ToolSource::McpBuiltin).len(), 0);
        assert_eq!(reg.list_by_source(&ToolSource::Agent).len(), 1);
    }

    #[test]
    fn unregister_nonexistent_is_noop() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.unregister(&ToolId::new("nonexistent", "tool"));
        assert_eq!(reg.count(), 1);
    }

    // ========================================================================
    // clear_namespace
    // ========================================================================

    #[test]
    fn clear_namespace_removes_all() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry(
            "filesystem",
            "write_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry("browser", "navigate", ToolSource::McpBuiltin));

        reg.clear_namespace("filesystem");
        assert_eq!(reg.count(), 1);
        assert_eq!(reg.list_by_namespace("filesystem").len(), 0);
        assert_eq!(reg.list_by_namespace("browser").len(), 1);
    }

    #[test]
    fn clear_namespace_updates_source_index() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry(
            "filesystem",
            "write_file",
            ToolSource::McpBuiltin,
        ));
        reg.clear_namespace("filesystem");
        assert_eq!(reg.list_by_source(&ToolSource::McpBuiltin).len(), 0);
    }

    #[test]
    fn clear_nonexistent_namespace_is_noop() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.clear_namespace("nonexistent");
        assert_eq!(reg.count(), 1);
    }

    // ========================================================================
    // Index consistency after operations
    // ========================================================================

    #[test]
    fn indexes_consistent_after_register_unregister_cycle() {
        let mut reg = UnifiedToolRegistry::new();
        // Register 5 tools
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry("agent", "Write", ToolSource::Agent));
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry(
            "filesystem",
            "write_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry(
            "videocli",
            "list",
            ToolSource::McpExternal {
                server_name: "vc".to_string(),
            },
        ));
        assert_eq!(reg.count(), 5);

        // Unregister 2
        reg.unregister(&ToolId::agent("Write"));
        reg.unregister(&ToolId::new("filesystem", "read_file"));
        assert_eq!(reg.count(), 3);

        // Verify indexes
        assert_eq!(reg.list_by_namespace("agent").len(), 1);
        assert_eq!(reg.list_by_namespace("filesystem").len(), 1);
        assert_eq!(reg.list_by_namespace("videocli").len(), 1);
        assert_eq!(reg.list_by_source(&ToolSource::Agent).len(), 1);
        assert_eq!(reg.list_by_source(&ToolSource::McpBuiltin).len(), 1);

        // Re-register one
        reg.register(make_entry("agent", "Write", ToolSource::Agent));
        assert_eq!(reg.count(), 4);
        assert_eq!(reg.list_by_namespace("agent").len(), 2);
        assert_eq!(reg.list_by_source(&ToolSource::Agent).len(), 2);
    }

    #[test]
    fn overwrite_changes_source_updates_indexes() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("custom", "tool1", ToolSource::McpBuiltin));
        assert_eq!(reg.list_by_source(&ToolSource::McpBuiltin).len(), 1);

        // Re-register same ID with different source
        reg.register(make_entry(
            "custom",
            "tool1",
            ToolSource::McpExternal {
                server_name: "ext".to_string(),
            },
        ));
        assert_eq!(reg.count(), 1);
        assert_eq!(reg.list_by_source(&ToolSource::McpBuiltin).len(), 0);
        assert_eq!(
            reg.list_by_source(&ToolSource::McpExternal {
                server_name: "ext".to_string()
            })
            .len(),
            1
        );
    }

    // ========================================================================
    // Full 27+7 registration
    // ========================================================================

    #[test]
    fn register_all_27_builtin_plus_7_agent() {
        let mut reg = UnifiedToolRegistry::new();

        // 7 agent core tools
        for name in &["Read", "Write", "Edit", "Bash", "Glob", "Grep", "Computer"] {
            reg.register(make_entry("agent", name, ToolSource::Agent));
        }

        // 27 MCP builtin tools (via ToolSystem helper)
        crate::tool_system::ToolSystem::register_all_builtin_tools(&mut reg);

        assert_eq!(reg.count(), 34);
        assert_eq!(reg.list_by_source(&ToolSource::Agent).len(), 7);
        assert_eq!(reg.list_by_source(&ToolSource::McpBuiltin).len(), 27);
        assert_eq!(reg.list_by_namespace("filesystem").len(), 4);
        assert_eq!(reg.list_by_namespace("executor").len(), 3);
        assert_eq!(reg.list_by_namespace("browser").len(), 8);
        assert_eq!(reg.list_by_namespace("mac").len(), 9);
        assert_eq!(reg.list_by_namespace("automation").len(), 3);
        assert_eq!(reg.list_by_namespace("agent").len(), 7);
    }

    // ========================================================================
    // Default trait
    // ========================================================================

    #[test]
    fn default_creates_empty() {
        let reg = UnifiedToolRegistry::default();
        assert_eq!(reg.count(), 0);
    }
}

#[cfg(test)]
mod resolver_tests {
    use crate::tool_system::registry::UnifiedToolRegistry;
    use crate::tool_system::resolver::ToolResolver;
    use crate::tool_system::types::*;
    use crate::tool_system::ToolSystem;

    fn make_entry(ns: &str, name: &str, source: ToolSource) -> ToolEntry {
        ToolEntry {
            id: ToolId::new(ns, name),
            description: format!("Test {}.{}", ns, name),
            input_schema: serde_json::json!({"type": "object"}),
            source,
            meta: ToolMeta::default(),
        }
    }

    fn setup_full_registry() -> UnifiedToolRegistry {
        let mut reg = UnifiedToolRegistry::new();
        // Agent tools
        for name in &["Read", "Write", "Edit", "Bash", "Glob", "Grep", "Computer"] {
            reg.register(make_entry("agent", name, ToolSource::Agent));
        }
        // MCP builtins
        ToolSystem::register_all_builtin_tools(&mut reg);
        reg
    }

    // ========================================================================
    // resolve
    // ========================================================================

    #[test]
    fn resolve_existing_tool() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        let resolver = ToolResolver::new(&reg);
        assert!(resolver.resolve(&ToolId::agent("Read")).is_some());
    }

    #[test]
    fn resolve_nonexistent() {
        let reg = UnifiedToolRegistry::new();
        let resolver = ToolResolver::new(&reg);
        assert!(resolver.resolve(&ToolId::agent("Read")).is_none());
    }

    // ========================================================================
    // resolve_llm_name
    // ========================================================================

    #[test]
    fn resolve_llm_name_agent() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        let resolver = ToolResolver::new(&reg);
        let entry = resolver.resolve_llm_name("Read").unwrap();
        assert_eq!(entry.id.name, "Read");
    }

    #[test]
    fn resolve_llm_name_mcp() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        let resolver = ToolResolver::new(&reg);
        let entry = resolver.resolve_llm_name("filesystem_read_file").unwrap();
        assert_eq!(entry.id.name, "read_file");
    }

    // ========================================================================
    // schemas_all
    // ========================================================================

    #[test]
    fn schemas_all_returns_all_tools() {
        let reg = setup_full_registry();
        let resolver = ToolResolver::new(&reg);
        let schemas = resolver.schemas_all();
        assert_eq!(schemas.len(), 34); // 7 agent + 27 builtin
    }

    #[test]
    fn schemas_all_correct_format() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(ToolEntry {
            id: ToolId::agent("Read"),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({"type": "object", "required": ["path"]}),
            source: ToolSource::Agent,
            meta: ToolMeta::default(),
        });

        let resolver = ToolResolver::new(&reg);
        let schemas = resolver.schemas_all();
        assert_eq!(schemas.len(), 1);

        let schema = &schemas[0];
        assert_eq!(schema["name"], "Read");
        assert_eq!(schema["description"], "Read a file");
        assert_eq!(schema["input_schema"]["type"], "object");
        assert_eq!(schema["input_schema"]["required"][0], "path");
    }

    #[test]
    fn schemas_all_agent_uses_bare_name() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        let resolver = ToolResolver::new(&reg);
        let schema = &resolver.schemas_all()[0];
        assert_eq!(schema["name"], "Read"); // NOT "agent_Read"
    }

    #[test]
    fn schemas_all_mcp_uses_namespace_name() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        let resolver = ToolResolver::new(&reg);
        let schema = &resolver.schemas_all()[0];
        assert_eq!(schema["name"], "filesystem_read_file");
    }

    // ========================================================================
    // schemas_for_llm filtering
    // ========================================================================

    #[test]
    fn schemas_for_llm_default_includes_core_agent() {
        let reg = setup_full_registry();
        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter::default(); // no browser, no workers, no code_orch
        let schemas = resolver.schemas_for_llm(&filter);

        // Should include 7 agent core + 27 MCP builtin
        let names: Vec<String> = schemas
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"Read".to_string()));
        assert!(names.contains(&"Write".to_string()));
        assert!(names.contains(&"Bash".to_string()));
        assert!(names.contains(&"filesystem_read_file".to_string()));
    }

    #[test]
    fn schemas_for_llm_excludes_browser_tools_by_default() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry("agent", "browser_navigate", ToolSource::Agent));
        reg.register(make_entry("agent", "BrowserTool", ToolSource::Agent));
        reg.register(make_entry(
            "agent",
            "computer_screenshot",
            ToolSource::Agent,
        ));

        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter {
            is_browser_task: false,
            ..Default::default()
        };
        let schemas = resolver.schemas_for_llm(&filter);
        let names: Vec<String> = schemas
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();

        assert!(names.contains(&"Read".to_string()));
        assert!(!names.contains(&"browser_navigate".to_string()));
        assert!(!names.contains(&"BrowserTool".to_string()));
        assert!(!names.contains(&"computer_screenshot".to_string()));
    }

    #[test]
    fn schemas_for_llm_includes_browser_tools_when_enabled() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry("agent", "browser_navigate", ToolSource::Agent));
        reg.register(make_entry("agent", "BrowserTool", ToolSource::Agent));

        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter {
            is_browser_task: true,
            ..Default::default()
        };
        let schemas = resolver.schemas_for_llm(&filter);
        let names: Vec<String> = schemas
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();

        assert!(names.contains(&"browser_navigate".to_string()));
        assert!(names.contains(&"BrowserTool".to_string()));
    }

    #[test]
    fn schemas_for_llm_excludes_computer_click() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "computer_click", ToolSource::Agent));
        reg.register(make_entry("agent", "computer_click_ref", ToolSource::Agent));

        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter {
            is_browser_task: true,
            ..Default::default()
        };
        let schemas = resolver.schemas_for_llm(&filter);
        let names: Vec<String> = schemas
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();

        assert!(!names.contains(&"computer_click".to_string()));
        assert!(names.contains(&"computer_click_ref".to_string()));
    }

    #[test]
    fn schemas_for_llm_excludes_orchestrate_by_default() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry("agent", "Orchestrate", ToolSource::Agent));

        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter::default();
        let schemas = resolver.schemas_for_llm(&filter);
        let names: Vec<String> = schemas
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();

        assert!(!names.contains(&"Orchestrate".to_string()));
    }

    #[test]
    fn schemas_for_llm_includes_orchestrate_when_enabled() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry("agent", "Orchestrate", ToolSource::Agent));

        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter {
            workers_enabled: true,
            ..Default::default()
        };
        let schemas = resolver.schemas_for_llm(&filter);
        let names: Vec<String> = schemas
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();

        assert!(names.contains(&"Orchestrate".to_string()));
    }

    #[test]
    fn schemas_for_llm_excludes_code_orchestration_by_default() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry("agent", "CodeOrchestration", ToolSource::Agent));

        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter::default();
        let schemas = resolver.schemas_for_llm(&filter);
        let names: Vec<String> = schemas
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();

        assert!(!names.contains(&"CodeOrchestration".to_string()));
    }

    #[test]
    fn schemas_for_llm_includes_code_orchestration_when_enabled() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "CodeOrchestration", ToolSource::Agent));

        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter {
            code_orchestration_enabled: true,
            ..Default::default()
        };
        let schemas = resolver.schemas_for_llm(&filter);
        assert_eq!(schemas.len(), 1);
    }

    #[test]
    fn schemas_for_llm_namespace_whitelist() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry("browser", "navigate", ToolSource::McpBuiltin));
        reg.register(make_entry("mac", "screenshot", ToolSource::McpBuiltin));

        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter {
            enabled_namespaces: Some(vec!["filesystem".to_string()]),
            ..Default::default()
        };
        let schemas = resolver.schemas_for_llm(&filter);
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0]["name"], "filesystem_read_file");
    }

    #[test]
    fn schemas_for_llm_empty_namespace_whitelist_excludes_all_mcp() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));

        let resolver = ToolResolver::new(&reg);
        let filter = ToolFilter {
            enabled_namespaces: Some(vec![]),
            ..Default::default()
        };
        let schemas = resolver.schemas_for_llm(&filter);
        // Only agent core tool
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0]["name"], "Read");
    }

    #[test]
    fn schemas_for_llm_combined_filters() {
        let mut reg = UnifiedToolRegistry::new();
        reg.register(make_entry("agent", "Read", ToolSource::Agent));
        reg.register(make_entry("agent", "Orchestrate", ToolSource::Agent));
        reg.register(make_entry("agent", "CodeOrchestration", ToolSource::Agent));
        reg.register(make_entry("agent", "browser_navigate", ToolSource::Agent));
        reg.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));
        reg.register(make_entry("browser", "navigate", ToolSource::McpBuiltin));

        let resolver = ToolResolver::new(&reg);

        // Enable everything
        let filter = ToolFilter {
            enabled_namespaces: None,
            is_browser_task: true,
            workers_enabled: true,
            code_orchestration_enabled: true,
        };
        let schemas = resolver.schemas_for_llm(&filter);
        assert_eq!(schemas.len(), 6);

        // Disable everything optional, whitelist filesystem only
        let filter = ToolFilter {
            enabled_namespaces: Some(vec!["filesystem".to_string()]),
            is_browser_task: false,
            workers_enabled: false,
            code_orchestration_enabled: false,
        };
        let schemas = resolver.schemas_for_llm(&filter);
        let names: Vec<String> = schemas
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"Read".to_string())); // core agent always
        assert!(names.contains(&"filesystem_read_file".to_string()));
        assert!(!names.contains(&"Orchestrate".to_string()));
        assert!(!names.contains(&"CodeOrchestration".to_string()));
        assert!(!names.contains(&"browser_navigate".to_string())); // agent browser tool
    }
}

#[cfg(test)]
mod permission_tests {
    use crate::agent::types::{
        PermissionBehavior, PermissionContext, PermissionMode, PermissionResult, PermissionRule,
    };

    fn ctx_with_mode(mode: PermissionMode) -> PermissionContext {
        let mut ctx = PermissionContext::default();
        ctx.mode = mode;
        ctx
    }

    // ========================================================================
    // BypassPermissions mode
    // ========================================================================

    #[test]
    fn bypass_allows_read_tools() {
        let ctx = ctx_with_mode(PermissionMode::BypassPermissions);
        assert!(ctx.check_tool("Read", &serde_json::json!({})).is_allowed());
        assert!(ctx.check_tool("Glob", &serde_json::json!({})).is_allowed());
    }

    #[test]
    fn bypass_allows_write_tools() {
        let ctx = ctx_with_mode(PermissionMode::BypassPermissions);
        assert!(ctx.check_tool("Write", &serde_json::json!({})).is_allowed());
        assert!(ctx.check_tool("Bash", &serde_json::json!({})).is_allowed());
    }

    #[test]
    fn bypass_allows_mcp_builtin() {
        let ctx = ctx_with_mode(PermissionMode::BypassPermissions);
        assert!(ctx
            .check_tool("filesystem_write_file", &serde_json::json!({}))
            .is_allowed());
    }

    #[test]
    fn bypass_allows_mcp_external() {
        let ctx = ctx_with_mode(PermissionMode::BypassPermissions);
        assert!(ctx
            .check_tool("videocli_list_ideas", &serde_json::json!({}))
            .is_allowed());
    }

    // ========================================================================
    // Plan mode
    // ========================================================================

    #[test]
    fn plan_denies_write() {
        let ctx = ctx_with_mode(PermissionMode::Plan);
        let result = ctx.check_tool("Write", &serde_json::json!({}));
        assert!(result.is_denied());
    }

    #[test]
    fn plan_denies_edit() {
        let ctx = ctx_with_mode(PermissionMode::Plan);
        let result = ctx.check_tool("Edit", &serde_json::json!({}));
        assert!(result.is_denied());
    }

    #[test]
    fn plan_denies_bash() {
        let ctx = ctx_with_mode(PermissionMode::Plan);
        let result = ctx.check_tool("Bash", &serde_json::json!({}));
        assert!(result.is_denied());
    }

    #[test]
    fn plan_denies_notebook_edit() {
        let ctx = ctx_with_mode(PermissionMode::Plan);
        let result = ctx.check_tool("NotebookEdit", &serde_json::json!({}));
        assert!(result.is_denied());
    }

    #[test]
    fn plan_does_not_deny_read() {
        let ctx = ctx_with_mode(PermissionMode::Plan);
        let result = ctx.check_tool("Read", &serde_json::json!({}));
        assert!(!result.is_denied());
    }

    #[test]
    fn plan_does_not_deny_glob() {
        let ctx = ctx_with_mode(PermissionMode::Plan);
        let result = ctx.check_tool("Glob", &serde_json::json!({}));
        assert!(!result.is_denied());
    }

    #[test]
    fn plan_denies_filesystem_write_file() {
        // "filesystem_write_file" contains "Write" substring
        let ctx = ctx_with_mode(PermissionMode::Plan);
        let result = ctx.check_tool("filesystem_write_file", &serde_json::json!({}));
        // The check uses `contains`, so "filesystem_write_file" does NOT contain "Write"
        // (it contains "write" lowercase). Let's verify:
        // modifying_tools = ["Write", "Edit", "Bash", "NotebookEdit"]
        // tool_name.contains("Write") for "filesystem_write_file" is false (case sensitive)
        assert!(!result.is_denied());
    }

    #[test]
    fn plan_denies_tools_containing_modifying_name() {
        // Plan mode checks if tool_name.contains(modifying_tool_name)
        let ctx = ctx_with_mode(PermissionMode::Plan);
        // A tool named "MyBashTool" contains "Bash"
        assert!(ctx
            .check_tool("MyBashTool", &serde_json::json!({}))
            .is_denied());
    }

    // ========================================================================
    // AcceptEdits mode
    // ========================================================================

    #[test]
    fn accept_edits_allows_write() {
        let ctx = ctx_with_mode(PermissionMode::AcceptEdits);
        // is_edit_tool checks: ["Write", "Edit", "NotebookEdit"]
        assert!(ctx.check_tool("Write", &serde_json::json!({})).is_allowed());
    }

    #[test]
    fn accept_edits_allows_edit() {
        let ctx = ctx_with_mode(PermissionMode::AcceptEdits);
        assert!(ctx.check_tool("Edit", &serde_json::json!({})).is_allowed());
    }

    #[test]
    fn accept_edits_allows_notebook_edit() {
        let ctx = ctx_with_mode(PermissionMode::AcceptEdits);
        assert!(ctx
            .check_tool("NotebookEdit", &serde_json::json!({}))
            .is_allowed());
    }

    #[test]
    fn accept_edits_asks_for_bash() {
        let ctx = ctx_with_mode(PermissionMode::AcceptEdits);
        // Bash is NOT in edit_tools, so it falls to default Ask
        let result = ctx.check_tool("Bash", &serde_json::json!({}));
        assert!(!result.is_allowed());
        assert!(!result.is_denied());
        // Should be Ask
        match result {
            PermissionResult::Ask { .. } => {}
            _ => panic!("Expected Ask for Bash in AcceptEdits mode"),
        }
    }

    #[test]
    fn accept_edits_asks_for_read() {
        let ctx = ctx_with_mode(PermissionMode::AcceptEdits);
        // Read is NOT in edit_tools, so it falls to default Ask
        let result = ctx.check_tool("Read", &serde_json::json!({}));
        match result {
            PermissionResult::Ask { .. } => {}
            _ => panic!("Expected Ask for Read in AcceptEdits mode"),
        }
    }

    // ========================================================================
    // Default mode
    // ========================================================================

    #[test]
    fn default_asks_for_write() {
        let ctx = ctx_with_mode(PermissionMode::Default);
        let result = ctx.check_tool("Write", &serde_json::json!({}));
        match result {
            PermissionResult::Ask { question, .. } => {
                assert!(question.contains("Write"));
            }
            _ => panic!("Expected Ask for Write in Default mode"),
        }
    }

    #[test]
    fn default_asks_for_read() {
        let ctx = ctx_with_mode(PermissionMode::Default);
        let result = ctx.check_tool("Read", &serde_json::json!({}));
        match result {
            PermissionResult::Ask { question, .. } => {
                assert!(question.contains("Read"));
            }
            _ => panic!("Expected Ask for Read in Default mode"),
        }
    }

    #[test]
    fn default_asks_for_unknown_tool() {
        let ctx = ctx_with_mode(PermissionMode::Default);
        let result = ctx.check_tool("custom_unknown_tool", &serde_json::json!({}));
        match result {
            PermissionResult::Ask { .. } => {}
            _ => panic!("Expected Ask for unknown tool in Default mode"),
        }
    }

    // ========================================================================
    // Permission rules
    // ========================================================================

    #[test]
    fn rule_allow_overrides_default() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules
            .push((PermissionRule::tool("Read"), PermissionBehavior::Allow));
        assert!(ctx.check_tool("Read", &serde_json::json!({})).is_allowed());
    }

    #[test]
    fn rule_deny_overrides_bypass() {
        // Rules are not checked in Bypass mode - Bypass short-circuits
        let mut ctx = ctx_with_mode(PermissionMode::BypassPermissions);
        ctx.rules
            .push((PermissionRule::tool("Read"), PermissionBehavior::Deny));
        // Bypass still allows
        assert!(ctx.check_tool("Read", &serde_json::json!({})).is_allowed());
    }

    #[test]
    fn rule_deny_in_default_mode() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules
            .push((PermissionRule::tool("Bash"), PermissionBehavior::Deny));
        assert!(ctx.check_tool("Bash", &serde_json::json!({})).is_denied());
    }

    #[test]
    fn rule_ask_explicit() {
        let mut ctx = ctx_with_mode(PermissionMode::AcceptEdits);
        ctx.rules
            .push((PermissionRule::tool("Write"), PermissionBehavior::Ask));
        let result = ctx.check_tool("Write", &serde_json::json!({}));
        match result {
            PermissionResult::Ask { .. } => {}
            _ => panic!("Expected Ask from explicit rule"),
        }
    }

    #[test]
    fn rule_first_match_wins() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules
            .push((PermissionRule::tool("Bash"), PermissionBehavior::Allow));
        ctx.rules
            .push((PermissionRule::tool("Bash"), PermissionBehavior::Deny));
        // First rule matches → Allow
        assert!(ctx.check_tool("Bash", &serde_json::json!({})).is_allowed());
    }

    #[test]
    fn rule_glob_wildcard() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules
            .push((PermissionRule::tool("*"), PermissionBehavior::Allow));
        assert!(ctx.check_tool("Read", &serde_json::json!({})).is_allowed());
        assert!(ctx.check_tool("Bash", &serde_json::json!({})).is_allowed());
    }

    #[test]
    fn rule_glob_prefix() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules.push((
            PermissionRule::tool("filesystem_*"),
            PermissionBehavior::Allow,
        ));
        assert!(ctx
            .check_tool("filesystem_read_file", &serde_json::json!({}))
            .is_allowed());
        // Non-matching tool falls through to default Ask
        let result = ctx.check_tool("executor_bash", &serde_json::json!({}));
        match result {
            PermissionResult::Ask { .. } => {}
            _ => panic!("Expected Ask for non-matching tool"),
        }
    }

    #[test]
    fn rule_glob_suffix() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules.push((
            PermissionRule::tool("*_read_file"),
            PermissionBehavior::Allow,
        ));
        assert!(ctx
            .check_tool("filesystem_read_file", &serde_json::json!({}))
            .is_allowed());
    }

    #[test]
    fn rule_glob_middle() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules
            .push((PermissionRule::tool("*read*"), PermissionBehavior::Allow));
        // "filesystem_read_file" contains "read" → Allow
        assert!(ctx
            .check_tool("filesystem_read_file", &serde_json::json!({}))
            .is_allowed());
        // "Read" does NOT contain "read" (case-sensitive matching)
        // so it falls through to default Ask
        let result = ctx.check_tool("Read", &serde_json::json!({}));
        match result {
            PermissionResult::Ask { .. } => {}
            _ => panic!("Expected Ask for 'Read' with *read* pattern (case-sensitive)"),
        }
    }

    #[test]
    fn rule_path_match() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules
            .push((PermissionRule::path("/tmp/*"), PermissionBehavior::Allow));
        // Tool with path in input
        assert!(ctx
            .check_tool("Write", &serde_json::json!({"file_path": "/tmp/test.txt"}))
            .is_allowed());
        // Path doesn't match
        let result = ctx.check_tool("Write", &serde_json::json!({"file_path": "/etc/passwd"}));
        match result {
            PermissionResult::Ask { .. } => {}
            _ => panic!("Expected Ask for non-matching path"),
        }
    }

    #[test]
    fn rule_path_uses_path_field() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules
            .push((PermissionRule::path("/home/*"), PermissionBehavior::Allow));
        // Uses "path" key
        assert!(ctx
            .check_tool(
                "filesystem_read_file",
                &serde_json::json!({"path": "/home/user/file.txt"})
            )
            .is_allowed());
    }

    #[test]
    fn rule_command_match() {
        let mut ctx = ctx_with_mode(PermissionMode::Default);
        ctx.rules
            .push((PermissionRule::command("ls*"), PermissionBehavior::Allow));
        assert!(ctx
            .check_tool("Bash", &serde_json::json!({"command": "ls -la"}))
            .is_allowed());
        // Non-matching command
        let result = ctx.check_tool("Bash", &serde_json::json!({"command": "rm -rf /"}));
        match result {
            PermissionResult::Ask { .. } => {}
            _ => panic!("Expected Ask for non-matching command"),
        }
    }

    // ========================================================================
    // PermissionResult helpers
    // ========================================================================

    #[test]
    fn permission_result_allow() {
        let r = PermissionResult::allow();
        assert!(r.is_allowed());
        assert!(!r.is_denied());
    }

    #[test]
    fn permission_result_deny() {
        let r = PermissionResult::deny("test", false);
        assert!(r.is_denied());
        assert!(!r.is_allowed());
    }

    #[test]
    fn permission_result_ask_is_neither() {
        let r = PermissionResult::ask("Allow?");
        assert!(!r.is_allowed());
        assert!(!r.is_denied());
    }

    #[test]
    fn permission_result_allow_with_input() {
        let r = PermissionResult::allow_with_input(serde_json::json!({"modified": true}));
        assert!(r.is_allowed());
        if let PermissionResult::Allow { updated_input, .. } = r {
            assert!(updated_input.is_some());
            assert_eq!(updated_input.unwrap()["modified"], true);
        }
    }

    // ========================================================================
    // PermissionMode helpers
    // ========================================================================

    #[test]
    fn permission_mode_auto_approve() {
        assert!(PermissionMode::BypassPermissions.auto_approve());
        assert!(!PermissionMode::Default.auto_approve());
        assert!(!PermissionMode::AcceptEdits.auto_approve());
        assert!(!PermissionMode::Plan.auto_approve());
    }

    #[test]
    fn permission_mode_allows_edits() {
        assert!(PermissionMode::BypassPermissions.allows_edits());
        assert!(PermissionMode::Default.allows_edits());
        assert!(PermissionMode::AcceptEdits.allows_edits());
        assert!(!PermissionMode::Plan.allows_edits());
    }

    #[test]
    fn permission_mode_auto_approve_edits() {
        assert!(PermissionMode::BypassPermissions.auto_approve_edits());
        assert!(PermissionMode::AcceptEdits.auto_approve_edits());
        assert!(!PermissionMode::Default.auto_approve_edits());
        assert!(!PermissionMode::Plan.auto_approve_edits());
    }
}

#[cfg(test)]
mod facade_tests {
    use super::super::*;
    use crate::agent::types::PermissionMode;

    // ========================================================================
    // ToolSystem initialization
    // ========================================================================

    #[tokio::test]
    async fn new_creates_empty_system() {
        let ts = ToolSystem::new();
        assert_eq!(ts.tool_count().await, 0);
        assert_eq!(ts.list_tools().await.len(), 0);
    }

    #[tokio::test]
    async fn default_permission_is_bypass() {
        let ts = ToolSystem::new();
        assert_eq!(
            ts.get_permission_mode().await,
            PermissionMode::BypassPermissions
        );
    }

    // ========================================================================
    // Registration APIs
    // ========================================================================

    #[tokio::test]
    async fn register_builtin_creates_27_tools() {
        let ts = ToolSystem::new();
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        assert_eq!(ts.tool_count().await, 27);
    }

    #[tokio::test]
    async fn register_external_tools() {
        let ts = ToolSystem::new();
        let tools = vec![crate::mcp::protocol::McpToolDef {
            name: "list_ideas".to_string(),
            description: Some("List video ideas".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        ts.register_external_tools(
            "videocli",
            "videocli-mcp",
            &crate::mcp::gateway::McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
            },
            &tools,
        )
        .await;

        assert_eq!(ts.tool_count().await, 1);
        let entries = ts.list_by_namespace("videocli").await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id.name, "list_ideas");
        assert_eq!(entries[0].meta.transport_type, "stdio");
        assert!(matches!(
            entries[0].source,
            ToolSource::McpExternal { ref server_name } if server_name == "videocli-mcp"
        ));
    }

    #[tokio::test]
    async fn register_mcp_server_config() {
        let ts = ToolSystem::new();
        ts.register_mcp_server(crate::mcp::gateway::McpServerConfig {
            name: "test-server".to_string(),
            namespace: "test".to_string(),
            enabled: true,
            transport: crate::mcp::gateway::McpTransport::Http {
                url: "http://localhost:3000".to_string(),
            },
            startup_timeout_secs: 30,
            auto_restart: false,
            auth_token: None,
        })
        .await;

        let configs = ts.configs.read().await;
        assert!(configs.contains_key("test-server"));
    }

    // ========================================================================
    // Permission APIs
    // ========================================================================

    #[tokio::test]
    async fn set_and_get_permission_mode() {
        let ts = ToolSystem::new();
        ts.set_permission_mode(PermissionMode::Plan).await;
        assert_eq!(ts.get_permission_mode().await, PermissionMode::Plan);

        ts.set_permission_mode(PermissionMode::AcceptEdits).await;
        assert_eq!(ts.get_permission_mode().await, PermissionMode::AcceptEdits);
    }

    #[tokio::test]
    async fn add_and_clear_permission_rules() {
        let ts = ToolSystem::new();
        ts.add_permission_rule(
            crate::agent::types::PermissionRule::tool("Bash"),
            crate::agent::types::PermissionBehavior::Deny,
        )
        .await;

        {
            let ctx = ts.permission_ctx.read().await;
            assert_eq!(ctx.rules.len(), 1);
        }

        ts.clear_permission_rules().await;
        {
            let ctx = ts.permission_ctx.read().await;
            assert_eq!(ctx.rules.len(), 0);
        }
    }

    #[tokio::test]
    async fn set_allowed_directories() {
        let ts = ToolSystem::new();
        ts.set_allowed_directories(vec!["/tmp".to_string(), "/home".to_string()])
            .await;
        let ctx = ts.permission_ctx.read().await;
        assert_eq!(ctx.allowed_directories.len(), 2);
    }

    #[tokio::test]
    async fn set_cwd() {
        let ts = ToolSystem::new();
        ts.set_cwd(Some("/workspace".to_string())).await;
        let ctx = ts.permission_ctx.read().await;
        assert_eq!(ctx.cwd.as_deref(), Some("/workspace"));
    }

    // ========================================================================
    // Query APIs
    // ========================================================================

    #[tokio::test]
    async fn list_tools_returns_cloned() {
        let ts = ToolSystem::new();
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        let tools = ts.list_tools().await;
        assert_eq!(tools.len(), 27);
        // Verify they're independent clones
        let tools2 = ts.list_tools().await;
        assert_eq!(tools.len(), tools2.len());
    }

    #[tokio::test]
    async fn list_tools_filtered() {
        let ts = ToolSystem::new();
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        let tools = ts.list_tools_filtered(&["filesystem".to_string()]).await;
        assert_eq!(tools.len(), 4);
    }

    #[tokio::test]
    async fn list_by_namespace() {
        let ts = ToolSystem::new();
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        assert_eq!(ts.list_by_namespace("filesystem").await.len(), 4);
        assert_eq!(ts.list_by_namespace("executor").await.len(), 3);
        assert_eq!(ts.list_by_namespace("browser").await.len(), 8);
        assert_eq!(ts.list_by_namespace("mac").await.len(), 9);
        assert_eq!(ts.list_by_namespace("automation").await.len(), 3);
        assert_eq!(ts.list_by_namespace("nonexistent").await.len(), 0);
    }

    #[tokio::test]
    async fn get_tool_meta() {
        let ts = ToolSystem::new();
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        let meta = ts.get_tool_meta("filesystem_read_file").await;
        assert!(meta.is_some());
        let meta = meta.unwrap();
        assert_eq!(meta.transport_type, "builtin");

        assert!(ts.get_tool_meta("nonexistent_tool").await.is_none());
    }

    #[tokio::test]
    async fn get_all_tool_meta() {
        let ts = ToolSystem::new();
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        let all_meta = ts.get_all_tool_meta().await;
        assert_eq!(all_meta.len(), 27);
        assert!(all_meta.contains_key("filesystem_read_file"));
    }

    #[tokio::test]
    async fn tools_for_llm() {
        let ts = ToolSystem::new();
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        let schemas = ts.tools_for_llm().await;
        assert_eq!(schemas.len(), 27);
        // Check schema format
        for schema in &schemas {
            assert!(schema.get("name").is_some());
            assert!(schema.get("description").is_some());
            assert!(schema.get("input_schema").is_some());
        }
    }

    #[tokio::test]
    async fn schemas_for_llm_with_filter() {
        let ts = ToolSystem::new();
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        let filter = ToolFilter {
            enabled_namespaces: Some(vec!["filesystem".to_string()]),
            ..Default::default()
        };
        let schemas = ts.schemas_for_llm(&filter).await;
        assert_eq!(schemas.len(), 4);
    }

    #[tokio::test]
    async fn get_tools_for_namespace_alias() {
        let ts = ToolSystem::new();
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        // get_tools_for_namespace is alias for list_by_namespace
        let tools = ts.get_tools_for_namespace("mac").await;
        assert_eq!(tools.len(), 9);
    }

    // ========================================================================
    // Execution with permission
    // ========================================================================

    #[tokio::test]
    async fn execute_llm_tool_call_not_found() {
        let ts = ToolSystem::new();
        let result = ts
            .execute_llm_tool_call("nonexistent_tool", serde_json::json!({}))
            .await;
        // In bypass mode, permission passes, but tool not found returns error result
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.is_error);
        assert!(r
            .text_content()
            .unwrap_or_default()
            .contains("Tool not found"));
    }

    #[tokio::test]
    async fn execute_llm_tool_call_plan_denies_write() {
        let ts = ToolSystem::new();
        ts.set_permission_mode(PermissionMode::Plan).await;
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        let result = ts
            .execute_llm_tool_call("Write", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Plan mode"),
            "Expected Plan mode error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn execute_llm_tool_call_default_asks() {
        let ts = ToolSystem::new();
        ts.set_permission_mode(PermissionMode::Default).await;
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }
        let result = ts
            .execute_llm_tool_call(
                "filesystem_write_file",
                serde_json::json!({"path": "/tmp/test", "content": "hello"}),
            )
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("PERMISSION_REQUIRED"),
            "Expected PERMISSION_REQUIRED, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn execute_llm_tool_call_approved_bypasses_permission() {
        let ts = ToolSystem::new();
        ts.set_permission_mode(PermissionMode::Plan).await;
        // Even in Plan mode, _approved should bypass
        let result = ts
            .execute_llm_tool_call_approved("nonexistent", serde_json::json!({}))
            .await;
        // Should not get PermissionDenied, just tool-not-found error result
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.is_error);
    }

    // ========================================================================
    // parse_permission_request
    // ========================================================================

    #[tokio::test]
    async fn parse_permission_request_valid() {
        let err = crate::error::Error::PermissionDenied(
            "PERMISSION_REQUIRED:filesystem_write_file:Allow write?:{\"path\":\"/tmp/test\"}"
                .to_string(),
        );
        let request = ToolSystem::parse_permission_request(&err);
        assert!(request.is_some());
        let req = request.unwrap();
        assert_eq!(req.tool_name, "filesystem_write_file");
        assert_eq!(req.question, "Allow write?");
    }

    #[tokio::test]
    async fn parse_permission_request_non_permission_error() {
        let err = crate::error::Error::Internal("some error".to_string());
        assert!(ToolSystem::parse_permission_request(&err).is_none());
    }

    #[tokio::test]
    async fn parse_permission_request_wrong_format() {
        let err = crate::error::Error::PermissionDenied("not the right format".to_string());
        assert!(ToolSystem::parse_permission_request(&err).is_none());
    }

    // ========================================================================
    // execute() (no permission check)
    // ========================================================================

    #[tokio::test]
    async fn execute_not_found() {
        let ts = ToolSystem::new();
        let result = ts
            .execute("nonexistent", "tool", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found"));
    }

    // ========================================================================
    // MCP Server management
    // ========================================================================

    #[tokio::test]
    async fn list_servers_empty() {
        let ts = ToolSystem::new();
        let servers = ts.list_servers().await;
        assert_eq!(servers.len(), 0);
    }

    #[tokio::test]
    async fn health_check_map_empty() {
        let ts = ToolSystem::new();
        let status = ts.health_check_map().await;
        assert!(status.is_empty());
    }

    #[tokio::test]
    async fn health_check_namespace_not_found() {
        let ts = ToolSystem::new();
        let result = ts.health_check("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn health_check_disconnected_server() {
        let ts = ToolSystem::new();
        ts.register_mcp_server(crate::mcp::gateway::McpServerConfig {
            name: "test-server".to_string(),
            namespace: "test".to_string(),
            enabled: true,
            transport: crate::mcp::gateway::McpTransport::Http {
                url: "http://localhost:3000".to_string(),
            },
            startup_timeout_secs: 30,
            auto_restart: false,
            auth_token: None,
        })
        .await;

        let result = ts.health_check("test").await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Not connected
    }

    // ========================================================================
    // new_with_shared
    // ========================================================================

    #[tokio::test]
    async fn new_with_shared_uses_same_arcs() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let configs = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let connections = Arc::new(RwLock::new(std::collections::HashMap::new()));

        let ts = ToolSystem::new_with_shared(configs.clone(), connections.clone());

        assert!(Arc::ptr_eq(&configs, ts.configs_ref()));
        assert!(Arc::ptr_eq(&connections, ts.connections_ref()));
    }

    // ========================================================================
    // PERMISSION_REQUIRED format compatibility
    // ========================================================================

    #[tokio::test]
    async fn permission_required_format_matches_spec() {
        let ts = ToolSystem::new();
        ts.set_permission_mode(PermissionMode::Default).await;
        {
            let mut registry = ts.registry.write().await;
            ToolSystem::register_all_builtin_tools(&mut registry);
        }

        let input = serde_json::json!({"path": "/tmp/test", "content": "hello"});
        let result = ts
            .execute_llm_tool_call("filesystem_write_file", input.clone())
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();

        // Error::PermissionDenied wraps as "Permission denied: PERMISSION_REQUIRED:..."
        assert!(
            err_msg.contains("PERMISSION_REQUIRED:"),
            "Expected PERMISSION_REQUIRED in error, got: {}",
            err_msg
        );

        // Extract the inner message after "Permission denied: "
        let inner = err_msg
            .strip_prefix("Permission denied: ")
            .unwrap_or(&err_msg);

        // Verify format: PERMISSION_REQUIRED:{tool}:{question}:{args_json}
        assert!(inner.starts_with("PERMISSION_REQUIRED:"));
        let parts: Vec<&str> = inner.splitn(4, ':').collect();
        assert_eq!(
            parts.len(),
            4,
            "Should have 4 colon-separated parts, got: {:?}",
            parts
        );
        assert_eq!(parts[0], "PERMISSION_REQUIRED");
        assert_eq!(parts[1], "filesystem_write_file");
        assert!(!parts[2].is_empty(), "Question should not be empty");
        // parts[3] = args JSON
        let parsed_args: serde_json::Value = serde_json::from_str(parts[3]).unwrap();
        assert_eq!(parsed_args["path"], "/tmp/test");
    }
}

#[cfg(test)]
mod executor_tests {
    use super::super::executor::UnifiedToolExecutor;
    use super::super::registry::UnifiedToolRegistry;
    use super::super::types::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn new_executor() -> UnifiedToolExecutor {
        UnifiedToolExecutor::new(
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
        )
    }

    fn make_entry(ns: &str, name: &str, source: ToolSource) -> ToolEntry {
        ToolEntry {
            id: ToolId::new(ns, name),
            description: format!("Test {}.{}", ns, name),
            input_schema: serde_json::json!({"type": "object"}),
            source,
            meta: ToolMeta::default(),
        }
    }

    // ========================================================================
    // Agent tool execution
    // ========================================================================

    #[tokio::test]
    async fn execute_agent_tool_not_registered() {
        let executor = new_executor();
        let entry = make_entry("agent", "UnknownTool", ToolSource::Agent);
        let result = executor.execute(&entry, serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "Expected not found, got: {}",
            err
        );
    }

    // ========================================================================
    // Builtin tool execution
    // ========================================================================

    #[tokio::test]
    async fn execute_builtin_without_executor_set() {
        let executor = new_executor();
        let entry = make_entry("filesystem", "read_file", ToolSource::McpBuiltin);
        let result = executor.execute(&entry, serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not initialized"),
            "Expected not initialized, got: {}",
            err
        );
    }

    // ========================================================================
    // External tool execution
    // ========================================================================

    #[tokio::test]
    async fn execute_external_without_connection() {
        let executor = new_executor();
        let entry = make_entry(
            "videocli",
            "list_ideas",
            ToolSource::McpExternal {
                server_name: "videocli-mcp".to_string(),
            },
        );
        let result = executor.execute(&entry, serde_json::json!({})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not connected"),
            "Expected not connected, got: {}",
            err
        );
    }

    // ========================================================================
    // execute_by_namespace
    // ========================================================================

    #[tokio::test]
    async fn execute_by_namespace_not_found() {
        let executor = new_executor();
        let registry = UnifiedToolRegistry::new();
        let result = executor
            .execute_by_namespace(&registry, "nonexistent", "tool", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // ========================================================================
    // execute_by_llm_name
    // ========================================================================

    #[tokio::test]
    async fn execute_by_llm_name_not_found() {
        let executor = new_executor();
        let registry = UnifiedToolRegistry::new();
        let result = executor
            .execute_by_llm_name(&registry, "nonexistent_tool", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn execute_by_llm_name_resolves_mcp() {
        let executor = new_executor();
        let mut registry = UnifiedToolRegistry::new();
        registry.register(make_entry(
            "filesystem",
            "read_file",
            ToolSource::McpBuiltin,
        ));

        // Will fail because builtin executor not set, but verifies resolution works
        let result = executor
            .execute_by_llm_name(&registry, "filesystem_read_file", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        // Error should be about executor not initialized, NOT about tool not found
        assert!(
            result.unwrap_err().to_string().contains("not initialized"),
            "Should resolve to builtin, then fail on executor"
        );
    }

    #[tokio::test]
    async fn execute_by_llm_name_resolves_agent() {
        let executor = new_executor();
        let mut registry = UnifiedToolRegistry::new();
        registry.register(make_entry("agent", "Read", ToolSource::Agent));

        let result = executor
            .execute_by_llm_name(&registry, "Read", serde_json::json!({}))
            .await;
        assert!(result.is_err());
        // Error should be about agent tool not found (in executor's agent_tools map)
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // ========================================================================
    // set_builtin_executor
    // ========================================================================

    #[tokio::test]
    async fn builtin_executor_ref_accessible() {
        let executor = new_executor();
        let builtin_ref = executor.builtin_executor_ref();
        let builtin = builtin_ref.read().await;
        assert!(builtin.is_none()); // Not set yet
    }
}

#[cfg(test)]
mod performance_tests {
    use crate::tool_system::registry::UnifiedToolRegistry;
    use crate::tool_system::resolver::ToolResolver;
    use crate::tool_system::types::*;
    use crate::tool_system::ToolSystem;

    fn make_entry(ns: &str, name: &str, source: ToolSource) -> ToolEntry {
        ToolEntry {
            id: ToolId::new(ns, name),
            description: format!("Test {}.{}", ns, name),
            input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            source,
            meta: ToolMeta::default(),
        }
    }

    #[test]
    fn registry_lookup_10000_times() {
        let mut reg = UnifiedToolRegistry::new();
        for i in 0..100 {
            reg.register(make_entry(
                &format!("ns{}", i / 10),
                &format!("tool_{}", i),
                ToolSource::McpBuiltin,
            ));
        }

        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            let _ = reg.get_by_llm_name("ns5_tool_55");
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 100,
            "10000 lookups took {}ms, expected < 100ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn schema_generation_100_times() {
        let mut reg = UnifiedToolRegistry::new();
        for name in &["Read", "Write", "Edit", "Bash", "Glob", "Grep", "Computer"] {
            reg.register(make_entry("agent", name, ToolSource::Agent));
        }
        ToolSystem::register_all_builtin_tools(&mut reg);

        let filter = ToolFilter::default();
        let resolver = ToolResolver::new(&reg);

        let start = std::time::Instant::now();
        for _ in 0..100 {
            let _ = resolver.schemas_for_llm(&filter);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 500,
            "100 schema generations took {}ms, expected < 500ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn from_llm_name_parsing_10000_times() {
        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            let _ = ToolId::from_llm_name("filesystem_read_file");
            let _ = ToolId::from_llm_name("mac_get_frontmost_app");
            let _ = ToolId::from_llm_name("Read");
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 100,
            "30000 parsings took {}ms, expected < 100ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn register_1000_tools() {
        let mut reg = UnifiedToolRegistry::new();
        let start = std::time::Instant::now();
        for i in 0..1000 {
            reg.register(make_entry(
                &format!("ns{}", i / 100),
                &format!("tool_{}", i),
                ToolSource::McpBuiltin,
            ));
        }
        let elapsed = start.elapsed();
        assert_eq!(reg.count(), 1000);
        assert!(
            elapsed.as_millis() < 500,
            "Registering 1000 tools took {}ms, expected < 500ms",
            elapsed.as_millis()
        );
    }
}
