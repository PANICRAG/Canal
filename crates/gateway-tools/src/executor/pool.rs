//! Container Pool
//!
//! This module provides a container pool that manages warm containers
//! for efficient code execution. It handles container lifecycle, health checks,
//! and automatic recycling.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::time::{Duration, Instant};

use super::container::ContainerManager;
use super::types::{ContainerConfig, ContainerRuntime, PoolConfig, PoolStats};

/// A warm container entry in the pool
#[derive(Debug)]
struct WarmContainer {
    /// Container runtime info
    runtime: ContainerRuntime,
    /// When this container became warm
    warmed_at: Instant,
}

impl WarmContainer {
    fn new(runtime: ContainerRuntime) -> Self {
        Self {
            runtime,
            warmed_at: Instant::now(),
        }
    }

    /// Check if container has exceeded TTL
    fn is_expired(&self, ttl: Duration) -> bool {
        self.warmed_at.elapsed() > ttl
    }
}

/// Container pool for managing warm containers
pub struct ContainerPool {
    /// Container manager
    manager: Arc<ContainerManager>,
    /// Pool configuration
    config: PoolConfig,
    /// Warm containers available for reuse (by image)
    warm_containers: Arc<Mutex<std::collections::HashMap<String, VecDeque<WarmContainer>>>>,
    /// Pool statistics
    stats: Arc<RwLock<PoolStats>>,
    /// Semaphore for limiting total containers
    container_semaphore: Arc<Semaphore>,
    /// Shutdown flag
    shutdown: Arc<RwLock<bool>>,
}

impl ContainerPool {
    /// Create a new container pool
    pub async fn new(config: PoolConfig) -> Result<Self> {
        let manager = Arc::new(ContainerManager::with_prefix(&config.container_prefix).await?);

        let pool = Self {
            manager,
            container_semaphore: Arc::new(Semaphore::new(config.max_containers)),
            config,
            warm_containers: Arc::new(Mutex::new(std::collections::HashMap::new())),
            stats: Arc::new(RwLock::new(PoolStats::default())),
            shutdown: Arc::new(RwLock::new(false)),
        };

        Ok(pool)
    }

    /// Create a pool with an existing container manager (for testing)
    #[cfg(test)]
    pub fn with_manager(manager: Arc<ContainerManager>, config: PoolConfig) -> Self {
        Self {
            manager,
            container_semaphore: Arc::new(Semaphore::new(config.max_containers)),
            config,
            warm_containers: Arc::new(Mutex::new(std::collections::HashMap::new())),
            stats: Arc::new(RwLock::new(PoolStats::default())),
            shutdown: Arc::new(RwLock::new(false)),
        }
    }

    /// Start the pool (begins background maintenance tasks)
    pub async fn start(&self) -> Result<()> {
        // Clean up any orphaned containers from previous runs
        let removed = self.manager.cleanup_orphaned().await?;
        if removed > 0 {
            tracing::info!("Cleaned up {} orphaned containers", removed);
        }

        // Pre-warm containers
        self.prewarm().await?;

        // Start background maintenance task
        self.spawn_maintenance_task();

        Ok(())
    }

    /// Pre-warm the pool with containers
    pub async fn prewarm(&self) -> Result<()> {
        let image = &self.config.default_warm_image;
        let needed = self.config.min_warm_containers;

        tracing::info!("Pre-warming {} containers with image {}", needed, image);

        for _ in 0..needed {
            if let Err(e) = self.create_warm_container(image).await {
                tracing::warn!("Failed to create warm container: {}", e);
            }
        }

        Ok(())
    }

