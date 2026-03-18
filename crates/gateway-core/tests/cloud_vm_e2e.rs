// Browser module removed (CV8: replaced by canal-cv)
#![cfg(feature = "browser-legacy-tests")]

//! End-to-End tests for Cloud VM functionality
//!
//! These tests verify the integration of Cloud VM components including:
//! - VM lifecycle management (VmManager, VmPool)
//! - VM executor for code and browser operations (VmExecutor)
//! - Snapshot management (SnapshotManager)
//! - VNC connection and permissions (VncConfig, VncAccessManager)
//! - Network isolation and security
//! - Error recovery scenarios
//!
//! Run with: `cargo test --package gateway-core --test cloud_vm_e2e`
//!
//! Note: Some tests use mock implementations for Firecracker since
//! actual Firecracker requires root privileges and specific infrastructure.

use gateway_core::browser::cloud::{CloudBrowserConfig, CloudBrowserSession};
use gateway_core::error::{Error, Result};
use gateway_core::vm::{
    BrowserAction, ExecutionContext, ExecutionResult, ExecutionStatus, SnapshotConfig, SnapshotId,
    SnapshotInfo, SnapshotManager, SnapshotState, VmInstance, VmManagerConfig, VmPool,
    VmPoolConfig, VmPoolStats, VmStatus, VncAccessConfig, VncAccessError, VncAccessManager,
    VncConfig, VncInfo, VncPermissionType, VncPermissions, VncState, VncStatus,
};

use chrono::Duration;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

// ============================================================================
// Mock Components for Testing (without real Firecracker)
// ============================================================================

/// Mock VM instance for testing without real Firecracker
#[derive(Debug)]
pub struct MockVmInstance {
    pub id: String,
    pub ip: Ipv4Addr,
    pub port: u16,
    pub vnc_port: u16,
    pub status: VmStatus,
    pub created_at: Instant,
    pub index: u32,
    pub healthy: Arc<AtomicBool>,
}

impl Clone for MockVmInstance {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            ip: self.ip,
            port: self.port,
            vnc_port: self.vnc_port,
            status: self.status,
            created_at: self.created_at,
            index: self.index,
            healthy: Arc::clone(&self.healthy),
        }
    }
}

