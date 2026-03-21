//! VM Configuration types for Firecracker microVMs.

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::PathBuf;

/// Configuration for a Firecracker microVM instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    /// Unique identifier for the VM instance.
    pub id: String,
    /// Number of virtual CPUs (1-32).
    pub vcpu_count: u8,
    /// Memory size in MiB (128-32768).
    pub mem_size_mib: u32,
    /// Path to the root filesystem image.
    pub rootfs_path: PathBuf,
    /// Path to the kernel image.
    pub kernel_path: PathBuf,
    /// TAP device name for networking.
    pub tap_device: String,
    /// IP address assigned to the VM.
    pub ip_address: Ipv4Addr,
    /// Optional kernel boot arguments.
    #[serde(default)]
    pub boot_args: Option<String>,
    /// Optional additional disk drives.
    #[serde(default)]
    pub additional_drives: Vec<DriveConfig>,
}

impl VmConfig {
    /// Create a new VM configuration with default values.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            vcpu_count: 1,
            mem_size_mib: 512,
            rootfs_path: PathBuf::from("/var/lib/firecracker/rootfs.ext4"),
            kernel_path: PathBuf::from("/var/lib/firecracker/vmlinux"),
            tap_device: String::from("tap0"),
            ip_address: Ipv4Addr::new(172, 16, 0, 2),
            boot_args: Some(String::from("console=ttyS0 reboot=k panic=1 pci=off")),
            additional_drives: Vec::new(),
        }
    }

    /// Set the number of vCPUs.
    pub fn with_vcpu_count(mut self, count: u8) -> Self {
        self.vcpu_count = count.clamp(1, 32);
        self
    }

    /// Set the memory size in MiB.
    pub fn with_mem_size_mib(mut self, size: u32) -> Self {
        self.mem_size_mib = size.clamp(128, 32768);
        self
    }

    /// Set the root filesystem path.
    pub fn with_rootfs_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.rootfs_path = path.into();
        self
    }

    /// Set the kernel path.
    pub fn with_kernel_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.kernel_path = path.into();
        self
    }

    /// Set the TAP device name.
    pub fn with_tap_device(mut self, device: impl Into<String>) -> Self {
        self.tap_device = device.into();
        self
    }

    /// Set the IP address.
    pub fn with_ip_address(mut self, ip: Ipv4Addr) -> Self {
        self.ip_address = ip;
        self
    }

    /// Set boot arguments.
    pub fn with_boot_args(mut self, args: impl Into<String>) -> Self {
        self.boot_args = Some(args.into());
        self
    }

    /// Add an additional drive.
    pub fn with_drive(mut self, drive: DriveConfig) -> Self {
        self.additional_drives.push(drive);
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.id.is_empty() {
            return Err(ConfigError::InvalidId("VM ID cannot be empty".into()));
        }

        if self.vcpu_count == 0 || self.vcpu_count > 32 {
            return Err(ConfigError::InvalidVcpuCount(self.vcpu_count));
        }

        if self.mem_size_mib < 128 || self.mem_size_mib > 32768 {
            return Err(ConfigError::InvalidMemorySize(self.mem_size_mib));
        }

        if !self.rootfs_path.as_os_str().is_empty() && !self.rootfs_path.exists() {
            return Err(ConfigError::RootfsNotFound(self.rootfs_path.clone()));
        }

        if !self.kernel_path.as_os_str().is_empty() && !self.kernel_path.exists() {
            return Err(ConfigError::KernelNotFound(self.kernel_path.clone()));
        }

        Ok(())
    }
}

impl Default for VmConfig {
    fn default() -> Self {
        Self::new("default")
    }
}

/// Configuration for an additional drive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveConfig {
    /// Drive identifier.
    pub drive_id: String,
    /// Path to the drive image.
    pub path_on_host: PathBuf,
    /// Whether the drive is read-only.
    pub is_read_only: bool,
    /// Whether this is the root device.
    pub is_root_device: bool,
}

impl DriveConfig {
    /// Create a new drive configuration.
    pub fn new(drive_id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            drive_id: drive_id.into(),
            path_on_host: path.into(),
            is_read_only: false,
            is_root_device: false,
        }
    }

    /// Set the drive as read-only.
    pub fn read_only(mut self) -> Self {
        self.is_read_only = true;
        self
    }

    /// Set the drive as the root device.
    pub fn as_root_device(mut self) -> Self {
        self.is_root_device = true;
        self
    }
}

/// Configuration for the VM pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Maximum number of VM instances.
    pub max_instances: usize,
    /// Number of warm (pre-created) instances to maintain.
    pub warm_pool_size: usize,
    /// Idle timeout in seconds before a VM is destroyed.
    pub idle_timeout_secs: u64,
    /// Path to the Firecracker binary.
    pub firecracker_bin: PathBuf,
    /// Base path for Unix sockets.
    pub socket_base_path: PathBuf,
    /// Default VM configuration template.
    pub default_vm_config: VmConfig,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_instances: 10,
            warm_pool_size: 2,
            idle_timeout_secs: 300,
            firecracker_bin: PathBuf::from("/usr/bin/firecracker"),
            socket_base_path: PathBuf::from("/tmp/firecracker"),
            default_vm_config: VmConfig::default(),
        }
    }
}

