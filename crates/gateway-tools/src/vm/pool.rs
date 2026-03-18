//! VM Pool Management
//!
//! Manages a pool of pre-warmed Firecracker VMs for fast acquisition.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// VM pool configuration
#[derive(Debug, Clone)]
pub struct VmPoolConfig {
    /// Minimum number of VMs to keep ready in the pool
    pub min_pool_size: usize,
    /// Maximum number of VMs allowed in the pool
    pub max_pool_size: usize,
    /// Maximum age of a VM before it's recycled (seconds)
    pub max_vm_age_secs: u64,
    /// Health check interval (seconds)
    pub health_check_interval_secs: u64,
    /// Base subnet for VM IPs (e.g., 172.16.0.0)
    pub base_subnet: Ipv4Addr,
    /// API port on VM
    pub api_port: u16,
    /// VNC port on VM
    pub vnc_port: u16,
}

impl Default for VmPoolConfig {
    fn default() -> Self {
        Self {
            min_pool_size: 2,
            max_pool_size: 10,
            max_vm_age_secs: 3600, // 1 hour
            health_check_interval_secs: 30,
            base_subnet: Ipv4Addr::new(172, 16, 0, 0),
            api_port: 8080,
            vnc_port: 5900,
        }
    }
}

/// Entry in the VM pool
#[derive(Debug, Clone)]
pub struct PoolEntry {
    /// VM identifier
    pub vm_id: String,
    /// VM IP address
    pub ip: Ipv4Addr,
    /// When the VM was created
    pub created_at: Instant,
    /// Last health check time
    pub last_health_check: Instant,
    /// Whether VM is currently healthy
    pub healthy: bool,
    /// VM index (used for IP allocation)
    pub index: u32,
}

/// Statistics about the VM pool
#[derive(Debug, Clone, Default)]
pub struct VmPoolStats {
    /// Number of available VMs in pool
    pub available: usize,
    /// Number of VMs currently in use
    pub in_use: usize,
    /// Total VMs created
    pub total_created: u64,
    /// Total VMs destroyed
    pub total_destroyed: u64,
    /// Total acquire requests
    pub total_acquires: u64,
    /// Total release requests
    pub total_releases: u64,
    /// Failed health checks
    pub failed_health_checks: u64,
}

/// VM Pool for managing pre-warmed Firecracker VMs
pub struct VmPool {
    config: VmPoolConfig,
    /// Available VMs ready for use
    available: RwLock<VecDeque<PoolEntry>>,
    /// VMs currently in use
    in_use: RwLock<Vec<PoolEntry>>,
    /// Counter for VM IDs
    vm_counter: AtomicU64,
    /// Statistics
    stats: Arc<RwLock<VmPoolStats>>,
    /// Index allocator for IPs
    next_index: AtomicU64,
    /// Released indices for reuse
    released_indices: RwLock<Vec<u32>>,
}

impl VmPool {
    /// Create a new VM pool
    pub fn new(config: VmPoolConfig) -> Self {
        Self {
            config,
            available: RwLock::new(VecDeque::new()),
            in_use: RwLock::new(Vec::new()),
            vm_counter: AtomicU64::new(0),
            stats: Arc::new(RwLock::new(VmPoolStats::default())),
            next_index: AtomicU64::new(0),
            released_indices: RwLock::new(Vec::new()),
        }
    }

    /// Get pool configuration
    pub fn config(&self) -> &VmPoolConfig {
        &self.config
    }

    /// Generate a new VM ID
    pub fn generate_vm_id(&self) -> String {
        let counter = self.vm_counter.fetch_add(1, Ordering::SeqCst);
        format!("vm-{}-{}", counter, uuid::Uuid::new_v4().as_simple())
    }

    /// Allocate an index for IP addressing
    pub async fn allocate_index(&self) -> u32 {
        // Try to reuse a released index first
        {
            let mut released = self.released_indices.write().await;
            if let Some(index) = released.pop() {
                return index;
            }
        }
        // Otherwise allocate a new one
        self.next_index.fetch_add(1, Ordering::SeqCst) as u32
    }

    /// Release an index for reuse
    pub async fn release_index(&self, index: u32) {
        let mut released = self.released_indices.write().await;
        if !released.contains(&index) {
            released.push(index);
        }
    }

    /// Calculate IP address for a VM index
    pub fn ip_for_index(&self, index: u32) -> Ipv4Addr {
        let base = u32::from(self.config.base_subnet);
        // VM gets .2 in its subnet (gateway is .1)
        let vm_ip = base + ((index as u32) << 8) + 2;
        Ipv4Addr::from(vm_ip)
    }

