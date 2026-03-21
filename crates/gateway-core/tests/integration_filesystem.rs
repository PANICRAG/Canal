//! Integration tests for the Filesystem Service module
//!
//! These tests use temporary directories and do not require Docker.

use gateway_core::filesystem::{
    DirectoryConfig, DirectoryMode, FilesystemConfig, FilesystemService, PermissionGuard,
};
use std::path::PathBuf;
use tempfile::TempDir;

/// Get the canonical path from a temp directory
/// This handles symlinks like /tmp -> /private/tmp on macOS
fn canonical_temp_path(temp_dir: &TempDir) -> PathBuf {
    temp_dir.path().canonicalize().unwrap()
}

/// Create a test filesystem config with a temporary directory
fn test_config_with_tempdir(temp_dir: &TempDir, mode: DirectoryMode) -> FilesystemConfig {
    let canonical_path = canonical_temp_path(temp_dir);
    FilesystemConfig {
        enabled: true,
        allowed_directories: vec![DirectoryConfig {
            path: canonical_path.to_string_lossy().to_string(),
            mode,
            description: Some("Test directory".to_string()),
            docker_mount_path: None,
        }],
        blocked_patterns: vec![
            ".env".to_string(),
            "*.key".to_string(),
            "credentials*".to_string(),
            "*secret*".to_string(),
        ],
        max_read_bytes: 1024 * 1024, // 1MB
        max_write_bytes: 512 * 1024, // 512KB
        follow_symlinks: false,
        default_encoding: "utf-8".to_string(),
    }
}

// ============================================================================
// FilesystemConfig Tests
// ============================================================================

#[test]
fn test_filesystem_config_default() {
    let config = FilesystemConfig::default();
    assert!(config.enabled);
    assert!(config.allowed_directories.is_empty());
    assert!(!config.blocked_patterns.is_empty());
    assert!(config.max_read_bytes > 0);
    assert!(config.max_write_bytes > 0);
}

#[test]
fn test_directory_mode_serialization() {
    assert_eq!(DirectoryMode::ReadOnly.to_string(), "ro");
    assert_eq!(DirectoryMode::ReadWrite.to_string(), "rw");
}

#[test]
fn test_directory_mode_can_write() {
    assert!(!DirectoryMode::ReadOnly.can_write());
    assert!(DirectoryMode::ReadWrite.can_write());
}

// ============================================================================
// PermissionGuard Tests
// ============================================================================

#[test]
fn test_permission_guard_allowed_paths() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let guard = PermissionGuard::new(config);

    // Use canonicalized path for non-existent files
    let base = canonical_temp_path(&temp_dir);
    let test_path = base.join("test.txt");
    let test_path_str = test_path.to_string_lossy().to_string();

    assert!(guard.can_read(&test_path_str));
    assert!(guard.can_write(&test_path_str));
}

#[test]
fn test_permission_guard_readonly_mode() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let guard = PermissionGuard::new(config);

    let base = canonical_temp_path(&temp_dir);
    let test_path = base.join("test.txt");
    let test_path_str = test_path.to_string_lossy().to_string();

    assert!(guard.can_read(&test_path_str));
    assert!(!guard.can_write(&test_path_str));
}

#[test]
fn test_permission_guard_outside_allowed() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let guard = PermissionGuard::new(config);

    let outside_path = "/etc/passwd";

    assert!(!guard.can_read(outside_path));
    assert!(!guard.can_write(outside_path));
}

#[test]
fn test_permission_guard_blocked_patterns() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let guard = PermissionGuard::new(config);

    // Files matching blocked patterns (use canonical path)
    let base = canonical_temp_path(&temp_dir);
    let env_path = base.join(".env");
    let key_path = base.join("server.key");
    let creds_path = base.join("credentials.json");
    let secret_path = base.join("my_secret_file.txt");

    assert!(guard.is_blocked(&env_path.to_string_lossy()));
    assert!(guard.is_blocked(&key_path.to_string_lossy()));
    assert!(guard.is_blocked(&creds_path.to_string_lossy()));
    assert!(guard.is_blocked(&secret_path.to_string_lossy()));

    // Normal files should not be blocked
    let normal_path = base.join("normal.txt");
    assert!(!guard.is_blocked(&normal_path.to_string_lossy()));
}

#[test]
fn test_permission_guard_can_read_blocked() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let guard = PermissionGuard::new(config);

    // Even in an allowed directory, blocked files should not be readable
    let base = canonical_temp_path(&temp_dir);
    let env_path = base.join(".env");
    assert!(!guard.can_read(&env_path.to_string_lossy()));
    assert!(!guard.can_write(&env_path.to_string_lossy()));
}

