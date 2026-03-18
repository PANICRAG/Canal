//! VM Manager
//!
//! High-level VM management layer that orchestrates Firecracker VMs,
//! including lifecycle management, health checking, and command execution.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use crate::vm::client::{FirecrackerClient, FirecrackerConfig, VmConfig};
use crate::vm::pool::{PoolEntry, VmPool, VmPoolConfig, VmPoolStats};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Child;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, warn};

/// VM Manager configuration
#[derive(Debug, Clone)]
pub struct VmManagerConfig {
    /// Firecracker configuration
    pub firecracker: FirecrackerConfig,
    /// VM pool configuration
    pub pool: VmPoolConfig,
    /// Health check timeout in milliseconds
    pub health_check_timeout_ms: u64,
    /// Command execution timeout in milliseconds
    pub exec_timeout_ms: u64,
    /// Number of health check retries
    pub health_check_retries: u32,
    /// Whether to auto-warm the pool on startup
    pub auto_warm: bool,
}

impl Default for VmManagerConfig {
    fn default() -> Self {
        Self {
            firecracker: FirecrackerConfig::default(),
            pool: VmPoolConfig::default(),
            health_check_timeout_ms: 5000,
            exec_timeout_ms: 30000,
            health_check_retries: 3,
            auto_warm: true,
        }
    }
}

/// Status of a VM instance
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VmStatus {
    /// VM is being created
    Creating,
    /// VM is starting
    Starting,
    /// VM is running and healthy
    Running,
    /// VM is unhealthy
    Unhealthy,
    /// VM is stopping
    Stopping,
    /// VM has stopped
    Stopped,
    /// VM has encountered an error
    Error,
}

impl std::fmt::Display for VmStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmStatus::Creating => write!(f, "creating"),
            VmStatus::Starting => write!(f, "starting"),
            VmStatus::Running => write!(f, "running"),
            VmStatus::Unhealthy => write!(f, "unhealthy"),
            VmStatus::Stopping => write!(f, "stopping"),
            VmStatus::Stopped => write!(f, "stopped"),
            VmStatus::Error => write!(f, "error"),
        }
    }
}

/// Helper function to get current Instant (used for serde default)
fn instant_now() -> Instant {
    Instant::now()
}

/// A VM instance handle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInstance {
    /// Unique VM identifier
    pub id: String,
    /// VM IP address
    pub ip: Ipv4Addr,
    /// API port (default 8080)
    pub port: u16,
    /// VNC port (default 5900)
    pub vnc_port: u16,
    /// Current status
    pub status: VmStatus,
    /// When the VM was created (skipped in serialization)
    #[serde(skip, default = "instant_now")]
    pub created_at: Instant,
    /// VM index for resource allocation
    #[serde(skip, default)]
    pub index: u32,
}

impl VmInstance {
    /// Get the HTTP base URL for this VM
    pub fn http_url(&self) -> String {
        format!("http://{}:{}", self.ip, self.port)
    }

    /// Get the VNC URL for this VM
    pub fn vnc_url(&self) -> String {
        format!("vnc://{}:{}", self.ip, self.vnc_port)
    }

    /// Get the age of this VM in seconds
    pub fn age_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }
}

/// Result of command execution in VM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// VM Manager for managing Firecracker microVMs
pub struct VmManager {
    firecracker: Arc<FirecrackerClient>,
    pool: Arc<VmPool>,
    config: VmManagerConfig,
    /// Firecracker process handles
    processes: Arc<RwLock<HashMap<String, Child>>>,
    /// HTTP client for health checks and commands
    http_client: reqwest::Client,
}

