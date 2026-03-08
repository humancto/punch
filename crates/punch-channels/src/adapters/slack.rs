use async_trait::async_trait;

use punch_types::{PunchError, PunchResult};

use crate::{ChannelAdapter, IncomingMessage};

/// Slack channel adapter (stub).
pub struct SlackAdapter {
    /// Bot token for the Slack API.
    pub bot_token: String,
}

impl SlackAdapter {
    pub fn new(bot_token: String) -> Self {
        Self { bot_token }
    }
}

#[async_trait]
impl ChannelAdapter for SlackAdapter {
    fn name(&self) -> &str {
        "slack"
    }

    async fn connect(&self) -> PunchResult<()> {
        Err(PunchError::Channel {
            channel: "slack".to_string(),
            message: "slack adapter not yet implemented".to_string(),
        })
    }

    async fn send_message(&self, _channel_id: &str, _text: &str) -> PunchResult<()> {
        Err(PunchError::Channel {
            channel: "slack".to_string(),
            message: "slack adapter not yet implemented".to_string(),
        })
    }

    async fn receive_messages(&self) -> PunchResult<Vec<IncomingMessage>> {
        Err(PunchError::Channel {
            channel: "slack".to_string(),
            message: "slack adapter not yet implemented".to_string(),
        })
    }
}
