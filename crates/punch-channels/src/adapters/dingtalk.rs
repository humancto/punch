//! DingTalk (Chinese enterprise messaging) adapter.
//!
//! Sends messages via the DingTalk Robot API with HMAC-SHA256 signature
//! verification and supports message card format.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const DINGTALK_ROBOT_API: &str = "https://oapi.dingtalk.com/robot/send";

type HmacSha256 = Hmac<Sha256>;

/// DingTalk robot adapter for enterprise messaging.
pub struct DingTalkAdapter {
    /// Robot access token.
    access_token: String,
    /// Secret for HMAC-SHA256 signature.
    secret: String,
    /// HTTP client.
    client: reqwest::Client,
    running: AtomicBool,
    started_at: RwLock<Option<DateTime<Utc>>>,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl DingTalkAdapter {
    /// Create a new DingTalk adapter.
    pub fn new(access_token: String, secret: String) -> Self {
        Self {
            access_token,
            secret,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Compute the DingTalk webhook signature.
    ///
    /// Signature = Base64(HmacSHA256(secret, timestamp + "\n" + secret))
    pub fn compute_signature(&self, timestamp_ms: i64) -> String {
        let string_to_sign = format!("{}\n{}", timestamp_ms, self.secret);
        let mut mac = HmacSha256::new_from_slice(self.secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(string_to_sign.as_bytes());
        let result = mac.finalize().into_bytes();
        base64::engine::general_purpose::STANDARD.encode(result)
    }

    /// Verify a DingTalk webhook signature.
    pub fn verify_signature(&self, timestamp_ms: i64, signature: &str) -> bool {
        let expected = self.compute_signature(timestamp_ms);
        expected == signature
    }

    /// Send a text message via the DingTalk Robot API.
    pub async fn send_text(&self, text: &str) -> PunchResult<()> {
        let timestamp_ms = Utc::now().timestamp_millis();
        let sign = self.compute_signature(timestamp_ms);
        let sign_encoded = urlencoding::encode(&sign);

        let url = format!(
            "{}?access_token={}&timestamp={}&sign={}",
            DINGTALK_ROBOT_API, self.access_token, timestamp_ms, sign_encoded
        );

        let body = serde_json::json!({
            "msgtype": "text",
            "text": { "content": text }
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "dingtalk".to_string(),
                message: format!("failed to send message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("DingTalk send failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Send a message card via the DingTalk Robot API.
    pub async fn send_action_card(&self, title: &str, markdown: &str) -> PunchResult<()> {
        let timestamp_ms = Utc::now().timestamp_millis();
        let sign = self.compute_signature(timestamp_ms);
        let sign_encoded = urlencoding::encode(&sign);

        let url = format!(
            "{}?access_token={}&timestamp={}&sign={}",
            DINGTALK_ROBOT_API, self.access_token, timestamp_ms, sign_encoded
        );

        let body = serde_json::json!({
            "msgtype": "actionCard",
            "actionCard": {
                "title": title,
                "text": markdown,
                "hideAvatar": "0",
                "btnOrientation": "0"
            }
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "dingtalk".to_string(),
                message: format!("failed to send action card: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("DingTalk action card send failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Parse a DingTalk webhook payload into an `IncomingMessage`.
    ///
    /// Expected payload format:
    /// ```json
    /// {
    ///   "msgtype": "text",
    ///   "text": { "content": "Hello" },
    ///   "msgId": "msg-123",
    ///   "createAt": 1705320000000,
    ///   "conversationId": "conv-456",
    ///   "senderId": "user-789",
    ///   "senderNick": "Alice"
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let msgtype = payload.get("msgtype")?.as_str()?;
        if msgtype != "text" {
            return None;
        }

        let text = payload
            .get("text")
            .and_then(|t| t.get("content"))
            .and_then(|v| v.as_str())?;
        if text.is_empty() {
            return None;
        }

        let msg_id = payload
            .get("msgId")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let sender_id = payload
            .get("senderId")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let sender_nick = payload
            .get("senderNick")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let conversation_id = payload
            .get("conversationId")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let created_at = payload
            .get("createAt")
            .and_then(|v| v.as_i64())
            .and_then(DateTime::from_timestamp_millis)
            .unwrap_or_else(Utc::now);

        let is_group = payload
            .get("conversationType")
            .and_then(|v| v.as_str())
            .map(|t| t == "2")
            .unwrap_or(false);

        let metadata = HashMap::new();

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: conversation_id.to_string(),
            user_id: sender_id.to_string(),
            display_name: sender_nick.to_string(),
            text: text.to_string(),
            timestamp: created_at,
            platform: ChannelPlatform::DingTalk,
            platform_message_id: msg_id.to_string(),
            is_group,
            metadata,
        })
    }
}

#[async_trait]
impl ChannelAdapter for DingTalkAdapter {
    fn name(&self) -> &str {
        "dingtalk"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::DingTalk
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("DingTalk adapter started");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("DingTalk adapter stopped");
        Ok(())
    }

    async fn send_response(&self, _channel_id: &str, message: &str) -> PunchResult<()> {
        self.send_text(message).await
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

    fn make_adapter() -> DingTalkAdapter {
        DingTalkAdapter::new(
            "test-access-token".to_string(),
            "SECtest-secret-key".to_string(),
        )
    }

    #[test]
    fn test_dingtalk_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "dingtalk");
        assert_eq!(adapter.platform(), ChannelPlatform::DingTalk);
    }

    #[test]
    fn test_compute_and_verify_signature() {
        let adapter = make_adapter();
        let timestamp = 1705320000000_i64;
        let sig = adapter.compute_signature(timestamp);
        assert!(!sig.is_empty());
        assert!(adapter.verify_signature(timestamp, &sig));
        assert!(!adapter.verify_signature(timestamp, "wrong-signature"));
    }

    #[test]
    fn test_parse_webhook_text_message() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "msgtype": "text",
            "text": { "content": "Hello DingTalk" },
            "msgId": "msg-123",
            "createAt": 1705320000000_i64,
            "conversationId": "conv-456",
            "senderId": "user-789",
            "senderNick": "Alice",
            "conversationType": "2"
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::DingTalk);
        assert_eq!(msg.text, "Hello DingTalk");
        assert_eq!(msg.user_id, "user-789");
        assert!(msg.is_group);
    }

    #[test]
    fn test_parse_webhook_non_text_ignored() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "msgtype": "image",
            "image": { "downloadCode": "abc" }
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[tokio::test]
    async fn test_dingtalk_start_stop() {
        let adapter = make_adapter();
        assert!(!adapter.status().connected);
        adapter.start().await.unwrap();
        assert!(adapter.status().connected);
        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
