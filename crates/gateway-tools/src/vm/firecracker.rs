//! Firecracker API Client
//!
//! Provides a client for interacting with the Firecracker microVM API
//! via Unix domain sockets.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::{debug, error, info, warn};

use super::config::{DriveConfig, NetworkConfig, VmConfig};

/// Firecracker API client for managing microVMs.
pub struct FirecrackerClient {
    /// Path to the Unix socket.
    socket_path: PathBuf,
    /// Firecracker process handle (if started by us).
    process: Option<Child>,
    /// VM configuration.
    config: VmConfig,
    /// Current VM state.
    state: VmState,
}

/// State of a Firecracker VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VmState {
    /// VM is not created yet.
    NotCreated,
    /// VM is configured but not started.
    Configured,
    /// VM is running.
    Running,
    /// VM is paused.
    Paused,
    /// VM has been stopped.
    Stopped,
}

impl Default for VmState {
    fn default() -> Self {
        Self::NotCreated
    }
}

/// Machine configuration for Firecracker API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineConfig {
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smt: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_dirty_pages: Option<bool>,
}

/// Boot source configuration for Firecracker API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootSource {
    pub kernel_image_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_args: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initrd_path: Option<String>,
}

/// Drive configuration for Firecracker API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drive {
    pub drive_id: String,
    pub path_on_host: String,
    pub is_root_device: bool,
    pub is_read_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partuuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limiter: Option<RateLimiter>,
}

/// Network interface configuration for Firecracker API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub iface_id: String,
    pub host_dev_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx_rate_limiter: Option<RateLimiter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_rate_limiter: Option<RateLimiter>,
}

/// Rate limiter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimiter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<TokenBucket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops: Option<TokenBucket>,
}

/// Token bucket for rate limiting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBucket {
    pub size: u64,
    pub one_time_burst: Option<u64>,
    pub refill_time: u64,
}

/// VM action request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceActionInfo {
    pub action_type: InstanceAction,
}

/// VM action types.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InstanceAction {
    FlushMetrics,
    InstanceStart,
    SendCtrlAltDel,
}

/// API response from Firecracker.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiResponse {
    #[serde(default)]
    pub fault_message: Option<String>,
}

impl FirecrackerClient {
    /// Create a new Firecracker client.
    pub fn new(socket_path: impl Into<PathBuf>, config: VmConfig) -> Self {
        Self {
            socket_path: socket_path.into(),
            process: None,
            config,
            state: VmState::NotCreated,
        }
    }

    /// Get the VM ID.
    pub fn vm_id(&self) -> &str {
        &self.config.id
    }

    /// Get the current VM state.
    pub fn state(&self) -> VmState {
        self.state
    }

