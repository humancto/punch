//! OpenAI-compatible chat completions and models endpoints.
//!
//! Provides a drop-in replacement for the OpenAI API so that any
//! OpenAI-compatible client library can talk to Punch fighters.
//!
//! Endpoints:
//!   POST /v1/chat/completions
//!   GET  /v1/models

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use punch_types::{AgentCoordinator, FighterManifest, ModelConfig, Provider, WeightClass};

use crate::AppState;

/// Build the chat routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
}

// ─── OpenAI-compatible request types ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    /// Model name — maps to a fighter name or the configured model.
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<OaiMessage>,
    /// Whether to stream the response as SSE.
    #[serde(default)]
    pub stream: bool,
    /// Maximum tokens to generate.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Tool definitions (accepted but not used for routing — fighters use their own tools).
    #[serde(default)]
    pub tools: Option<Vec<serde_json::Value>>,
    /// Tool choice strategy.
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OaiMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

// ─── OpenAI-compatible response types ────────────────────────────────────────

#[derive(Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: &'static str,
    created: i64,
    model: String,
    choices: Vec<Choice>,
    usage: UsageInfo,
}

#[derive(Serialize)]
struct Choice {
    index: u32,
    message: ChoiceMessage,
    finish_reason: &'static str,
}

#[derive(Serialize)]
struct ChoiceMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OaiToolCall>>,
}

#[derive(Serialize)]
struct UsageInfo {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OaiToolCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "type")]
    pub call_type: Option<String>,
    pub function: OaiToolCallFunction,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OaiToolCallFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

// ─── Streaming response types ────────────────────────────────────────────────

#[derive(Serialize)]
struct ChatCompletionChunk {
    id: String,
    object: &'static str,
    created: i64,
    model: String,
    choices: Vec<ChunkChoice>,
}

#[derive(Serialize)]
struct ChunkChoice {
    index: u32,
    delta: ChunkDelta,
    finish_reason: Option<&'static str>,
}

#[derive(Serialize)]
struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

// ─── Error response ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct OaiErrorResponse {
    error: OaiError,
}

#[derive(Serialize)]
struct OaiError {
    message: String,
    r#type: &'static str,
    code: Option<&'static str>,
}

// ─── Models response types ──────────────────────────────────────────────────

#[derive(Serialize)]
struct ModelObject {
    id: String,
    object: &'static str,
    created: i64,
    owned_by: String,
}

#[derive(Serialize)]
struct ModelListResponse {
    object: &'static str,
    data: Vec<ModelObject>,
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// POST /v1/chat/completions — OpenAI-compatible chat endpoint.
///
/// The `model` field is used to look up a fighter by name. If no fighter
/// matches, a temporary fighter is spawned using the configured default model.
/// The last user message in the `messages` array is sent to the fighter.
#[instrument(skip_all, fields(model = %body.model))]
async fn chat_completions(
    State(state): State<AppState>,
    Json(body): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    // Extract the last user message.
    let user_message = body
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.clone())
        .unwrap_or_default();

