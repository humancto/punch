//! Simple chat endpoint for the `punch chat` CLI command.
//!
//! Provides a lightweight `/api/chat` endpoint that spawns (or reuses) a
//! temporary fighter and returns a plain JSON response. The OpenAI-compatible
//! `/v1/chat/completions` endpoint lives in `openai_compat.rs`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use punch_types::{AgentCoordinator, Capability, FighterManifest, ModelConfig, WeightClass};

use crate::AppState;

/// Build the chat routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/chat", post(simple_chat))
}

// ─── Request / response types ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SimpleChatRequest {
    message: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    system: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    stream: bool,
}

#[derive(Serialize)]
struct SimpleChatResponse {
    response: String,
    usage: SimpleUsage,
}

#[derive(Serialize)]
struct SimpleUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

// ─── Handler ────────────────────────────────────────────────────────────────

/// POST /api/chat — simple chat endpoint used by `punch chat "message"`.
///
/// Spawns a temporary fighter (or reuses an existing one matching the model
/// name) and returns a plain `{ "response": "...", "usage": {...} }` payload.
#[instrument(skip_all)]
async fn simple_chat(
    State(state): State<AppState>,
    Json(body): Json<SimpleChatRequest>,
) -> impl IntoResponse {
    let model_name = body.model.unwrap_or_else(|| "punch-chat".to_string());

    // Reuse existing fighter or spawn a temporary one.
    let fighters = state.ring.list_fighters();
    let existing = fighters
        .iter()
        .find(|(_, manifest, _)| manifest.name == model_name);

    let fighter_id = if let Some((id, _, _)) = existing {
        *id
    } else {
        let system_prompt = body
            .system
            .unwrap_or_else(|| "You are a helpful assistant.".to_string());
        let manifest = FighterManifest {
            name: model_name,
            description: "Temporary fighter for punch chat".to_string(),
            model: ModelConfig {
                provider: state.config.default_model.provider.clone(),
                model: state.config.default_model.model.clone(),
                api_key_env: state.config.default_model.api_key_env.clone(),
                base_url: state.config.default_model.base_url.clone(),
                max_tokens: state.config.default_model.max_tokens,
                temperature: state.config.default_model.temperature,
            },
            system_prompt,
            // Full access is safe here: the API binds to 127.0.0.1 (localhost
            // only). If the API is ever exposed on 0.0.0.0 or via a reverse proxy,
            // this must be replaced with a restricted capability set.
            capabilities: Capability::full_access(),
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        };
        state.ring.spawn_fighter(manifest).await
    };

    let coordinator: Arc<dyn AgentCoordinator> =
        Arc::clone(&state.ring) as Arc<dyn AgentCoordinator>;

    match state
        .ring
        .send_message_with_coordinator(&fighter_id, body.message, Some(coordinator), vec![])
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(&SimpleChatResponse {
                    response: result.response,
                    usage: SimpleUsage {
                        input_tokens: result.usage.input_tokens,
                        output_tokens: result.usage.output_tokens,
                    },
                })
                .unwrap_or_default(),
            ),
        )
            .into_response(),
        Err(e) => {
            let status = match &e {
                punch_types::PunchError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (
                status,
                Json(
                    serde_json::to_value(&ErrorBody {
                        error: e.to_string(),
                    })
                    .unwrap_or_default(),
                ),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_chat_request_deserialization_minimal() {
        let json = r#"{"message": "Hello"}"#;
        let req: SimpleChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "Hello");
        assert!(req.model.is_none());
        assert!(req.system.is_none());
        assert!(!req.stream);
    }

    #[test]
    fn test_simple_chat_request_deserialization_full() {
        let json = r#"{
            "message": "Hello",
            "model": "qwen3:8b",
            "system": "Be brief",
            "stream": true
        }"#;
        let req: SimpleChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "Hello");
        assert_eq!(req.model.as_deref(), Some("qwen3:8b"));
        assert_eq!(req.system.as_deref(), Some("Be brief"));
        assert!(req.stream);
    }

    #[test]
    fn test_simple_chat_response_serialization() {
        let resp = SimpleChatResponse {
            response: "Hi there!".to_string(),
            usage: SimpleUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["response"], "Hi there!");
        assert_eq!(json["usage"]["input_tokens"], 10);
        assert_eq!(json["usage"]["output_tokens"], 5);
    }

    #[test]
    fn test_error_body_serialization() {
        let err = ErrorBody {
            error: "something went wrong".to_string(),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["error"], "something went wrong");
    }
}
