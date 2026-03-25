//! Integration tests for Ring methods that were previously untested:
//! - ensure_creed: auto-creates creed on spawn with self-awareness
//! - send_message: full message flow through the fighter loop
//! - fighter_to_fighter: inter-agent messaging
//! - update_fighter_relationships: creed relationship tracking
//!
//! Run: cargo test -p punch-kernel --test ring_integration_test -- --nocapture

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
// Mock LLM driver
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

        Ok(CompletionResponse {
            message: punch_types::Message {
                role: punch_types::Role::Assistant,
                content: format!("[response-{}] {}", count, user_content),
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
// Test helpers
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
            provider: Provider::Ollama,
            model: "test-model".to_string(),
            api_key_env: None,
            base_url: Some("http://localhost:11434".to_string()),
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

/// ensure_creed creates a creed on spawn with self-awareness.
/// Note: spawn_fighter runs creed creation in a background tokio::spawn task.
#[tokio::test]
async fn test_ensure_creed_creates_on_spawn() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));
    let manifest = test_manifest("Creed-Fighter");
    let _id = ring.spawn_fighter(manifest.clone()).await;

    // Give the spawned background creed task time to complete
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let creed = ring
        .memory()
        .load_creed_by_name("Creed-Fighter")
        .await
        .expect("should load")
        .expect("creed should exist");

    assert_eq!(creed.fighter_name, "Creed-Fighter");
    assert_eq!(creed.bout_count, 0);
    // Self-awareness should be populated
    assert!(!creed.self_model.model_name.is_empty());

    ring.shutdown();
}

/// ensure_creed does not overwrite an existing creed.
#[tokio::test]
async fn test_ensure_creed_does_not_overwrite() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));

    // Create a creed manually first
    let mut creed = punch_types::Creed::new("Existing-Fighter");
    creed.identity = "I have a custom identity".to_string();
    creed.bout_count = 42;
    ring.memory().save_creed(&creed).await.expect("save creed");

    // Spawn fighter with the same name — should not overwrite
    let manifest = test_manifest("Existing-Fighter");
    let _id = ring.spawn_fighter(manifest).await;

    let loaded = ring
        .memory()
        .load_creed_by_name("Existing-Fighter")
        .await
        .expect("load")
        .expect("exists");

    assert_eq!(loaded.identity, "I have a custom identity");
    assert_eq!(loaded.bout_count, 42);

    ring.shutdown();
}

/// send_message runs the fighter loop and returns a response.
#[tokio::test]
async fn test_send_message_returns_response() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));
    let manifest = test_manifest("Responder");
    let id = ring.spawn_fighter(manifest).await;

    let result = ring
        .send_message(&id, "Hello fighter!".to_string())
        .await
        .expect("send_message should succeed");

    assert!(
        result.response.contains("Hello fighter!"),
        "response should echo input: {}",
        result.response
    );
    assert!(result.usage.total() > 0);

    // Fighter should return to Idle after responding
    let entry = ring.get_fighter(&id).expect("fighter should exist");
    assert_eq!(entry.status, FighterStatus::Idle);

    ring.shutdown();
}

/// send_message to nonexistent fighter returns error.
#[tokio::test]
async fn test_send_message_nonexistent_fighter() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));

    let fake_id = punch_types::FighterId::new();
    let result = ring.send_message(&fake_id, "Hello?".to_string()).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not found"),
        "should contain 'not found': {}",
        err
    );

    ring.shutdown();
}

/// fighter_to_fighter sends a message from one fighter to another.
#[tokio::test]
async fn test_fighter_to_fighter_communication() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));
    let id_a = ring.spawn_fighter(test_manifest("Alpha")).await;
    let id_b = ring.spawn_fighter(test_manifest("Bravo")).await;

    let result = ring
        .fighter_to_fighter(&id_a, &id_b, "Relay this message".to_string())
        .await
        .expect("f2f should succeed");

    // Response should contain the enriched message (with source context)
    assert!(
        result.response.contains("Alpha") || result.response.contains("Relay this message"),
        "response should include context: {}",
        result.response
    );

    ring.shutdown();
}

