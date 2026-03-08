//! OpenAI-compatible chat completions endpoint.
//!
//! Maps the standard `/v1/chat/completions` request format to internal
//! fighter message sending, allowing any OpenAI-compatible client to
//! interact with Punch fighters.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;


use crate::AppState;

/// Build the chat routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/v1/chat/completions", post(chat_completions))
}

// ---------------------------------------------------------------------------
// OpenAI-compatible request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChatCompletionRequest {
    /// Model name — used to select a fighter by name.
    model: String,
    /// The conversation messages.
    messages: Vec<ChatMessage>,
    /// Maximum tokens to generate (ignored — controlled by fighter config).
    #[serde(default)]
    #[allow(dead_code)]
    max_tokens: Option<u32>,
    /// Sampling temperature (ignored — controlled by fighter config).
    #[serde(default)]
    #[allow(dead_code)]
    temperature: Option<f32>,
}

#[derive(Deserialize, Serialize, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: &'static str,
    created: i64,
    model: String,
    choices: Vec<ChatChoice>,
    usage: ChatUsage,
}

#[derive(Serialize)]
struct ChatChoice {
    index: u32,
    message: ChatMessage,
    finish_reason: &'static str,
}

#[derive(Serialize)]
struct ChatUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ChatError,
}

#[derive(Serialize)]
struct ChatError {
    message: String,
    r#type: &'static str,
    code: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// POST /v1/chat/completions — OpenAI-compatible chat endpoint.
///
/// The `model` field is used to look up a fighter by name. The last user
/// message in the `messages` array is sent to the fighter.
#[instrument(skip_all, fields(model = %body.model))]
async fn chat_completions(
    State(state): State<AppState>,
    Json(body): Json<ChatCompletionRequest>,
) -> Result<Json<ChatCompletionResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Find a fighter whose name matches the requested model.
    let fighters = state.ring.list_fighters();

    let (fighter_id, _, _) = fighters
        .iter()
        .find(|(_, manifest, _)| manifest.name == body.model)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: ChatError {
                        message: format!("no fighter found with name '{}'", body.model),
                        r#type: "invalid_request_error",
                        code: Some("model_not_found"),
                    },
                }),
            )
        })?;

    // Extract the last user message.
    let user_message = body
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: ChatError {
                        message: "no user message found in messages array".to_string(),
                        r#type: "invalid_request_error",
                        code: Some("missing_user_message"),
                    },
                }),
            )
        })?;

    // Send the message to the fighter.
    let result = state
        .ring
        .send_message(fighter_id, user_message)
        .await
        .map_err(|e| {
            let (status, code) = match &e {
                punch_types::PunchError::RateLimited { .. } => {
                    (StatusCode::TOO_MANY_REQUESTS, Some("rate_limit_exceeded"))
                }
                _ => (StatusCode::INTERNAL_SERVER_ERROR, Some("internal_error")),
            };
            (
                status,
                Json(ErrorResponse {
                    error: ChatError {
                        message: e.to_string(),
                        r#type: "api_error",
                        code,
                    },
                }),
            )
        })?;

    let total_tokens = result.usage.total();

    Ok(Json(ChatCompletionResponse {
        id: format!("chatcmpl-{}", Uuid::new_v4()),
        object: "chat.completion",
        created: chrono::Utc::now().timestamp(),
        model: body.model,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: result.response,
            },
            finish_reason: "stop",
        }],
        usage: ChatUsage {
            prompt_tokens: 0,
            completion_tokens: total_tokens,
            total_tokens,
        },
    }))
}
