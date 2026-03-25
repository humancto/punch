//! Channel notification trait for proactive outbound messaging.
//!
//! This trait is defined in `punch-types` so that `punch-runtime` can use it
//! without depending on `punch-channels`. The `ChannelBridge` in `punch-channels`
//! (wrapped in the API layer) provides the concrete implementation.

use async_trait::async_trait;

use crate::error::PunchResult;

/// Trait for sending proactive notifications to external channels.
///
/// This allows the tool executor in `punch-runtime` to send messages to
/// Telegram, Slack, Discord, etc. without depending on `punch-channels`.
/// The API layer implements this trait and passes it as
/// `Arc<dyn ChannelNotifier>` into the tool execution context.
#[async_trait]
pub trait ChannelNotifier: Send + Sync {
    /// Send a message to a specific channel.
    ///
    /// - `adapter_name`: The channel adapter name (e.g., "telegram", "discord", "slack").
    /// - `chat_id`: The channel/conversation identifier on the platform.
    /// - `message`: The text message to send.
    async fn notify(&self, adapter_name: &str, chat_id: &str, message: &str) -> PunchResult<()>;

    /// List available channel adapters by name.
    async fn list_channels(&self) -> PunchResult<Vec<String>>;
}