impl VmManager {
    /// Create a new VM manager
    pub fn new(config: VmManagerConfig) -> Self {
        let firecracker = Arc::new(FirecrackerClient::new(config.firecracker.clone()));
        let pool = Arc::new(VmPool::new(config.pool.clone()));

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.exec_timeout_ms))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            firecracker,
            pool,
            config,
            processes: Arc::new(RwLock::new(HashMap::new())),
            http_client,
        }
    }

    /// Initialize the VM manager and optionally warm the pool
    #[instrument(skip(self))]
    pub async fn init(&self) -> Result<()> {
        info!("Initializing VM Manager");

        // Ensure socket directory exists
        tokio::fs::create_dir_all(&self.config.firecracker.socket_dir)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create socket directory: {}", e)))?;

        if self.config.auto_warm {
            self.warm_pool().await?;
        }

        info!("VM Manager initialized");
        Ok(())
    }

    /// Warm the pool with minimum number of VMs
    #[instrument(skip(self))]
    pub async fn warm_pool(&self) -> Result<()> {
        let min_size = self.config.pool.min_pool_size;
        let current = self.pool.available_count().await;

        if current >= min_size {
            debug!(
                current = current,
                min = min_size,
                "Pool already has enough VMs"
            );
            return Ok(());
        }

        let to_create = min_size - current;
        info!(to_create = to_create, target = min_size, "Warming VM pool");

        for _ in 0..to_create {
            match self.create_vm_internal().await {
                Ok(entry) => {
                    self.pool.add_available(entry).await?;
                }
                Err(e) => {
                    error!(error = %e, "Failed to create VM for pool warming");
                }
            }
        }

        let final_count = self.pool.available_count().await;
        info!(available = final_count, "Pool warming complete");

        Ok(())
    }

    /// Create a new VM internally
    #[instrument(skip(self))]
    async fn create_vm_internal(&self) -> Result<PoolEntry> {
        let vm_id = self.pool.generate_vm_id();
        let index = self.pool.allocate_index().await;

        info!(vm_id = %vm_id, index = index, "Creating new VM");

        // Start Firecracker process
        let child = self.firecracker.start_firecracker_process(&vm_id).await?;

        // Store process handle
        {
            let mut processes = self.processes.write().await;
            processes.insert(vm_id.clone(), child);
        }

        // Configure VM
        let vm_config = VmConfig {
            vm_id: vm_id.clone(),
            memory_mb: self.config.firecracker.default_memory_mb,
            vcpus: self.config.firecracker.default_vcpus,
            tap_device: self.pool.tap_for_index(index),
            guest_mac: format!(
                "AA:FC:00:00:{:02X}:{:02X}",
                (index >> 8) & 0xFF,
                index & 0xFF
            ),
            guest_ip: self.pool.ip_for_index(index).to_string(),
            gateway_ip: self.pool.gateway_for_index(index).to_string(),
        };

        self.firecracker.configure_vm(&vm_config).await?;

        // Start instance
        self.firecracker.start_instance(&vm_id).await?;

        // Wait for VM to be healthy
        let ip = self.pool.ip_for_index(index);
        self.wait_for_health(&vm_id, ip).await?;

        let entry = PoolEntry {
            vm_id: vm_id.clone(),
            ip,
            created_at: Instant::now(),
            last_health_check: Instant::now(),
            healthy: true,
            index,
        };

        info!(vm_id = %vm_id, ip = %ip, "VM created and healthy");

        Ok(entry)
    }

    /// Wait for VM to become healthy
    #[instrument(skip(self))]
    async fn wait_for_health(&self, vm_id: &str, ip: Ipv4Addr) -> Result<()> {
        let url = format!("http://{}:{}/api/health", ip, self.config.pool.api_port);
        let timeout = Duration::from_millis(self.config.health_check_timeout_ms);
        let retries = self.config.health_check_retries;

        for attempt in 1..=retries {
            debug!(
                vm_id = vm_id,
                attempt = attempt,
                url = %url,
                "Health check attempt"
            );

            match tokio::time::timeout(timeout, self.http_client.get(&url).send()).await {
                Ok(Ok(response)) if response.status().is_success() => {
                    debug!(vm_id = vm_id, "Health check passed");
                    return Ok(());
                }
                Ok(Ok(response)) => {
                    warn!(
                        vm_id = vm_id,
                        status = %response.status(),
                        "Health check returned non-success status"
                    );
                }
                Ok(Err(e)) => {
                    debug!(vm_id = vm_id, error = %e, "Health check request failed");
                }
                Err(_) => {
                    debug!(vm_id = vm_id, "Health check timed out");
                }
            }

            if attempt < retries {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }

        Err(Error::Timeout(format!(
            "VM {} failed health check after {} attempts",
            vm_id, retries
        )))
    }

    /// Acquire a VM from the pool (or create a new one)
    #[instrument(skip(self))]
    pub async fn acquire(&self) -> Result<VmInstance> {
        // Try to get from pool first
        if let Some(entry) = self.pool.acquire().await {
            return Ok(VmInstance {
                id: entry.vm_id,
                ip: entry.ip,
                port: self.config.pool.api_port,
                vnc_port: self.config.pool.vnc_port,
                status: VmStatus::Running,
                created_at: entry.created_at,
                index: entry.index,
            });
        }

        // Pool empty, create a new VM
        info!("Pool empty, creating new VM");
        let entry = self.create_vm_internal().await?;

        // Add to pool as in-use
        let instance = VmInstance {
            id: entry.vm_id.clone(),
            ip: entry.ip,
            port: self.config.pool.api_port,
            vnc_port: self.config.pool.vnc_port,
            status: VmStatus::Running,
            created_at: entry.created_at,
            index: entry.index,
        };

        // Track as acquired
        self.pool.add_available(entry).await?;
        self.pool.acquire().await;

        Ok(instance)
    }

    /// Release a VM back to the pool (or destroy it)
    #[instrument(skip(self))]
    pub async fn release(&self, instance: VmInstance) -> Result<()> {
        info!(vm_id = %instance.id, "Releasing VM");

        // Check health before returning to pool
        let healthy = self.check_health(&instance).await.is_ok();

        if !healthy {
            warn!(vm_id = %instance.id, "VM unhealthy, destroying instead of returning to pool");
            return self.destroy_vm(&instance.id, instance.index).await;
        }

        // Release back to pool
        match self.pool.release(&instance.id).await {
            Some(_) => {
                debug!(vm_id = %instance.id, "VM returned to pool");
                Ok(())
            }
            None => {
                // Pool said to destroy (too old or pool full)
                self.destroy_vm(&instance.id, instance.index).await
            }
        }
    }

    /// Execute a command in a VM
    #[instrument(skip(self, cmd))]
    pub async fn exec(&self, instance: &VmInstance, cmd: &str) -> Result<ExecResult> {
        let url = format!("http://{}:{}/exec", instance.ip, instance.port);
        let start = Instant::now();

        #[derive(Serialize)]
        struct ExecRequest<'a> {
            command: &'a str,
        }

        #[derive(Deserialize)]
        struct ExecResponse {
            stdout: String,
            stderr: String,
            exit_code: i32,
        }

        let timeout = Duration::from_millis(self.config.exec_timeout_ms);

        let response = tokio::time::timeout(
            timeout,
            self.http_client
                .post(&url)
                .json(&ExecRequest { command: cmd })
                .send(),
        )
        .await
        .map_err(|_| {
            Error::Timeout(format!(
                "Command execution timed out after {}ms",
                self.config.exec_timeout_ms
            ))
        })?
        .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::ExecutionFailed(format!(
                "VM exec failed with status {}: {}",
                status, body
            )));
        }

        let exec_response: ExecResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse exec response: {}", e)))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(ExecResult {
            stdout: exec_response.stdout,
            stderr: exec_response.stderr,
            exit_code: exec_response.exit_code,
            duration_ms,
        })
    }

    /// Check VM health
    #[instrument(skip(self))]
    pub async fn check_health(&self, instance: &VmInstance) -> Result<()> {
        let url = format!("http://{}:{}/api/health", instance.ip, instance.port);
        let timeout = Duration::from_millis(self.config.health_check_timeout_ms);

        let response = tokio::time::timeout(timeout, self.http_client.get(&url).send())
            .await
            .map_err(|_| Error::Timeout("Health check timed out".to_string()))?
            .map_err(|e| Error::Http(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Internal(format!(
                "Health check failed with status {}",
                response.status()
            )))
        }
    }

    /// Get VM status
    #[instrument(skip(self))]
    pub async fn status(&self, instance: &VmInstance) -> Result<VmStatus> {
        // Check if process is still running
        {
            let processes = self.processes.read().await;
            if !processes.contains_key(&instance.id) {
                return Ok(VmStatus::Stopped);
            }
        }

        // Check health
        match self.check_health(instance).await {
            Ok(_) => Ok(VmStatus::Running),
            Err(_) => Ok(VmStatus::Unhealthy),
        }
    }

    /// Destroy a specific VM
    #[instrument(skip(self))]
    async fn destroy_vm(&self, vm_id: &str, index: u32) -> Result<()> {
        info!(vm_id = vm_id, "Destroying VM");

        // Remove from pool tracking
        self.pool.remove(vm_id).await;

        // Stop the Firecracker instance gracefully
        let _ = self.firecracker.stop_instance(vm_id).await;

        // Kill the process
        {
            let mut processes = self.processes.write().await;
            if let Some(mut child) = processes.remove(vm_id) {
                let _ = child.kill().await;
            }
        }

        // Cleanup socket and logs
        self.firecracker.cleanup_vm(vm_id).await?;

        // Release the index for reuse
        self.pool.release_index(index).await;

        info!(vm_id = vm_id, "VM destroyed");
        Ok(())
    }

    /// Shutdown all VMs
    #[instrument(skip(self))]
    pub async fn shutdown_all(&self) -> Result<()> {
        info!("Shutting down all VMs");

        let entries = self.pool.clear().await;

        for entry in entries {
            if let Err(e) = self.destroy_vm(&entry.vm_id, entry.index).await {
                error!(vm_id = %entry.vm_id, error = %e, "Failed to destroy VM during shutdown");
            }
        }

        info!("All VMs shut down");
        Ok(())
    }

    /// Run health check maintenance
    #[instrument(skip(self))]
    pub async fn run_health_checks(&self) -> Result<()> {
        debug!("Running health check maintenance");

        // Get VMs needing health checks
        let stale = self.pool.get_stale_vms().await;

        for entry in stale {
            let instance = VmInstance {
                id: entry.vm_id.clone(),
                ip: entry.ip,
                port: self.config.pool.api_port,
                vnc_port: self.config.pool.vnc_port,
                status: VmStatus::Running,
                created_at: entry.created_at,
                index: entry.index,
            };

            match self.check_health(&instance).await {
                Ok(_) => {
                    self.pool.mark_healthy(&entry.vm_id).await;
                }
                Err(e) => {
                    warn!(vm_id = %entry.vm_id, error = %e, "VM failed health check");
                    self.pool.mark_unhealthy(&entry.vm_id).await;
                }
            }
        }

        // Remove unhealthy VMs
        let unhealthy = self.pool.get_unhealthy_vms().await;
        for entry in unhealthy {
            info!(vm_id = %entry.vm_id, "Removing unhealthy VM");
            if let Err(e) = self.destroy_vm(&entry.vm_id, entry.index).await {
                error!(vm_id = %entry.vm_id, error = %e, "Failed to destroy unhealthy VM");
            }
        }

        // Remove expired VMs
        let expired = self.pool.get_expired_vms().await;
        for entry in expired {
            info!(vm_id = %entry.vm_id, age_secs = entry.created_at.elapsed().as_secs(), "Removing expired VM");
            if let Err(e) = self.destroy_vm(&entry.vm_id, entry.index).await {
                error!(vm_id = %entry.vm_id, error = %e, "Failed to destroy expired VM");
            }
        }

        // Replenish pool if needed
        if self.pool.needs_replenishment().await {
            if let Err(e) = self.warm_pool().await {
                error!(error = %e, "Failed to replenish pool");
            }
        }

        Ok(())
    }

    /// Start background health check task
    pub fn start_health_check_task(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let manager = Arc::clone(self);
        let interval = Duration::from_secs(self.config.pool.health_check_interval_secs);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);

            loop {
                ticker.tick().await;

                if let Err(e) = manager.run_health_checks().await {
                    error!(error = %e, "Health check task failed");
                }
            }
        })
    }

    /// Get pool statistics
    pub async fn stats(&self) -> VmPoolStats {
        self.pool.stats().await
    }

    /// Get the Firecracker client
    pub fn firecracker(&self) -> &FirecrackerClient {
        &self.firecracker
    }

    /// Get the VM pool
    pub fn pool(&self) -> &VmPool {
        &self.pool
    }

    /// Get the configuration
    pub fn config(&self) -> &VmManagerConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_status_display() {
        assert_eq!(VmStatus::Running.to_string(), "running");
        assert_eq!(VmStatus::Creating.to_string(), "creating");
        assert_eq!(VmStatus::Stopped.to_string(), "stopped");
    }

    #[test]
    fn test_vm_instance_urls() {
        let instance = VmInstance {
            id: "test-vm".to_string(),
            ip: Ipv4Addr::new(172, 16, 0, 2),
            port: 8080,
            vnc_port: 5900,
            status: VmStatus::Running,
            created_at: Instant::now(),
            index: 0,
        };

        assert_eq!(instance.http_url(), "http://172.16.0.2:8080");
        assert_eq!(instance.vnc_url(), "vnc://172.16.0.2:5900");
    }

    #[test]
    fn test_vm_instance_age() {
        let instance = VmInstance {
            id: "test-vm".to_string(),
            ip: Ipv4Addr::new(172, 16, 0, 2),
            port: 8080,
            vnc_port: 5900,
            status: VmStatus::Running,
            created_at: Instant::now(),
            index: 0,
        };

        // Age should be very small (just created)
        assert!(instance.age_secs() < 1);
    }

    #[test]
    fn test_exec_result_serialization() {
        let result = ExecResult {
            stdout: "Hello".to_string(),
            stderr: "".to_string(),
            exit_code: 0,
            duration_ms: 100,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("Hello"));
        assert!(json.contains("exit_code"));
    }

    #[test]
    fn test_default_config() {
        let config = VmManagerConfig::default();
        assert_eq!(config.health_check_timeout_ms, 5000);
        assert_eq!(config.exec_timeout_ms, 30000);
        assert_eq!(config.health_check_retries, 3);
        assert!(config.auto_warm);
    }

    #[tokio::test]
    async fn test_manager_creation() {
        let config = VmManagerConfig {
            auto_warm: false,
            ..Default::default()
        };
        let manager = VmManager::new(config);

        let stats = manager.stats().await;
        assert_eq!(stats.available, 0);
        assert_eq!(stats.in_use, 0);
    }

    #[test]
    fn test_vm_status_serialization() {
        let status = VmStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");

        let deserialized: VmStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, VmStatus::Running);
    }

    #[test]
    fn test_vm_instance_serialization() {
        let instance = VmInstance {
            id: "vm-123".to_string(),
            ip: Ipv4Addr::new(172, 16, 0, 2),
            port: 8080,
            vnc_port: 5900,
            status: VmStatus::Running,
            created_at: Instant::now(),
            index: 0,
        };

        let json = serde_json::to_string(&instance).unwrap();
        assert!(json.contains("vm-123"));
        assert!(json.contains("172.16.0.2"));
        assert!(json.contains("running"));
        // created_at and index should be skipped
        assert!(!json.contains("index"));
    }
}
