//! # punch-channels
//!
//! Channel adapters for messaging platforms in the Punch Agent Combat System.
//!
//! Provides a unified [`ChannelAdapter`] trait that abstracts over different
//! messaging platforms (Telegram, Discord, Slack, etc.), a [`ChannelRouter`]
//! that maps platform users to fighters, and a [`ChannelBridge`] that manages
//! adapters and dispatches messages through the Ring.

pub mod adapters;
pub mod bridge;
pub mod router;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Identifies the messaging platform an adapter connects to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPlatform {
    Telegram,
    Discord,
    Slack,
    WhatsApp,
    Signal,
    Matrix,
    Email,
    Teams,
    Irc,
    Mastodon,
    Reddit,
    Twitch,
    GitHub,
    Line,
    WebChat,
    GoogleChat,
    Bluesky,
    LinkedIn,
    Sms,
    DingTalk,
    Feishu,
    Nostr,
    Mattermost,
    Zulip,
    RocketChat,
    Custom(String),
}

impl std::fmt::Display for ChannelPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Telegram => write!(f, "telegram"),
            Self::Discord => write!(f, "discord"),
            Self::Slack => write!(f, "slack"),
            Self::WhatsApp => write!(f, "whatsapp"),
            Self::Signal => write!(f, "signal"),
            Self::Matrix => write!(f, "matrix"),
            Self::Email => write!(f, "email"),
            Self::Teams => write!(f, "teams"),
            Self::Irc => write!(f, "irc"),
            Self::Mastodon => write!(f, "mastodon"),
            Self::Reddit => write!(f, "reddit"),
            Self::Twitch => write!(f, "twitch"),
            Self::GitHub => write!(f, "github"),
            Self::Line => write!(f, "line"),
            Self::WebChat => write!(f, "webchat"),
            Self::GoogleChat => write!(f, "google_chat"),
            Self::Bluesky => write!(f, "bluesky"),
            Self::LinkedIn => write!(f, "linkedin"),
            Self::Sms => write!(f, "sms"),
            Self::DingTalk => write!(f, "dingtalk"),
            Self::Feishu => write!(f, "feishu"),
            Self::Nostr => write!(f, "nostr"),
            Self::Mattermost => write!(f, "mattermost"),
            Self::Zulip => write!(f, "zulip"),
            Self::RocketChat => write!(f, "rocketchat"),
            Self::Custom(name) => write!(f, "custom({})", name),
        }
    }
}

/// A message received from an external messaging platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    /// The channel or conversation identifier on the platform.
    pub channel_id: String,
    /// The user identifier on the platform.
    pub user_id: String,
    /// The display name of the user.
    pub display_name: String,
    /// The text content of the message.
    pub text: String,
    /// When the message was sent.
    pub timestamp: DateTime<Utc>,
    /// Which platform the message originated from.
    pub platform: ChannelPlatform,
    /// Platform-specific message ID.
    pub platform_message_id: String,
    /// Whether this is from a group chat.
    #[serde(default)]
    pub is_group: bool,
    /// Arbitrary platform metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Status of a channel adapter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelStatus {
    /// Whether the adapter is currently running.
    pub connected: bool,
    /// When the adapter was started.
    pub started_at: Option<DateTime<Utc>>,
    /// Total messages received since start.
    pub messages_received: u64,
    /// Total messages sent since start.
    pub messages_sent: u64,
    /// Last error message (if any).
    pub last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over a messaging platform connection.
///
/// Each adapter receives incoming messages and can send responses back.
/// The lifecycle is: start() -> process messages -> stop().
#[async_trait]
pub trait ChannelAdapter: Send + Sync + 'static {
    /// Human-readable name for this adapter (e.g. "telegram", "discord").
    fn name(&self) -> &str;

    /// The platform this adapter connects to.
    fn platform(&self) -> ChannelPlatform;

    /// Start the adapter and begin listening for messages.
    async fn start(&self) -> PunchResult<()>;

    /// Stop the adapter and clean up resources.
    async fn stop(&self) -> PunchResult<()>;

    /// Send a text response to a specific channel/conversation.
    async fn send_response(&self, channel_id: &str, message: &str) -> PunchResult<()>;

    /// Get the current status of this adapter.
    fn status(&self) -> ChannelStatus {
        ChannelStatus::default()
    }
}

