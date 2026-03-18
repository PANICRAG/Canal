//! Firecracker API Client
//!
//! Low-level client for communicating with the Firecracker hypervisor via Unix socket.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::{debug, instrument};

/// Firecracker client configuration
#[derive(Debug, Clone)]
pub struct FirecrackerConfig {
    /// Path to the Firecracker binary
    pub binary_path: PathBuf,
    /// Base directory for VM sockets and logs
    pub socket_dir: PathBuf,
    /// Path to the kernel image
    pub kernel_path: PathBuf,
    /// Path to the rootfs image
    pub rootfs_path: PathBuf,
    /// Default memory size in MB
    pub default_memory_mb: u64,
    /// Default number of vCPUs
    pub default_vcpus: u8,
}

impl Default for FirecrackerConfig {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::from("/usr/bin/firecracker"),
            socket_dir: PathBuf::from("/tmp/firecracker"),
            kernel_path: PathBuf::from("/var/lib/firecracker/vmlinux"),
            rootfs_path: PathBuf::from("/var/lib/firecracker/rootfs.ext4"),
            default_memory_mb: 512,
            default_vcpus: 2,
        }
    }
}

/// VM configuration for Firecracker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    /// Unique VM identifier
    pub vm_id: String,
    /// Memory size in MB
    pub memory_mb: u64,
    /// Number of vCPUs
    pub vcpus: u8,
    /// Network TAP device name
    pub tap_device: String,
    /// Guest MAC address
    pub guest_mac: String,
    /// Guest IP address
    pub guest_ip: String,
    /// Host gateway IP
    pub gateway_ip: String,
}

/// Boot source configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootSource {
    pub kernel_image_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_args: Option<String>,
}

/// Drive configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveConfig {
    pub drive_id: String,
    pub path_on_host: String,
    pub is_root_device: bool,
    pub is_read_only: bool,
}

/// Network interface configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInterface {
    pub iface_id: String,
    pub guest_mac: String,
    pub host_dev_name: String,
}

/// Machine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineConfig {
    pub vcpu_count: u8,
    pub mem_size_mib: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smt: Option<bool>,
}

/// Action for VM control
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceActionInfo {
    pub action_type: String,
}

/// Firecracker API response
#[derive(Debug, Clone, Deserialize)]
pub struct ApiResponse {
    #[serde(default)]
    pub fault_message: Option<String>,
}

/// Instance info from Firecracker
#[derive(Debug, Clone, Deserialize)]
pub struct InstanceInfo {
    pub state: String,
    #[serde(default)]
    pub started: bool,
}

/// Firecracker API client for managing microVMs
pub struct FirecrackerClient {
    config: FirecrackerConfig,
}

impl FirecrackerClient {
    /// Create a new Firecracker client
    pub fn new(config: FirecrackerConfig) -> Self {
        Self { config }
    }

    /// Get the socket path for a VM
    pub fn socket_path(&self, vm_id: &str) -> PathBuf {
        self.config.socket_dir.join(format!("{}.sock", vm_id))
    }

    /// Check if a VM socket exists
    pub fn socket_exists(&self, vm_id: &str) -> bool {
        self.socket_path(vm_id).exists()
    }

    /// Start a new Firecracker process
    #[instrument(skip(self))]
    pub async fn start_firecracker_process(&self, vm_id: &str) -> Result<tokio::process::Child> {
        // Ensure socket directory exists
        tokio::fs::create_dir_all(&self.config.socket_dir)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create socket directory: {}", e)))?;

        let socket_path = self.socket_path(vm_id);
        let log_path = self.config.socket_dir.join(format!("{}.log", vm_id));

        // Remove existing socket if present
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await.ok();
        }

        debug!(
            vm_id = vm_id,
            socket_path = %socket_path.display(),
            "Starting Firecracker process"
        );

        let child = tokio::process::Command::new(&self.config.binary_path)
            .arg("--api-sock")
            .arg(&socket_path)
            .arg("--log-path")
            .arg(&log_path)
            .arg("--level")
            .arg("Debug")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to start Firecracker: {}", e)))?;

