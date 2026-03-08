//! LLM driver trait and provider implementations.
//!
//! The [`LlmDriver`] trait abstracts over different LLM providers so the
//! fighter loop is provider-agnostic. Concrete implementations handle the
//! wire format differences between Anthropic, OpenAI-compatible APIs, etc.

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use punch_types::{
    Message, ModelConfig, Provider, PunchError, PunchResult, Role, ToolCall, ToolDefinition,
};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its turn naturally.
    EndTurn,
    /// The model wants to invoke one or more tools.
    ToolUse,
    /// The response was truncated due to max_tokens.
    MaxTokens,
    /// An error occurred during generation.
    Error,
}

/// Token usage statistics for a single completion.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl TokenUsage {
    /// Add another usage on top of this one (accumulator).
    pub fn accumulate(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }

    /// Total tokens consumed.
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// A request to the LLM for a completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// Model identifier (e.g. "claude-sonnet-4-20250514").
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Tools available for the model to call.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// System prompt (separate from messages for providers that support it).
    pub system_prompt: Option<String>,
}

/// The response from an LLM completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// The assistant message (may contain tool calls).
    pub message: Message,
    /// Token usage for this completion.
    pub usage: TokenUsage,
    /// Why the model stopped.
    pub stop_reason: StopReason,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over LLM providers.
#[async_trait]
pub trait LlmDriver: Send + Sync + 'static {
    /// Send a completion request and return the response.
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse>;

    /// Streaming variant. Default implementation falls back to `complete`.
    async fn stream_complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        self.complete(request).await
    }
}

// ---------------------------------------------------------------------------
// Anthropic driver
// ---------------------------------------------------------------------------

/// Driver for the Anthropic Messages API (api.anthropic.com).
pub struct AnthropicDriver {
    client: Client,
    api_key: String,
    base_url: String,
}

impl AnthropicDriver {
    /// Create a new Anthropic driver.
    ///
    /// `api_key` is the raw key value, not the env var name.
    pub fn new(api_key: String, base_url: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string()),
        }
    }

    /// Build the Anthropic API request body from our internal types.
    fn build_request_body(&self, request: &CompletionRequest) -> serde_json::Value {
        let mut messages = Vec::new();

        for msg in &request.messages {
            match msg.role {
                Role::User => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": msg.content,
                    }));
                }
                Role::Assistant => {
                    let mut content_blocks: Vec<serde_json::Value> = Vec::new();

                    if !msg.content.is_empty() {
                        content_blocks.push(serde_json::json!({
                            "type": "text",
                            "text": msg.content,
                        }));
                    }

                    for tc in &msg.tool_calls {
                        content_blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.input,
                        }));
                    }

                    if content_blocks.is_empty() {
                        content_blocks.push(serde_json::json!({
                            "type": "text",
                            "text": "",
                        }));
                    }

                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content_blocks,
                    }));
                }
                Role::Tool => {
                    let mut result_blocks: Vec<serde_json::Value> = Vec::new();
                    for tr in &msg.tool_results {
                        result_blocks.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tr.id,
                            "content": tr.content,
                            "is_error": tr.is_error,
                        }));
                    }
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": result_blocks,
                    }));
                }
                Role::System => {
                    // System messages are handled via the top-level `system` param;
                    // skip them in the messages array.
                }
            }
        }

        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
        });

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if let Some(ref system) = request.system_prompt {
            body["system"] = serde_json::json!(system);
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
        }

        body
    }

    /// Parse the Anthropic API response into our internal types.
    fn parse_response(&self, body: &serde_json::Value) -> PunchResult<CompletionResponse> {
        let stop_reason = match body["stop_reason"].as_str() {
            Some("end_turn") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            _ => StopReason::Error,
        };

        let usage = TokenUsage {
            input_tokens: body["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: body["usage"]["output_tokens"].as_u64().unwrap_or(0),
        };

        let mut text_content = String::new();
        let mut tool_calls = Vec::new();

        if let Some(content_array) = body["content"].as_array() {
            for block in content_array {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = block["text"].as_str() {
                            if !text_content.is_empty() {
                                text_content.push('\n');
                            }
                            text_content.push_str(text);
                        }
                    }
                    Some("tool_use") => {
                        tool_calls.push(ToolCall {
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                            input: block["input"].clone(),
                        });
                    }
                    _ => {}
                }
            }
        }

        let message = Message {
            role: Role::Assistant,
            content: text_content,
            tool_calls,
            tool_results: Vec::new(),
            timestamp: chrono::Utc::now(),
        };

        Ok(CompletionResponse {
            message,
            usage,
            stop_reason,
        })
    }
}