impl MockVmInstance {
    pub fn new(id: &str, index: u32) -> Self {
        let ip = Ipv4Addr::new(172, 16, index as u8, 2);
        Self {
            id: id.to_string(),
            ip,
            port: 8080,
            vnc_port: 5900,
            status: VmStatus::Running,
            created_at: Instant::now(),
            index,
            healthy: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn to_vm_instance(&self) -> VmInstance {
        VmInstance {
            id: self.id.clone(),
            ip: self.ip,
            port: self.port,
            vnc_port: self.vnc_port,
            status: self.status,
            created_at: self.created_at,
            index: self.index,
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::SeqCst)
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.healthy.store(healthy, Ordering::SeqCst);
    }
}

/// Mock VM Manager for testing without real Firecracker
pub struct MockVmManager {
    pool: Arc<RwLock<Vec<MockVmInstance>>>,
    in_use: Arc<RwLock<Vec<MockVmInstance>>>,
    config: VmManagerConfig,
    vm_counter: AtomicU32,
    total_created: AtomicU64,
    total_destroyed: AtomicU64,
    auto_fail_health_check: AtomicBool,
    startup_delay_ms: AtomicU64,
}

impl MockVmManager {
    pub fn new(config: VmManagerConfig) -> Self {
        Self {
            pool: Arc::new(RwLock::new(Vec::new())),
            in_use: Arc::new(RwLock::new(Vec::new())),
            config,
            vm_counter: AtomicU32::new(0),
            total_created: AtomicU64::new(0),
            total_destroyed: AtomicU64::new(0),
            auto_fail_health_check: AtomicBool::new(false),
            startup_delay_ms: AtomicU64::new(0),
        }
    }

    /// Create a mock VM manager with default configuration
    pub fn new_default() -> Self {
        Self::new(VmManagerConfig::default())
    }

    /// Set whether health checks should automatically fail
    pub fn set_auto_fail_health_check(&self, fail: bool) {
        self.auto_fail_health_check.store(fail, Ordering::SeqCst);
    }

    /// Set startup delay in milliseconds (simulates slow VM boot)
    pub fn set_startup_delay_ms(&self, delay: u64) {
        self.startup_delay_ms.store(delay, Ordering::SeqCst);
    }

    /// Warm the pool with a specified number of VMs
    pub async fn warm_pool(&self, count: usize) -> Result<()> {
        for _ in 0..count {
            let vm = self.create_vm_internal().await?;
            let mut pool = self.pool.write().await;
            pool.push(vm);
        }
        Ok(())
    }

    /// Create a new mock VM
    async fn create_vm_internal(&self) -> Result<MockVmInstance> {
        // Simulate startup delay if configured
        let delay = self.startup_delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }

        let index = self.vm_counter.fetch_add(1, Ordering::SeqCst);
        let vm_id = format!("mock-vm-{}", index);
        let vm = MockVmInstance::new(&vm_id, index);

        self.total_created.fetch_add(1, Ordering::SeqCst);

        Ok(vm)
    }

    /// Acquire a VM from the pool
    pub async fn acquire(&self) -> Result<VmInstance> {
        // Try to get from pool first
        {
            let mut pool = self.pool.write().await;
            if let Some(vm) = pool.pop() {
                let mut in_use = self.in_use.write().await;
                in_use.push(vm.clone());
                return Ok(vm.to_vm_instance());
            }
        }

        // Create a new VM if pool is empty
        let vm = self.create_vm_internal().await?;
        {
            let mut in_use = self.in_use.write().await;
            in_use.push(vm.clone());
        }
        Ok(vm.to_vm_instance())
    }

    /// Release a VM back to the pool
    pub async fn release(&self, instance: VmInstance) -> Result<()> {
        let mut in_use = self.in_use.write().await;
        if let Some(pos) = in_use.iter().position(|vm| vm.id == instance.id) {
            let vm = in_use.remove(pos);

            // Check health before returning to pool
            if !self.auto_fail_health_check.load(Ordering::SeqCst) && vm.is_healthy() {
                let mut pool = self.pool.write().await;
                pool.push(vm);
            } else {
                self.total_destroyed.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    /// Destroy a VM
    pub async fn destroy(&self, vm_id: &str) -> Result<()> {
        // Remove from in_use
        {
            let mut in_use = self.in_use.write().await;
            in_use.retain(|vm| vm.id != vm_id);
        }

        // Remove from pool
        {
            let mut pool = self.pool.write().await;
            pool.retain(|vm| vm.id != vm_id);
        }

        self.total_destroyed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Check VM health
    pub async fn check_health(&self, instance: &VmInstance) -> Result<()> {
        if self.auto_fail_health_check.load(Ordering::SeqCst) {
            return Err(Error::Timeout("Health check failed".to_string()));
        }

        let in_use = self.in_use.read().await;
        if let Some(vm) = in_use.iter().find(|vm| vm.id == instance.id) {
            if vm.is_healthy() {
                return Ok(());
            }
        }

        let pool = self.pool.read().await;
        if let Some(vm) = pool.iter().find(|vm| vm.id == instance.id) {
            if vm.is_healthy() {
                return Ok(());
            }
        }

        Err(Error::Internal("VM unhealthy".to_string()))
    }

    /// Get pool statistics
    pub async fn stats(&self) -> VmPoolStats {
        let pool = self.pool.read().await;
        let in_use = self.in_use.read().await;

        VmPoolStats {
            available: pool.len(),
            in_use: in_use.len(),
            total_created: self.total_created.load(Ordering::SeqCst),
            total_destroyed: self.total_destroyed.load(Ordering::SeqCst),
            total_acquires: 0,
            total_releases: 0,
            failed_health_checks: 0,
        }
    }

    /// Get available pool size
    pub async fn pool_size(&self) -> usize {
        self.pool.read().await.len()
    }

    /// Get in-use count
    pub async fn in_use_count(&self) -> usize {
        self.in_use.read().await.len()
    }

    /// Shutdown all VMs
    pub async fn shutdown_all(&self) -> Result<()> {
        {
            let mut pool = self.pool.write().await;
            self.total_destroyed
                .fetch_add(pool.len() as u64, Ordering::SeqCst);
            pool.clear();
        }
        {
            let mut in_use = self.in_use.write().await;
            self.total_destroyed
                .fetch_add(in_use.len() as u64, Ordering::SeqCst);
            in_use.clear();
        }
        Ok(())
    }

    /// Mark a specific VM as unhealthy
    pub async fn mark_unhealthy(&self, vm_id: &str) {
        let pool = self.pool.read().await;
        for vm in pool.iter() {
            if vm.id == vm_id {
                vm.set_healthy(false);
                return;
            }
        }

        let in_use = self.in_use.read().await;
        for vm in in_use.iter() {
            if vm.id == vm_id {
                vm.set_healthy(false);
                return;
            }
        }
    }
}

/// Mock HTTP server responses for VM API
#[derive(Debug)]
pub struct MockVmApiServer {
    responses: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    fail_next: Arc<AtomicBool>,
    latency_ms: Arc<AtomicU64>,
}

impl MockVmApiServer {
    pub fn new() -> Self {
        Self {
            responses: Arc::new(RwLock::new(HashMap::new())),
            fail_next: Arc::new(AtomicBool::new(false)),
            latency_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn set_response(&self, endpoint: &str, response: serde_json::Value) {
        let mut responses = self.responses.write().await;
        responses.insert(endpoint.to_string(), response);
    }

    pub fn set_fail_next(&self, fail: bool) {
        self.fail_next.store(fail, Ordering::SeqCst);
    }

    pub fn set_latency_ms(&self, latency: u64) {
        self.latency_ms.store(latency, Ordering::SeqCst);
    }

    pub async fn get_response(&self, endpoint: &str) -> Option<serde_json::Value> {
        // Simulate latency
        let latency = self.latency_ms.load(Ordering::SeqCst);
        if latency > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(latency)).await;
        }

        // Check if we should fail
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return None;
        }

        let responses = self.responses.read().await;
        responses.get(endpoint).cloned()
    }
}

/// Mock browser result for testing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockBrowserResult {
    pub request_id: String,
    pub success: bool,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

impl MockBrowserResult {
    pub fn success(request_id: &str, data: serde_json::Value) -> Self {
        Self {
            request_id: request_id.to_string(),
            success: true,
            data: Some(data),
            error: None,
            duration_ms: 100,
        }
    }

    pub fn error(request_id: &str, error: &str) -> Self {
        Self {
            request_id: request_id.to_string(),
            success: false,
            data: None,
            error: Some(error.to_string()),
            duration_ms: 0,
        }
    }
}

// ============================================================================
// VM Lifecycle Tests
// ============================================================================

#[tokio::test]
async fn test_vm_start_and_stop() {
    let manager = MockVmManager::new_default();

    // Acquire a VM (should create one since pool is empty)
    let vm = manager.acquire().await.unwrap();
    assert!(vm.id.starts_with("mock-vm-"));
    assert_eq!(vm.status, VmStatus::Running);
    assert_eq!(vm.port, 8080);
    assert_eq!(vm.vnc_port, 5900);

    // Verify stats
    let stats = manager.stats().await;
    assert_eq!(stats.total_created, 1);
    assert_eq!(stats.in_use, 1);
    assert_eq!(stats.available, 0);

    // Release the VM
    manager.release(vm.clone()).await.unwrap();

    // Verify VM returned to pool
    let stats = manager.stats().await;
    assert_eq!(stats.in_use, 0);
    assert_eq!(stats.available, 1);
    assert_eq!(stats.total_destroyed, 0);

    // Destroy the VM
    manager.destroy(&vm.id).await.unwrap();

    // Verify cleanup
    let stats = manager.stats().await;
    assert_eq!(stats.available, 0);
    assert_eq!(stats.total_destroyed, 1);
}

#[tokio::test]
async fn test_vm_pool_warmup() {
    let config = VmManagerConfig {
        auto_warm: true,
        ..Default::default()
    };
    let manager = MockVmManager::new(config);

    // Warm the pool with 3 VMs
    manager.warm_pool(3).await.unwrap();

    // Verify pool has 3 VMs
    assert_eq!(manager.pool_size().await, 3);
    let stats = manager.stats().await;
    assert_eq!(stats.total_created, 3);
    assert_eq!(stats.available, 3);

    // Acquire a VM (should come from pool)
    let vm = manager.acquire().await.unwrap();
    assert_eq!(manager.pool_size().await, 2);
    assert_eq!(manager.in_use_count().await, 1);

    // Release back to pool
    manager.release(vm).await.unwrap();
    assert_eq!(manager.pool_size().await, 3);
    assert_eq!(manager.in_use_count().await, 0);
}

#[tokio::test]
async fn test_vm_pool_reuse() {
    let manager = MockVmManager::new_default();

    // Create and release multiple VMs
    let vm1 = manager.acquire().await.unwrap();
    let vm1_id = vm1.id.clone();
    manager.release(vm1).await.unwrap();

    // Acquire again - should get the same VM from pool
    let vm2 = manager.acquire().await.unwrap();
    assert_eq!(vm2.id, vm1_id);

    // Only 1 VM should have been created total
    let stats = manager.stats().await;
    assert_eq!(stats.total_created, 1);
}

#[tokio::test]
async fn test_vm_timeout_cleanup() {
    let manager = MockVmManager::new_default();

    // Create a VM and mark it unhealthy
    let vm = manager.acquire().await.unwrap();
    manager.mark_unhealthy(&vm.id).await;

    // Release should destroy instead of returning to pool
    manager.release(vm).await.unwrap();

    // VM should be destroyed, not returned to pool
    let stats = manager.stats().await;
    assert_eq!(stats.available, 0);
    assert_eq!(stats.total_destroyed, 1);
}

#[tokio::test]
async fn test_vm_health_check() {
    let manager = MockVmManager::new_default();

    let vm = manager.acquire().await.unwrap();

    // Health check should pass
    assert!(manager.check_health(&vm).await.is_ok());

    // Mark as unhealthy
    manager.mark_unhealthy(&vm.id).await;

    // Health check should fail
    assert!(manager.check_health(&vm).await.is_err());
}

#[tokio::test]
async fn test_vm_multiple_instances() {
    let manager = MockVmManager::new_default();

    // Acquire multiple VMs
    let vm1 = manager.acquire().await.unwrap();
    let vm2 = manager.acquire().await.unwrap();
    let vm3 = manager.acquire().await.unwrap();

    // All VMs should have unique IDs
    assert_ne!(vm1.id, vm2.id);
    assert_ne!(vm2.id, vm3.id);
    assert_ne!(vm1.id, vm3.id);

    // All should have unique IPs
    assert_ne!(vm1.ip, vm2.ip);
    assert_ne!(vm2.ip, vm3.ip);

    let stats = manager.stats().await;
    assert_eq!(stats.total_created, 3);
    assert_eq!(stats.in_use, 3);
}

#[tokio::test]
async fn test_vm_shutdown_all() {
    let manager = MockVmManager::new_default();

    // Create some VMs (3 in pool initially)
    manager.warm_pool(3).await.unwrap();
    // Acquire 1, leaving 2 in pool
    let _vm = manager.acquire().await.unwrap();

    assert_eq!(manager.pool_size().await, 2);
    assert_eq!(manager.in_use_count().await, 1);

    // Shutdown all
    manager.shutdown_all().await.unwrap();

    // Everything should be gone
    let stats = manager.stats().await;
    assert_eq!(stats.available, 0);
    assert_eq!(stats.in_use, 0);
    // 2 in pool + 1 in use = 3 destroyed during shutdown
    assert_eq!(stats.total_destroyed, 3);
}

// ============================================================================
// Browser Automation Tests (Playwright operations)
// ============================================================================

#[tokio::test]
async fn test_cloud_browser_navigate() {
    // Create a mock VM instance
    let vm = VmInstance {
        id: "test-vm-nav".to_string(),
        ip: Ipv4Addr::new(172, 16, 0, 2),
        port: 8080,
        vnc_port: 5900,
        status: VmStatus::Running,
        created_at: Instant::now(),
        index: 0,
    };

    // Create browser action for navigation
    let navigate = BrowserAction::Navigate {
        url: "https://example.com".to_string(),
        wait_until: "load".to_string(),
        timeout: 30000,
    };

    // Verify action can be serialized
    let json = serde_json::to_string(&navigate).unwrap();
    assert!(json.contains("navigate"));
    assert!(json.contains("example.com"));

    // Verify VM URL format
    assert_eq!(vm.http_url(), "http://172.16.0.2:8080");
}

#[tokio::test]
async fn test_cloud_browser_click_action() {
    let click = BrowserAction::Click {
        selector: "#submit-button".to_string(),
        button: "left".to_string(),
        click_count: 1,
        delay: 0,
    };

    let json = serde_json::to_string(&click).unwrap();
    assert!(json.contains("click"));
    assert!(json.contains("submit-button"));
    assert!(json.contains("left"));
}

#[tokio::test]
async fn test_cloud_browser_fill_action() {
    let fill = BrowserAction::Fill {
        selector: "#email-input".to_string(),
        value: "test@example.com".to_string(),
        timeout: 5000,
    };

    let json = serde_json::to_string(&fill).unwrap();
    assert!(json.contains("fill"));
    assert!(json.contains("email-input"));
    assert!(json.contains("test@example.com"));
}

#[tokio::test]
async fn test_cloud_browser_screenshot_action() {
    let screenshot = BrowserAction::Screenshot {
        full_page: true,
        image_type: "png".to_string(),
        quality: Some(85),
        selector: None,
    };

    let json = serde_json::to_string(&screenshot).unwrap();
    assert!(json.contains("screenshot"));
    assert!(json.contains("full_page"));
    assert!(json.contains("png"));
}

#[tokio::test]
async fn test_cloud_browser_a11y_tree() {
    let snapshot = BrowserAction::Snapshot;

    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(json.contains("snapshot"));

    // Verify the snapshot action is the right type
    match snapshot {
        BrowserAction::Snapshot => (),
        _ => panic!("Expected Snapshot action"),
    }
}

#[tokio::test]
async fn test_browser_action_all_variants() {
    // Test all browser action variants can be serialized
    let actions = vec![
        BrowserAction::Navigate {
            url: "https://test.com".to_string(),
            wait_until: "load".to_string(),
            timeout: 30000,
        },
        BrowserAction::Screenshot {
            full_page: false,
            image_type: "jpeg".to_string(),
            quality: Some(90),
            selector: Some("#main".to_string()),
        },
        BrowserAction::Click {
            selector: "button".to_string(),
            button: "left".to_string(),
            click_count: 2,
            delay: 100,
        },
        BrowserAction::Fill {
            selector: "input".to_string(),
            value: "test".to_string(),
            timeout: 5000,
        },
        BrowserAction::Execute {
            script: "return 1+1".to_string(),
            arg: Some(serde_json::json!({"key": "value"})),
        },
        BrowserAction::Snapshot,
        BrowserAction::Wait {
            selector: "#loaded".to_string(),
            timeout: 10000,
            state: "visible".to_string(),
        },
        BrowserAction::Content,
        BrowserAction::GetCookies { urls: None },
        BrowserAction::AddCookies {
            cookies: vec![serde_json::json!({"name": "test", "value": "123"})],
        },
        BrowserAction::ClearCookies,
        BrowserAction::SetViewport {
            width: 1280,
            height: 720,
        },
        BrowserAction::Back,
        BrowserAction::Forward,
        BrowserAction::Reload {
            wait_until: "networkidle".to_string(),
        },
    ];

    for action in actions {
        let json = serde_json::to_string(&action).unwrap();
        assert!(!json.is_empty());
    }
}

// ============================================================================
// VNC Connection Tests
// ============================================================================

#[tokio::test]
async fn test_vnc_connection_config() {
    let config = VncConfig::enabled()
        .with_port(5901)
        .with_websocket_port(6081)
        .with_password("secret123");

    assert!(config.enabled);
    assert_eq!(config.port, 5901);
    assert_eq!(config.websocket_port, 6081);
    assert_eq!(config.password, Some("secret123".to_string()));

    // Validate configuration
    assert!(config.validate().is_ok());
}

#[tokio::test]
async fn test_vnc_info_generation() {
    let config = VncConfig::enabled().with_password("test");

    let info = VncInfo::new("192.168.1.10", &config, 1920, 1080);

    assert_eq!(info.url, "vnc://192.168.1.10:5900");
    assert_eq!(info.websocket_url, "ws://192.168.1.10:6080");
    assert_eq!(info.novnc_url, "http://192.168.1.10:6080/vnc.html");
    assert_eq!(info.width, 1920);
    assert_eq!(info.height, 1080);
    assert!(info.auth_required);
    assert!(!info.ssl_enabled);
}

#[tokio::test]
async fn test_vnc_info_with_ssl() {
    let config = VncConfig::enabled().with_ssl("/path/to/cert.pem", "/path/to/key.pem");

    let info = VncInfo::new("10.0.0.5", &config, 1280, 720);

    assert_eq!(info.websocket_url, "wss://10.0.0.5:6080");
    assert_eq!(info.novnc_url, "https://10.0.0.5:6080/vnc.html");
    assert!(info.ssl_enabled);
}

#[tokio::test]
async fn test_vnc_status_states() {
    assert_eq!(VncStatus::Stopped.to_string(), "stopped");
    assert_eq!(VncStatus::Starting.to_string(), "starting");
    assert_eq!(VncStatus::Running.to_string(), "running");
    assert_eq!(VncStatus::Unknown.to_string(), "unknown");
    assert_eq!(VncStatus::Error.to_string(), "error");

    let state = VncState {
        status: VncStatus::Running,
        vnc_port: 5900,
        websocket_port: 6080,
        vnc_pid: Some(12345),
        websockify_pid: Some(12346),
        width: 1920,
        height: 1080,
        auth_enabled: true,
        ssl_enabled: false,
        timestamp: None,
        error: None,
    };

    assert!(state.is_running());
    assert!(state.is_available());
}

// ============================================================================
// VNC Permission Token Tests
// ============================================================================

#[tokio::test]
async fn test_vnc_permission_token_create() {
    let mut manager = VncAccessManager::new();

    let token = manager
        .create_token("vm-123", VncPermissions::full_access(), Duration::hours(1))
        .unwrap();

    assert!(!token.token.is_empty());
    assert_eq!(token.vm_id, "vm-123");
    assert!(token.is_valid());
    assert!(!token.is_expired());
    assert!(token.remaining_ttl_secs() > 3500);
}

#[tokio::test]
async fn test_vnc_permission_token_validate() {
    let mut manager = VncAccessManager::new();

    let token = manager
        .create_token_default("vm-456", VncPermissions::full_access())
        .unwrap();

    // Validate token
    let validated = manager.validate_token(&token.token).unwrap();
    assert_eq!(validated.vm_id, "vm-456");

    // Try to validate non-existent token
    let result = manager.validate_token("invalid-token");
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        VncAccessError::TokenNotFound { .. }
    ));
}

#[tokio::test]
async fn test_vnc_permission_token_ttl_clamping() {
    // Note: VncAccessManager clamps TTL to min_ttl_secs (60 seconds) by default
    // So we test that short TTL gets clamped rather than testing expired tokens
    let mut manager = VncAccessManager::new();

    // Create a token with very short TTL - should be clamped to minimum (60 seconds)
    let token = manager
        .create_token("vm-789", VncPermissions::default(), Duration::seconds(1))
        .unwrap();

    // Token should NOT be expired immediately because TTL was clamped
    assert!(!token.is_expired());
    assert!(token.is_valid());

    // Remaining TTL should be around 60 seconds (the minimum)
    let remaining = token.remaining_ttl_secs();
    assert!(remaining >= 55 && remaining <= 60);

    // Token should be valid
    let result = manager.validate_token(&token.token);
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_vnc_permission_revoked_vs_expired() {
    let mut manager = VncAccessManager::new();

    // Create a valid token
    let token = manager
        .create_token(
            "vm-revoke-test",
            VncPermissions::default(),
            Duration::hours(1),
        )
        .unwrap();

    // Token should be valid initially
    assert!(manager.validate_token(&token.token).is_ok());

    // Revoke the token
    assert!(manager.revoke_token(&token.token));

    // Validation should fail with TokenRevoked error
    let result = manager.validate_token(&token.token);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        VncAccessError::TokenRevoked { .. }
    ));
}

#[tokio::test]
async fn test_vnc_permission_token_revocation() {
    let mut manager = VncAccessManager::new();

    let token = manager
        .create_token_default("vm-test", VncPermissions::default())
        .unwrap();

    // Token should be valid
    assert!(manager.validate_token(&token.token).is_ok());

    // Revoke the token
    assert!(manager.revoke_token(&token.token));

    // Token should no longer be valid
    let result = manager.validate_token(&token.token);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        VncAccessError::TokenRevoked { .. }
    ));
}

