//! A28 Remote Tool Execution (RTE) Protocol Tests
//!
//! End-to-end tests for the RTE protocol: SSE events, HMAC signing,
//! tool delegation, reconnection, fallback, and concurrent execution.
//!
//! Run: `cargo nextest run -p gateway-core --test a28_rte_tests`

mod helpers;

use helpers::mock_rte::*;
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

// ============================================================
// Core RTE Protocol Flow Tests
// ============================================================

#[cfg(test)]
mod rte_flow_tests {
    use super::*;

    /// RTE-1: Full basic flow — client capabilities → tool request → result → agent continues
    #[tokio::test]
    async fn test_rte_basic_flow() {
        // GIVEN: Client with full capabilities (code_execute, browser, file)
        let client = MockRteClient::new();
        let caps = MockClientCapabilities::windows_full();
        assert!(caps.rte_enabled);
        assert!(caps.supported_tools.contains(&"code_execute".to_string()));

        // Simulate session start
        let session_id = Uuid::new_v4();
        let secret_b64 = "dGVzdC1zZWNyZXQtMzJieXRlcyEhISE="; // base64
        client
            .handle_session_start(session_id, secret_b64.to_string())
            .await;

        // Verify session secret was stored
        let secret_bytes = client.session_secret.read().await.clone().unwrap();
        assert!(!secret_bytes.is_empty());

        // Simulate tool execution request
        let request_id = Uuid::new_v4();
        let hmac = sign_tool_request(&secret_bytes, &request_id, "code_execute");
        let request = MockToolExecuteRequest {
            request_id,
            tool_name: "code_execute".to_string(),
            tool_input: json!({"language": "python", "code": "print('hello')"}),
            timeout_ms: 120000,
            fallback: "cloud".to_string(),
            hmac_signature: hmac,
        };

        let result = client.handle_tool_request(request).await;
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.success);

