//! Enhanced OpenAI-compatible chat completions and models endpoints.
//!
//! Provides a fully spec-compliant drop-in replacement for the OpenAI API so
//! that any OpenAI client library can talk to Punch fighters without
//! modification.
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

use punch_types::{
    AgentCoordinator, Capability, FighterManifest, ModelCatalog, ModelConfig, WeightClass,
};

use crate::AppState;

// ─── OpenAI-compatible request types ─────────────────────────────────────────

/// A chat completion request matching the OpenAI API specification.
#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    /// Model name -- maps to a fighter name or the configured model.
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<ChatMessage>,
    /// Sampling temperature (0.0 to 2.0).
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Nucleus sampling parameter.
    #[serde(default)]
    pub top_p: Option<f64>,
    /// Maximum tokens to generate.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Whether to stream the response as SSE.
    #[serde(default)]
    pub stream: Option<bool>,
    /// Stop sequences -- generation halts when any of these are produced.
    #[serde(default)]
    pub stop: Option<StopSequence>,
    /// Tool definitions available to the model.
    #[serde(default)]
    pub tools: Option<Vec<ToolSpec>>,
    /// Controls which tool the model should call.
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

/// Stop sequences can be a single string or an array of strings.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum StopSequence {
    Single(String),
    Multiple(Vec<String>),
}

impl StopSequence {
    /// Normalise into a `Vec<String>` regardless of variant.
    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::Single(s) => vec![s],
            Self::Multiple(v) => v,
        }
    }
}

/// A message in the chat conversation, matching the OpenAI message format.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    /// Role of the message author: "system", "user", "assistant", or "tool".
    pub role: String,
    /// Text content of the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// An optional name for the participant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool calls requested by the assistant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallSpec>>,
    /// The ID of the tool call this message is responding to (role = "tool").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// An OpenAI-format tool definition.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolSpec {
    /// Always "function" in the current OpenAI API.
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function definition.
    pub function: ToolFunctionSpec,
}

/// Function definition inside a tool spec.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolFunctionSpec {
    /// The name of the function.
    pub name: String,
    /// A description of what the function does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema describing the function parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// A tool call emitted by the assistant.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallSpec {
    /// Unique identifier for this tool call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The type of tool call (always "function").
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub call_type: Option<String>,
    /// The function call details.
    pub function: ToolCallFunctionSpec,
    /// Index for streaming tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
}

/// Function call details inside a tool call.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallFunctionSpec {
    /// Name of the function to call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Stringified JSON arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

// ─── OpenAI-compatible response types ────────────────────────────────────────

/// A non-streaming chat completion response.
#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    /// Unique identifier for this completion.
    pub id: String,
    /// Object type -- always "chat.completion".
    pub object: &'static str,
    /// Unix timestamp when the completion was created.
    pub created: i64,
    /// The model used for the completion.
    pub model: String,
    /// Completion choices (typically one).
    pub choices: Vec<Choice>,
    /// Token usage statistics.
    pub usage: Usage,
}

/// A single completion choice.
#[derive(Debug, Serialize)]
pub struct Choice {
    /// Index of this choice.
    pub index: u32,
    /// The message produced by the model.
    pub message: ChatMessage,
    /// Reason the model stopped generating: "stop", "length", or "tool_calls".
    pub finish_reason: String,
}

/// Token usage statistics.
#[derive(Debug, Serialize)]
pub struct Usage {
    /// Number of tokens in the prompt.
    pub prompt_tokens: u64,
    /// Number of tokens in the completion.
    pub completion_tokens: u64,
    /// Total tokens used (prompt + completion).
    pub total_tokens: u64,
}

// ─── Streaming response types ────────────────────────────────────────────────

/// A streaming chunk of a chat completion response.
#[derive(Debug, Serialize)]
struct ChatCompletionChunk {
    id: String,
    object: &'static str,
    created: i64,
    model: String,
    choices: Vec<ChunkChoice>,
}

/// A choice within a streaming chunk.
#[derive(Debug, Serialize)]
struct ChunkChoice {
    index: u32,
    delta: ChunkDelta,
    finish_reason: Option<String>,
}

/// The delta content in a streaming chunk.
#[derive(Debug, Serialize)]
struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCallSpec>>,
}

// ─── Error response ─────────────────────────────────────────────────────────

/// OpenAI-format error envelope.
#[derive(Debug, Serialize)]
struct OaiErrorResponse {
    error: OaiError,
}

/// OpenAI-format error body.
#[derive(Debug, Serialize)]
struct OaiError {
    message: String,
    r#type: String,
    code: Option<String>,
}

// ─── Models response types ──────────────────────────────────────────────────

/// An individual model object in the OpenAI models listing.
#[derive(Debug, Serialize)]
pub struct ModelObject {
    /// The model identifier.
    pub id: String,
    /// Object type -- always "model".
    pub object: &'static str,
    /// Unix timestamp when the model was created / registered.
    pub created: i64,
    /// The organisation or provider that owns the model.
    pub owned_by: String,
}

