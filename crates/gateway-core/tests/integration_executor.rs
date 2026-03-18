//! Integration tests for the Code Executor module
//!
//! These tests require Docker to be running and available.
//! To skip Docker tests when Docker is not available, use:
//! `cargo test -- --skip docker`

use gateway_core::executor::{
    CodeExecutor, ExecutionRequest, ExecutionStatus, ExecutorConfig, Language,
};

/// Check if Docker is available
async fn docker_available() -> bool {
    let output = tokio::process::Command::new("docker")
        .args(["info"])
        .output()
        .await;
    output.map(|o| o.status.success()).unwrap_or(false)
}

// ============================================================================
// Executor Config Tests
// ============================================================================

#[test]
fn test_executor_config_default() {
    let config = ExecutorConfig::default();
    assert!(config.enabled);
    assert!(config.python.enabled);
    assert!(config.bash.enabled);
    assert_eq!(config.docker.network_mode, "none");
}

#[test]
fn test_executor_config_docker_security() {
    let config = ExecutorConfig::default();
    assert!(config.docker.default_limits.read_only_rootfs);
    assert_eq!(config.docker.network_mode, "none");
    assert_eq!(config.docker.default_limits.user_id, 1000);
}

#[test]
fn test_executor_config_resource_limits() {
    let config = ExecutorConfig::default();
    assert_eq!(config.docker.default_limits.memory, "512m");
    assert_eq!(config.docker.default_limits.cpu, "1.0");
    assert!(!config.docker.default_limits.tmpfs_mounts.is_empty());
}

// ============================================================================
// Language Parsing Tests
// ============================================================================

#[test]
fn test_language_from_str() {
    assert_eq!("python".parse::<Language>().unwrap(), Language::Python);
    assert_eq!("Python".parse::<Language>().unwrap(), Language::Python);
    assert_eq!("py".parse::<Language>().unwrap(), Language::Python);
    assert_eq!("python3".parse::<Language>().unwrap(), Language::Python);

    assert_eq!("bash".parse::<Language>().unwrap(), Language::Bash);
    assert_eq!("sh".parse::<Language>().unwrap(), Language::Bash);
    assert_eq!("shell".parse::<Language>().unwrap(), Language::Bash);

    assert_eq!(
        "javascript".parse::<Language>().unwrap(),
        Language::JavaScript
    );
    assert_eq!("js".parse::<Language>().unwrap(), Language::JavaScript);
    assert_eq!(
        "typescript".parse::<Language>().unwrap(),
        Language::TypeScript
    );
    assert_eq!("go".parse::<Language>().unwrap(), Language::Go);
    assert_eq!("rust".parse::<Language>().unwrap(), Language::Rust);

    assert!("ruby".parse::<Language>().is_err());
    assert!("java".parse::<Language>().is_err());
}

#[test]
fn test_language_display() {
    assert_eq!(Language::Python.to_string(), "python");
    assert_eq!(Language::Bash.to_string(), "bash");
}

// ============================================================================
// Bash Config Tests
// ============================================================================

#[test]
fn test_bash_config_allowed_commands() {
    let config = ExecutorConfig::default();

    // Essential commands should be allowed
    assert!(config.bash.allowed_commands.contains(&"echo".to_string()));
    assert!(config.bash.allowed_commands.contains(&"ls".to_string()));
    assert!(config.bash.allowed_commands.contains(&"pwd".to_string()));
    assert!(config.bash.allowed_commands.contains(&"cat".to_string()));
    assert!(config.bash.allowed_commands.contains(&"grep".to_string()));
}

#[test]
fn test_bash_config_blocked_patterns() {
    let config = ExecutorConfig::default();

    // Dangerous patterns should be blocked
    assert!(config
        .bash
        .blocked_patterns
        .contains(&"rm -rf /".to_string()));
    assert!(config.bash.blocked_patterns.contains(&"sudo".to_string()));
    assert!(config.bash.blocked_patterns.contains(&"wget".to_string()));
    assert!(config.bash.blocked_patterns.contains(&"curl".to_string()));
}

// ============================================================================
// Python Config Tests
// ============================================================================

#[test]
fn test_python_config_defaults() {
    let config = ExecutorConfig::default();

    assert!(config.python.enabled);
    assert_eq!(config.python.docker_image, "python:3.11-slim");
    assert!(config.python.timeout_ms > 0);
}