    /// Acquire a container for use
    ///
    /// This will first try to get a warm container from the pool.
    /// If none available, it will create a new one.
    pub async fn acquire(&self, config: ContainerConfig) -> Result<ContainerRuntime> {
        // Check if shutting down
        if *self.shutdown.read().await {
            return Err(Error::Internal("Pool is shutting down".into()));
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.requests_served += 1;
        }

        // Try to get a warm container
        if let Some(container) = self.get_warm_container(&config.image).await {
            {
                let mut stats = self.stats.write().await;
                stats.cache_hits += 1;
            }
            tracing::debug!(
                "Reusing warm container {} for image {}",
                container.config.id,
                config.image
            );
            return Ok(container);
        }

        // No warm container available, create a new one
        {
            let mut stats = self.stats.write().await;
            stats.cache_misses += 1;
        }

        self.create_container(config).await
    }

    /// Release a container back to the pool
    ///
    /// The container may be kept warm for reuse or destroyed based on
    /// pool policy and container health.
    pub async fn release(&self, id: &str, keep_warm: bool) -> Result<()> {
        let runtime = match self.manager.get(id).await {
            Ok(r) => r,
            Err(_) => {
                // Container already removed, nothing to do
                return Ok(());
            }
        };

        // Check if container should be kept warm
        let should_keep = keep_warm
            && runtime.healthy
            && runtime.reuse_count < self.config.max_reuse_count
            && !*self.shutdown.read().await;

        if should_keep {
            // Mark as warm and return to pool
            self.manager.mark_warm(id).await?;

            let updated_runtime = self.manager.get(id).await?;
            let warm = WarmContainer::new(updated_runtime.clone());

            let mut warm_containers = self.warm_containers.lock().await;
            let queue = warm_containers
                .entry(updated_runtime.config.image.clone())
                .or_insert_with(VecDeque::new);
            queue.push_back(warm);

            {
                let mut stats = self.stats.write().await;
                stats.warm_containers += 1;
            }

            tracing::debug!("Container {} returned to warm pool", id);
        } else {
            // Destroy the container
            self.destroy(id).await?;
        }

        Ok(())
    }

    /// Destroy a container
    pub async fn destroy(&self, id: &str) -> Result<()> {
        self.manager.stop_and_remove(id, true).await?;
        self.container_semaphore.add_permits(1);

        {
            let mut stats = self.stats.write().await;
            stats.total_containers = stats.total_containers.saturating_sub(1);
            stats.containers_recycled += 1;
        }

        tracing::debug!("Destroyed container {}", id);
        Ok(())
    }

    /// Execute a command in a container from the pool
    pub async fn execute(
        &self,
        config: ContainerConfig,
        command: Vec<String>,
    ) -> Result<(String, String, i32)> {
        let container = self.acquire(config).await?;
        let id = container.config.id.clone();

        // Execute the command
        let result = self.manager.exec(&id, command).await;

        // Release the container (keep warm if execution succeeded)
        let keep_warm = result
            .as_ref()
            .map(|(_, _, code)| *code == 0)
            .unwrap_or(false);
        if let Err(e) = self.release(&id, keep_warm).await {
            tracing::warn!("Failed to release container: {}", e);
        }

        result
    }

    /// Get pool statistics
    pub async fn stats(&self) -> PoolStats {
        let stats = self.stats.read().await;
        stats.clone()
    }

    /// Get current pool state
    pub async fn state(&self) -> PoolState {
        let warm_containers = self.warm_containers.lock().await;
        let total_warm: usize = warm_containers.values().map(|q| q.len()).sum();

        let stats = self.stats.read().await;
        let managed = self.manager.count().await;

        PoolState {
            total_containers: managed,
            warm_containers: total_warm,
            running_containers: stats.running_containers,
            max_containers: self.config.max_containers,
            available_permits: self.container_semaphore.available_permits(),
        }
    }