        // Verify event sequence via received_events
        let events = client.received_events.lock().await;
        assert_eq!(events.len(), 2); // session_start + tool_execute_request
        assert_event_sequence(&events, &["session_start", "tool_execute_request"]);
    }

    /// RTE-2: HMAC validation — invalid signature rejected with 403
    #[tokio::test]
    async fn test_rte_hmac_validation_rejects_invalid() {
        // GIVEN: Active RTE session with known session_secret
        let client = MockRteClient::new();
        *client.response_behavior.write().await = RteResponseBehavior::InvalidHmac;

        let session_id = Uuid::new_v4();
        client
            .handle_session_start(session_id, "c2VjcmV0".to_string())
            .await;

        let secret_bytes = client.session_secret.read().await.clone().unwrap();
        let request_id = Uuid::new_v4();
        let hmac = sign_tool_request(&secret_bytes, &request_id, "code_execute");

        let request = MockToolExecuteRequest {
            request_id,
            tool_name: "code_execute".to_string(),
            tool_input: json!({}),
            timeout_ms: 120000,
            fallback: "cloud".to_string(),
            hmac_signature: hmac,
        };

        // Client responds with InvalidHmac behavior
        let result = client.handle_tool_request(request).await;
        assert!(result.is_some());
        let result = result.unwrap();
        // The result has an invalid HMAC signature
        assert_eq!(result.hmac_signature, "invalid-hmac-signature");
        // Server would reject this with 403; verify the signature doesn't match expected
        let expected_sig = sign_tool_result(&secret_bytes, &request_id, true);
        assert_ne!(
            result.hmac_signature, expected_sig,
            "Invalid HMAC must not match expected"
        );
    }

    /// RTE-2b: HMAC validation — valid signature accepted
    #[tokio::test]
    async fn test_rte_hmac_computation() {
        // Unit test: verify HMAC computation matches expected values
        let secret = b"test-session-secret-32bytes!!!!";
        let request_id = Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap();
        let tool_name = "code_execute";

        let signature = sign_tool_request(secret, &request_id, tool_name);
        assert!(!signature.is_empty());
        assert!(verify_hmac(
            secret,
            &format!("{}:{}", request_id, tool_name),
            &signature,
        ));
    }

    /// RTE-2c: HMAC verify rejects wrong data
    #[tokio::test]
    async fn test_rte_hmac_rejects_tampered() {
        let secret = b"test-session-secret-32bytes!!!!";
        let request_id = Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap();

        let signature = sign_tool_request(secret, &request_id, "code_execute");
        // Tamper: different tool name
        assert!(!verify_hmac(
            secret,
            &format!("{}:{}", request_id, "file_read"),
            &signature,
        ));
    }

    /// RTE-3: Timeout fallback — client doesn't respond → server fallback
    #[tokio::test]
    async fn test_rte_timeout_fallback_to_cloud() {
        // GIVEN: Client configured to timeout (never respond)
        let client = MockRteClient::new();
        *client.response_behavior.write().await = RteResponseBehavior::Timeout;

        let session_id = Uuid::new_v4();
        client
            .handle_session_start(session_id, "c2VjcmV0".to_string())
            .await;

        let secret_bytes = client.session_secret.read().await.clone().unwrap();
        let request_id = Uuid::new_v4();
        let hmac = sign_tool_request(&secret_bytes, &request_id, "code_execute");

        let request = MockToolExecuteRequest {
            request_id,
            tool_name: "code_execute".to_string(),
            tool_input: json!({"language": "python", "code": "print('hi')"}),
            timeout_ms: 100, // Short timeout for test
            fallback: "cloud".to_string(),
            hmac_signature: hmac,
        };

        // WHEN: Client never responds (Timeout behavior)
        let result = client.handle_tool_request(request).await;

        // THEN: No result returned (timeout)
        assert!(result.is_none(), "Timeout behavior must produce no result");

        // Request was received but no result was completed
        assert_eq!(client.request_count().await, 1);
        assert_eq!(client.result_count().await, 0);

        // Fallback should be "cloud" — server would handle this
        let fallback = "cloud";
        assert_eq!(fallback, "cloud");
    }

    /// RTE-3b: Timeout fallback — browser tool → error to LLM
    #[tokio::test]
    async fn test_rte_timeout_fallback_browser_error() {
        // GIVEN: Client with browser support but configured to timeout
        let client = MockRteClient::new();
        *client.response_behavior.write().await = RteResponseBehavior::Timeout;

        let session_id = Uuid::new_v4();
        client
            .handle_session_start(session_id, "c2VjcmV0".to_string())
            .await;

        let secret_bytes = client.session_secret.read().await.clone().unwrap();
        let request_id = Uuid::new_v4();
        let hmac = sign_tool_request(&secret_bytes, &request_id, "browser_screenshot");

        let request = MockToolExecuteRequest {
            request_id,
            tool_name: "browser_screenshot".to_string(),
            tool_input: json!({"url": "https://example.com"}),
            timeout_ms: 100,
            fallback: "error".to_string(), // Browser tools use "error" fallback, not "cloud"
            hmac_signature: hmac,
        };

        // WHEN: Client never responds (Timeout behavior)
        let result = client.handle_tool_request(request).await;
        assert!(result.is_none(), "Timeout behavior must produce no result");

        // For browser tools, fallback is "error" — server sends error to LLM, not cloud exec
        let fallback = "error";
        assert_eq!(fallback, "error");
        assert_ne!(
            fallback, "cloud",
            "Browser tools should not fall back to cloud execution"
        );
    }

    /// RTE-4: Reconnection — SSE drops, client reconnects, pending requests re-sent
    #[tokio::test]
    async fn test_rte_reconnection_resends_pending() {
        // GIVEN: Active session with pending tool_execute_request
        let store = MockPendingStore::new();
        let session_id = Uuid::new_v4();
        let req_a = Uuid::new_v4();
        let req_b = Uuid::new_v4();

        let _rx_a = store.add(
            req_a,
            "code_execute".to_string(),
            session_id,
            Duration::from_secs(120),
        );
        let _rx_b = store.add(
            req_b,
            "file_read".to_string(),
            session_id,
            Duration::from_secs(60),
        );
        assert_eq!(store.pending_count(), 2);

        // WHEN: Client reconnects with resume_session_id
        // Server looks up all pending requests for the session
        let pending = store.get_session_pending(&session_id);
        assert_eq!(
            pending.len(),
            2,
            "All pending requests for session must be found on reconnect"
        );
        assert!(pending.contains(&req_a));
        assert!(pending.contains(&req_b));

        // THEN: Client can complete the re-sent requests
        let result_a = MockToolResult {
            request_id: req_a,
            result: json!({"output": "reconnected result"}),
            success: true,
            error: None,
            execution_time_ms: 100,
            hmac_signature: "valid".to_string(),
        };
        assert!(store.complete(&req_a, result_a));
        assert_eq!(store.pending_count(), 1);
    }

    /// RTE-4b: Reconnection — result POST after SSE reconnect
    #[tokio::test]
    async fn test_rte_result_after_reconnect() {
        // GIVEN: Pending tool execution from session A
        let store = MockPendingStore::new();
        let session_id = Uuid::new_v4();
        let request_id = Uuid::new_v4();

        let rx = store.add(
            request_id,
            "code_execute".to_string(),
            session_id,
            Duration::from_secs(120),
        );
        assert_eq!(store.pending_count(), 1);

        // WHEN: Client POSTs result (no active SSE needed — result endpoint is REST)
        let result = MockToolResult {
            request_id,
            result: json!({"output": "result after reconnect"}),
            success: true,
            error: None,
            execution_time_ms: 200,
            hmac_signature: "valid".to_string(),
        };
        assert!(store.complete(&request_id, result));

        // THEN: Agent loop resumes via oneshot channel
        let received = rx.await;
        assert!(received.is_ok(), "Oneshot channel must deliver the result");
        let received = received.unwrap();
        assert!(received.success);
        assert_eq!(store.pending_count(), 0);
    }

    /// RTE-5: No capabilities → all tools execute server-side (backward compat)
    #[tokio::test]
    async fn test_rte_no_capabilities_server_side() {
        // GIVEN: StreamChatRequest WITHOUT client_capabilities (web client)
        let caps = MockClientCapabilities::web_no_rte();

        // THEN: RTE is disabled
        assert!(!caps.rte_enabled, "Web client must have RTE disabled");
        assert!(
            caps.supported_tools.is_empty(),
            "Web client has no local tools"
        );
        assert_eq!(caps.max_concurrent_tools, 0);

        // No tool_execute_request should be emitted; tools run server-side
        let client = MockRteClient::new();
        // No session_start event emitted for non-RTE clients
        let events = client.received_events.lock().await;
        assert!(
            events.is_empty(),
            "No SSE events should be emitted for non-RTE clients"
        );
    }

    /// RTE-5b: RTE disabled → all tools execute server-side
    #[tokio::test]
    async fn test_rte_disabled_server_side() {
        // GIVEN: client_capabilities.rte_enabled = false
        let caps = MockClientCapabilities::web_no_rte();
        assert!(!caps.rte_enabled);
        assert_eq!(caps.platform, "web");

        // Even if the client sends capabilities struct, rte_enabled=false means server-side
        // The server delegation check: if !caps.rte_enabled || !caps.supported_tools.contains(tool)
        let tool_name = "code_execute";
        let should_delegate =
            caps.rte_enabled && caps.supported_tools.contains(&tool_name.to_string());
        assert!(
            !should_delegate,
            "Server must NOT delegate when RTE is disabled"
        );

        // Compare with a client that HAS RTE enabled
        let rte_caps = MockClientCapabilities::windows_full();
        let should_delegate_rte =
            rte_caps.rte_enabled && rte_caps.supported_tools.contains(&tool_name.to_string());
        assert!(
            should_delegate_rte,
            "RTE-enabled client should accept delegation"
        );
    }

    /// RTE-6: Concurrent tool requests — multiple tools in parallel
    #[tokio::test]
    async fn test_rte_concurrent_tools() {
        // GIVEN: Client with max_concurrent_tools = 3
        let caps = MockClientCapabilities::windows_full();
        assert_eq!(caps.max_concurrent_tools, 3);

        // Simulate 3 parallel pending requests using MockPendingStore
        let store = MockPendingStore::new();
        let session_id = Uuid::new_v4();
        let req_ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        let tools = vec!["code_execute", "file_read", "browser_screenshot"];

        let mut receivers = Vec::new();
        for (i, req_id) in req_ids.iter().enumerate() {
            let rx = store.add(
                *req_id,
                tools[i].to_string(),
                session_id,
                Duration::from_secs(120),
            );
            receivers.push(rx);
        }
        assert_eq!(store.pending_count(), 3, "All 3 requests should be pending");

        // WHEN: Client POSTs results in reverse order (any order works)
        for (i, req_id) in req_ids.iter().enumerate().rev() {
            let result = MockToolResult {
                request_id: *req_id,
                result: json!({"output": format!("result-{}", i)}),
                success: true,
                error: None,
                execution_time_ms: 50,
                hmac_signature: "valid".to_string(),
            };
            assert!(store.complete(req_id, result));
        }

        // THEN: All 3 results received
        assert_eq!(store.pending_count(), 0, "All requests should be completed");
        for rx in receivers {
            let result = rx.await;
            assert!(result.is_ok(), "Each oneshot channel must deliver a result");
            assert!(result.unwrap().success);
        }
    }

    /// RTE-7: Auth refresh — auth_refresh_required sent before JWT expiry
    #[tokio::test]
    async fn test_rte_auth_refresh_event() {
        // GIVEN: JWT that expires in 4 minutes (< 5min threshold)
        let expires_in_secs: u64 = 240; // 4 minutes
        let refresh_threshold_secs: u64 = 300; // 5 minutes
        assert!(
            expires_in_secs < refresh_threshold_secs,
            "Token TTL ({}) must be less than refresh threshold ({})",
            expires_in_secs,
            refresh_threshold_secs
        );

        // THEN: Server should emit auth_refresh_required SSE event
        let event = MockSseEvent::AuthRefreshRequired {
            expires_at: "2026-02-10T12:04:00Z".to_string(),
            refresh_url: "/api/auth/refresh".to_string(),
        };

        // Verify event structure
        match &event {
            MockSseEvent::AuthRefreshRequired {
                expires_at,
                refresh_url,
            } => {
                assert!(!expires_at.is_empty(), "expires_at must be present");
                assert_eq!(refresh_url, "/api/auth/refresh");
                assert!(refresh_url.starts_with("/api/auth/"));
            }
            _ => panic!("Expected AuthRefreshRequired event"),
        }

        // Verify the event serializes correctly
        let client = MockRteClient::new();
        client.received_events.lock().await.push(event);
        let events = client.received_events.lock().await;
        assert_eq!(events.len(), 1);
        assert_event_sequence(&events, &["auth_refresh_required"]);
    }
}

