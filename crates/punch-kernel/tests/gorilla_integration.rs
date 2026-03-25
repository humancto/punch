//! Integration test: gorilla autonomous execution end-to-end.
//!
//! Uses a mock LLM driver that returns a canned response so we can validate
//! the full lifecycle without making real API calls.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use punch_kernel::Ring;
use punch_memory::MemorySubstrate;
use punch_runtime::{CompletionRequest, CompletionResponse, LlmDriver, StopReason, TokenUsage};
use punch_types::{
    GorillaManifest, GorillaStatus, ModelConfig, Provider, PunchConfig, PunchResult,
};

// ---------------------------------------------------------------------------
// Mock LLM Driver
// ---------------------------------------------------------------------------

/// A mock LLM driver that counts calls and returns a fixed response.
struct MockLlmDriver {
    /// Number of completions requested.
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

        // Return a simple end-turn response.
        Ok(CompletionResponse {
            message: punch_types::Message {
                role: punch_types::Role::Assistant,
                content: "Autonomous tick completed. No new actions required.".to_string(),
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                timestamp: chrono::Utc::now(),
            },
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 20,
            },
            stop_reason: StopReason::EndTurn,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
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
        memory: punch_types::config::MemoryConfig {
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

fn test_gorilla_manifest(schedule: &str) -> GorillaManifest {
    GorillaManifest {
        name: "test-gorilla".to_string(),
        description: "A test gorilla for integration testing.".to_string(),
        schedule: schedule.to_string(),
        moves_required: vec!["memory_store".to_string(), "memory_recall".to_string()],
        settings_schema: None,
        dashboard_metrics: Vec::new(),
        system_prompt: Some("You are a test gorilla. Simply acknowledge the tick.".to_string()),
        model: None,
        capabilities: Vec::new(),
        weight_class: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gorilla_single_tick_with_mock_driver() {
    let config = test_config();
    let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory db"));
    let driver = Arc::new(MockLlmDriver::new());

    let gorilla_id = punch_types::GorillaId::new();
    let manifest = test_gorilla_manifest("every 30s");

    let result = punch_kernel::run_gorilla_tick(
        gorilla_id,
        &manifest,
        &config.default_model,
        &memory,
        &(driver.clone() as Arc<dyn LlmDriver>),
    )
    .await;

    assert!(result.is_ok(), "gorilla tick should succeed: {:?}", result);
    let result = result.unwrap();

    assert!(!result.response.is_empty(), "response should not be empty");
    assert_eq!(driver.calls(), 1, "should have made exactly one LLM call");
    assert_eq!(
        result.usage.total(),
        120,
        "should report correct token usage"
    );
}

#[tokio::test]
async fn gorilla_unleash_and_cage_via_ring() {
    let config = test_config();
    let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory db"));
    let driver: Arc<dyn LlmDriver> = Arc::new(MockLlmDriver::new());

    let ring = Arc::new(Ring::new(config, memory, driver));

    let manifest = test_gorilla_manifest("every 2s");
    let gorilla_id = ring.register_gorilla(manifest);

    // Unleash the gorilla.
    let result = ring.unleash_gorilla(&gorilla_id).await;
    assert!(result.is_ok(), "unleash should succeed: {:?}", result);

    // Verify it is in the unleashed state.
    let gorillas = ring.list_gorillas().await;
    let entry = gorillas.iter().find(|(id, _, _, _)| *id == gorilla_id);
    assert!(entry.is_some(), "gorilla should be listed");
    let (_, _, status, _) = entry.unwrap();
    assert_eq!(*status, GorillaStatus::Unleashed);

    // Verify the background executor knows about it.
    assert!(
        ring.background().is_running(&gorilla_id),
        "gorilla should be running in background"
    );

    // Wait for at least one tick to execute (schedule is every 2s).
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Cage the gorilla.
    let cage_result = ring.cage_gorilla(&gorilla_id).await;
    assert!(cage_result.is_ok(), "cage should succeed");

    // Verify it stopped.
    assert!(
        !ring.background().is_running(&gorilla_id),
        "gorilla should no longer be running"
    );

    let gorillas = ring.list_gorillas().await;
    let entry = gorillas.iter().find(|(id, _, _, _)| *id == gorilla_id);
    let (_, _, status, _) = entry.unwrap();
    assert_eq!(*status, GorillaStatus::Caged);
}

#[tokio::test]
async fn gorilla_tick_uses_default_model_fallback() {
    let manifest = GorillaManifest {
        name: "no-model-gorilla".to_string(),
        description: "Has no model override.".to_string(),
        schedule: "every 30s".to_string(),
        moves_required: Vec::new(),
        settings_schema: None,
        dashboard_metrics: Vec::new(),
        system_prompt: Some("Test prompt.".to_string()),
        model: None, // No model override.
        capabilities: Vec::new(),
        weight_class: None,
    };

    let default_model = ModelConfig {
        provider: Provider::Ollama,
        model: "my-custom-model".to_string(),
        api_key_env: None,
        base_url: Some("http://localhost:11434".to_string()),
        max_tokens: Some(2048),
        temperature: Some(0.5),
    };

    let fighter = punch_kernel::fighter_manifest_from_gorilla(&manifest, &default_model);

    assert_eq!(fighter.model.model, "my-custom-model");
    assert_eq!(fighter.model.provider, Provider::Ollama);
    assert_eq!(fighter.model.max_tokens, Some(2048));
}

#[tokio::test]
async fn gorilla_tick_uses_manifest_model_when_specified() {
    let manifest = GorillaManifest {
        name: "custom-model-gorilla".to_string(),
        description: "Has its own model.".to_string(),
        schedule: "every 30s".to_string(),
        moves_required: Vec::new(),
        settings_schema: None,
        dashboard_metrics: Vec::new(),
        system_prompt: Some("Test prompt.".to_string()),
        model: Some(ModelConfig {
            provider: Provider::Anthropic,
            model: "claude-sonnet-4-20250514".to_string(),
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            base_url: None,
            max_tokens: Some(8192),
            temperature: Some(0.3),
        }),
        capabilities: Vec::new(),
        weight_class: None,
    };

    let default_model = ModelConfig {
        provider: Provider::Ollama,
        model: "should-not-be-used".to_string(),
        api_key_env: None,
        base_url: None,
        max_tokens: Some(2048),
        temperature: Some(0.5),
    };

    let fighter = punch_kernel::fighter_manifest_from_gorilla(&manifest, &default_model);

    assert_eq!(fighter.model.model, "claude-sonnet-4-20250514");
    assert_eq!(fighter.model.provider, Provider::Anthropic);
}

#[tokio::test]
async fn gorilla_capabilities_derived_from_moves_required() {
    let manifest = GorillaManifest {
        name: "test".to_string(),
        description: "test".to_string(),
        schedule: "every 30s".to_string(),
        moves_required: vec![
            "read_file".to_string(),
            "write_file".to_string(),
            "memory_store".to_string(),
            "memory_recall".to_string(),
            "web_fetch".to_string(),
        ],
        settings_schema: None,
        dashboard_metrics: Vec::new(),
        system_prompt: None,
        model: None,
        capabilities: Vec::new(),
        weight_class: None,
    };

    let caps = manifest.effective_capabilities();

    // Should have FileRead, FileWrite, Memory, Network.
    assert!(
        caps.iter()
            .any(|c| matches!(c, punch_types::Capability::FileRead(_))),
        "should have FileRead capability"
    );
    assert!(
        caps.iter()
            .any(|c| matches!(c, punch_types::Capability::FileWrite(_))),
        "should have FileWrite capability"
    );
    assert!(
        caps.iter()
            .any(|c| matches!(c, punch_types::Capability::Memory)),
        "should have Memory capability"
    );
    assert!(
        caps.iter()
            .any(|c| matches!(c, punch_types::Capability::Network(_))),
        "should have Network capability"
    );
}

#[tokio::test]
async fn gorilla_double_unleash_returns_error() {
    let config = test_config();
    let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory db"));
    let driver: Arc<dyn LlmDriver> = Arc::new(MockLlmDriver::new());

    let ring = Arc::new(Ring::new(config, memory, driver));
    let manifest = test_gorilla_manifest("every 30s");
    let gorilla_id = ring.register_gorilla(manifest);

    let result1 = ring.unleash_gorilla(&gorilla_id).await;
    assert!(result1.is_ok());

    let result2 = ring.unleash_gorilla(&gorilla_id).await;
    assert!(result2.is_err(), "double unleash should fail");

    // Clean up.
    ring.shutdown();
}

#[tokio::test]
async fn gorilla_run_tick_via_ring() {
    let config = test_config();
    let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory db"));
    let driver: Arc<dyn LlmDriver> = Arc::new(MockLlmDriver::new());

    let ring = Arc::new(Ring::new(config, memory, driver));
    let manifest = test_gorilla_manifest("every 30s");
    let gorilla_id = ring.register_gorilla(manifest);

    // Run a single tick via the Ring method.
    let result = ring.run_gorilla_tick(&gorilla_id).await;
    assert!(
        result.is_ok(),
        "ring.run_gorilla_tick should succeed: {:?}",
        result
    );

    let result = result.unwrap();
    assert!(!result.response.is_empty());
}

#[tokio::test]
async fn gorilla_find_by_name() {
    let config = test_config();
    let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory db"));
    let driver: Arc<dyn LlmDriver> = Arc::new(MockLlmDriver::new());

    let ring = Arc::new(Ring::new(config, memory, driver));
    let manifest = test_gorilla_manifest("every 30s");
    let gorilla_id = ring.register_gorilla(manifest);

    let found = ring.find_gorilla_by_name("test-gorilla").await;
    assert_eq!(found, Some(gorilla_id));

    let found_case = ring.find_gorilla_by_name("TEST-GORILLA").await;
    assert_eq!(found_case, Some(gorilla_id));

    let not_found = ring.find_gorilla_by_name("nonexistent").await;
    assert_eq!(not_found, None);
}
