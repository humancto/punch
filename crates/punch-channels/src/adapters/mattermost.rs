//! Mattermost adapter.
//!
//! Sends posts via the Mattermost REST API and parses incoming
//! webhook payloads.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

/// Mattermost REST API adapter.
///
/// Sends posts to channels and parses webhook payloads.
pub struct MattermostAdapter {
    /// Mattermost server base URL (e.g. "https://mattermost.example.com").
    server_url: String,
    /// Bearer token for authentication.
    token: String,
    /// Default team ID.
    team_id: String,
    /// HTTP client.
    client: reqwest::Client,
    running: AtomicBool,
    started_at: RwLock<Option<DateTime<Utc>>>,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl MattermostAdapter {
    /// Create a new Mattermost adapter.
    pub fn new(server_url: String, token: String, team_id: String) -> Self {
        // Strip trailing slash
        let server_url = server_url.trim_end_matches('/').to_string();
        Self {
            server_url,
            token,
            team_id,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Send a post to a channel via the Mattermost REST API.
    async fn api_create_post(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        let url = format!("{}/api/v4/posts", self.server_url);

        let body = serde_json::json!({
            "channel_id": channel_id,
            "message": message,
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "mattermost".to_string(),
                message: format!("failed to create post: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Mattermost post failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Parse a Mattermost outgoing webhook payload into an `IncomingMessage`.
    ///
    /// Expected payload format:
    /// ```json
    /// {
    ///   "token": "webhook-token",
    ///   "team_id": "team-123",
    ///   "channel_id": "chan-456",
    ///   "channel_name": "general",
    ///   "user_id": "user-789",
    ///   "user_name": "alice",
    ///   "post_id": "post-abc",
    ///   "text": "Hello Mattermost",
    ///   "timestamp": 1705320000
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
        let post_id = payload
            .get("post_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let timestamp = payload
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
            .unwrap_or_else(Utc::now);

        let mut metadata = HashMap::new();
        if let Some(team_id) = payload.get("team_id").and_then(|v| v.as_str()) {
            metadata.insert(
                "team_id".to_string(),
                serde_json::Value::String(team_id.to_string()),
            );
        }
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
            platform: ChannelPlatform::Mattermost,
            platform_message_id: post_id.to_string(),
            is_group: true,
            metadata,
        })
    }

    /// Get the configured team ID.
    pub fn team_id(&self) -> &str {
        &self.team_id
    }
}

#[async_trait]
impl ChannelAdapter for MattermostAdapter {
    fn name(&self) -> &str {
        "mattermost"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Mattermost
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(
            server = %self.server_url,
            team = %self.team_id,
            "Mattermost adapter started"
        );
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Mattermost adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        self.api_create_post(channel_id, message).await
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

    fn make_adapter() -> MattermostAdapter {
        MattermostAdapter::new(
            "https://mattermost.example.com".to_string(),
            "bearer-token-123".to_string(),
            "team-abc".to_string(),
        )
    }

    #[test]
    fn test_mattermost_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "mattermost");
        assert_eq!(adapter.platform(), ChannelPlatform::Mattermost);
        assert_eq!(adapter.team_id(), "team-abc");
    }

    #[test]
    fn test_parse_webhook_payload() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "token": "webhook-token",
            "team_id": "team-123",
            "channel_id": "chan-456",
            "channel_name": "general",
            "user_id": "user-789",
            "user_name": "alice",
            "post_id": "post-abc",
            "text": "Hello Mattermost",
            "timestamp": 1705320000
        });

        let msg = adapter.parse_webhook_payload(&payload).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Mattermost);
        assert_eq!(msg.text, "Hello Mattermost");
        assert_eq!(msg.user_id, "user-789");
        assert_eq!(msg.display_name, "alice");
        assert_eq!(msg.channel_id, "chan-456");
        assert!(msg.is_group);
        assert_eq!(
            msg.metadata.get("channel_name").unwrap(),
            &serde_json::Value::String("general".to_string())
        );
    }

    #[test]
    fn test_parse_webhook_empty_text() {
        let adapter = make_adapter();
        let payload = serde_json::json!({
            "channel_id": "chan-456",
            "text": "",
            "user_id": "user-789"
        });
        assert!(adapter.parse_webhook_payload(&payload).is_none());
    }

    #[test]
    fn test_trailing_slash_stripped() {
        let adapter = MattermostAdapter::new(
            "https://mm.example.com/".to_string(),
            "token".to_string(),
            "team".to_string(),
        );
        assert_eq!(adapter.server_url, "https://mm.example.com");
    }

    #[tokio::test]
    async fn test_mattermost_start_stop() {
        let adapter = make_adapter();
        assert!(!adapter.status().connected);
        adapter.start().await.unwrap();
        assert!(adapter.status().connected);
        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
