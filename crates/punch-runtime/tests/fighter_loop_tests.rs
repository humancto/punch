//! Integration tests for the fighter loop — the heart of the Punch runtime.
//!
//! Tests all StopReason paths, tool-calling multi-turn conversations,
//! empty response handling, MaxTokens continuation, loop guard enforcement,
//! and creed evolution.
//!
//! Run: cargo test -p punch-runtime --test fighter_loop_tests -- --nocapture

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use punch_memory::MemorySubstrate;
use punch_runtime::{
    CompletionRequest, CompletionResponse, FighterLoopParams, LlmDriver, StopReason, TokenUsage,
    run_fighter_loop,
};
use punch_types::{
    Capability, FighterId, FighterManifest, Message, ModelConfig, Provider, PunchResult, Role,
    ToolCall, ToolDefinition, WeightClass,
};

// ---------------------------------------------------------------------------
// Configurable mock LLM driver
// ---------------------------------------------------------------------------

/// A response script entry: what the mock should return on each call.
#[derive(Clone)]
struct ScriptedResponse {
    content: String,
    tool_calls: Vec<ToolCall>,
    stop_reason: StopReason,
}

/// A mock LLM driver that returns scripted responses in sequence.
/// When the script is exhausted, returns a default EndTurn response.
struct ScriptedMockDriver {
    script: Mutex<Vec<ScriptedResponse>>,
    call_count: AtomicU64,
}

impl ScriptedMockDriver {
    fn new(script: Vec<ScriptedResponse>) -> Self {
        Self {
            script: Mutex::new(script),
            call_count: AtomicU64::new(0),
        }
    }

    /// Simple driver that always returns text with EndTurn.
    fn text_only(response: &str) -> Self {
        Self::new(vec![ScriptedResponse {
            content: response.to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
        }])
    }
}

#[async_trait]
impl LlmDriver for ScriptedMockDriver {
    async fn complete(&self, _request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);

        let entry = {
            let mut script = self.script.lock().unwrap();
            if script.is_empty() {
                // Default: return done
                ScriptedResponse {
                    content: format!("[fallback-response-{}]", count),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                }
            } else {
                script.remove(0)
            }
        };

        Ok(CompletionResponse {
            message: Message {
                role: Role::Assistant,
                content: entry.content,
                tool_calls: entry.tool_calls,
                tool_results: Vec::new(),
                timestamp: chrono::Utc::now(),
            },
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
            },
            stop_reason: entry.stop_reason,
        })
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn test_memory() -> Arc<MemorySubstrate> {
    Arc::new(MemorySubstrate::new(std::path::Path::new(":memory:")).expect("memory init"))
}

fn test_manifest() -> FighterManifest {
    FighterManifest {
        name: "test-fighter".to_string(),
        description: "Test fighter".to_string(),
        model: ModelConfig {
            provider: Provider::Ollama,
            model: "test-model".to_string(),
            api_key_env: None,
            base_url: Some("http://localhost:11434".to_string()),
            max_tokens: Some(4096),
            temperature: Some(0.7),
        },
        system_prompt: "You are a test fighter.".to_string(),
        capabilities: vec![Capability::Memory],
        weight_class: WeightClass::Featherweight,
        tenant_id: None,
    }
}

fn make_params(
    driver: Arc<dyn LlmDriver>,
    memory: Arc<MemorySubstrate>,
    user_msg: &str,
) -> FighterLoopParams {
    let bout_id = punch_memory::BoutId::new();
    let fighter_id = FighterId::new();
    FighterLoopParams {
        manifest: test_manifest(),
        user_message: user_msg.to_string(),
        bout_id,
        fighter_id,
        memory,
        driver,
        available_tools: Vec::new(),
        max_iterations: Some(10),
        context_window: Some(200_000),
        tool_timeout_secs: Some(30),
        coordinator: None,
        approval_engine: None,
        sandbox: None,
        mcp_clients: None,
    }
}

