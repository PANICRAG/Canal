//! WebSocket server for extension connections
//!
//! NOTE: The Chrome Extension browser integration has been removed (CV8: replaced by canal-cv).
//! This module retains the WebSocketManager for general WebSocket connection management
//! and the cleanup task, but the browser-specific handler has been removed.

use axum::Router;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::state::AppState;

/// Maximum time to wait for a pong response before considering connection dead
const CLIENT_TIMEOUT: Duration = Duration::from_secs(600);

/// Connection identifier
pub type ConnectionId = Uuid;

/// Information about a connected client
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    /// Unique connection ID
    #[allow(dead_code)]
    pub id: ConnectionId,
    /// Client name/identifier
    pub client_name: Option<String>,
    /// When the connection was established
    #[allow(dead_code)]
    pub connected_at: Instant,
    /// Last activity timestamp
    pub last_activity: Instant,
    /// Client metadata
    pub metadata: HashMap<String, String>,
}

impl ConnectionInfo {
    fn new(id: ConnectionId) -> Self {
        let now = Instant::now();
        Self {
            id,
            client_name: None,
            connected_at: now,
            last_activity: now,
            metadata: HashMap::new(),
        }
    }
}

/// Command sent to a connection handler
#[derive(Debug, Clone)]
pub enum ConnectionCommand {
    /// Send a message to the client
    Send(String),
    /// Close the connection
    Close,
}

/// Manager for all WebSocket connections
#[derive(Debug, Default)]
pub struct WebSocketManager {
    /// Active connections: connection_id -> (info, sender channel)
    connections:
        RwLock<HashMap<ConnectionId, (ConnectionInfo, mpsc::UnboundedSender<ConnectionCommand>)>>,
}

impl WebSocketManager {
    /// Create a new WebSocket manager
    pub fn new() -> Self {
        Self {
            connections: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new connection
    pub async fn register(
        &self,
        id: ConnectionId,
        tx: mpsc::UnboundedSender<ConnectionCommand>,
    ) -> ConnectionInfo {
        let info = ConnectionInfo::new(id);
        let mut connections = self.connections.write().await;
        connections.insert(id, (info.clone(), tx));
        tracing::info!(connection_id = %id, "WebSocket connection registered");
        info
    }

    /// Unregister a connection
    pub async fn unregister(&self, id: &ConnectionId) {
        let mut connections = self.connections.write().await;
        if connections.remove(id).is_some() {
            tracing::info!(connection_id = %id, "WebSocket connection unregistered");
        }
    }

    /// Update connection info (e.g., client name, metadata)
    pub async fn update_info<F>(&self, id: &ConnectionId, f: F)
    where
        F: FnOnce(&mut ConnectionInfo),
    {
        let mut connections = self.connections.write().await;
        if let Some((info, _)) = connections.get_mut(id) {
            f(info);
        }
    }

    /// Update last activity timestamp
    pub async fn update_activity(&self, id: &ConnectionId) {
        let mut connections = self.connections.write().await;
        if let Some((info, _)) = connections.get_mut(id) {
            info.last_activity = Instant::now();
        }
    }

    /// Get connection count
    #[allow(dead_code)]
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Get all connection IDs
    #[allow(dead_code)]
    pub async fn get_connection_ids(&self) -> Vec<ConnectionId> {
        self.connections.read().await.keys().cloned().collect()
    }

    /// Get connection info
    pub async fn get_info(&self, id: &ConnectionId) -> Option<ConnectionInfo> {
        self.connections
            .read()
            .await
            .get(id)
            .map(|(info, _)| info.clone())
    }

    /// Get all connection infos
    #[allow(dead_code)]
    pub async fn get_all_infos(&self) -> Vec<ConnectionInfo> {
        self.connections
            .read()
            .await
            .values()
            .map(|(info, _)| info.clone())
            .collect()
    }

    /// Send a message to a specific connection
    pub async fn send_to(&self, id: &ConnectionId, message: String) -> bool {
        let connections = self.connections.read().await;
        if let Some((_, tx)) = connections.get(id) {
            tx.send(ConnectionCommand::Send(message)).is_ok()
        } else {
            false
        }
    }

    /// Broadcast a message to all connections
    #[allow(dead_code)]
    pub async fn broadcast(&self, message: String) {
        let connections = self.connections.read().await;
        for (id, (_, tx)) in connections.iter() {
            if tx.send(ConnectionCommand::Send(message.clone())).is_err() {
                tracing::warn!(connection_id = %id, "Failed to send broadcast to connection");
            }
        }
    }

    /// Broadcast a message to all connections except one
    #[allow(dead_code)]
    pub async fn broadcast_except(&self, exclude_id: &ConnectionId, message: String) {
        let connections = self.connections.read().await;
        for (id, (_, tx)) in connections.iter() {
            if id != exclude_id {
                if tx.send(ConnectionCommand::Send(message.clone())).is_err() {
                    tracing::warn!(connection_id = %id, "Failed to send broadcast to connection");
                }
            }
        }
    }

    /// Close a specific connection
    pub async fn close(&self, id: &ConnectionId) {
        let connections = self.connections.read().await;
        if let Some((_, tx)) = connections.get(id) {
            let _ = tx.send(ConnectionCommand::Close);
        }
    }

    /// Close all connections
    #[allow(dead_code)]
    pub async fn close_all(&self) {
        let connections = self.connections.read().await;
        for (_, tx) in connections.values() {
            let _ = tx.send(ConnectionCommand::Close);
        }
    }

    /// Remove stale connections (no activity within timeout)
    pub async fn cleanup_stale(&self) {
        let now = Instant::now();
        let connections = self.connections.read().await;
        let stale_entries: Vec<(ConnectionId, u64)> = connections
            .iter()
            .filter(|(_, (info, _))| now.duration_since(info.last_activity) > CLIENT_TIMEOUT)
            .map(|(id, (info, _))| (*id, now.duration_since(info.last_activity).as_secs()))
            .collect();
        drop(connections);

        for (id, inactive_secs) in &stale_entries {
            tracing::warn!(
                connection_id = %id,
                inactive_duration_secs = inactive_secs,
                timeout_secs = CLIENT_TIMEOUT.as_secs(),
                "Closing stale WebSocket connection"
            );
            self.close(id).await;
            self.unregister(id).await;
        }
    }
}

/// Create the WebSocket routes (browser handler removed in CV8 migration)
pub fn routes() -> Router<AppState> {
    Router::new()
}

/// Spawn a background task that periodically cleans up stale connections
pub fn spawn_cleanup_task(ws_manager: Arc<WebSocketManager>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            ws_manager.cleanup_stale().await;
        }
    });
}
