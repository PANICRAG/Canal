//! Local (in-process) implementation of MemoryService.

use std::sync::Arc;

use async_trait::async_trait;
use gateway_service_traits::error::{ServiceError, ServiceResult};
use gateway_service_traits::memory::{MemoryItem, MemoryService};

use crate::memory::UnifiedMemoryStore;

/// In-process memory service wrapping a concrete `UnifiedMemoryStore`.
pub struct LocalMemoryService {
    store: Arc<UnifiedMemoryStore>,
}

impl LocalMemoryService {
    /// Create a new local memory service.
    pub fn new(store: Arc<UnifiedMemoryStore>) -> Self {
        Self { store }
    }

    /// Get the underlying store (for internal use).
    pub fn store(&self) -> &Arc<UnifiedMemoryStore> {
        &self.store
    }
}

/// Convert a MemoryEntry to the service boundary MemoryItem.
fn entry_to_item(e: crate::memory::MemoryEntry) -> MemoryItem {
    MemoryItem {
        id: e.id.to_string(),
        category: format!("{:?}", e.category),
        content: e.content,
        confidence: match e.confidence {
            crate::memory::Confidence::High | crate::memory::Confidence::Confirmed => 0.95,
            crate::memory::Confidence::Medium => 0.7,
            crate::memory::Confidence::Low => 0.3,
        },
        source: format!("{:?}", e.source),
        created_at: e.created_at.to_rfc3339(),
    }
}

#[async_trait]
impl MemoryService for LocalMemoryService {
    async fn store(&self, item: MemoryItem) -> ServiceResult<()> {
        use crate::memory::{Confidence, MemoryCategory, MemoryEntry, MemorySource};
        use uuid::Uuid;

        let user_id = Uuid::parse_str(&item.id).unwrap_or_else(|_| Uuid::new_v4());
        let category = match item.category.as_str() {
            "preference" | "Preference" => MemoryCategory::Preference,
            "pattern" | "Pattern" => MemoryCategory::Pattern,
            "project" | "Project" => MemoryCategory::Project,
            "task" | "Task" => MemoryCategory::Task,
            "knowledge" | "Knowledge" => MemoryCategory::Knowledge,
            "custom_instruction" | "CustomInstruction" => MemoryCategory::CustomInstruction,
            _ => MemoryCategory::Knowledge,
        };
        let confidence = if item.confidence >= 0.9 {
            Confidence::High
        } else if item.confidence >= 0.5 {
            Confidence::Medium
        } else {
            Confidence::Low
        };

        let now = chrono::Utc::now();
        let entry = MemoryEntry {
            id: Uuid::new_v4(),
            key: item.content.clone(),
            category,
            title: None,
            content: item.content,
            structured_data: None,
            tags: vec![],
            confidence,
            source: MemorySource::System,
            metadata: Default::default(),
            session_id: None,
            created_at: now,
            updated_at: now,
            access_count: 0,
            last_accessed: now,
            version: 1,
        };

        self.store
            .store(user_id, entry)
            .await
            .map_err(|e| ServiceError::Internal(e.to_string()))
    }

    async fn query(
        &self,
        _category: &str,
        query: &str,
        limit: usize,
    ) -> ServiceResult<Vec<MemoryItem>> {
        use uuid::Uuid;

        let user_id = Uuid::nil();
        let results = self.store.search(user_id, query, limit).await;
        Ok(results.into_iter().map(entry_to_item).collect())
    }

    async fn list_by_category(
        &self,
        _category: &str,
        _limit: usize,
    ) -> ServiceResult<Vec<MemoryItem>> {
        Ok(vec![])
    }

    async fn delete(&self, _id: &str) -> ServiceResult<()> {
        Ok(())
    }

    async fn count(&self) -> ServiceResult<usize> {
        use uuid::Uuid;
        let results = self.store.search(Uuid::nil(), "", 10000).await;
        Ok(results.len())
    }

    async fn health(&self) -> ServiceResult<bool> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_memory_service_health() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let service = LocalMemoryService::new(store);
        assert!(service.health().await.unwrap());
    }

    #[tokio::test]
    async fn test_local_memory_service_count_empty() {
        let store = Arc::new(UnifiedMemoryStore::new());
        let service = LocalMemoryService::new(store);
        assert_eq!(service.count().await.unwrap(), 0);
    }
}