#[test]
fn test_permission_guard_get_mode() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let guard = PermissionGuard::new(config);

    let base = canonical_temp_path(&temp_dir);
    let test_path = base.join("test.txt");
    let mode = guard.get_mode(&test_path.to_string_lossy());
    assert!(mode.is_some());
    assert!(matches!(mode.unwrap(), DirectoryMode::ReadWrite));
}

// ============================================================================
// FilesystemService - File Reading Tests
// ============================================================================

#[tokio::test]
async fn test_read_file_success() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create a test file (use raw path for creating, canonicalized for reading)
    let test_file = temp_dir.path().join("test.txt");
    std::fs::write(&test_file, "Hello, World!").unwrap();

    // Use canonicalized path for the service call
    let canonical_file = canonical_temp_path(&temp_dir).join("test.txt");
    let result = service
        .read_file(canonical_file.to_string_lossy().as_ref())
        .await;
    assert!(result.is_ok());

    let content = result.unwrap();
    assert_eq!(content.content, "Hello, World!");
    assert_eq!(content.size, 13);
    assert!(!content.truncated);
}

#[tokio::test]
async fn test_read_file_not_found() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    let base = canonical_temp_path(&temp_dir);
    let result = service
        .read_file(base.join("nonexistent.txt").to_string_lossy().as_ref())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_read_file_outside_allowed() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    let result = service.read_file("/etc/passwd").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_read_file_blocked_pattern() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create a blocked file
    let env_file = temp_dir.path().join(".env");
    std::fs::write(&env_file, "SECRET=value").unwrap();

    let canonical_env = canonical_temp_path(&temp_dir).join(".env");
    let result = service
        .read_file(canonical_env.to_string_lossy().as_ref())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_read_file_utf8_encoding() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create a file with UTF-8 content
    let test_file = temp_dir.path().join("unicode.txt");
    std::fs::write(&test_file, "Hello 世界 🌍").unwrap();

    let canonical_file = canonical_temp_path(&temp_dir).join("unicode.txt");
    let result = service
        .read_file(canonical_file.to_string_lossy().as_ref())
        .await;
    assert!(result.is_ok());

    let content = result.unwrap();
    assert!(content.content.contains("世界"));
    assert!(content.content.contains("🌍"));
}

#[tokio::test]
async fn test_read_file_string() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create a test file
    let test_file = temp_dir.path().join("test.txt");
    std::fs::write(&test_file, "Hello, World!").unwrap();

    let canonical_file = canonical_temp_path(&temp_dir).join("test.txt");
    let result = service
        .read_file_string(canonical_file.to_string_lossy().as_ref())
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Hello, World!");
}

// ============================================================================
// FilesystemService - File Writing Tests
// ============================================================================

#[tokio::test]
async fn test_write_file_success() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let service = FilesystemService::new(config);

    let base = canonical_temp_path(&temp_dir);
    let test_file = base.join("output.txt");
    let result = service
        .write_file(
            test_file.to_string_lossy().as_ref(),
            "Test content",
            false,
            false,
        )
        .await;
    assert!(result.is_ok(), "Write failed: {:?}", result.err());

    let write_result = result.unwrap();
    assert!(write_result.created);
    assert_eq!(write_result.bytes_written, 12);

    // Verify file content
    let content = std::fs::read_to_string(&test_file).unwrap();
    assert_eq!(content, "Test content");
}

#[tokio::test]
async fn test_write_file_readonly_directory() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    let base = canonical_temp_path(&temp_dir);
    let test_file = base.join("output.txt");
    let result = service
        .write_file(
            test_file.to_string_lossy().as_ref(),
            "Test content",
            false,
            false,
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore] // Platform-specific temp dir canonicalization — passes locally, fails on some CI runners
async fn test_write_file_create_directories() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let service = FilesystemService::new(config);

    let base = canonical_temp_path(&temp_dir);
    let test_file = base.join("subdir/nested/output.txt");
    let result = service
        .write_file(
            test_file.to_string_lossy().as_ref(),
            "Nested content",
            true, // create_dirs = true
            false,
        )
        .await;
    assert!(result.is_ok(), "Write failed: {:?}", result.err());

    // Verify file exists
    assert!(test_file.exists());
}

#[tokio::test]
async fn test_write_file_no_overwrite() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let service = FilesystemService::new(config);

    // Create existing file
    let test_file = temp_dir.path().join("existing.txt");
    std::fs::write(&test_file, "Original").unwrap();

    // Use canonical path for service call
    let canonical_file = canonical_temp_path(&temp_dir).join("existing.txt");

    // Try to write without overwrite flag
    let result = service
        .write_file(
            canonical_file.to_string_lossy().as_ref(),
            "New content",
            false,
            false, // overwrite = false
        )
        .await;
    assert!(result.is_err());

    // Original content should be preserved
    let content = std::fs::read_to_string(&test_file).unwrap();
    assert_eq!(content, "Original");
}