    /// Perform health check on all containers
    pub async fn health_check(&self) -> Result<HealthCheckResult> {
        let mut healthy = 0;
        let mut unhealthy = 0;
        let mut removed = 0;

        // Check Docker daemon
        if !self.manager.health_check().await? {
            return Err(Error::Docker("Docker daemon unhealthy".into()));
        }

        // Check warm containers
        let mut warm_containers = self.warm_containers.lock().await;
        let ttl = Duration::from_secs(self.config.warm_ttl_seconds);

        for (_image, queue) in warm_containers.iter_mut() {
            let mut i = 0;
            while i < queue.len() {
                let container = &queue[i];

                // Check if expired
                if container.is_expired(ttl) {
                    if let Some(removed_container) = queue.remove(i) {
                        let _ = self
                            .manager
                            .stop_and_remove(&removed_container.runtime.config.id, true)
                            .await;
                        self.container_semaphore.add_permits(1);
                        removed += 1;
                    }
                    continue;
                }

                // Check container health
                if container.runtime.healthy {
                    healthy += 1;
                } else {
                    unhealthy += 1;
                    if let Some(removed_container) = queue.remove(i) {
                        let _ = self
                            .manager
                            .stop_and_remove(&removed_container.runtime.config.id, true)
                            .await;
                        self.container_semaphore.add_permits(1);
                        removed += 1;
                    }
                    continue;
                }

                i += 1;
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.health_check_failures += unhealthy as u64;
        }

        Ok(HealthCheckResult {
            healthy,
            unhealthy,
            removed,
        })
    }

    /// Shutdown the pool
    pub async fn shutdown(&self) -> Result<()> {
        tracing::info!("Shutting down container pool");

        // Set shutdown flag
        {
            let mut shutdown = self.shutdown.write().await;
            *shutdown = true;
        }

        // Destroy all warm containers
        let mut warm_containers = self.warm_containers.lock().await;
        for (_image, queue) in warm_containers.drain() {
            for container in queue {
                let _ = self
                    .manager
                    .stop_and_remove(&container.runtime.config.id, true)
                    .await;
            }
        }

        // Destroy all remaining managed containers
        let containers = self.manager.list().await;
        for container in containers {
            let _ = self
                .manager
                .stop_and_remove(&container.config.id, true)
                .await;
        }

        tracing::info!("Container pool shutdown complete");
        Ok(())
    }

    // === Private helpers ===

    /// Create a new container
    async fn create_container(&self, config: ContainerConfig) -> Result<ContainerRuntime> {
        // Acquire permit (blocks if at max capacity)
        let permit = self
            .container_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Internal("Failed to acquire container permit".into()))?;

        // R5-M15: Only forget permit AFTER successful creation.
        // If create_and_start fails, dropping `permit` returns it to the semaphore.
        let runtime = match self.manager.create_and_start(config).await {
            Ok(rt) => {
                std::mem::forget(permit);
                rt
            }
            Err(e) => {
                drop(permit);
                return Err(e);
            }
        };

        {
            let mut stats = self.stats.write().await;
            stats.total_containers += 1;
            stats.running_containers += 1;
            stats.containers_created += 1;
        }

        Ok(runtime)
    }

    /// Create a warm container for the pool
    async fn create_warm_container(&self, image: &str) -> Result<()> {
        let config =
            ContainerConfig::new(image).with_command(vec!["sleep".into(), "infinity".into()]);

        let runtime = self.create_container(config).await?;
        self.manager.mark_warm(&runtime.config.id).await?;

        let updated_runtime = self.manager.get(&runtime.config.id).await?;
        let warm = WarmContainer::new(updated_runtime.clone());

        let mut warm_containers = self.warm_containers.lock().await;
        let queue = warm_containers
            .entry(image.to_string())
            .or_insert_with(VecDeque::new);
        queue.push_back(warm);

        {
            let mut stats = self.stats.write().await;
            stats.warm_containers += 1;
        }

        Ok(())
    }