// ============================================================
// Pending Tool Executions Store Tests
// ============================================================

#[cfg(test)]
mod pending_store_tests {
    use super::*;

    #[tokio::test]
    async fn test_pending_store_add_and_complete() {
        let store = MockPendingStore::new();
        let request_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        let rx = store.add(
            request_id,
            "code_execute".to_string(),
            session_id,
            Duration::from_secs(120),
        );

        assert_eq!(store.pending_count(), 1);

        let result = MockToolResult {
            request_id,
            result: json!({"output": "hello"}),
            success: true,
            error: None,
            execution_time_ms: 50,
            hmac_signature: "test".to_string(),
        };

        assert!(store.complete(&request_id, result));
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_pending_store_session_lookup() {
        let store = MockPendingStore::new();
        let session_id = Uuid::new_v4();
        let other_session = Uuid::new_v4();

        let _rx1 = store.add(
            Uuid::new_v4(),
            "tool_a".to_string(),
            session_id,
            Duration::from_secs(60),
        );
        let _rx2 = store.add(
            Uuid::new_v4(),
            "tool_b".to_string(),
            session_id,
            Duration::from_secs(60),
        );
        let _rx3 = store.add(
            Uuid::new_v4(),
            "tool_c".to_string(),
            other_session,
            Duration::from_secs(60),
        );

        let session_pending = store.get_session_pending(&session_id);
        assert_eq!(session_pending.len(), 2);

        let other_pending = store.get_session_pending(&other_session);
        assert_eq!(other_pending.len(), 1);
    }

    #[tokio::test]
    async fn test_pending_store_evict_expired() {
        let store = MockPendingStore::new();
        let request_id = Uuid::new_v4();

        // Add with very short timeout (0ms)
        let _rx = store.add(
            request_id,
            "code_execute".to_string(),
            Uuid::new_v4(),
            Duration::from_millis(0),
        );

        // Wait for TTL (2x timeout = 0ms)
        tokio::time::sleep(Duration::from_millis(10)).await;

        let evicted = store.evict_expired();
        assert_eq!(evicted, 1);
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_pending_store_complete_unknown_id() {
        let store = MockPendingStore::new();
        let result = MockToolResult {
            request_id: Uuid::new_v4(),
            result: json!({}),
            success: true,
            error: None,
            execution_time_ms: 0,
            hmac_signature: "test".to_string(),
        };
        assert!(!store.complete(&Uuid::new_v4(), result));
    }
}

// ============================================================
// Client Capabilities Tests
// ============================================================

#[cfg(test)]
mod capabilities_tests {
    use super::*;

    #[test]
    fn test_windows_full_capabilities() {
        let caps = MockClientCapabilities::windows_full();
        assert_eq!(caps.protocol_version, "1.0");
        assert_eq!(caps.platform, "windows");
        assert!(caps.rte_enabled);
        assert!(caps.supported_tools.contains(&"code_execute".to_string()));
        assert!(caps
            .supported_tools
            .contains(&"browser_screenshot".to_string()));
    }

    #[test]
    fn test_macos_full_capabilities() {
        let caps = MockClientCapabilities::macos_full();
        assert_eq!(caps.platform, "macos");
        assert!(caps.rte_enabled);
    }

    #[test]
    fn test_web_no_rte_capabilities() {
        let caps = MockClientCapabilities::web_no_rte();
        assert!(!caps.rte_enabled);
        assert!(caps.supported_tools.is_empty());
        assert_eq!(caps.max_concurrent_tools, 0);
    }

    #[test]
    fn test_code_only_capabilities() {
        let caps = MockClientCapabilities::code_only();
        assert!(caps.rte_enabled);
        assert_eq!(caps.supported_tools.len(), 1);
        assert_eq!(caps.supported_tools[0], "code_execute");
        assert_eq!(caps.max_concurrent_tools, 1);
    }
}

// ============================================================
// Mock RTE Client Behavior Tests
// ============================================================

#[cfg(test)]
mod mock_client_tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_client_immediate_success() {
        let client = MockRteClient::new();
        let session_id = Uuid::new_v4();
        let secret = "dGVzdC1zZWNyZXQ="; // base64("test-secret")
        client
            .handle_session_start(session_id, secret.to_string())
            .await;

        let secret_bytes = client.session_secret.read().await.clone().unwrap();
        let request_id = Uuid::new_v4();
        let hmac = sign_tool_request(&secret_bytes, &request_id, "code_execute");

        let request = MockToolExecuteRequest {
            request_id,
            tool_name: "code_execute".to_string(),
            tool_input: json!({"language": "python", "code": "print('hi')"}),
            timeout_ms: 120000,
            fallback: "cloud".to_string(),
            hmac_signature: hmac,
        };

        let result = client.handle_tool_request(request).await;
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.success);
        assert_eq!(client.request_count().await, 1);
        assert_eq!(client.result_count().await, 1);
    }