#[tokio::test]
async fn test_vnc_permission_types() {
    let mut manager = VncAccessManager::new();

    // Create view-only token
    let token = manager
        .create_token_default("vm-view", VncPermissions::view_only())
        .unwrap();

    // Should allow viewing
    assert!(manager
        .check_permission(&token.token, VncPermissionType::View)
        .unwrap());

    // Should deny other operations
    assert!(!manager
        .check_permission(&token.token, VncPermissionType::Keyboard)
        .unwrap());
    assert!(!manager
        .check_permission(&token.token, VncPermissionType::Mouse)
        .unwrap());
    assert!(!manager
        .check_permission(&token.token, VncPermissionType::Clipboard)
        .unwrap());
}

#[tokio::test]
async fn test_vnc_permission_full_access() {
    let mut manager = VncAccessManager::new();

    let token = manager
        .create_token_default("vm-full", VncPermissions::full_access())
        .unwrap();

    // All permissions should be granted
    assert!(manager
        .check_permission(&token.token, VncPermissionType::View)
        .unwrap());
    assert!(manager
        .check_permission(&token.token, VncPermissionType::Keyboard)
        .unwrap());
    assert!(manager
        .check_permission(&token.token, VncPermissionType::Mouse)
        .unwrap());
    assert!(manager
        .check_permission(&token.token, VncPermissionType::Clipboard)
        .unwrap());
}

