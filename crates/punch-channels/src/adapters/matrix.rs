//! Matrix protocol channel adapter.
//!
//! Sends messages via the Matrix client-server API and receives events
//! via the sync endpoint or webhook relay.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

/// Matrix protocol channel adapter.
///
/// Connects to a Matrix homeserver and sends messages via the client-server API.
/// Incoming events are parsed from Matrix sync responses or webhook relays.
pub struct MatrixAdapter {
    /// The homeserver base URL (e.g. `https://matrix.org`).
    homeserver_url: String,
    /// The access token for authentication.
    access_token: String,
    /// Room IDs the adapter is active in.
    #[allow(dead_code)]
    room_ids: Vec<String>,
    /// HTTP client for API calls.
    client: reqwest::Client,
    /// Whether the adapter is currently running.
    running: AtomicBool,
    /// When the adapter was started.
    started_at: RwLock<Option<DateTime<Utc>>>,
    /// Message counters.
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
    /// Transaction ID counter for idempotent sends.
    txn_counter: AtomicU64,
}

impl MatrixAdapter {
    /// Create a new Matrix adapter.
    ///
    /// `homeserver_url`: Base URL of the Matrix homeserver.
    /// `access_token`: Access token for authentication.
    /// `room_ids`: Room IDs to join and listen in.
    pub fn new(homeserver_url: String, access_token: String, room_ids: Vec<String>) -> Self {
        Self {
            homeserver_url,
            access_token,
            room_ids,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
            txn_counter: AtomicU64::new(0),
        }
    }

    /// Parse a Matrix room event into an `IncomingMessage`.
    ///
    /// Expected JSON format (Matrix m.room.message event):
    /// ```json
    /// {
    ///   "type": "m.room.message",
    ///   "event_id": "$abc123",
    ///   "room_id": "!room:matrix.org",
    ///   "sender": "@alice:matrix.org",
    ///   "origin_server_ts": 1700000000000,
    ///   "content": {
    ///     "msgtype": "m.text",
    ///     "body": "Hello!"
    ///   }
    /// }
    /// ```
    pub fn parse_matrix_event(&self, event: &serde_json::Value) -> Option<IncomingMessage> {
        let event_type = event["type"].as_str()?;
        if event_type != "m.room.message" {
            return None;
        }

        let content = event.get("content")?;
        let msgtype = content["msgtype"].as_str()?;
        if msgtype != "m.text" {
            return None;
        }

        let body = content["body"].as_str()?;
        if body.is_empty() {
            return None;
        }

        let sender = event["sender"].as_str()?;
        let room_id = event["room_id"].as_str()?;
        let event_id = event["event_id"].as_str().unwrap_or("unknown");

        // Matrix timestamps are milliseconds since epoch
        let timestamp = event["origin_server_ts"]
            .as_i64()
            .and_then(DateTime::from_timestamp_millis)
            .unwrap_or_else(Utc::now);

        // Extract a display name from the sender (e.g. "@alice:matrix.org" -> "alice")
        let display_name = sender
            .strip_prefix('@')
            .and_then(|s| s.split(':').next())
            .unwrap_or(sender);

        self.messages_received.fetch_add(1, Ordering::Relaxed);

        Some(IncomingMessage {
            channel_id: room_id.to_string(),
            user_id: sender.to_string(),
            display_name: display_name.to_string(),
            text: body.to_string(),
            timestamp,
            platform: ChannelPlatform::Matrix,
            platform_message_id: event_id.to_string(),
            is_group: true, // Matrix rooms are always group-like
            metadata: HashMap::new(),
        })
    }