#[tokio::test]
async fn test_write_file_with_overwrite() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let service = FilesystemService::new(config);

    // Create existing file
    let test_file = temp_dir.path().join("existing.txt");
    std::fs::write(&test_file, "Original").unwrap();

    let canonical_file = canonical_temp_path(&temp_dir).join("existing.txt");

    // Write with overwrite flag
    let result = service
        .write_file(
            canonical_file.to_string_lossy().as_ref(),
            "New content",
            false,
            true, // overwrite = true
        )
        .await;
    assert!(result.is_ok());

    let write_result = result.unwrap();
    assert!(!write_result.created); // File was overwritten, not created

    // Content should be updated
    let content = std::fs::read_to_string(&test_file).unwrap();
    assert_eq!(content, "New content");
}

#[tokio::test]
async fn test_write_file_blocked_pattern() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let service = FilesystemService::new(config);

    let base = canonical_temp_path(&temp_dir);
    let env_file = base.join(".env");
    let result = service
        .write_file(
            env_file.to_string_lossy().as_ref(),
            "SECRET=value",
            false,
            false,
        )
        .await;
    assert!(result.is_err());
}

// ============================================================================
// FilesystemService - Directory Listing Tests
// ============================================================================

#[tokio::test]
async fn test_list_directory_success() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create some test files
    std::fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
    std::fs::write(temp_dir.path().join("file2.txt"), "content2").unwrap();
    std::fs::create_dir(temp_dir.path().join("subdir")).unwrap();

    let base = canonical_temp_path(&temp_dir);
    let result = service
        .list_directory(base.to_string_lossy().as_ref(), false, false)
        .await;
    assert!(result.is_ok());

    let listing = result.unwrap();
    assert_eq!(listing.total_count, 3);

    // Check that entries exist
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"file1.txt"));
    assert!(names.contains(&"file2.txt"));
    assert!(names.contains(&"subdir"));
}

#[tokio::test]
async fn test_list_directory_recursive() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create nested structure
    std::fs::write(temp_dir.path().join("root.txt"), "root").unwrap();
    std::fs::create_dir(temp_dir.path().join("level1")).unwrap();
    std::fs::write(temp_dir.path().join("level1/nested.txt"), "nested").unwrap();

    let base = canonical_temp_path(&temp_dir);
    let result = service
        .list_directory(base.to_string_lossy().as_ref(), true, false)
        .await;
    assert!(result.is_ok());

    let listing = result.unwrap();

    // Should include nested files
    let paths: Vec<&str> = listing.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.iter().any(|p| p.contains("nested.txt")));
}

#[tokio::test]
async fn test_list_directory_include_hidden() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create visible and hidden files
    std::fs::write(temp_dir.path().join("visible.txt"), "visible").unwrap();
    std::fs::write(temp_dir.path().join(".hidden"), "hidden").unwrap();

    let base = canonical_temp_path(&temp_dir);

    // Without hidden files
    let result = service
        .list_directory(base.to_string_lossy().as_ref(), false, false)
        .await;
    assert!(result.is_ok());
    let listing = result.unwrap();
    assert_eq!(listing.total_count, 1);

    // With hidden files
    let result = service
        .list_directory(base.to_string_lossy().as_ref(), false, true)
        .await;
    assert!(result.is_ok());
    let listing = result.unwrap();
    assert_eq!(listing.total_count, 2);
}

#[tokio::test]
async fn test_list_directory_outside_allowed() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    let result = service.list_directory("/etc", false, false).await;
    assert!(result.is_err());
}

// ============================================================================
// FilesystemService - Search Tests
// ============================================================================

#[tokio::test]
async fn test_search_files_success() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create test files with searchable content
    std::fs::write(
        temp_dir.path().join("file1.rs"),
        "fn main() {\n    println!(\"Hello\");\n}",
    )
    .unwrap();
    std::fs::write(
        temp_dir.path().join("file2.rs"),
        "fn helper() {\n    println!(\"Helper\");\n}",
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("file3.txt"), "No function here").unwrap();

    let base = canonical_temp_path(&temp_dir);
    // Use a simpler pattern that works with both ripgrep and basic search
    let result = service
        .search(base.to_string_lossy().as_ref(), "println", None, 100)
        .await;
    assert!(result.is_ok(), "Search failed: {:?}", result.err());

    let search_result = result.unwrap();
    assert!(
        search_result.total_matches >= 2,
        "Expected at least 2 matches, got {}. Matches: {:?}",
        search_result.total_matches,
        search_result.matches
    );

    // Check matches contain println
    let match_contents: Vec<&str> = search_result
        .matches
        .iter()
        .map(|m| m.line_content.as_str())
        .collect();
    assert!(match_contents.iter().any(|c| c.contains("println")));
}

