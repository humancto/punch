//! LINE Messaging API channel adapter.
//!
//! Sends reply and push messages via the LINE Messaging API and parses
//! incoming webhook events. Includes HMAC-SHA256 webhook signature verification.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const LINE_API_BASE: &str = "https://api.line.me";

type HmacSha256 = Hmac<Sha256>;

/// LINE Messaging API adapter.
///
/// Receives: LINE webhook events (message, follow, etc.).
/// Sends: reply messages and push messages via the LINE Messaging API.
pub struct LineAdapter {
    /// Channel access token for the LINE Messaging API.
    channel_access_token: String,
    /// Channel secret for webhook signature verification.
    channel_secret: String,
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

impl LineAdapter {
    /// Create a new LINE adapter.
    ///
    /// `channel_access_token`: Long-lived channel access token.
    /// `channel_secret`: Channel secret for webhook verification.
    pub fn new(channel_access_token: String, channel_secret: String) -> Self {
        Self {
            channel_access_token,
            channel_secret,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Verify a LINE webhook signature (HMAC-SHA256, base64-encoded).
    ///
    /// `signature`: The value of the `X-Line-Signature` header.
    /// `body`: The raw request body bytes.
    pub fn verify_webhook_signature(&self, signature: &str, body: &[u8]) -> bool {
        let mut mac = match HmacSha256::new_from_slice(self.channel_secret.as_bytes()) {
            Ok(m) => m,
            Err(_) => return false,
        };
        mac.update(body);
        let expected = mac.finalize().into_bytes();
        let expected_b64 = BASE64_STANDARD.encode(expected);

        // Constant-time comparison
        constant_time_eq(expected_b64.as_bytes(), signature.as_bytes())
    }

    /// Parse a LINE webhook event payload into `IncomingMessage`s.
    ///
    /// Expected JSON format:
    /// ```json
    /// {
    ///   "destination": "U...",
    ///   "events": [{
    ///     "type": "message",
    ///     "replyToken": "abc123",
    ///     "source": {
    ///       "type": "user",
    ///       "userId": "U..."
    ///     },
    ///     "timestamp": 1700000000000,
    ///     "message": {
    ///       "id": "msg123",
    ///       "type": "text",
    ///       "text": "Hello!"
    ///     }
    ///   }]
    /// }
    /// ```
    pub fn parse_webhook_payload(
        &self,
        payload: &serde_json::Value,
    ) -> Vec<IncomingMessage> {
        let events = match payload.get("events").and_then(|v| v.as_array()) {
            Some(events) => events,
            None => return Vec::new(),
        };

        let mut messages = Vec::new();

        for event in events {
            if let Some(msg) = self.parse_single_event(event) {
                messages.push(msg);
            }
        }

        messages
    }

    fn parse_single_event(&self, event: &serde_json::Value) -> Option<IncomingMessage> {
        let event_type = event.get("type")?.as_str()?;
        if event_type != "message" {
            return None;
        }

        let message = event.get("message")?;
        let msg_type = message.get("type")?.as_str()?;
        if msg_type != "text" {
            return None;
        }

        let text = message.get("text")?.as_str()?;
        if text.is_empty() {
            return None;
        }

        let msg_id = message
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let source = event.get("source")?;
        let source_type = source.get("type")?.as_str()?;
        let user_id = source.get("userId")?.as_str()?;

        let (channel_id, is_group) = match source_type {
            "group" => {
                let group_id = source
                    .get("groupId")
                    .and_then(|v| v.as_str())
                    .unwrap_or(user_id);
                (group_id.to_string(), true)
            }
            "room" => {
                let room_id = source
                    .get("roomId")
                    .and_then(|v| v.as_str())
                    .unwrap_or(user_id);
                (room_id.to_string(), true)
            }
            _ => (user_id.to_string(), false),
        };

        let timestamp_ms = event.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
        let timestamp =
            DateTime::from_timestamp(timestamp_ms / 1000, 0).unwrap_or_else(Utc::now);

        let mut metadata = HashMap::new();
        if let Some(reply_token) = event.get("replyToken").and_then(|v| v.as_str()) {
            metadata.insert(
                "reply_token".to_string(),
                serde_json::Value::String(reply_token.to_string()),
            );
        }

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id,
            user_id: user_id.to_string(),
            display_name: user_id.to_string(), // LINE doesn't provide display name in webhooks
            text: text.to_string(),
            timestamp,
            platform: ChannelPlatform::Line,
            platform_message_id: msg_id.to_string(),
            is_group,
            metadata,
        })
    }