#[tokio::test]
async fn test_vnc_token_cleanup_expired() {
    let mut manager = VncAccessManager::new();

    // Create a valid token
    manager
        .create_token_default("vm-1", VncPermissions::default())
        .unwrap();

    // Create an expired token (manually manipulate)
    let expired_token = manager
        .create_token("vm-2", VncPermissions::default(), Duration::hours(1))
        .unwrap();

    // Revoke one to simulate cleanup target
    manager.revoke_token(&expired_token.token);

    // Cleanup should remove revoked token
    let cleaned = manager.cleanup_expired();
    assert_eq!(cleaned, 1);

    // Stats should reflect cleanup
    let stats = manager.stats();
    assert_eq!(stats.total_tokens, 1);
    assert_eq!(stats.valid_tokens, 1);
}

#[tokio::test]
async fn test_vnc_per_vm_token_limit() {
    let config = VncAccessConfig::default().with_max_tokens_per_vm(2);
    let mut manager = VncAccessManager::with_config(config);

    // Create 2 tokens for same VM
    manager
        .create_token_default("vm-limit", VncPermissions::default())
        .unwrap();
    manager
        .create_token_default("vm-limit", VncPermissions::default())
        .unwrap();

    // Third should fail
    let result = manager.create_token_default("vm-limit", VncPermissions::default());
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        VncAccessError::TooManyTokensForVm { .. }
    ));
}

