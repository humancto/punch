//! LLM driver trait and provider implementations.
//!
//! The [`LlmDriver`] trait abstracts over different LLM providers so the
//! fighter loop is provider-agnostic. Concrete implementations handle the
//! wire format differences between Anthropic, OpenAI-compatible APIs, etc.

use std::sync::Arc;

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
// Think-tag stripping
// ---------------------------------------------------------------------------

/// Strip reasoning/thinking tags from LLM responses.
///
/// Many reasoning models (Qwen, DeepSeek, etc.) wrap internal chain-of-thought
/// in `<think>...</think>`, `<thinking>...</thinking>`, or `<reasoning>...</reasoning>`
/// tags. This function extracts only the visible output.
///
/// If the entire response is inside think tags (no visible output), returns
/// the original content unchanged so the user still sees something.
pub fn strip_thinking_tags(content: &str) -> String {
    let mut result = content.to_string();

    // Strip all known thinking tag variants
    for tag in &["think", "thinking", "reasoning", "reflection"] {
        let open = format!("<{}>", tag);
        let close = format!("</{}>", tag);

        // Remove all occurrences of <tag>...</tag> blocks
        while let Some(start) = result.find(&open) {
            if let Some(end) = result[start..].find(&close) {
                let block_end = start + end + close.len();
                result = format!("{}{}", &result[..start], &result[block_end..]);
            } else {
                // Unclosed tag — remove from open tag to end
                result = result[..start].to_string();
                break;
            }
        }
    }

    let trimmed = result.trim().to_string();

    // If stripping removed everything, return original content
    // (the model used all tokens for thinking)
    if trimmed.is_empty() {
        content.to_string()
    } else {
        trimmed
    }
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
    async fn stream_complete(
        &self,
        request: CompletionRequest,
    ) -> PunchResult<CompletionResponse> {
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

    /// Create a new Anthropic driver with a shared HTTP client.
    ///
    /// This allows connection pooling across all drivers.
    pub fn with_client(client: Client, api_key: String, base_url: Option<String>) -> Self {
        Self {
            client,
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

        // Strip thinking tags from reasoning models
        let text_content = strip_thinking_tags(&text_content);

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
/// Works with OpenAI, Groq, DeepSeek, Together, Fireworks,
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

    /// Create a new OpenAI-compatible driver with a shared HTTP client.
    pub fn with_client(
        client: Client,
        api_key: String,
        base_url: String,
        provider_name: String,
    ) -> Self {
        Self {
            client,
            api_key,
            base_url,
            provider_name,
        }
    }

    /// Build the OpenAI chat completions request body.
    pub fn build_request_body(&self, request: &CompletionRequest) -> serde_json::Value {
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
    pub fn parse_response(&self, body: &serde_json::Value) -> PunchResult<CompletionResponse> {
        let choice = body["choices"]
            .get(0)
            .ok_or_else(|| PunchError::Provider {
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
        let raw_content = msg["content"].as_str().unwrap_or("");
        // Strip thinking tags from reasoning models (Qwen, DeepSeek R1, etc.)
        let content = strip_thinking_tags(raw_content);

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
// Gemini driver
// ---------------------------------------------------------------------------

/// Driver for the Google Gemini (Generative Language) API.
pub struct GeminiDriver {
    client: Client,
    api_key: String,
    base_url: String,
}

impl GeminiDriver {
    /// Create a new Gemini driver.
    pub fn new(api_key: String, base_url: Option<String>) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: base_url
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string()),
        }
    }

    /// Create a new Gemini driver with a shared HTTP client.
    pub fn with_client(client: Client, api_key: String, base_url: Option<String>) -> Self {
        Self {
            client,
            api_key,
            base_url: base_url
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string()),
        }
    }

    /// Build the Gemini API request body.
    pub fn build_request_body(&self, request: &CompletionRequest) -> serde_json::Value {
        let mut contents = Vec::new();
        let mut system_text: Option<String> = request.system_prompt.clone();

        for msg in &request.messages {
            match msg.role {
                Role::System => {
                    // Gemini does not have a system role; prepend to first user message.
                    let existing = system_text.take().unwrap_or_default();
                    let combined = if existing.is_empty() {
                        msg.content.clone()
                    } else {
                        format!("{}\n{}", existing, msg.content)
                    };
                    system_text = Some(combined);
                }
                Role::User => {
                    let mut text = String::new();
                    if let Some(sys) = system_text.take()
                        && !sys.is_empty()
                    {
                        text.push_str(&sys);
                        text.push_str("\n\n");
                    }
                    text.push_str(&msg.content);
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": [{"text": text}],
                    }));
                }
                Role::Assistant => {
                    let mut parts: Vec<serde_json::Value> = Vec::new();
                    if !msg.content.is_empty() {
                        parts.push(serde_json::json!({"text": msg.content}));
                    }
                    for tc in &msg.tool_calls {
                        parts.push(serde_json::json!({
                            "functionCall": {
                                "name": tc.name,
                                "args": tc.input,
                            }
                        }));
                    }
                    if parts.is_empty() {
                        parts.push(serde_json::json!({"text": ""}));
                    }
                    contents.push(serde_json::json!({
                        "role": "model",
                        "parts": parts,
                    }));
                }
                Role::Tool => {
                    let mut parts: Vec<serde_json::Value> = Vec::new();
                    for tr in &msg.tool_results {
                        parts.push(serde_json::json!({
                            "functionResponse": {
                                "name": tr.id.clone(),
                                "response": {"content": tr.content},
                            }
                        }));
                    }
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": parts,
                    }));
                }
            }
        }

        // If we still have an unused system prompt (no user messages yet), add it.
        if let Some(sys) = system_text
            && !sys.is_empty()
        {
            contents.insert(
                0,
                serde_json::json!({
                    "role": "user",
                    "parts": [{"text": sys}],
                }),
            );
        }

        let mut body = serde_json::json!({
            "contents": contents,
        });

        let mut gen_config = serde_json::json!({
            "maxOutputTokens": request.max_tokens,
        });
        if let Some(temp) = request.temperature {
            gen_config["temperature"] = serde_json::json!(temp);
        }
        body["generationConfig"] = gen_config;

        if !request.tools.is_empty() {
            let func_decls: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!([{"function_declarations": func_decls}]);
        }

        body
    }

    /// Build the full URL for a Gemini request.
    pub fn build_url(&self, model: &str) -> String {
        format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.base_url.trim_end_matches('/'),
            model,
            self.api_key,
        )
    }

    /// Parse the Gemini API response.
    pub fn parse_response(&self, body: &serde_json::Value) -> PunchResult<CompletionResponse> {
        let candidate = body["candidates"]
            .get(0)
            .ok_or_else(|| PunchError::Provider {
                provider: "gemini".to_string(),
                message: "no candidates in response".to_string(),
            })?;

        let parts = candidate["content"]["parts"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut text_content = String::new();
        let mut tool_calls = Vec::new();

        for part in &parts {
            if let Some(text) = part["text"].as_str() {
                if !text_content.is_empty() {
                    text_content.push('\n');
                }
                text_content.push_str(text);
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc["name"].as_str().unwrap_or_default().to_string();
                let args = fc["args"].clone();
                tool_calls.push(ToolCall {
                    id: format!("gemini-{}", uuid::Uuid::new_v4()),
                    name,
                    input: args,
                });
            }
        }

        let finish_reason = candidate["finishReason"].as_str().unwrap_or("STOP");
        let stop_reason = if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            match finish_reason {
                "STOP" => StopReason::EndTurn,
                "MAX_TOKENS" => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            }
        };

        let usage = TokenUsage {
            input_tokens: body["usageMetadata"]["promptTokenCount"]
                .as_u64()
                .unwrap_or(0),
            output_tokens: body["usageMetadata"]["candidatesTokenCount"]
                .as_u64()
                .unwrap_or(0),
        };

        // Strip thinking tags from reasoning models
        let text_content = strip_thinking_tags(&text_content);

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
impl LlmDriver for GeminiDriver {
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let url = self.build_url(&request.model);
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Provider {
                provider: "gemini".to_string(),
                message: format!("request failed: {e}"),
            })?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(PunchError::RateLimited {
                provider: "gemini".to_string(),
                retry_after_ms: 60_000,
            });
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(PunchError::Auth(
                "Gemini API key is invalid or lacks permissions".to_string(),
            ));
        }

        let response_body: serde_json::Value =
            response.json().await.map_err(|e| PunchError::Provider {
                provider: "gemini".to_string(),
                message: format!("failed to parse response: {e}"),
            })?;

        if !status.is_success() {
            let error_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(PunchError::Provider {
                provider: "gemini".to_string(),
                message: format!("API error ({}): {}", status, error_msg),
            });
        }

        self.parse_response(&response_body)
    }
}

