//! Docker Container Lifecycle Manager
//!
//! This module provides a Docker container manager using the bollard crate
//! for direct Docker API interaction. It handles container lifecycle operations
//! including creation, starting, stopping, and deletion with proper resource limits.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, LogOutput, LogsOptions,
    RemoveContainerOptions, StartContainerOptions, StopContainerOptions, WaitContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, Mount as BollardMount, MountTypeEnum};
use bollard::Docker;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::types::{ContainerConfig, ContainerRuntime, ContainerState, Mount, MountType};

/// Docker container lifecycle manager
pub struct ContainerManager {
    /// Docker client
    docker: Docker,
    /// Active containers (internal_id -> runtime)
    containers: Arc<Mutex<HashMap<String, ContainerRuntime>>>,
    /// Container name prefix
    prefix: String,
}

impl ContainerManager {
    /// Create a new container manager
    pub async fn new() -> Result<Self> {
        Self::with_prefix("canal").await
    }

    /// Create a new container manager with custom prefix
    pub async fn with_prefix(prefix: impl Into<String>) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| Error::Docker(format!("Failed to connect to Docker: {}", e)))?;

        // Verify Docker connection
        docker
            .ping()
            .await
            .map_err(|e| Error::Docker(format!("Docker ping failed: {}", e)))?;

        Ok(Self {
            docker,
            containers: Arc::new(Mutex::new(HashMap::new())),
            prefix: prefix.into(),
        })
    }

    /// Create a new container manager with existing Docker client (for testing)
    #[cfg(test)]
    pub fn with_docker(docker: Docker, prefix: impl Into<String>) -> Self {
        Self {
            docker,
            containers: Arc::new(Mutex::new(HashMap::new())),
            prefix: prefix.into(),
        }
    }

    /// Check Docker daemon health
    pub async fn health_check(&self) -> Result<bool> {
        self.docker
            .ping()
            .await
            .map_err(|e| Error::Docker(format!("Health check failed: {}", e)))?;
        Ok(true)
    }

    /// Get Docker version info
    pub async fn version(&self) -> Result<String> {
        let version = self
            .docker
            .version()
            .await
            .map_err(|e| Error::Docker(format!("Failed to get version: {}", e)))?;

        Ok(version.version.unwrap_or_else(|| "unknown".to_string()))
    }

    /// Ensure an image is available locally, pulling if necessary
    pub async fn ensure_image(&self, image: &str) -> Result<()> {
        // Check if image exists locally
        match self.docker.inspect_image(image).await {
            Ok(_) => return Ok(()),
            Err(_) => {
                tracing::info!("Pulling Docker image: {}", image);
            }
        }

        // Pull the image
        let options = CreateImageOptions {
            from_image: image,
            ..Default::default()
        };

        let mut stream = self.docker.create_image(Some(options), None, None);

        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(status) = info.status {
                        tracing::debug!("Pull status: {}", status);
                    }
                }
                Err(e) => {
                    return Err(Error::Docker(format!(
                        "Failed to pull image {}: {}",
                        image, e
                    )));
                }
            }
        }

        Ok(())
    }

    /// Create a container from configuration
    pub async fn create(&self, config: ContainerConfig) -> Result<ContainerRuntime> {
        // Ensure image is available
        self.ensure_image(&config.image).await?;

        let container_name = format!("{}-{}", self.prefix, &config.id[..8]);

        // Build mounts
        let mounts = self.build_mounts(&config.mounts);

        // Build host config
        let host_config = HostConfig {
            memory: Some((config.memory_limit_mb * 1024 * 1024) as i64),
            nano_cpus: Some((config.cpu_limit * 1_000_000_000.0) as i64),
            network_mode: Some(config.network_mode.to_string()),
            mounts: if mounts.is_empty() {
                None
            } else {
                Some(mounts)
            },
            readonly_rootfs: Some(config.read_only_rootfs),
            cap_drop: if config.drop_all_caps {
                Some(vec!["ALL".to_string()])
            } else {
                None
            },
            security_opt: if config.no_new_privileges {
                Some(vec!["no-new-privileges".to_string()])
            } else {
                None
            },
            ..Default::default()
        };

        // Build environment variables
        let env: Vec<String> = config
            .env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        // Build labels
        let mut labels = config.labels.clone();
        labels.insert("canal.managed".to_string(), "true".to_string());
        labels.insert("canal.id".to_string(), config.id.clone());

        // Build container config
        let container_config = Config {
            image: Some(config.image.clone()),
            cmd: if config.command.is_empty() {
                None
            } else {
                Some(config.command.clone())
            },
            entrypoint: config.entrypoint.clone(),
            env: if env.is_empty() { None } else { Some(env) },
            working_dir: config.working_dir.clone(),
            user: Some(config.user.clone()),
            labels: Some(labels),
            host_config: Some(host_config),
            tty: Some(false),
            attach_stdin: Some(false),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };

        // Create the container
        let options = CreateContainerOptions {
            name: container_name.clone(),
            platform: None,
        };

        let response = self
            .docker
            .create_container(Some(options), container_config)
            .await
            .map_err(|e| Error::Docker(format!("Failed to create container: {}", e)))?;

        let runtime = ContainerRuntime::new(response.id, container_name, config);

        // Track the container
        {
            let mut containers = self.containers.lock().await;
            containers.insert(runtime.config.id.clone(), runtime.clone());
        }

        Ok(runtime)
    }

    /// Start a container
    pub async fn start(&self, id: &str) -> Result<()> {
        let docker_id = self.get_docker_id(id).await?;

        self.docker
            .start_container(&docker_id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| Error::Docker(format!("Failed to start container: {}", e)))?;

        // Update state
        {
            let mut containers = self.containers.lock().await;
            if let Some(runtime) = containers.get_mut(id) {
                runtime.mark_started();
            }
        }

        Ok(())
    }

    /// Stop a container
    pub async fn stop(&self, id: &str, timeout_seconds: Option<i64>) -> Result<()> {
        let docker_id = self.get_docker_id(id).await?;

        let options = StopContainerOptions {
            t: timeout_seconds.unwrap_or(10),
        };

        self.docker
            .stop_container(&docker_id, Some(options))
            .await
            .map_err(|e| Error::Docker(format!("Failed to stop container: {}", e)))?;

        // Update state
        {
            let mut containers = self.containers.lock().await;
            if let Some(runtime) = containers.get_mut(id) {
                runtime.mark_stopped(0);
            }
        }

        Ok(())
    }

    /// Kill a container immediately
    pub async fn kill(&self, id: &str) -> Result<()> {
        let docker_id = self.get_docker_id(id).await?;

        self.docker
            .kill_container(
                &docker_id,
                None::<bollard::container::KillContainerOptions<String>>,
            )
            .await
            .map_err(|e| Error::Docker(format!("Failed to kill container: {}", e)))?;

        // Update state
        {
            let mut containers = self.containers.lock().await;
            if let Some(runtime) = containers.get_mut(id) {
                runtime.mark_stopped(-9);
            }
        }

        Ok(())
    }

    /// Remove a container
    pub async fn remove(&self, id: &str, force: bool) -> Result<()> {
        let docker_id = self.get_docker_id(id).await?;

        let options = RemoveContainerOptions {
            force,
            v: true, // Remove volumes
            ..Default::default()
        };

        self.docker
            .remove_container(&docker_id, Some(options))
            .await
            .map_err(|e| Error::Docker(format!("Failed to remove container: {}", e)))?;

        // Remove from tracking
        {
            let mut containers = self.containers.lock().await;
            containers.remove(id);
        }

        Ok(())
    }

    /// Execute a command in a running container
    pub async fn exec(&self, id: &str, command: Vec<String>) -> Result<(String, String, i32)> {
        let docker_id = self.get_docker_id(id).await?;

        // Create exec instance
        let exec_options = CreateExecOptions {
            cmd: Some(command),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };

        let exec = self
            .docker
            .create_exec(&docker_id, exec_options)
            .await
            .map_err(|e| Error::Docker(format!("Failed to create exec: {}", e)))?;

        // Start exec and capture output
        let result = self
            .docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|e| Error::Docker(format!("Failed to start exec: {}", e)))?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        if let StartExecResults::Attached { mut output, .. } = result {
            while let Some(msg) = output.next().await {
                match msg {
                    Ok(LogOutput::StdOut { message }) => {
                        stdout.push_str(&String::from_utf8_lossy(&message));
                    }
                    Ok(LogOutput::StdErr { message }) => {
                        stderr.push_str(&String::from_utf8_lossy(&message));
                    }
                    _ => {}
                }
            }
        }

        // Get exit code
        let exec_info = self
            .docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| Error::Docker(format!("Failed to inspect exec: {}", e)))?;

        let exit_code = exec_info.exit_code.unwrap_or(-1) as i32;

        Ok((stdout, stderr, exit_code))
    }

    /// Get container logs
    pub async fn logs(&self, id: &str, tail: Option<usize>) -> Result<(String, String)> {
        let docker_id = self.get_docker_id(id).await?;

        let options = LogsOptions {
            stdout: true,
            stderr: true,
            tail: tail
                .map(|n| n.to_string())
                .unwrap_or_else(|| "all".to_string()),
            ..Default::default()
        };

        let mut stream = self.docker.logs(&docker_id, Some(options));

        let mut stdout = String::new();
        let mut stderr = String::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(LogOutput::StdOut { message }) => {
                    stdout.push_str(&String::from_utf8_lossy(&message));
                }
                Ok(LogOutput::StdErr { message }) => {
                    stderr.push_str(&String::from_utf8_lossy(&message));
                }
                _ => {}
            }
        }

        Ok((stdout, stderr))
    }

    /// Wait for container to finish
    pub async fn wait(&self, id: &str) -> Result<i32> {
        let docker_id = self.get_docker_id(id).await?;

        let options = WaitContainerOptions {
            condition: "not-running",
        };

        let mut stream = self.docker.wait_container(&docker_id, Some(options));

        while let Some(result) = stream.next().await {
            match result {
                Ok(response) => {
                    let exit_code = response.status_code as i32;

                    // Update state
                    {
                        let mut containers = self.containers.lock().await;
                        if let Some(runtime) = containers.get_mut(id) {
                            runtime.mark_stopped(exit_code);
                        }
                    }

                    return Ok(exit_code);
                }
                Err(e) => {
                    return Err(Error::Docker(format!("Wait failed: {}", e)));
                }
            }
        }

        Err(Error::Docker(
            "Container wait stream ended unexpectedly".into(),
        ))
    }

    /// Get container runtime info
    pub async fn get(&self, id: &str) -> Result<ContainerRuntime> {
        let containers = self.containers.lock().await;
        containers
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("Container not found: {}", id)))
    }

    /// List all managed containers
    pub async fn list(&self) -> Vec<ContainerRuntime> {
        let containers = self.containers.lock().await;
        containers.values().cloned().collect()
    }

    /// List containers by state
    pub async fn list_by_state(&self, state: ContainerState) -> Vec<ContainerRuntime> {
        let containers = self.containers.lock().await;
        containers
            .values()
            .filter(|c| c.state == state)
            .cloned()
            .collect()
    }

    /// Get count of containers
    pub async fn count(&self) -> usize {
        self.containers.lock().await.len()
    }

    /// Mark container as warm (ready for reuse)
    pub async fn mark_warm(&self, id: &str) -> Result<()> {
        let mut containers = self.containers.lock().await;
        let runtime = containers
            .get_mut(id)
            .ok_or_else(|| Error::NotFound(format!("Container not found: {}", id)))?;

        runtime.mark_warm();
        Ok(())
    }

    /// Mark container as having an error
    pub async fn mark_error(&self, id: &str, error: impl Into<String>) -> Result<()> {
        let mut containers = self.containers.lock().await;
        let runtime = containers
            .get_mut(id)
            .ok_or_else(|| Error::NotFound(format!("Container not found: {}", id)))?;

        runtime.mark_error(error);
        Ok(())
    }

    /// Cleanup orphaned containers from previous runs
    pub async fn cleanup_orphaned(&self) -> Result<usize> {
        let label_filter = "canal.managed=true".to_string();
        let mut filters = HashMap::new();
        filters.insert("label", vec![label_filter.as_str()]);

        let options = ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        };

        let containers = self
            .docker
            .list_containers(Some(options))
            .await
            .map_err(|e| Error::Docker(format!("Failed to list containers: {}", e)))?;

        let mut removed = 0;
        for container in containers {
            if let Some(id) = container.id {
                tracing::info!("Cleaning up orphaned container: {}", id);

                let options = RemoveContainerOptions {
                    force: true,
                    v: true,
                    ..Default::default()
                };

                if self
                    .docker
                    .remove_container(&id, Some(options))
                    .await
                    .is_ok()
                {
                    removed += 1;
                }
            }
        }

        Ok(removed)
    }

    /// Create and start a container in one operation
    pub async fn create_and_start(&self, config: ContainerConfig) -> Result<ContainerRuntime> {
        let runtime = self.create(config).await?;
        self.start(&runtime.config.id).await?;
        self.get(&runtime.config.id).await
    }

    /// Stop and remove a container in one operation
    pub async fn stop_and_remove(&self, id: &str, force: bool) -> Result<()> {
        // Try to stop first (ignore errors if force)
        if force {
            let _ = self.kill(id).await;
        } else if let Err(e) = self.stop(id, Some(5)).await {
            tracing::warn!("Failed to stop container {}: {}", id, e);
        }

        self.remove(id, force).await
    }

    // === Private helpers ===

    /// Get Docker container ID from internal ID
    async fn get_docker_id(&self, id: &str) -> Result<String> {
        let containers = self.containers.lock().await;
        let runtime = containers
            .get(id)
            .ok_or_else(|| Error::NotFound(format!("Container not found: {}", id)))?;
        Ok(runtime.docker_id.clone())
    }

    /// Build bollard mounts from our mount config
    fn build_mounts(&self, mounts: &[Mount]) -> Vec<BollardMount> {
        mounts
            .iter()
            .map(|m| {
                let mount_type = match m.mount_type {
                    MountType::Bind => Some(MountTypeEnum::BIND),
                    MountType::Volume => Some(MountTypeEnum::VOLUME),
                    MountType::Tmpfs => Some(MountTypeEnum::TMPFS),
                };

                BollardMount {
                    target: Some(m.target.clone()),
                    source: if m.source.is_empty() {
                        None
                    } else {
                        Some(m.source.clone())
                    },
                    typ: mount_type,
                    read_only: Some(m.readonly),
                    tmpfs_options: m.tmpfs_size.as_ref().map(|size| {
                        bollard::models::MountTmpfsOptions {
                            size_bytes: parse_size(size),
                            ..Default::default()
                        }
                    }),
                    ..Default::default()
                }
            })
            .collect()
    }
}

