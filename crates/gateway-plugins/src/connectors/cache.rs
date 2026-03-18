//! Cache manager — versioned local cache with SHA-256 integrity verification.
//!
//! Provides a local versioned copy of connectors and bundles with
//! per-file SHA-256 hashes for integrity verification and rollback support.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// SHA-256 manifest for a cached connector/bundle version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    /// Connector or bundle name.
    pub name: String,

    /// Semantic version.
    pub version: String,

    /// When this cache entry was created.
    pub created_at: String,

    /// Per-file metadata: relative_path → FileMeta.
    pub files: HashMap<String, FileMeta>,

    /// Total size in bytes.
    pub total_size: u64,

    /// Total file count.
    pub file_count: usize,
}

/// Metadata for a single cached file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    /// SHA-256 hex digest of the file contents.
    pub sha256: String,

    /// File size in bytes.
    pub size: u64,
}

/// A cached connector or bundle entry.
#[derive(Debug, Clone)]
pub struct CachedEntry {
    /// Name of the connector/bundle.
    pub name: String,

    /// Cached version.
    pub version: String,

    /// Path to the cached version directory.
    pub path: PathBuf,

    /// Manifest for integrity verification.
    pub manifest: FileManifest,
}

/// Manages versioned local cache of connectors and bundles.
pub struct CacheManager {
    /// Root cache directory.
    cache_dir: PathBuf,
}

impl CacheManager {
    /// Create a new cache manager with the given root directory.
    pub fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Cache a source entry (copy from source to versioned cache).
    ///
    /// Returns the cached entry with manifest.
    pub fn cache_from_source(
        &self,
        name: &str,
        version: &str,
        source_path: &Path,
        kind: CacheKind,
    ) -> Result<CachedEntry, CacheError> {
        let dest_dir = self.version_dir(name, version, kind);

        // Create destination directory
        std::fs::create_dir_all(&dest_dir)
            .map_err(|e| CacheError::Io(format!("create cache dir: {}", e)))?;

        // Copy files and build manifest
        let mut files = HashMap::new();
        let mut total_size = 0u64;

        Self::copy_dir_recursive(
            source_path,
            &dest_dir,
            source_path,
            &mut files,
            &mut total_size,
        )?;

        let manifest = FileManifest {
            name: name.to_string(),
            version: version.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            file_count: files.len(),
            total_size,
            files,
        };

        // Write manifest
        let manifest_path = dest_dir.join(".manifest.json");
        let manifest_json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| CacheError::Serialization(e.to_string()))?;
        std::fs::write(&manifest_path, &manifest_json)
            .map_err(|e| CacheError::Io(format!("write manifest: {}", e)))?;