    /// Get a warm container from the pool
    async fn get_warm_container(&self, image: &str) -> Option<ContainerRuntime> {
        let mut warm_containers = self.warm_containers.lock().await;
        let ttl = Duration::from_secs(self.config.warm_ttl_seconds);

        if let Some(queue) = warm_containers.get_mut(image) {
            // Find a non-expired container
            while let Some(container) = queue.pop_front() {
                if !container.is_expired(ttl) && container.runtime.healthy {
                    {
                        let mut stats = self.stats.write().await;
                        stats.warm_containers = stats.warm_containers.saturating_sub(1);
                    }
                    return Some(container.runtime);
                } else {
                    // Expired or unhealthy, destroy it
                    let _ = self
                        .manager
                        .stop_and_remove(&container.runtime.config.id, true)
                        .await;
                    self.container_semaphore.add_permits(1);
                }
            }
        }

        None
    }

    /// Spawn background maintenance task
    fn spawn_maintenance_task(&self) {
        let pool = self.clone_for_task();
        let interval = Duration::from_secs(self.config.health_check_interval_seconds);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                ticker.tick().await;

                // Check shutdown flag
                if *pool.shutdown.read().await {
                    break;
                }

                // Run health check
                match pool.health_check().await {
                    Ok(result) => {
                        if result.removed > 0 {
                            tracing::debug!(
                                "Health check: {} healthy, {} unhealthy, {} removed",
                                result.healthy,
                                result.unhealthy,
                                result.removed
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Health check failed: {}", e);
                    }
                }

                // Maintain minimum warm containers
                let state = pool.state().await;
                if state.warm_containers < pool.config.min_warm_containers {
                    let needed = pool.config.min_warm_containers - state.warm_containers;
                    for _ in 0..needed {
                        if let Err(e) = pool
                            .create_warm_container(&pool.config.default_warm_image)
                            .await
                        {
                            tracing::warn!("Failed to create warm container: {}", e);
                            break;
                        }
                    }
                }
            }

            tracing::debug!("Maintenance task stopped");
        });
    }

    /// Clone references needed for background task
    fn clone_for_task(&self) -> ContainerPoolTask {
        ContainerPoolTask {
            manager: self.manager.clone(),
            config: self.config.clone(),
            warm_containers: self.warm_containers.clone(),
            stats: self.stats.clone(),
            container_semaphore: self.container_semaphore.clone(),
            shutdown: self.shutdown.clone(),
        }
    }
}

/// Internal struct for background task (avoids cloning the whole pool)
struct ContainerPoolTask {
    manager: Arc<ContainerManager>,
    config: PoolConfig,
    warm_containers: Arc<Mutex<std::collections::HashMap<String, VecDeque<WarmContainer>>>>,
    stats: Arc<RwLock<PoolStats>>,
    container_semaphore: Arc<Semaphore>,
    shutdown: Arc<RwLock<bool>>,
}

impl ContainerPoolTask {
    async fn health_check(&self) -> Result<HealthCheckResult> {
        let mut healthy = 0;
        let mut unhealthy = 0;
        let mut removed = 0;

        if !self.manager.health_check().await? {
            return Err(Error::Docker("Docker daemon unhealthy".into()));
        }

        let mut warm_containers = self.warm_containers.lock().await;
        let ttl = Duration::from_secs(self.config.warm_ttl_seconds);

        for (_image, queue) in warm_containers.iter_mut() {
            let mut i = 0;
            while i < queue.len() {
                let container = &queue[i];

                if container.is_expired(ttl) {
                    if let Some(removed_container) = queue.remove(i) {
                        let _ = self
                            .manager
                            .stop_and_remove(&removed_container.runtime.config.id, true)
                            .await;
                        self.container_semaphore.add_permits(1);
                        removed += 1;
                    }
                    continue;
                }

                if container.runtime.healthy {
                    healthy += 1;
                } else {
                    unhealthy += 1;
                    if let Some(removed_container) = queue.remove(i) {
                        let _ = self
                            .manager
                            .stop_and_remove(&removed_container.runtime.config.id, true)
                            .await;
                        self.container_semaphore.add_permits(1);
                        removed += 1;
                    }
                    continue;
                }

                i += 1;
            }
        }

        {
            let mut stats = self.stats.write().await;
            stats.health_check_failures += unhealthy as u64;
        }

        Ok(HealthCheckResult {
            healthy,
            unhealthy,
            removed,
        })
    }