// ---------------------------------------------------------------------------
// Ollama driver
// ---------------------------------------------------------------------------

/// Driver for local Ollama instances using the chat API.
pub struct OllamaDriver {
    client: Client,
    base_url: String,
}

impl OllamaDriver {
    /// Create a new Ollama driver.
    pub fn new(base_url: Option<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.unwrap_or_else(|| "http://localhost:11434".to_string()),
        }
    }

    /// Create a new Ollama driver with a shared HTTP client.
    pub fn with_client(client: Client, base_url: Option<String>) -> Self {
        Self {
            client,
            base_url: base_url.unwrap_or_else(|| "http://localhost:11434".to_string()),
        }
    }

    /// Get the base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Build the Ollama chat request body.
    pub fn build_request_body(&self, request: &CompletionRequest) -> serde_json::Value {
        let mut messages = Vec::new();

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
                        "content": msg.content,
                    });
                    if !msg.tool_calls.is_empty() {
                        let tc: Vec<serde_json::Value> = msg
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.input,
                                    }
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
                            "content": tr.content,
                        }));
                    }
                }
            }
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "stream": false,
        });

        let mut options = serde_json::json!({});
        if let Some(temp) = request.temperature {
            options["temperature"] = serde_json::json!(temp);
        }
        if request.max_tokens > 0 {
            options["num_predict"] = serde_json::json!(request.max_tokens);
        }
        body["options"] = options;

        // Disable thinking mode for reasoning models (Qwen, DeepSeek) to prevent
        // the model from spending its entire token budget on internal reasoning.
        // The think tags get stripped anyway, so we avoid wasting tokens.
        body["think"] = serde_json::json!(false);

        if !request.tools.is_empty() {
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
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);
        }

        body
    }

    /// Parse the Ollama chat response.
    pub fn parse_response(&self, body: &serde_json::Value) -> PunchResult<CompletionResponse> {
        let msg = &body["message"];
        let raw_content = msg["content"].as_str().unwrap_or("");
        // Strip thinking tags from reasoning models (Qwen, DeepSeek, etc.)
        let content = strip_thinking_tags(raw_content);

        let mut tool_calls = Vec::new();
        if let Some(tc_array) = msg["tool_calls"].as_array() {
            for tc in tc_array {
                let name = tc["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let input = tc["function"]["arguments"].clone();
                tool_calls.push(ToolCall {
                    id: format!("ollama-{}", uuid::Uuid::new_v4()),
                    name,
                    input,
                });
            }
        }

        let stop_reason = if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else if body["done"].as_bool().unwrap_or(true) {
            StopReason::EndTurn
        } else {
            StopReason::MaxTokens
        };

        let usage = TokenUsage {
            input_tokens: body["prompt_eval_count"].as_u64().unwrap_or(0),
            output_tokens: body["eval_count"].as_u64().unwrap_or(0),
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
impl LlmDriver for OllamaDriver {
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let body = self.build_request_body(&request);

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Provider {
                provider: "ollama".to_string(),
                message: format!("request failed: {e}"),
            })?;

        let status = response.status();
        let response_body: serde_json::Value =
            response.json().await.map_err(|e| PunchError::Provider {
                provider: "ollama".to_string(),
                message: format!("failed to parse response: {e}"),
            })?;

        if !status.is_success() {
            let error_msg = response_body["error"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(PunchError::Provider {
                provider: "ollama".to_string(),
                message: format!("API error ({}): {}", status, error_msg),
            });
        }

        self.parse_response(&response_body)
    }
}

// ---------------------------------------------------------------------------
// AWS Bedrock driver
// ---------------------------------------------------------------------------

/// Driver for AWS Bedrock using the Converse API with SigV4 authentication.
pub struct BedrockDriver {
    client: Client,
    access_key: String,
    secret_key: String,
    region: String,
}

impl BedrockDriver {
    /// Create a new Bedrock driver.
    pub fn new(access_key: String, secret_key: String, region: String) -> Self {
        Self {
            client: Client::new(),
            access_key,
            secret_key,
            region,
        }
    }

    /// Create a new Bedrock driver with a shared HTTP client.
    pub fn with_client(
        client: Client,
        access_key: String,
        secret_key: String,
        region: String,
    ) -> Self {
        Self {
            client,
            access_key,
            secret_key,
            region,
        }
    }

    /// Build the Bedrock Converse API request body.
    pub fn build_request_body(&self, request: &CompletionRequest) -> serde_json::Value {
        let mut messages = Vec::new();

        for msg in &request.messages {
            match msg.role {
                Role::User => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{"text": msg.content}],
                    }));
                }
                Role::Assistant => {
                    let mut content: Vec<serde_json::Value> = Vec::new();
                    if !msg.content.is_empty() {
                        content.push(serde_json::json!({"text": msg.content}));
                    }
                    for tc in &msg.tool_calls {
                        content.push(serde_json::json!({
                            "toolUse": {
                                "toolUseId": tc.id,
                                "name": tc.name,
                                "input": tc.input,
                            }
                        }));
                    }
                    if content.is_empty() {
                        content.push(serde_json::json!({"text": ""}));
                    }
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content,
                    }));
                }
                Role::Tool => {
                    let mut content: Vec<serde_json::Value> = Vec::new();
                    for tr in &msg.tool_results {
                        content.push(serde_json::json!({
                            "toolResult": {
                                "toolUseId": tr.id,
                                "content": [{"text": tr.content}],
                                "status": if tr.is_error { "error" } else { "success" },
                            }
                        }));
                    }
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": content,
                    }));
                }
                Role::System => {
                    // System messages handled separately.
                }
            }
        }

        let mut body = serde_json::json!({
            "messages": messages,
        });

        let mut inference_config = serde_json::json!({
            "maxTokens": request.max_tokens,
        });
        if let Some(temp) = request.temperature {
            inference_config["temperature"] = serde_json::json!(temp);
        }
        body["inferenceConfig"] = inference_config;

        if let Some(ref system) = request.system_prompt {
            body["system"] = serde_json::json!([{"text": system}]);
        }

        if !request.tools.is_empty() {
            let tool_config: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "toolSpec": {
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": {"json": t.input_schema},
                        }
                    })
                })
                .collect();
            body["toolConfig"] = serde_json::json!({"tools": tool_config});
        }

        body
    }

    /// Build the endpoint URL for a model.
    pub fn build_url(&self, model_id: &str) -> String {
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/converse",
            self.region, model_id,
        )
    }

    /// Parse the Bedrock Converse API response.
    pub fn parse_response(&self, body: &serde_json::Value) -> PunchResult<CompletionResponse> {
        let content = body["output"]["message"]["content"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut text_content = String::new();
        let mut tool_calls = Vec::new();

        for block in &content {
            if let Some(text) = block["text"].as_str() {
                if !text_content.is_empty() {
                    text_content.push('\n');
                }
                text_content.push_str(text);
            }
            if let Some(tu) = block.get("toolUse") {
                tool_calls.push(ToolCall {
                    id: tu["toolUseId"].as_str().unwrap_or_default().to_string(),
                    name: tu["name"].as_str().unwrap_or_default().to_string(),
                    input: tu["input"].clone(),
                });
            }
        }

        let stop_reason_str = body["stopReason"].as_str().unwrap_or("end_turn");
        let stop_reason = if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            match stop_reason_str {
                "end_turn" => StopReason::EndTurn,
                "tool_use" => StopReason::ToolUse,
                "max_tokens" => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            }
        };

        let usage = TokenUsage {
            input_tokens: body["usage"]["inputTokens"].as_u64().unwrap_or(0),
            output_tokens: body["usage"]["outputTokens"].as_u64().unwrap_or(0),
        };

        // Strip thinking tags from reasoning models
        let text_content = strip_thinking_tags(&text_content);

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

    /// Compute an AWS SigV4 signature and return the Authorization header value.
    ///
    /// This is a basic implementation sufficient for Bedrock API calls.
    pub fn sign_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        payload: &[u8],
        timestamp: &str, // format: "20260313T120000Z"
    ) -> PunchResult<String> {
        let date = &timestamp[..8]; // "20260313"
        let service = "bedrock";

        // Parse the URL to get host and path.
        let parsed = url::Url::parse(url).map_err(|e| PunchError::Provider {
            provider: "bedrock".to_string(),
            message: format!("invalid URL: {e}"),
        })?;
        let host = parsed.host_str().unwrap_or("");
        let path = parsed.path();

        // 1. Create canonical request.
        let payload_hash = hex_sha256(payload);

        let mut signed_header_names: Vec<String> =
            headers.iter().map(|(k, _)| k.to_lowercase()).collect();
        signed_header_names.push("host".to_string());
        signed_header_names.push("x-amz-date".to_string());
        signed_header_names.sort();
        signed_header_names.dedup();

        let mut header_map: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.trim().to_string()))
            .collect();
        header_map.push(("host".to_string(), host.to_string()));
        header_map.push(("x-amz-date".to_string(), timestamp.to_string()));
        header_map.sort_by(|a, b| a.0.cmp(&b.0));
        header_map.dedup_by(|a, b| a.0 == b.0);

        let canonical_headers: String = header_map
            .iter()
            .map(|(k, v)| format!("{}:{}\n", k, v))
            .collect();

        let signed_headers = signed_header_names.join(";");

        let canonical_request = format!(
            "{}\n{}\n\n{}\n{}\n{}",
            method, path, canonical_headers, signed_headers, payload_hash,
        );

        // 2. Create string to sign.
        let credential_scope = format!("{}/{}/{}/aws4_request", date, self.region, service);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            timestamp,
            credential_scope,
            hex_sha256(canonical_request.as_bytes()),
        );

        // 3. Calculate signing key.
        let k_date = hmac_sha256(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, service.as_bytes());
        let k_signing = hmac_sha256(&k_service, b"aws4_request");

        // 4. Calculate signature.
        let signature = hex_encode(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

        // 5. Build Authorization header.
        Ok(format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_headers, signature,
        ))
    }
}

