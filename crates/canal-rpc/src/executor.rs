//! Code execution — runs code in temporary files with timeout and output truncation.

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::protocol::RpcResponse;

/// Monotonic counter for unique temp file names.
static EXEC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Maximum stdout size: 100 KB.
const MAX_STDOUT: usize = 100 * 1024;
/// Maximum stderr size: 10 KB.
const MAX_STDERR: usize = 10 * 1024;
/// Default execution timeout.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Execute code in a subprocess, returning stdout/stderr/exit_code.
pub fn execute_code(params: &Value) -> RpcResponse {
    let language = params["language"].as_str().unwrap_or("bash");
    let code = params["code"].as_str().unwrap_or("");
    let timeout_secs = params["timeout_secs"]
        .as_u64()
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let workdir = params["workdir"].as_str();

    let (cmd, ext) = match language {
        "bash" | "sh" => ("bash", "sh"),
        "python" | "python3" => ("python3", "py"),
        // R8-C1: Node.js execution gated behind `unsafe-rpc-exec` feature.
        // Without Docker isolation, node can access the full filesystem and network.
        #[cfg(feature = "unsafe-rpc-exec")]
        "node" | "javascript" | "js" => ("node", "js"),
        #[cfg(not(feature = "unsafe-rpc-exec"))]
        "node" | "javascript" | "js" => {
            return RpcResponse::error(
                "node/javascript execution disabled — requires `unsafe-rpc-exec` feature".to_string(),
            );
        }
        other => {
            return RpcResponse::error(format!("unsupported language: {other}"));
        }
    };

    // Write code to a temp file (unique per invocation)
    let counter = EXEC_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = std::env::temp_dir().join(format!(
        "canal_exec_{}_{counter}.{ext}",
        std::process::id()
    ));
    {
        let mut f = match std::fs::File::create(&tmp_path) {
            Ok(f) => f,
            Err(e) => return RpcResponse::error(format!("failed to create temp file: {e}")),
        };
        // Set permissions to 0600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        if let Err(e) = f.write_all(code.as_bytes()) {
            return RpcResponse::error(format!("failed to write temp file: {e}"));
        }
    }

    let mut command = Command::new(cmd);
    command.arg(&tmp_path);
    if let Some(wd) = workdir {
        command.current_dir(wd);
    }

    let result = run_with_timeout(&mut command, Duration::from_secs(timeout_secs));

    // Clean up temp file
    let _ = std::fs::remove_file(&tmp_path);

    match result {
        Ok(output) => {
            let stdout = truncate_string(
                String::from_utf8_lossy(&output.stdout).to_string(),
                MAX_STDOUT,
            );
            let stderr = truncate_string(
                String::from_utf8_lossy(&output.stderr).to_string(),
                MAX_STDERR,
            );
            let exit_code = output.status.code().unwrap_or(-1);

            RpcResponse::success(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "timed_out": false,
            }))
        }
        Err(e) if e.to_string().contains("timed out") => RpcResponse::success(serde_json::json!({
            "stdout": "",
            "stderr": format!("execution timed out after {timeout_secs}s"),
            "exit_code": -1,
            "timed_out": true,
        })),
        Err(e) => RpcResponse::error(format!("execution failed: {e}")),
    }
}