fn memory_store_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: "memory_store".to_string(),
        description: "Store a key-value pair in memory.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "key": {"type": "string"},
                "value": {"type": "string"},
                "confidence": {"type": "number"}
            },
            "required": ["key", "value"]
        }),
        category: punch_types::ToolCategory::Memory,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// GAP 1: Happy path — single turn text response with EndTurn.
#[tokio::test]
async fn test_single_turn_text_response() {
    let driver = Arc::new(ScriptedMockDriver::text_only("Hello, I am your fighter."));
    let memory = test_memory();
    let params = make_params(driver, memory.clone(), "Hi there!");

    let result = run_fighter_loop(params).await.expect("loop should succeed");

    assert_eq!(result.response, "Hello, I am your fighter.");
    assert_eq!(result.tool_calls_made, 0);
    assert!(result.iterations == 0); // EndTurn on first call = 0 iterations recorded
    assert!(result.usage.input_tokens > 0);
    assert!(result.usage.output_tokens > 0);
}

/// GAP 1: Empty response on first iteration triggers one-shot retry.
#[tokio::test]
async fn test_empty_response_retry_on_first_iteration() {
    let driver = Arc::new(ScriptedMockDriver::new(vec![
        // First call: empty response
        ScriptedResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
        },
        // Second call (retry): real response
        ScriptedResponse {
            content: "Got it on retry!".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
        },
    ]));
    let memory = test_memory();
    let params = make_params(driver.clone(), memory, "Hello");

    let result = run_fighter_loop(params).await.expect("loop should succeed");

    assert_eq!(result.response, "Got it on retry!");
    // Driver was called twice
    assert_eq!(driver.call_count.load(Ordering::SeqCst), 2);
}

/// GAP 1: MaxTokens triggers continuation prompt, then EndTurn finishes.
#[tokio::test]
async fn test_max_tokens_continuation() {
    let driver = Arc::new(ScriptedMockDriver::new(vec![
        // First call: partial response with MaxTokens
        ScriptedResponse {
            content: "Here is part one of the answer...".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::MaxTokens,
        },
        // Second call (continuation): finish with EndTurn
        ScriptedResponse {
            content: "And here is the rest.".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
        },
    ]));
    let memory = test_memory();
    let params = make_params(driver.clone(), memory, "Tell me a long story");

    let result = run_fighter_loop(params).await.expect("loop should succeed");

    assert_eq!(result.response, "And here is the rest.");
    assert_eq!(driver.call_count.load(Ordering::SeqCst), 2);
}

/// GAP 1: MaxTokens exceeding MAX_CONTINUATION_LOOPS returns partial.
#[tokio::test]
async fn test_max_tokens_exceeded_returns_partial() {
    // Create 6 MaxTokens responses (exceeds MAX_CONTINUATION_LOOPS=5)
    let mut script = Vec::new();
    for i in 0..6 {
        script.push(ScriptedResponse {
            content: format!("Part {}", i),
            tool_calls: Vec::new(),
            stop_reason: StopReason::MaxTokens,
        });
    }
    // Add a final EndTurn that should never be reached
    script.push(ScriptedResponse {
        content: "Should not reach here".to_string(),
        tool_calls: Vec::new(),
        stop_reason: StopReason::EndTurn,
    });

    let driver = Arc::new(ScriptedMockDriver::new(script));
    let memory = test_memory();
    let params = make_params(driver.clone(), memory, "Overflow test");

    let result = run_fighter_loop(params).await.expect("loop should succeed");

    // Should return the partial response from the 6th MaxTokens (index 5)
    assert_eq!(result.response, "Part 5");
    // 1 initial + 5 continuations = 6 calls
    assert_eq!(driver.call_count.load(Ordering::SeqCst), 6);
}