/// Compute SHA-256 hex digest.
fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(hasher.finalize().as_slice())
}

/// Compute HMAC-SHA256.
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Hex-encode bytes without an external crate.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[async_trait]
impl LlmDriver for BedrockDriver {
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let url = self.build_url(&request.model);
        let body = self.build_request_body(&request);
        let payload = serde_json::to_vec(&body).map_err(|e| PunchError::Provider {
            provider: "bedrock".to_string(),
            message: format!("failed to serialize request: {e}"),
        })?;

        let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

        let auth_header = self.sign_request(
            "POST",
            &url,
            &[("content-type".to_string(), "application/json".to_string())],
            &payload,
            &timestamp,
        )?;

        let parsed_url = url::Url::parse(&url).map_err(|e| PunchError::Provider {
            provider: "bedrock".to_string(),
            message: format!("invalid URL: {e}"),
        })?;
        let host = parsed_url.host_str().unwrap_or_default().to_string();

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .header("host", &host)
            .header("x-amz-date", &timestamp)
            .header("authorization", &auth_header)
            .body(payload)
            .send()
            .await
            .map_err(|e| PunchError::Provider {
                provider: "bedrock".to_string(),
                message: format!("request failed: {e}"),
            })?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(PunchError::RateLimited {
                provider: "bedrock".to_string(),
                retry_after_ms: 60_000,
            });
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(PunchError::Auth(
                "AWS Bedrock credentials are invalid or lack permissions".to_string(),
            ));
        }

        let response_body: serde_json::Value =
            response.json().await.map_err(|e| PunchError::Provider {
                provider: "bedrock".to_string(),
                message: format!("failed to parse response: {e}"),
            })?;

        if !status.is_success() {
            let error_msg = response_body["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(PunchError::Provider {
                provider: "bedrock".to_string(),
                message: format!("API error ({}): {}", status, error_msg),
            });
        }

        self.parse_response(&response_body)
    }
}

// ---------------------------------------------------------------------------
// Azure OpenAI driver
// ---------------------------------------------------------------------------

/// Driver for Azure OpenAI deployments.
///
/// Uses the same request/response format as OpenAI but with Azure-specific
/// URL construction and API key header.
pub struct AzureOpenAiDriver {
    inner: OpenAiCompatibleDriver,
    resource: String,
    deployment: String,
    api_version: String,
}

impl AzureOpenAiDriver {
    /// Create a new Azure OpenAI driver.
    ///
    /// - `api_key`: The Azure OpenAI API key.
    /// - `resource`: The Azure resource name (subdomain).
    /// - `deployment`: The deployment name.
    /// - `api_version`: API version string (e.g., "2024-02-01").
    pub fn new(
        api_key: String,
        resource: String,
        deployment: String,
        api_version: Option<String>,
    ) -> Self {
        let base_url = format!("https://{}.openai.azure.com", resource);
        Self {
            inner: OpenAiCompatibleDriver::new(api_key, base_url, "azure_openai".to_string()),
            resource,
            deployment,
            api_version: api_version.unwrap_or_else(|| "2024-02-01".to_string()),
        }
    }

    /// Create a new Azure OpenAI driver with a shared HTTP client.
    pub fn with_client(
        client: Client,
        api_key: String,
        resource: String,
        deployment: String,
        api_version: Option<String>,
    ) -> Self {
        let base_url = format!("https://{}.openai.azure.com", resource);
        Self {
            inner: OpenAiCompatibleDriver::with_client(
                client,
                api_key,
                base_url,
                "azure_openai".to_string(),
            ),
            resource,
            deployment,
            api_version: api_version.unwrap_or_else(|| "2024-02-01".to_string()),
        }
    }

    /// Build the Azure OpenAI endpoint URL.
    pub fn build_url(&self) -> String {
        format!(
            "https://{}.openai.azure.com/openai/deployments/{}/chat/completions?api-version={}",
            self.resource, self.deployment, self.api_version,
        )
    }

    /// Get the resource name.
    pub fn resource(&self) -> &str {
        &self.resource
    }

