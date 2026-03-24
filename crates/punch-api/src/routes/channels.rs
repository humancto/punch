//! Webhook endpoints for channel adapters.
//!
//! Security architecture:
//! 1. Parse raw payload (platform-specific)
//! 2. Verify webhook signature (Telegram secret_token, Slack HMAC-SHA256)
//! 3. Check user allowlist (deny unknown users)
//! 4. Check per-user rate limit (prevent DoS)
//! 5. Route to fighter via persistent ChannelRouter (from AppState)
//! 6. Send message via MCP-aware bridge handle (fighters get MCP tools)
//!
//! All security checks happen BEFORE any fighter interaction.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::body::Bytes;
use axum::{Json, Router};
use serde::Serialize;
use tracing::{info, warn};

use punch_channels::ChannelAdapter;
use punch_channels::ChannelPlatform;
use punch_channels::adapters::{DiscordAdapter, SlackAdapter, TelegramAdapter};
use punch_channels::bridge::{self, ChannelBridgeHandle};
use punch_channels::security::ChannelGateway;
use punch_types::FighterId;
use punch_types::config::ChannelConfig;

use crate::AppState;

/// Build the channel webhook routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/channels/discord/webhook", post(discord_webhook))
        .route("/api/channels/telegram/webhook", post(telegram_webhook))
        .route("/api/channels/slack/events", post(slack_events))
}

#[derive(Serialize)]
struct WebhookResponse {
    ok: bool,
    response: Option<String>,
    error: Option<String>,
}

fn ok_response(response: String) -> (StatusCode, Json<WebhookResponse>) {
    (
        StatusCode::OK,
        Json(WebhookResponse {
            ok: true,
            response: Some(response),
            error: None,
        }),
    )
}

fn err_response(error: String) -> (StatusCode, Json<WebhookResponse>) {
    (
        StatusCode::OK,
        Json(WebhookResponse {
            ok: false,
            response: None,
            error: Some(error),
        }),
    )
}

fn filtered_response() -> (StatusCode, Json<WebhookResponse>) {
    (
        StatusCode::OK,
        Json(WebhookResponse {
            ok: true,
            response: None,
            error: Some("Message filtered or unparseable".to_string()),
        }),
    )
}

fn denied_response(reason: &str) -> (StatusCode, Json<WebhookResponse>) {
    (
        StatusCode::FORBIDDEN,
        Json(WebhookResponse {
            ok: false,
            response: None,
            error: Some(reason.to_string()),
        }),
    )
}

/// MCP-aware Ring bridge handle.
///
/// Uses `send_message_with_coordinator` so fighters routed from channels
/// have access to MCP tools (LocalMind, etc.) — not just plain send_message.
struct RingBridgeHandle {
    ring: Arc<punch_kernel::Ring>,
}

#[async_trait::async_trait]
impl ChannelBridgeHandle for RingBridgeHandle {
    async fn send_message(&self, fighter_id: FighterId, message: &str) -> Result<String, String> {
        // Use coordinator-aware path so channel fighters get MCP tools.
        match self
            .ring
            .send_message_with_coordinator(&fighter_id, message.to_string(), None)
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
        Err("Auto-spawn not available in webhook mode. Create a fighter first via `punch fighter spawn`.".to_string())
    }
}