/// GAP 1: Error stop reason returns an error.
#[tokio::test]
async fn test_error_stop_reason() {
    let driver = Arc::new(ScriptedMockDriver::new(vec![ScriptedResponse {
        content: String::new(),
        tool_calls: Vec::new(),
        stop_reason: StopReason::Error,
    }]));
    let memory = test_memory();
    let params = make_params(driver, memory, "Fail please");

    let result = run_fighter_loop(params).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("error"),
        "error should mention 'error': {}",
        err
    );
}

/// GAP 2: Multi-turn tool calling — LLM returns tool call, tool executes,
/// results feed back, LLM responds with text.
#[tokio::test]
async fn test_tool_calling_multi_turn() {
    let tool_call = ToolCall {
        id: "tc_001".to_string(),
        name: "memory_store".to_string(),
        input: serde_json::json!({
            "key": "test_key",
            "value": "test_value",
            "confidence": 0.9
        }),
    };

    let driver = Arc::new(ScriptedMockDriver::new(vec![
        // First call: LLM requests a tool call
        ScriptedResponse {
            content: "I'll store that for you.".to_string(),
            tool_calls: vec![tool_call],
            stop_reason: StopReason::ToolUse,
        },
        // Second call (after tool results): LLM provides final answer
        ScriptedResponse {
            content: "Done! I stored the value.".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
        },
    ]));

    let memory = test_memory();
    let mut params = make_params(driver.clone(), memory.clone(), "Remember test_key=test_value");
    params.available_tools = vec![memory_store_tool_def()];

    let result = run_fighter_loop(params).await.expect("loop should succeed");

    assert_eq!(result.response, "Done! I stored the value.");
    assert_eq!(result.tool_calls_made, 1);
    assert_eq!(driver.call_count.load(Ordering::SeqCst), 2);
}

/// GAP 2: Multiple tool calls in a single turn.
#[tokio::test]
async fn test_multiple_tool_calls_single_turn() {
    let tool_calls = vec![
        ToolCall {
            id: "tc_001".to_string(),
            name: "memory_store".to_string(),
            input: serde_json::json!({"key": "fruit", "value": "apple"}),
        },
        ToolCall {
            id: "tc_002".to_string(),
            name: "memory_store".to_string(),
            input: serde_json::json!({"key": "color", "value": "red"}),
        },
    ];

    let driver = Arc::new(ScriptedMockDriver::new(vec![
        ScriptedResponse {
            content: "Storing both values.".to_string(),
            tool_calls,
            stop_reason: StopReason::ToolUse,
        },
        ScriptedResponse {
            content: "Both values stored successfully.".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
        },
    ]));

    let memory = test_memory();
    let mut params = make_params(driver.clone(), memory, "Store fruit=apple and color=red");
    params.available_tools = vec![memory_store_tool_def()];

    let result = run_fighter_loop(params).await.expect("loop should succeed");

    assert_eq!(result.response, "Both values stored successfully.");
    assert_eq!(result.tool_calls_made, 2);
}

/// GAP 1: Empty response after tool use inserts fallback message.
#[tokio::test]
async fn test_empty_response_after_tool_use_fallback() {
    let tool_call = ToolCall {
        id: "tc_001".to_string(),
        name: "memory_store".to_string(),
        input: serde_json::json!({"key": "k", "value": "v"}),
    };

    let driver = Arc::new(ScriptedMockDriver::new(vec![
        // First: tool call
        ScriptedResponse {
            content: "Storing.".to_string(),
            tool_calls: vec![tool_call],
            stop_reason: StopReason::ToolUse,
        },
        // Second: empty response after tool results
        ScriptedResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
        },
    ]));

    let memory = test_memory();
    let mut params = make_params(driver, memory, "Store something");
    params.available_tools = vec![memory_store_tool_def()];

    let result = run_fighter_loop(params).await.expect("loop should succeed");

    // Should contain the fallback message
    assert!(
        result.response.contains("completed the requested operations"),
        "should get fallback: {}",
        result.response
    );
}