    async fn state(&self) -> PoolState {
        let warm_containers = self.warm_containers.lock().await;
        let total_warm: usize = warm_containers.values().map(|q| q.len()).sum();

        let stats = self.stats.read().await;
        let managed = self.manager.count().await;

        PoolState {
            total_containers: managed,
            warm_containers: total_warm,
            running_containers: stats.running_containers,
            max_containers: self.config.max_containers,
            available_permits: self.container_semaphore.available_permits(),
        }
    }

    async fn create_warm_container(&self, image: &str) -> Result<()> {
        let permit = self
            .container_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Internal("Failed to acquire container permit".into()))?;

        let config =
            ContainerConfig::new(image).with_command(vec!["sleep".into(), "infinity".into()]);

        // R5-M15: Only forget permit AFTER successful creation
        let runtime = match self.manager.create_and_start(config).await {
            Ok(rt) => {
                std::mem::forget(permit);
                rt
            }
            Err(e) => {
                drop(permit);
                return Err(e);
            }
        };
        self.manager.mark_warm(&runtime.config.id).await?;

        let updated_runtime = self.manager.get(&runtime.config.id).await?;
        let warm = WarmContainer::new(updated_runtime.clone());

        let mut warm_containers = self.warm_containers.lock().await;
        let queue = warm_containers
            .entry(image.to_string())
            .or_insert_with(VecDeque::new);
        queue.push_back(warm);

        {
            let mut stats = self.stats.write().await;
            stats.total_containers += 1;
            stats.running_containers += 1;
            stats.containers_created += 1;
            stats.warm_containers += 1;
        }

        Ok(())
    }
}

/// Current pool state
#[derive(Debug, Clone)]
pub struct PoolState {
    /// Total containers managed
    pub total_containers: usize,
    /// Warm containers ready for reuse
    pub warm_containers: usize,
    /// Running containers (in use)
    pub running_containers: usize,
    /// Maximum containers allowed
    pub max_containers: usize,
    /// Available permits for new containers
    pub available_permits: usize,
}

/// Health check result
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    /// Number of healthy containers
    pub healthy: usize,
    /// Number of unhealthy containers
    pub unhealthy: usize,
    /// Number of containers removed
    pub removed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_config_defaults() {
        let config = PoolConfig::default();
        assert_eq!(config.min_warm_containers, 2);
        assert_eq!(config.max_containers, 10);
        assert_eq!(config.warm_ttl_seconds, 300);
        assert_eq!(config.max_reuse_count, 10);
    }

    #[test]
    fn test_warm_container_expiry() {
        let config = ContainerConfig::default();
        let runtime = ContainerRuntime::new("test123".into(), "test-container".into(), config);

        let warm = WarmContainer::new(runtime);

        // Should not be expired immediately
        assert!(!warm.is_expired(Duration::from_secs(60)));

        // Would be expired with 0 duration
        assert!(warm.is_expired(Duration::ZERO));
    }

    #[tokio::test]
    async fn test_pool_state() {
        let config = PoolConfig {
            max_containers: 5,
            min_warm_containers: 0, // Don't prewarm for test
            ..Default::default()
        };

        // We can't actually test without Docker, but we can test the config
        assert_eq!(config.max_containers, 5);
        assert_eq!(config.min_warm_containers, 0);
    }

    #[test]
    fn test_pool_stats_cache_hit_rate() {
        let mut stats = PoolStats::default();
        assert_eq!(stats.cache_hit_rate(), 0.0);

        stats.requests_served = 100;
        stats.cache_hits = 80;
        assert!((stats.cache_hit_rate() - 0.8).abs() < 0.001);
    }
}
