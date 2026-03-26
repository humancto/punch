//! Real HTTP integration tests for the Punch Arena API.
//!
//! These tests boot a real Ring with a mock LLM driver, start a real axum HTTP
//! server on a random port, and hit actual endpoints with reqwest.
//!
//! Run: cargo test -p punch-api --test api_integration_test -- --nocapture

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use punch_api::AppState;
use punch_api::routes::a2a::A2AState;
use punch_api::server::build_router;
use punch_kernel::Ring;
use punch_memory::MemorySubstrate;
use punch_runtime::{CompletionRequest, CompletionResponse, LlmDriver, StopReason, TokenUsage};
use punch_types::a2a::A2ARegistry;
use punch_types::config::MemoryConfig;
use punch_types::{ModelConfig, Provider, PunchConfig, PunchResult};

// ---------------------------------------------------------------------------
// Mock LLM Driver
// ---------------------------------------------------------------------------

struct MockLlmDriver {
    call_count: AtomicU64,
}

impl MockLlmDriver {
    fn new() -> Self {
        Self {
            call_count: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl LlmDriver for MockLlmDriver {
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);

        let user_content = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == punch_types::Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let response = format!("[mock-response-{}] {}", count, user_content);

        Ok(CompletionResponse {
            message: punch_types::Message {
                role: punch_types::Role::Assistant,
                content: response,
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                content_parts: Vec::new(),
                timestamp: chrono::Utc::now(),
            },
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            stop_reason: StopReason::EndTurn,
        })
    }
}

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

fn test_config() -> PunchConfig {
    PunchConfig {
        api_listen: "127.0.0.1:0".to_string(),
        api_key: String::new(),
        rate_limit_rpm: 60,
        default_model: ModelConfig {
            provider: Provider::Ollama,
            model: "gpt-oss:20b".to_string(),
            api_key_env: None,
            base_url: Some("http://localhost:11434".to_string()),
            max_tokens: Some(4096),
            temperature: Some(0.7),
        },
        memory: MemoryConfig {
            db_path: ":memory:".to_string(),
            knowledge_graph_enabled: false,
            max_entries: None,
        },
        tunnel: None,
        channels: Default::default(),
        mcp_servers: Default::default(),
        model_routing: Default::default(),
        budget: Default::default(),
    }
}

struct TestServer {
    base_url: String,
    _ring: Arc<Ring>,
}

async fn start_test_server() -> TestServer {
    start_test_server_with_auth("").await
}

async fn start_test_server_with_auth(api_key: &str) -> TestServer {
    let config = test_config();
    let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory db"));
    let driver: Arc<dyn LlmDriver> = Arc::new(MockLlmDriver::new());
    let ring = Arc::new(Ring::new(config.clone(), memory, driver));

    let state = AppState {
        ring: ring.clone(),
        started_at: chrono::Utc::now(),
        config: Arc::new(config.clone()),
        a2a: A2AState::new(A2ARegistry::our_card(
            "test-agent",
            "http://localhost:0",
            vec![],
        )),
        channel_router: Arc::new(punch_channels::router::ChannelRouter::new()),
    };

    let app = build_router(state, api_key, 60);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind test server");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestServer {
        base_url: format!("http://{}", addr),
        _ring: ring,
    }
}

async fn start_test_server_with_rate_limit(rpm: u32) -> TestServer {
    let config = test_config();
    let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory db"));
    let driver: Arc<dyn LlmDriver> = Arc::new(MockLlmDriver::new());
    let ring = Arc::new(Ring::new(config.clone(), memory, driver));

    let state = AppState {
        ring: ring.clone(),
        started_at: chrono::Utc::now(),
        config: Arc::new(config.clone()),
        a2a: A2AState::new(A2ARegistry::our_card(
            "test-agent",
            "http://localhost:0",
            vec![],
        )),
        channel_router: Arc::new(punch_channels::router::ChannelRouter::new()),
    };

    let app = build_router(state, "", rpm);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind test server");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestServer {
        base_url: format!("http://{}", addr),
        _ring: ring,
    }
}

// ---------------------------------------------------------------------------
// Health endpoint tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_health_endpoint() {
    let server = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/health", server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