/// GAP 1: Token usage accumulates across multi-turn calls.
#[tokio::test]
async fn test_token_usage_accumulation() {
    let tool_call = ToolCall {
        id: "tc_001".to_string(),
        name: "memory_store".to_string(),
        input: serde_json::json!({"key": "k", "value": "v"}),
    };

    let driver = Arc::new(ScriptedMockDriver::new(vec![
        ScriptedResponse {
            content: "Storing.".to_string(),
            tool_calls: vec![tool_call],
            stop_reason: StopReason::ToolUse,
        },
        ScriptedResponse {
            content: "Done.".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
        },
    ]));

    let memory = test_memory();
    let mut params = make_params(driver, memory, "Store it");
    params.available_tools = vec![memory_store_tool_def()];

    let result = run_fighter_loop(params).await.expect("loop should succeed");

    // Each call returns 100 input + 50 output, two calls total
    assert_eq!(result.usage.input_tokens, 200);
    assert_eq!(result.usage.output_tokens, 100);
    assert_eq!(result.usage.total(), 300);
}

/// GAP 1: Messages are persisted to the memory substrate.
#[tokio::test]
async fn test_messages_persisted_to_memory() {
    let driver = Arc::new(ScriptedMockDriver::text_only("Persisted response"));
    let memory = test_memory();
    let bout_id = punch_memory::BoutId::new();
    let fighter_id = FighterId::new();

    let params = FighterLoopParams {
        manifest: test_manifest(),
        user_message: "Test persistence".to_string(),
        bout_id,
        fighter_id,
        memory: memory.clone(),
        driver,
        available_tools: Vec::new(),
        max_iterations: Some(10),
        context_window: Some(200_000),
        tool_timeout_secs: Some(30),
        coordinator: None,
        approval_engine: None,
        sandbox: None,
        mcp_clients: None,
    };

    let _result = run_fighter_loop(params).await.expect("loop should succeed");

    // Load messages back from memory
    let saved_messages = memory
        .load_messages(&bout_id)
        .await
        .expect("should load messages");

    // Should have at least user message + assistant response
    assert!(
        saved_messages.len() >= 2,
        "expected at least 2 messages, got {}",
        saved_messages.len()
    );

    let user_msg = saved_messages.iter().find(|m| m.role == Role::User);
    assert!(user_msg.is_some(), "should have user message");
    assert_eq!(user_msg.unwrap().content, "Test persistence");

    let assistant_msg = saved_messages.iter().find(|m| m.role == Role::Assistant);
    assert!(assistant_msg.is_some(), "should have assistant message");
    assert_eq!(assistant_msg.unwrap().content, "Persisted response");
}

/// GAP 3: Creed evolution — bout_count increments after a bout.
#[tokio::test]
async fn test_creed_bout_count_increments() {
    let driver = Arc::new(ScriptedMockDriver::text_only("I am evolving."));
    let memory = test_memory();
    let fighter_name = "creed-test-fighter";

    // Create a creed for this fighter
    let creed = punch_types::Creed::new(fighter_name);
    memory
        .save_creed(&creed)
        .await
        .expect("should save creed");

    let bout_id = punch_memory::BoutId::new();
    let fighter_id = FighterId::new();

    let mut manifest = test_manifest();
    manifest.name = fighter_name.to_string();

    let params = FighterLoopParams {
        manifest,
        user_message: "Test creed".to_string(),
        bout_id,
        fighter_id,
        memory: memory.clone(),
        driver,
        available_tools: Vec::new(),
        max_iterations: Some(10),
        context_window: Some(200_000),
        tool_timeout_secs: Some(30),
        coordinator: None,
        approval_engine: None,
        sandbox: None,
        mcp_clients: None,
    };

    let _result = run_fighter_loop(params).await.expect("loop should succeed");

    // Give the async reflection task a moment to complete
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Load creed and verify bout_count
    let updated_creed = memory
        .load_creed_by_name(fighter_name)
        .await
        .expect("should load creed")
        .expect("creed should exist");

    assert_eq!(
        updated_creed.bout_count, 1,
        "bout_count should increment from 0 to 1"
    );
}

