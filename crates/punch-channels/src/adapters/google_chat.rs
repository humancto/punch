//! Google Chat (Workspace) adapter.
//!
//! Sends messages via the Google Chat REST API and parses incoming
//! webhook payloads for space messages and thread replies.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const GOOGLE_CHAT_API_BASE: &str = "https://chat.googleapis.com/v1";

/// Google Chat adapter for Workspace messaging.
///
/// Receives: Google Chat webhook payloads (MESSAGE events).
/// Sends: messages via the Google Chat REST API.
pub struct GoogleChatAdapter {
    /// OAuth2 access token or service account token.
    access_token: String,
    /// Default space to send messages to (e.g. "spaces/AAAA").
    default_space: String,
    /// HTTP client for API calls.
    client: reqwest::Client,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl GoogleChatAdapter {
    /// Create a new Google Chat adapter.
    pub fn new(access_token: String, default_space: String) -> Self {
        Self {
            access_token,
            default_space,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Parse a Google Chat webhook payload into an `IncomingMessage`.
    ///
    /// Expected payload format:
    /// ```json
    /// {
    ///   "type": "MESSAGE",
    ///   "message": {
    ///     "name": "spaces/AAAA/messages/BBBB",
    ///     "sender": { "name": "users/123", "displayName": "Alice" },
    ///     "text": "Hello bot",
    ///     "createTime": "2024-01-15T12:00:00Z",
    ///     "thread": { "name": "spaces/AAAA/threads/CCCC" },
    ///     "space": { "name": "spaces/AAAA", "type": "ROOM" }
    ///   }
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let event_type = payload.get("type")?.as_str()?;
        if event_type != "MESSAGE" {
            return None;
        }

        let message = payload.get("message")?;
        let sender = message.get("sender")?;

        let user_name = sender.get("name")?.as_str()?;
        let display_name = sender
            .get("displayName")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let text = message.get("text")?.as_str()?;
        if text.is_empty() {
            return None;
        }

        let message_name = message.get("name")?.as_str()?;
        let space = message
            .get("space")
            .and_then(|s| s.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.default_space);

        let created_at = message
            .get("createTime")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let is_room = message
            .get("space")
            .and_then(|s| s.get("type"))
            .and_then(|v| v.as_str())
            .map(|t| t == "ROOM")
            .unwrap_or(false);

        let mut metadata = HashMap::new();
        if let Some(thread) = message
            .get("thread")
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
        {
            metadata.insert(
                "thread_name".to_string(),
                serde_json::Value::String(thread.to_string()),
            );
        }

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: space.to_string(),
            user_id: user_name.to_string(),
            display_name: display_name.to_string(),
            text: text.to_string(),
            timestamp: created_at,
            platform: ChannelPlatform::GoogleChat,
            platform_message_id: message_name.to_string(),
            is_group: is_room,
            metadata,
        })
    }

    /// Send a message to a Google Chat space, optionally in a thread.
    async fn api_send_message(
        &self,
        space: &str,
        text: &str,
        thread_name: Option<&str>,
    ) -> PunchResult<()> {
        let url = format!("{}/{}/messages", GOOGLE_CHAT_API_BASE, space);

        let mut body = serde_json::json!({ "text": text });
        if let Some(thread) = thread_name {
            body["thread"] = serde_json::json!({ "name": thread });
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "google_chat".to_string(),
                message: format!("failed to send message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Google Chat send failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for GoogleChatAdapter {
    fn name(&self) -> &str {
        "google_chat"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::GoogleChat
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(space = %self.default_space, "Google Chat adapter started");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Google Chat adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.api_send_message(channel_id, message, None).await
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

    fn make_adapter() -> GoogleChatAdapter {
        GoogleChatAdapter::new("ya29.test-token".to_string(), "spaces/AAAA".to_string())
    }

    #[test]
    fn test_google_chat_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "google_chat");
        assert_eq!(adapter.platform(), ChannelPlatform::GoogleChat);
    }

    #[test]
    fn test_parse_webhook_message() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "type": "MESSAGE",
            "message": {
                "name": "spaces/AAAA/messages/BBBB",
                "sender": { "name": "users/123", "displayName": "Alice" },
                "text": "Hello bot",
                "createTime": "2024-01-15T12:00:00Z",
                "thread": { "name": "spaces/AAAA/threads/CCCC" },
                "space": { "name": "spaces/AAAA", "type": "ROOM" }
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::GoogleChat);
        assert_eq!(msg.user_id, "users/123");
        assert_eq!(msg.display_name, "Alice");
        assert_eq!(msg.text, "Hello bot");
        assert_eq!(msg.channel_id, "spaces/AAAA");
        assert!(msg.is_group);
        assert_eq!(
            msg.metadata.get("thread_name").unwrap(),
            &serde_json::Value::String("spaces/AAAA/threads/CCCC".to_string())
        );
    }

    #[test]
    fn test_parse_webhook_non_message_event() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "type": "ADDED_TO_SPACE",
            "space": { "name": "spaces/AAAA", "type": "ROOM" }
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[tokio::test]
    async fn test_google_chat_start_stop() {
        let adapter = make_adapter();
        assert!(!adapter.status().connected);
        adapter.start().await.unwrap();
        let status = adapter.status();
        assert!(status.connected);
        assert!(status.started_at.is_some());
        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