    /// Calculate gateway IP for a VM index
    pub fn gateway_for_index(&self, index: u32) -> Ipv4Addr {
        let base = u32::from(self.config.base_subnet);
        // Gateway is .1 in the subnet
        let gw_ip = base + ((index as u32) << 8) + 1;
        Ipv4Addr::from(gw_ip)
    }

    /// Calculate TAP device name for a VM index
    pub fn tap_for_index(&self, index: u32) -> String {
        format!("tap{}", index)
    }

    /// Add a VM to the available pool
    pub async fn add_available(&self, entry: PoolEntry) -> Result<()> {
        let mut available = self.available.write().await;

        if available.len() >= self.config.max_pool_size {
            return Err(Error::Internal("Pool is at maximum capacity".to_string()));
        }

        available.push_back(entry);

        let mut stats = self.stats.write().await;
        stats.total_created += 1;

        debug!(
            available = available.len(),
            max = self.config.max_pool_size,
            "VM added to pool"
        );

        Ok(())
    }

    /// Acquire a VM from the pool
    pub async fn acquire(&self) -> Option<PoolEntry> {
        let mut available = self.available.write().await;

        // Find a healthy VM
        let mut entry = None;
        while let Some(candidate) = available.pop_front() {
            if candidate.healthy {
                entry = Some(candidate);
                break;
            } else {
                // Unhealthy VM - will be cleaned up separately
                warn!(vm_id = %candidate.vm_id, "Skipping unhealthy VM");
            }
        }

        if let Some(ref e) = entry {
            let mut in_use = self.in_use.write().await;
            in_use.push(e.clone());

            let mut stats = self.stats.write().await;
            stats.total_acquires += 1;
            stats.available = available.len();
            stats.in_use = in_use.len();

            info!(
                vm_id = %e.vm_id,
                ip = %e.ip,
                available = available.len(),
                "VM acquired from pool"
            );
        }

        entry
    }

    /// Release a VM back to the pool
    pub async fn release(&self, vm_id: &str) -> Option<PoolEntry> {
        let mut in_use = self.in_use.write().await;

        // Find and remove from in_use
        let pos = in_use.iter().position(|e| e.vm_id == vm_id)?;
        let mut entry = in_use.remove(pos);

        // Update last health check time
        entry.last_health_check = Instant::now();

        let mut available = self.available.write().await;

        // Check if VM is too old
        let age_secs = entry.created_at.elapsed().as_secs();
        if age_secs > self.config.max_vm_age_secs {
            info!(
                vm_id = %entry.vm_id,
                age_secs = age_secs,
                "VM too old, will be destroyed"
            );
            let mut stats = self.stats.write().await;
            stats.total_releases += 1;
            stats.total_destroyed += 1;
            return None;
        }

        // Return to pool if there's space
        if available.len() < self.config.max_pool_size {
            available.push_back(entry.clone());

            let mut stats = self.stats.write().await;
            stats.total_releases += 1;
            stats.available = available.len();
            stats.in_use = in_use.len();

            info!(
                vm_id = %entry.vm_id,
                available = available.len(),
                "VM released back to pool"
            );

            Some(entry)
        } else {
            info!(
                vm_id = %entry.vm_id,
                "Pool full, VM will be destroyed"
            );
            let mut stats = self.stats.write().await;
            stats.total_releases += 1;
            stats.total_destroyed += 1;
            None
        }
    }

    /// Remove a VM from tracking (both available and in_use)
    pub async fn remove(&self, vm_id: &str) -> Option<PoolEntry> {
        // Try to remove from available
        {
            let mut available = self.available.write().await;
            if let Some(pos) = available.iter().position(|e| e.vm_id == vm_id) {
                let entry = available.remove(pos).unwrap();
                let mut stats = self.stats.write().await;
                stats.total_destroyed += 1;
                stats.available = available.len();
                return Some(entry);
            }
        }

        // Try to remove from in_use
        {
            let mut in_use = self.in_use.write().await;
            if let Some(pos) = in_use.iter().position(|e| e.vm_id == vm_id) {
                let entry = in_use.remove(pos);
                let mut stats = self.stats.write().await;
                stats.total_destroyed += 1;
                stats.in_use = in_use.len();
                return Some(entry);
            }
        }

        None
    }

    /// Mark a VM as unhealthy
    pub async fn mark_unhealthy(&self, vm_id: &str) {
        // Check available pool
        {
            let mut available = self.available.write().await;
            for entry in available.iter_mut() {
                if entry.vm_id == vm_id {
                    entry.healthy = false;
                    let mut stats = self.stats.write().await;
                    stats.failed_health_checks += 1;
                    return;
                }
            }
        }

        // Check in_use pool
        {
            let mut in_use = self.in_use.write().await;
            for entry in in_use.iter_mut() {
                if entry.vm_id == vm_id {
                    entry.healthy = false;
                    let mut stats = self.stats.write().await;
                    stats.failed_health_checks += 1;
                    return;
                }
            }
        }
    }