// ============================================================================
// Snapshot Tests
// ============================================================================

#[tokio::test]
async fn test_snapshot_config_defaults() {
    let config = SnapshotConfig::default();

    assert_eq!(config.max_snapshots, 100);
    assert_eq!(config.max_age_secs, 86400); // 24 hours
    assert!(!config.enable_diff_snapshots);
    assert!(config.validate_on_create);
    assert!(config.validate().is_ok());
}

#[tokio::test]
async fn test_snapshot_config_builder() {
    let config = SnapshotConfig::new()
        .with_max_snapshots(50)
        .with_max_age_secs(3600)
        .with_diff_snapshots(true);

    assert_eq!(config.max_snapshots, 50);
    assert_eq!(config.max_age_secs, 3600);
    assert!(config.enable_diff_snapshots);
}

#[tokio::test]
async fn test_snapshot_info_creation() {
    let snapshot_id = SnapshotId::from_string("snap-test-1");
    let info = SnapshotInfo::new(
        snapshot_id.clone(),
        "vm-source-1",
        std::path::PathBuf::from("/tmp/memory.bin"),
        std::path::PathBuf::from("/tmp/state.bin"),
    );

    assert_eq!(info.id, "snap-test-1");
    assert_eq!(info.vm_id, "vm-source-1");
    assert_eq!(info.state, SnapshotState::Creating);
    assert!(!info.is_ready());
    assert_eq!(info.size_bytes, 0);
}

#[tokio::test]
async fn test_snapshot_state_transitions() {
    let mut info = SnapshotInfo::new(
        SnapshotId::new(),
        "vm-1",
        std::path::PathBuf::from("/tmp/mem"),
        std::path::PathBuf::from("/tmp/state"),
    );

    // Initial state
    assert_eq!(info.state, SnapshotState::Creating);
    assert!(!info.is_ready());

    // Transition to ready
    info.state = SnapshotState::Ready;
    assert!(info.is_ready());

    // Transition to restoring
    info.state = SnapshotState::Restoring;
    assert!(!info.is_ready());

    // State display
    assert_eq!(SnapshotState::Creating.to_string(), "creating");
    assert_eq!(SnapshotState::Ready.to_string(), "ready");
    assert_eq!(SnapshotState::Restoring.to_string(), "restoring");
    assert_eq!(SnapshotState::Invalid.to_string(), "invalid");
    assert_eq!(SnapshotState::Deleting.to_string(), "deleting");
}

#[tokio::test]
async fn test_snapshot_id_uniqueness() {
    let id1 = SnapshotId::new();
    let id2 = SnapshotId::new();
    let id3 = SnapshotId::new();

    assert_ne!(id1.as_str(), id2.as_str());
    assert_ne!(id2.as_str(), id3.as_str());
    assert_ne!(id1.as_str(), id3.as_str());
}

#[tokio::test]
async fn test_snapshot_manager_creation() {
    let config = SnapshotConfig::default();
    let manager = SnapshotManager::new("/tmp/test-snapshots", config);

    assert_eq!(
        manager.storage_path(),
        std::path::Path::new("/tmp/test-snapshots")
    );
    assert_eq!(manager.count().await, 0);
    assert_eq!(manager.total_size_bytes().await, 0);
}

#[tokio::test]
async fn test_snapshot_config_validation() {
    // Valid config
    let valid = SnapshotConfig::default();
    assert!(valid.validate().is_ok());

    // Invalid: max_snapshots = 0
    let invalid1 = SnapshotConfig {
        max_snapshots: 0,
        ..Default::default()
    };
    assert!(invalid1.validate().is_err());

    // Invalid: max_age_secs = 0
    let invalid2 = SnapshotConfig {
        max_age_secs: 0,
        ..Default::default()
    };
    assert!(invalid2.validate().is_err());
}

// ============================================================================
// Network Isolation Tests
// ============================================================================

#[tokio::test]
async fn test_vm_network_ip_allocation() {
    let config = VmPoolConfig::default();
    let pool = VmPool::new(config);

    // Test IP allocation for multiple VMs
    let ip0 = pool.ip_for_index(0);
    let ip1 = pool.ip_for_index(1);
    let ip2 = pool.ip_for_index(2);

    assert_eq!(ip0, Ipv4Addr::new(172, 16, 0, 2));
    assert_eq!(ip1, Ipv4Addr::new(172, 16, 1, 2));
    assert_eq!(ip2, Ipv4Addr::new(172, 16, 2, 2));

    // Test gateway allocation
    let gw0 = pool.gateway_for_index(0);
    let gw1 = pool.gateway_for_index(1);

    assert_eq!(gw0, Ipv4Addr::new(172, 16, 0, 1));
    assert_eq!(gw1, Ipv4Addr::new(172, 16, 1, 1));
}