#[test]
fn test_python_preinstalled_packages() {
    let config = ExecutorConfig::default();

    // Common data science packages should be pre-installed
    assert!(config
        .python
        .preinstalled_packages
        .contains(&"numpy".to_string()));
    assert!(config
        .python
        .preinstalled_packages
        .contains(&"pandas".to_string()));
}

// ============================================================================
// Execution Request Tests
// ============================================================================

#[test]
fn test_execution_request_serialization() {
    let request = ExecutionRequest {
        code: "print('hello')".to_string(),
        language: Language::Python,
        timeout_ms: Some(5000),
        stream: false,
        working_dir: None,
    };

    let json = serde_json::to_string(&request).unwrap();
    let parsed: ExecutionRequest = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.code, request.code);
    assert_eq!(parsed.language, request.language);
    assert_eq!(parsed.timeout_ms, request.timeout_ms);
}

#[test]
fn test_execution_status_serialization() {
    assert_eq!(
        serde_json::to_string(&ExecutionStatus::Success).unwrap(),
        "\"success\""
    );
    assert_eq!(
        serde_json::to_string(&ExecutionStatus::Error).unwrap(),
        "\"error\""
    );
    assert_eq!(
        serde_json::to_string(&ExecutionStatus::Timeout).unwrap(),
        "\"timeout\""
    );
}

// ============================================================================
// Code Executor Integration Tests (Require Docker)
// ============================================================================

#[tokio::test]
async fn test_code_executor_creation() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let result = CodeExecutor::new(config).await;

    // Should successfully create executor if Docker is available
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_code_executor_health_check() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    let health = executor.health_check().await;
    assert!(health.is_ok());
    assert!(health.unwrap());
}

#[tokio::test]
async fn test_code_executor_python_hello_world() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    let request = ExecutionRequest {
        code: r#"print("Hello, World!")"#.to_string(),
        language: Language::Python,
        timeout_ms: Some(15000),
        stream: false,
        working_dir: None,
    };

    let result = executor.execute(request).await;
    assert!(result.is_ok(), "Execution failed: {:?}", result.err());

    let result = result.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
    assert!(result.stdout.contains("Hello, World!"));
    assert!(result.stderr.is_empty() || result.stderr.trim().is_empty());
}

#[tokio::test]
async fn test_code_executor_python_calculation() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    let request = ExecutionRequest {
        code: r#"
result = sum(range(1, 101))
print(f"Sum 1-100: {result}")
"#
        .to_string(),
        language: Language::Python,
        timeout_ms: Some(15000),
        stream: false,
        working_dir: None,
    };

    let result = executor.execute(request).await;
    assert!(result.is_ok());

    let result = result.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
    assert!(result.stdout.contains("Sum 1-100: 5050"));
}

#[tokio::test]
async fn test_code_executor_python_error() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    let request = ExecutionRequest {
        code: r#"print(undefined_variable)"#.to_string(),
        language: Language::Python,
        timeout_ms: Some(15000),
        stream: false,
        working_dir: None,
    };

    let result = executor.execute(request).await;
    assert!(result.is_ok());

    let result = result.unwrap();
    assert_eq!(result.status, ExecutionStatus::Error);
    assert!(result.stderr.contains("NameError"));
}

#[tokio::test]
async fn test_code_executor_bash_echo() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    let request = ExecutionRequest {
        code: "echo 'Hello from Bash!'".to_string(),
        language: Language::Bash,
        timeout_ms: Some(10000),
        stream: false,
        working_dir: None,
    };

    let result = executor.execute(request).await;
    assert!(result.is_ok());

    let result = result.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
    assert!(result.stdout.contains("Hello from Bash!"));
}

#[tokio::test]
async fn test_code_executor_bash_piped_command() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    let request = ExecutionRequest {
        code: "echo -e 'a\\nb\\nc' | wc -l".to_string(),
        language: Language::Bash,
        timeout_ms: Some(10000),
        stream: false,
        working_dir: None,
    };

    let result = executor.execute(request).await;
    assert!(result.is_ok());

    let result = result.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
    assert!(result.stdout.contains("3"));
}

