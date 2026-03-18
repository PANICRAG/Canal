//! Asset Store - Layer 5 of the Five-Layer Automation Architecture
//!
//! Stores generated scripts for reuse, enabling subsequent operations
//! with near-zero token consumption.

use super::types::{AssetQuery, AssetStats, ScriptAsset};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

// ============================================================================
// Error Types
// ============================================================================

#[derive(Error, Debug)]
pub enum AssetStoreError {
    #[error("Asset not found: {0}")]
    NotFound(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

// ============================================================================
// Asset Store Trait
// ============================================================================

/// Trait for asset storage backends
#[async_trait]
pub trait AssetStore: Send + Sync {
    /// Save a script asset
    async fn save(&self, asset: ScriptAsset) -> Result<String, AssetStoreError>;

    /// Get an asset by ID
    async fn get(&self, id: &str) -> Result<Option<ScriptAsset>, AssetStoreError>;

    /// Find an asset matching the query
    async fn find(&self, query: &AssetQuery) -> Result<Option<ScriptAsset>, AssetStoreError>;

    /// Update an asset
    async fn update(&self, asset: &ScriptAsset) -> Result<(), AssetStoreError>;

    /// Delete an asset
    async fn delete(&self, id: &str) -> Result<(), AssetStoreError>;

    /// List all assets
    async fn list(&self, limit: Option<usize>) -> Result<Vec<ScriptAsset>, AssetStoreError>;

    /// Get store statistics
    async fn stats(&self) -> Result<AssetStats, AssetStoreError>;

    /// Record a usage (success/failure)
    async fn record_usage(&self, id: &str, success: bool) -> Result<(), AssetStoreError>;

    /// Clean up old/unused assets
    async fn cleanup(
        &self,
        max_age_secs: u64,
        min_success_rate: f64,
    ) -> Result<usize, AssetStoreError>;
}

// ============================================================================
// Asset Store Configuration
// ============================================================================

/// Configuration for asset stores
#[derive(Debug, Clone)]
pub struct AssetStoreConfig {
    /// Maximum number of assets to store
    pub max_assets: usize,
    /// Maximum age for assets (seconds)
    pub max_asset_age_secs: u64,
    /// Minimum success rate to keep an asset
    pub min_success_rate: f64,
    /// Auto-cleanup interval (seconds)
    pub cleanup_interval_secs: u64,
}

impl Default for AssetStoreConfig {
    fn default() -> Self {
        Self {
            max_assets: 1000,
            max_asset_age_secs: 86400 * 30, // 30 days
            min_success_rate: 0.5,
            cleanup_interval_secs: 3600, // 1 hour
        }
    }
}

// ============================================================================
// Memory Asset Store
// ============================================================================

/// In-memory asset store (for testing and development)
pub struct MemoryAssetStore {
    assets: RwLock<HashMap<String, ScriptAsset>>,
    config: AssetStoreConfig,
}

impl MemoryAssetStore {
    /// Create a new memory asset store
    pub fn new() -> Self {
        Self {
            assets: RwLock::new(HashMap::new()),
            config: AssetStoreConfig::default(),
        }
    }

    /// Create with config
    pub fn with_config(config: AssetStoreConfig) -> Self {
        Self {
            assets: RwLock::new(HashMap::new()),
            config,
        }
    }
}

impl Default for MemoryAssetStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AssetStore for MemoryAssetStore {
    async fn save(&self, asset: ScriptAsset) -> Result<String, AssetStoreError> {
        let id = asset.id.clone();
        let mut assets = self.assets.write().await;

        // Check max assets limit
        if assets.len() >= self.config.max_assets {
            // Remove oldest unused asset
            let oldest_id = assets
                .iter()
                .min_by_key(|(_, a)| a.last_used_at)
                .map(|(id, _)| id.clone());

            if let Some(old_id) = oldest_id {
                assets.remove(&old_id);
            }
        }

        assets.insert(id.clone(), asset);
        Ok(id)
    }

    async fn get(&self, id: &str) -> Result<Option<ScriptAsset>, AssetStoreError> {
        let assets = self.assets.read().await;
        Ok(assets.get(id).cloned())
    }

    async fn find(&self, query: &AssetQuery) -> Result<Option<ScriptAsset>, AssetStoreError> {
        let assets = self.assets.read().await;

        let matching: Vec<_> = assets
            .values()
            .filter(|asset| {
                // Match task signature
                if let Some(ref sig) = query.task_signature {
                    if !asset.task_signature.contains(sig) {
                        return false;
                    }
                }

                // Match URL pattern
                if let Some(ref pattern) = query.url_pattern {
                    if let Some(ref asset_pattern) = asset.url_pattern {
                        if !asset_pattern.contains(pattern) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }

                // Match success rate
                if let Some(min_rate) = query.min_success_rate {
                    if asset.success_rate < min_rate {
                        return false;
                    }
                }

                // Match age
                if let Some(max_age) = query.max_age_secs {
                    let age = Utc::now().signed_duration_since(asset.created_at);
                    if age.num_seconds() as u64 > max_age {
                        return false;
                    }
                }

                true
            })
            .collect();

        // Return the best match (highest success rate, most recent)
        let best = matching
            .into_iter()
            .max_by(|a, b| {
                a.success_rate
                    .partial_cmp(&b.success_rate)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.last_used_at.cmp(&b.last_used_at))
            })
            .cloned();

        Ok(best)
    }

    async fn update(&self, asset: &ScriptAsset) -> Result<(), AssetStoreError> {
        let mut assets = self.assets.write().await;
        if assets.contains_key(&asset.id) {
            assets.insert(asset.id.clone(), asset.clone());
            Ok(())
        } else {
            Err(AssetStoreError::NotFound(asset.id.clone()))
        }
    }

    async fn delete(&self, id: &str) -> Result<(), AssetStoreError> {
        let mut assets = self.assets.write().await;
        assets.remove(id);
        Ok(())
    }

    async fn list(&self, limit: Option<usize>) -> Result<Vec<ScriptAsset>, AssetStoreError> {
        let assets = self.assets.read().await;
        let mut list: Vec<_> = assets.values().cloned().collect();

        // Sort by last used (most recent first)
        list.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));

        if let Some(limit) = limit {
            list.truncate(limit);
        }

        Ok(list)
    }