#[tokio::test]
async fn test_vm_network_tap_devices() {
    let pool = VmPool::new(VmPoolConfig::default());

    assert_eq!(pool.tap_for_index(0), "tap0");
    assert_eq!(pool.tap_for_index(5), "tap5");
    assert_eq!(pool.tap_for_index(100), "tap100");
}

#[tokio::test]
async fn test_vm_network_index_reuse() {
    let pool = VmPool::new(VmPoolConfig::default());

    // Allocate some indices
    let idx0 = pool.allocate_index().await;
    let idx1 = pool.allocate_index().await;
    let idx2 = pool.allocate_index().await;

    assert_eq!(idx0, 0);
    assert_eq!(idx1, 1);
    assert_eq!(idx2, 2);

    // Release idx1
    pool.release_index(idx1).await;

    // Next allocation should reuse idx1
    let idx3 = pool.allocate_index().await;
    assert_eq!(idx3, 1);
}

#[tokio::test]
async fn test_vm_instance_url_formats() {
    let vm = VmInstance {
        id: "vm-url-test".to_string(),
        ip: Ipv4Addr::new(172, 16, 5, 2),
        port: 8080,
        vnc_port: 5905,
        status: VmStatus::Running,
        created_at: Instant::now(),
        index: 5,
    };

    assert_eq!(vm.http_url(), "http://172.16.5.2:8080");
    assert_eq!(vm.vnc_url(), "vnc://172.16.5.2:5905");
}

// ============================================================================
// Error Recovery Tests
// ============================================================================

#[tokio::test]
async fn test_vm_crash_recovery_unhealthy() {
    let manager = MockVmManager::new_default();

    // Create and warm pool
    manager.warm_pool(2).await.unwrap();

    // Acquire a VM
    let vm = manager.acquire().await.unwrap();

    // Simulate crash by marking unhealthy
    manager.mark_unhealthy(&vm.id).await;

    // Health check should fail
    assert!(manager.check_health(&vm).await.is_err());

    // Release should destroy the unhealthy VM
    manager.release(vm).await.unwrap();

    // Pool should not have the unhealthy VM
    let stats = manager.stats().await;
    assert_eq!(stats.total_destroyed, 1);
}

#[tokio::test]
async fn test_vm_connection_retry_on_failure() {
    let manager = MockVmManager::new_default();

    // Set health checks to fail
    manager.set_auto_fail_health_check(true);

    let vm = manager.acquire().await.unwrap();

    // Health check should fail
    let result = manager.check_health(&vm).await;
    assert!(result.is_err());

    // Re-enable health checks
    manager.set_auto_fail_health_check(false);

    // Now health check should pass
    let result = manager.check_health(&vm).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_vm_pool_recovery_after_failure() {
    let manager = MockVmManager::new_default();

    // Warm pool
    manager.warm_pool(3).await.unwrap();
    let initial_pool_size = manager.pool_size().await;
    assert_eq!(initial_pool_size, 3);

    // Acquire a VM
    let vm = manager.acquire().await.unwrap();
    assert_eq!(manager.pool_size().await, 2);
    assert_eq!(manager.in_use_count().await, 1);

    // Mark the VM unhealthy
    manager.mark_unhealthy(&vm.id).await;

    // Release should destroy unhealthy VM instead of returning to pool
    manager.release(vm).await.unwrap();

    // Pool should have 2 VMs (the unhealthy one was destroyed)
    let final_pool_size = manager.pool_size().await;
    assert_eq!(final_pool_size, 2);

    // Stats should show 1 destroyed
    let stats = manager.stats().await;
    assert_eq!(stats.total_destroyed, 1);
}

#[tokio::test]
async fn test_execution_context_defaults() {
    let context = ExecutionContext::default();

    assert_eq!(context.timeout_ms, 30000);
    assert!(context.capture_output);
    assert!(!context.sandbox_mode);
    assert!(context.env_vars.is_empty());
    assert!(context.working_dir.is_none());
    assert!(context.allowed_imports.is_none());
}

#[tokio::test]
async fn test_execution_context_customization() {
    let mut context = ExecutionContext::default();
    context.timeout_ms = 60000;
    context.sandbox_mode = true;
    context.working_dir = Some("/tmp/workspace".to_string());
    context
        .env_vars
        .insert("MY_VAR".to_string(), "value".to_string());
    context.allowed_imports = Some(vec!["os".to_string(), "sys".to_string()]);

    assert_eq!(context.timeout_ms, 60000);
    assert!(context.sandbox_mode);
    assert_eq!(context.working_dir, Some("/tmp/workspace".to_string()));
    assert_eq!(context.env_vars.get("MY_VAR"), Some(&"value".to_string()));
    assert_eq!(context.allowed_imports.unwrap().len(), 2);
}

#[tokio::test]
async fn test_execution_result_serialization() {
    let result = ExecutionResult {
        execution_id: "exec-123".to_string(),
        success: true,
        stdout: "Hello, World!".to_string(),
        stderr: "".to_string(),
        return_value: Some(serde_json::json!(42)),
        duration_ms: 150,
        exit_code: 0,
        error: None,
        captured_vars: HashMap::new(),
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("exec-123"));
    assert!(json.contains("Hello, World!"));
    assert!(json.contains("42"));

    let deserialized: ExecutionResult = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.execution_id, "exec-123");
    assert!(deserialized.success);
}

#[tokio::test]
async fn test_execution_status_tracking() {
    // Test all execution status variants
    let statuses = vec![
        ExecutionStatus::Pending,
        ExecutionStatus::Running,
        ExecutionStatus::Completed,
        ExecutionStatus::Failed,
        ExecutionStatus::Cancelled,
        ExecutionStatus::TimedOut,
    ];

    for status in statuses {
        let json = serde_json::to_string(&status).unwrap();
        let deserialized: ExecutionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(
            std::mem::discriminant(&status),
            std::mem::discriminant(&deserialized)
        );
    }
}

// ============================================================================
// VM Status Tests
// ============================================================================

#[test]
fn test_vm_status_display() {
    assert_eq!(VmStatus::Creating.to_string(), "creating");
    assert_eq!(VmStatus::Starting.to_string(), "starting");
    assert_eq!(VmStatus::Running.to_string(), "running");
    assert_eq!(VmStatus::Unhealthy.to_string(), "unhealthy");
    assert_eq!(VmStatus::Stopping.to_string(), "stopping");
    assert_eq!(VmStatus::Stopped.to_string(), "stopped");
    assert_eq!(VmStatus::Error.to_string(), "error");
}

