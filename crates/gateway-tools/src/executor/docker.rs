//! Docker Container Manager
//!
//! Manages Docker containers for secure code execution.
//! Handles container lifecycle, resource limits, and cleanup.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use uuid::Uuid;

use super::config::{DockerConfig, ResourceLimits};

/// Container status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerStatus {
    Created,
    Running,
    Exited,
    Dead,
    Unknown,
}

/// Container information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: ContainerStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub exit_code: Option<i32>,
}

/// Docker container manager
pub struct DockerManager {
    config: DockerConfig,
    /// Active containers (container_id -> info)
    active_containers: Arc<Mutex<HashMap<String, ContainerInfo>>>,
}

#[cfg(test)]
impl DockerManager {
    /// Create a mock DockerManager for testing (skips health check)
    pub fn new_mock(config: DockerConfig) -> Self {
        Self {
            config,
            active_containers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DockerManager {
    /// Create a new Docker manager
    pub async fn new(config: DockerConfig) -> Result<Self> {
        let manager = Self {
            config,
            active_containers: Arc::new(Mutex::new(HashMap::new())),
        };

        // Verify Docker is available
        if manager.config.enabled {
            manager.health_check().await?;
        }

        Ok(manager)
    }

    /// Check if Docker daemon is available
    pub async fn health_check(&self) -> Result<bool> {
        let output = Command::new("docker")
            .args(["version", "--format", "{{.Server.Version}}"])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to check Docker: {}", e)))?;

        if !output.status.success() {
            return Err(Error::Internal(
                "Docker daemon is not running or accessible".into(),
            ));
        }

        Ok(true)
    }

    /// Create and start a container for code execution
    pub async fn create_container(
        &self,
        image: &str,
        command: Vec<String>,
        limits: Option<&ResourceLimits>,
        working_dir: Option<&str>,
        env_vars: Option<HashMap<String, String>>,
    ) -> Result<String> {
        // Check concurrent container limit
        {
            let containers = self.active_containers.lock().await;
            if containers.len() >= self.config.max_concurrent_containers {
                return Err(Error::Internal(
                    "Maximum concurrent container limit reached".into(),
                ));
            }
        }

        let container_id = Uuid::new_v4().to_string();
        let container_name = format!("{}-{}", self.config.container_prefix, &container_id[..8]);

        let limits = limits.unwrap_or(&self.config.default_limits);

        // Build docker run command
        let mut docker_args = vec![
            "run".to_string(),
            "--name".to_string(),
            container_name.clone(),
            "--rm".to_string(),
            "-d".to_string(), // Detached mode
        ];

        // Network isolation
        docker_args.push(format!("--network={}", self.config.network_mode));

        // Resource limits
        docker_args.push(format!("--memory={}", limits.memory));
        docker_args.push(format!("--cpus={}", limits.cpu));

        // Security options
        if limits.read_only_rootfs {
            docker_args.push("--read-only".to_string());
        }

        // User/group (non-root)
        docker_args.push(format!("--user={}:{}", limits.user_id, limits.group_id));

        // Tmpfs mounts for writable temporary directories
        for tmpfs in &limits.tmpfs_mounts {
            docker_args.push(format!("--tmpfs={}:size={}", tmpfs.path, tmpfs.size));
        }

        // Working directory
        if let Some(dir) = working_dir {
            docker_args.push(format!("--workdir={}", dir));
        }

        // Environment variables — R5-H15: block security-critical env var names
        if let Some(vars) = env_vars {
            let blocked_keys = [
                "LD_PRELOAD",
                "LD_LIBRARY_PATH",
                "PATH",
                "HOME",
                "SHELL",
                "USER",
                "LOGNAME",
                "TERM",
            ];
            for (key, value) in vars {
                let key_upper = key.to_uppercase();
                if blocked_keys.iter().any(|b| *b == key_upper) {
                    tracing::warn!(key = %key, "Blocked security-critical env var injection");
                    continue;
                }
                // Reject keys/values containing newlines or null bytes
                if key.contains('\n')
                    || key.contains('\0')
                    || value.contains('\n')
                    || value.contains('\0')
                {
                    tracing::warn!(key = %key, "Blocked env var with control characters");
                    continue;
                }
                docker_args.push("-e".to_string());
                docker_args.push(format!("{}={}", key, value));
            }
        }

        // Security: drop all capabilities
        docker_args.push("--cap-drop=ALL".to_string());

        // Security: no new privileges
        docker_args.push("--security-opt=no-new-privileges".to_string());

        // Image
        docker_args.push(image.to_string());

        // Command
        docker_args.extend(command);

        tracing::debug!("Creating container with args: {:?}", docker_args);

        let output = Command::new("docker")
            .args(&docker_args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create container: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!(
                "Failed to create container: {}",
                stderr
            )));
        }

        let docker_container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Track the container
        {
            let mut containers = self.active_containers.lock().await;
            containers.insert(
                container_id.clone(),
                ContainerInfo {
                    id: docker_container_id.clone(),
                    name: container_name,
                    image: image.to_string(),
                    status: ContainerStatus::Running,
                    created_at: chrono::Utc::now(),
                    exit_code: None,
                },
            );
        }

        Ok(docker_container_id)
    }

    /// Execute a command in an existing container
    pub async fn exec_in_container(
        &self,
        container_id: &str,
        command: Vec<String>,
    ) -> Result<(String, String, i32)> {
        let mut exec_args = vec!["exec".to_string(), container_id.to_string()];
        exec_args.extend(command);

        let output = Command::new("docker")
            .args(&exec_args)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to exec in container: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        Ok((stdout, stderr, exit_code))
    }

    /// Run a container and stream output
    pub async fn run_with_streaming<F>(
        &self,
        image: &str,
        command: Vec<String>,
        limits: Option<&ResourceLimits>,
        timeout_ms: u64,
        on_output: F,
    ) -> Result<(i32, String, String)>
    where
        F: Fn(bool, &str) + Send + 'static,
    {
        let limits = limits.unwrap_or(&self.config.default_limits);
        let container_name = format!(
            "{}-{}",
            self.config.container_prefix,
            &Uuid::new_v4().to_string()[..8]
        );

        // Build docker run command (non-detached for streaming)
        let mut docker_args = vec![
            "run".to_string(),
            "--name".to_string(),
            container_name.clone(),
            "--rm".to_string(),
        ];

        // Network isolation
        docker_args.push(format!("--network={}", self.config.network_mode));

        // Resource limits
        docker_args.push(format!("--memory={}", limits.memory));
        docker_args.push(format!("--cpus={}", limits.cpu));

        // Security options
        if limits.read_only_rootfs {
            docker_args.push("--read-only".to_string());
        }

        docker_args.push(format!("--user={}:{}", limits.user_id, limits.group_id));

        for tmpfs in &limits.tmpfs_mounts {
            docker_args.push(format!("--tmpfs={}:size={}", tmpfs.path, tmpfs.size));
        }

        docker_args.push("--cap-drop=ALL".to_string());
        docker_args.push("--security-opt=no-new-privileges".to_string());

        // Image and command
        docker_args.push(image.to_string());
        docker_args.extend(command);

        let mut child = Command::new("docker")
            .args(&docker_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to start container: {}", e)))?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut stdout_output = String::new();
        let mut stderr_output = String::new();

        let timeout = tokio::time::Duration::from_millis(timeout_ms);

        // Stream output with timeout
        let result = tokio::time::timeout(timeout, async {
            loop {
                tokio::select! {
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(text)) => {
                                stdout_output.push_str(&text);
                                stdout_output.push('\n');
                                on_output(false, &text);
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::error!("Error reading stdout: {}", e);
                                break;
                            }
                        }
                    }
                    line = stderr_reader.next_line() => {
                        match line {
                            Ok(Some(text)) => {
                                stderr_output.push_str(&text);
                                stderr_output.push('\n');
                                on_output(true, &text);
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tracing::error!("Error reading stderr: {}", e);
                            }
                        }
                    }
                }
            }

            child.wait().await
        })
        .await;