        Ok(CachedEntry {
            name: name.to_string(),
            version: version.to_string(),
            path: dest_dir,
            manifest,
        })
    }

    /// Verify integrity of a cached version.
    ///
    /// Returns `true` if all files match their SHA-256 hashes.
    pub fn verify_integrity(
        &self,
        name: &str,
        version: &str,
        kind: CacheKind,
    ) -> Result<bool, CacheError> {
        let dir = self.version_dir(name, version, kind);
        let manifest = self.load_manifest(&dir)?;

        for (rel_path, meta) in &manifest.files {
            let file_path = dir.join(rel_path);
            if !file_path.exists() {
                return Ok(false);
            }

            let actual_hash = Self::sha256_file(&file_path)?;
            if actual_hash != meta.sha256 {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Get a cached entry if it exists.
    pub fn get_cached(&self, name: &str, version: &str, kind: CacheKind) -> Option<CachedEntry> {
        let dir = self.version_dir(name, version, kind);
        if !dir.exists() {
            return None;
        }

        let manifest = self.load_manifest(&dir).ok()?;
        Some(CachedEntry {
            name: name.to_string(),
            version: version.to_string(),
            path: dir,
            manifest,
        })
    }

    /// List all cached versions for a name.
    pub fn list_versions(&self, name: &str, kind: CacheKind) -> Vec<String> {
        let base = self.name_dir(name, kind);
        if !base.exists() {
            return Vec::new();
        }

        let mut versions = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(v) = entry.file_name().to_str() {
                        versions.push(v.to_string());
                    }
                }
            }
        }
        // R5-L: Sort by numeric semver components instead of lexicographic order.
        // Lexicographic sort puts "1.10.0" before "1.9.0" which is wrong.
        versions.sort_by(|a, b| compare_version_strings(a, b));
        versions
    }

    /// Purge old versions, keeping only the most recent `keep_count`.
    pub fn purge_old_versions(
        &self,
        name: &str,
        keep_count: usize,
        kind: CacheKind,
    ) -> Result<usize, CacheError> {
        let mut versions = self.list_versions(name, kind);
        if versions.len() <= keep_count {
            return Ok(0);
        }

        // Sort by numeric semver components (oldest first)
        versions.sort_by(|a, b| compare_version_strings(a, b));
        let to_remove = versions.len() - keep_count;
        let mut removed = 0;

        for version in versions.iter().take(to_remove) {
            let dir = self.version_dir(name, version, kind);
            if std::fs::remove_dir_all(&dir).is_ok() {
                removed += 1;
            }
        }

        Ok(removed)
    }

    // --- Private helpers ---

    fn version_dir(&self, name: &str, version: &str, kind: CacheKind) -> PathBuf {
        self.cache_dir.join(kind.subdir()).join(name).join(version)
    }

    fn name_dir(&self, name: &str, kind: CacheKind) -> PathBuf {
        self.cache_dir.join(kind.subdir()).join(name)
    }

    fn load_manifest(&self, dir: &Path) -> Result<FileManifest, CacheError> {
        let manifest_path = dir.join(".manifest.json");
        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| CacheError::Io(format!("read manifest: {}", e)))?;
        serde_json::from_str(&content)
            .map_err(|e| CacheError::Serialization(format!("parse manifest: {}", e)))
    }

    fn sha256_file(path: &Path) -> Result<String, CacheError> {
        let bytes = std::fs::read(path)
            .map_err(|e| CacheError::Io(format!("read file {}: {}", path.display(), e)))?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn copy_dir_recursive(
        src: &Path,
        dest: &Path,
        base: &Path,
        files: &mut HashMap<String, FileMeta>,
        total_size: &mut u64,
    ) -> Result<(), CacheError> {
        if !src.is_dir() {
            return Ok(());
        }

        let entries = std::fs::read_dir(src)
            .map_err(|e| CacheError::Io(format!("read dir {}: {}", src.display(), e)))?;

        for entry in entries.flatten() {
            let src_path = entry.path();
            let rel = src_path
                .strip_prefix(base)
                .unwrap_or(&src_path)
                .to_string_lossy()
                .to_string();

            let dest_path = dest.join(&rel);

            if src_path.is_dir() {
                std::fs::create_dir_all(&dest_path)
                    .map_err(|e| CacheError::Io(format!("create dir: {}", e)))?;
                Self::copy_dir_recursive(&src_path, dest, base, files, total_size)?;
            } else {
                // Ensure parent dir exists
                if let Some(parent) = dest_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| CacheError::Io(format!("create parent: {}", e)))?;
                }

                let bytes = std::fs::read(&src_path)
                    .map_err(|e| CacheError::Io(format!("read {}: {}", src_path.display(), e)))?;
                let size = bytes.len() as u64;

                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                let sha256 = format!("{:x}", hasher.finalize());

                std::fs::write(&dest_path, &bytes)
                    .map_err(|e| CacheError::Io(format!("write {}: {}", dest_path.display(), e)))?;

                files.insert(rel, FileMeta { sha256, size });
                *total_size += size;
            }
        }

        Ok(())
    }
}

/// Kind of cached item (connector or plugin bundle).
#[derive(Debug, Clone, Copy)]
pub enum CacheKind {
    /// Individual connector.
    Connector,
    /// Plugin bundle.
    Plugin,
}

impl CacheKind {
    fn subdir(self) -> &'static str {
        match self {
            CacheKind::Connector => "connectors",
            CacheKind::Plugin => "plugins",
        }
    }
}

/// Errors from cache operations.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// Filesystem I/O error.
    #[error("cache io error: {0}")]
    Io(String),

    /// Serialization error.
    #[error("cache serialization error: {0}")]
    Serialization(String),

    /// Integrity verification failed.
    #[error("integrity check failed for {name} v{version}")]
    IntegrityFailed {
        /// Name of the corrupted entry.
        name: String,
        /// Version of the corrupted entry.
        version: String,
    },

    /// Entry not found in cache.
    #[error("not cached: {0} v{1}")]
    NotCached(String, String),
}