impl PoolConfig {
    /// Create a new pool configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of instances.
    pub fn with_max_instances(mut self, max: usize) -> Self {
        self.max_instances = max;
        self
    }

    /// Set the warm pool size.
    pub fn with_warm_pool_size(mut self, size: usize) -> Self {
        self.warm_pool_size = size.min(self.max_instances);
        self
    }

    /// Set the idle timeout.
    pub fn with_idle_timeout(mut self, secs: u64) -> Self {
        self.idle_timeout_secs = secs;
        self
    }

    /// Set the Firecracker binary path.
    pub fn with_firecracker_bin(mut self, path: impl Into<PathBuf>) -> Self {
        self.firecracker_bin = path.into();
        self
    }

    /// Set the socket base path.
    pub fn with_socket_base_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.socket_base_path = path.into();
        self
    }

    /// Set the default VM configuration.
    pub fn with_default_vm_config(mut self, config: VmConfig) -> Self {
        self.default_vm_config = config;
        self
    }
}

/// Network configuration for a VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Interface ID (e.g., "eth0").
    pub iface_id: String,
    /// TAP device name on the host.
    pub host_dev_name: String,
    /// Guest MAC address.
    pub guest_mac: Option<String>,
}

impl NetworkConfig {
    /// Create a new network configuration.
    pub fn new(iface_id: impl Into<String>, host_dev_name: impl Into<String>) -> Self {
        Self {
            iface_id: iface_id.into(),
            host_dev_name: host_dev_name.into(),
            guest_mac: None,
        }
    }

    /// Set the guest MAC address.
    pub fn with_guest_mac(mut self, mac: impl Into<String>) -> Self {
        self.guest_mac = Some(mac.into());
        self
    }
}

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Invalid VM ID: {0}")]
    InvalidId(String),

    #[error("Invalid vCPU count: {0} (must be 1-32)")]
    InvalidVcpuCount(u8),

    #[error("Invalid memory size: {0} MiB (must be 128-32768)")]
    InvalidMemorySize(u32),

    #[error("Root filesystem not found: {0}")]
    RootfsNotFound(PathBuf),

    #[error("Kernel not found: {0}")]
    KernelNotFound(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_config_new() {
        let config = VmConfig::new("test-vm");
        assert_eq!(config.id, "test-vm");
        assert_eq!(config.vcpu_count, 1);
        assert_eq!(config.mem_size_mib, 512);
    }

    #[test]
    fn test_vm_config_builder() {
        let config = VmConfig::new("test-vm")
            .with_vcpu_count(4)
            .with_mem_size_mib(2048)
            .with_ip_address(Ipv4Addr::new(10, 0, 0, 5));

        assert_eq!(config.vcpu_count, 4);
        assert_eq!(config.mem_size_mib, 2048);
        assert_eq!(config.ip_address, Ipv4Addr::new(10, 0, 0, 5));
    }

    #[test]
    fn test_vm_config_clamp_values() {
        let config = VmConfig::new("test")
            .with_vcpu_count(100) // Should be clamped to 32
            .with_mem_size_mib(50); // Should be clamped to 128

        assert_eq!(config.vcpu_count, 32);
        assert_eq!(config.mem_size_mib, 128);
    }

    #[test]
    fn test_drive_config() {
        let drive = DriveConfig::new("data", "/mnt/data.ext4")
            .read_only()
            .as_root_device();

        assert_eq!(drive.drive_id, "data");
        assert!(drive.is_read_only);
        assert!(drive.is_root_device);
    }

    #[test]
    fn test_pool_config_default() {
        let config = PoolConfig::default();
        assert_eq!(config.max_instances, 10);
        assert_eq!(config.warm_pool_size, 2);
        assert_eq!(config.idle_timeout_secs, 300);
    }

    #[test]
    fn test_pool_config_builder() {
        let config = PoolConfig::new()
            .with_max_instances(20)
            .with_warm_pool_size(5)
            .with_idle_timeout(600);

        assert_eq!(config.max_instances, 20);
        assert_eq!(config.warm_pool_size, 5);
        assert_eq!(config.idle_timeout_secs, 600);
    }

    #[test]
    fn test_network_config() {
        let config = NetworkConfig::new("eth0", "tap0").with_guest_mac("AA:BB:CC:DD:EE:FF");

        assert_eq!(config.iface_id, "eth0");
        assert_eq!(config.host_dev_name, "tap0");
        assert_eq!(config.guest_mac, Some("AA:BB:CC:DD:EE:FF".to_string()));
    }
}
