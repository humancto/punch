//! WebSocket-based web chat adapter.
//!
//! Manages WebSocket sessions for browser-based chat clients. Each session
//! has a tokio broadcast channel for pushing messages back to the connected client.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

/// Capacity of each per-session broadcast channel.
const SESSION_CHANNEL_CAPACITY: usize = 256;

/// Represents an active WebSocket chat session.
#[derive(Debug, Clone)]
pub struct WebChatSession {
    /// Unique session identifier.
    pub session_id: String,
    /// Display name of the connected client.
    pub display_name: String,
    /// When the session was established.
    pub connected_at: DateTime<Utc>,
    /// Whether the session is currently active.
    pub active: bool,
}

/// WebSocket-based web chat adapter.
///
/// Manages multiple concurrent WebSocket sessions, each with its own
/// broadcast channel for sending messages back to clients.
pub struct WebChatAdapter {
    /// Active sessions, keyed by session ID.
    sessions: Arc<DashMap<String, WebChatSession>>,
    /// Broadcast senders per session for pushing messages to clients.
    senders: Arc<DashMap<String, broadcast::Sender<String>>>,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl WebChatAdapter {
    /// Create a new web chat adapter.
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            senders: Arc::new(DashMap::new()),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Register a new WebSocket session.
    ///
    /// Returns a broadcast `Receiver` that the WebSocket handler should use
    /// to forward messages to the connected client.
    pub fn register_session(
        &self,
        session_id: String,
        display_name: String,
    ) -> broadcast::Receiver<String> {
        let (tx, rx) = broadcast::channel(SESSION_CHANNEL_CAPACITY);

        let session = WebChatSession {
            session_id: session_id.clone(),
            display_name,
            connected_at: Utc::now(),
            active: true,
        };

        self.sessions.insert(session_id.clone(), session);
        self.senders.insert(session_id, tx);

        rx
    }

    /// Remove a WebSocket session (e.g. on disconnect).
    pub fn remove_session(&self, session_id: &str) {
        self.sessions.remove(session_id);
        self.senders.remove(session_id);
    }

    /// Get a list of active session IDs.
    pub fn active_sessions(&self) -> Vec<String> {
        self.sessions
            .iter()
            .filter(|entry| entry.value().active)
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Get the number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Create an `IncomingMessage` from a WebSocket text frame.
    ///
    /// `session_id`: The session that sent the message.
    /// `payload`: The JSON payload from the WebSocket frame.
    ///
    /// Expected JSON format:
    /// ```json
    /// {
    ///   "text": "Hello!",
    ///   "message_id": "optional-client-id"
    /// }
    /// ```
    pub fn create_message_from_ws(
        &self,
        session_id: &str,
        payload: &serde_json::Value,
    ) -> Option<IncomingMessage> {
        let text = payload["text"].as_str()?;
        if text.is_empty() {
            return None;
        }

        let session = self.sessions.get(session_id)?;
        let display_name = session.display_name.clone();

        let message_id = payload["message_id"].as_str().unwrap_or("").to_string();

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: session_id.to_string(),
            user_id: session_id.to_string(),
            display_name,
            text: text.to_string(),
            timestamp: Utc::now(),
            platform: ChannelPlatform::WebChat,
            platform_message_id: message_id,
            is_group: false,
            metadata: HashMap::new(),
        })
    }

    /// Send a message to a specific session via its broadcast channel.
    fn send_to_session(&self, session_id: &str, message: &str) -> PunchResult<()> {
        let sender = self
            .senders
            .get(session_id)
            .ok_or_else(|| PunchError::Channel {
                channel: "webchat".to_string(),
                message: format!("session {session_id} not found"),
            })?;

        sender.send(message.to_string()).map_err(|e| {
            warn!("Failed to send to webchat session {session_id}: {e}");
            PunchError::Channel {
                channel: "webchat".to_string(),
                message: format!("broadcast send failed for {session_id}: {e}"),
            }
        })?;

        Ok(())
    }
}

impl Default for WebChatAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelAdapter for WebChatAdapter {
    fn name(&self) -> &str {
        "webchat"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::WebChat
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("WebChat adapter started (WebSocket mode)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        // Clear all sessions
        self.sessions.clear();
        self.senders.clear();
        info!("WebChat adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.send_to_session(channel_id, message)?;
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn status(&self) -> ChannelStatus {
        ChannelStatus {
            connected: self.running.load(Ordering::Relaxed),
            started_at: self.started_at.try_read().ok().and_then(|g| *g),
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            last_error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webchat_adapter_creation() {
        let adapter = WebChatAdapter::new();
        assert_eq!(adapter.name(), "webchat");
        assert_eq!(adapter.platform(), ChannelPlatform::WebChat);
    }

    #[test]
    fn test_register_and_remove_session() {
        let adapter = WebChatAdapter::new();

        let _rx = adapter.register_session("sess-1".to_string(), "Alice".to_string());
        assert_eq!(adapter.session_count(), 1);
        assert_eq!(adapter.active_sessions(), vec!["sess-1"]);

        adapter.remove_session("sess-1");
        assert_eq!(adapter.session_count(), 0);
    }

    #[test]
    fn test_create_message_from_ws() {
        let adapter = WebChatAdapter::new();
        let _rx = adapter.register_session("sess-1".to_string(), "Alice".to_string());

        let payload = serde_json::json!({
            "text": "Hello from browser!",
            "message_id": "client-msg-1"
        });

        let msg = adapter.create_message_from_ws("sess-1", &payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::WebChat);
        assert_eq!(msg.user_id, "sess-1");
        assert_eq!(msg.display_name, "Alice");
        assert_eq!(msg.text, "Hello from browser!");
        assert_eq!(msg.platform_message_id, "client-msg-1");
        assert!(!msg.is_group);
    }

    #[test]
    fn test_create_message_from_ws_empty_text() {
        let adapter = WebChatAdapter::new();
        let _rx = adapter.register_session("sess-1".to_string(), "Alice".to_string());

        let payload = serde_json::json!({ "text": "" });
        let msg = adapter.create_message_from_ws("sess-1", &payload);
        assert!(msg.is_none());
    }

    #[test]
    fn test_create_message_from_ws_unknown_session() {
        let adapter = WebChatAdapter::new();

        let payload = serde_json::json!({ "text": "Hello" });
        let msg = adapter.create_message_from_ws("nonexistent", &payload);
        assert!(msg.is_none());
    }

    #[test]
    fn test_send_to_session() {
        let adapter = WebChatAdapter::new();
        let mut rx = adapter.register_session("sess-1".to_string(), "Alice".to_string());

        adapter
            .send_to_session("sess-1", "Response message")
            .unwrap();

        let received = rx.try_recv().unwrap();
        assert_eq!(received, "Response message");
    }

    #[test]
    fn test_send_to_unknown_session() {
        let adapter = WebChatAdapter::new();

        let result = adapter.send_to_session("nonexistent", "Hello");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_webchat_adapter_start_stop() {
        let adapter = WebChatAdapter::new();

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        assert!(adapter.status().connected);

        // Sessions should be cleared on stop
        let _rx = adapter.register_session("sess-1".to_string(), "Alice".to_string());
        assert_eq!(adapter.session_count(), 1);

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
        assert_eq!(adapter.session_count(), 0);
    }
}