/// R5-L: Compare version strings by numeric components instead of lexicographic order.
/// Splits on `.` and `-`, compares each segment numerically (falls back to string compare).
/// This correctly sorts "1.9.0" before "1.10.0".
fn compare_version_strings(a: &str, b: &str) -> std::cmp::Ordering {
    let parse_segments = |s: &str| -> Vec<u64> {
        s.split(|c: char| c == '.' || c == '-')
            .map(|seg| seg.parse::<u64>().unwrap_or(u64::MAX))
            .collect()
    };
    parse_segments(a).cmp(&parse_segments(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_source(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "# Test Skill\n\nContent").unwrap();
        let scripts = dir.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("run.py"), "print('hello')").unwrap();
    }

    #[test]
    fn test_cache_from_source() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        create_test_source(&source);

        let cache = CacheManager::new(tmp.path().join("cache"));
        let entry = cache
            .cache_from_source("test-conn", "1.0.0", &source, CacheKind::Connector)
            .unwrap();

        assert_eq!(entry.name, "test-conn");
        assert_eq!(entry.version, "1.0.0");
        assert_eq!(entry.manifest.file_count, 2);
        assert!(entry.manifest.total_size > 0);
        assert!(entry.manifest.files.contains_key("SKILL.md"));
        assert!(entry.manifest.files.contains_key("scripts/run.py"));
    }

    #[test]
    fn test_verify_integrity_valid() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        create_test_source(&source);

        let cache = CacheManager::new(tmp.path().join("cache"));
        cache
            .cache_from_source("test", "1.0.0", &source, CacheKind::Connector)
            .unwrap();

        assert!(cache
            .verify_integrity("test", "1.0.0", CacheKind::Connector)
            .unwrap());
    }

    #[test]
    fn test_verify_integrity_tampered() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        create_test_source(&source);

        let cache = CacheManager::new(tmp.path().join("cache"));
        let entry = cache
            .cache_from_source("test", "1.0.0", &source, CacheKind::Connector)
            .unwrap();

        // Tamper with a file
        std::fs::write(entry.path.join("SKILL.md"), "TAMPERED!").unwrap();

        assert!(!cache
            .verify_integrity("test", "1.0.0", CacheKind::Connector)
            .unwrap());
    }

    #[test]
    fn test_get_cached() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        create_test_source(&source);

        let cache = CacheManager::new(tmp.path().join("cache"));
        cache
            .cache_from_source("test", "1.0.0", &source, CacheKind::Connector)
            .unwrap();

        let cached = cache.get_cached("test", "1.0.0", CacheKind::Connector);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().version, "1.0.0");

        let missing = cache.get_cached("test", "2.0.0", CacheKind::Connector);
        assert!(missing.is_none());
    }

    #[test]
    fn test_list_versions() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        create_test_source(&source);

        let cache = CacheManager::new(tmp.path().join("cache"));
        cache
            .cache_from_source("test", "1.0.0", &source, CacheKind::Connector)
            .unwrap();
        cache
            .cache_from_source("test", "1.1.0", &source, CacheKind::Connector)
            .unwrap();

        let versions = cache.list_versions("test", CacheKind::Connector);
        assert_eq!(versions, vec!["1.0.0", "1.1.0"]);
    }

    #[test]
    fn test_purge_old_versions() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        create_test_source(&source);

        let cache = CacheManager::new(tmp.path().join("cache"));
        cache
            .cache_from_source("test", "1.0.0", &source, CacheKind::Connector)
            .unwrap();
        cache
            .cache_from_source("test", "1.1.0", &source, CacheKind::Connector)
            .unwrap();
        cache
            .cache_from_source("test", "1.2.0", &source, CacheKind::Connector)
            .unwrap();

        let removed = cache
            .purge_old_versions("test", 1, CacheKind::Connector)
            .unwrap();
        assert_eq!(removed, 2);

        let remaining = cache.list_versions("test", CacheKind::Connector);
        assert_eq!(remaining, vec!["1.2.0"]);
    }

    #[test]
    fn test_cache_plugin_kind() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        create_test_source(&source);

        let cache = CacheManager::new(tmp.path().join("cache"));
        cache
            .cache_from_source("my-bundle", "1.0.0", &source, CacheKind::Plugin)
            .unwrap();

        let cached = cache.get_cached("my-bundle", "1.0.0", CacheKind::Plugin);
        assert!(cached.is_some());
        assert!(cached.unwrap().path.to_string_lossy().contains("plugins"));
    }
}