    /// Get the deployment name.
    pub fn deployment(&self) -> &str {
        &self.deployment
    }

    /// Build request body (delegates to inner OpenAI-compatible driver).
    pub fn build_request_body(&self, request: &CompletionRequest) -> serde_json::Value {
        self.inner.build_request_body(request)
    }

    /// Parse response (delegates to inner OpenAI-compatible driver).
    pub fn parse_response(&self, body: &serde_json::Value) -> PunchResult<CompletionResponse> {
        self.inner.parse_response(body)
    }
}

#[async_trait]
impl LlmDriver for AzureOpenAiDriver {
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let url = self.build_url();
        let body = self.inner.build_request_body(&request);

        let response = self
            .inner
            .client
            .post(&url)
            .header("api-key", &self.inner.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PunchError::Provider {
                provider: "azure_openai".to_string(),
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
                provider: "azure_openai".to_string(),
                retry_after_ms: retry_after,
            });
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(PunchError::Auth(
                "Azure OpenAI API key is invalid or lacks permissions".to_string(),
            ));
        }

        let response_body: serde_json::Value =
            response.json().await.map_err(|e| PunchError::Provider {
                provider: "azure_openai".to_string(),
                message: format!("failed to parse response: {e}"),
            })?;

        if !status.is_success() {
            let error_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(PunchError::Provider {
                provider: "azure_openai".to_string(),
                message: format!("API error ({}): {}", status, error_msg),
            });
        }

        self.inner.parse_response(&response_body)
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
        Provider::Bedrock => "https://bedrock-runtime.us-east-1.amazonaws.com",
        Provider::AzureOpenAi => "",
        Provider::Custom(_) => "",
    }
}

/// Create an [`LlmDriver`] from a [`ModelConfig`].
///
/// Reads the API key from the environment variable specified in
/// `config.api_key_env`. Returns an error if the env var is missing
/// (except for Ollama which does not require auth).
/// Create a driver from config, optionally using a shared HTTP client.
///
/// If `shared_client` is `Some`, the driver will use that client for
/// connection pooling. Otherwise it creates its own client (backward compat).
pub fn create_driver(config: &ModelConfig) -> PunchResult<Arc<dyn LlmDriver>> {
    create_driver_with_client(config, None)
}