// ---------------------------------------------------------------------------
// Chat completions endpoint tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_completions_non_streaming() {
    let server = start_test_server().await;
    let client = reqwest::Client::new();

    // First, spawn a fighter named "gpt-oss:20b" so the model lookup works.
    let spawn_resp = client
        .post(format!("{}/api/fighters", server.base_url))
        .json(&serde_json::json!({
            "manifest": {
                "name": "gpt-oss:20b",
                "description": "Test fighter",
                "model": {
                    "provider": "ollama",
                    "model": "gpt-oss:20b",
                    "base_url": "http://localhost:11434"
                },
                "system_prompt": "You are helpful",
                "capabilities": [],
                "weight_class": "middleweight"
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(spawn_resp.status(), 201);

    // Send chat completion request.
    let resp = client
        .post(format!("{}/v1/chat/completions", server.base_url))
        .json(&serde_json::json!({
            "model": "gpt-oss:20b",
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hello"}
            ],
            "max_tokens": 4096,
            "temperature": 0.7,
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    // Verify OpenAI-compatible response format.
    assert_eq!(body["object"], "chat.completion");
    assert!(body["id"].as_str().unwrap().starts_with("chatcmpl-"));
    assert!(body["created"].as_i64().is_some());
    assert_eq!(body["model"], "gpt-oss:20b");

    // Check choices.
    let choices = body["choices"].as_array().unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0]["index"], 0);
    assert_eq!(choices[0]["message"]["role"], "assistant");
    assert!(
        choices[0]["message"]["content"]
            .as_str()
            .unwrap()
            .contains("Hello")
    );
    assert_eq!(choices[0]["finish_reason"], "stop");

    // Check usage.
    assert!(body["usage"]["prompt_tokens"].as_u64().is_some());
    assert!(body["usage"]["completion_tokens"].as_u64().is_some());
    assert!(body["usage"]["total_tokens"].as_u64().is_some());
}

#[tokio::test]
async fn test_chat_completions_auto_spawn_fighter() {
    let server = start_test_server().await;
    let client = reqwest::Client::new();

    // Send a chat completion without pre-spawning a fighter.
    // The endpoint should auto-spawn a temporary fighter.
    let resp = client
        .post(format!("{}/v1/chat/completions", server.base_url))
        .json(&serde_json::json!({
            "model": "auto-test-model",
            "messages": [
                {"role": "user", "content": "Hello from auto-spawn"}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert!(
        body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .contains("Hello from auto-spawn")
    );
}

#[tokio::test]
async fn test_chat_completions_missing_user_message() {
    let server = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/chat/completions", server.base_url))
        .json(&serde_json::json!({
            "model": "test",
            "messages": [
                {"role": "system", "content": "You are helpful"}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("user message")
    );
}

#[tokio::test]
async fn test_chat_completions_streaming() {
    let server = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/chat/completions", server.base_url))
        .json(&serde_json::json!({
            "model": "stream-test",
            "messages": [
                {"role": "user", "content": "Stream test"}
            ],
            "stream": true
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    // Verify it's an SSE response.
    let content_type = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/event-stream"),
        "Expected SSE content type, got: {}",
        content_type
    );

    // Read the full body and parse SSE events.
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("data: "),
        "SSE body should contain data events"
    );
    assert!(
        body.contains("[DONE]"),
        "SSE body should contain [DONE] sentinel"
    );
    assert!(
        body.contains("chat.completion.chunk"),
        "SSE body should contain chunk objects"
    );
}

// ---------------------------------------------------------------------------
// Models endpoint tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_models_endpoint() {
    let server = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/v1/models", server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    assert_eq!(body["object"], "list");
    let data = body["data"].as_array().unwrap();
    assert!(!data.is_empty(), "Should have at least one model");

    // The configured default model should always be present.
    let has_default = data.iter().any(|m| m["id"] == "gpt-oss:20b");
    assert!(has_default, "Should include the configured default model");

    // Check model object format.
    let model = &data[0];
    assert_eq!(model["object"], "model");
    assert!(model["created"].as_i64().is_some());
    assert!(model["owned_by"].as_str().is_some());
}

// ---------------------------------------------------------------------------
// Auth middleware tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_auth_health_is_public() {
    let server = start_test_server_with_auth("secret-test-key").await;
    let client = reqwest::Client::new();

    // /health should be accessible without auth.
    let resp = client
        .get(format!("{}/health", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_auth_rejects_no_token() {
    let server = start_test_server_with_auth("secret-test-key").await;
    let client = reqwest::Client::new();

    // Protected endpoint without auth header should return 401.
    let resp = client
        .get(format!("{}/api/status", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Missing")
    );
}

#[tokio::test]
async fn test_auth_rejects_wrong_token() {
    let server = start_test_server_with_auth("secret-test-key").await;
    let client = reqwest::Client::new();

    // Wrong bearer token should return 401.
    let resp = client
        .get(format!("{}/api/status", server.base_url))
        .header("authorization", "Bearer wrong-key")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Invalid")
    );
}

#[tokio::test]
async fn test_auth_accepts_bearer_token() {
    let server = start_test_server_with_auth("secret-test-key").await;
    let client = reqwest::Client::new();

    // Correct bearer token should return 200.
    let resp = client
        .get(format!("{}/api/status", server.base_url))
        .header("authorization", "Bearer secret-test-key")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_auth_accepts_x_api_key() {
    let server = start_test_server_with_auth("secret-test-key").await;
    let client = reqwest::Client::new();

    // X-API-Key header should also work.
    let resp = client
        .get(format!("{}/api/status", server.base_url))
        .header("x-api-key", "secret-test-key")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_auth_disabled_when_no_key() {
    let server = start_test_server().await;
    let client = reqwest::Client::new();

    // When no API key is configured, all endpoints should be accessible.
    let resp = client
        .get(format!("{}/api/status", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// ---------------------------------------------------------------------------
// Rate limiting tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rate_limiting_returns_429() {
    // Set a very low rate limit for testing.
    let server = start_test_server_with_rate_limit(3).await;
    let client = reqwest::Client::new();

    // Send requests up to the limit.
    for i in 0..3 {
        let resp = client
            .get(format!("{}/api/status", server.base_url))
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            200,
            "Request {} should succeed (within limit)",
            i + 1
        );
    }

    // The next request should be rate limited.
    let resp = client
        .get(format!("{}/api/status", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 429);

    // Verify Retry-After header.
    assert!(resp.headers().contains_key("retry-after"));
}

#[tokio::test]
async fn test_rate_limiting_health_is_exempt() {
    // Set a very low rate limit for testing.
    let server = start_test_server_with_rate_limit(2).await;
    let client = reqwest::Client::new();

    // Exhaust the limit on /api/status.
    for _ in 0..2 {
        client
            .get(format!("{}/api/status", server.base_url))
            .send()
            .await
            .unwrap();
    }

    // Verify /api/status is rate limited.
    let resp = client
        .get(format!("{}/api/status", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 429);

    // /health should still work.
    let resp = client
        .get(format!("{}/health", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// ---------------------------------------------------------------------------
// Fighter endpoint tests (verify existing endpoints still work)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fighter_spawn_list_kill() {
    let server = start_test_server().await;
    let client = reqwest::Client::new();

    // Spawn a fighter.
    let resp = client
        .post(format!("{}/api/fighters", server.base_url))
        .json(&serde_json::json!({
            "manifest": {
                "name": "test-fighter",
                "description": "Integration test fighter",
                "model": {
                    "provider": "ollama",
                    "model": "test-model",
                    "base_url": "http://localhost:11434"
                },
                "system_prompt": "You are a test agent.",
                "capabilities": [],
                "weight_class": "middleweight"
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "test-fighter");
    let fighter_id = body["id"].as_str().unwrap().to_string();
    assert!(!fighter_id.is_empty());

    // List fighters.
    let resp = client
        .get(format!("{}/api/fighters", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let fighters: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(fighters.iter().any(|f| f["name"] == "test-fighter"));

    // Kill the fighter.
    let resp = client
        .delete(format!("{}/api/fighters/{}", server.base_url, fighter_id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
}

#[tokio::test]
async fn test_fighter_send_message() {
    let server = start_test_server().await;
    let client = reqwest::Client::new();

    // Spawn a fighter.
    let resp = client
        .post(format!("{}/api/fighters", server.base_url))
        .json(&serde_json::json!({
            "manifest": {
                "name": "msg-test-fighter",
                "description": "Message test",
                "model": {
                    "provider": "ollama",
                    "model": "test-model",
                    "base_url": "http://localhost:11434"
                },
                "system_prompt": "Be helpful.",
                "capabilities": [],
                "weight_class": "middleweight"
            }
        }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let fighter_id = body["id"].as_str().unwrap();

    // Send a message.
    let resp = client
        .post(format!(
            "{}/api/fighters/{}/message",
            server.base_url, fighter_id
        ))
        .json(&serde_json::json!({"message": "Hello from integration test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["response"].as_str().unwrap().is_empty());
    assert!(body["tokens_used"].as_u64().unwrap() > 0);
}

// ---------------------------------------------------------------------------
// OpenAI chat completions with v1 endpoints auth test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_completions_with_auth() {
    let server = start_test_server_with_auth("test-api-key").await;
    let client = reqwest::Client::new();

    // Without auth, /v1/chat/completions should return 401.
    let resp = client
        .post(format!("{}/v1/chat/completions", server.base_url))
        .json(&serde_json::json!({
            "model": "test",
            "messages": [{"role": "user", "content": "Hello"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // With auth, it should work.
    let resp = client
        .post(format!("{}/v1/chat/completions", server.base_url))
        .header("authorization", "Bearer test-api-key")
        .json(&serde_json::json!({
            "model": "test",
            "messages": [{"role": "user", "content": "Hello with auth"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
}
