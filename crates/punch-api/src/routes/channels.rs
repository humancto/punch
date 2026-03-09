//! Webhook endpoints for channel adapters.
//!
//! These endpoints receive incoming messages from external platforms
//! (Discord, Telegram, Slack) and route them to fighters via the Ring.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use punch_channels::ChannelAdapter;
use serde::Serialize;
use tracing::{info, warn};

use punch_channels::adapters::{DiscordAdapter, SlackAdapter, TelegramAdapter};
use punch_channels::bridge::{self, ChannelBridgeHandle};
use punch_channels::router::ChannelRouter;
use punch_channels::ChannelPlatform;
use punch_types::FighterId;

use crate::AppState;

/// Build the channel webhook routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/channels/discord/webhook",
            post(discord_webhook),
        )
        .route(
            "/api/channels/telegram/webhook",
            post(telegram_webhook),
        )
        .route(
            "/api/channels/slack/events",
            post(slack_events),
        )
}

#[derive(Serialize)]
struct WebhookResponse {
    ok: bool,
    response: Option<String>,
    error: Option<String>,
}

/// Ring-backed implementation of ChannelBridgeHandle.
struct RingBridgeHandle {
    ring: std::sync::Arc<punch_kernel::Ring>,
}

#[async_trait::async_trait]
impl ChannelBridgeHandle for RingBridgeHandle {
    async fn send_message(
        &self,
        fighter_id: FighterId,
        message: &str,
    ) -> Result<String, String> {
        match self
            .ring
            .send_message(&fighter_id, message.to_string())
            .await
        {
            Ok(result) => Ok(result.response),
            Err(e) => Err(format!("Fighter error: {e}")),
        }
    }

    async fn find_fighter_by_name(&self, name: &str) -> Result<Option<FighterId>, String> {
        let fighters = self.ring.list_fighters();
        Ok(fighters
            .iter()
            .find(|(_, manifest, _)| manifest.name == name)
            .map(|(id, _, _)| *id))
    }

    async fn list_fighters(&self) -> Result<Vec<(FighterId, String)>, String> {
        let fighters = self.ring.list_fighters();
        Ok(fighters
            .iter()
            .map(|(id, manifest, _)| (*id, manifest.name.clone()))
            .collect())
    }

    async fn spawn_fighter_by_name(&self, _manifest_name: &str) -> Result<FighterId, String> {
        // For webhook mode, we don't auto-spawn fighters.
        // Fighters should be pre-created via the API or CLI.
        Err("Auto-spawn not available in webhook mode. Create a fighter first via `punch fighter spawn`.".to_string())
    }
}

// ---------------------------------------------------------------------------
// Discord webhook handler
// ---------------------------------------------------------------------------

/// POST /api/channels/discord/webhook — receive Discord messages.
async fn discord_webhook(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Create a temporary adapter for parsing (no bot token needed for parsing)
    let adapter = DiscordAdapter::new(String::new(), None);

    let msg = match adapter.parse_webhook_payload(&payload) {
        Some(msg) => msg,
        None => {
            return (
                StatusCode::OK,
                Json(WebhookResponse {
                    ok: true,
                    response: None,
                    error: Some("Message filtered or unparseable".to_string()),
                }),
            );
        }
    };

    info!(
        user_id = %msg.user_id,
        channel_id = %msg.channel_id,
        "Discord webhook message received"
    );

    let handle = RingBridgeHandle {
        ring: state.ring.clone(),
    };
    let router = ChannelRouter::new();

    match bridge::process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Discord,
        &msg.user_id,
        &msg.display_name,
        &msg.text,
    )
    .await
    {
        Ok(response) => (
            StatusCode::OK,
            Json(WebhookResponse {
                ok: true,
                response: Some(response),
                error: None,
            }),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(WebhookResponse {
                ok: false,
                response: None,
                error: Some(e),
            }),
        ),
    }
}

// ---------------------------------------------------------------------------
// Telegram webhook handler
// ---------------------------------------------------------------------------

/// POST /api/channels/telegram/webhook — receive Telegram updates.
async fn telegram_webhook(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let adapter = TelegramAdapter::new(String::new());

    let msg = match adapter.parse_webhook_payload(&payload) {
        Some(msg) => msg,
        None => {
            return (
                StatusCode::OK,
                Json(WebhookResponse {
                    ok: true,
                    response: None,
                    error: Some("Message filtered or unparseable".to_string()),
                }),
            );
        }
    };

    info!(
        user_id = %msg.user_id,
        chat_id = %msg.channel_id,
        "Telegram webhook message received"
    );

    let handle = RingBridgeHandle {
        ring: state.ring.clone(),
    };
    let router = ChannelRouter::new();

    match bridge::process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Telegram,
        &msg.user_id,
        &msg.display_name,
        &msg.text,
    )
    .await
    {
        Ok(response) => {
            // Also send the response back via Telegram API if bot token is configured
            let bot_token_env = std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
            if !bot_token_env.is_empty() {
                let tg = TelegramAdapter::new(bot_token_env);
                if let Err(e) = tg.send_response(&msg.channel_id, &response).await {
                    warn!("Failed to send Telegram response: {e}");
                }
            }

            (
                StatusCode::OK,
                Json(WebhookResponse {
                    ok: true,
                    response: Some(response),
                    error: None,
                }),
            )
        }
        Err(e) => (
            StatusCode::OK,
            Json(WebhookResponse {
                ok: false,
                response: None,
                error: Some(e),
            }),
        ),
    }
}

// ---------------------------------------------------------------------------
// Slack events handler
// ---------------------------------------------------------------------------

/// POST /api/channels/slack/events — receive Slack Events API payloads.
async fn slack_events(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let adapter = SlackAdapter::new(String::new(), None);

    // Handle URL verification challenge
    if let Some(challenge) = adapter.check_url_verification(&payload) {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "challenge": challenge })),
        )
            .into_response();
    }

    let msg = match adapter.parse_webhook_payload(&payload).await {
        Some(msg) => msg,
        None => {
            return (
                StatusCode::OK,
                Json(WebhookResponse {
                    ok: true,
                    response: None,
                    error: Some("Message filtered or unparseable".to_string()),
                }),
            )
                .into_response();
        }
    };

    info!(
        user_id = %msg.user_id,
        channel_id = %msg.channel_id,
        "Slack event message received"
    );

    let handle = RingBridgeHandle {
        ring: state.ring.clone(),
    };
    let router = ChannelRouter::new();

    match bridge::process_incoming_message(
        &handle,
        &router,
        &ChannelPlatform::Slack,
        &msg.user_id,
        &msg.display_name,
        &msg.text,
    )
    .await
    {
        Ok(response) => {
            // Send the response back via Slack API if bot token is configured
            let bot_token_env = std::env::var("SLACK_BOT_TOKEN").unwrap_or_default();
            if !bot_token_env.is_empty() {
                let slack = SlackAdapter::new(bot_token_env, None);
                if let Err(e) = slack.send_response(&msg.channel_id, &response).await {
                    warn!("Failed to send Slack response: {e}");
                }
            }

            (
                StatusCode::OK,
                Json(WebhookResponse {
                    ok: true,
                    response: Some(response),
                    error: None,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::OK,
            Json(WebhookResponse {
                ok: false,
                response: None,
                error: Some(e),
            }),
        )
            .into_response(),
    }
}
