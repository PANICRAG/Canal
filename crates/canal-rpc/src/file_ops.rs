//! File operations — read/write files within a sandboxed workspace directory.
//!
//! All paths are resolved relative to the workspace and checked for traversal attacks.

use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde_json::Value;

use crate::protocol::RpcResponse;

/// Read a file within the workspace.
pub fn file_read(params: &Value, workspace: &Path) -> RpcResponse {
    let rel_path = match params["path"].as_str() {
        Some(p) => p,
        None => return RpcResponse::error("missing 'path' parameter".to_string()),
    };

    let resolved = match resolve_safe_path(workspace, rel_path) {
        Ok(p) => p,
        Err(e) => return RpcResponse::error(e),
    };

    // R8-H11: Use O_NOFOLLOW to prevent TOCTOU symlink swap between
    // resolve_safe_path() validation and actual file read
    match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&resolved)
    {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            if let Err(e) = file.read_to_end(&mut bytes) {
                return RpcResponse::error(format!("read error: {e}"));
            }
            let content = String::from_utf8_lossy(&bytes).to_string();
            RpcResponse::success(serde_json::json!({
                "content": content,
                "size": bytes.len(),
            }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            RpcResponse::error(format!("not found: {rel_path}"))
        }
        Err(e) => RpcResponse::error(format!("read error: {e}")),
    }
}

/// Write a file within the workspace, creating intermediate directories.
pub fn file_write(params: &Value, workspace: &Path) -> RpcResponse {
    let rel_path = match params["path"].as_str() {
        Some(p) => p,
        None => return RpcResponse::error("missing 'path' parameter".to_string()),
    };
    let content = match params["content"].as_str() {
        Some(c) => c,
        None => return RpcResponse::error("missing 'content' parameter".to_string()),
    };

    let resolved = match resolve_safe_path(workspace, rel_path) {
        Ok(p) => p,
        Err(e) => return RpcResponse::error(e),
    };

    // Create parent directories
    if let Some(parent) = resolved.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return RpcResponse::error(format!("failed to create directories: {e}"));
        }
    }

    // R8-H11: Use O_NOFOLLOW to prevent TOCTOU symlink swap between
    // resolve_safe_path() validation and actual file write
    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&resolved)
    {
        Ok(mut file) => {
            use std::io::Write;
            match file.write_all(content.as_bytes()) {
                Ok(()) => RpcResponse::success(serde_json::json!({
                    "path": rel_path,
                    "size": content.len(),
                })),
                Err(e) => RpcResponse::error(format!("write error: {e}")),
            }
        }
        Err(e) => RpcResponse::error(format!("write error: {e}")),
    }
}