#[test]
fn test_vm_status_serialization() {
    let status = VmStatus::Running;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"running\"");

    let deserialized: VmStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, VmStatus::Running);
}

// ============================================================================
// Pool Configuration Tests
// ============================================================================

#[test]
fn test_pool_config_defaults() {
    let config = VmPoolConfig::default();

    assert_eq!(config.min_pool_size, 2);
    assert_eq!(config.max_pool_size, 10);
    assert_eq!(config.max_vm_age_secs, 3600);
    assert_eq!(config.health_check_interval_secs, 30);
    assert_eq!(config.api_port, 8080);
    assert_eq!(config.vnc_port, 5900);
}

#[test]
fn test_pool_config_custom() {
    let config = VmPoolConfig {
        min_pool_size: 5,
        max_pool_size: 20,
        max_vm_age_secs: 7200,
        health_check_interval_secs: 60,
        base_subnet: Ipv4Addr::new(10, 0, 0, 0),
        api_port: 9090,
        vnc_port: 5901,
    };

    assert_eq!(config.min_pool_size, 5);
    assert_eq!(config.max_pool_size, 20);
    assert_eq!(config.base_subnet, Ipv4Addr::new(10, 0, 0, 0));
    assert_eq!(config.api_port, 9090);
}

#[tokio::test]
async fn test_pool_ip_allocation() {
    let pool = VmPool::new(VmPoolConfig::default());

    // Test IP allocation for different indices
    let ip0 = pool.ip_for_index(0);
    let ip1 = pool.ip_for_index(1);
    let ip2 = pool.ip_for_index(2);

    assert_eq!(ip0, Ipv4Addr::new(172, 16, 0, 2));
    assert_eq!(ip1, Ipv4Addr::new(172, 16, 1, 2));
    assert_eq!(ip2, Ipv4Addr::new(172, 16, 2, 2));

    // Test gateway allocation
    let gw0 = pool.gateway_for_index(0);
    let gw1 = pool.gateway_for_index(1);

    assert_eq!(gw0, Ipv4Addr::new(172, 16, 0, 1));
    assert_eq!(gw1, Ipv4Addr::new(172, 16, 1, 1));
}

#[tokio::test]
async fn test_pool_tap_device_naming() {
    let pool = VmPool::new(VmPoolConfig::default());

    assert_eq!(pool.tap_for_index(0), "tap0");
    assert_eq!(pool.tap_for_index(5), "tap5");
    assert_eq!(pool.tap_for_index(100), "tap100");
}

#[tokio::test]
async fn test_pool_index_allocation() {
    let pool = VmPool::new(VmPoolConfig::default());

    // Allocate sequential indices
    let idx0 = pool.allocate_index().await;
    let idx1 = pool.allocate_index().await;
    let idx2 = pool.allocate_index().await;

    assert_eq!(idx0, 0);
    assert_eq!(idx1, 1);
    assert_eq!(idx2, 2);

    // Release an index
    pool.release_index(idx1).await;

    // Next allocation should reuse the released index
    let idx3 = pool.allocate_index().await;
    assert_eq!(idx3, 1);
}

#[tokio::test]
async fn test_pool_stats_initial() {
    let pool = VmPool::new(VmPoolConfig::default());

    let stats = pool.stats().await;

    assert_eq!(stats.available, 0);
    assert_eq!(stats.in_use, 0);
    assert_eq!(stats.total_created, 0);
    assert_eq!(stats.total_destroyed, 0);
    assert_eq!(stats.total_acquires, 0);
    assert_eq!(stats.total_releases, 0);
}

#[tokio::test]
async fn test_pool_counts_initial() {
    let pool = VmPool::new(VmPoolConfig::default());

    assert_eq!(pool.available_count().await, 0);
    assert_eq!(pool.in_use_count().await, 0);
    assert_eq!(pool.total_count().await, 0);
}

#[tokio::test]
async fn test_pool_replenishment_check() {
    let config = VmPoolConfig {
        min_pool_size: 3,
        ..Default::default()
    };
    let pool = VmPool::new(config);

    // Initially needs replenishment since pool is empty
    assert!(pool.needs_replenishment().await);
}

// ============================================================================
// VNC Config Error Tests
// ============================================================================

#[test]
fn test_vnc_config_validation_errors() {
    // Port conflict
    let config = VncConfig::new().with_port(5900).with_websocket_port(5900);
    let result = config.validate();
    assert!(result.is_err());

    // SSL without cert
    let config = VncConfig {
        ssl_enabled: true,
        ssl_cert_path: None,
        ssl_key_path: Some("/path/to/key".to_string()),
        ..Default::default()
    };
    let result = config.validate();
    assert!(result.is_err());

    // SSL without key
    let config = VncConfig {
        ssl_enabled: true,
        ssl_cert_path: Some("/path/to/cert".to_string()),
        ssl_key_path: None,
        ..Default::default()
    };
    let result = config.validate();
    assert!(result.is_err());
}

// ============================================================================
// VNC Resize Tests
// ============================================================================

#[test]
fn test_vnc_resize_validation() {
    use gateway_core::vm::VncResizeRequest;

    // Valid resize
    let valid = VncResizeRequest::new(1920, 1080);
    assert!(valid.validate().is_ok());

    // Too small width
    let small_width = VncResizeRequest::new(320, 1080);
    assert!(small_width.validate().is_err());

    // Too small height
    let small_height = VncResizeRequest::new(1920, 240);
    assert!(small_height.validate().is_err());

    // Too large
    let too_large = VncResizeRequest::new(8000, 4000);
    assert!(too_large.validate().is_err());
}

// ============================================================================
// Cloud Browser Session Tests
// ============================================================================

#[tokio::test]
async fn test_cloud_browser_session_creation() {
    let vm = VmInstance {
        id: "vm-session-test".to_string(),
        ip: Ipv4Addr::new(172, 16, 0, 2),
        port: 8080,
        vnc_port: 5900,
        status: VmStatus::Running,
        created_at: Instant::now(),
        index: 0,
    };

    let session = CloudBrowserSession::new("session-1".to_string(), &vm, true);

    assert_eq!(session.session_id, "session-1");
    assert_eq!(session.vm_id, "vm-session-test");
    assert_eq!(session.browser_url, "http://172.16.0.2:8080/api/browser");
    assert_eq!(session.vnc_url, Some("vnc://172.16.0.2:5900".to_string()));
    assert!(session.current_url.is_none());
    assert!(session.metadata.is_empty());
}