    if user_message.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::to_value(&OaiErrorResponse {
                    error: OaiError {
                        message: "No user message found in messages array".to_string(),
                        r#type: "invalid_request_error",
                        code: Some("missing_user_message"),
                    },
                })
                .unwrap_or_default(),
            ),
        )
            .into_response();
    }

    // Build the system prompt from system messages.
    let system_prompt = body
        .messages
        .iter()
        .filter(|m| m.role == "system")
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");

    let request_id = format!("chatcmpl-{}", Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();

    // Try to find an existing fighter by name.
    let fighters = state.ring.list_fighters();
    let existing = fighters
        .iter()
        .find(|(_, manifest, _)| manifest.name == body.model);

    let fighter_id = if let Some((id, _, _)) = existing {
        *id
    } else {
        // Spawn a temporary fighter with the default model config.
        let manifest = FighterManifest {
            name: body.model.clone(),
            description: format!("Temporary fighter for model {}", body.model),
            model: ModelConfig {
                provider: state.config.default_model.provider.clone(),
                model: state.config.default_model.model.clone(),
                api_key_env: state.config.default_model.api_key_env.clone(),
                base_url: state.config.default_model.base_url.clone(),
                max_tokens: body.max_tokens.or(state.config.default_model.max_tokens),
                temperature: body.temperature.or(state.config.default_model.temperature),
            },
            system_prompt: if system_prompt.is_empty() {
                "You are a helpful assistant.".to_string()
            } else {
                system_prompt
            },
            capabilities: vec![],
            weight_class: WeightClass::Middleweight,
        };
        state.ring.spawn_fighter(manifest).await
    };

    // Handle streaming response.
    if body.stream {
        let coordinator: Arc<dyn AgentCoordinator> =
            Arc::clone(&state.ring) as Arc<dyn AgentCoordinator>;

        let result = state
            .ring
            .send_message_with_coordinator(&fighter_id, user_message, Some(coordinator))
            .await;

        match result {
            Ok(result) => {
                // Send the full response as SSE chunks.
                let (tx, rx) =
                    tokio::sync::mpsc::channel::<Result<SseEvent, std::convert::Infallible>>(16);

                let model = body.model.clone();
                let rid = request_id.clone();

                tokio::spawn(async move {
                    // First chunk: role
                    let first = ChatCompletionChunk {
                        id: rid.clone(),
                        object: "chat.completion.chunk",
                        created,
                        model: model.clone(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: ChunkDelta {
                                role: Some("assistant"),
                                content: None,
                            },
                            finish_reason: None,
                        }],
                    };
                    let _ = tx
                        .send(Ok(SseEvent::default()
                            .data(serde_json::to_string(&first).unwrap_or_default())))
                        .await;

                    // Content chunk
                    let content_chunk = ChatCompletionChunk {
                        id: rid.clone(),
                        object: "chat.completion.chunk",
                        created,
                        model: model.clone(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: ChunkDelta {
                                role: None,
                                content: Some(result.response),
                            },
                            finish_reason: None,
                        }],
                    };
                    let _ = tx
                        .send(Ok(SseEvent::default().data(
                            serde_json::to_string(&content_chunk).unwrap_or_default(),
                        )))
                        .await;

                    // Final chunk: finish_reason
                    let final_chunk = ChatCompletionChunk {
                        id: rid.clone(),
                        object: "chat.completion.chunk",
                        created,
                        model: model.clone(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: ChunkDelta {
                                role: None,
                                content: None,
                            },
                            finish_reason: Some("stop"),
                        }],
                    };
                    let _ =
                        tx.send(Ok(SseEvent::default()
                            .data(serde_json::to_string(&final_chunk).unwrap_or_default())))
                            .await;

                    // [DONE] sentinel
                    let _ = tx.send(Ok(SseEvent::default().data("[DONE]"))).await;
                });

                let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
                Sse::new(stream)
                    .keep_alive(KeepAlive::default())
                    .into_response()
            }
            Err(e) => {
                let (status, code) = match &e {
                    punch_types::PunchError::RateLimited { .. } => {
                        (StatusCode::TOO_MANY_REQUESTS, Some("rate_limit_exceeded"))
                    }
                    _ => (StatusCode::INTERNAL_SERVER_ERROR, Some("internal_error")),
                };
                (
                    status,
                    Json(
                        serde_json::to_value(&OaiErrorResponse {
                            error: OaiError {
                                message: e.to_string(),
                                r#type: "api_error",
                                code,
                            },
                        })
                        .unwrap_or_default(),
                    ),
                )
                    .into_response()
            }
        }
    } else {
        // Non-streaming response.
        let coordinator: Arc<dyn AgentCoordinator> =
            Arc::clone(&state.ring) as Arc<dyn AgentCoordinator>;

        match state
            .ring
            .send_message_with_coordinator(&fighter_id, user_message, Some(coordinator))
            .await
        {
            Ok(result) => {
                let response = ChatCompletionResponse {
                    id: request_id,
                    object: "chat.completion",
                    created,
                    model: body.model,
                    choices: vec![Choice {
                        index: 0,
                        message: ChoiceMessage {
                            role: "assistant",
                            content: Some(result.response),
                            tool_calls: None,
                        },
                        finish_reason: "stop",
                    }],
                    usage: UsageInfo {
                        prompt_tokens: result.usage.input_tokens,
                        completion_tokens: result.usage.output_tokens,
                        total_tokens: result.usage.total(),
                    },
                };
                Json(serde_json::to_value(&response).unwrap_or_default()).into_response()
            }
            Err(e) => {
                let (status, code) = match &e {
                    punch_types::PunchError::RateLimited { .. } => {
                        (StatusCode::TOO_MANY_REQUESTS, Some("rate_limit_exceeded"))
                    }
                    _ => (StatusCode::INTERNAL_SERVER_ERROR, Some("internal_error")),
                };
                (
                    status,
                    Json(
                        serde_json::to_value(&OaiErrorResponse {
                            error: OaiError {
                                message: e.to_string(),
                                r#type: "api_error",
                                code,
                            },
                        })
                        .unwrap_or_default(),
                    ),
                )
                    .into_response()
            }
        }
    }
}