/// Resolve a relative path against the workspace, rejecting traversal attacks.
///
/// Rejects:
/// - Absolute paths
/// - Paths containing `..`
/// - Symlinks pointing outside workspace
fn resolve_safe_path(workspace: &Path, rel_path: &str) -> Result<std::path::PathBuf, String> {
    let path = Path::new(rel_path);

    // Reject absolute paths
    if path.is_absolute() {
        return Err("outside workspace: absolute paths not allowed".to_string());
    }

    // Reject .. components
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            return Err("outside workspace: '..' not allowed".to_string());
        }
    }

    let joined = workspace.join(rel_path);

    // If the file exists, canonicalize and verify it's inside workspace
    if joined.exists() {
        let canonical = joined
            .canonicalize()
            .map_err(|e| format!("path resolution failed: {e}"))?;
        let ws_canonical = workspace
            .canonicalize()
            .map_err(|e| format!("workspace resolution failed: {e}"))?;
        if !canonical.starts_with(&ws_canonical) {
            return Err("outside workspace: path resolves outside workspace".to_string());
        }
        Ok(canonical)
    } else {
        // For new files, verify the joined path (before creation)
        // Canonicalize the workspace and check the joined path starts correctly
        let ws_canonical = workspace
            .canonicalize()
            .map_err(|e| format!("workspace resolution failed: {e}"))?;
        // Since file doesn't exist, we rebuild from canonical workspace + rel
        Ok(ws_canonical.join(rel_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn test_workspace() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("canal_rpc_file_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_file_write_and_read() {
        let ws = test_workspace();
        let write_resp = file_write(
            &serde_json::json!({"path": "a.txt", "content": "hello"}),
            &ws,
        );
        assert!(write_resp.ok);

        let read_resp = file_read(&serde_json::json!({"path": "a.txt"}), &ws);
        assert!(read_resp.ok);
        assert_eq!(read_resp.data["content"].as_str().unwrap(), "hello");
    }

    #[test]
    fn test_file_read_nonexistent() {
        let ws = test_workspace();
        let resp = file_read(&serde_json::json!({"path": "nope.txt"}), &ws);
        assert!(!resp.ok);
        assert!(resp.error.as_ref().unwrap().contains("not found"));
    }

    #[test]
    fn test_path_traversal_dotdot() {
        let ws = test_workspace();
        let resp = file_read(&serde_json::json!({"path": "../../etc/passwd"}), &ws);
        assert!(!resp.ok);
        assert!(resp.error.as_ref().unwrap().contains("outside workspace"));
    }

    #[test]
    fn test_path_traversal_absolute() {
        let ws = test_workspace();
        let resp = file_read(&serde_json::json!({"path": "/etc/passwd"}), &ws);
        assert!(!resp.ok);
        assert!(resp.error.as_ref().unwrap().contains("outside workspace"));
    }

    #[test]
    fn test_path_traversal_symlink() {
        let ws = test_workspace();
        let link_path = ws.join("evil_link");
        let _ = std::fs::remove_file(&link_path);
        symlink("/etc/passwd", &link_path).ok();
        let resp = file_read(&serde_json::json!({"path": "evil_link"}), &ws);
        assert!(!resp.ok);
        assert!(resp.error.as_ref().unwrap().contains("outside workspace"));
        let _ = std::fs::remove_file(&link_path);
    }

    #[test]
    fn test_file_write_creates_dirs() {
        let ws = test_workspace();
        let resp = file_write(
            &serde_json::json!({"path": "sub/dir/file.txt", "content": "nested"}),
            &ws,
        );
        assert!(resp.ok);
        let canonical = ws.canonicalize().unwrap();
        let content = std::fs::read_to_string(canonical.join("sub/dir/file.txt")).unwrap();
        assert_eq!(content, "nested");
    }

    #[test]
    fn test_file_write_overwrite() {
        let ws = test_workspace();
        file_write(
            &serde_json::json!({"path": "overwrite.txt", "content": "first"}),
            &ws,
        );
        let resp = file_write(
            &serde_json::json!({"path": "overwrite.txt", "content": "second"}),
            &ws,
        );
        assert!(resp.ok);
        let canonical = ws.canonicalize().unwrap();
        let content = std::fs::read_to_string(canonical.join("overwrite.txt")).unwrap();
        assert_eq!(content, "second");
    }

    #[test]
    fn test_file_read_binary() {
        let ws = test_workspace();
        // Write raw bytes then read back
        let canonical = ws.canonicalize().unwrap();
        let bin_path = canonical.join("binary.bin");
        std::fs::write(&bin_path, &[0u8, 1, 2, 255, 128]).unwrap();

        let resp = file_read(&serde_json::json!({"path": "binary.bin"}), &ws);
        assert!(resp.ok);
        assert_eq!(resp.data["size"], 5);
    }

    #[test]
    fn test_file_write_empty() {
        let ws = test_workspace();
        let resp = file_write(
            &serde_json::json!({"path": "empty.txt", "content": ""}),
            &ws,
        );
        assert!(resp.ok);
        let canonical = ws.canonicalize().unwrap();
        let meta = std::fs::metadata(canonical.join("empty.txt")).unwrap();
        assert_eq!(meta.len(), 0);
    }
}