#[async_trait]
impl LlmDriver for AnthropicDriver {
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let url = format!("{}/v1/messages", self.base_url);
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Provider {
                provider: "anthropic".to_string(),
                message: format!("request failed: {e}"),
            })?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60)
                * 1000;

            return Err(PunchError::RateLimited {
                provider: "anthropic".to_string(),
                retry_after_ms: retry_after,
            });
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(PunchError::Auth(
                "anthropic API key is invalid or lacks permissions".to_string(),
            ));
        }

        let response_body: serde_json::Value =
            response.json().await.map_err(|e| PunchError::Provider {
                provider: "anthropic".to_string(),
                message: format!("failed to parse response: {e}"),
            })?;

        if !status.is_success() {
            let error_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(PunchError::Provider {
                provider: "anthropic".to_string(),
                message: format!("API error ({}): {}", status, error_msg),
            });
        }

        self.parse_response(&response_body)
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible driver
// ---------------------------------------------------------------------------

/// Driver for OpenAI-compatible chat completions APIs.
///
/// Works with OpenAI, Groq, DeepSeek, Ollama, Together, Fireworks,
/// Cerebras, xAI, Mistral, and any other provider exposing the
/// `/v1/chat/completions` endpoint.
pub struct OpenAiCompatibleDriver {
    client: Client,
    api_key: String,
    base_url: String,
    provider_name: String,
}

impl OpenAiCompatibleDriver {
    /// Create a new OpenAI-compatible driver.
    pub fn new(api_key: String, base_url: String, provider_name: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url,
            provider_name,
        }
    }

    /// Build the OpenAI chat completions request body.
    fn build_request_body(&self, request: &CompletionRequest) -> serde_json::Value {
        let mut messages = Vec::new();

        // System prompt as a system message.
        if let Some(ref system) = request.system_prompt {
            messages.push(serde_json::json!({
                "role": "system",
                "content": system,
            }));
        }

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    messages.push(serde_json::json!({
                        "role": "system",
                        "content": msg.content,
                    }));
                }
                Role::User => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": msg.content,
                    }));
                }
                Role::Assistant => {
                    let mut m = serde_json::json!({
                        "role": "assistant",
                    });

                    if !msg.content.is_empty() {
                        m["content"] = serde_json::json!(msg.content);
                    }

                    if !msg.tool_calls.is_empty() {
                        let tc: Vec<serde_json::Value> = msg
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.input.to_string(),
                                    },
                                })
                            })
                            .collect();
                        m["tool_calls"] = serde_json::json!(tc);
                    }

                    messages.push(m);
                }
                Role::Tool => {
                    for tr in &msg.tool_results {
                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": tr.id,
                            "content": tr.content,
                        }));
                    }
                }
            }
        }

        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    },
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
        });

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
        }

        body
    }

    /// Parse the OpenAI chat completions response.
    fn parse_response(&self, body: &serde_json::Value) -> PunchResult<CompletionResponse> {
        let choice = body["choices"].get(0).ok_or_else(|| PunchError::Provider {
            provider: self.provider_name.clone(),
            message: "no choices in response".to_string(),
        })?;

        let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");
        let stop_reason = match finish_reason {
            "stop" => StopReason::EndTurn,
            "tool_calls" => StopReason::ToolUse,
            "length" => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

        let msg = &choice["message"];
        let content = msg["content"].as_str().unwrap_or("").to_string();

        let mut tool_calls = Vec::new();
        if let Some(tc_array) = msg["tool_calls"].as_array() {
            for tc in tc_array {
                let id = tc["id"].as_str().unwrap_or_default().to_string();
                let name = tc["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                let input: serde_json::Value =
                    serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));

                tool_calls.push(ToolCall { id, name, input });
            }
        }

        let usage = TokenUsage {
            input_tokens: body["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: body["usage"]["completion_tokens"].as_u64().unwrap_or(0),
        };

        // If there are tool calls but finish_reason was not "tool_calls", fix it up.
        let stop_reason = if !tool_calls.is_empty() && stop_reason != StopReason::ToolUse {
            StopReason::ToolUse
        } else {
            stop_reason
        };

        let message = Message {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_results: Vec::new(),
            timestamp: chrono::Utc::now(),
        };

        Ok(CompletionResponse {
            message,
            usage,
            stop_reason,
        })
    }
}