/// fighter_to_fighter with nonexistent source returns error.
#[tokio::test]
async fn test_fighter_to_fighter_nonexistent_source() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));
    let id_b = ring.spawn_fighter(test_manifest("Target")).await;
    let fake_id = punch_types::FighterId::new();

    let result = ring
        .fighter_to_fighter(&fake_id, &id_b, "Hello".to_string())
        .await;

    assert!(result.is_err());

    ring.shutdown();
}

/// fighter_to_fighter with nonexistent target returns error.
#[tokio::test]
async fn test_fighter_to_fighter_nonexistent_target() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));
    let id_a = ring.spawn_fighter(test_manifest("Source")).await;
    let fake_id = punch_types::FighterId::new();

    let result = ring
        .fighter_to_fighter(&id_a, &fake_id, "Hello".to_string())
        .await;

    assert!(result.is_err());

    ring.shutdown();
}

/// update_fighter_relationships creates relationship entries in both creeds.
#[tokio::test]
async fn test_update_fighter_relationships() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));

    // Spawn both fighters (which creates creeds via ensure_creed)
    let _id_a = ring.spawn_fighter(test_manifest("RelA")).await;
    let _id_b = ring.spawn_fighter(test_manifest("RelB")).await;

    // Update relationships
    ring.update_fighter_relationships("RelA", "RelB").await;

    // Give the spawned background task time to complete
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Check that RelA has a relationship to RelB
    let creed_a = ring
        .memory()
        .load_creed_by_name("RelA")
        .await
        .expect("load")
        .expect("exists");

    let rel_to_b = creed_a.relationships.iter().find(|r| r.entity == "RelB");
    assert!(
        rel_to_b.is_some(),
        "RelA should have a relationship to RelB"
    );

    // Check that RelB has a relationship to RelA
    let creed_b = ring
        .memory()
        .load_creed_by_name("RelB")
        .await
        .expect("load")
        .expect("exists");

    let rel_to_a = creed_b.relationships.iter().find(|r| r.entity == "RelA");
    assert!(
        rel_to_a.is_some(),
        "RelB should have a relationship to RelA"
    );

    ring.shutdown();
}

/// update_fighter_relationships increments interaction_count on repeated calls.
#[tokio::test]
async fn test_relationship_interaction_count_increments() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));

    let _id_a = ring.spawn_fighter(test_manifest("CountA")).await;
    let _id_b = ring.spawn_fighter(test_manifest("CountB")).await;

    // First interaction
    ring.update_fighter_relationships("CountA", "CountB").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Second interaction
    ring.update_fighter_relationships("CountA", "CountB").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let creed_a = ring
        .memory()
        .load_creed_by_name("CountA")
        .await
        .expect("load")
        .expect("exists");

    let rel = creed_a
        .relationships
        .iter()
        .find(|r| r.entity == "CountB")
        .expect("relationship should exist");

    assert_eq!(
        rel.interaction_count, 2,
        "interaction_count should be 2 after two updates"
    );

    ring.shutdown();
}

/// Creed bout_count increments after send_message (end-to-end through Ring).
#[tokio::test]
async fn test_creed_evolves_through_ring_message() {
    let ring = create_ring(Arc::new(MockLlmDriver::new()));
    let manifest = test_manifest("Evolving");
    let id = ring.spawn_fighter(manifest).await;

    // Wait for background creed creation to complete
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Send a message — this runs the fighter loop which updates creed
    let _result = ring
        .send_message(&id, "Test evolution".to_string())
        .await
        .expect("should succeed");

    // Give async reflection time
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let creed = ring
        .memory()
        .load_creed_by_name("Evolving")
        .await
        .expect("load")
        .expect("exists");

    assert_eq!(
        creed.bout_count, 1,
        "bout_count should be 1 after one message"
    );

    ring.shutdown();
}