#[tokio::test]
async fn test_code_executor_language_enabled() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    assert!(executor.is_language_enabled(Language::Python));
    assert!(executor.is_language_enabled(Language::Bash));
}

// ============================================================================
// Security Tests
// ============================================================================

#[tokio::test]
async fn test_network_isolation() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    // Try to access network - should fail due to --network=none
    let request = ExecutionRequest {
        code: r#"
import socket
try:
    socket.create_connection(("8.8.8.8", 53), timeout=2)
    print("NETWORK_AVAILABLE")
except Exception as e:
    print(f"NETWORK_ISOLATED: {e}")
"#
        .to_string(),
        language: Language::Python,
        timeout_ms: Some(10000),
        stream: false,
        working_dir: None,
    };

    let result = executor.execute(request).await;
    assert!(result.is_ok());

    let result = result.unwrap();
    // Should not be able to connect
    assert!(
        result.stdout.contains("NETWORK_ISOLATED") || !result.stdout.contains("NETWORK_AVAILABLE"),
        "Network should be isolated but got: {}",
        result.stdout
    );
}

#[tokio::test]
async fn test_filesystem_readonly() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    // Try to write to filesystem outside /tmp - should fail due to --read-only
    let request = ExecutionRequest {
        code: r#"
import os
try:
    with open('/test_file.txt', 'w') as f:
        f.write('test')
    print("WRITE_SUCCESS")
except Exception as e:
    print(f"WRITE_BLOCKED: {e}")
"#
        .to_string(),
        language: Language::Python,
        timeout_ms: Some(10000),
        stream: false,
        working_dir: None,
    };

    let result = executor.execute(request).await;
    assert!(result.is_ok());

    let result = result.unwrap();
    // Should not be able to write
    assert!(
        result.stdout.contains("WRITE_BLOCKED") || !result.stdout.contains("WRITE_SUCCESS"),
        "Filesystem should be read-only but got: {}",
        result.stdout
    );
}

#[tokio::test]
async fn test_tmpfs_writable() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    // /tmp should be writable (tmpfs mount)
    let request = ExecutionRequest {
        code: r#"
import os
try:
    with open('/tmp/test_file.txt', 'w') as f:
        f.write('test content')
    with open('/tmp/test_file.txt', 'r') as f:
        content = f.read()
    print(f"WRITE_SUCCESS: {content}")
except Exception as e:
    print(f"WRITE_FAILED: {e}")
"#
        .to_string(),
        language: Language::Python,
        timeout_ms: Some(10000),
        stream: false,
        working_dir: None,
    };

    let result = executor.execute(request).await;
    assert!(result.is_ok());

    let result = result.unwrap();
    assert!(
        result.stdout.contains("WRITE_SUCCESS"),
        "/tmp should be writable but got: {}",
        result.stdout
    );
}

// ============================================================================
// Timeout Tests
// ============================================================================

#[tokio::test]
async fn test_execution_timeout() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    // Infinite loop with short timeout
    let request = ExecutionRequest {
        code: r#"
import time
while True:
    time.sleep(0.1)
"#
        .to_string(),
        language: Language::Python,
        timeout_ms: Some(2000), // 2 second timeout
        stream: false,
        working_dir: None,
    };

    let start = std::time::Instant::now();
    let result = executor.execute(request).await;
    let elapsed = start.elapsed();

    // Should complete within reasonable time (timeout + overhead)
    assert!(
        elapsed.as_secs() < 15,
        "Execution took too long: {:?}",
        elapsed
    );

    // Result should indicate timeout or failure
    if let Ok(result) = result {
        assert!(
            result.status == ExecutionStatus::Timeout || result.status == ExecutionStatus::Killed,
            "Expected timeout/killed status but got: {:?}",
            result.status
        );
    }
}

// ============================================================================
// Cleanup Tests
// ============================================================================

#[tokio::test]
async fn test_executor_cleanup() {
    if !docker_available().await {
        println!("Skipping Docker test - Docker not available");
        return;
    }

    let config = ExecutorConfig::default();
    let executor = match CodeExecutor::new(config).await {
        Ok(e) => e,
        Err(e) => {
            println!("Skipping test - executor creation failed: {}", e);
            return;
        }
    };

    // Cleanup should not fail even if no containers exist
    let result = executor.cleanup().await;
    assert!(result.is_ok());
}