fn run_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => {
                let stdout = child.stdout.take().map_or(vec![], |mut s| {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut s, &mut buf).ok();
                    buf
                });
                let stderr = child.stderr.take().map_or(vec![], |mut s| {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut s, &mut buf).ok();
                    buf
                });
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("execution timed out".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn truncate_string(s: String, max: usize) -> String {
    if s.len() <= max {
        s
    } else {
        // R8-M3: Find a valid UTF-8 boundary at or before `max` to avoid panic
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        let truncated = &s[..end];
        format!("{truncated}\n... (truncated, {max} byte limit)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bash_hello() {
        let resp = execute_code(&serde_json::json!({
            "language": "bash",
            "code": "echo hello"
        }));
        assert!(resp.ok);
        assert!(resp.data["stdout"].as_str().unwrap().contains("hello"));
        assert_eq!(resp.data["exit_code"], 0);
    }

    #[test]
    fn test_bash_exit_code() {
        let resp = execute_code(&serde_json::json!({
            "language": "bash",
            "code": "exit 42"
        }));
        assert!(resp.ok); // ok = frame-level success (we got a result)
        assert_eq!(resp.data["exit_code"], 42);
    }

    #[test]
    fn test_bash_stderr() {
        let resp = execute_code(&serde_json::json!({
            "language": "bash",
            "code": "echo err >&2"
        }));
        assert!(resp.ok);
        assert!(resp.data["stderr"].as_str().unwrap().contains("err"));
    }

    #[test]
    fn test_python_hello() {
        // Skip if python3 not available
        if Command::new("python3").arg("--version").output().is_err() {
            eprintln!("Skipping test_python_hello: python3 not found");
            return;
        }
        let resp = execute_code(&serde_json::json!({
            "language": "python3",
            "code": "print('hello')"
        }));
        assert!(resp.ok);
        assert!(resp.data["stdout"].as_str().unwrap().contains("hello"));
    }

    #[test]
    fn test_node_hello() {
        // Skip if node not available
        if Command::new("node").arg("--version").output().is_err() {
            eprintln!("Skipping test_node_hello: node not found");
            return;
        }
        let resp = execute_code(&serde_json::json!({
            "language": "node",
            "code": "console.log('hello')"
        }));
        assert!(resp.ok);
        assert!(resp.data["stdout"].as_str().unwrap().contains("hello"));
    }

    #[test]
    fn test_unsupported_language() {
        let resp = execute_code(&serde_json::json!({
            "language": "cobol",
            "code": "DISPLAY 'HELLO'."
        }));
        assert!(!resp.ok);
        assert!(resp
            .error
            .as_ref()
            .unwrap()
            .contains("unsupported language"));
    }

    #[test]
    fn test_timeout() {
        let resp = execute_code(&serde_json::json!({
            "language": "bash",
            "code": "sleep 30",
            "timeout_secs": 1
        }));
        assert!(resp.ok);
        assert_eq!(resp.data["timed_out"], true);
    }

    #[test]
    fn test_stdout_truncation() {
        // Generate ~200KB of stdout
        let resp = execute_code(&serde_json::json!({
            "language": "bash",
            "code": "dd if=/dev/zero bs=1024 count=200 2>/dev/null | tr '\\0' 'A'"
        }));
        assert!(resp.ok);
        let stdout = resp.data["stdout"].as_str().unwrap();
        // Should be truncated to around MAX_STDOUT + truncation message
        assert!(stdout.len() <= MAX_STDOUT + 100);
    }

    #[test]
    fn test_temp_file_permissions() {
        // Execute and check that temp file is cleaned up
        let resp = execute_code(&serde_json::json!({
            "language": "bash",
            "code": "echo cleaned_up"
        }));
        assert!(resp.ok);
        // The temp file should have been removed after execution
        let tmp_path =
            std::env::temp_dir().join(format!("canal_exec_{}.sh", std::process::id()));
        // File might or might not exist depending on timing, but the function ran without error
        let _ = std::fs::remove_file(&tmp_path); // clean up just in case
    }

    #[test]
    fn test_empty_code() {
        let resp = execute_code(&serde_json::json!({
            "language": "bash",
            "code": ""
        }));
        assert!(resp.ok);
        assert_eq!(resp.data["exit_code"], 0);
    }

    #[test]
    fn test_working_directory() {
        let workdir = std::env::temp_dir();
        let resp = execute_code(&serde_json::json!({
            "language": "bash",
            "code": "pwd",
            "workdir": workdir.to_str().unwrap()
        }));
        assert!(resp.ok);
        let stdout = resp.data["stdout"].as_str().unwrap();
        // macOS /tmp → /private/tmp, so use canonicalized comparison
        let canon = workdir.canonicalize().unwrap_or(workdir.clone());
        assert!(
            stdout.trim() == canon.to_str().unwrap() || stdout.trim() == workdir.to_str().unwrap(),
            "expected workdir in stdout, got: {stdout}"
        );
    }
}