    /// Join a Matrix room by room ID.
    async fn join_room(&self, room_id: &str) -> PunchResult<()> {
        let url = format!(
            "{}/_matrix/client/v3/join/{}",
            self.homeserver_url,
            urlencoding::encode(room_id)
        );

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "matrix".to_string(),
                message: format!("failed to join room {room_id}: {e}"),
            })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!("Matrix join room {room_id} failed: {body}");
        }

        Ok(())
    }

    /// Send a text message to a Matrix room via the client-server API.
    async fn api_send_message(&self, room_id: &str, text: &str) -> PunchResult<()> {
        let txn_id = self.txn_counter.fetch_add(1, Ordering::Relaxed);

        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/txn{}",
            self.homeserver_url,
            urlencoding::encode(room_id),
            txn_id
        );

        let body = serde_json::json!({
            "msgtype": "m.text",
            "body": text
        });

        let resp = self
            .client
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Channel {
                channel: "matrix".to_string(),
                message: format!("failed to send message to {room_id}: {e}"),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            warn!("Matrix send message failed ({status}): {body_text}");
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl ChannelAdapter for MatrixAdapter {
    fn name(&self) -> &str {
        "matrix"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Matrix
    }

    async fn start(&self) -> PunchResult<()> {
        // Join configured rooms
        for room_id in &self.room_ids {
            self.join_room(room_id).await?;
        }

        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(
            homeserver = %self.homeserver_url,
            rooms = self.room_ids.len(),
            "Matrix adapter started"
        );
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Matrix adapter stopped");
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

    fn make_adapter() -> MatrixAdapter {
        MatrixAdapter::new(
            "https://matrix.example.org".to_string(),
            "syt_test_token".to_string(),
            vec!["!room1:example.org".to_string()],
        )
    }

    #[test]
    fn test_matrix_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "matrix");
        assert_eq!(adapter.platform(), ChannelPlatform::Matrix);
    }

    #[test]
    fn test_parse_matrix_event_basic() {
        let adapter = make_adapter();

        let event = serde_json::json!({
            "type": "m.room.message",
            "event_id": "$evt123",
            "room_id": "!room1:example.org",
            "sender": "@alice:example.org",
            "origin_server_ts": 1700000000000i64,
            "content": {
                "msgtype": "m.text",
                "body": "Hello from Matrix!"
            }
        });

        let msg = adapter.parse_matrix_event(&event).unwrap();
        assert_eq!(msg.platform, ChannelPlatform::Matrix);
        assert_eq!(msg.user_id, "@alice:example.org");
        assert_eq!(msg.display_name, "alice");
        assert_eq!(msg.channel_id, "!room1:example.org");
        assert_eq!(msg.text, "Hello from Matrix!");
        assert_eq!(msg.platform_message_id, "$evt123");
        assert!(msg.is_group);
    }

    #[test]
    fn test_parse_matrix_event_non_text() {
        let adapter = make_adapter();

        let event = serde_json::json!({
            "type": "m.room.message",
            "event_id": "$evt456",
            "room_id": "!room1:example.org",
            "sender": "@bob:example.org",
            "origin_server_ts": 1700000000000i64,
            "content": {
                "msgtype": "m.image",
                "body": "photo.jpg",
                "url": "mxc://example.org/abc"
            }
        });

        let msg = adapter.parse_matrix_event(&event);
        assert!(msg.is_none());
    }

    #[test]
    fn test_parse_matrix_event_wrong_type() {
        let adapter = make_adapter();

        let event = serde_json::json!({
            "type": "m.room.member",
            "event_id": "$evt789",
            "room_id": "!room1:example.org",
            "sender": "@alice:example.org",
            "origin_server_ts": 1700000000000i64,
            "content": {
                "membership": "join"
            }
        });

        let msg = adapter.parse_matrix_event(&event);
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_matrix_adapter_start_stop() {
        // Use an adapter with no rooms to avoid real HTTP calls during start
        let adapter = MatrixAdapter::new(
            "https://matrix.example.org".to_string(),
            "syt_test".to_string(),
            vec![],
        );

        assert!(!adapter.status().connected);

        adapter.start().await.unwrap();
        assert!(adapter.status().connected);

        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