    /// Get the socket path.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Start the Firecracker process.
    pub async fn start_firecracker_process(&mut self, firecracker_bin: &Path) -> Result<()> {
        if self.process.is_some() {
            return Err(Error::Internal(
                "Firecracker process already started".into(),
            ));
        }

        // Ensure socket directory exists
        if let Some(parent) = self.socket_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                Error::Internal(format!("Failed to create socket directory: {}", e))
            })?;
        }

        // Remove existing socket if present
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path)
                .await
                .map_err(|e| Error::Internal(format!("Failed to remove existing socket: {}", e)))?;
        }

        info!(
            "Starting Firecracker process for VM {} at {:?}",
            self.config.id, self.socket_path
        );

        let child = Command::new(firecracker_bin)
            .arg("--api-sock")
            .arg(&self.socket_path)
            .arg("--id")
            .arg(&self.config.id)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to start Firecracker: {}", e)))?;

        self.process = Some(child);

        // Wait for socket to become available
        self.wait_for_socket(5000).await?;

        self.state = VmState::NotCreated;
        Ok(())
    }

    /// Wait for the Unix socket to become available.
    async fn wait_for_socket(&self, timeout_ms: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(timeout_ms);

        while start.elapsed() < timeout {
            if self.socket_path.exists() {
                // Try to connect to verify socket is ready
                match UnixStream::connect(&self.socket_path).await {
                    Ok(_) => {
                        debug!("Socket is ready: {:?}", self.socket_path);
                        return Ok(());
                    }
                    Err(_) => {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }

        Err(Error::Timeout(format!(
            "Socket not available after {}ms: {:?}",
            timeout_ms, self.socket_path
        )))
    }

    /// Send an HTTP request to the Firecracker API via Unix socket.
    async fn send_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<(u16, String)> {
        let mut stream = UnixStream::connect(&self.socket_path).await.map_err(|e| {
            Error::Internal(format!("Failed to connect to Firecracker socket: {}", e))
        })?;

        let content_length = body.map(|b| b.len()).unwrap_or(0);
        let request = if let Some(body) = body {
            format!(
                "{} {} HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Accept: application/json\r\n\
                 \r\n\
                 {}",
                method, path, content_length, body
            )
        } else {
            format!(
                "{} {} HTTP/1.1\r\n\
                 Host: localhost\r\n\
                 Accept: application/json\r\n\
                 \r\n",
                method, path
            )
        };

        debug!("Sending request to Firecracker:\n{}", request);

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| Error::Internal(format!("Failed to send request: {}", e)))?;

        let mut response = vec![0u8; 8192];
        let n = stream
            .read(&mut response)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read response: {}", e)))?;

        let response_str = String::from_utf8_lossy(&response[..n]).to_string();
        debug!("Firecracker response:\n{}", response_str);

        // Parse HTTP response
        let (status_code, body) = self.parse_http_response(&response_str)?;

        Ok((status_code, body))
    }

    /// Parse HTTP response to extract status code and body.
    fn parse_http_response(&self, response: &str) -> Result<(u16, String)> {
        let mut lines = response.lines();

        // Parse status line
        let status_line = lines
            .next()
            .ok_or_else(|| Error::Internal("Empty response".into()))?;

        let parts: Vec<&str> = status_line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(Error::Internal(format!(
                "Invalid status line: {}",
                status_line
            )));
        }

        let status_code: u16 = parts[1]
            .parse()
            .map_err(|_| Error::Internal(format!("Invalid status code: {}", parts[1])))?;

        // Find body (after empty line)
        let mut in_body = false;
        let mut body = String::new();
        for line in lines {
            if in_body {
                body.push_str(line);
                body.push('\n');
            } else if line.is_empty() {
                in_body = true;
            }
        }

        Ok((status_code, body.trim().to_string()))
    }

    /// Configure the VM machine (vCPU, memory).
    pub async fn configure_machine(&mut self) -> Result<()> {
        let machine_config = MachineConfig {
            vcpu_count: self.config.vcpu_count,
            mem_size_mib: self.config.mem_size_mib,
            smt: Some(false),
            track_dirty_pages: Some(false),
        };

        let body = serde_json::to_string(&machine_config)
            .map_err(|e| Error::Serialization(e.to_string()))?;

        let (status, response) = self
            .send_request("PUT", "/machine-config", Some(&body))
            .await?;

        if status >= 400 {
            return Err(Error::Internal(format!(
                "Failed to configure machine: {} - {}",
                status, response
            )));
        }

        info!(
            "Configured VM {} with {} vCPUs, {} MiB memory",
            self.config.id, self.config.vcpu_count, self.config.mem_size_mib
        );

        Ok(())
    }

    /// Configure the boot source (kernel, boot args).
    pub async fn configure_boot_source(&mut self) -> Result<()> {
        let boot_source = BootSource {
            kernel_image_path: self.config.kernel_path.to_string_lossy().to_string(),
            boot_args: self.config.boot_args.clone(),
            initrd_path: None,
        };

        let body =
            serde_json::to_string(&boot_source).map_err(|e| Error::Serialization(e.to_string()))?;

        let (status, response) = self
            .send_request("PUT", "/boot-source", Some(&body))
            .await?;

        if status >= 400 {
            return Err(Error::Internal(format!(
                "Failed to configure boot source: {} - {}",
                status, response
            )));
        }

        info!("Configured boot source for VM {}", self.config.id);

        Ok(())
    }

    /// Configure the root filesystem drive.
    pub async fn configure_rootfs(&mut self) -> Result<()> {
        let drive = Drive {
            drive_id: "rootfs".to_string(),
            path_on_host: self.config.rootfs_path.to_string_lossy().to_string(),
            is_root_device: true,
            is_read_only: false,
            partuuid: None,
            rate_limiter: None,
        };

        let body =
            serde_json::to_string(&drive).map_err(|e| Error::Serialization(e.to_string()))?;

        let (status, response) = self
            .send_request("PUT", "/drives/rootfs", Some(&body))
            .await?;

        if status >= 400 {
            return Err(Error::Internal(format!(
                "Failed to configure rootfs: {} - {}",
                status, response
            )));
        }

        info!("Configured rootfs for VM {}", self.config.id);

        Ok(())
    }

    /// Configure an additional drive.
    pub async fn configure_drive(&mut self, drive_config: &DriveConfig) -> Result<()> {
        let drive = Drive {
            drive_id: drive_config.drive_id.clone(),
            path_on_host: drive_config.path_on_host.to_string_lossy().to_string(),
            is_root_device: drive_config.is_root_device,
            is_read_only: drive_config.is_read_only,
            partuuid: None,
            rate_limiter: None,
        };

        let body =
            serde_json::to_string(&drive).map_err(|e| Error::Serialization(e.to_string()))?;

        let path = format!("/drives/{}", drive_config.drive_id);
        let (status, response) = self.send_request("PUT", &path, Some(&body)).await?;

        if status >= 400 {
            return Err(Error::Internal(format!(
                "Failed to configure drive {}: {} - {}",
                drive_config.drive_id, status, response
            )));
        }

        info!(
            "Configured drive {} for VM {}",
            drive_config.drive_id, self.config.id
        );

        Ok(())
    }

    /// Configure network interface.
    pub async fn configure_network(&mut self, network_config: &NetworkConfig) -> Result<()> {
        let network = NetworkInterface {
            iface_id: network_config.iface_id.clone(),
            host_dev_name: network_config.host_dev_name.clone(),
            guest_mac: network_config.guest_mac.clone(),
            rx_rate_limiter: None,
            tx_rate_limiter: None,
        };

        let body =
            serde_json::to_string(&network).map_err(|e| Error::Serialization(e.to_string()))?;

        let path = format!("/network-interfaces/{}", network_config.iface_id);
        let (status, response) = self.send_request("PUT", &path, Some(&body)).await?;

        if status >= 400 {
            return Err(Error::Internal(format!(
                "Failed to configure network: {} - {}",
                status, response
            )));
        }

        info!(
            "Configured network interface {} for VM {}",
            network_config.iface_id, self.config.id
        );

        Ok(())
    }

    /// Configure the VM with all settings from VmConfig.
    pub async fn configure(&mut self) -> Result<()> {
        self.configure_machine().await?;
        self.configure_boot_source().await?;
        self.configure_rootfs().await?;

        // Configure additional drives
        for drive in &self.config.additional_drives.clone() {
            self.configure_drive(drive).await?;
        }

        // Configure network
        let network = NetworkConfig::new("eth0", &self.config.tap_device);
        self.configure_network(&network).await?;

        self.state = VmState::Configured;
        Ok(())
    }

    /// Start the VM.
    pub async fn start(&mut self) -> Result<()> {
        if self.state != VmState::Configured && self.state != VmState::Paused {
            return Err(Error::Internal(format!(
                "Cannot start VM in state {:?}",
                self.state
            )));
        }

        let action = InstanceActionInfo {
            action_type: InstanceAction::InstanceStart,
        };

        let body =
            serde_json::to_string(&action).map_err(|e| Error::Serialization(e.to_string()))?;

        let (status, response) = self.send_request("PUT", "/actions", Some(&body)).await?;

        if status >= 400 {
            return Err(Error::Internal(format!(
                "Failed to start VM: {} - {}",
                status, response
            )));
        }

        self.state = VmState::Running;
        info!("Started VM {}", self.config.id);

        Ok(())
    }

    /// Stop the VM by sending Ctrl+Alt+Del.
    pub async fn stop(&mut self) -> Result<()> {
        if self.state != VmState::Running {
            return Err(Error::Internal(format!(
                "Cannot stop VM in state {:?}",
                self.state
            )));
        }

        let action = InstanceActionInfo {
            action_type: InstanceAction::SendCtrlAltDel,
        };

        let body =
            serde_json::to_string(&action).map_err(|e| Error::Serialization(e.to_string()))?;

        let (status, response) = self.send_request("PUT", "/actions", Some(&body)).await?;

        if status >= 400 {
            warn!(
                "Failed to gracefully stop VM {}: {} - {}",
                self.config.id, status, response
            );
        }

        self.state = VmState::Stopped;
        info!("Stopped VM {}", self.config.id);

        Ok(())
    }

    /// Destroy the VM and clean up resources.
    pub async fn destroy(&mut self) -> Result<()> {
        // Kill the Firecracker process if we own it
        if let Some(mut process) = self.process.take() {
            match process.kill() {
                Ok(_) => {
                    let _ = process.wait();
                    info!("Killed Firecracker process for VM {}", self.config.id);
                }
                Err(e) => {
                    error!(
                        "Failed to kill Firecracker process for VM {}: {}",
                        self.config.id, e
                    );
                }
            }
        }

        // Remove socket file
        if self.socket_path.exists() {
            if let Err(e) = tokio::fs::remove_file(&self.socket_path).await {
                warn!("Failed to remove socket file {:?}: {}", self.socket_path, e);
            }
        }

        self.state = VmState::Stopped;
        info!("Destroyed VM {}", self.config.id);

        Ok(())
    }

    /// Get VM info from Firecracker.
    pub async fn get_info(&self) -> Result<serde_json::Value> {
        let (status, response) = self.send_request("GET", "/", None).await?;

        if status >= 400 {
            return Err(Error::Internal(format!(
                "Failed to get VM info: {} - {}",
                status, response
            )));
        }

        serde_json::from_str(&response)
            .map_err(|e| Error::Internal(format!("Failed to parse VM info: {}", e)))
    }

    /// Get machine configuration from Firecracker.
    pub async fn get_machine_config(&self) -> Result<MachineConfig> {
        let (status, response) = self.send_request("GET", "/machine-config", None).await?;

        if status >= 400 {
            return Err(Error::Internal(format!(
                "Failed to get machine config: {} - {}",
                status, response
            )));
        }

        serde_json::from_str(&response)
            .map_err(|e| Error::Internal(format!("Failed to parse machine config: {}", e)))
    }

    /// Check if the VM is running.
    pub fn is_running(&self) -> bool {
        self.state == VmState::Running
    }

    /// Get the VM configuration.
    pub fn config(&self) -> &VmConfig {
        &self.config
    }
}