/// The response envelope for listing models.
#[derive(Debug, Serialize)]
pub struct ModelListResponse {
    /// Object type -- always "list".
    pub object: &'static str,
    /// Array of model objects.
    pub data: Vec<ModelObject>,
}

// ─── Conversion helpers ─────────────────────────────────────────────────────

/// Convert an OpenAI `ChatMessage` to Punch's internal `Message` format.
pub fn oai_message_to_punch(msg: &ChatMessage) -> punch_types::Message {
    let role = match msg.role.as_str() {
        "system" => punch_types::Role::System,
        "assistant" => punch_types::Role::Assistant,
        "tool" => punch_types::Role::Tool,
        _ => punch_types::Role::User,
    };

    let mut punch_msg = punch_types::Message::new(role, msg.content.clone().unwrap_or_default());

    // Convert tool calls from OAI format to Punch format.
    if let Some(ref tool_calls) = msg.tool_calls {
        punch_msg.tool_calls = tool_calls
            .iter()
            .map(|tc| punch_types::ToolCall {
                id: tc.id.clone().unwrap_or_default(),
                name: tc.function.name.clone().unwrap_or_default(),
                input: tc
                    .function
                    .arguments
                    .as_ref()
                    .and_then(|a| serde_json::from_str(a).ok())
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
            })
            .collect();
    }

    // Convert tool results for role="tool" messages.
    if msg.role == "tool"
        && let Some(ref tool_call_id) = msg.tool_call_id
    {
        punch_msg.tool_results = vec![punch_types::ToolCallResult {
            id: tool_call_id.clone(),
            content: msg.content.clone().unwrap_or_default(),
            is_error: false,
            image: None,
        }];
    }

    punch_msg
}

/// Convert a Punch `Message` back to an OpenAI `ChatMessage`.
pub fn punch_message_to_oai(msg: &punch_types::Message) -> ChatMessage {
    let role = match msg.role {
        punch_types::Role::System => "system",
        punch_types::Role::User => "user",
        punch_types::Role::Assistant => "assistant",
        punch_types::Role::Tool => "tool",
    };

    let tool_calls = if msg.tool_calls.is_empty() {
        None
    } else {
        Some(
            msg.tool_calls
                .iter()
                .enumerate()
                .map(|(i, tc)| ToolCallSpec {
                    id: Some(tc.id.clone()),
                    call_type: Some("function".to_string()),
                    function: ToolCallFunctionSpec {
                        name: Some(tc.name.clone()),
                        arguments: Some(serde_json::to_string(&tc.input).unwrap_or_default()),
                    },
                    index: Some(i as u32),
                })
                .collect(),
        )
    };

    let tool_call_id = msg.tool_results.first().map(|r| r.id.clone());

    let content = if msg.content.is_empty() {
        None
    } else {
        Some(msg.content.clone())
    };

    ChatMessage {
        role: role.to_string(),
        content,
        name: None,
        tool_calls,
        tool_call_id,
    }
}

/// Generate a unique chat completion ID.
pub fn generate_completion_id() -> String {
    format!("chatcmpl-{}", Uuid::new_v4())
}

/// Apply stop sequences to a response string, truncating at the first match.
pub fn apply_stop_sequences(text: &str, stop: &[String]) -> (String, &'static str) {
    for seq in stop {
        if let Some(pos) = text.find(seq.as_str()) {
            return (text[..pos].to_string(), "stop");
        }
    }
    (text.to_string(), "stop")
}

/// Estimate prompt token count from messages (rough heuristic: ~4 chars per token).
pub fn estimate_prompt_tokens(messages: &[ChatMessage]) -> u64 {
    let total_chars: usize = messages
        .iter()
        .map(|m| {
            let content_len = m.content.as_ref().map_or(0, |c| c.len());
            let role_len = m.role.len();
            let name_len = m.name.as_ref().map_or(0, |n| n.len());
            // Each message has overhead tokens for role/separators (~4 tokens).
            content_len + role_len + name_len + 4
        })
        .sum();
    (total_chars as u64) / 4
}

/// Estimate completion token count (rough heuristic: ~4 chars per token).
pub fn estimate_completion_tokens(text: &str) -> u64 {
    (text.len() as u64) / 4
}

// ─── Router ─────────────────────────────────────────────────────────────────

/// Build the OpenAI-compatible routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// Build an OpenAI-format error response.
fn oai_error_response(
    status: StatusCode,
    message: String,
    error_type: &str,
    code: Option<&str>,
) -> axum::response::Response {
    (
        status,
        Json(
            serde_json::to_value(OaiErrorResponse {
                error: OaiError {
                    message,
                    r#type: error_type.to_string(),
                    code: code.map(String::from),
                },
            })
            .unwrap_or_default(),
        ),
    )
        .into_response()
}