    /// Send a reply message via the LINE Messaging API.
    ///
    /// Requires a valid `reply_token` from an incoming webhook event.
    pub async fn send_reply(&self, reply_token: &str, text: &str) -> PunchResult<()> {
        let url = format!("{}/v2/bot/message/reply", LINE_API_BASE);

        let body = serde_json::json!({
            "replyToken": reply_token,
            "messages": [{
                "type": "text",
                "text": text
            }]
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.channel_access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "line".to_string(),
                message: format!("failed to send reply: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("LINE send reply failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Send a push message to a specific user or group.
    pub async fn send_push(&self, to: &str, text: &str) -> PunchResult<()> {
        let url = format!("{}/v2/bot/message/push", LINE_API_BASE);

        let body = serde_json::json!({
            "to": to,
            "messages": [{
                "type": "text",
                "text": text
            }]
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.channel_access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "line".to_string(),
                message: format!("failed to send push message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("LINE push message failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Constant-time byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[async_trait]
impl ChannelAdapter for LineAdapter {
    fn name(&self) -> &str {
        "line"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Line
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("LINE adapter started (webhook mode)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("LINE adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        // Use push message for direct sends (reply requires a reply token)
        self.send_push(channel_id, message).await
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

    fn make_adapter() -> LineAdapter {
        LineAdapter::new(
            "test-channel-access-token".to_string(),
            "test-channel-secret".to_string(),
        )
    }

    #[test]
    fn test_line_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "line");
        assert_eq!(adapter.platform(), ChannelPlatform::Line);
    }

    #[test]
    fn test_verify_webhook_signature_valid() {
        let adapter = make_adapter();
        let body = b"test payload body";

        // Compute expected signature
        let mut mac = HmacSha256::new_from_slice(b"test-channel-secret").unwrap();
        mac.update(body);
        let expected = mac.finalize().into_bytes();
        let signature = BASE64_STANDARD.encode(expected);

        assert!(adapter.verify_webhook_signature(&signature, body));
    }

    #[test]
    fn test_verify_webhook_signature_invalid() {
        let adapter = make_adapter();
        let body = b"test payload body";
        let bad_signature = BASE64_STANDARD.encode(b"wrong-signature-value-padding!!");

        assert!(!adapter.verify_webhook_signature(&bad_signature, body));
    }

    #[test]
    fn test_parse_line_text_message() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "destination": "Udeadbeef",
            "events": [{
                "type": "message",
                "replyToken": "reply-token-123",
                "source": {
                    "type": "user",
                    "userId": "U1234567890"
                },
                "timestamp": 1700000000000_i64,
                "message": {
                    "id": "msg456",
                    "type": "text",
                    "text": "Hello from LINE!"
                }
            }]
        });

        let messages = adapter.parse_webhook_payload(&payload);
        assert_eq!(messages.len(), 1);

        let msg = &messages[0];
        assert_eq!(msg.platform, ChannelPlatform::Line);
        assert_eq!(msg.user_id, "U1234567890");
        assert_eq!(msg.text, "Hello from LINE!");
        assert_eq!(msg.platform_message_id, "msg456");
        assert!(!msg.is_group);
        assert!(msg.metadata.contains_key("reply_token"));
    }

    #[test]
    fn test_parse_line_group_message() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "destination": "Udeadbeef",
            "events": [{
                "type": "message",
                "replyToken": "reply-token-456",
                "source": {
                    "type": "group",
                    "groupId": "G9876543210",
                    "userId": "U1234567890"
                },
                "timestamp": 1700000001000_i64,
                "message": {
                    "id": "msg789",
                    "type": "text",
                    "text": "Group message!"
                }
            }]
        });

        let messages = adapter.parse_webhook_payload(&payload);
        assert_eq!(messages.len(), 1);

        let msg = &messages[0];
        assert!(msg.is_group);
        assert_eq!(msg.channel_id, "G9876543210");
        assert_eq!(msg.user_id, "U1234567890");
    }

    #[test]
    fn test_parse_line_non_text_message_ignored() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "destination": "Udeadbeef",
            "events": [{
                "type": "message",
                "replyToken": "reply-token-789",
                "source": {
                    "type": "user",
                    "userId": "U111"
                },
                "timestamp": 1700000000000_i64,
                "message": {
                    "id": "img123",
                    "type": "image"
                }
            }]
        });

        let messages = adapter.parse_webhook_payload(&payload);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_parse_line_follow_event_ignored() {
        let adapter = make_adapter();

        let payload = serde_json::json!({
            "destination": "Udeadbeef",
            "events": [{
                "type": "follow",
                "replyToken": "reply-token-000",
                "source": {
                    "type": "user",
                    "userId": "U222"
                },
                "timestamp": 1700000000000_i64
            }]
        });

        let messages = adapter.parse_webhook_payload(&payload);
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_line_adapter_start_stop() {
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