/// GAP 3: Creed heartbeat tasks are marked as checked after a bout.
#[tokio::test]
async fn test_heartbeat_tasks_marked_checked() {
    let driver = Arc::new(ScriptedMockDriver::text_only("Heartbeat test."));
    let memory = test_memory();
    let fighter_name = "heartbeat-fighter";

    // Create a creed with an every_bout heartbeat task
    let mut creed = punch_types::Creed::new(fighter_name);
    creed.heartbeat.push(punch_types::HeartbeatTask {
        task: "Check system status".to_string(),
        cadence: "every_bout".to_string(),
        active: true,
        last_checked: None,
        execution_count: 0,
    });
    memory.save_creed(&creed).await.expect("save creed");

    let mut manifest = test_manifest();
    manifest.name = fighter_name.to_string();

    let params = FighterLoopParams {
        manifest,
        user_message: "Heartbeat check".to_string(),
        bout_id: punch_memory::BoutId::new(),
        fighter_id: FighterId::new(),
        memory: memory.clone(),
        driver,
        available_tools: Vec::new(),
        max_iterations: Some(10),
        context_window: Some(200_000),
        tool_timeout_secs: Some(30),
        coordinator: None,
        approval_engine: None,
        sandbox: None,
        mcp_clients: None,
    };

    let _result = run_fighter_loop(params).await.expect("loop should succeed");

    // Give async reflection a moment
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let updated = memory
        .load_creed_by_name(fighter_name)
        .await
        .expect("load creed")
        .expect("creed exists");

    assert!(
        updated.heartbeat[0].last_checked.is_some(),
        "heartbeat task should be marked as checked"
    );
    assert_eq!(
        updated.heartbeat[0].execution_count, 1,
        "execution_count should be 1"
    );
}

/// GAP 2: Tool call with unknown tool returns error result and loop continues.
#[tokio::test]
async fn test_unknown_tool_returns_error_and_continues() {
    let tool_call = ToolCall {
        id: "tc_unknown".to_string(),
        name: "nonexistent_tool".to_string(),
        input: serde_json::json!({}),
    };

    let driver = Arc::new(ScriptedMockDriver::new(vec![
        ScriptedResponse {
            content: "Calling unknown tool.".to_string(),
            tool_calls: vec![tool_call],
            stop_reason: StopReason::ToolUse,
        },
        ScriptedResponse {
            content: "I see the tool failed, sorry about that.".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
        },
    ]));

    let memory = test_memory();
    let params = make_params(driver.clone(), memory, "Call unknown tool");

    let result = run_fighter_loop(params).await.expect("loop should succeed");

    // The loop should have continued after the tool error
    assert_eq!(result.response, "I see the tool failed, sorry about that.");
    assert_eq!(result.tool_calls_made, 1);
    assert_eq!(driver.call_count.load(Ordering::SeqCst), 2);
}

/// GAP 1: LLM completion error propagates correctly.
#[tokio::test]
async fn test_llm_completion_error_propagates() {
    /// A driver that always errors on complete()
    struct ErrorDriver;

    #[async_trait]
    impl LlmDriver for ErrorDriver {
        async fn complete(&self, _request: CompletionRequest) -> PunchResult<CompletionResponse> {
            Err(punch_types::PunchError::Provider {
                provider: "test".to_string(),
                message: "Connection refused".to_string(),
            })
        }
    }

    let driver: Arc<dyn LlmDriver> = Arc::new(ErrorDriver);
    let memory = test_memory();
    let params = make_params(driver, memory, "This should fail");

    let result = run_fighter_loop(params).await;
    assert!(result.is_err());
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("Connection refused"),
        "should propagate error: {}",
        err_str
    );
}
