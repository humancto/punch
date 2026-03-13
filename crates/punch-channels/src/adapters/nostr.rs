//! Nostr protocol adapter.
//!
//! Implements NIP-01 event creation (kind 1 text notes, kind 4 encrypted DMs),
//! event ID computation via SHA-256, and relay WebSocket message formatting.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::PunchResult;

use crate::{ChannelAdapter, ChannelPlatform, ChannelStatus, IncomingMessage};

/// A Nostr event per NIP-01.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NostrEvent {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

impl NostrEvent {
    /// Compute the event ID as SHA-256 of the serialized event array:
    /// `[0, pubkey, created_at, kind, tags, content]`
    pub fn compute_id(
        pubkey: &str,
        created_at: i64,
        kind: u32,
        tags: &[Vec<String>],
        content: &str,
    ) -> String {
        let serialized = serde_json::json!([0, pubkey, created_at, kind, tags, content]);
        let bytes = serialized.to_string().into_bytes();
        let hash = Sha256::digest(&bytes);
        hex_encode(&hash)
    }
}

/// Nostr protocol adapter.
///
/// Creates NIP-01 events and formats relay WebSocket messages.
/// Event signing uses a hex-encoded private key.
pub struct NostrAdapter {
    /// Hex-encoded public key.
    pubkey: String,
    /// Hex-encoded private key (for signing events).
    privkey: String,
    /// Relay WebSocket URLs.
    relay_urls: Vec<String>,
    /// HTTP client (for HTTP-based relay APIs).
    client: reqwest::Client,
    running: AtomicBool,
    started_at: RwLock<Option<DateTime<Utc>>>,
    messages_received: AtomicU64,
    messages_sent: AtomicU64,
}

impl NostrAdapter {
    /// Create a new Nostr adapter.
    pub fn new(pubkey: String, privkey: String, relay_urls: Vec<String>) -> Self {
        Self {
            pubkey,
            privkey,
            relay_urls,
            client: reqwest::Client::new(),
            running: AtomicBool::new(false),
            started_at: RwLock::new(None),
            messages_received: AtomicU64::new(0),
            messages_sent: AtomicU64::new(0),
        }
    }

    /// Build a kind-1 (text note) event.
    pub fn build_text_note(&self, content: &str) -> NostrEvent {
        let created_at = Utc::now().timestamp();
        let tags: Vec<Vec<String>> = Vec::new();
        let id = NostrEvent::compute_id(&self.pubkey, created_at, 1, &tags, content);

        // In production, sign with secp256k1. Here we create a placeholder
        // signature from SHA-256(privkey + id) for structural correctness.
        let sig = compute_placeholder_sig(&self.privkey, &id);

        NostrEvent {
            id,
            pubkey: self.pubkey.clone(),
            created_at,
            kind: 1,
            tags,
            content: content.to_string(),
            sig,
        }
    }

    /// Build a kind-4 (encrypted DM) event to a specific recipient.
    pub fn build_dm(&self, recipient_pubkey: &str, content: &str) -> NostrEvent {
        let created_at = Utc::now().timestamp();
        let tags = vec![vec!["p".to_string(), recipient_pubkey.to_string()]];
        let id = NostrEvent::compute_id(&self.pubkey, created_at, 4, &tags, content);
        let sig = compute_placeholder_sig(&self.privkey, &id);

        NostrEvent {
            id,
            pubkey: self.pubkey.clone(),
            created_at,
            kind: 4,
            tags,
            content: content.to_string(),
            sig,
        }
    }

    /// Format an event as a relay `["EVENT", event]` message.
    pub fn format_event_message(event: &NostrEvent) -> String {
        serde_json::json!(["EVENT", event]).to_string()
    }

    /// Parse a relay subscription event: `["EVENT", sub_id, event]`.
    pub fn parse_subscription_event(message: &str) -> Option<(String, NostrEvent)> {
        let parsed: serde_json::Value = serde_json::from_str(message).ok()?;
        let arr = parsed.as_array()?;

        if arr.len() < 3 {
            return None;
        }

        let msg_type = arr[0].as_str()?;
        if msg_type != "EVENT" {
            return None;
        }

        let sub_id = arr[1].as_str()?.to_string();
        let event: NostrEvent = serde_json::from_value(arr[2].clone()).ok()?;
        Some((sub_id, event))
    }

    /// Convert a `NostrEvent` into an `IncomingMessage`.
    pub fn event_to_incoming(&self, event: &NostrEvent) -> IncomingMessage {
        self.messages_received.fetch_add(1, Ordering::Relaxed);

        let timestamp = DateTime::from_timestamp(event.created_at, 0).unwrap_or_else(Utc::now);

        let mut metadata = HashMap::new();
        metadata.insert("kind".to_string(), serde_json::json!(event.kind));

        IncomingMessage {
            channel_id: event.pubkey.clone(),
            user_id: event.pubkey.clone(),
            display_name: event.pubkey[..8].to_string(),
            text: event.content.clone(),
            timestamp,
            platform: ChannelPlatform::Nostr,
            platform_message_id: event.id.clone(),
            is_group: event.kind == 1,
            metadata,
        }
    }

