//! LinkedIn messaging adapter.
//!
//! Uses OAuth2 tokens to send messages via the LinkedIn Marketing API
//! and parses webhook notifications for incoming messages.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const LINKEDIN_API_BASE: &str = "https://api.linkedin.com/v2";

/// LinkedIn messaging adapter.
///
/// Sends messages via the LinkedIn API and parses webhook notifications.
pub struct LinkedInAdapter {
    /// OAuth2 access token.
    access_token: RwLock<String>,
    /// LinkedIn organization ID (for page messaging).
    organization_id: String,
    /// HTTP client.
    client: reqwest::Client,
    running: AtomicBool,
    started_at: RwLock<Option<DateTime<Utc>>>,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl LinkedInAdapter {
    /// Create a new LinkedIn adapter.
    pub fn new(access_token: String, organization_id: String) -> Self {
        Self {
            access_token: RwLock::new(access_token),
            organization_id,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Update the OAuth2 access token (e.g. after refresh).
    pub async fn set_access_token(&self, token: String) {
        *self.access_token.write().await = token;
    }

    /// Send a message to a LinkedIn conversation.
    async fn api_send_message(&self, conversation_urn: &str, text: &str) -> PunchResult<()> {
        let url = format!("{}/messages", LINKEDIN_API_BASE);
        let token = self.access_token.read().await.clone();

        let body = serde_json::json!({
            "recipients": [conversation_urn],
            "subject": "Message",
            "body": {
                "contentType": "text/plain",
                "text": text
            }
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("X-Restli-Protocol-Version", "2.0.0")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "linkedin".to_string(),
                message: format!("failed to send message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("LinkedIn send failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Parse a LinkedIn webhook notification payload into an `IncomingMessage`.
    ///
    /// Expected payload format:
    /// ```json
    /// {
    ///   "event": "MESSAGE",
    ///   "message": {
    ///     "id": "msg-123",
    ///     "sender": { "urn": "urn:li:person:abc", "name": "Alice" },
    ///     "body": { "text": "Hello" },
    ///     "createdAt": 1705320000000,
    ///     "conversationUrn": "urn:li:conversation:456"
    ///   }
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let event = payload.get("event")?.as_str()?;
        if event != "MESSAGE" {
            return None;
        }

        let message = payload.get("message")?;
        let sender = message.get("sender")?;

        let sender_urn = sender.get("urn")?.as_str()?;
        let sender_name = sender
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");

        let text = message
            .get("body")
            .and_then(|b| b.get("text"))
            .and_then(|v| v.as_str())?;
        if text.is_empty() {
            return None;
        }

        let msg_id = message
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let conversation_urn = message
            .get("conversationUrn")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let created_at = message
            .get("createdAt")
            .and_then(|v| v.as_i64())
            .and_then(DateTime::from_timestamp_millis)
            .unwrap_or_else(Utc::now);

        let metadata = HashMap::new();

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: conversation_urn.to_string(),
            user_id: sender_urn.to_string(),
            display_name: sender_name.to_string(),
            text: text.to_string(),
            timestamp: created_at,
            platform: ChannelPlatform::LinkedIn,
            platform_message_id: msg_id.to_string(),
            is_group: false,
            metadata,
        })
    }
}

#[async_trait]
impl ChannelAdapter for LinkedInAdapter {
    fn name(&self) -> &str {
        "linkedin"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::LinkedIn
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(org = %self.organization_id, "LinkedIn adapter started");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("LinkedIn adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.api_send_message(channel_id, message).await
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

    fn make_adapter() -> LinkedInAdapter {
        LinkedInAdapter::new("test-oauth-token".to_string(), "org-12345".to_string())
    }

    #[test]
    fn test_linkedin_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "linkedin");
        assert_eq!(adapter.platform(), ChannelPlatform::LinkedIn);
    }

    #[test]
    fn test_parse_webhook_message() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "event": "MESSAGE",
            "message": {
                "id": "msg-123",
                "sender": {
                    "urn": "urn:li:person:abc",
                    "name": "Alice"
                },
                "body": { "text": "Hello there" },
                "createdAt": 1705320000000_i64,
                "conversationUrn": "urn:li:conversation:456"
            }
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::LinkedIn);
        assert_eq!(msg.user_id, "urn:li:person:abc");
        assert_eq!(msg.display_name, "Alice");
        assert_eq!(msg.text, "Hello there");
        assert_eq!(msg.channel_id, "urn:li:conversation:456");
    }

    #[test]
    fn test_parse_webhook_non_message_event() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "event": "CONNECTION_REQUEST",
            "data": {}
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[tokio::test]
    async fn test_linkedin_start_stop() {
        let adapter = make_adapter();
        assert!(!adapter.status().connected);
        adapter.start().await.unwrap();
        assert!(adapter.status().connected);
        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