#[tokio::test]
async fn test_cloud_browser_session_without_vnc() {
    let vm = VmInstance {
        id: "vm-no-vnc".to_string(),
        ip: Ipv4Addr::new(172, 16, 1, 2),
        port: 8080,
        vnc_port: 5900,
        status: VmStatus::Running,
        created_at: Instant::now(),
        index: 1,
    };

    let session = CloudBrowserSession::new("session-2".to_string(), &vm, false);

    assert!(session.vnc_url.is_none());
}

#[tokio::test]
async fn test_cloud_browser_session_touch() {
    let vm = VmInstance {
        id: "vm-touch".to_string(),
        ip: Ipv4Addr::new(172, 16, 2, 2),
        port: 8080,
        vnc_port: 5900,
        status: VmStatus::Running,
        created_at: Instant::now(),
        index: 2,
    };

    let mut session = CloudBrowserSession::new("session-3".to_string(), &vm, true);
    let initial_activity = session.last_activity;

    // Wait a tiny bit
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    session.touch();

    assert!(session.last_activity >= initial_activity);
}

#[tokio::test]
async fn test_cloud_browser_session_serialization() {
    let vm = VmInstance {
        id: "vm-serial".to_string(),
        ip: Ipv4Addr::new(172, 16, 3, 2),
        port: 8080,
        vnc_port: 5900,
        status: VmStatus::Running,
        created_at: Instant::now(),
        index: 3,
    };

    let session = CloudBrowserSession::new("session-4".to_string(), &vm, true);

    let json = serde_json::to_string(&session).unwrap();
    assert!(json.contains("session-4"));
    assert!(json.contains("vm-serial"));
    assert!(json.contains("browser_url"));

    let deserialized: CloudBrowserSession = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.session_id, session.session_id);
    assert_eq!(deserialized.vm_id, session.vm_id);
}

// ============================================================================
// Cloud Browser Config Tests
// ============================================================================

#[test]
fn test_cloud_browser_config_defaults() {
    let config = CloudBrowserConfig::default();

    assert_eq!(config.timeout_ms, 30_000);
    assert_eq!(config.max_sessions, 10);
    assert_eq!(config.idle_timeout_ms, 300_000);
    assert!(config.enable_vnc);
    assert_eq!(config.viewport_width, 1920);
    assert_eq!(config.viewport_height, 1080);
    assert_eq!(config.health_check_interval_secs, 60);
    assert_eq!(config.max_retries, 3);
    assert_eq!(config.retry_delay_ms, 1000);
}

#[test]
fn test_cloud_browser_config_customization() {
    let mut config = CloudBrowserConfig::default();
    config.timeout_ms = 60_000;
    config.max_sessions = 20;
    config.enable_vnc = false;
    config.viewport_width = 1280;
    config.viewport_height = 720;

    assert_eq!(config.timeout_ms, 60_000);
    assert_eq!(config.max_sessions, 20);
    assert!(!config.enable_vnc);
    assert_eq!(config.viewport_width, 1280);
    assert_eq!(config.viewport_height, 720);
}

// ============================================================================
// Comprehensive Integration Scenario Tests
// ============================================================================

#[tokio::test]
async fn test_full_vm_lifecycle_scenario() {
    // This test simulates a complete VM lifecycle with browser automation

    let manager = MockVmManager::new_default();

    // 1. Warm the pool
    manager.warm_pool(2).await.unwrap();
    assert_eq!(manager.pool_size().await, 2);

    // 2. Acquire a VM
    let vm = manager.acquire().await.unwrap();
    assert!(vm.id.starts_with("mock-vm-"));
    assert_eq!(manager.pool_size().await, 1);
    assert_eq!(manager.in_use_count().await, 1);

    // 3. Verify VM is healthy
    assert!(manager.check_health(&vm).await.is_ok());

    // 4. Simulate browser operation (just verify action creation)
    let navigate = BrowserAction::Navigate {
        url: "https://example.com".to_string(),
        wait_until: "load".to_string(),
        timeout: 30000,
    };
    assert!(serde_json::to_string(&navigate).is_ok());

    // 5. Release VM back to pool
    manager.release(vm.clone()).await.unwrap();
    assert_eq!(manager.pool_size().await, 2);
    assert_eq!(manager.in_use_count().await, 0);

    // 6. Shutdown all
    manager.shutdown_all().await.unwrap();
    assert_eq!(manager.pool_size().await, 0);
}

#[tokio::test]
async fn test_vnc_access_full_workflow() {
    let mut manager = VncAccessManager::new();

    // 1. Create a token for a VM
    let token = manager
        .create_token(
            "vm-workflow",
            VncPermissions::full_access(),
            Duration::hours(1),
        )
        .unwrap();

    // 2. Validate the token
    let validated = manager.validate_token(&token.token).unwrap();
    assert_eq!(validated.vm_id, "vm-workflow");

    // 3. Check permissions
    assert!(manager
        .check_permission(&token.token, VncPermissionType::View)
        .unwrap());
    assert!(manager
        .check_permission(&token.token, VncPermissionType::Keyboard)
        .unwrap());

    // 4. Use the token
    let used = manager.use_token(&token.token).unwrap();
    assert_eq!(used.use_count, 1);

    // 5. Extend the token
    manager
        .extend_token(&token.token, Duration::hours(1))
        .unwrap();

    // 6. Revoke the token
    assert!(manager.revoke_token(&token.token));

    // 7. Cleanup
    let cleaned = manager.cleanup_expired();
    assert_eq!(cleaned, 1);

    // 8. Verify stats
    let stats = manager.stats();
    assert_eq!(stats.total_tokens, 0);
}

#[tokio::test]
async fn test_mock_vm_manager_stress_scenario() {
    // Use MockVmManager for stress testing instead of VmPool directly
    // since PoolEntry is not exported from the vm module
    let manager = MockVmManager::new_default();

    // Warm pool with VMs
    manager.warm_pool(5).await.unwrap();
    assert_eq!(manager.pool_size().await, 5);

    // Acquire all VMs
    let mut acquired = Vec::new();
    for _ in 0..5 {
        let vm = manager.acquire().await.unwrap();
        acquired.push(vm);
    }

    assert_eq!(acquired.len(), 5);
    assert_eq!(manager.pool_size().await, 0);
    assert_eq!(manager.in_use_count().await, 5);

    // Release all back
    for vm in acquired {
        manager.release(vm).await.unwrap();
    }

    assert_eq!(manager.pool_size().await, 5);
    assert_eq!(manager.in_use_count().await, 0);
}