        // Wait for socket to be created
        for _ in 0..50 {
            if socket_path.exists() {
                debug!(vm_id = vm_id, "Firecracker socket ready");
                return Ok(child);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        Err(Error::Timeout(format!(
            "Firecracker socket not created for VM {}",
            vm_id
        )))
    }

    /// Send an HTTP request to Firecracker via Unix socket
    #[instrument(skip(self, body))]
    async fn send_request(
        &self,
        vm_id: &str,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<String> {
        let socket_path = self.socket_path(vm_id);

        let mut stream = UnixStream::connect(&socket_path).await.map_err(|e| {
            Error::Internal(format!("Failed to connect to Firecracker socket: {}", e))
        })?;

        let content_length = body.map(|b| b.len()).unwrap_or(0);
        let request = if let Some(body) = body {
            format!(
                "{} {} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                method, path, content_length, body
            )
        } else {
            format!("{} {} HTTP/1.1\r\nHost: localhost\r\n\r\n", method, path)
        };

        debug!(
            vm_id = vm_id,
            method = method,
            path = path,
            "Sending request to Firecracker"
        );

        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| Error::Internal(format!("Failed to write to socket: {}", e)))?;

        let mut response = vec![0u8; 65536];
        let n = stream
            .read(&mut response)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read from socket: {}", e)))?;

        let response_str = String::from_utf8_lossy(&response[..n]).to_string();
        debug!(
            vm_id = vm_id,
            response_len = n,
            "Received response from Firecracker"
        );

        // Parse HTTP response
        if let Some(status_line) = response_str.lines().next() {
            if status_line.contains("204") || status_line.contains("200") {
                // Extract body if present
                if let Some(body_start) = response_str.find("\r\n\r\n") {
                    return Ok(response_str[body_start + 4..].to_string());
                }
                return Ok(String::new());
            }
        }

        // Try to extract error message
        if let Some(body_start) = response_str.find("\r\n\r\n") {
            let body = &response_str[body_start + 4..];
            if let Ok(api_response) = serde_json::from_str::<ApiResponse>(body) {
                if let Some(fault) = api_response.fault_message {
                    return Err(Error::Internal(format!("Firecracker API error: {}", fault)));
                }
            }
        }

        Err(Error::Internal(format!(
            "Firecracker API request failed: {}",
            response_str
        )))
    }

    /// Configure the boot source
    #[instrument(skip(self))]
    pub async fn configure_boot_source(&self, vm_id: &str, boot_source: &BootSource) -> Result<()> {
        let body =
            serde_json::to_string(boot_source).map_err(|e| Error::Serialization(e.to_string()))?;

        self.send_request(vm_id, "PUT", "/boot-source", Some(&body))
            .await?;

        debug!(vm_id = vm_id, "Boot source configured");
        Ok(())
    }

    /// Configure a drive
    #[instrument(skip(self))]
    pub async fn configure_drive(&self, vm_id: &str, drive: &DriveConfig) -> Result<()> {
        let body = serde_json::to_string(drive).map_err(|e| Error::Serialization(e.to_string()))?;

        self.send_request(
            vm_id,
            "PUT",
            &format!("/drives/{}", drive.drive_id),
            Some(&body),
        )
        .await?;

        debug!(vm_id = vm_id, drive_id = %drive.drive_id, "Drive configured");
        Ok(())
    }

    /// Configure a network interface
    #[instrument(skip(self))]
    pub async fn configure_network(&self, vm_id: &str, network: &NetworkInterface) -> Result<()> {
        let body =
            serde_json::to_string(network).map_err(|e| Error::Serialization(e.to_string()))?;

        self.send_request(
            vm_id,
            "PUT",
            &format!("/network-interfaces/{}", network.iface_id),
            Some(&body),
        )
        .await?;

        debug!(vm_id = vm_id, iface_id = %network.iface_id, "Network interface configured");
        Ok(())
    }

    /// Configure machine resources
    #[instrument(skip(self))]
    pub async fn configure_machine(&self, vm_id: &str, config: &MachineConfig) -> Result<()> {
        let body =
            serde_json::to_string(config).map_err(|e| Error::Serialization(e.to_string()))?;

        self.send_request(vm_id, "PUT", "/machine-config", Some(&body))
            .await?;

        debug!(
            vm_id = vm_id,
            vcpus = config.vcpu_count,
            memory_mb = config.mem_size_mib,
            "Machine config configured"
        );
        Ok(())
    }

    /// Start the VM instance
    #[instrument(skip(self))]
    pub async fn start_instance(&self, vm_id: &str) -> Result<()> {
        let action = InstanceActionInfo {
            action_type: "InstanceStart".to_string(),
        };

        let body =
            serde_json::to_string(&action).map_err(|e| Error::Serialization(e.to_string()))?;

        self.send_request(vm_id, "PUT", "/actions", Some(&body))
            .await?;

        debug!(vm_id = vm_id, "VM instance started");
        Ok(())
    }

    /// Stop the VM instance
    #[instrument(skip(self))]
    pub async fn stop_instance(&self, vm_id: &str) -> Result<()> {
        let action = InstanceActionInfo {
            action_type: "SendCtrlAltDel".to_string(),
        };

        let body =
            serde_json::to_string(&action).map_err(|e| Error::Serialization(e.to_string()))?;

        // Ignore errors as VM might already be stopped
        let _ = self
            .send_request(vm_id, "PUT", "/actions", Some(&body))
            .await;

        debug!(vm_id = vm_id, "VM instance stop requested");
        Ok(())
    }

    /// Get instance info
    #[instrument(skip(self))]
    pub async fn get_instance_info(&self, vm_id: &str) -> Result<InstanceInfo> {
        let response = self.send_request(vm_id, "GET", "/", None).await?;

        serde_json::from_str(&response)
            .map_err(|e| Error::Internal(format!("Failed to parse instance info: {}", e)))
    }

    /// Configure and start a complete VM
    #[instrument(skip(self))]
    pub async fn configure_vm(&self, config: &VmConfig) -> Result<()> {
        // Configure machine
        let machine_config = MachineConfig {
            vcpu_count: config.vcpus,
            mem_size_mib: config.memory_mb,
            smt: Some(false),
        };
        self.configure_machine(&config.vm_id, &machine_config)
            .await?;

        // Configure boot source
        let boot_args = format!(
            "console=ttyS0 reboot=k panic=1 pci=off ip={}::{}:255.255.255.0::eth0:off",
            config.guest_ip, config.gateway_ip
        );
        let boot_source = BootSource {
            kernel_image_path: self.config.kernel_path.to_string_lossy().to_string(),
            boot_args: Some(boot_args),
        };
        self.configure_boot_source(&config.vm_id, &boot_source)
            .await?;

        // Configure root drive
        let drive = DriveConfig {
            drive_id: "rootfs".to_string(),
            path_on_host: self.config.rootfs_path.to_string_lossy().to_string(),
            is_root_device: true,
            is_read_only: false,
        };
        self.configure_drive(&config.vm_id, &drive).await?;

        // Configure network
        let network = NetworkInterface {
            iface_id: "eth0".to_string(),
            guest_mac: config.guest_mac.clone(),
            host_dev_name: config.tap_device.clone(),
        };
        self.configure_network(&config.vm_id, &network).await?;

        debug!(vm_id = %config.vm_id, "VM fully configured");
        Ok(())
    }

    /// Clean up VM resources (socket, logs)
    #[instrument(skip(self))]
    pub async fn cleanup_vm(&self, vm_id: &str) -> Result<()> {
        let socket_path = self.socket_path(vm_id);
        let log_path = self.config.socket_dir.join(format!("{}.log", vm_id));

        // Remove socket
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await.ok();
        }

        // Remove log
        if log_path.exists() {
            tokio::fs::remove_file(&log_path).await.ok();
        }

        debug!(vm_id = vm_id, "VM resources cleaned up");
        Ok(())
    }

