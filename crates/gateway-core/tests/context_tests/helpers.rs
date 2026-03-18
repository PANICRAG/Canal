//! Helper functions for context hierarchy testing
//!
//! Provides mock context creation and assertion utilities for testing
//! the context hierarchy system.

use serde_json::{json, Value};

/// Create a mock organization context
pub fn create_mock_organization_context() -> Value {
    json!({
        "id": "org-123",
        "name": "Test Organization",
        "rules": {
            "code_style": {
                "language": "rust",
                "formatting": "rustfmt"
            },
            "conventions": {
                "commit_prefix": "[TEST]",
                "branch_pattern": "feature/*"
            }
        }
    })
}

/// Create a mock user context
pub fn create_mock_user_context() -> Value {
    json!({
        "id": "user-456",
        "preferences": {
            "coding_style": {
                "prefer_explicit_types": true,
                "max_line_length": 100
            }
        },
        "memory": []
    })
}

/// Create a mock session context
pub fn create_mock_session_context() -> Value {
    json!({
        "session_id": "sess-789",
        "messages": [],
        "working_files": {},
        "loaded_skills": []
    })
}

/// Create a mock task context
pub fn create_mock_task_context() -> Value {
    json!({
        "task_id": "task-001",
        "description": "Test task",
        "loaded_skills": [],
        "working_memory": {}
    })
}

/// Assert that a string contains all expected substrings
pub fn assert_contains_all(content: &str, expected: &[&str]) {
    for exp in expected {
        assert!(
            content.contains(exp),
            "Expected content to contain '{}', but it was not found.\nContent:\n{}",
            exp,
            content
        );
    }
}

/// Assert that a string does not contain any of the given substrings
pub fn assert_contains_none(content: &str, unexpected: &[&str]) {
    for unexp in unexpected {
        assert!(
            !content.contains(unexp),
            "Expected content NOT to contain '{}', but it was found.\nContent:\n{}",
            unexp,
            content
        );
    }
}