/// Create a driver from config with an optional shared [`reqwest::Client`].
pub fn create_driver_with_client(
    config: &ModelConfig,
    shared_client: Option<&Client>,
) -> PunchResult<Arc<dyn LlmDriver>> {
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
        Provider::Anthropic => {
            if let Some(client) = shared_client {
                Ok(Arc::new(AnthropicDriver::with_client(
                    client.clone(),
                    api_key,
                    Some(base_url),
                )))
            } else {
                Ok(Arc::new(AnthropicDriver::new(api_key, Some(base_url))))
            }
        }
        Provider::Google => {
            if let Some(client) = shared_client {
                Ok(Arc::new(GeminiDriver::with_client(
                    client.clone(),
                    api_key,
                    Some(base_url),
                )))
            } else {
                Ok(Arc::new(GeminiDriver::new(api_key, Some(base_url))))
            }
        }
        Provider::Ollama => {
            if let Some(client) = shared_client {
                Ok(Arc::new(OllamaDriver::with_client(
                    client.clone(),
                    Some(base_url),
                )))
            } else {
                Ok(Arc::new(OllamaDriver::new(Some(base_url))))
            }
        }
        Provider::Bedrock => {
            // For Bedrock, api_key is expected to be "ACCESS_KEY:SECRET_KEY" or
            // we read AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY from env.
            let (access_key, secret_key) = if api_key.contains(':') {
                let parts: Vec<&str> = api_key.splitn(2, ':').collect();
                (parts[0].to_string(), parts[1].to_string())
            } else {
                let ak = std::env::var("AWS_ACCESS_KEY_ID").unwrap_or(api_key);
                let sk = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default();
                (ak, sk)
            };
            // Extract region from base_url or default to us-east-1.
            let region = if base_url.contains("bedrock-runtime.") {
                base_url
                    .trim_start_matches("https://bedrock-runtime.")
                    .split('.')
                    .next()
                    .unwrap_or("us-east-1")
                    .to_string()
            } else {
                "us-east-1".to_string()
            };
            if let Some(client) = shared_client {
                Ok(Arc::new(BedrockDriver::with_client(
                    client.clone(),
                    access_key,
                    secret_key,
                    region,
                )))
            } else {
                Ok(Arc::new(BedrockDriver::new(access_key, secret_key, region)))
            }
        }
        Provider::AzureOpenAi => {
            // For Azure, base_url should be "https://{resource}.openai.azure.com"
            // and model is the deployment name.
            let resource = if base_url.contains(".openai.azure.com") {
                base_url
                    .trim_start_matches("https://")
                    .split('.')
                    .next()
                    .unwrap_or("default")
                    .to_string()
            } else {
                base_url.clone()
            };
            let deployment = config.model.clone();
            if let Some(client) = shared_client {
                Ok(Arc::new(AzureOpenAiDriver::with_client(
                    client.clone(),
                    api_key,
                    resource,
                    deployment,
                    None,
                )))
            } else {
                Ok(Arc::new(AzureOpenAiDriver::new(
                    api_key,
                    resource,
                    deployment,
                    None,
                )))
            }
        }
        provider => {
            let name = provider.to_string();
            if let Some(client) = shared_client {
                Ok(Arc::new(OpenAiCompatibleDriver::with_client(
                    client.clone(),
                    api_key,
                    base_url,
                    name,
                )))
            } else {
                Ok(Arc::new(OpenAiCompatibleDriver::new(
                    api_key, base_url, name,
                )))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::ToolCategory;

    /// Helper to build a simple completion request for testing.
    fn simple_request() -> CompletionRequest {
        CompletionRequest {
            model: "test-model".to_string(),
            messages: vec![Message::new(Role::User, "Hello")],
            tools: Vec::new(),
            max_tokens: 4096,
            temperature: Some(0.7),
            system_prompt: Some("You are helpful.".to_string()),
        }
    }

    /// Helper to build a request with tools.
    fn request_with_tools() -> CompletionRequest {
        CompletionRequest {
            model: "test-model".to_string(),
            messages: vec![Message::new(Role::User, "Use the tool")],
            tools: vec![ToolDefinition {
                name: "get_weather".to_string(),
                description: "Get weather for a city".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    }
                }),
                category: ToolCategory::Web,
            }],
            max_tokens: 4096,
            temperature: Some(0.7),
            system_prompt: None,
        }
    }

    // -----------------------------------------------------------------------
    // Gemini tests
    // -----------------------------------------------------------------------

    #[test]
    fn gemini_request_formatting() {
        let driver = GeminiDriver::new("test-key".to_string(), None);
        let body = driver.build_request_body(&simple_request());

        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        // System prompt should be prepended to user message.
        let first_text = contents[0]["parts"][0]["text"].as_str().unwrap();
        assert!(first_text.contains("You are helpful."));
        assert!(first_text.contains("Hello"));
        // Role should be "user" (not "system").
        assert_eq!(contents[0]["role"].as_str().unwrap(), "user");

        assert_eq!(body["generationConfig"]["maxOutputTokens"], 4096);
        assert!((body["generationConfig"]["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
    }

    #[test]
    fn gemini_response_parsing() {
        let driver = GeminiDriver::new("test-key".to_string(), None);
        let response_body = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello there!"}],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5
            }
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Hello there!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn gemini_role_mapping_system_prepended() {
        let driver = GeminiDriver::new("test-key".to_string(), None);
        let req = CompletionRequest {
            model: "gemini-pro".to_string(),
            messages: vec![
                Message::new(Role::System, "Be concise."),
                Message::new(Role::User, "Hi"),
            ],
            tools: Vec::new(),
            max_tokens: 1024,
            temperature: None,
            system_prompt: None,
        };
        let body = driver.build_request_body(&req);
        let contents = body["contents"].as_array().unwrap();
        // System message should be merged into user message.
        assert_eq!(contents.len(), 1);
        let text = contents[0]["parts"][0]["text"].as_str().unwrap();
        assert!(text.contains("Be concise."));
        assert!(text.contains("Hi"));
    }

    #[test]
    fn gemini_function_call_parsing() {
        let driver = GeminiDriver::new("test-key".to_string(), None);
        let response_body = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Let me check the weather."},
                        {
                            "functionCall": {
                                "name": "get_weather",
                                "args": {"city": "London"}
                            }
                        }
                    ],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 15,
                "candidatesTokenCount": 8
            }
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Let me check the weather.");
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.message.tool_calls.len(), 1);
        assert_eq!(resp.message.tool_calls[0].name, "get_weather");
        assert_eq!(resp.message.tool_calls[0].input["city"], "London");
    }

    #[test]
    fn gemini_api_key_in_url() {
        let driver = GeminiDriver::new("my-secret-key".to_string(), None);
        let url = driver.build_url("gemini-pro");
        assert!(url.contains("key=my-secret-key"));
        assert!(url.contains("models/gemini-pro:generateContent"));
    }

    // -----------------------------------------------------------------------
    // Ollama tests
    // -----------------------------------------------------------------------

    #[test]
    fn ollama_request_formatting() {
        let driver = OllamaDriver::new(None);
        let body = driver.build_request_body(&simple_request());

        assert_eq!(body["model"], "test-model");
        assert_eq!(body["stream"], false);
        let messages = body["messages"].as_array().unwrap();
        // system prompt + user message = 2 messages
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are helpful.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello");
        assert!((body["options"]["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
    }

    #[test]
    fn ollama_response_parsing() {
        let driver = OllamaDriver::new(None);
        let response_body = serde_json::json!({
            "message": {
                "role": "assistant",
                "content": "Hi there!"
            },
            "done": true,
            "prompt_eval_count": 20,
            "eval_count": 10
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Hi there!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 20);
        assert_eq!(resp.usage.output_tokens, 10);
    }

    #[test]
    fn ollama_default_endpoint() {
        let driver = OllamaDriver::new(None);
        assert_eq!(driver.base_url(), "http://localhost:11434");
    }

    #[test]
    fn ollama_custom_endpoint() {
        let driver = OllamaDriver::new(Some("http://myhost:9999".to_string()));
        assert_eq!(driver.base_url(), "http://myhost:9999");
    }

    // -----------------------------------------------------------------------
    // Bedrock tests
    // -----------------------------------------------------------------------

    #[test]
    fn bedrock_request_formatting() {
        let driver = BedrockDriver::new(
            "TESTKEY".to_string(),
            "testsecret".to_string(),
            "us-west-2".to_string(),
        );
        let body = driver.build_request_body(&simple_request());

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"][0]["text"], "Hello");

        assert_eq!(body["inferenceConfig"]["maxTokens"], 4096);
        assert!((body["inferenceConfig"]["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
        assert_eq!(body["system"][0]["text"], "You are helpful.");
    }

    #[test]
    fn bedrock_sigv4_canonical_request() {
        let driver = BedrockDriver::new(
            "TESTACCESS1234567890".to_string(),
            "TestSecretKeyValue1234567890abcdefghijk".to_string(),
            "us-east-1".to_string(),
        );

        let payload = b"{}";
        let timestamp = "20260313T120000Z";

        let auth = driver
            .sign_request(
                "POST",
                "https://bedrock-runtime.us-east-1.amazonaws.com/model/test/converse",
                &[("content-type".to_string(), "application/json".to_string())],
                payload,
                timestamp,
            )
            .unwrap();

        assert!(auth.starts_with(
            "AWS4-HMAC-SHA256 Credential=TESTACCESS1234567890/20260313/us-east-1/bedrock/aws4_request"
        ));
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-date"));
        assert!(auth.contains("Signature="));
    }

    #[test]
    fn bedrock_response_parsing() {
        let driver = BedrockDriver::new(
            "key".to_string(),
            "secret".to_string(),
            "us-east-1".to_string(),
        );
        let response_body = serde_json::json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{"text": "The answer is 42."}]
                }
            },
            "stopReason": "end_turn",
            "usage": {
                "inputTokens": 100,
                "outputTokens": 50
            }
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "The answer is 42.");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 100);
        assert_eq!(resp.usage.output_tokens, 50);
    }

    // -----------------------------------------------------------------------
    // Azure OpenAI tests
    // -----------------------------------------------------------------------

    #[test]
    fn azure_openai_url_construction() {
        let driver = AzureOpenAiDriver::new(
            "my-azure-key".to_string(),
            "myresource".to_string(),
            "gpt-4-deployment".to_string(),
            None,
        );
        let url = driver.build_url();
        assert_eq!(
            url,
            "https://myresource.openai.azure.com/openai/deployments/gpt-4-deployment/chat/completions?api-version=2024-02-01"
        );
    }

    #[test]
    fn azure_openai_custom_api_version() {
        let driver = AzureOpenAiDriver::new(
            "key".to_string(),
            "res".to_string(),
            "dep".to_string(),
            Some("2024-06-01".to_string()),
        );
        let url = driver.build_url();
        assert!(url.contains("api-version=2024-06-01"));
    }

    #[test]
    fn azure_openai_request_formatting() {
        let driver = AzureOpenAiDriver::new(
            "key".to_string(),
            "res".to_string(),
            "dep".to_string(),
            None,
        );
        let body = driver.build_request_body(&simple_request());
        // Should use OpenAI format.
        let messages = body["messages"].as_array().unwrap();
        // system prompt + user message = 2
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(body["model"], "test-model");
    }

    #[test]
    fn azure_openai_resource_and_deployment() {
        let driver = AzureOpenAiDriver::new(
            "key".to_string(),
            "my-resource".to_string(),
            "my-deploy".to_string(),
            None,
        );
        assert_eq!(driver.resource(), "my-resource");
        assert_eq!(driver.deployment(), "my-deploy");
    }

    // -----------------------------------------------------------------------
    // create_driver dispatch tests
    // -----------------------------------------------------------------------

    #[test]
    fn create_driver_dispatches_ollama() {
        let config = ModelConfig {
            provider: Provider::Ollama,
            model: "llama3".to_string(),
            api_key_env: None,
            base_url: None,
            max_tokens: None,
            temperature: None,
        };
        // Ollama does not need an API key, so this should succeed.
        let driver = create_driver(&config);
        assert!(driver.is_ok());
    }

    #[test]
    fn create_driver_dispatches_gemini() {
        // Set a fake env var for this test.
        // SAFETY: Test is single-threaded relative to this env var name.
        unsafe { std::env::set_var("TEST_GEMINI_KEY_DISPATCH", "fake-key") };
        let config = ModelConfig {
            provider: Provider::Google,
            model: "gemini-pro".to_string(),
            api_key_env: Some("TEST_GEMINI_KEY_DISPATCH".to_string()),
            base_url: None,
            max_tokens: None,
            temperature: None,
        };
        let driver = create_driver(&config);
        assert!(driver.is_ok());
        unsafe { std::env::remove_var("TEST_GEMINI_KEY_DISPATCH") };
    }

    #[test]
    fn create_driver_dispatches_bedrock() {
        // SAFETY: Test is single-threaded relative to this env var name.
        unsafe { std::env::set_var("TEST_BEDROCK_KEY_DISPATCH", "TESTKEY:TESTSECRET") };
        let config = ModelConfig {
            provider: Provider::Bedrock,
            model: "anthropic.claude-v2".to_string(),
            api_key_env: Some("TEST_BEDROCK_KEY_DISPATCH".to_string()),
            base_url: None,
            max_tokens: None,
            temperature: None,
        };
        let driver = create_driver(&config);
        assert!(driver.is_ok());
        unsafe { std::env::remove_var("TEST_BEDROCK_KEY_DISPATCH") };
    }

    #[test]
    fn create_driver_dispatches_azure_openai() {
        // SAFETY: Test is single-threaded relative to this env var name.
        unsafe { std::env::set_var("TEST_AZURE_KEY_DISPATCH", "azure-key") };
        let config = ModelConfig {
            provider: Provider::AzureOpenAi,
            model: "gpt-4".to_string(),
            api_key_env: Some("TEST_AZURE_KEY_DISPATCH".to_string()),
            base_url: Some("https://myres.openai.azure.com".to_string()),
            max_tokens: None,
            temperature: None,
        };
        let driver = create_driver(&config);
        assert!(driver.is_ok());
        unsafe { std::env::remove_var("TEST_AZURE_KEY_DISPATCH") };
    }

    #[test]
    fn gemini_tools_in_request() {
        let driver = GeminiDriver::new("key".to_string(), None);
        let body = driver.build_request_body(&request_with_tools());

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        let func_decls = tools[0]["function_declarations"].as_array().unwrap();
        assert_eq!(func_decls.len(), 1);
        assert_eq!(func_decls[0]["name"], "get_weather");
    }

    #[test]
    fn ollama_tools_in_request() {
        let driver = OllamaDriver::new(None);
        let body = driver.build_request_body(&request_with_tools());

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "get_weather");
    }

    #[test]
    fn bedrock_url_construction() {
        let driver = BedrockDriver::new(
            "key".to_string(),
            "secret".to_string(),
            "eu-west-1".to_string(),
        );
        let url = driver.build_url("anthropic.claude-3-sonnet");
        assert_eq!(
            url,
            "https://bedrock-runtime.eu-west-1.amazonaws.com/model/anthropic.claude-3-sonnet/converse"
        );
    }

    // -----------------------------------------------------------------------
    // TokenUsage tests
    // -----------------------------------------------------------------------

    #[test]
    fn token_usage_default() {
        let u = TokenUsage::default();
        assert_eq!(u.input_tokens, 0);
        assert_eq!(u.output_tokens, 0);
        assert_eq!(u.total(), 0);
    }

    #[test]
    fn token_usage_accumulate() {
        let mut u = TokenUsage { input_tokens: 10, output_tokens: 20 };
        let other = TokenUsage { input_tokens: 5, output_tokens: 15 };
        u.accumulate(&other);
        assert_eq!(u.input_tokens, 15);
        assert_eq!(u.output_tokens, 35);
        assert_eq!(u.total(), 50);
    }

    #[test]
    fn token_usage_total() {
        let u = TokenUsage { input_tokens: 100, output_tokens: 200 };
        assert_eq!(u.total(), 300);
    }

    // -----------------------------------------------------------------------
    // StopReason serialization
    // -----------------------------------------------------------------------

    #[test]
    fn stop_reason_serialization() {
        let json = serde_json::to_string(&StopReason::EndTurn).unwrap();
        assert_eq!(json, "\"end_turn\"");

        let json = serde_json::to_string(&StopReason::ToolUse).unwrap();
        assert_eq!(json, "\"tool_use\"");

        let json = serde_json::to_string(&StopReason::MaxTokens).unwrap();
        assert_eq!(json, "\"max_tokens\"");

        let json = serde_json::to_string(&StopReason::Error).unwrap();
        assert_eq!(json, "\"error\"");
    }

    #[test]
    fn stop_reason_deserialization() {
        let sr: StopReason = serde_json::from_str("\"end_turn\"").unwrap();
        assert_eq!(sr, StopReason::EndTurn);

        let sr: StopReason = serde_json::from_str("\"tool_use\"").unwrap();
        assert_eq!(sr, StopReason::ToolUse);
    }

    // -----------------------------------------------------------------------
    // Anthropic driver tests
    // -----------------------------------------------------------------------

    #[test]
    fn anthropic_request_body_simple() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let body = driver.build_request_body(&simple_request());

        assert_eq!(body["model"], "test-model");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["system"], "You are helpful.");
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello");
    }

    #[test]
    fn anthropic_request_body_with_tools() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let body = driver.build_request_body(&request_with_tools());

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "get_weather");
        assert!(tools[0]["input_schema"]["properties"].is_object());
    }

    #[test]
    fn anthropic_request_body_no_system_prompt() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let req = CompletionRequest {
            model: "test".into(),
            messages: vec![Message::new(Role::User, "Hi")],
            tools: Vec::new(),
            max_tokens: 100,
            temperature: None,
            system_prompt: None,
        };
        let body = driver.build_request_body(&req);
        assert!(body.get("system").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn anthropic_parse_response_text() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let response_body = serde_json::json!({
            "content": [
                {"type": "text", "text": "Hello!"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Hello!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
        assert!(resp.message.tool_calls.is_empty());
    }

    #[test]
    fn anthropic_parse_response_tool_use() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let response_body = serde_json::json!({
            "content": [
                {"type": "text", "text": "Let me check."},
                {
                    "type": "tool_use",
                    "id": "tool_abc",
                    "name": "get_weather",
                    "input": {"city": "NYC"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 20, "output_tokens": 15}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Let me check.");
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.message.tool_calls.len(), 1);
        assert_eq!(resp.message.tool_calls[0].id, "tool_abc");
        assert_eq!(resp.message.tool_calls[0].name, "get_weather");
        assert_eq!(resp.message.tool_calls[0].input["city"], "NYC");
    }

    #[test]
    fn anthropic_parse_response_max_tokens() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let response_body = serde_json::json!({
            "content": [{"type": "text", "text": "truncated"}],
            "stop_reason": "max_tokens",
            "usage": {"input_tokens": 5, "output_tokens": 100}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn anthropic_parse_response_unknown_stop_reason() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let response_body = serde_json::json!({
            "content": [{"type": "text", "text": "err"}],
            "stop_reason": "something_unknown",
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::Error);
    }

    #[test]
    fn anthropic_request_body_with_assistant_and_tool_messages() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let req = CompletionRequest {
            model: "test".into(),
            messages: vec![
                Message::new(Role::User, "Hi"),
                Message {
                    role: Role::Assistant,
                    content: "I'll check".into(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "file_read".into(),
                        input: serde_json::json!({"path": "/tmp/test"}),
                    }],
                    tool_results: Vec::new(),
                    timestamp: chrono::Utc::now(),
                },
                Message {
                    role: Role::Tool,
                    content: String::new(),
                    tool_calls: Vec::new(),
                    tool_results: vec![punch_types::ToolCallResult {
                        id: "call_1".into(),
                        content: "file contents".into(),
                        is_error: false,
                    }],
                    timestamp: chrono::Utc::now(),
                },
            ],
            tools: Vec::new(),
            max_tokens: 100,
            temperature: None,
            system_prompt: None,
        };

        let body = driver.build_request_body(&req);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "user"); // Tool results go as user role
    }

    #[test]
    fn anthropic_request_body_system_message_skipped() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let req = CompletionRequest {
            model: "test".into(),
            messages: vec![
                Message::new(Role::System, "System instruction"),
                Message::new(Role::User, "Hi"),
            ],
            tools: Vec::new(),
            max_tokens: 100,
            temperature: None,
            system_prompt: None,
        };

        let body = driver.build_request_body(&req);
        let messages = body["messages"].as_array().unwrap();
        // System messages are skipped in messages array
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    // -----------------------------------------------------------------------
    // OpenAI-compatible driver tests
    // -----------------------------------------------------------------------

    #[test]
    fn openai_request_body_simple() {
        let driver = OpenAiCompatibleDriver::new(
            "key".into(),
            "https://api.openai.com".into(),
            "openai".into(),
        );
        let body = driver.build_request_body(&simple_request());

        assert_eq!(body["model"], "test-model");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are helpful.");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn openai_request_body_with_tools() {
        let driver = OpenAiCompatibleDriver::new(
            "key".into(),
            "https://api.openai.com".into(),
            "openai".into(),
        );
        let body = driver.build_request_body(&request_with_tools());

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "get_weather");
    }

    #[test]
    fn openai_parse_response_text() {
        let driver = OpenAiCompatibleDriver::new(
            "key".into(),
            "https://api.openai.com".into(),
            "openai".into(),
        );
        let response_body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5
            }
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Hello!");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn openai_parse_response_tool_calls() {
        let driver = OpenAiCompatibleDriver::new(
            "key".into(),
            "https://api.openai.com".into(),
            "openai".into(),
        );
        let response_body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\": \"NYC\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.message.tool_calls.len(), 1);
        assert_eq!(resp.message.tool_calls[0].name, "get_weather");
        assert_eq!(resp.message.tool_calls[0].input["city"], "NYC");
    }

    #[test]
    fn openai_parse_response_tool_calls_fix_stop_reason() {
        let driver = OpenAiCompatibleDriver::new(
            "key".into(),
            "https://api.openai.com".into(),
            "openai".into(),
        );
        // finish_reason is "stop" but there are tool_calls — should fix to ToolUse
        let response_body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Using tool",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "test_tool",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn openai_parse_response_length_stop_reason() {
        let driver = OpenAiCompatibleDriver::new(
            "key".into(),
            "https://api.openai.com".into(),
            "openai".into(),
        );
        let response_body = serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": "cut off"},
                "finish_reason": "length"
            }],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn openai_parse_response_no_choices_error() {
        let driver = OpenAiCompatibleDriver::new(
            "key".into(),
            "https://api.openai.com".into(),
            "openai".into(),
        );
        let response_body = serde_json::json!({"choices": []});

        let result = driver.parse_response(&response_body);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Gemini driver additional tests
    // -----------------------------------------------------------------------

    #[test]
    fn gemini_assistant_message_formatting() {
        let driver = GeminiDriver::new("key".to_string(), None);
        let req = CompletionRequest {
            model: "gemini-pro".into(),
            messages: vec![
                Message::new(Role::User, "Hi"),
                Message {
                    role: Role::Assistant,
                    content: "Let me help".into(),
                    tool_calls: vec![ToolCall {
                        id: "tc1".into(),
                        name: "get_weather".into(),
                        input: serde_json::json!({"city": "NYC"}),
                    }],
                    tool_results: Vec::new(),
                    timestamp: chrono::Utc::now(),
                },
            ],
            tools: Vec::new(),
            max_tokens: 100,
            temperature: None,
            system_prompt: None,
        };

        let body = driver.build_request_body(&req);
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents[1]["role"], "model"); // Gemini uses "model" not "assistant"
        let parts = contents[1]["parts"].as_array().unwrap();
        assert!(parts.len() >= 2); // text part + functionCall part
    }

    #[test]
    fn gemini_max_tokens_stop_reason() {
        let driver = GeminiDriver::new("key".to_string(), None);
        let response_body = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "truncated"}],
                    "role": "model"
                },
                "finishReason": "MAX_TOKENS"
            }],
            "usageMetadata": {"promptTokenCount": 0, "candidatesTokenCount": 0}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn gemini_custom_base_url() {
        let driver = GeminiDriver::new("key".to_string(), Some("https://custom.example.com".into()));
        let url = driver.build_url("gemini-pro");
        assert!(url.starts_with("https://custom.example.com/"));
    }

    // -----------------------------------------------------------------------
    // Ollama driver additional tests
    // -----------------------------------------------------------------------

    #[test]
    fn ollama_response_with_tool_calls() {
        let driver = OllamaDriver::new(None);
        let response_body = serde_json::json!({
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": "get_weather",
                        "arguments": {"city": "London"}
                    }
                }]
            },
            "done": true,
            "prompt_eval_count": 10,
            "eval_count": 5
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.message.tool_calls.len(), 1);
        assert_eq!(resp.message.tool_calls[0].name, "get_weather");
    }

    #[test]
    fn ollama_response_not_done() {
        let driver = OllamaDriver::new(None);
        let response_body = serde_json::json!({
            "message": {"role": "assistant", "content": "partial"},
            "done": false,
            "prompt_eval_count": 10,
            "eval_count": 5
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }

    // -----------------------------------------------------------------------
    // Bedrock driver additional tests
    // -----------------------------------------------------------------------

    #[test]
    fn bedrock_request_with_tools() {
        let driver = BedrockDriver::new("key".into(), "secret".into(), "us-east-1".into());
        let body = driver.build_request_body(&request_with_tools());

        let tool_config = &body["toolConfig"]["tools"];
        assert!(tool_config.is_array());
        let tools = tool_config.as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["toolSpec"]["name"], "get_weather");
    }

    #[test]
    fn bedrock_response_with_tool_use() {
        let driver = BedrockDriver::new("key".into(), "secret".into(), "us-east-1".into());
        let response_body = serde_json::json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [
                        {"text": "Using tool"},
                        {"toolUse": {
                            "toolUseId": "tu_123",
                            "name": "get_weather",
                            "input": {"city": "NYC"}
                        }}
                    ]
                }
            },
            "stopReason": "tool_use",
            "usage": {"inputTokens": 10, "outputTokens": 20}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.message.tool_calls.len(), 1);
        assert_eq!(resp.message.tool_calls[0].id, "tu_123");
        assert_eq!(resp.message.tool_calls[0].name, "get_weather");
    }

    #[test]
    fn bedrock_request_with_tool_results() {
        let driver = BedrockDriver::new("key".into(), "secret".into(), "us-east-1".into());
        let req = CompletionRequest {
            model: "test".into(),
            messages: vec![
                Message::new(Role::User, "Hi"),
                Message {
                    role: Role::Tool,
                    content: String::new(),
                    tool_calls: Vec::new(),
                    tool_results: vec![punch_types::ToolCallResult {
                        id: "tu_1".into(),
                        content: "result data".into(),
                        is_error: false,
                    }],
                    timestamp: chrono::Utc::now(),
                },
            ],
            tools: Vec::new(),
            max_tokens: 100,
            temperature: None,
            system_prompt: None,
        };

        let body = driver.build_request_body(&req);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[1]["role"], "user"); // Bedrock sends tool results as user
        let content = messages[1]["content"].as_array().unwrap();
        assert!(content[0]["toolResult"].is_object());
        assert_eq!(content[0]["toolResult"]["status"], "success");
    }

    #[test]
    fn bedrock_url_different_regions() {
        let driver = BedrockDriver::new("k".into(), "s".into(), "ap-southeast-1".into());
        let url = driver.build_url("model-id");
        assert!(url.contains("ap-southeast-1"));
    }

    // -----------------------------------------------------------------------
    // Azure OpenAI additional tests
    // -----------------------------------------------------------------------

    #[test]
    fn azure_openai_delegates_parse_to_openai() {
        let driver = AzureOpenAiDriver::new(
            "key".into(), "res".into(), "dep".into(), None,
        );
        let response_body = serde_json::json!({
            "choices": [{
                "message": {"role": "assistant", "content": "Azure response"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Azure response");
    }

    // -----------------------------------------------------------------------
    // default_base_url tests
    // -----------------------------------------------------------------------

    #[test]
    fn default_base_url_anthropic() {
        assert_eq!(default_base_url(&Provider::Anthropic), "https://api.anthropic.com");
    }

    #[test]
    fn default_base_url_openai() {
        assert_eq!(default_base_url(&Provider::OpenAI), "https://api.openai.com");
    }

    #[test]
    fn default_base_url_google() {
        assert_eq!(default_base_url(&Provider::Google), "https://generativelanguage.googleapis.com");
    }

    #[test]
    fn default_base_url_ollama() {
        assert_eq!(default_base_url(&Provider::Ollama), "http://localhost:11434");
    }

    #[test]
    fn default_base_url_groq() {
        assert_eq!(default_base_url(&Provider::Groq), "https://api.groq.com/openai");
    }

    #[test]
    fn default_base_url_deepseek() {
        assert_eq!(default_base_url(&Provider::DeepSeek), "https://api.deepseek.com");
    }

    // -----------------------------------------------------------------------
    // hex_sha256 and hex_encode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hex_sha256() {
        let hash = hex_sha256(b"");
        assert_eq!(hash, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0x0a, 0xbc]), "00ff0abc");
    }

    #[test]
    fn test_hmac_sha256_basic() {
        let result = hmac_sha256(b"key", b"data");
        assert!(!result.is_empty());
        assert_eq!(result.len(), 32); // SHA-256 produces 32 bytes
    }

    // -----------------------------------------------------------------------
    // create_driver error cases
    // -----------------------------------------------------------------------

    #[test]
    fn create_driver_missing_api_key_env() {
        let config = ModelConfig {
            provider: Provider::Anthropic,
            model: "claude-3".into(),
            api_key_env: Some("PUNCH_TEST_NONEXISTENT_KEY_XYZ".into()),
            base_url: None,
            max_tokens: None,
            temperature: None,
        };
        let result = create_driver(&config);
        assert!(result.is_err());
    }

    #[test]
    fn create_driver_openai_compatible_fallback() {
        // Custom provider should fall through to OpenAI-compatible
        unsafe { std::env::set_var("TEST_CUSTOM_KEY_DRIVER", "fake-key") };
        let config = ModelConfig {
            provider: Provider::Custom("my-custom".into()),
            model: "custom-model".into(),
            api_key_env: Some("TEST_CUSTOM_KEY_DRIVER".into()),
            base_url: Some("https://custom.api.com".into()),
            max_tokens: None,
            temperature: None,
        };
        let result = create_driver(&config);
        assert!(result.is_ok());
        unsafe { std::env::remove_var("TEST_CUSTOM_KEY_DRIVER") };
    }

    // -----------------------------------------------------------------------
    // strip_thinking_tags tests
    // -----------------------------------------------------------------------

    #[test]
    fn strip_thinking_tags_removes_think_block() {
        let input = "<think>internal reasoning here</think>The answer is 42.";
        assert_eq!(strip_thinking_tags(input), "The answer is 42.");
    }

    #[test]
    fn strip_thinking_tags_removes_thinking_block() {
        let input = "<thinking>step by step reasoning</thinking>Hello world!";
        assert_eq!(strip_thinking_tags(input), "Hello world!");
    }

    #[test]
    fn strip_thinking_tags_removes_reasoning_block() {
        let input = "<reasoning>let me figure this out</reasoning>The result is correct.";
        assert_eq!(strip_thinking_tags(input), "The result is correct.");
    }

    #[test]
    fn strip_thinking_tags_removes_reflection_block() {
        let input = "<reflection>checking my work</reflection>Yes, that's right.";
        assert_eq!(strip_thinking_tags(input), "Yes, that's right.");
    }

    #[test]
    fn strip_thinking_tags_removes_multiple_blocks() {
        let input = "<think>first thought</think>Hello <thinking>second thought</thinking>world!";
        assert_eq!(strip_thinking_tags(input), "Hello world!");
    }

    #[test]
    fn strip_thinking_tags_preserves_content_without_tags() {
        let input = "Just a normal response with no thinking tags.";
        assert_eq!(strip_thinking_tags(input), input);
    }

    #[test]
    fn strip_thinking_tags_handles_multiline_tags() {
        let input = "<think>\nLine 1\nLine 2\nLine 3\n</think>\nThe final answer.";
        assert_eq!(strip_thinking_tags(input), "The final answer.");
    }

    #[test]
    fn strip_thinking_tags_returns_original_if_all_thinking() {
        // If the entire response is thinking with no visible output,
        // return the original so the user sees something.
        let input = "<think>this is all thinking content and nothing else</think>";
        assert_eq!(strip_thinking_tags(input), input);
    }

    #[test]
    fn strip_thinking_tags_handles_unclosed_tag() {
        let input = "Some text<think>unclosed thinking block";
        assert_eq!(strip_thinking_tags(input), "Some text");
    }

    #[test]
    fn strip_thinking_tags_handles_empty_input() {
        assert_eq!(strip_thinking_tags(""), "");
    }

    #[test]
    fn strip_thinking_tags_handles_empty_think_block() {
        let input = "<think></think>Visible content.";
        assert_eq!(strip_thinking_tags(input), "Visible content.");
    }

    #[test]
    fn strip_thinking_tags_trims_whitespace() {
        let input = "  <think>reasoning</think>  Result  ";
        assert_eq!(strip_thinking_tags(input), "Result");
    }

    #[test]
    fn strip_thinking_tags_mixed_tag_types() {
        let input = "<think>t1</think>A<reasoning>r1</reasoning>B<reflection>f1</reflection>C";
        assert_eq!(strip_thinking_tags(input), "ABC");
    }

    #[test]
    fn ollama_response_strips_thinking_tags() {
        let driver = OllamaDriver::new(None);
        let response_body = serde_json::json!({
            "message": {
                "role": "assistant",
                "content": "<think>\nLet me think about this...\nThe user wants hello world.\n</think>\nHello, world!"
            },
            "done": true,
            "prompt_eval_count": 20,
            "eval_count": 50
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Hello, world!");
        assert!(!resp.message.content.contains("<think>"));
    }

    #[test]
    fn gemini_response_strips_thinking_tags() {
        let driver = GeminiDriver::new("test-key".to_string(), None);
        let response_body = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "<thinking>reasoning step</thinking>The answer is 7."}],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20
            }
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "The answer is 7.");
        assert!(!resp.message.content.contains("<thinking>"));
    }

    #[test]
    fn anthropic_response_strips_thinking_tags() {
        let driver = AnthropicDriver::new("test-key".to_string(), None);
        let response_body = serde_json::json!({
            "content": [
                {"type": "text", "text": "<think>internal thought</think>Clean output."}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Clean output.");
    }

    #[test]
    fn bedrock_response_strips_thinking_tags() {
        let driver = BedrockDriver::new(
            "key".to_string(),
            "secret".to_string(),
            "us-east-1".to_string(),
        );
        let response_body = serde_json::json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{"text": "<reasoning>deep thought</reasoning>Result here."}]
                }
            },
            "stopReason": "end_turn",
            "usage": {"inputTokens": 50, "outputTokens": 25}
        });

        let resp = driver.parse_response(&response_body).unwrap();
        assert_eq!(resp.message.content, "Result here.");
    }
}