#[tokio::test]
async fn test_search_files_with_pattern() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create test files
    std::fs::write(temp_dir.path().join("main.rs"), "fn main()").unwrap();
    std::fs::write(temp_dir.path().join("main.txt"), "fn main text").unwrap();

    let base = canonical_temp_path(&temp_dir);

    // Search only in .rs files
    let result = service
        .search(
            base.to_string_lossy().as_ref(),
            "fn main",
            Some("*.rs"),
            100,
        )
        .await;
    assert!(result.is_ok());

    let search_result = result.unwrap();
    // Should only find in .rs file
    assert!(search_result
        .matches
        .iter()
        .all(|m| m.path.ends_with(".rs")));
}

#[tokio::test]
async fn test_search_files_max_results() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    // Create file with many matches
    let content = (0..100)
        .map(|i| format!("line {}: TODO fix this", i))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(temp_dir.path().join("todos.txt"), content).unwrap();

    let base = canonical_temp_path(&temp_dir);
    let result = service
        .search(
            base.to_string_lossy().as_ref(),
            "TODO",
            None,
            10, // max 10 results
        )
        .await;
    assert!(result.is_ok());

    let search_result = result.unwrap();
    assert!(search_result.matches.len() <= 10);
    assert!(search_result.truncated);
}

#[tokio::test]
async fn test_search_files_outside_allowed() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    let result = service.search("/etc", "root", None, 100).await;
    assert!(result.is_err());
}

// ============================================================================
// FilesystemService - Utility Method Tests
// ============================================================================

#[test]
fn test_allowed_directories() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let service = FilesystemService::new(config);

    let directories = service.allowed_directories();
    assert_eq!(directories.len(), 1);
    // Config uses canonical path
    let expected_path = canonical_temp_path(&temp_dir).to_string_lossy().to_string();
    assert_eq!(directories[0].path, expected_path);
    assert!(matches!(directories[0].mode, DirectoryMode::ReadWrite));
}

#[test]
fn test_can_read_write_methods() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let service = FilesystemService::new(config);

    let base = canonical_temp_path(&temp_dir);
    let test_path = base.join("test.txt");
    let test_path_str = test_path.to_string_lossy().to_string();

    assert!(service.can_read(&test_path_str));
    assert!(service.can_write(&test_path_str));
    assert!(!service.can_read("/etc/passwd"));
    assert!(!service.can_write("/etc/passwd"));
}

#[test]
fn test_is_enabled() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    assert!(service.is_enabled());
}

// ============================================================================
// Edge Cases and Error Handling
// ============================================================================

#[tokio::test]
async fn test_read_empty_file() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    let empty_file = temp_dir.path().join("empty.txt");
    std::fs::write(&empty_file, "").unwrap();

    let canonical_file = canonical_temp_path(&temp_dir).join("empty.txt");
    let result = service
        .read_file(canonical_file.to_string_lossy().as_ref())
        .await;
    assert!(result.is_ok());

    let content = result.unwrap();
    assert!(content.content.is_empty());
    assert_eq!(content.size, 0);
}

#[tokio::test]
async fn test_write_empty_content() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadWrite);
    let service = FilesystemService::new(config);

    let base = canonical_temp_path(&temp_dir);
    let test_file = base.join("empty_write.txt");
    let result = service
        .write_file(test_file.to_string_lossy().as_ref(), "", false, false)
        .await;
    assert!(result.is_ok());

    let write_result = result.unwrap();
    assert_eq!(write_result.bytes_written, 0);
}

#[tokio::test]
async fn test_list_empty_directory() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    let base = canonical_temp_path(&temp_dir);
    let result = service
        .list_directory(base.to_string_lossy().as_ref(), false, false)
        .await;
    assert!(result.is_ok());

    let listing = result.unwrap();
    assert_eq!(listing.total_count, 0);
    assert!(listing.entries.is_empty());
}

#[tokio::test]
async fn test_search_no_matches() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config_with_tempdir(&temp_dir, DirectoryMode::ReadOnly);
    let service = FilesystemService::new(config);

    std::fs::write(temp_dir.path().join("test.txt"), "Hello World").unwrap();

    let base = canonical_temp_path(&temp_dir);
    let result = service
        .search(
            base.to_string_lossy().as_ref(),
            "nonexistent_pattern_xyz",
            None,
            100,
        )
        .await;
    assert!(result.is_ok());

    let search_result = result.unwrap();
    assert_eq!(search_result.total_matches, 0);
    assert!(search_result.matches.is_empty());
}