/// Resolve the ChannelGateway for a given channel type.
///
/// Looks up the channel config by type name and builds a gateway with
/// security settings (allowlist, rate limit, webhook secret).
fn resolve_gateway(state: &AppState, channel_type: &str) -> ChannelGateway {
    let config = state
        .config
        .channels
        .values()
        .find(|c| c.channel_type == channel_type);

    match config {
        Some(cfg) => ChannelGateway::from_config(channel_type, cfg),
        None => {
            // No config for this channel — create a permissive gateway (dev mode).
            warn!(
                channel = %channel_type,
                "no channel config found — running with open access (dev mode)"
            );
            ChannelGateway::from_config(
                channel_type,
                &ChannelConfig {
                    channel_type: channel_type.to_string(),
                    token_env: None,
                    webhook_secret_env: None,
                    allowed_user_ids: vec![],
                    rate_limit_per_user: 20,
                    settings: Default::default(),
                },
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Telegram webhook handler
// ---------------------------------------------------------------------------

/// POST /api/channels/telegram/webhook — receive Telegram updates.
///
/// Security: Verifies X-Telegram-Bot-Api-Secret-Token header against
/// the configured webhook secret. This header is set when you register
/// the webhook with Telegram's setWebhook API.
async fn telegram_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    // TODO: Cache ChannelGateway in AppState instead of constructing per-request
    let gateway = resolve_gateway(&state, "telegram");

    // 1. Verify Telegram secret token header.
    if let Some(ref expected_secret) = gateway.webhook_secret {
        let provided = headers
            .get("x-telegram-bot-api-secret-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != expected_secret {
            warn!("Telegram webhook signature verification FAILED");
            return denied_response("Invalid webhook secret");
        }
    }

    // 2. Parse the payload.
    let adapter = TelegramAdapter::new(String::new());
    let msg = match adapter.parse_webhook_payload(&payload) {
        Some(msg) => msg,
        None => return filtered_response(),
    };

    info!(
        user_id = %msg.user_id,
        chat_id = %msg.channel_id,
        "Telegram webhook message received"
    );

    // 3. Security checks: allowlist + rate limit.
    if let Err(reason) = gateway.authorize_request(&msg.user_id) {
        return denied_response(&reason);
    }

    // 4. Route to fighter via persistent router.
    let handle = RingBridgeHandle {
        ring: state.ring.clone(),
    };

    match bridge::process_incoming_message(
        &handle,
        &state.channel_router,
        &ChannelPlatform::Telegram,
        &msg.user_id,
        &msg.display_name,
        &msg.text,
    )
    .await
    {
        Ok(response) => {
            // Send response back via Telegram Bot API.
            if let Some(cfg) = state
                .config
                .channels
                .values()
                .find(|c| c.channel_type == "telegram")
                && let Some(ref env_var) = cfg.token_env
                && let Ok(token) = std::env::var(env_var)
            {
                let tg = TelegramAdapter::new(token);
                if let Err(e) = tg.send_response(&msg.channel_id, &response).await {
                    warn!("Failed to send Telegram response: {e}");
                }
            }
            ok_response(response)
        }
        Err(e) => err_response(e),
    }
}

// ---------------------------------------------------------------------------
// Discord webhook handler
// ---------------------------------------------------------------------------

/// POST /api/channels/discord/webhook — receive Discord messages.
///
/// Security: Verifies X-Punch-Secret header against the configured
/// webhook secret. Discord's native Ed25519 verification requires the
/// `ed25519-dalek` dependency — for now we use a shared secret header
/// that should be set on the Discord bot's webhook configuration.
async fn discord_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    // TODO: Cache ChannelGateway in AppState instead of constructing per-request
    let gateway = resolve_gateway(&state, "discord");

    // 1. Verify shared secret header.
    if let Some(ref expected_secret) = gateway.webhook_secret {
        let provided = headers
            .get("x-punch-secret")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != expected_secret {
            warn!("Discord webhook secret verification FAILED");
            return denied_response("Invalid webhook secret");
        }
    }

    // 2. Parse the payload.
    let adapter = DiscordAdapter::new(String::new(), None);
    let msg = match adapter.parse_webhook_payload(&payload) {
        Some(msg) => msg,
        None => return filtered_response(),
    };

    info!(
        user_id = %msg.user_id,
        channel_id = %msg.channel_id,
        "Discord webhook message received"
    );

    // 3. Security checks.
    if let Err(reason) = gateway.authorize_request(&msg.user_id) {
        return denied_response(&reason);
    }

    // 4. Route to fighter.
    let handle = RingBridgeHandle {
        ring: state.ring.clone(),
    };

    match bridge::process_incoming_message(
        &handle,
        &state.channel_router,
        &ChannelPlatform::Discord,
        &msg.user_id,
        &msg.display_name,
        &msg.text,
    )
    .await
    {
        Ok(response) => ok_response(response),
        Err(e) => err_response(e),
    }
}

// ---------------------------------------------------------------------------
// Slack events handler
// ---------------------------------------------------------------------------

/// POST /api/channels/slack/events — receive Slack Events API payloads.
///
/// Security: Verifies the X-Slack-Signature header using HMAC-SHA256
/// with the configured signing secret. This is Slack's standard
/// request verification mechanism.
async fn slack_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    bytes: Bytes,
) -> impl IntoResponse {
    // TODO: Cache ChannelGateway in AppState instead of constructing per-request
    let gateway = resolve_gateway(&state, "slack");

    // Parse JSON from raw bytes — we keep `bytes` around for HMAC verification.
    let payload: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            warn!("Slack webhook: failed to parse JSON body: {e}");
            return err_response(format!("Invalid JSON: {e}")).into_response();
        }
    };

    // Resolve signing secret for Slack HMAC verification.
    let signing_secret = gateway.webhook_secret.clone();
    let adapter = SlackAdapter::new(String::new(), signing_secret.clone());

    // Handle URL verification challenge (no auth needed — Slack sends this
    // during webhook setup and expects the challenge echoed back).
    if let Some(challenge) = adapter.check_url_verification(&payload) {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "challenge": challenge })),
        )
            .into_response();
    }

    // 1. Verify Slack HMAC-SHA256 signature against the raw body bytes.
    if signing_secret.is_some() {
        let timestamp = headers
            .get("x-slack-request-timestamp")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let signature = headers
            .get("x-slack-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Replay attack protection: reject requests older than 5 minutes.
        if let Ok(ts) = timestamp.parse::<i64>() {
            let now = chrono::Utc::now().timestamp();
            if (now - ts).unsigned_abs() > 300 {
                warn!("Slack webhook rejected: timestamp too old (replay attack?)");
                return denied_response("Request too old").into_response();
            }
        }

        // Verify against raw body bytes (not re-serialized JSON) so the HMAC
        // matches exactly what Slack computed.
        if !adapter.verify_webhook_signature(timestamp, signature, &bytes) {
            warn!("Slack webhook signature verification FAILED");
            return denied_response("Invalid signature").into_response();
        }
    }

    // 2. Parse the event.
    let msg = match adapter.parse_webhook_payload(&payload).await {
        Some(msg) => msg,
        None => return filtered_response().into_response(),
    };

    info!(
        user_id = %msg.user_id,
        channel_id = %msg.channel_id,
        "Slack event message received"
    );

    // 3. Security checks.
    if let Err(reason) = gateway.authorize_request(&msg.user_id) {
        return denied_response(&reason).into_response();
    }

    // 4. Route to fighter.
    let handle = RingBridgeHandle {
        ring: state.ring.clone(),
    };

    match bridge::process_incoming_message(
        &handle,
        &state.channel_router,
        &ChannelPlatform::Slack,
        &msg.user_id,
        &msg.display_name,
        &msg.text,
    )
    .await
    {
        Ok(response) => {
            // Send response back via Slack Web API.
            if let Some(cfg) = state
                .config
                .channels
                .values()
                .find(|c| c.channel_type == "slack")
                && let Some(ref env_var) = cfg.token_env
                && let Ok(token) = std::env::var(env_var)
            {
                let slack = SlackAdapter::new(token, None);
                if let Err(e) = slack.send_response(&msg.channel_id, &response).await {
                    warn!("Failed to send Slack response: {e}");
                }
            }
            ok_response(response).into_response()
        }
        Err(e) => err_response(e).into_response(),
    }
}