    async fn stats(&self) -> Result<AssetStats, AssetStoreError> {
        let assets = self.assets.read().await;

        let total_assets = assets.len() as u64;
        let total_executions: u64 = assets.values().map(|a| a.total_executions).sum();
        let successful_executions: u64 = assets.values().map(|a| a.successful_executions).sum();

        let average_success_rate = if !assets.is_empty() {
            assets.values().map(|a| a.success_rate).sum::<f64>() / assets.len() as f64
        } else {
            0.0
        };

        // Get most used assets
        let mut sorted: Vec<_> = assets.values().collect();
        sorted.sort_by(|a, b| b.use_count.cmp(&a.use_count));
        let most_used: Vec<_> = sorted.iter().take(5).map(|a| a.id.clone()).collect();

        // Estimate tokens saved (assuming 4000 tokens per CV operation avoided)
        let tokens_saved = successful_executions * 4000;

        Ok(AssetStats {
            total_assets,
            total_executions,
            successful_executions,
            average_success_rate,
            most_used_assets: most_used,
            tokens_saved,
        })
    }

    async fn record_usage(&self, id: &str, success: bool) -> Result<(), AssetStoreError> {
        let mut assets = self.assets.write().await;

        if let Some(asset) = assets.get_mut(id) {
            asset.record_usage(success);
            Ok(())
        } else {
            Err(AssetStoreError::NotFound(id.to_string()))
        }
    }

    async fn cleanup(
        &self,
        max_age_secs: u64,
        min_success_rate: f64,
    ) -> Result<usize, AssetStoreError> {
        let mut assets = self.assets.write().await;
        let now = Utc::now();

        let to_remove: Vec<_> = assets
            .iter()
            .filter(|(_, asset)| {
                let age = now.signed_duration_since(asset.created_at);
                age.num_seconds() as u64 > max_age_secs
                    || (asset.total_executions > 5 && asset.success_rate < min_success_rate)
            })
            .map(|(id, _)| id.clone())
            .collect();

        let count = to_remove.len();
        for id in to_remove {
            assets.remove(&id);
        }

        Ok(count)
    }
}

// ============================================================================
// File Asset Store
// ============================================================================

/// File-based asset store
pub struct FileAssetStore {
    base_path: PathBuf,
    index: RwLock<HashMap<String, ScriptAsset>>,
    config: AssetStoreConfig,
}

