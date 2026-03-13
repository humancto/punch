//! Rocket.Chat adapter.
//!
//! Sends messages via the Rocket.Chat REST API, supports authentication
//! via personal access token or user/password, and parses webhook payloads.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

/// Authentication method for Rocket.Chat.
pub enum RocketChatAuth {
    /// Personal access token + user ID.
    PersonalAccessToken { token: String, user_id: String },
    /// Username + password (will use login endpoint).
    UserPassword { username: String, password: String },
}

/// Rocket.Chat adapter.
///
/// Sends messages to channels/DMs and parses incoming webhook payloads.
pub struct RocketChatAdapter {
    /// Rocket.Chat server base URL (e.g. "https://chat.example.com").
    server_url: String,
    /// Auth method.
    auth: RocketChatAuth,
    /// Auth token (obtained after login or from personal access token).
    auth_token: RwLock<Option<String>>,
    /// User ID for API auth.
    auth_user_id: RwLock<Option<String>>,
    /// HTTP client.
    client: reqwest::Client,
    running: AtomicBool,
    started_at: RwLock<Option<DateTime<Utc>>>,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl RocketChatAdapter {
    /// Create a new Rocket.Chat adapter.
    pub fn new(server_url: String, auth: RocketChatAuth) -> Self {
        let server_url = server_url.trim_end_matches('/').to_string();
        Self {
            server_url,
            auth,
            auth_token: RwLock::new(None),
            auth_user_id: RwLock::new(None),
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Authenticate with the Rocket.Chat server.
    async fn authenticate(&self) -> PunchResult<()> {
        match &self.auth {
            RocketChatAuth::PersonalAccessToken { token, user_id } => {
                *self.auth_token.write().await = Some(token.clone());
                *self.auth_user_id.write().await = Some(user_id.clone());
                Ok(())
            }
            RocketChatAuth::UserPassword { username, password } => {
                let url = format!("{}/api/v1/login", self.server_url);

                let body = serde_json::json!({
                    "user": username,
                    "password": password,
                });

                let resp = self
                    .client
                    .post(&url)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| PunchError::Channel {
                        channel: "rocketchat".to_string(),
                        message: format!("login failed: {e}"),
                    })?;

                if !resp.status().is_success() {
                    let err_text = resp.text().await.unwrap_or_default();
                    return Err(PunchError::Channel {
                        channel: "rocketchat".to_string(),
                        message: format!("login failed: {err_text}"),
                    });
                }

                let data: serde_json::Value =
                    resp.json().await.map_err(|e| PunchError::Channel {
                        channel: "rocketchat".to_string(),
                        message: format!("parse login response: {e}"),
                    })?;

                let token = data
                    .get("data")
                    .and_then(|d| d.get("authToken"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| PunchError::Channel {
                        channel: "rocketchat".to_string(),
                        message: "missing authToken".to_string(),
                    })?;

                let user_id = data
                    .get("data")
                    .and_then(|d| d.get("userId"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| PunchError::Channel {
                        channel: "rocketchat".to_string(),
                        message: "missing userId".to_string(),
                    })?;

                *self.auth_token.write().await = Some(token.to_string());
                *self.auth_user_id.write().await = Some(user_id.to_string());
                Ok(())
            }
        }
    }

    /// Send a message to a channel or DM.
    async fn api_send_message(&self, room_id: &str, text: &str) -> PunchResult<()> {
        let url = format!("{}/api/v1/chat.sendMessage", self.server_url);

        let token = self.auth_token.read().await;
        let user_id = self.auth_user_id.read().await;

        let token = token.as_ref().ok_or_else(|| PunchError::Channel {
            channel: "rocketchat".to_string(),
            message: "not authenticated".to_string(),
        })?;
        let user_id = user_id.as_ref().ok_or_else(|| PunchError::Channel {
            channel: "rocketchat".to_string(),
            message: "not authenticated".to_string(),
        })?;

        let body = serde_json::json!({
            "message": {
                "rid": room_id,
                "msg": text,
            }
        });

        let resp = self
            .client
            .post(&url)
            .header("X-Auth-Token", token.as_str())
            .header("X-User-Id", user_id.as_str())
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "rocketchat".to_string(),
                message: format!("failed to send message: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Rocket.Chat send failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Parse a Rocket.Chat webhook payload into an `IncomingMessage`.
    ///
    /// Expected payload format:
    /// ```json
    /// {
    ///   "token": "webhook-token",
    ///   "channel_id": "GENERAL",
    ///   "channel_name": "general",
    ///   "message_id": "msg-123",
    ///   "user_id": "user-456",
    ///   "user_name": "alice",
    ///   "text": "Hello Rocket.Chat",
    ///   "timestamp": "2024-01-15T12:00:00Z",
    ///   "isGroupMessage": true
    /// }
    /// ```
    pub fn parse_webhook_payload(&self, payload: &serde_json::Value) -> Option<IncomingMessage> {
        let text = payload.get("text")?.as_str()?;
        if text.is_empty() {
            return None;
        }

        let channel_id = payload.get("channel_id")?.as_str()?;
        let user_id = payload
            .get("user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let user_name = payload
            .get("user_name")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let message_id = payload
            .get("message_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let timestamp = payload
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let is_group = payload
            .get("isGroupMessage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut metadata = HashMap::new();
        if let Some(channel_name) = payload.get("channel_name").and_then(|v| v.as_str()) {
            metadata.insert(
                "channel_name".to_string(),
                serde_json::Value::String(channel_name.to_string()),
            );
        }

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: channel_id.to_string(),
            user_id: user_id.to_string(),
            display_name: user_name.to_string(),
            text: text.to_string(),
            timestamp,
            platform: ChannelPlatform::RocketChat,
            platform_message_id: message_id.to_string(),
            is_group,
            metadata,
        })
    }
}

#[async_trait]
impl ChannelAdapter for RocketChatAdapter {
    fn name(&self) -> &str {
        "rocketchat"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::RocketChat
    }

    async fn start(&self) -> PunchResult<()> {
        self.authenticate().await?;
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(server = %self.server_url, "Rocket.Chat adapter started");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        *self.auth_token.write().await = None;
        *self.auth_user_id.write().await = None;
        info!("Rocket.Chat adapter stopped");
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

    fn make_adapter() -> RocketChatAdapter {
        RocketChatAdapter::new(
            "https://chat.example.com".to_string(),
            RocketChatAuth::PersonalAccessToken {
                token: "test-token".to_string(),
                user_id: "user-123".to_string(),
            },
        )
    }

    #[test]
    fn test_rocketchat_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "rocketchat");
        assert_eq!(adapter.platform(), ChannelPlatform::RocketChat);
    }

    #[test]
    fn test_parse_webhook_payload() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "token": "webhook-token",
            "channel_id": "GENERAL",
            "channel_name": "general",
            "message_id": "msg-123",
            "user_id": "user-456",
            "user_name": "alice",
            "text": "Hello Rocket.Chat",
            "timestamp": "2024-01-15T12:00:00Z",
            "isGroupMessage": true
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::RocketChat);
        assert_eq!(msg.text, "Hello Rocket.Chat");
        assert_eq!(msg.user_id, "user-456");
        assert_eq!(msg.display_name, "alice");
        assert_eq!(msg.channel_id, "GENERAL");
        assert!(msg.is_group);
    }

    #[test]
    fn test_parse_webhook_dm() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "channel_id": "DM-789",
            "message_id": "msg-456",
            "user_id": "user-111",
            "user_name": "bob",
            "text": "Private message",
            "timestamp": "2024-01-15T14:00:00Z",
            "isGroupMessage": false
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert!(!msg.is_group);
        assert_eq!(msg.channel_id, "DM-789");
    }

    #[test]
    fn test_parse_webhook_empty_text() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "channel_id": "GENERAL",
            "text": ""
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[tokio::test]
    async fn test_rocketchat_start_stop_with_pat() {
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
