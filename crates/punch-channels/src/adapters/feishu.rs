//! Feishu / Lark (ByteDance) adapter.
//!
//! Acquires a tenant access token, sends messages via the Feishu API,
//! and parses event subscription webhook payloads.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";

/// Feishu / Lark adapter for ByteDance enterprise messaging.
pub struct FeishuAdapter {
    /// App ID.
    app_id: String,
    /// App secret.
    app_secret: String,
    /// Tenant access token (refreshed periodically).
    tenant_token: RwLock<Option<String>>,
    /// HTTP client.
    client: reqwest::Client,
    running: AtomicBool,
    started_at: RwLock<Option<DateTime<Utc>>>,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl FeishuAdapter {
    /// Create a new Feishu adapter.
    pub fn new(app_id: String, app_secret: String) -> Self {
        Self {
            app_id,
            app_secret,
            tenant_token: RwLock::new(None),
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Acquire a tenant access token from the Feishu API.
    pub async fn acquire_tenant_token(&self) -> PunchResult<String> {
        let url = format!(
            "{}/auth/v3/tenant_access_token/internal",
            FEISHU_API_BASE
        );

        let body = serde_json::json!({
            "app_id": self.app_id,
            "app_secret": self.app_secret,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "feishu".to_string(),
                message: format!("failed to acquire token: {e}"),
            })?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(PunchError::Channel {
                channel: "feishu".to_string(),
                message: format!("token request failed: {err_text}"),
            });
        }

        let data: serde_json::Value = resp.json().await.map_err(|e| PunchError::Channel {
            channel: "feishu".to_string(),
            message: format!("parse error: {e}"),
        })?;

        let token = data
            .get("tenant_access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PunchError::Channel {
                channel: "feishu".to_string(),
                message: "missing tenant_access_token in response".to_string(),
            })?
            .to_string();

        *self.tenant_token.write().await = Some(token.clone());
        Ok(token)
    }

    /// Send a message to a Feishu chat.
    ///
    /// `receive_id`: The chat ID, user ID, or open_id.
    /// `receive_id_type`: "chat_id", "user_id", or "open_id".
    pub async fn send_message(
        &self,
        receive_id: &str,
        receive_id_type: &str,
        text: &str,
    ) -> PunchResult<()> {
        let token_guard = self.tenant_token.read().await;
        let token = token_guard.as_ref().ok_or_else(|| PunchError::Channel {
            channel: "feishu".to_string(),
            message: "no tenant token — call acquire_tenant_token first".to_string(),
        })?;

        let url = format!(
            "{}/im/v1/messages?receive_id_type={}",
            FEISHU_API_BASE, receive_id_type
        );

        let body = serde_json::json!({
            "receive_id": receive_id,
            "msg_type": "text",
            "content": serde_json::json!({ "text": text }).to_string(),
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "feishu".to_string(),
                message: format!("failed to send message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Feishu send failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Parse a Feishu event subscription webhook payload.
    ///
    /// Expected payload format (v2.0):
    /// ```json
    /// {
    ///   "schema": "2.0",
    ///   "header": { "event_type": "im.message.receive_v1", "event_id": "evt-123" },
    ///   "event": {
    ///     "message": {
    ///       "message_id": "om_123",
    ///       "chat_id": "oc_456",
    ///       "message_type": "text",
    ///       "content": "{\"text\":\"hello\"}",
    ///       "create_time": "1705320000000"
    ///     },
    ///     "sender": {
    ///       "sender_id": { "open_id": "ou_789" },
    ///       "sender_type": "user"
    ///     }
    ///   }
    /// }
    /// ```
    pub fn parse_event_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let header = payload.get("header")?;
        let event_type = header.get("event_type")?.as_str()?;
        if event_type != "im.message.receive_v1" {
            return None;
        }

        let event = payload.get("event")?;
        let message = event.get("message")?;
        let sender = event.get("sender")?;

        let message_type = message.get("message_type")?.as_str()?;
        if message_type != "text" {
            return None;
        }

        let content_str = message.get("content")?.as_str()?;
        let content: serde_json::Value = serde_json::from_str(content_str).ok()?;
        let text = content.get("text")?.as_str()?;
        if text.is_empty() {
            return None;
        }

        let message_id = message.get("message_id")?.as_str()?;
        let chat_id = message
            .get("chat_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let open_id = sender
            .get("sender_id")
            .and_then(|s| s.get("open_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let created_at = message
            .get("create_time")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(DateTime::from_timestamp_millis)
            .unwrap_or_else(Utc::now);

        let metadata = HashMap::new();

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: chat_id.to_string(),
            user_id: open_id.to_string(),
            display_name: open_id.to_string(),
            text: text.to_string(),
            timestamp: created_at,
            platform: ChannelPlatform::Feishu,
            platform_message_id: message_id.to_string(),
            is_group: true,
            metadata,
        })
    }
}

#[async_trait]
impl ChannelAdapter for FeishuAdapter {
    fn name(&self) -> &str {
        "feishu"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Feishu
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(app_id = %self.app_id, "Feishu adapter started");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        *self.tenant_token.write().await = None;
        info!("Feishu adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.send_message(channel_id, "chat_id", message).await
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

    fn make_adapter() -> FeishuAdapter {
        FeishuAdapter::new("cli_test_app_id".to_string(), "test-app-secret".to_string())
    }

    #[test]
    fn test_feishu_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "feishu");
        assert_eq!(adapter.platform(), ChannelPlatform::Feishu);
    }

    #[test]
    fn test_parse_event_message() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1",
                "event_id": "evt-123"
            },
            "event": {
                "message": {
                    "message_id": "om_123",
                    "chat_id": "oc_456",
                    "message_type": "text",
                    "content": "{\"text\":\"hello feishu\"}",
                    "create_time": "1705320000000"
                },
                "sender": {
                    "sender_id": { "open_id": "ou_789" },
                    "sender_type": "user"
                }
            }
        });

        let msg = adapter.parse_event_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Feishu);
        assert_eq!(msg.text, "hello feishu");
        assert_eq!(msg.user_id, "ou_789");
        assert_eq!(msg.channel_id, "oc_456");
    }

    #[test]
    fn test_parse_event_wrong_type() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.chat.member.bot.added_v1",
                "event_id": "evt-456"
            },
            "event": {}
        });
        assert!(adapter.parse_event_payload(&payload).is_none());
    }

    #[tokio::test]
    async fn test_feishu_start_stop() {
        let adapter = make_adapter();
        assert!(!adapter.status().connected);
        adapter.start().await.unwrap();
        assert!(adapter.status().connected);
        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