impl FileAssetStore {
    /// Create a new file asset store
    pub async fn new(base_path: impl Into<PathBuf>) -> Result<Self, AssetStoreError> {
        let base_path = base_path.into();

        // Create directory if it doesn't exist
        tokio::fs::create_dir_all(&base_path).await?;

        let store = Self {
            base_path,
            index: RwLock::new(HashMap::new()),
            config: AssetStoreConfig::default(),
        };

        // Load existing assets
        store.load_index().await?;

        Ok(store)
    }

    /// Create with config
    pub async fn with_config(
        base_path: impl Into<PathBuf>,
        config: AssetStoreConfig,
    ) -> Result<Self, AssetStoreError> {
        let base_path = base_path.into();
        tokio::fs::create_dir_all(&base_path).await?;

        let store = Self {
            base_path,
            index: RwLock::new(HashMap::new()),
            config,
        };

        store.load_index().await?;
        Ok(store)
    }

    /// Load index from disk
    async fn load_index(&self) -> Result<(), AssetStoreError> {
        let index_path = self.base_path.join("index.json");

        if index_path.exists() {
            let content = tokio::fs::read_to_string(&index_path).await?;
            let loaded: HashMap<String, ScriptAsset> = serde_json::from_str(&content)
                .map_err(|e| AssetStoreError::SerializationError(e.to_string()))?;

            let mut index = self.index.write().await;
            *index = loaded;
        }

        Ok(())
    }

    /// Save index to disk
    async fn save_index(&self) -> Result<(), AssetStoreError> {
        let index = self.index.read().await;
        let content = serde_json::to_string_pretty(&*index)
            .map_err(|e| AssetStoreError::SerializationError(e.to_string()))?;

        let index_path = self.base_path.join("index.json");
        tokio::fs::write(&index_path, content).await?;

        Ok(())
    }

    /// Get script file path
    fn script_path(&self, id: &str) -> PathBuf {
        self.base_path.join(format!("{}.script", id))
    }
}

#[async_trait]
impl AssetStore for FileAssetStore {
    async fn save(&self, asset: ScriptAsset) -> Result<String, AssetStoreError> {
        let id = asset.id.clone();

        // Save script code to separate file
        let script_path = self.script_path(&id);
        tokio::fs::write(&script_path, &asset.code).await?;

        // Save metadata to index
        {
            let mut index = self.index.write().await;

            // Check limit
            if index.len() >= self.config.max_assets {
                let oldest_id = index
                    .iter()
                    .min_by_key(|(_, a)| a.last_used_at)
                    .map(|(id, _)| id.clone());

                if let Some(old_id) = oldest_id {
                    index.remove(&old_id);
                    let old_path = self.script_path(&old_id);
                    let _ = tokio::fs::remove_file(old_path).await;
                }
            }

            index.insert(id.clone(), asset);
        }

        self.save_index().await?;
        Ok(id)
    }

    async fn get(&self, id: &str) -> Result<Option<ScriptAsset>, AssetStoreError> {
        let index = self.index.read().await;

        if let Some(mut asset) = index.get(id).cloned() {
            // Load script code from file if needed
            let script_path = self.script_path(id);
            if script_path.exists() {
                asset.code = tokio::fs::read_to_string(&script_path).await?;
            }
            Ok(Some(asset))
        } else {
            Ok(None)
        }
    }