impl Drop for FirecrackerClient {
    fn drop(&mut self) {
        // Attempt to kill the Firecracker process on drop
        if let Some(mut process) = self.process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_state_default() {
        let state = VmState::default();
        assert_eq!(state, VmState::NotCreated);
    }

    #[test]
    fn test_firecracker_client_new() {
        let config = VmConfig::new("test-vm");
        let client = FirecrackerClient::new("/tmp/fc.sock", config);

        assert_eq!(client.vm_id(), "test-vm");
        assert_eq!(client.state(), VmState::NotCreated);
        assert_eq!(client.socket_path(), Path::new("/tmp/fc.sock"));
    }

    #[test]
    fn test_machine_config_serialize() {
        let config = MachineConfig {
            vcpu_count: 2,
            mem_size_mib: 1024,
            smt: Some(false),
            track_dirty_pages: None,
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"vcpu_count\":2"));
        assert!(json.contains("\"mem_size_mib\":1024"));
    }

    #[test]
    fn test_boot_source_serialize() {
        let boot = BootSource {
            kernel_image_path: "/path/to/kernel".to_string(),
            boot_args: Some("console=ttyS0".to_string()),
            initrd_path: None,
        };

        let json = serde_json::to_string(&boot).unwrap();
        assert!(json.contains("/path/to/kernel"));
        assert!(json.contains("console=ttyS0"));
    }