/// POST /v1/chat/completions -- OpenAI-compatible chat endpoint.
///
/// The `model` field is used to look up a fighter by name. If no fighter
/// matches, a temporary fighter is spawned using the configured default model.
/// The last user message in the `messages` array is sent to the fighter.
#[instrument(skip_all, fields(model = %body.model, stream = ?body.stream))]
async fn chat_completions(
    State(state): State<AppState>,
    Json(body): Json<ChatCompletionRequest>,
) -> axum::response::Response {
    let is_stream = body.stream.unwrap_or(false);

    // Extract the last user message.
    let user_message = body
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.clone())
        .unwrap_or_default();

    if user_message.is_empty() {
        return oai_error_response(
            StatusCode::BAD_REQUEST,
            "No user message found in messages array".to_string(),
            "invalid_request_error",
            Some("missing_user_message"),
        );
    }

    // Build the system prompt from system messages.
    let system_prompt = body
        .messages
        .iter()
        .filter(|m| m.role == "system")
        .filter_map(|m| m.content.as_deref())
        .collect::<Vec<_>>()
        .join("\n");

    // Collect stop sequences.
    let stop_sequences: Vec<String> = body.stop.map(|s| s.into_vec()).unwrap_or_default();

    let request_id = generate_completion_id();
    let created = chrono::Utc::now().timestamp();

    // Resolve temperature: OpenAI uses f64 (0.0-2.0), Punch uses f32.
    let temperature = body.temperature.map(|t| t as f32);
    let max_tokens = body.max_tokens;

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
                max_tokens: max_tokens.or(state.config.default_model.max_tokens),
                temperature: temperature.or(state.config.default_model.temperature),
            },
            system_prompt: if system_prompt.is_empty() {
                "You are a helpful assistant.".to_string()
            } else {
                system_prompt
            },
            // Full access is safe here: the API binds to 127.0.0.1 (localhost
            // only). If the API is ever exposed on 0.0.0.0 or via a reverse proxy,
            // this must be replaced with a restricted capability set.
            capabilities: Capability::full_access(),
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        };
        state.ring.spawn_fighter(manifest).await
    };

    // Handle streaming response.
    if is_stream {
        handle_streaming(
            state,
            body.model,
            fighter_id,
            user_message,
            request_id,
            created,
            stop_sequences,
        )
        .await
    } else {
        handle_non_streaming(
            state,
            body.model,
            fighter_id,
            user_message,
            request_id,
            created,
            stop_sequences,
        )
        .await
    }
}

/// Handle a streaming chat completion request.
async fn handle_streaming(
    state: AppState,
    model: String,
    fighter_id: punch_types::FighterId,
    user_message: String,
    request_id: String,
    created: i64,
    stop_sequences: Vec<String>,
) -> axum::response::Response {
    let coordinator: Arc<dyn AgentCoordinator> =
        Arc::clone(&state.ring) as Arc<dyn AgentCoordinator>;

    let result = state
        .ring
        .send_message_with_coordinator(&fighter_id, user_message, Some(coordinator), vec![])
        .await;

    match result {
        Ok(result) => {
            let (tx, rx) =
                tokio::sync::mpsc::channel::<Result<SseEvent, std::convert::Infallible>>(16);

            let rid = request_id;
            let model_name = model;
            let stops = stop_sequences;

            tokio::spawn(async move {
                // First chunk: role indicator.
                let first = ChatCompletionChunk {
                    id: rid.clone(),
                    object: "chat.completion.chunk",
                    created,
                    model: model_name.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role: Some("assistant".to_string()),
                            content: None,
                            tool_calls: None,
                        },
                        finish_reason: None,
                    }],
                };
                let _ = tx
                    .send(Ok(
                        SseEvent::default().data(serde_json::to_string(&first).unwrap_or_default())
                    ))
                    .await;

                // Apply stop sequences to the response.
                let (response_text, finish) = if stops.is_empty() {
                    (result.response.clone(), "stop".to_string())
                } else {
                    let (text, reason) = apply_stop_sequences(&result.response, &stops);
                    (text, reason.to_string())
                };

                // Content chunk.
                let content_chunk = ChatCompletionChunk {
                    id: rid.clone(),
                    object: "chat.completion.chunk",
                    created,
                    model: model_name.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role: None,
                            content: Some(response_text),
                            tool_calls: None,
                        },
                        finish_reason: None,
                    }],
                };
                let _ = tx
                    .send(Ok(SseEvent::default().data(
                        serde_json::to_string(&content_chunk).unwrap_or_default(),
                    )))
                    .await;

                // Final chunk: finish reason.
                let final_chunk = ChatCompletionChunk {
                    id: rid.clone(),
                    object: "chat.completion.chunk",
                    created,
                    model: model_name.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: ChunkDelta {
                            role: None,
                            content: None,
                            tool_calls: None,
                        },
                        finish_reason: Some(finish),
                    }],
                };
                let _ =
                    tx.send(Ok(SseEvent::default()
                        .data(serde_json::to_string(&final_chunk).unwrap_or_default())))
                        .await;

                // [DONE] sentinel.
                let _ = tx.send(Ok(SseEvent::default().data("[DONE]"))).await;
            });

            let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
            Sse::new(stream)
                .keep_alive(KeepAlive::default())
                .into_response()
        }
        Err(e) => punch_error_to_oai_response(e),
    }
}

