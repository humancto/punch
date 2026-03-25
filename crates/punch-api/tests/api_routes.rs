//! Integration tests for the Punch Arena HTTP API routes.
//!
//! Tests cover the status endpoint, fighter CRUD via REST, chat completions
//! (non-streaming and streaming), model listing, error responses, and
//! concurrent request handling.

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

        let response = format!("[mock-{}] {}", count, user_content);

        Ok(CompletionResponse {
            message: punch_types::Message {
                role: punch_types::Role::Assistant,
                content: response,
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
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
// Test Infrastructure
// ---------------------------------------------------------------------------

fn test_config() -> PunchConfig {
    PunchConfig {
        api_listen: "127.0.0.1:0".to_string(),
        api_key: String::new(),
        rate_limit_rpm: 60,
        default_model: ModelConfig {
            provider: Provider::Ollama,
            model: "test-model".to_string(),
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
    }
}

struct TestServer {
    base_url: String,
    ring: Arc<Ring>,
}

async fn start_server() -> TestServer {
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

    let app = build_router(state, "", 60);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestServer {
        base_url: format!("http://{}", addr),
        ring,
    }
}

// ---------------------------------------------------------------------------
// Status endpoint
// ---------------------------------------------------------------------------

/// GET /api/status returns 200 with expected fields.
#[tokio::test]
async fn test_status_endpoint_fields() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/api/status", server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("uptime_secs").is_some());
    assert!(body.get("fighter_count").is_some());
}

// ---------------------------------------------------------------------------
// Fighter CRUD
// ---------------------------------------------------------------------------

/// POST /api/fighters creates a fighter and returns 201.
#[tokio::test]
async fn test_create_fighter_returns_201() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/fighters", server.base_url))
        .json(&serde_json::json!({
            "manifest": {
                "name": "route-test-fighter",
                "description": "API route test",
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
    assert_eq!(body["name"], "route-test-fighter");
    assert!(body["id"].as_str().is_some());
}

/// GET /api/fighters lists all spawned fighters.
#[tokio::test]
async fn test_list_fighters_after_spawn() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    // Spawn two fighters.
    for name in &["fighter-a", "fighter-b"] {
        client
            .post(format!("{}/api/fighters", server.base_url))
            .json(&serde_json::json!({
                "manifest": {
                    "name": name,
                    "description": "test",
                    "model": {
                        "provider": "ollama",
                        "model": "test-model",
                        "base_url": "http://localhost:11434"
                    },
                    "system_prompt": "test",
                    "capabilities": [],
                    "weight_class": "featherweight"
                }
            }))
            .send()
            .await
            .unwrap();
    }

    let resp = client
        .get(format!("{}/api/fighters", server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let fighters: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(fighters.len() >= 2);
}

/// DELETE /api/fighters/:id removes a fighter.
#[tokio::test]
async fn test_delete_fighter_returns_204() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/fighters", server.base_url))
        .json(&serde_json::json!({
            "manifest": {
                "name": "to-delete",
                "description": "will be deleted",
                "model": {
                    "provider": "ollama",
                    "model": "test-model",
                    "base_url": "http://localhost:11434"
                },
                "system_prompt": "test",
                "capabilities": [],
                "weight_class": "middleweight"
            }
        }))
        .send()
        .await
        .unwrap();

    let body: serde_json::Value = resp.json().await.unwrap();
    let id = body["id"].as_str().unwrap();

    let del_resp = client
        .delete(format!("{}/api/fighters/{}", server.base_url, id))
        .send()
        .await
        .unwrap();

    assert_eq!(del_resp.status(), 204);

    // Verify it's gone from the listing.
    let list_resp = client
        .get(format!("{}/api/fighters", server.base_url))
        .send()
        .await
        .unwrap();
    let fighters: Vec<serde_json::Value> = list_resp.json().await.unwrap();
    assert!(
        !fighters.iter().any(|f| f["id"].as_str() == Some(id)),
        "deleted fighter should not appear in list"
    );
}

// ---------------------------------------------------------------------------
// Message sending
// ---------------------------------------------------------------------------

/// POST /api/fighters/:id/message returns a response from the mock driver.
#[tokio::test]
async fn test_send_message_returns_response() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/fighters", server.base_url))
        .json(&serde_json::json!({
            "manifest": {
                "name": "msg-fighter",
                "description": "message test",
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

    let msg_resp = client
        .post(format!(
            "{}/api/fighters/{}/message",
            server.base_url, fighter_id
        ))
        .json(&serde_json::json!({"message": "Hello from route test"}))
        .send()
        .await
        .unwrap();

    assert_eq!(msg_resp.status(), 200);
    let msg_body: serde_json::Value = msg_resp.json().await.unwrap();
    assert!(!msg_body["response"].as_str().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Chat completions
// ---------------------------------------------------------------------------

/// POST /v1/chat/completions returns OpenAI-compatible format.
#[tokio::test]
async fn test_chat_completions_response_format() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/chat/completions", server.base_url))
        .json(&serde_json::json!({
            "model": "auto-test",
            "messages": [
                {"role": "user", "content": "Hello"}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    assert_eq!(body["object"], "chat.completion");
    assert!(body["id"].as_str().unwrap().starts_with("chatcmpl-"));
    assert!(body["choices"].as_array().unwrap().len() >= 1);
    assert_eq!(body["choices"][0]["message"]["role"], "assistant");
    assert!(body["usage"]["total_tokens"].as_u64().is_some());
}

/// POST /v1/chat/completions with empty messages returns 400.
#[tokio::test]
async fn test_chat_completions_empty_messages_400() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/chat/completions", server.base_url))
        .json(&serde_json::json!({
            "model": "test",
            "messages": [
                {"role": "system", "content": "system only"}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400);
}

/// POST /v1/chat/completions with stream=true returns SSE.
#[tokio::test]
async fn test_chat_completions_streaming_sse() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/chat/completions", server.base_url))
        .json(&serde_json::json!({
            "model": "stream-test",
            "messages": [
                {"role": "user", "content": "Stream it"}
            ],
            "stream": true
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("text/event-stream"), "should be SSE: {}", ct);

    let body = resp.text().await.unwrap();
    assert!(body.contains("data: "));
    assert!(body.contains("[DONE]"));
}

// ---------------------------------------------------------------------------
// Models endpoint
// ---------------------------------------------------------------------------

/// GET /v1/models returns the default model.
#[tokio::test]
async fn test_models_lists_default() {
    let server = start_server().await;
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
    assert!(!data.is_empty());
    assert!(data.iter().any(|m| m["id"] == "test-model"));
}

// ---------------------------------------------------------------------------
// Health endpoint
// ---------------------------------------------------------------------------

/// GET /health returns 200 with status "ok".
#[tokio::test]
async fn test_health_ok() {
    let server = start_server().await;
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
// Error handling
// ---------------------------------------------------------------------------

/// Unknown routes return 404.
#[tokio::test]
async fn test_unknown_route_returns_404() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/api/nonexistent", server.base_url))
        .send()
        .await
        .unwrap();

    // axum returns 404 for unmatched routes.
    assert!(
        resp.status() == 404 || resp.status() == 405,
        "expected 404 or 405, got {}",
        resp.status()
    );
}

// ---------------------------------------------------------------------------
// Concurrent requests
// ---------------------------------------------------------------------------

/// Multiple concurrent requests are handled without panics.
#[tokio::test]
async fn test_concurrent_requests_succeed() {
    let server = start_server().await;
    let client = reqwest::Client::new();

    let mut handles = Vec::new();
    for _ in 0..5 {
        let url = format!("{}/health", server.base_url);
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            let resp = c.get(&url).send().await.unwrap();
            assert_eq!(resp.status(), 200);
        }));
    }

    for h in handles {
        h.await.expect("concurrent request should succeed");
    }
}

/// The Ring remains accessible through the server under concurrent load.
#[tokio::test]
async fn test_ring_accessible_through_server() {
    let server = start_server().await;

    // The Ring should be accessible even though the server is running.
    let fighters = server.ring.list_fighters();
    assert!(fighters.is_empty(), "no fighters spawned yet");
}
