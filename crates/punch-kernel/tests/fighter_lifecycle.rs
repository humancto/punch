//! Integration tests for the complete fighter lifecycle.
//!
//! Tests spawn, list, kill, status transitions, and config preservation
//! through the Ring, using a mock LLM driver to avoid real API calls.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use punch_kernel::Ring;
use punch_memory::MemorySubstrate;
use punch_runtime::{CompletionRequest, CompletionResponse, LlmDriver, StopReason, TokenUsage};
use punch_types::{
    Capability, FighterManifest, FighterStatus, ModelConfig, Provider, PunchConfig, PunchResult,
    WeightClass,
};

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
    async fn complete(&self, _request: CompletionRequest) -> PunchResult<CompletionResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(CompletionResponse {
            message: punch_types::Message {
                role: punch_types::Role::Assistant,
                content: "Done.".to_string(),
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                content_parts: Vec::new(),
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
        budget: Default::default(),
    }
}

fn create_ring(driver: Arc<dyn LlmDriver>) -> Arc<Ring> {
    let config = test_config();
    let memory = Arc::new(
        MemorySubstrate::new(std::path::Path::new(":memory:")).expect("memory should init"),
    );
    Arc::new(Ring::new(config, memory, driver))
}

fn test_manifest(name: &str) -> FighterManifest {
    FighterManifest {
        name: name.to_string(),
        description: format!("{name} fighter"),
        model: ModelConfig {
            provider: Provider::Anthropic,
            model: "claude-sonnet-4-20250514".to_string(),
            api_key_env: None,
            base_url: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        },
        system_prompt: "You are helpful.".to_string(),
        capabilities: vec![Capability::Memory],
        weight_class: WeightClass::Middleweight,
        tenant_id: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Spawn a fighter with a manifest and verify it exists in the Ring.
#[tokio::test]
async fn test_fighter_spawn_and_exists() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));
    let manifest = test_manifest("Alpha");
    let id = ring.spawn_fighter(manifest).await;

    let entry = ring.get_fighter(&id);
    assert!(entry.is_some(), "spawned fighter should exist in Ring");

    let entry = entry.unwrap();
    assert_eq!(entry.manifest.name, "Alpha");
    assert_eq!(entry.status, FighterStatus::Idle);
    assert!(entry.current_bout.is_none());

    ring.shutdown();
}

/// Spawn multiple fighters and verify all are listed.
#[tokio::test]
async fn test_spawn_multiple_fighters_all_listed() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));

    let names = ["Fighter-A", "Fighter-B", "Fighter-C", "Fighter-D"];
    let mut ids = Vec::new();
    for name in &names {
        ids.push(ring.spawn_fighter(test_manifest(name)).await);
    }

    let listed = ring.list_fighters();
    assert_eq!(
        listed.len(),
        names.len(),
        "all spawned fighters should be listed"
    );

    // Verify each spawned ID appears in the listing.
    for id in &ids {
        assert!(
            listed.iter().any(|(lid, _, _)| lid == id),
            "fighter {} should appear in list",
            id
        );
    }

    ring.shutdown();
}

/// Kill a fighter and verify it is removed from the Ring.
#[tokio::test]
async fn test_fighter_kill_removes_from_ring() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));
    let id = ring.spawn_fighter(test_manifest("Doomed")).await;

    assert!(ring.get_fighter(&id).is_some());

    ring.kill_fighter(&id);

    assert!(
        ring.get_fighter(&id).is_none(),
        "killed fighter should no longer exist"
    );
    assert!(
        ring.list_fighters().is_empty(),
        "fighter list should be empty after kill"
    );

    ring.shutdown();
}

/// Full lifecycle: spawn, check status, kill, verify gone.
#[tokio::test]
async fn test_fighter_spawn_status_kill_lifecycle() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));
    let id = ring.spawn_fighter(test_manifest("Lifecycle")).await;

    // Check initial status.
    let entry = ring.get_fighter(&id).unwrap();
    assert_eq!(entry.status, FighterStatus::Idle);

    // Kill the fighter.
    ring.kill_fighter(&id);

    // Verify gone.
    assert!(ring.get_fighter(&id).is_none());
    assert!(ring.list_fighters().is_empty());

    ring.shutdown();
}

/// Spawn with custom model config and verify config is preserved.
#[tokio::test]
async fn test_fighter_custom_model_config_preserved() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));

    let mut manifest = test_manifest("CustomConfig");
    manifest.model = ModelConfig {
        provider: Provider::OpenAI,
        model: "gpt-4o".to_string(),
        api_key_env: Some("OPENAI_API_KEY".to_string()),
        base_url: Some("https://custom.openai.com".to_string()),
        max_tokens: Some(8192),
        temperature: Some(0.3),
    };
    manifest.weight_class = WeightClass::Heavyweight;
    manifest.capabilities = vec![Capability::Memory, Capability::FileRead("**".to_string())];

    let id = ring.spawn_fighter(manifest).await;
    let entry = ring.get_fighter(&id).unwrap();

    assert_eq!(entry.manifest.model.provider, Provider::OpenAI);
    assert_eq!(entry.manifest.model.model, "gpt-4o");
    assert_eq!(
        entry.manifest.model.base_url,
        Some("https://custom.openai.com".to_string())
    );
    assert_eq!(entry.manifest.model.max_tokens, Some(8192));
    assert_eq!(entry.manifest.model.temperature, Some(0.3));
    assert_eq!(entry.manifest.weight_class, WeightClass::Heavyweight);
    assert_eq!(entry.manifest.capabilities.len(), 2);

    ring.shutdown();
}

/// Attempt to kill a non-existent fighter and verify no panic.
/// The Ring logs a warning but does not error.
#[tokio::test]
async fn test_kill_nonexistent_fighter_no_panic() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));

    let fake_id = punch_types::FighterId::new();
    // Should not panic; Ring logs a warning.
    ring.kill_fighter(&fake_id);

    // Ring should still be operational.
    let id = ring.spawn_fighter(test_manifest("StillAlive")).await;
    assert!(ring.get_fighter(&id).is_some());

    ring.shutdown();
}

/// Spawning two fighters with the same name should produce distinct IDs.
#[tokio::test]
async fn test_same_name_fighters_distinct_ids() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));

    let id1 = ring.spawn_fighter(test_manifest("Duplicate")).await;
    let id2 = ring.spawn_fighter(test_manifest("Duplicate")).await;

    assert_ne!(id1, id2, "same-name fighters should have distinct IDs");
    assert_eq!(ring.list_fighters().len(), 2);

    ring.shutdown();
}

/// Kill one fighter while others remain.
#[tokio::test]
async fn test_kill_one_keeps_others() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));

    let id1 = ring.spawn_fighter(test_manifest("Keep")).await;
    let id2 = ring.spawn_fighter(test_manifest("Remove")).await;
    let id3 = ring.spawn_fighter(test_manifest("Keep2")).await;

    ring.kill_fighter(&id2);

    assert!(ring.get_fighter(&id1).is_some());
    assert!(ring.get_fighter(&id2).is_none());
    assert!(ring.get_fighter(&id3).is_some());
    assert_eq!(ring.list_fighters().len(), 2);

    ring.shutdown();
}