    /// Mark a VM as healthy
    pub async fn mark_healthy(&self, vm_id: &str) {
        // Check available pool
        {
            let mut available = self.available.write().await;
            for entry in available.iter_mut() {
                if entry.vm_id == vm_id {
                    entry.healthy = true;
                    entry.last_health_check = Instant::now();
                    return;
                }
            }
        }

        // Check in_use pool
        {
            let mut in_use = self.in_use.write().await;
            for entry in in_use.iter_mut() {
                if entry.vm_id == vm_id {
                    entry.healthy = true;
                    entry.last_health_check = Instant::now();
                    return;
                }
            }
        }
    }

    /// Get all VMs that need health checks
    pub async fn get_stale_vms(&self) -> Vec<PoolEntry> {
        let threshold = std::time::Duration::from_secs(self.config.health_check_interval_secs);
        let mut stale = Vec::new();

        {
            let available = self.available.read().await;
            for entry in available.iter() {
                if entry.last_health_check.elapsed() > threshold {
                    stale.push(entry.clone());
                }
            }
        }

        stale
    }

    /// Get VMs that are too old and should be removed
    pub async fn get_expired_vms(&self) -> Vec<PoolEntry> {
        let max_age = std::time::Duration::from_secs(self.config.max_vm_age_secs);
        let mut expired = Vec::new();

        {
            let available = self.available.read().await;
            for entry in available.iter() {
                if entry.created_at.elapsed() > max_age {
                    expired.push(entry.clone());
                }
            }
        }

        expired
    }

    /// Get unhealthy VMs that should be removed
    pub async fn get_unhealthy_vms(&self) -> Vec<PoolEntry> {
        let available = self.available.read().await;
        available.iter().filter(|e| !e.healthy).cloned().collect()
    }

    /// Get pool statistics
    pub async fn stats(&self) -> VmPoolStats {
        let available = self.available.read().await;
        let in_use = self.in_use.read().await;
        let mut stats = self.stats.read().await.clone();
        stats.available = available.len();
        stats.in_use = in_use.len();
        stats
    }

    /// Get current pool size
    pub async fn available_count(&self) -> usize {
        self.available.read().await.len()
    }

    /// Get number of VMs in use
    pub async fn in_use_count(&self) -> usize {
        self.in_use.read().await.len()
    }

    /// Get total VM count (available + in_use)
    pub async fn total_count(&self) -> usize {
        let available = self.available.read().await.len();
        let in_use = self.in_use.read().await.len();
        available + in_use
    }

    /// Check if pool needs more VMs
    pub async fn needs_replenishment(&self) -> bool {
        self.available.read().await.len() < self.config.min_pool_size
    }

    /// Get all VM IDs (for shutdown)
    pub async fn all_vm_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();

        {
            let available = self.available.read().await;
            ids.extend(available.iter().map(|e| e.vm_id.clone()));
        }

        {
            let in_use = self.in_use.read().await;
            ids.extend(in_use.iter().map(|e| e.vm_id.clone()));
        }

