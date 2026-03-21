//! Bash Command Executor
//!
//! Executes Bash commands in Docker containers with security restrictions.
//! Only allows whitelisted commands and blocks dangerous patterns.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::config::BashConfig;
use super::docker::DockerManager;
use super::{
    ExecutionEvent, ExecutionRequest, ExecutionResult, ExecutionStatus, Language, LanguageExecutor,
};

/// Bash command executor with security restrictions
pub struct BashExecutor {
    docker_manager: Arc<DockerManager>,
    config: BashConfig,
}

impl BashExecutor {
    /// Create a new Bash executor
    pub fn new(docker_manager: Arc<DockerManager>, config: BashConfig) -> Self {
        Self {
            docker_manager,
            config,
        }
    }

    /// Validate command against security rules
    fn validate_command(&self, command: &str) -> Result<()> {
        // Check for blocked patterns
        for pattern in &self.config.blocked_patterns {
            if command.contains(pattern) {
                return Err(Error::Internal(format!(
                    "Command blocked: contains forbidden pattern '{}'",
                    pattern
                )));
            }
        }

        // For complex commands with pipes/chains, split on multi-char delimiters first
        // then single-char ones, to correctly handle && and ||
        let command_str = command.replace("&&", "\x1F").replace("||", "\x1F");
        let commands: Vec<&str> = command_str
            .split(|c| c == '|' || c == ';' || c == '\x1F')
            .collect();

        for cmd in commands {
            let cmd = cmd.trim();
            if cmd.is_empty() {
                continue;
            }

            let base = cmd
                .split_whitespace()
                .next()
                .unwrap_or("")
                .split('/')
                .last()
                .unwrap_or("");

            // Skip empty or environment variable assignments
            if base.is_empty() || base.contains('=') {
                continue;
            }

            // Check if the base command is in the allowed list
            if !self.config.allowed_commands.iter().any(|allowed| {
                allowed == base
                    || allowed == &format!("/bin/{}", base)
                    || allowed == &format!("/usr/bin/{}", base)
            }) {
                return Err(Error::Internal(format!(
                    "Command '{}' is not in the allowed list. Allowed commands: {:?}",
                    base, self.config.allowed_commands
                )));
            }
        }

        // Additional security checks
        self.check_path_traversal(command)?;
        self.check_command_injection(command)?;

        Ok(())
    }

    /// Check for path traversal attempts
    fn check_path_traversal(&self, command: &str) -> Result<()> {
        // Block attempts to access sensitive directories
        let sensitive_dirs = [
            "/etc",
            "/var",
            "/root",
            "/home",
            "/proc",
            "/sys",
            "/dev",
            "/tmp",
            "/usr/sbin",
            "/sbin",
            "/boot",
            "/mnt",
        ];
        if command.contains("../") {
            for dir in &sensitive_dirs {
                if command.contains(dir) {
                    return Err(Error::Internal(format!(
                        "Path traversal to sensitive directory '{}' detected",
                        dir
                    )));
                }
            }
        }
        Ok(())
    }

    /// Check for command injection attempts
    fn check_command_injection(&self, command: &str) -> Result<()> {
        // Block common injection patterns
        let injection_patterns = [
            "$(", // Command substitution
            "`",  // Backtick substitution
            "\n", // Newline injection
            "\r", // Carriage return injection
        ];

        for pattern in &injection_patterns {
            if command.contains(pattern) {
                return Err(Error::Internal(format!(
                    "Potentially dangerous pattern detected: '{}'",
                    pattern
                )));
            }
        }

        Ok(())
    }

    /// Escape command for shell execution
    fn escape_for_shell(command: &str) -> String {
        // Use base64 encoding to safely pass command to the container
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(command)
    }
}

#[async_trait::async_trait]
impl LanguageExecutor for BashExecutor {
    fn language(&self) -> Language {
        Language::Bash
    }

    async fn execute(&self, request: &ExecutionRequest) -> Result<ExecutionResult> {
        let execution_id = Uuid::new_v4().to_string();
        let start_time = std::time::Instant::now();

        // Validate command before execution
        self.validate_command(&request.code)?;

        // Ensure image is available
        self.docker_manager
            .ensure_image(&self.config.docker_image)
            .await?;

        let timeout_ms = request.timeout_ms.unwrap_or(self.config.timeout_ms);
        let encoded_command = Self::escape_for_shell(&request.code);

        // Build command to decode and execute
        let command = vec![
            "bash".to_string(),
            "-c".to_string(),
            format!("echo '{}' | base64 -d | bash", encoded_command),
        ];

        let (exit_code, stdout, stderr) = self
            .docker_manager
            .run_with_streaming(
                &self.config.docker_image,
                command,
                self.config.limits.as_ref(),
                timeout_ms,
                |_is_stderr, _text| {},
            )
            .await
            .map_err(|e| {
                if e.to_string().contains("timed out") {
                    Error::Internal("Execution timed out".into())
                } else {
                    e
                }
            })?;

        let duration_ms = start_time.elapsed().as_millis() as u64;

        let status = if exit_code == 0 {
            ExecutionStatus::Success
        } else {
            ExecutionStatus::Error
        };

        Ok(ExecutionResult {
            execution_id,
            language: Language::Bash,
            stdout,
            stderr,
            exit_code,
            status,
            duration_ms,
        })
    }