/// GET /v1/models — list available models in OpenAI format.
///
/// If the configured provider is Ollama, fetches models from `localhost:11434/api/tags`.
/// Otherwise, returns the configured default model and any active fighters.
#[instrument(skip_all)]
async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let created = chrono::Utc::now().timestamp();
    let mut models: Vec<ModelObject> = Vec::new();

    // Try to fetch from Ollama if configured.
    if state.config.default_model.provider == Provider::Ollama {
        let base_url = state
            .config
            .default_model
            .base_url
            .as_deref()
            .unwrap_or("http://localhost:11434");

        if let Ok(resp) = reqwest::get(format!("{}/api/tags", base_url)).await
            && let Ok(body) = resp.json::<serde_json::Value>().await
            && let Some(ollama_models) = body["models"].as_array()
        {
            for m in ollama_models {
                if let Some(name) = m["name"].as_str() {
                    models.push(ModelObject {
                        id: name.to_string(),
                        object: "model",
                        created,
                        owned_by: "ollama".to_string(),
                    });
                }
            }
        }
    }

    // Always include the configured default model.
    let default_id = state.config.default_model.model.clone();
    if !models.iter().any(|m| m.id == default_id) {
        models.push(ModelObject {
            id: default_id,
            object: "model",
            created,
            owned_by: state.config.default_model.provider.to_string(),
        });
    }

    // Include active fighters as "models".
    let fighters = state.ring.list_fighters();
    for (_, manifest, _) in &fighters {
        if !models.iter().any(|m| m.id == manifest.name) {
            models.push(ModelObject {
                id: manifest.name.clone(),
                object: "model",
                created,
                owned_by: manifest.model.provider.to_string(),
            });
        }
    }

    Json(
        serde_json::to_value(&ModelListResponse {
            object: "list",
            data: models,
        })
        .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_deserialization_minimal() {
        let json = r#"{
            "model": "gpt-oss:20b",
            "messages": [{"role": "user", "content": "Hello"}]
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-oss:20b");
        assert!(!req.stream);
        assert!(req.max_tokens.is_none());
    }

    #[test]
    fn test_request_deserialization_full() {
        let json = r#"{
            "model": "gpt-oss:20b",
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hello"}
            ],
            "max_tokens": 4096,
            "temperature": 0.7,
            "stream": true,
            "tools": [{"type": "function", "function": {"name": "test"}}],
            "tool_choice": "auto"
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-oss:20b");
        assert!(req.stream);
        assert_eq!(req.max_tokens, Some(4096));
        assert_eq!(req.temperature, Some(0.7));
        assert!(req.tools.is_some());
    }

    #[test]
    fn test_response_serialization() {
        let resp = ChatCompletionResponse {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion",
            created: 1234567890,
            model: "gpt-oss:20b".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChoiceMessage {
                    role: "assistant",
                    content: Some("Hello!".to_string()),
                    tool_calls: None,
                },
                finish_reason: "stop",
            }],
            usage: UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["object"], "chat.completion");
        assert_eq!(json["choices"][0]["message"]["content"], "Hello!");
        assert_eq!(json["usage"]["total_tokens"], 15);
        // tool_calls should be omitted when None
        assert!(json["choices"][0]["message"].get("tool_calls").is_none());
    }

    #[test]
    fn test_response_with_tool_calls() {
        let resp = ChatCompletionResponse {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion",
            created: 1234567890,
            model: "gpt-oss:20b".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChoiceMessage {
                    role: "assistant",
                    content: None,
                    tool_calls: Some(vec![OaiToolCall {
                        index: Some(0),
                        id: Some("call_abc123".to_string()),
                        call_type: Some("function".to_string()),
                        function: OaiToolCallFunction {
                            name: Some("get_weather".to_string()),
                            arguments: Some(r#"{"location":"NYC"}"#.to_string()),
                        },
                    }]),
                },
                finish_reason: "tool_calls",
            }],
            usage: UsageInfo {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["choices"][0]["message"].get("content").is_none());
        let tc = &json["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(tc["function"]["name"], "get_weather");
    }

    #[test]
    fn test_chunk_serialization() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion.chunk",
            created: 1234567890,
            model: "gpt-oss:20b".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: Some("Hello".to_string()),
                },
                finish_reason: None,
            }],
        };
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json["object"], "chat.completion.chunk");
        assert_eq!(json["choices"][0]["delta"]["content"], "Hello");
    }

    #[test]
    fn test_models_response_serialization() {
        let resp = ModelListResponse {
            object: "list",
            data: vec![ModelObject {
                id: "gpt-oss:20b".to_string(),
                object: "model",
                created: 1234567890,
                owned_by: "ollama".to_string(),
            }],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["object"], "list");
        assert_eq!(json["data"][0]["id"], "gpt-oss:20b");
        assert_eq!(json["data"][0]["owned_by"], "ollama");
    }
}
