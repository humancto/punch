//! Bluesky / AT Protocol adapter.
//!
//! Authenticates via `com.atproto.server.createSession`, posts via
//! `com.atproto.repo.createRecord`, and polls notifications for mentions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

const BLUESKY_API_BASE: &str = "https://bsky.social/xrpc";

/// Session data returned by `com.atproto.server.createSession`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueskySession {
    pub did: String,
    pub handle: String,
    #[serde(rename = "accessJwt")]
    pub access_jwt: String,
    #[serde(rename = "refreshJwt")]
    pub refresh_jwt: String,
}

/// Bluesky adapter using the AT Protocol.
pub struct BlueskyAdapter {
    /// Account identifier (handle or email).
    identifier: String,
    /// App password.
    password: String,
    /// PDS host (defaults to bsky.social).
    pds_host: String,
    /// Active session (set after login).
    session: RwLock<Option<BlueskySession>>,
    /// HTTP client.
    client: reqwest::Client,
    running: AtomicBool,
    started_at: RwLock<Option<DateTime<Utc>>>,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl BlueskyAdapter {
    /// Create a new Bluesky adapter.
    pub fn new(identifier: String, password: String) -> Self {
        Self {
            identifier,
            password,
            pds_host: BLUESKY_API_BASE.to_string(),
            session: RwLock::new(None),
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Create a new Bluesky adapter with a custom PDS host.
    pub fn with_pds_host(identifier: String, password: String, pds_host: String) -> Self {
        Self {
            identifier,
            password,
            pds_host,
            session: RwLock::new(None),
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Authenticate with the PDS and obtain a session.
    pub async fn create_session(&self) -> PunchResult<BlueskySession> {
        let url = format!("{}/com.atproto.server.createSession", self.pds_host);

        let body = serde_json::json!({
            "identifier": self.identifier,
            "password": self.password,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "bluesky".to_string(),
                message: format!("failed to create session: {e}"),
            })?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(PunchError::Channel {
                channel: "bluesky".to_string(),
                message: format!("auth failed: {err_text}"),
            });
        }

        let session: BlueskySession = resp.json().await.map_err(|e| PunchError::Channel {
            channel: "bluesky".to_string(),
            message: format!("failed to parse session: {e}"),
        })?;

        *self.session.write().await = Some(session.clone());
        Ok(session)
    }

    /// Post a new text record (skeet) via `com.atproto.repo.createRecord`.
    pub async fn create_post(&self, text: &str) -> PunchResult<()> {
        let session = self.session.read().await;
        let session = session.as_ref().ok_or_else(|| PunchError::Channel {
            channel: "bluesky".to_string(),
            message: "not authenticated — call create_session first".to_string(),
        })?;

        let url = format!("{}/com.atproto.repo.createRecord", self.pds_host);
        let now = Utc::now().to_rfc3339();

        let body = serde_json::json!({
            "repo": session.did,
            "collection": "app.bsky.feed.post",
            "record": {
                "$type": "app.bsky.feed.post",
                "text": text,
                "createdAt": now,
            }
        });

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", session.access_jwt))
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "bluesky".to_string(),
                message: format!("failed to create post: {e}"),
            })?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            warn!("Bluesky createRecord failed: {err_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Poll notifications for mentions.
    pub async fn poll_notifications(&self) -> PunchResult<Vec<IncomingMessage>> {
        let session = self.session.read().await;
        let session = session.as_ref().ok_or_else(|| PunchError::Channel {
            channel: "bluesky".to_string(),
            message: "not authenticated".to_string(),
        })?;

        let url = format!(
            "{}/app.bsky.notification.listNotifications?limit=20",
            self.pds_host
        );

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", session.access_jwt))
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "bluesky".to_string(),
                message: format!("failed to list notifications: {e}"),
            })?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(PunchError::Channel {
                channel: "bluesky".to_string(),
                message: format!("notifications failed: {err_text}"),
            });
        }

        let data: serde_json::Value = resp.json().await.map_err(|e| PunchError::Channel {
            channel: "bluesky".to_string(),
            message: format!("parse error: {e}"),
        })?;

        let mut messages = Vec::new();
        if let Some(notifications) = data.get("notifications").and_then(|v| v.as_array()) {
            for notif in notifications {
                if let Some(msg) = self.parse_notification(notif) {
                    self.messages_received.fetch_add(1, Ordering::Relaxed);
                    messages.push(msg);
                }
            }
        }

        Ok(messages)
    }

    fn parse_notification(&self, notif: &serde_json::Value) -> Option<IncomingMessage> {
        let reason = notif.get("reason")?.as_str()?;
        if reason != "mention" && reason != "reply" {
            return None;
        }

        let author = notif.get("author")?;
        let did = author.get("did")?.as_str()?;
        let handle = author
            .get("handle")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let record = notif.get("record")?;
        let text = record.get("text")?.as_str()?;
        if text.is_empty() {
            return None;
        }

        let uri = notif.get("uri")?.as_str()?;
        let indexed_at = notif
            .get("indexedAt")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let mut metadata = HashMap::new();
        metadata.insert(
            "reason".to_string(),
            serde_json::Value::String(reason.to_string()),
        );

        Some(IncomingMessage {
            channel_id: did.to_string(),
            user_id: did.to_string(),
            display_name: handle.to_string(),
            text: text.to_string(),
            timestamp: indexed_at,
            platform: ChannelPlatform::Bluesky,
            platform_message_id: uri.to_string(),
            is_group: false,
            metadata,
        })
    }
}

#[async_trait]
impl ChannelAdapter for BlueskyAdapter {
    fn name(&self) -> &str {
        "bluesky"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Bluesky
    }