/// Parse size string (e.g., "100m", "1g") to bytes
fn parse_size(s: &str) -> Option<i64> {
    let s = s.trim().to_lowercase();
    let (num, suffix) = if s.ends_with('g') || s.ends_with('m') || s.ends_with('k') {
        let (n, s) = s.split_at(s.len() - 1);
        (n, s)
    } else {
        (s.as_str(), "")
    };

    let base: i64 = num.parse().ok()?;
    let multiplier: i64 = match suffix {
        "k" => 1024,
        "m" => 1024 * 1024,
        "g" => 1024 * 1024 * 1024,
        _ => 1,
    };

    Some(base * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::NetworkMode;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("100"), Some(100));
        assert_eq!(parse_size("100k"), Some(100 * 1024));
        assert_eq!(parse_size("100m"), Some(100 * 1024 * 1024));
        assert_eq!(parse_size("1g"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_size("100M"), Some(100 * 1024 * 1024));
        assert_eq!(parse_size("invalid"), None);
    }

    #[test]
    fn test_network_mode_to_string() {
        assert_eq!(NetworkMode::None.to_string(), "none");
        assert_eq!(NetworkMode::Bridge.to_string(), "bridge");
        assert_eq!(NetworkMode::Host.to_string(), "host");
        assert_eq!(NetworkMode::Custom("mynet".into()).to_string(), "mynet");
    }

    #[tokio::test]
    async fn test_container_config_to_host_config() {
        let config = ContainerConfig::new("alpine:latest")
            .with_cpu_limit(0.5)
            .with_memory_limit(256)
            .with_network_mode(NetworkMode::None);

        assert_eq!(config.cpu_limit, 0.5);
        assert_eq!(config.memory_limit_mb, 256);
        assert_eq!(config.network_mode, NetworkMode::None);
    }
}