/// Handle a non-streaming chat completion request.
async fn handle_non_streaming(
    state: AppState,
    model: String,
    fighter_id: punch_types::FighterId,
    user_message: String,
    request_id: String,
    created: i64,
    stop_sequences: Vec<String>,
) -> axum::response::Response {
    let coordinator: Arc<dyn AgentCoordinator> =
        Arc::clone(&state.ring) as Arc<dyn AgentCoordinator>;

    match state
        .ring
        .send_message_with_coordinator(&fighter_id, user_message, Some(coordinator), vec![])
        .await
    {
        Ok(result) => {
            // Apply stop sequences.
            let (response_text, finish_reason) = if stop_sequences.is_empty() {
                (result.response.clone(), "stop".to_string())
            } else {
                let (text, reason) = apply_stop_sequences(&result.response, &stop_sequences);
                (text, reason.to_string())
            };

            let prompt_tokens = result.usage.input_tokens;
            let completion_tokens = result.usage.output_tokens;

            let response = ChatCompletionResponse {
                id: request_id,
                object: "chat.completion",
                created,
                model,
                choices: vec![Choice {
                    index: 0,
                    message: ChatMessage {
                        role: "assistant".to_string(),
                        content: Some(response_text),
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                    },
                    finish_reason,
                }],
                usage: Usage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens: result.usage.total(),
                },
            };
            Json(serde_json::to_value(&response).unwrap_or_default()).into_response()
        }
        Err(e) => punch_error_to_oai_response(e),
    }
}

/// Map a `PunchError` to an OpenAI-format error response.
fn punch_error_to_oai_response(e: punch_types::PunchError) -> axum::response::Response {
    let (status, code) = match &e {
        punch_types::PunchError::RateLimited { .. } => {
            (StatusCode::TOO_MANY_REQUESTS, Some("rate_limit_exceeded"))
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Some("internal_error")),
    };
    oai_error_response(status, e.to_string(), "api_error", code)
}

