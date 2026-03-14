//! Zulip chat adapter.
//!
//! Sends messages via the Zulip API using basic auth (email:api_key),
//! supports stream + topic addressing, and parses Zulip webhook payloads.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

/// Zulip chat adapter.
///
/// Sends messages to streams/topics and private messages.
/// Authenticates with email + API key (HTTP Basic Auth).
pub struct ZulipAdapter {
    /// Zulip server base URL (e.g. "https://yourorg.zulipchat.com").
    server_url: String,
    /// Bot email for authentication.
    bot_email: String,
    /// API key for authentication.
    api_key: String,
    /// HTTP client.
    client: reqwest::Client,
    running: AtomicBool,
    started_at: RwLock<Option<DateTime<Utc>>>,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl ZulipAdapter {
    /// Create a new Zulip adapter.
    pub fn new(server_url: String, bot_email: String, api_key: String) -> Self {
        let server_url = server_url.trim_end_matches('/').to_string();
        Self {
            server_url,
            bot_email,
            api_key,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Send a message to a Zulip stream with a topic.
    pub async fn send_stream_message(
        &self,
        stream: &str,
        topic: &str,
        content: &str,
    ) -> PunchResult<()> {
        let url = format!("{}/api/v1/messages", self.server_url);

        let params = [
            ("type", "stream"),
            ("to", stream),
            ("topic", topic),
            ("content", content),
        ];

        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.bot_email, Some(&self.api_key))
            .form(&params)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "zulip".to_string(),
                message: format!("failed to send message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Zulip send failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Send a private message to one or more users.
    pub async fn send_private_message(
        &self,
        user_emails: &[&str],
        content: &str,
    ) -> PunchResult<()> {
        let url = format!("{}/api/v1/messages", self.server_url);
        let to_json = serde_json::json!(user_emails).to_string();

        let params = [("type", "private"), ("to", &to_json), ("content", content)];

        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.bot_email, Some(&self.api_key))
            .form(&params)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "zulip".to_string(),
                message: format!("failed to send private message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Zulip private message failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Parse a Zulip outgoing webhook payload into an `IncomingMessage`.
    ///
    /// Expected payload format:
    /// ```json
    /// {
    ///   "message": {
    ///     "id": 12345,
    ///     "sender_id": 67890,
    ///     "sender_full_name": "Alice",
    ///     "sender_email": "alice@example.com",
    ///     "content": "Hello bot",
    ///     "timestamp": 1705320000,
    ///     "type": "stream",
    ///     "display_recipient": "general",
    ///     "subject": "greetings"
    ///   },
    ///   "trigger": "mention"
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let message = payload.get("message")?;

        let content = message.get("content")?.as_str()?;
        if content.is_empty() {
            return None;
        }

        let msg_id = message.get("id")?.as_u64()?;
        let sender_id = message
            .get("sender_id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let sender_name = message
            .get("sender_full_name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let sender_email = message
            .get("sender_email")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let msg_type = message
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("stream");
        let is_stream = msg_type == "stream";

        let timestamp = message
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
            .unwrap_or_else(Utc::now);

        let mut metadata = HashMap::new();
        metadata.insert(
            "sender_email".to_string(),
            serde_json::Value::String(sender_email.to_string()),
        );
        metadata.insert(
            "message_type".to_string(),
            serde_json::Value::String(msg_type.to_string()),
        );

        // For stream messages, include stream name and topic
        if is_stream {
            if let Some(stream) = message.get("display_recipient").and_then(|v| v.as_str()) {
                metadata.insert(
                    "stream".to_string(),
                    serde_json::Value::String(stream.to_string()),
                );
            }
            if let Some(topic) = message.get("subject").and_then(|v| v.as_str()) {
                metadata.insert(
                    "topic".to_string(),
                    serde_json::Value::String(topic.to_string()),
                );
            }
        }

        // Channel ID encodes "stream/topic" for stream messages
        let channel_id = if is_stream {
            let stream = message
                .get("display_recipient")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let topic = message
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("{stream}/{topic}")
        } else {
            sender_email.to_string()
        };

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id,
            user_id: sender_id.to_string(),
            display_name: sender_name.to_string(),
            text: content.to_string(),
            timestamp,
            platform: ChannelPlatform::Zulip,
            platform_message_id: msg_id.to_string(),
            is_group: is_stream,
            metadata,
        })
    }
}

#[async_trait]
impl ChannelAdapter for ZulipAdapter {
    fn name(&self) -> &str {
        "zulip"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Zulip
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(server = %self.server_url, "Zulip adapter started");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Zulip adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        // channel_id is "stream/topic" format
        if let Some((stream, topic)) = channel_id.split_once('/') {
            self.send_stream_message(stream, topic, message).await
        } else {
            // Treat as private message
            self.send_private_message(&[channel_id], message).await
        }
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

    fn make_adapter() -> ZulipAdapter {
        ZulipAdapter::new(
            "https://myorg.zulipchat.com".to_string(),
            "bot@myorg.zulipchat.com".to_string(),
            "test-api-key".to_string(),
        )
    }

    #[test]
    fn test_zulip_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "zulip");
        assert_eq!(adapter.platform(), ChannelPlatform::Zulip);
    }

    #[test]
    fn test_parse_stream_message() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "message": {
                "id": 12345,
                "sender_id": 67890,
                "sender_full_name": "Alice",
                "sender_email": "alice@example.com",
                "content": "Hello @bot",
                "timestamp": 1705320000,
                "type": "stream",
                "display_recipient": "general",
                "subject": "greetings"
            },
            "trigger": "mention"
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Zulip);
        assert_eq!(msg.text, "Hello @bot");
        assert_eq!(msg.channel_id, "general/greetings");
        assert_eq!(msg.display_name, "Alice");
        assert!(msg.is_group);
        assert_eq!(
            msg.metadata.get("stream").unwrap(),
            &serde_json::Value::String("general".to_string())
        );
        assert_eq!(
            msg.metadata.get("topic").unwrap(),
            &serde_json::Value::String("greetings".to_string())
        );
    }

    #[test]
    fn test_parse_private_message() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "message": {
                "id": 99999,
                "sender_id": 11111,
                "sender_full_name": "Bob",
                "sender_email": "bob@example.com",
                "content": "Private hello",
                "timestamp": 1705320000,
                "type": "private",
                "display_recipient": [
                    {"email": "bot@myorg.zulipchat.com"},
                    {"email": "bob@example.com"}
                ]
            },
            "trigger": "private_message"
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.channel_id, "bob@example.com");
        assert!(!msg.is_group);
    }

    #[test]
    fn test_parse_webhook_empty_content() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "message": {
                "id": 1,
                "content": "",
                "type": "stream"
            }
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[tokio::test]
    async fn test_zulip_start_stop() {
        let adapter = make_adapter();
        assert!(!adapter.status().connected);
        adapter.start().await.unwrap();
        assert!(adapter.status().connected);
        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
