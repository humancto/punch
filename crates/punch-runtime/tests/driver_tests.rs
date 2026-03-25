//! Integration tests for the LLM driver abstraction layer, circuit breaker,
//! context budget management, and loop guard.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use punch_runtime::{
    CircuitStatus, CompletionRequest, CompletionResponse, ContextBudget, LlmDriver, LoopGuard,
    ProviderCircuitBreaker, StopReason, TokenUsage, strip_thinking_tags,
};
use punch_types::{Message, PunchResult, Role};

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

    fn calls(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmDriver for MockLlmDriver {
    async fn complete(&self, _request: CompletionRequest) -> PunchResult<CompletionResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(CompletionResponse {
            message: Message {
                role: Role::Assistant,
                content: "Mock response.".to_string(),
                content_parts: Vec::new(),
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                timestamp: chrono::Utc::now(),
            },
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        })
    }
}

struct FailingDriver;

#[async_trait]
impl LlmDriver for FailingDriver {
    async fn complete(&self, _request: CompletionRequest) -> PunchResult<CompletionResponse> {
        Err(punch_types::PunchError::Provider {
            provider: "mock".to_string(),
            message: "simulated failure".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_request(content: &str) -> CompletionRequest {
    CompletionRequest {
        model: "test-model".to_string(),
        messages: vec![Message::new(Role::User, content)],
        tools: Vec::new(),
        max_tokens: 4096,
        temperature: Some(0.7),
        system_prompt: None,
    }
}

// ---------------------------------------------------------------------------
// LlmDriver trait tests
// ---------------------------------------------------------------------------

/// Mock driver returns a valid CompletionResponse.
#[tokio::test]
async fn test_mock_driver_returns_response() {
    let driver = MockLlmDriver::new();
    let resp = driver.complete(make_request("Hello")).await.unwrap();

    assert_eq!(resp.message.role, Role::Assistant);
    assert_eq!(resp.message.content, "Mock response.");
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    assert_eq!(resp.usage.input_tokens, 10);
    assert_eq!(resp.usage.output_tokens, 5);
}

/// Mock driver tracks call count correctly.
#[tokio::test]
async fn test_mock_driver_call_count() {
    let driver = MockLlmDriver::new();
    assert_eq!(driver.calls(), 0);

    driver.complete(make_request("a")).await.unwrap();
    driver.complete(make_request("b")).await.unwrap();
    driver.complete(make_request("c")).await.unwrap();

    assert_eq!(driver.calls(), 3);
}

/// Failing driver propagates the error.
#[tokio::test]
async fn test_failing_driver_returns_error() {
    let driver = FailingDriver;
    let result = driver.complete(make_request("fail")).await;
    assert!(result.is_err());
}

/// LlmDriver is object-safe and can be used as Arc<dyn LlmDriver>.
#[tokio::test]
async fn test_driver_trait_object_safe() {
    let driver: Arc<dyn LlmDriver> = Arc::new(MockLlmDriver::new());
    let resp = driver.complete(make_request("trait object")).await.unwrap();
    assert_eq!(resp.message.role, Role::Assistant);
}

// ---------------------------------------------------------------------------
// TokenUsage tests
// ---------------------------------------------------------------------------

/// TokenUsage::total sums input and output tokens.
#[test]
fn test_token_usage_total() {
    let usage = TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
    };
    assert_eq!(usage.total(), 150);
}

/// TokenUsage::accumulate adds another usage on top.
#[test]
fn test_token_usage_accumulate() {
    let mut total = TokenUsage {
        input_tokens: 10,
        output_tokens: 5,
    };
    let other = TokenUsage {
        input_tokens: 20,
        output_tokens: 15,
    };
    total.accumulate(&other);
    assert_eq!(total.input_tokens, 30);
    assert_eq!(total.output_tokens, 20);
    assert_eq!(total.total(), 50);
}

/// Default TokenUsage is all zeros.
#[test]
fn test_token_usage_default() {
    let usage = TokenUsage::default();
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
    assert_eq!(usage.total(), 0);
}

// ---------------------------------------------------------------------------
// Circuit breaker tests
// ---------------------------------------------------------------------------

/// New circuit breaker starts in Closed state.
#[test]
fn test_circuit_breaker_starts_closed() {
    let cb = ProviderCircuitBreaker::new();
    assert_eq!(cb.get_status("anthropic"), CircuitStatus::Closed);
    assert!(cb.should_allow("anthropic"));
}

/// Circuit trips Open after reaching failure threshold.
#[test]
fn test_circuit_breaker_trips_on_threshold() {
    let cb = ProviderCircuitBreaker::with_config(3, Duration::from_secs(60));

    for _ in 0..3 {
        cb.record_failure("test-provider");
    }

    assert_eq!(cb.get_status("test-provider"), CircuitStatus::Open);
    assert!(!cb.should_allow("test-provider"));
}

/// Failures below threshold keep circuit closed.
#[test]
fn test_circuit_breaker_below_threshold_stays_closed() {
    let cb = ProviderCircuitBreaker::with_config(5, Duration::from_secs(60));

    cb.record_failure("provider");
    cb.record_failure("provider");

    assert_eq!(cb.get_status("provider"), CircuitStatus::Closed);
    assert!(cb.should_allow("provider"));
}

/// Success resets failure count and returns circuit to Closed.
#[test]
fn test_circuit_breaker_success_resets() {
    let cb = ProviderCircuitBreaker::with_config(3, Duration::from_secs(60));

    cb.record_failure("p");
    cb.record_failure("p");
    cb.record_success("p");

    assert_eq!(cb.get_status("p"), CircuitStatus::Closed);

    // The counter was reset, so 2 more failures should NOT trip it.
    cb.record_failure("p");
    cb.record_failure("p");
    assert_eq!(cb.get_status("p"), CircuitStatus::Closed);
}

/// Different providers have independent circuit states.
#[test]
fn test_circuit_breaker_providers_independent() {
    let cb = ProviderCircuitBreaker::with_config(2, Duration::from_secs(60));

    cb.record_failure("anthropic");
    cb.record_failure("anthropic");

    assert_eq!(cb.get_status("anthropic"), CircuitStatus::Open);
    assert_eq!(cb.get_status("openai"), CircuitStatus::Closed);
    assert!(cb.should_allow("openai"));
}

/// Circuit breaker transitions to HalfOpen after cooldown.
#[test]
fn test_circuit_breaker_half_open_after_cooldown() {
    // Use a zero-second cooldown for immediate transition.
    let cb = ProviderCircuitBreaker::with_config(1, Duration::from_secs(0));

    cb.record_failure("p");
    assert_eq!(cb.get_status("p"), CircuitStatus::Open);

    // With 0s cooldown, should_allow should transition to HalfOpen.
    assert!(cb.should_allow("p"));
    assert_eq!(cb.get_status("p"), CircuitStatus::HalfOpen);
}

/// Success in HalfOpen state closes the circuit.
#[test]
fn test_circuit_breaker_half_open_success_closes() {
    let cb = ProviderCircuitBreaker::with_config(1, Duration::from_secs(0));

    cb.record_failure("p");
    assert_eq!(cb.get_status("p"), CircuitStatus::Open);

    // Trigger transition to HalfOpen.
    cb.should_allow("p");
    assert_eq!(cb.get_status("p"), CircuitStatus::HalfOpen);

    // Success closes it.
    cb.record_success("p");
    assert_eq!(cb.get_status("p"), CircuitStatus::Closed);
}

// ---------------------------------------------------------------------------
// Think-tag stripping tests
// ---------------------------------------------------------------------------

/// strip_thinking_tags removes <think>...</think> blocks.
#[test]
fn test_strip_thinking_tags_removes_think() {
    let input = "<think>Some reasoning here</think>The actual response.";
    let result = strip_thinking_tags(input);
    assert!(
        !result.contains("<think>"),
        "should remove think tags: {}",
        result
    );
    assert!(result.contains("The actual response."));
}

/// strip_thinking_tags passes through clean text unchanged.
#[test]
fn test_strip_thinking_tags_no_tags() {
    let input = "Just a normal response with no tags.";
    let result = strip_thinking_tags(input);
    assert_eq!(result.trim(), input);
}

/// strip_thinking_tags handles multiple think blocks.
#[test]
fn test_strip_thinking_tags_multiple() {
    let input =
        "<think>first thought</think>Answer A. <thinking>second thought</thinking>Answer B.";
    let result = strip_thinking_tags(input);
    assert!(result.contains("Answer A."));
    assert!(result.contains("Answer B."));
}

// ---------------------------------------------------------------------------
// Context budget tests
// ---------------------------------------------------------------------------

/// ContextBudget estimates tokens from messages using chars/4 heuristic.
#[test]
fn test_context_budget_estimate_tokens() {
    let budget = ContextBudget::new(200_000);
    // 400 chars / 4 = 100 tokens.
    let msg = Message::new(Role::User, &"x".repeat(400));
    let tokens = budget.estimate_tokens(&[msg], &[]);
    assert_eq!(tokens, 100);
}

/// ContextBudget returns no trim needed for small messages.
#[test]
fn test_context_budget_no_trim_needed() {
    let budget = ContextBudget::new(200_000);
    let msgs = vec![Message::new(Role::User, "hello")];
    assert!(budget.check_trim_needed(&msgs, &[]).is_none());
}

/// ContextBudget returns moderate trim when usage exceeds 70%.
#[test]
fn test_context_budget_moderate_trim() {
    let budget = ContextBudget::new(1_000); // 1K token window.
    // 750 tokens * 4 chars = 3000 chars -> 75% of window.
    let msgs = vec![Message::new(Role::User, &"x".repeat(3000))];
    let action = budget.check_trim_needed(&msgs, &[]);
    assert_eq!(action, Some(punch_runtime::TrimAction::Moderate));
}

/// ContextBudget returns aggressive trim when usage exceeds 90%.
#[test]
fn test_context_budget_aggressive_trim() {
    let budget = ContextBudget::new(1_000); // 1K token window.
    // 950 tokens * 4 chars = 3800 chars -> 95% of window.
    let msgs = vec![Message::new(Role::User, &"x".repeat(3800))];
    let action = budget.check_trim_needed(&msgs, &[]);
    assert_eq!(action, Some(punch_runtime::TrimAction::Aggressive));
}

/// ContextBudget default window size is 200K tokens.
#[test]
fn test_context_budget_default() {
    let budget = ContextBudget::default();
    assert_eq!(budget.window_size, 200_000);
}

/// ContextBudget truncate_result preserves short results.
#[test]
fn test_context_budget_truncate_short() {
    let result = ContextBudget::truncate_result("short text", 100);
    assert_eq!(result, "short text");
}

/// ContextBudget truncate_result adds marker for long results.
#[test]
fn test_context_budget_truncate_long() {
    let text = "a".repeat(1000);
    let result = ContextBudget::truncate_result(&text, 200);
    assert!(result.contains("[truncated"));
    assert!(result.len() < 1000);
}

// ---------------------------------------------------------------------------
// Loop guard tests
// ---------------------------------------------------------------------------

/// LoopGuard allows tool calls within the iteration limit.
#[test]
fn test_loop_guard_within_limit() {
    let mut guard = LoopGuard::new(5, 3);
    let tool_call = punch_types::ToolCall {
        id: "call_1".to_string(),
        name: "ls".to_string(),
        input: serde_json::json!({}),
    };

    let verdict = guard.record_tool_calls(&[tool_call]);
    assert!(
        matches!(verdict, punch_runtime::LoopGuardVerdict::Continue),
        "should allow within limit"
    );
}

/// LoopGuard breaks after exceeding max iterations.
#[test]
fn test_loop_guard_breaks_at_limit() {
    let mut guard = LoopGuard::new(3, 3);

    for i in 0..4 {
        let tool_call = punch_types::ToolCall {
            id: format!("call_{i}"),
            name: format!("tool_{i}"),
            input: serde_json::json!({"unique": i}),
        };
        let verdict = guard.record_tool_calls(&[tool_call]);
        if i >= 3 {
            assert!(
                matches!(verdict, punch_runtime::LoopGuardVerdict::Break(_)),
                "should break at iteration {}: {:?}",
                i,
                verdict
            );
        }
    }
}

// ---------------------------------------------------------------------------
// StopReason serialization
// ---------------------------------------------------------------------------

/// StopReason serializes to expected JSON values.
#[test]
fn test_stop_reason_serde() {
    let reasons = vec![
        StopReason::EndTurn,
        StopReason::ToolUse,
        StopReason::MaxTokens,
        StopReason::Error,
    ];

    for reason in reasons {
        let json = serde_json::to_string(&reason).expect("serialize");
        let deser: StopReason = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser, reason);
    }
}

/// CompletionResponse roundtrips through serde.
#[test]
fn test_completion_response_serde_roundtrip() {
    let resp = CompletionResponse {
        message: Message {
            role: Role::Assistant,
            content: "Test".to_string(),
            content_parts: Vec::new(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            timestamp: chrono::Utc::now(),
        },
        stop_reason: StopReason::EndTurn,
        usage: TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
        },
    };

    let json = serde_json::to_string(&resp).expect("serialize");
    let deser: CompletionResponse = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(deser.message.role, Role::Assistant);
    assert_eq!(deser.usage.total(), 150);
    assert_eq!(deser.stop_reason, StopReason::EndTurn);
}