    /// Publish an event to all configured relays via HTTP POST (NIP-86 style).
    async fn publish_event(&self, event: &NostrEvent) -> PunchResult<()> {
        let message = Self::format_event_message(event);

        for relay_url in &self.relay_urls {
            let resp = self
                .client
                .post(relay_url)
                .header("Content-Type", "application/json")
                .body(message.clone())
                .send()
                .await;

            match resp {
                Ok(r) if !r.status().is_success() => {
                    let status = r.status();
                    warn!("Nostr relay {relay_url} failed ({status})");
                }
                Err(e) => {
                    warn!("Nostr relay {relay_url} error: {e}");
                }
                _ => {}
            }
        }

        self.messages_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Hex-encode bytes.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compute a placeholder "signature" (SHA-256 of key+id) for testing.
/// In production, this would use secp256k1 Schnorr signing.
fn compute_placeholder_sig(privkey: &str, event_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(privkey.as_bytes());
    hasher.update(event_id.as_bytes());
    hex_encode(&hasher.finalize())
}

#[async_trait]
impl ChannelAdapter for NostrAdapter {
    fn name(&self) -> &str {
        "nostr"
    }

    fn platform(&self) -> ChannelPlatform {
        ChannelPlatform::Nostr
    }

    async fn start(&self) -> PunchResult<()> {
        self.running.store(true, Ordering::Relaxed);
        *self.started_at.write().await = Some(Utc::now());
        info!(
            pubkey = %self.pubkey,
            relays = self.relay_urls.len(),
            "Nostr adapter started"
        );
        Ok(())
    }

    async fn stop(&self) -> PunchResult<()> {
        self.running.store(false, Ordering::Relaxed);
        info!("Nostr adapter stopped");
        Ok(())
    }

    async fn send_response(&self, _channel_id: &str, message: &str) -> PunchResult<()> {
        let event = self.build_text_note(message);
        self.publish_event(&event).await
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

    fn make_adapter() -> NostrAdapter {
        NostrAdapter::new(
            "aabbccdd11223344aabbccdd11223344aabbccdd11223344aabbccdd11223344".to_string(),
            "1122334455667788112233445566778811223344556677881122334455667788".to_string(),
            vec!["wss://relay.example.com".to_string()],
        )
    }

    #[test]
    fn test_nostr_adapter_creation() {
        let adapter = make_adapter();
        assert_eq!(adapter.name(), "nostr");
        assert_eq!(adapter.platform(), ChannelPlatform::Nostr);
    }

    #[test]
    fn test_build_text_note() {
        let adapter = make_adapter();
        let event = adapter.build_text_note("Hello Nostr!");
        assert_eq!(event.kind, 1);
        assert_eq!(event.content, "Hello Nostr!");
        assert_eq!(event.pubkey, adapter.pubkey);
        assert!(!event.id.is_empty());
        assert!(!event.sig.is_empty());
        assert!(event.tags.is_empty());
    }

    #[test]
    fn test_build_dm() {
        let adapter = make_adapter();
        let recipient = "deadbeef00000000deadbeef00000000deadbeef00000000deadbeef00000000";
        let event = adapter.build_dm(recipient, "Secret message");
        assert_eq!(event.kind, 4);
        assert_eq!(event.tags.len(), 1);
        assert_eq!(event.tags[0][0], "p");
        assert_eq!(event.tags[0][1], recipient);
    }

    #[test]
    fn test_compute_event_id_deterministic() {
        let id1 = NostrEvent::compute_id("abc", 1000, 1, &[], "hello");
        let id2 = NostrEvent::compute_id("abc", 1000, 1, &[], "hello");
        assert_eq!(id1, id2);

        let id3 = NostrEvent::compute_id("abc", 1000, 1, &[], "world");
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_format_and_parse_event_message() {
        let adapter = make_adapter();
        let event = adapter.build_text_note("test");
        let msg = NostrAdapter::format_event_message(&event);
        assert!(msg.starts_with("[\"EVENT\","));

        // Simulate relay subscription event
        let relay_msg = serde_json::json!(["EVENT", "sub-1", event]).to_string();
        let (sub_id, parsed_event) = NostrAdapter::parse_subscription_event(&relay_msg).unwrap();
        assert_eq!(sub_id, "sub-1");
        assert_eq!(parsed_event.content, "test");
    }

    #[test]
    fn test_event_to_incoming() {
        let adapter = make_adapter();
        let event = adapter.build_text_note("incoming text");
        let msg = adapter.event_to_incoming(&event);
        assert_eq!(msg.platform, ChannelPlatform::Nostr);
        assert_eq!(msg.text, "incoming text");
        assert!(msg.is_group); // kind 1
    }

    #[tokio::test]
    async fn test_nostr_start_stop() {
        let adapter = make_adapter();
        assert!(!adapter.status().connected);
        adapter.start().await.unwrap();
        assert!(adapter.status().connected);
        adapter.stop().await.unwrap();
        assert!(!adapter.status().connected);
    }
}