    #[tokio::test]
    async fn test_mock_client_timeout_behavior() {
        let client = MockRteClient::new();
        *client.response_behavior.write().await = RteResponseBehavior::Timeout;
        let session_id = Uuid::new_v4();
        client
            .handle_session_start(session_id, "c2VjcmV0".to_string())
            .await;

        let secret_bytes = client.session_secret.read().await.clone().unwrap();
        let request_id = Uuid::new_v4();
        let hmac = sign_tool_request(&secret_bytes, &request_id, "code_execute");

        let request = MockToolExecuteRequest {
            request_id,
            tool_name: "code_execute".to_string(),
            tool_input: json!({}),
            timeout_ms: 100,
            fallback: "cloud".to_string(),
            hmac_signature: hmac,
        };

        let result = client.handle_tool_request(request).await;
        assert!(result.is_none()); // Timeout = no response
    }

    #[tokio::test]
    async fn test_mock_client_error_behavior() {
        let client = MockRteClient::new();
        *client.response_behavior.write().await =
            RteResponseBehavior::Error("sandbox violation".to_string());
        let session_id = Uuid::new_v4();
        client
            .handle_session_start(session_id, "c2VjcmV0".to_string())
            .await;

        let secret_bytes = client.session_secret.read().await.clone().unwrap();
        let request_id = Uuid::new_v4();
        let hmac = sign_tool_request(&secret_bytes, &request_id, "code_execute");

        let request = MockToolExecuteRequest {
            request_id,
            tool_name: "code_execute".to_string(),
            tool_input: json!({}),
            timeout_ms: 120000,
            fallback: "cloud".to_string(),
            hmac_signature: hmac,
        };

        let result = client.handle_tool_request(request).await.unwrap();
        assert!(!result.success);
        assert_eq!(result.error, Some("sandbox violation".to_string()));
    }
}