    async fn find(&self, query: &AssetQuery) -> Result<Option<ScriptAsset>, AssetStoreError> {
        let index = self.index.read().await;

        let matching: Vec<_> = index
            .values()
            .filter(|asset| {
                if let Some(ref sig) = query.task_signature {
                    if !asset.task_signature.contains(sig) {
                        return false;
                    }
                }

                if let Some(ref pattern) = query.url_pattern {
                    if let Some(ref asset_pattern) = asset.url_pattern {
                        if !asset_pattern.contains(pattern) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }

                if let Some(min_rate) = query.min_success_rate {
                    if asset.success_rate < min_rate {
                        return false;
                    }
                }

                if let Some(max_age) = query.max_age_secs {
                    let age = Utc::now().signed_duration_since(asset.created_at);
                    if age.num_seconds() as u64 > max_age {
                        return false;
                    }
                }

                true
            })
            .collect();

        let best = matching.into_iter().max_by(|a, b| {
            a.success_rate
                .partial_cmp(&b.success_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.last_used_at.cmp(&b.last_used_at))
        });

        if let Some(asset) = best {
            // Load full asset with code
            return self.get(&asset.id).await;
        }

        Ok(None)
    }

    async fn update(&self, asset: &ScriptAsset) -> Result<(), AssetStoreError> {
        {
            let mut index = self.index.write().await;
            if !index.contains_key(&asset.id) {
                return Err(AssetStoreError::NotFound(asset.id.clone()));
            }
            index.insert(asset.id.clone(), asset.clone());
        }

        // Update script file
        let script_path = self.script_path(&asset.id);
        tokio::fs::write(&script_path, &asset.code).await?;

        self.save_index().await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), AssetStoreError> {
        {
            let mut index = self.index.write().await;
            index.remove(id);
        }

        // Remove script file
        let script_path = self.script_path(id);
        let _ = tokio::fs::remove_file(script_path).await;

        self.save_index().await?;
        Ok(())
    }

    async fn list(&self, limit: Option<usize>) -> Result<Vec<ScriptAsset>, AssetStoreError> {
        let index = self.index.read().await;
        let mut list: Vec<_> = index.values().cloned().collect();

        list.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));

        if let Some(limit) = limit {
            list.truncate(limit);
        }