    #[test]
    fn test_drive_serialize() {
        let drive = Drive {
            drive_id: "rootfs".to_string(),
            path_on_host: "/path/to/rootfs.ext4".to_string(),
            is_root_device: true,
            is_read_only: false,
            partuuid: None,
            rate_limiter: None,
        };

        let json = serde_json::to_string(&drive).unwrap();
        assert!(json.contains("\"is_root_device\":true"));
    }

    #[test]
    fn test_network_interface_serialize() {
        let network = NetworkInterface {
            iface_id: "eth0".to_string(),
            host_dev_name: "tap0".to_string(),
            guest_mac: Some("AA:BB:CC:DD:EE:FF".to_string()),
            rx_rate_limiter: None,
            tx_rate_limiter: None,
        };

        let json = serde_json::to_string(&network).unwrap();
        assert!(json.contains("\"iface_id\":\"eth0\""));
        assert!(json.contains("AA:BB:CC:DD:EE:FF"));
    }

    #[test]
    fn test_parse_http_response() {
        let config = VmConfig::new("test");
        let client = FirecrackerClient::new("/tmp/test.sock", config);

        let response =
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"status\":\"ok\"}";
        let (status, body) = client.parse_http_response(response).unwrap();

        assert_eq!(status, 200);
        assert_eq!(body, "{\"status\":\"ok\"}");
    }

    #[test]
    fn test_parse_http_response_error() {
        let config = VmConfig::new("test");
        let client = FirecrackerClient::new("/tmp/test.sock", config);

        let response =
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\r\n{\"error\":\"invalid\"}";
        let (status, body) = client.parse_http_response(response).unwrap();

        assert_eq!(status, 400);
        assert!(body.contains("invalid"));
    }
}