    /// Get the Firecracker configuration
    pub fn config(&self) -> &FirecrackerConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = FirecrackerConfig::default();
        assert_eq!(config.default_memory_mb, 512);
        assert_eq!(config.default_vcpus, 2);
    }

    #[test]
    fn test_socket_path() {
        let config = FirecrackerConfig::default();
        let client = FirecrackerClient::new(config);
        let path = client.socket_path("test-vm");
        assert!(path.to_string_lossy().contains("test-vm.sock"));
    }

    #[test]
    fn test_vm_config_serialization() {
        let config = VmConfig {
            vm_id: "test-1".to_string(),
            memory_mb: 1024,
            vcpus: 4,
            tap_device: "tap0".to_string(),
            guest_mac: "AA:FC:00:00:00:01".to_string(),
            guest_ip: "172.16.0.2".to_string(),
            gateway_ip: "172.16.0.1".to_string(),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("test-1"));
        assert!(json.contains("1024"));
    }

    #[test]
    fn test_boot_source_serialization() {
        let boot_source = BootSource {
            kernel_image_path: "/path/to/kernel".to_string(),
            boot_args: Some("console=ttyS0".to_string()),
        };

        let json = serde_json::to_string(&boot_source).unwrap();
        assert!(json.contains("kernel_image_path"));
        assert!(json.contains("boot_args"));
    }

    #[test]
    fn test_machine_config_serialization() {
        let config = MachineConfig {
            vcpu_count: 2,
            mem_size_mib: 512,
            smt: None,
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("vcpu_count"));
        assert!(!json.contains("smt")); // Should be skipped when None
    }
}