/// GET /v1/models -- list available models in OpenAI format.
///
/// Returns models from the ModelCatalog (if available), the configured
/// default model, and any active fighters as pseudo-models.
#[instrument(skip_all)]
async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let created = chrono::Utc::now().timestamp();
    let mut models: Vec<ModelObject> = Vec::new();

    // Pull models from the builtin model catalog.
    let catalog = ModelCatalog::with_builtin_models();
    for info in catalog.list_models() {
        models.push(ModelObject {
            id: info.id.clone(),
            object: "model",
            created,
            owned_by: info.provider.to_string(),
        });
    }

    // Always include the configured default model if not already listed.
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
        serde_json::to_value(ModelListResponse {
            object: "list",
            data: models,
        })
        .unwrap_or_default(),
    )
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -- Request deserialization tests --

    #[test]
    fn test_request_deserialization_minimal() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hello"}]
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert!(req.stream.is_none());
        assert!(req.max_tokens.is_none());
        assert!(req.temperature.is_none());
        assert!(req.top_p.is_none());
        assert!(req.stop.is_none());
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
    }

    #[test]
    fn test_request_deserialization_all_fields() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant"},
                {"role": "user", "content": "Hello", "name": "alice"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"NYC\"}"}}
                ]},
                {"role": "tool", "content": "72F sunny", "tool_call_id": "call_1"}
            ],
            "temperature": 0.7,
            "top_p": 0.95,
            "max_tokens": 4096,
            "stream": true,
            "stop": ["\n\n", "END"],
            "tools": [{"type": "function", "function": {"name": "get_weather", "description": "Get weather", "parameters": {"type": "object"}}}],
            "tool_choice": "auto"
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.messages.len(), 4);
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.top_p, Some(0.95));
        assert_eq!(req.max_tokens, Some(4096));
        assert_eq!(req.stream, Some(true));
        assert!(req.stop.is_some());
        assert!(req.tools.is_some());
        assert_eq!(req.tools.as_ref().unwrap().len(), 1);
        assert_eq!(req.messages[1].name, Some("alice".to_string()));
        assert!(req.messages[2].tool_calls.is_some());
        assert_eq!(req.messages[3].tool_call_id, Some("call_1".to_string()));
    }

    #[test]
    fn test_request_stop_single_string() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hello"}],
            "stop": "\n"
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        let stops = req.stop.unwrap().into_vec();
        assert_eq!(stops, vec!["\n"]);
    }

    // -- Response serialization tests --

    #[test]
    fn test_response_serialization() {
        let resp = ChatCompletionResponse {
            id: "chatcmpl-abc123".to_string(),
            object: "chat.completion",
            created: 1700000000,
            model: "gpt-4o".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some("Hello! How can I help?".to_string()),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: "stop".to_string(),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 8,
                total_tokens: 18,
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], "chatcmpl-abc123");
        assert_eq!(json["object"], "chat.completion");
        assert_eq!(json["choices"][0]["finish_reason"], "stop");
        assert_eq!(json["choices"][0]["message"]["role"], "assistant");
        assert_eq!(
            json["choices"][0]["message"]["content"],
            "Hello! How can I help?"
        );
        assert_eq!(json["usage"]["prompt_tokens"], 10);
        assert_eq!(json["usage"]["completion_tokens"], 8);
        assert_eq!(json["usage"]["total_tokens"], 18);
        // Optional fields should be omitted when None.
        assert!(json["choices"][0]["message"].get("tool_calls").is_none());
        assert!(json["choices"][0]["message"].get("name").is_none());
        assert!(json["choices"][0]["message"].get("tool_call_id").is_none());
    }

    #[test]
    fn test_response_with_tool_calls_serialization() {
        let resp = ChatCompletionResponse {
            id: "chatcmpl-xyz".to_string(),
            object: "chat.completion",
            created: 1700000000,
            model: "gpt-4o".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: None,
                    name: None,
                    tool_calls: Some(vec![ToolCallSpec {
                        id: Some("call_abc".to_string()),
                        call_type: Some("function".to_string()),
                        function: ToolCallFunctionSpec {
                            name: Some("get_weather".to_string()),
                            arguments: Some(r#"{"city":"NYC"}"#.to_string()),
                        },
                        index: Some(0),
                    }]),
                    tool_call_id: None,
                },
                finish_reason: "tool_calls".to_string(),
            }],
            usage: Usage {
                prompt_tokens: 15,
                completion_tokens: 20,
                total_tokens: 35,
            },
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["choices"][0]["finish_reason"], "tool_calls");
        assert!(json["choices"][0]["message"].get("content").is_none());
        let tc = &json["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(tc["id"], "call_abc");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "get_weather");
        assert_eq!(tc["function"]["arguments"], r#"{"city":"NYC"}"#);
    }

    // -- Message format conversion tests --

    #[test]
    fn test_oai_to_punch_user_message() {
        let oai = ChatMessage {
            role: "user".to_string(),
            content: Some("Hello world".to_string()),
            name: Some("alice".to_string()),
            tool_calls: None,
            tool_call_id: None,
        };
        let punch = oai_message_to_punch(&oai);
        assert_eq!(punch.role, punch_types::Role::User);
        assert_eq!(punch.content, "Hello world");
        assert!(punch.tool_calls.is_empty());
    }

    #[test]
    fn test_oai_to_punch_tool_message() {
        let oai = ChatMessage {
            role: "tool".to_string(),
            content: Some("72F and sunny".to_string()),
            name: None,
            tool_calls: None,
            tool_call_id: Some("call_123".to_string()),
        };
        let punch = oai_message_to_punch(&oai);
        assert_eq!(punch.role, punch_types::Role::Tool);
        assert_eq!(punch.tool_results.len(), 1);
        assert_eq!(punch.tool_results[0].id, "call_123");
        assert_eq!(punch.tool_results[0].content, "72F and sunny");
    }

    #[test]
    fn test_oai_to_punch_assistant_with_tool_calls() {
        let oai = ChatMessage {
            role: "assistant".to_string(),
            content: None,
            name: None,
            tool_calls: Some(vec![ToolCallSpec {
                id: Some("call_abc".to_string()),
                call_type: Some("function".to_string()),
                function: ToolCallFunctionSpec {
                    name: Some("search".to_string()),
                    arguments: Some(r#"{"query":"rust"}"#.to_string()),
                },
                index: None,
            }]),
            tool_call_id: None,
        };
        let punch = oai_message_to_punch(&oai);
        assert_eq!(punch.role, punch_types::Role::Assistant);
        assert_eq!(punch.tool_calls.len(), 1);
        assert_eq!(punch.tool_calls[0].id, "call_abc");
        assert_eq!(punch.tool_calls[0].name, "search");
        assert_eq!(punch.tool_calls[0].input["query"], "rust");
    }

    #[test]
    fn test_punch_to_oai_roundtrip() {
        let punch_msg = punch_types::Message {
            role: punch_types::Role::Assistant,
            content: "Here is the result".to_string(),
            tool_calls: vec![punch_types::ToolCall {
                id: "call_1".to_string(),
                name: "calculator".to_string(),
                input: serde_json::json!({"expression": "2+2"}),
            }],
            tool_results: vec![],
            content_parts: Vec::new(),
            timestamp: chrono::Utc::now(),
        };
        let oai = punch_message_to_oai(&punch_msg);
        assert_eq!(oai.role, "assistant");
        assert_eq!(oai.content, Some("Here is the result".to_string()));
        let tc = oai.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, Some("call_1".to_string()));
        assert_eq!(tc[0].call_type, Some("function".to_string()));
        assert_eq!(tc[0].function.name, Some("calculator".to_string()));
    }

    #[test]
    fn test_punch_to_oai_empty_content() {
        let punch_msg = punch_types::Message::new(punch_types::Role::Assistant, "");
        let oai = punch_message_to_oai(&punch_msg);
        assert!(oai.content.is_none());
        assert!(oai.tool_calls.is_none());
    }

    // -- Model listing test --

    #[test]
    fn test_model_list_response_serialization() {
        let resp = ModelListResponse {
            object: "list",
            data: vec![
                ModelObject {
                    id: "gpt-4o".to_string(),
                    object: "model",
                    created: 1700000000,
                    owned_by: "openai".to_string(),
                },
                ModelObject {
                    id: "claude-sonnet-4-20250514".to_string(),
                    object: "model",
                    created: 1700000000,
                    owned_by: "anthropic".to_string(),
                },
            ],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["object"], "list");
        assert_eq!(json["data"].as_array().unwrap().len(), 2);
        assert_eq!(json["data"][0]["id"], "gpt-4o");
        assert_eq!(json["data"][0]["object"], "model");
        assert_eq!(json["data"][1]["owned_by"], "anthropic");
    }

    // -- Streaming chunk format test --

    #[test]
    fn test_streaming_chunk_format() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion.chunk",
            created: 1700000000,
            model: "gpt-4o".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: Some("Hello".to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
        };
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json["object"], "chat.completion.chunk");
        assert_eq!(json["choices"][0]["delta"]["content"], "Hello");
        assert!(json["choices"][0]["finish_reason"].is_null());
        // role should be omitted when None
        assert!(json["choices"][0]["delta"].get("role").is_none());
    }

    #[test]
    fn test_streaming_role_chunk() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion.chunk",
            created: 1700000000,
            model: "gpt-4o".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: Some("assistant".to_string()),
                    content: None,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
        };
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json["choices"][0]["delta"]["role"], "assistant");
        assert!(json["choices"][0]["delta"].get("content").is_none());
    }

    // -- Auth header validation test --

    #[test]
    fn test_bearer_token_extraction() {
        // This tests the pattern used by the auth middleware.
        let header_value = "Bearer sk-test-key-123";
        let token = header_value.strip_prefix("Bearer ");
        assert_eq!(token, Some("sk-test-key-123"));

        let invalid = "Basic dXNlcjpwYXNz";
        let token = invalid.strip_prefix("Bearer ");
        assert!(token.is_none());

        let empty = "";
        let token = empty.strip_prefix("Bearer ");
        assert!(token.is_none());
    }

    // -- Stop sequence handling test --

    #[test]
    fn test_stop_sequence_applied() {
        let text = "Hello world\n\nThis is after the stop";
        let (result, reason) = apply_stop_sequences(text, &["\n\n".to_string()]);
        assert_eq!(result, "Hello world");
        assert_eq!(reason, "stop");
    }

    #[test]
    fn test_stop_sequence_no_match() {
        let text = "Hello world, no stop here";
        let (result, reason) = apply_stop_sequences(text, &["XYZZY".to_string()]);
        assert_eq!(result, "Hello world, no stop here");
        assert_eq!(reason, "stop");
    }

    #[test]
    fn test_stop_sequence_first_match_wins() {
        let text = "Hello END world STOP done";
        let (result, _) = apply_stop_sequences(text, &["END".to_string(), "STOP".to_string()]);
        assert_eq!(result, "Hello ");
    }

    // -- Token estimation tests --

    #[test]
    fn test_estimate_prompt_tokens() {
        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: Some("You are helpful".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: Some("Hi".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        let tokens = estimate_prompt_tokens(&messages);
        // Should be a reasonable positive number.
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_completion_tokens() {
        let text = "This is a sample response from the model.";
        let tokens = estimate_completion_tokens(text);
        assert!(tokens > 0);
        // ~42 chars / 4 = ~10 tokens
        assert_eq!(tokens, 10);
    }

    // -- Completion ID format test --

    #[test]
    fn test_completion_id_format() {
        let id = generate_completion_id();
        assert!(id.starts_with("chatcmpl-"));
        assert!(id.len() > "chatcmpl-".len());
        // Should be a valid UUID after the prefix.
        let uuid_part = &id["chatcmpl-".len()..];
        assert!(uuid::Uuid::parse_str(uuid_part).is_ok());
    }

    // -- Temperature / top_p defaults test --

    #[test]
    fn test_temperature_defaults() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}]
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert!(req.temperature.is_none());
        assert!(req.top_p.is_none());
        // When temperature is not set, the model should use its own default.
    }

    #[test]
    fn test_temperature_explicit_zero() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "temperature": 0.0,
            "top_p": 1.0
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.temperature, Some(0.0));
        assert_eq!(req.top_p, Some(1.0));
    }

    // -- Tool spec deserialization test --

    #[test]
    fn test_tool_spec_deserialization() {
        let json = r#"{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get the current weather",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {"type": "string"}
                    },
                    "required": ["location"]
                }
            }
        }"#;
        let tool: ToolSpec = serde_json::from_str(json).unwrap();
        assert_eq!(tool.tool_type, "function");
        assert_eq!(tool.function.name, "get_weather");
        assert!(tool.function.description.is_some());
        assert!(tool.function.parameters.is_some());
    }

    // -- Missing model handling (deserialization should succeed; routing is tested in integration) --

    #[test]
    fn test_unknown_model_accepted_in_request() {
        let json = r#"{
            "model": "nonexistent-model-42",
            "messages": [{"role": "user", "content": "Hello"}]
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "nonexistent-model-42");
    }

    // -- System message conversion --

    #[test]
    fn test_oai_to_punch_system_message() {
        let oai = ChatMessage {
            role: "system".to_string(),
            content: Some("You are a helpful coding assistant.".to_string()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        };
        let punch = oai_message_to_punch(&oai);
        assert_eq!(punch.role, punch_types::Role::System);
        assert_eq!(punch.content, "You are a helpful coding assistant.");
    }

    #[test]
    fn test_oai_to_punch_assistant_message() {
        let oai = ChatMessage {
            role: "assistant".to_string(),
            content: Some("Here is my response.".to_string()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        };
        let punch = oai_message_to_punch(&oai);
        assert_eq!(punch.role, punch_types::Role::Assistant);
        assert_eq!(punch.content, "Here is my response.");
    }

    #[test]
    fn test_oai_to_punch_unknown_role_defaults_to_user() {
        let oai = ChatMessage {
            role: "developer".to_string(),
            content: Some("test content".to_string()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        };
        let punch = oai_message_to_punch(&oai);
        assert_eq!(punch.role, punch_types::Role::User);
    }

    #[test]
    fn test_oai_to_punch_none_content() {
        let oai = ChatMessage {
            role: "user".to_string(),
            content: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
        };
        let punch = oai_message_to_punch(&oai);
        assert_eq!(punch.content, "");
    }

    // -- Punch to OAI conversions --

    #[test]
    fn test_punch_to_oai_system() {
        let msg = punch_types::Message::new(punch_types::Role::System, "system prompt");
        let oai = punch_message_to_oai(&msg);
        assert_eq!(oai.role, "system");
        assert_eq!(oai.content, Some("system prompt".to_string()));
    }

    #[test]
    fn test_punch_to_oai_user() {
        let msg = punch_types::Message::new(punch_types::Role::User, "hello");
        let oai = punch_message_to_oai(&msg);
        assert_eq!(oai.role, "user");
        assert_eq!(oai.content, Some("hello".to_string()));
    }

    #[test]
    fn test_punch_to_oai_tool_result() {
        let mut msg = punch_types::Message::new(punch_types::Role::Tool, "result data");
        msg.tool_results = vec![punch_types::ToolCallResult {
            id: "call_xyz".to_string(),
            content: "result data".to_string(),
            is_error: false,
            image: None,
        }];
        let oai = punch_message_to_oai(&msg);
        assert_eq!(oai.role, "tool");
        assert_eq!(oai.tool_call_id, Some("call_xyz".to_string()));
    }

    // -- Stop sequence edge cases --

    #[test]
    fn test_stop_sequence_empty_list() {
        let text = "Hello world";
        let (result, reason) = apply_stop_sequences(text, &[]);
        assert_eq!(result, "Hello world");
        assert_eq!(reason, "stop");
    }

    #[test]
    fn test_stop_sequence_at_start() {
        let text = "STOP the rest";
        let (result, _) = apply_stop_sequences(text, &["STOP".to_string()]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_stop_sequence_empty_text() {
        let text = "";
        let (result, reason) = apply_stop_sequences(text, &["stop".to_string()]);
        assert_eq!(result, "");
        assert_eq!(reason, "stop");
    }

    #[test]
    fn test_stop_sequence_multiple_strings() {
        let stop = StopSequence::Multiple(vec!["END".to_string(), "DONE".to_string()]);
        let vec = stop.into_vec();
        assert_eq!(vec.len(), 2);
        assert_eq!(vec[0], "END");
        assert_eq!(vec[1], "DONE");
    }

    #[test]
    fn test_stop_sequence_single_string() {
        let stop = StopSequence::Single("STOP".to_string());
        let vec = stop.into_vec();
        assert_eq!(vec.len(), 1);
        assert_eq!(vec[0], "STOP");
    }

    // -- Temperature edge cases --

    #[test]
    fn test_temperature_max_value() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "temperature": 2.0
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.temperature, Some(2.0));
    }

    #[test]
    fn test_max_tokens_handling() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "max_tokens": 1
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.max_tokens, Some(1));
    }

    #[test]
    fn test_max_tokens_large_value() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "max_tokens": 128000
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.max_tokens, Some(128000));
    }

    // -- Empty messages --

    #[test]
    fn test_empty_messages_array() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": []
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert!(req.messages.is_empty());
    }

    // -- Tool call serialization --

    #[test]
    fn test_tool_call_spec_serialization() {
        let tc = ToolCallSpec {
            id: Some("call_99".to_string()),
            call_type: Some("function".to_string()),
            function: ToolCallFunctionSpec {
                name: Some("calculator".to_string()),
                arguments: Some(r#"{"x":1,"y":2}"#.to_string()),
            },
            index: Some(0),
        };
        let json = serde_json::to_value(&tc).unwrap();
        assert_eq!(json["id"], "call_99");
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "calculator");
        assert_eq!(json["index"], 0);
    }

    #[test]
    fn test_tool_call_spec_minimal() {
        let tc = ToolCallSpec {
            id: None,
            call_type: None,
            function: ToolCallFunctionSpec {
                name: None,
                arguments: None,
            },
            index: None,
        };
        let json = serde_json::to_value(&tc).unwrap();
        // Optional fields should be omitted
        assert!(json.get("id").is_none());
        assert!(json.get("type").is_none());
    }

    // -- Streaming chunk with finish reason --

    #[test]
    fn test_streaming_chunk_finish_reason() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-done".to_string(),
            object: "chat.completion.chunk",
            created: 1700000000,
            model: "gpt-4o".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
        };
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json["choices"][0]["finish_reason"], "stop");
        // Both role and content should be omitted
        assert!(json["choices"][0]["delta"].get("role").is_none());
        assert!(json["choices"][0]["delta"].get("content").is_none());
    }

    #[test]
    fn test_streaming_chunk_with_tool_calls() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-tc".to_string(),
            object: "chat.completion.chunk",
            created: 1700000000,
            model: "gpt-4o".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![ToolCallSpec {
                        id: Some("call_delta".to_string()),
                        call_type: Some("function".to_string()),
                        function: ToolCallFunctionSpec {
                            name: Some("search".to_string()),
                            arguments: Some("{}".to_string()),
                        },
                        index: Some(0),
                    }]),
                },
                finish_reason: None,
            }],
        };
        let json = serde_json::to_value(&chunk).unwrap();
        assert!(json["choices"][0]["delta"]["tool_calls"].is_array());
    }

    // -- Model object format --

    #[test]
    fn test_model_object_format() {
        let model = ModelObject {
            id: "gpt-4o-mini".to_string(),
            object: "model",
            created: 1700000000,
            owned_by: "openai".to_string(),
        };
        let json = serde_json::to_value(&model).unwrap();
        assert_eq!(json["id"], "gpt-4o-mini");
        assert_eq!(json["object"], "model");
        assert_eq!(json["owned_by"], "openai");
        assert!(json["created"].is_number());
    }

    // -- Token estimation --

    #[test]
    fn test_estimate_prompt_tokens_empty() {
        let tokens = estimate_prompt_tokens(&[]);
        assert_eq!(tokens, 0);
    }

    #[test]
    fn test_estimate_prompt_tokens_with_name() {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("Hello".to_string()),
            name: Some("alice".to_string()),
            tool_calls: None,
            tool_call_id: None,
        }];
        let tokens = estimate_prompt_tokens(&messages);
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_completion_tokens_empty() {
        let tokens = estimate_completion_tokens("");
        assert_eq!(tokens, 0);
    }

    // -- Completion ID uniqueness --

    #[test]
    fn test_completion_ids_are_unique() {
        let id1 = generate_completion_id();
        let id2 = generate_completion_id();
        assert_ne!(id1, id2);
    }

    // -- OAI error response format --

    #[test]
    fn test_oai_error_serialization() {
        let err = OaiErrorResponse {
            error: OaiError {
                message: "Rate limit exceeded".to_string(),
                r#type: "rate_limit_error".to_string(),
                code: Some("rate_limit_exceeded".to_string()),
            },
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["error"]["message"], "Rate limit exceeded");
        assert_eq!(json["error"]["type"], "rate_limit_error");
        assert_eq!(json["error"]["code"], "rate_limit_exceeded");
    }

    #[test]
    fn test_oai_error_no_code() {
        let err = OaiErrorResponse {
            error: OaiError {
                message: "Internal error".to_string(),
                r#type: "api_error".to_string(),
                code: None,
            },
        };
        let json = serde_json::to_value(&err).unwrap();
        assert!(json["error"]["code"].is_null());
    }

    // -- Usage stats --

    #[test]
    fn test_usage_serialization() {
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        let json = serde_json::to_value(&usage).unwrap();
        assert_eq!(json["prompt_tokens"], 100);
        assert_eq!(json["completion_tokens"], 50);
        assert_eq!(json["total_tokens"], 150);
    }

    // -- Chat message serialization --

    #[test]
    fn test_chat_message_none_fields_skipped() {
        let msg = ChatMessage {
            role: "user".to_string(),
            content: Some("Hello".to_string()),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert!(json.get("name").is_none());
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("tool_call_id").is_none());
    }

    // -- Tool function spec --

    #[test]
    fn test_tool_function_spec_no_params() {
        let json = r#"{
            "type": "function",
            "function": {
                "name": "get_time"
            }
        }"#;
        let tool: ToolSpec = serde_json::from_str(json).unwrap();
        assert_eq!(tool.function.name, "get_time");
        assert!(tool.function.description.is_none());
        assert!(tool.function.parameters.is_none());
    }

    // -- Stream option deserialization --

    #[test]
    fn test_stream_false() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "stream": false
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.stream, Some(false));
    }

    // -- Tool choice deserialization --

    #[test]
    fn test_tool_choice_auto() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "tool_choice": "auto"
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.tool_choice, Some(serde_json::json!("auto")));
    }

    #[test]
    fn test_tool_choice_none_value() {
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hi"}],
            "tool_choice": "none"
        }"#;
        let req: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.tool_choice, Some(serde_json::json!("none")));
    }
}