        match result {
            Ok(Ok(status)) => {
                let exit_code = status.code().unwrap_or(-1);
                Ok((exit_code, stdout_output, stderr_output))
            }
            Ok(Err(e)) => Err(Error::Internal(format!(
                "Container execution failed: {}",
                e
            ))),
            Err(_) => {
                // Timeout - kill the container
                let _ = self.kill_container_by_name(&container_name).await;
                Err(Error::Internal("Execution timed out".into()))
            }
        }
    }

    /// Stop a running container
    pub async fn stop_container(&self, container_id: &str, timeout_seconds: u64) -> Result<()> {
        let output = Command::new("docker")
            .args(["stop", "-t", &timeout_seconds.to_string(), container_id])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to stop container: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("Failed to stop container {}: {}", container_id, stderr);
        }

        // Remove from tracking
        {
            let mut containers = self.active_containers.lock().await;
            containers.remove(container_id);
        }

        Ok(())
    }

    /// Kill a container by name
    async fn kill_container_by_name(&self, name: &str) -> Result<()> {
        let _ = Command::new("docker").args(["kill", name]).output().await;

        // Also try to remove it
        let _ = Command::new("docker")
            .args(["rm", "-f", name])
            .output()
            .await;

        Ok(())
    }

    /// Kill a container immediately
    pub async fn kill_container(&self, container_id: &str) -> Result<()> {
        let _ = Command::new("docker")
            .args(["kill", container_id])
            .output()
            .await;

        {
            let mut containers = self.active_containers.lock().await;
            containers.remove(container_id);
        }

        Ok(())
    }

    /// Get container logs
    pub async fn get_logs(&self, container_id: &str) -> Result<(String, String)> {
        let output = Command::new("docker")
            .args(["logs", container_id])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get logs: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok((stdout, stderr))
    }

    /// Clean up orphaned containers from previous sessions
    pub async fn cleanup_orphaned_containers(&self) -> Result<()> {
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                &format!("name={}", self.config.container_prefix),
                "--format",
                "{{.ID}}",
            ])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to list containers: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let container_ids: Vec<&str> = stdout.lines().filter(|s| !s.is_empty()).collect();

        for id in container_ids {
            tracing::info!("Cleaning up orphaned container: {}", id);
            let _ = Command::new("docker").args(["rm", "-f", id]).output().await;
        }

        Ok(())
    }

    /// Pull an image if not present
    pub async fn ensure_image(&self, image: &str) -> Result<()> {
        // Check if image exists
        let check = Command::new("docker")
            .args(["image", "inspect", image])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to check image: {}", e)))?;

        if check.status.success() {
            return Ok(());
        }

        tracing::info!("Pulling Docker image: {}", image);

        let output = Command::new("docker")
            .args(["pull", image])
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to pull image: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Failed to pull image: {}", stderr)));
        }

        Ok(())
    }

    /// Get count of active containers
    pub async fn active_container_count(&self) -> usize {
        self.active_containers.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_status_serialize() {
        let status = ContainerStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");
    }
}