        Ok(list)
    }

    async fn stats(&self) -> Result<AssetStats, AssetStoreError> {
        let index = self.index.read().await;

        let total_assets = index.len() as u64;
        let total_executions: u64 = index.values().map(|a| a.total_executions).sum();
        let successful_executions: u64 = index.values().map(|a| a.successful_executions).sum();

        let average_success_rate = if !index.is_empty() {
            index.values().map(|a| a.success_rate).sum::<f64>() / index.len() as f64
        } else {
            0.0
        };

        let mut sorted: Vec<_> = index.values().collect();
        sorted.sort_by(|a, b| b.use_count.cmp(&a.use_count));
        let most_used: Vec<_> = sorted.iter().take(5).map(|a| a.id.clone()).collect();

        let tokens_saved = successful_executions * 4000;

        Ok(AssetStats {
            total_assets,
            total_executions,
            successful_executions,
            average_success_rate,
            most_used_assets: most_used,
            tokens_saved,
        })
    }

    async fn record_usage(&self, id: &str, success: bool) -> Result<(), AssetStoreError> {
        {
            let mut index = self.index.write().await;
            if let Some(asset) = index.get_mut(id) {
                asset.record_usage(success);
            } else {
                return Err(AssetStoreError::NotFound(id.to_string()));
            }
        }

        self.save_index().await?;
        Ok(())
    }

    async fn cleanup(
        &self,
        max_age_secs: u64,
        min_success_rate: f64,
    ) -> Result<usize, AssetStoreError> {
        let now = Utc::now();
        let to_remove: Vec<String>;

        {
            let index = self.index.read().await;
            to_remove = index
                .iter()
                .filter(|(_, asset)| {
                    let age = now.signed_duration_since(asset.created_at);
                    age.num_seconds() as u64 > max_age_secs
                        || (asset.total_executions > 5 && asset.success_rate < min_success_rate)
                })
                .map(|(id, _)| id.clone())
                .collect();
        }

        let count = to_remove.len();

        {
            let mut index = self.index.write().await;
            for id in &to_remove {
                index.remove(id);
                let script_path = self.script_path(id);
                let _ = tokio::fs::remove_file(script_path).await;
            }
        }

        self.save_index().await?;
        Ok(count)
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for asset stores
#[derive(Default)]
pub struct AssetStoreBuilder {
    store_type: AssetStoreType,
    base_path: Option<PathBuf>,
    config: AssetStoreConfig,
}

#[derive(Default, Clone, Copy)]
enum AssetStoreType {
    #[default]
    Memory,
    File,
}

impl AssetStoreBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Use memory storage
    pub fn memory(mut self) -> Self {
        self.store_type = AssetStoreType::Memory;
        self
    }

    /// Use file storage
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.store_type = AssetStoreType::File;
        self.base_path = Some(path.into());
        self
    }

    /// Set max assets
    pub fn max_assets(mut self, max: usize) -> Self {
        self.config.max_assets = max;
        self
    }

    /// Set max age
    pub fn max_age(mut self, secs: u64) -> Self {
        self.config.max_asset_age_secs = secs;
        self
    }

    /// Set min success rate
    pub fn min_success_rate(mut self, rate: f64) -> Self {
        self.config.min_success_rate = rate;
        self
    }

    /// Build the store
    pub async fn build(self) -> Result<Arc<dyn AssetStore>, AssetStoreError> {
        match self.store_type {
            AssetStoreType::Memory => Ok(Arc::new(MemoryAssetStore::with_config(self.config))),
            AssetStoreType::File => {
                let path = self.base_path.ok_or_else(|| {
                    AssetStoreError::StorageError("File path not specified".to_string())
                })?;
                Ok(Arc::new(
                    FileAssetStore::with_config(path, self.config).await?,
                ))
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::automation::types::{GeneratedScript, ScriptType};

    fn create_test_asset(id: &str) -> ScriptAsset {
        let script = GeneratedScript::new(
            ScriptType::Playwright,
            "console.log('test');",
            "javascript",
            "hash123",
            "task_sig",
        );
        let mut asset = ScriptAsset::from_script(&script);
        asset.id = id.to_string();
        asset
    }

    #[tokio::test]
    async fn test_memory_store_save_and_get() {
        let store = MemoryAssetStore::new();

        let asset = create_test_asset("test-1");
        let id = store.save(asset.clone()).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.code, asset.code);
    }

    #[tokio::test]
    async fn test_memory_store_find() {
        let store = MemoryAssetStore::new();

        let mut asset1 = create_test_asset("test-1");
        asset1.task_signature = "google_sheets_fill".to_string();
        asset1.success_rate = 0.9;

        let mut asset2 = create_test_asset("test-2");
        asset2.task_signature = "notion_update".to_string();
        asset2.success_rate = 0.8;

        store.save(asset1).await.unwrap();
        store.save(asset2).await.unwrap();

        let query = AssetQuery::new().with_task_signature("google");

        let found = store.find(&query).await.unwrap();
        assert!(found.is_some());
        assert!(found.unwrap().task_signature.contains("google"));
    }

    #[tokio::test]
    async fn test_memory_store_record_usage() {
        let store = MemoryAssetStore::new();

        let asset = create_test_asset("test-1");
        let id = store.save(asset).await.unwrap();

        store.record_usage(&id, true).await.unwrap();
        store.record_usage(&id, true).await.unwrap();
        store.record_usage(&id, false).await.unwrap();

        let updated = store.get(&id).await.unwrap().unwrap();
        assert_eq!(updated.use_count, 3);
        assert_eq!(updated.total_executions, 3);
        assert_eq!(updated.successful_executions, 2);
    }

    #[tokio::test]
    async fn test_memory_store_stats() {
        let store = MemoryAssetStore::new();

        let mut asset1 = create_test_asset("test-1");
        asset1.total_executions = 10;
        asset1.successful_executions = 8;

        let mut asset2 = create_test_asset("test-2");
        asset2.total_executions = 5;
        asset2.successful_executions = 4;

        store.save(asset1).await.unwrap();
        store.save(asset2).await.unwrap();

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.total_assets, 2);
        assert_eq!(stats.total_executions, 15);
        assert_eq!(stats.successful_executions, 12);
    }

    #[tokio::test]
    async fn test_memory_store_cleanup() {
        let store = MemoryAssetStore::new();

        let mut asset1 = create_test_asset("test-1");
        asset1.total_executions = 10;
        asset1.successful_executions = 2;
        asset1.success_rate = 0.2; // Low success rate

        let asset2 = create_test_asset("test-2"); // Fresh asset

        store.save(asset1).await.unwrap();
        store.save(asset2).await.unwrap();

        // Cleanup with 50% min success rate
        let removed = store.cleanup(86400 * 365, 0.5).await.unwrap();
        assert_eq!(removed, 1); // Should remove low success rate asset

        let remaining = store.list(None).await.unwrap();
        assert_eq!(remaining.len(), 1);
    }

    #[tokio::test]
    async fn test_asset_query_builder() {
        let query = AssetQuery::new()
            .with_task_signature("test")
            .with_url_pattern("example.com")
            .with_min_success_rate(0.8);

        assert_eq!(query.task_signature, Some("test".to_string()));
        assert_eq!(query.url_pattern, Some("example.com".to_string()));
        assert_eq!(query.min_success_rate, Some(0.8));
    }
}