    async fn start(&self) -> PunchResult<()> {
        self.create_session().await?;
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(handle = %self.identifier, "Bluesky adapter started");
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        *self.session.write().await = None;
        info!("Bluesky adapter stopped");
        Ok(())
    }

    async fn send_response(&self, _channel_id: &str, message: &str) -> PunchResult<()> {
        self.create_post(message).await
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

    fn make_adapter() -> BlueskyAdapter {
        BlueskyAdapter::new("test.bsky.social".to_string(), "app-password".to_string())
    }

    #[test]
    fn test_bluesky_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "bluesky");
        assert_eq!(adapter.platform(), ChannelPlatform::Bluesky);
    }

    #[test]
    fn test_parse_mention_notification() {
        let adapter = make_adapter();
        // Simulate having a session
        let notif = serde_json::json!({
            "uri": "at://did:plc:abc/app.bsky.feed.post/123",
            "reason": "mention",
            "author": {
                "did": "did:plc:abc",
                "handle": "alice.bsky.social"
            },
            "record": {
                "text": "@bot help me",
                "createdAt": "2024-01-15T12:00:00Z"
            },
            "indexedAt": "2024-01-15T12:00:01Z"
        });

        let msg = adapter.parse_notification(&notif).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Bluesky);
        assert_eq!(msg.user_id, "did:plc:abc");
        assert_eq!(msg.display_name, "alice.bsky.social");
        assert_eq!(msg.text, "@bot help me");
    }

    #[test]
    fn test_parse_notification_ignores_likes() {
        let adapter = make_adapter();
        let notif = serde_json::json!({
            "uri": "at://did:plc:abc/app.bsky.feed.like/123",
            "reason": "like",
            "author": { "did": "did:plc:abc", "handle": "alice.bsky.social" },
            "record": { "subject": { "uri": "at://..." } }
        });
        assert!(adapter.parse_notification(&notif).is_none());
    }

    #[test]
    fn test_with_custom_pds_host() {
        let adapter = BlueskyAdapter::with_pds_host(
            "test.bsky.social".to_string(),
            "pass".to_string(),
            "https://custom.pds.example/xrpc".to_string(),
        );
        assert_eq!(adapter.pds_host, "https://custom.pds.example/xrpc");
    }
}
