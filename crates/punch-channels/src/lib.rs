//! # punch-channels
//!
//! Channel adapters for messaging platforms in the Punch Agent Combat System.
//!
//! Provides a unified [`ChannelAdapter`] trait that abstracts over different
//! messaging platforms (Telegram, Discord, Slack, etc.) and a [`ChannelBridge`]
//! that manages multiple adapters and routes messages between them.

pub mod adapters;

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
    /// The text content of the message.
    pub text: String,
    /// When the message was sent.
    pub timestamp: DateTime<Utc>,
    /// Which platform the message originated from.
    pub platform: ChannelPlatform,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over a messaging platform connection.
#[async_trait]
pub trait ChannelAdapter: Send + Sync + 'static {
    /// Human-readable name for this adapter (e.g. "telegram", "discord").
    fn name(&self) -> &str;

    /// Establish a connection to the messaging platform.
    async fn connect(&self) -> PunchResult<()>;

    /// Send a text message to a specific channel/conversation.
    async fn send_message(&self, channel_id: &str, text: &str) -> PunchResult<()>;

    /// Poll or receive pending incoming messages.
    async fn receive_messages(&self) -> PunchResult<Vec<IncomingMessage>>;
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

    /// Connect all registered adapters.
    pub async fn connect_all(&self) -> PunchResult<()> {
        let adapters = self.adapters.read().await;
        for (name, adapter) in adapters.iter() {
            info!(adapter = %name, "connecting channel adapter");
            adapter.connect().await.map_err(|e| PunchError::Channel {
                channel: name.clone(),
                message: format!("failed to connect: {e}"),
            })?;
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
        adapter.send_message(channel_id, text).await
    }

    /// Receive messages from all connected adapters.
    pub async fn receive_all(&self) -> PunchResult<Vec<IncomingMessage>> {
        let adapters = self.adapters.read().await;
        let mut all_messages = Vec::new();
        for (name, adapter) in adapters.iter() {
            match adapter.receive_messages().await {
                Ok(messages) => all_messages.extend(messages),
                Err(e) => {
                    warn!(adapter = %name, error = %e, "failed to receive messages");
                }
            }
        }
        Ok(all_messages)
    }

    /// List the names of all registered adapters.
    pub async fn list_adapters(&self) -> Vec<String> {
        self.adapters.read().await.keys().cloned().collect()
    }
}

impl Default for ChannelBridge {
    fn default() -> Self {
        Self::new()
    }
}