        ids
    }

    /// Clear all VMs from pool tracking
    pub async fn clear(&self) -> Vec<PoolEntry> {
        let mut all = Vec::new();

        {
            let mut available = self.available.write().await;
            all.extend(available.drain(..));
        }

        {
            let mut in_use = self.in_use.write().await;
            all.extend(in_use.drain(..));
        }

        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_entry(vm_id: &str, index: u32) -> PoolEntry {
        PoolEntry {
            vm_id: vm_id.to_string(),
            ip: Ipv4Addr::new(172, 16, index as u8, 2),
            created_at: Instant::now(),
            last_health_check: Instant::now(),
            healthy: true,
            index,
        }
    }

    #[tokio::test]
    async fn test_pool_add_and_acquire() {
        let pool = VmPool::new(VmPoolConfig::default());

        let entry = create_test_entry("vm-1", 0);
        pool.add_available(entry.clone()).await.unwrap();

        assert_eq!(pool.available_count().await, 1);

        let acquired = pool.acquire().await.unwrap();
        assert_eq!(acquired.vm_id, "vm-1");
        assert_eq!(pool.available_count().await, 0);
        assert_eq!(pool.in_use_count().await, 1);
    }

    #[tokio::test]
    async fn test_pool_release() {
        let pool = VmPool::new(VmPoolConfig::default());

        let entry = create_test_entry("vm-1", 0);
        pool.add_available(entry).await.unwrap();

        let _ = pool.acquire().await.unwrap();
        assert_eq!(pool.in_use_count().await, 1);

        let released = pool.release("vm-1").await;
        assert!(released.is_some());
        assert_eq!(pool.available_count().await, 1);
        assert_eq!(pool.in_use_count().await, 0);
    }

    #[tokio::test]
    async fn test_pool_max_capacity() {
        let config = VmPoolConfig {
            max_pool_size: 2,
            ..Default::default()
        };
        let pool = VmPool::new(config);

        pool.add_available(create_test_entry("vm-1", 0))
            .await
            .unwrap();
        pool.add_available(create_test_entry("vm-2", 1))
            .await
            .unwrap();

        let result = pool.add_available(create_test_entry("vm-3", 2)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ip_allocation() {
        let pool = VmPool::new(VmPoolConfig::default());

        let ip0 = pool.ip_for_index(0);
        let ip1 = pool.ip_for_index(1);
        let ip2 = pool.ip_for_index(2);

        assert_eq!(ip0, Ipv4Addr::new(172, 16, 0, 2));
        assert_eq!(ip1, Ipv4Addr::new(172, 16, 1, 2));
        assert_eq!(ip2, Ipv4Addr::new(172, 16, 2, 2));
    }

    #[tokio::test]
    async fn test_gateway_allocation() {
        let pool = VmPool::new(VmPoolConfig::default());

        let gw0 = pool.gateway_for_index(0);
        let gw1 = pool.gateway_for_index(1);

        assert_eq!(gw0, Ipv4Addr::new(172, 16, 0, 1));
        assert_eq!(gw1, Ipv4Addr::new(172, 16, 1, 1));
    }

    #[tokio::test]
    async fn test_tap_device_names() {
        let pool = VmPool::new(VmPoolConfig::default());

        assert_eq!(pool.tap_for_index(0), "tap0");
        assert_eq!(pool.tap_for_index(5), "tap5");
    }

    #[tokio::test]
    async fn test_index_allocation_and_release() {
        let pool = VmPool::new(VmPoolConfig::default());

        let idx0 = pool.allocate_index().await;
        let idx1 = pool.allocate_index().await;

        assert_eq!(idx0, 0);
        assert_eq!(idx1, 1);

        // Release idx0
        pool.release_index(idx0).await;

        // Next allocation should reuse idx0
        let idx2 = pool.allocate_index().await;
        assert_eq!(idx2, 0);
    }

    #[tokio::test]
    async fn test_mark_unhealthy() {
        let pool = VmPool::new(VmPoolConfig::default());

        pool.add_available(create_test_entry("vm-1", 0))
            .await
            .unwrap();
        pool.mark_unhealthy("vm-1").await;

        // Acquire should skip unhealthy VM
        let acquired = pool.acquire().await;
        assert!(acquired.is_none());
    }

    #[tokio::test]
    async fn test_stats() {
        let pool = VmPool::new(VmPoolConfig::default());

        pool.add_available(create_test_entry("vm-1", 0))
            .await
            .unwrap();
        pool.add_available(create_test_entry("vm-2", 1))
            .await
            .unwrap();

        let _ = pool.acquire().await;

        let stats = pool.stats().await;
        assert_eq!(stats.available, 1);
        assert_eq!(stats.in_use, 1);
        assert_eq!(stats.total_created, 2);
        assert_eq!(stats.total_acquires, 1);
    }

    #[tokio::test]
    async fn test_needs_replenishment() {
        let config = VmPoolConfig {
            min_pool_size: 3,
            ..Default::default()
        };
        let pool = VmPool::new(config);

        assert!(pool.needs_replenishment().await);

        pool.add_available(create_test_entry("vm-1", 0))
            .await
            .unwrap();
        pool.add_available(create_test_entry("vm-2", 1))
            .await
            .unwrap();

        assert!(pool.needs_replenishment().await);

        pool.add_available(create_test_entry("vm-3", 2))
            .await
            .unwrap();

        assert!(!pool.needs_replenishment().await);
    }

    #[tokio::test]
    async fn test_all_vm_ids() {
        let pool = VmPool::new(VmPoolConfig::default());

        pool.add_available(create_test_entry("vm-1", 0))
            .await
            .unwrap();
        pool.add_available(create_test_entry("vm-2", 1))
            .await
            .unwrap();

        let _ = pool.acquire().await;

        let ids = pool.all_vm_ids().await;
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"vm-1".to_string()) || ids.contains(&"vm-2".to_string()));
    }

    #[tokio::test]
    async fn test_clear() {
        let pool = VmPool::new(VmPoolConfig::default());

        pool.add_available(create_test_entry("vm-1", 0))
            .await
            .unwrap();
        pool.add_available(create_test_entry("vm-2", 1))
            .await
            .unwrap();
        let _ = pool.acquire().await;

        let cleared = pool.clear().await;
        assert_eq!(cleared.len(), 2);
        assert_eq!(pool.total_count().await, 0);
    }
}