#[async_trait]
impl LlmDriver for OpenAiCompatibleDriver {
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(&url)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Provider {
                provider: self.provider_name.clone(),
                message: format!("request failed: {e}"),
            })?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60)
                * 1000;

            return Err(PunchError::RateLimited {
                provider: self.provider_name.clone(),
                retry_after_ms: retry_after,
            });
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(PunchError::Auth(format!(
                "{} API key is invalid or lacks permissions",
                self.provider_name
            )));
        }

        let response_body: serde_json::Value =
            response.json().await.map_err(|e| PunchError::Provider {
                provider: self.provider_name.clone(),
                message: format!("failed to parse response: {e}"),
            })?;

        if !status.is_success() {
            let error_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(PunchError::Provider {
                provider: self.provider_name.clone(),
                message: format!("API error ({}): {}", status, error_msg),
            });
        }

        self.parse_response(&response_body)
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Default base URLs for known providers.
fn default_base_url(provider: &Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "https://api.anthropic.com",
        Provider::OpenAI => "https://api.openai.com",
        Provider::Google => "https://generativelanguage.googleapis.com",
        Provider::Groq => "https://api.groq.com/openai",
        Provider::DeepSeek => "https://api.deepseek.com",
        Provider::Ollama => "http://localhost:11434",
        Provider::Mistral => "https://api.mistral.ai",
        Provider::Together => "https://api.together.xyz",
        Provider::Fireworks => "https://api.fireworks.ai/inference",
        Provider::Cerebras => "https://api.cerebras.ai",
        Provider::XAI => "https://api.x.ai",
        Provider::Cohere => "https://api.cohere.ai",
        Provider::Custom(_) => "",
    }
}

/// Create an [`LlmDriver`] from a [`ModelConfig`].
///
/// Reads the API key from the environment variable specified in
/// `config.api_key_env`. Returns an error if the env var is missing
/// (except for Ollama which does not require auth).
pub fn create_driver(config: &ModelConfig) -> PunchResult<Arc<dyn LlmDriver>> {
    let api_key = match &config.api_key_env {
        Some(env_var) => std::env::var(env_var).map_err(|_| {
            PunchError::Auth(format!(
                "environment variable '{}' not set for {} driver",
                env_var, config.provider
            ))
        })?,
        None => {
            // Ollama typically has no auth; others will fail at the API.
            String::new()
        }
    };

    let base_url = config
        .base_url
        .clone()
        .unwrap_or_else(|| default_base_url(&config.provider).to_string());

    match &config.provider {
        Provider::Anthropic => Ok(Arc::new(AnthropicDriver::new(api_key, Some(base_url)))),
        provider => {
            let name = provider.to_string();
            Ok(Arc::new(OpenAiCompatibleDriver::new(
                api_key, base_url, name,
            )))
        }
    }
}