// ---------------------------------------------------------------------------
// ChannelBridge
// ---------------------------------------------------------------------------

/// Manages multiple [`ChannelAdapter`]s and routes messages between them.
pub struct ChannelBridge {
    adapters: RwLock<HashMap<String, Arc<dyn ChannelAdapter>>>,
}

impl ChannelBridge {
    /// Create a new, empty bridge.
    pub fn new() -> Self {
        Self {
            adapters: RwLock::new(HashMap::new()),
        }
    }

    /// Register an adapter with the bridge.
    pub async fn register(&self, adapter: Arc<dyn ChannelAdapter>) {
        let name = adapter.name().to_string();
        info!(adapter = %name, "registering channel adapter");
        self.adapters.write().await.insert(name, adapter);
    }

    /// Start all registered adapters.
    pub async fn start_all(&self) -> PunchResult<()> {
        let adapters = self.adapters.read().await;
        for (name, adapter) in adapters.iter() {
            info!(adapter = %name, "starting channel adapter");
            adapter.start().await.map_err(|e| PunchError::Channel {
                channel: name.clone(),
                message: format!("failed to start: {e}"),
            })?;
        }
        Ok(())
    }

    /// Stop all registered adapters.
    pub async fn stop_all(&self) -> PunchResult<()> {
        let adapters = self.adapters.read().await;
        for (name, adapter) in adapters.iter() {
            info!(adapter = %name, "stopping channel adapter");
            if let Err(e) = adapter.stop().await {
                warn!(adapter = %name, error = %e, "failed to stop adapter");
            }
        }
        Ok(())
    }

    /// Send a message through a specific adapter by name.
    pub async fn send_message(
        &self,
        adapter_name: &str,
        channel_id: &str,
        text: &str,
    ) -> PunchResult<()> {
        let adapters = self.adapters.read().await;
        let adapter = adapters
            .get(adapter_name)
            .ok_or_else(|| PunchError::Channel {
                channel: adapter_name.to_string(),
                message: "adapter not found".to_string(),
            })?;
        adapter.send_response(channel_id, text).await
    }

    /// List the names of all registered adapters.
    pub async fn list_adapters(&self) -> Vec<String> {
        self.adapters.read().await.keys().cloned().collect()
    }

    /// Get the status of all adapters.
    pub async fn adapter_statuses(&self) -> Vec<(String, ChannelPlatform, ChannelStatus)> {
        let adapters = self.adapters.read().await;
        adapters
            .iter()
            .map(|(name, adapter)| (name.clone(), adapter.platform(), adapter.status()))
            .collect()
    }
}

impl Default for ChannelBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Split a message into chunks of at most `max_len` characters,
/// preferring to split at newline boundaries.
pub fn split_message(text: &str, max_len: usize) -> Vec<&str> {
    if text.len() <= max_len {
        return vec![text];
    }
    let mut chunks = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining);
            break;
        }
        let split_at = remaining[..max_len].rfind('\n').unwrap_or(max_len);
        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk);
        remaining = rest
            .strip_prefix("\r\n")
            .or_else(|| rest.strip_prefix('\n'))
            .unwrap_or(rest);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_platform_display() {
        assert_eq!(ChannelPlatform::Telegram.to_string(), "telegram");
        assert_eq!(ChannelPlatform::Discord.to_string(), "discord");
        assert_eq!(ChannelPlatform::Slack.to_string(), "slack");
        assert_eq!(
            ChannelPlatform::Custom("irc".to_string()).to_string(),
            "custom(irc)"
        );
    }

    #[test]
    fn test_split_message_short() {
        assert_eq!(split_message("hello", 100), vec!["hello"]);
    }

    #[test]
    fn test_split_message_at_newlines() {
        let text = "line1\nline2\nline3";
        let chunks = split_message(text, 10);
        assert_eq!(chunks, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn test_incoming_message_serde() {
        let msg = IncomingMessage {
            channel_id: "ch1".to_string(),
            user_id: "user1".to_string(),
            display_name: "Alice".to_string(),
            text: "Hello!".to_string(),
            timestamp: Utc::now(),
            platform: ChannelPlatform::Telegram,
            platform_message_id: "123".to_string(),
            is_group: false,
            metadata: HashMap::new(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: IncomingMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.platform, ChannelPlatform::Telegram);
        assert_eq!(deserialized.user_id, "user1");
    }
}
