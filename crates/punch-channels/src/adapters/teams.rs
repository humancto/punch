//! Microsoft Teams channel adapter via Bot Framework.
//!
//! Receives activities from Bot Framework webhook and sends responses
//! via the Bot Framework REST API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const BOT_FRAMEWORK_AUTH_URL: &str =
    "https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token";

/// Microsoft Teams Bot Framework adapter.
///
/// Receives: Bot Framework Activity JSON via POST to the Arena webhook endpoint.
/// Sends: responses via Bot Framework REST API.
pub struct TeamsAdapter {
    /// The bot's Microsoft App ID.
    app_id: String,
    /// The bot's app password (client secret).
    app_password: String,
    /// Azure AD tenant ID (for multi-tenant bots, use "botframework.com").
    #[allow(dead_code)]
    tenant_id: String,
    /// HTTP client for API calls.
    client: reqwest::Client,
    /// Cached OAuth2 bearer token.
    bearer_token: RwLock<Option<String>>,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl TeamsAdapter {
    /// Create a new Teams adapter.
    ///
    /// `app_id`: Microsoft App ID for the bot.
    /// `app_password`: Client secret for the bot.
    /// `tenant_id`: Azure AD tenant ID.
    pub fn new(app_id: String, app_password: String, tenant_id: String) -> Self {
        Self {
            app_id,
            app_password,
            tenant_id,
            client: reqwest::Client::new(),
            bearer_token: RwLock::new(None),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Parse a Bot Framework Activity into an `IncomingMessage`.
    ///
    /// Expected JSON format (Bot Framework Activity):
    /// ```json
    /// {
    ///   "type": "message",
    ///   "id": "activity-id-123",
    ///   "timestamp": "2024-01-01T00:00:00Z",
    ///   "channelId": "msteams",
    ///   "from": { "id": "user-aad-id", "name": "Alice" },
    ///   "conversation": { "id": "conv-id", "isGroup": true },
    ///   "text": "Hello!",
    ///   "serviceUrl": "https://smba.trafficmanager.net/teams/"
    /// }
    /// ```
    pub fn parse_teams_activity(&self, activity: &serde_json::Value) -> Option<IncomingMessage> {
        let activity_type = activity["type"].as_str()?;
        if activity_type != "message" {
            return None;
        }

        let text = activity["text"].as_str().unwrap_or("");
        if text.is_empty() {
            return None;
        }

        let from = activity.get("from")?;
        let user_id = from["id"].as_str()?;
        let display_name = from["name"].as_str().unwrap_or("Unknown");

        let conversation = activity.get("conversation")?;
        let conversation_id = conversation["id"].as_str()?;
        let is_group = conversation["isGroup"].as_bool().unwrap_or(false);

        let activity_id = activity["id"].as_str().unwrap_or("unknown");

        let timestamp = activity["timestamp"]
            .as_str()
            .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        // Store service URL in metadata for reply routing
        let mut metadata = HashMap::new();
        if let Some(service_url) = activity["serviceUrl"].as_str() {
            metadata.insert(
                "service_url".to_string(),
                serde_json::Value::String(service_url.to_string()),
            );
        }

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: conversation_id.to_string(),
            user_id: user_id.to_string(),
            display_name: display_name.to_string(),
            text: text.to_string(),
            timestamp,
            platform: ChannelPlatform::Teams,
            platform_message_id: activity_id.to_string(),
            is_group,
            metadata,
        })
    }

    /// Acquire an OAuth2 token for the Bot Framework API.
    async fn acquire_token(&self) -> PunchResult<String> {
        // Check cached token
        if let Some(ref token) = *self.bearer_token.read().await {
            return Ok(token.clone());
        }

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", &self.app_id),
            ("client_secret", &self.app_password),
            ("scope", "https://api.botframework.com/.default"),
        ];

        let resp: serde_json::Value = self
            .client
            .post(BOT_FRAMEWORK_AUTH_URL)
            .form(&params)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "teams".to_string(),
                message: format!("OAuth2 token request failed: {e}"),
            })?
            .json()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "teams".to_string(),
                message: format!("OAuth2 token parse failed: {e}"),
            })?;

        let token = resp["access_token"]
            .as_str()
            .ok_or_else(|| PunchError::Channel {
                channel: "teams".to_string(),
                message: "no access_token in OAuth2 response".to_string(),
            })?
            .to_string();

        *self.bearer_token.write().await = Some(token.clone());
        Ok(token)
    }

    /// Send a reply activity via the Bot Framework REST API.
    ///
    /// `service_url`: The service URL from the incoming activity.
    /// `conversation_id`: The conversation to reply to.
    /// `text`: The message text.
    async fn api_send_message(
        &self,
        service_url: &str,
        conversation_id: &str,
        text: &str,
    ) -> PunchResult<()> {
        let token = self.acquire_token().await?;

        let url = format!(
            "{}v3/conversations/{}/activities",
            if service_url.ends_with('/') {
                service_url.to_string()
            } else {
                format!("{service_url}/")
            },
            conversation_id
        );

        let body = serde_json::json!({
            "type": "message",
            "text": text
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "teams".to_string(),
                message: format!("failed to send activity: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Teams send activity failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for TeamsAdapter {
    fn name(&self) -> &str {
        "teams"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Teams
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!("Teams adapter started (Bot Framework webhook mode)");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        *self.bearer_token.write().await = None;
        info!("Teams adapter stopped");
        Ok(())
    }

    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()> {
        // channel_id is expected as "service_url|conversation_id"
        // If no pipe separator, use the default Teams service URL
        let (service_url, conversation_id) = if let Some(idx) = channel_id.find('|') {
            (&channel_id[..idx], &channel_id[idx + 1..])
        } else {
            ("https://smba.trafficmanager.net/teams/", channel_id)
        };

        self.api_send_message(service_url, conversation_id, message)
            .await
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

    fn make_adapter() -> TeamsAdapter {
        TeamsAdapter::new(
            "app-id-123".to_string(),
            "app-password".to_string(),
            "tenant-id".to_string(),
        )
    }

    #[test]
    fn test_teams_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "teams");
        assert_eq!(adapter.platform(), ChannelPlatform::Teams);
    }

    #[test]
    fn test_parse_teams_activity_basic() {
        let adapter = make_adapter();

        let activity = serde_json::json!({
            "type": "message",
            "id": "activity-001",
            "timestamp": "2024-01-15T10:30:00Z",
            "channelId": "msteams",
            "from": { "id": "user-aad-123", "name": "Alice" },
            "conversation": { "id": "conv-456", "isGroup": false },
            "text": "Hello Teams!",
            "serviceUrl": "https://smba.trafficmanager.net/teams/"
        });

        let msg = adapter.parse_teams_activity(&activity).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Teams);
        assert_eq!(msg.user_id, "user-aad-123");
        assert_eq!(msg.display_name, "Alice");
        assert_eq!(msg.channel_id, "conv-456");
        assert_eq!(msg.text, "Hello Teams!");
        assert!(!msg.is_group);
        assert_eq!(
            msg.metadata.get("service_url").and_then(|v| v.as_str()),
            Some("https://smba.trafficmanager.net/teams/")
        );
    }

    #[test]
    fn test_parse_teams_activity_non_message() {
        let adapter = make_adapter();

        let activity = serde_json::json!({
            "type": "conversationUpdate",
            "id": "activity-002",
            "from": { "id": "user-123", "name": "Bot" },
            "conversation": { "id": "conv-789" }
        });

        let msg = adapter.parse_teams_activity(&activity);
        assert!(msg.is_none());
    }

    #[test]
    fn test_parse_teams_activity_group() {
        let adapter = make_adapter();

        let activity = serde_json::json!({
            "type": "message",
            "id": "activity-003",
            "from": { "id": "user-111", "name": "Bob" },
            "conversation": { "id": "conv-group-1", "isGroup": true },
            "text": "Group message"
        });

        let msg = adapter.parse_teams_activity(&activity).unwrap();
        assert!(msg.is_group);
    }

    #[tokio::test]
    async fn test_teams_adapter_start_stop() {
        let adapter = make_adapter();

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        assert!(adapter.status().connected);

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