    async fn execute_streaming(
        &self,
        request: &ExecutionRequest,
        output_tx: mpsc::Sender<ExecutionEvent>,
    ) -> Result<ExecutionResult> {
        let execution_id = Uuid::new_v4().to_string();
        let start_time = std::time::Instant::now();

        // Validate command before execution
        if let Err(e) = self.validate_command(&request.code) {
            let _ = output_tx
                .send(ExecutionEvent::Error {
                    message: e.to_string(),
                })
                .await;
            return Err(e);
        }

        // Send started event
        let _ = output_tx
            .send(ExecutionEvent::Started {
                execution_id: execution_id.clone(),
            })
            .await;

        // Ensure image is available
        self.docker_manager
            .ensure_image(&self.config.docker_image)
            .await?;

        let timeout_ms = request.timeout_ms.unwrap_or(self.config.timeout_ms);
        let encoded_command = Self::escape_for_shell(&request.code);

        let command = vec![
            "bash".to_string(),
            "-c".to_string(),
            format!("echo '{}' | base64 -d | bash", encoded_command),
        ];

        let output_tx_clone = output_tx.clone();
        let (exit_code, stdout, stderr) = self
            .docker_manager
            .run_with_streaming(
                &self.config.docker_image,
                command,
                self.config.limits.as_ref(),
                timeout_ms,
                move |is_stderr, text| {
                    let event = if is_stderr {
                        ExecutionEvent::Stderr {
                            text: text.to_string(),
                        }
                    } else {
                        ExecutionEvent::Stdout {
                            text: text.to_string(),
                        }
                    };
                    let tx = output_tx_clone.clone();
                    tokio::spawn(async move {
                        let _ = tx.send(event).await;
                    });
                },
            )
            .await
            .map_err(|e| {
                let error_msg = e.to_string();
                let is_timeout = error_msg.contains("timed out");
                let tx = output_tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(ExecutionEvent::Error { message: error_msg }).await;
                });
                if is_timeout {
                    Error::Internal("Execution timed out".into())
                } else {
                    Error::Internal(e.to_string())
                }
            })?;

        let duration_ms = start_time.elapsed().as_millis() as u64;

        let status = if exit_code == 0 {
            ExecutionStatus::Success
        } else {
            ExecutionStatus::Error
        };

        // Send completed event
        let _ = output_tx
            .send(ExecutionEvent::Completed {
                exit_code,
                duration_ms,
            })
            .await;

        Ok(ExecutionResult {
            execution_id,
            language: Language::Bash,
            stdout,
            stderr,
            exit_code,
            status,
            duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_executor() -> BashExecutor {
        let docker_manager = Arc::new(DockerManager::new_mock(
            super::super::config::DockerConfig::default(),
        ));
        BashExecutor::new(docker_manager, BashConfig::default())
    }

    #[test]
    fn test_validate_allowed_command() {
        let executor = create_test_executor();
        assert!(executor.validate_command("ls -la").is_ok());
        assert!(executor.validate_command("cat file.txt").is_ok());
        assert!(executor.validate_command("grep pattern file").is_ok());
    }

    #[test]
    fn test_validate_blocked_command() {
        let executor = create_test_executor();
        assert!(executor.validate_command("rm -rf /").is_err());
        assert!(executor.validate_command("sudo apt-get install").is_err());
    }

    #[test]
    fn test_validate_unknown_command() {
        let executor = create_test_executor();
        assert!(executor.validate_command("unknown_command").is_err());
    }

    #[test]
    fn test_command_injection_blocked() {
        let executor = create_test_executor();
        assert!(executor.validate_command("ls $(rm -rf /)").is_err());
        assert!(executor.validate_command("ls `rm -rf /`").is_err());
    }

    #[test]
    fn test_piped_commands() {
        let executor = create_test_executor();
        assert!(executor.validate_command("ls -la | grep txt").is_ok());
        assert!(executor.validate_command("cat file | wc -l").is_ok());
    }
}
