//! Integration tests for multi-tenancy: tenant registry, scoped fighters,
//! quota enforcement, and namespace isolation.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use punch_kernel::Ring;
use punch_memory::MemorySubstrate;
use punch_runtime::{CompletionRequest, CompletionResponse, LlmDriver, StopReason, TokenUsage};
use punch_types::{
    Capability, FighterManifest, ModelConfig, Provider, PunchConfig, PunchResult, TenantQuota,
    TenantStatus, WeightClass,
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
    }
}

fn test_manifest(name: &str) -> FighterManifest {
    FighterManifest {
        name: name.to_string(),
        description: format!("{} description", name),
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
        weight_class: WeightClass::Middleweight,
        tenant_id: None,
    }
}

async fn create_ring() -> Arc<Ring> {
    let config = test_config();
    let memory = Arc::new(MemorySubstrate::in_memory().unwrap());
    let driver = Arc::new(MockLlmDriver::new());
    Arc::new(Ring::new(config, memory, driver))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_register_tenant_and_verify_stored() {
    let ring = create_ring().await;

    let tenant = ring
        .tenant_registry()
        .register_tenant("Acme Corp".to_string(), TenantQuota::default());

    assert_eq!(tenant.name, "Acme Corp");
    assert_eq!(tenant.status, TenantStatus::Active);
    assert!(tenant.api_key.starts_with("pk_"));

    // Verify it can be looked up.
    let found = ring.tenant_registry().get_tenant(&tenant.id);
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "Acme Corp");
}

#[tokio::test]
async fn test_lookup_by_api_key() {
    let ring = create_ring().await;

    let tenant = ring
        .tenant_registry()
        .register_tenant("KeyTest".to_string(), TenantQuota::default());

    let found = ring
        .tenant_registry()
        .get_tenant_by_api_key(&tenant.api_key);
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, tenant.id);

    // Invalid key returns None.
    assert!(
        ring.tenant_registry()
            .get_tenant_by_api_key("invalid-key")
            .is_none()
    );
}

#[tokio::test]
async fn test_spawn_fighter_with_tenant_scoped() {
    let ring = create_ring().await;

    let tenant = ring
        .tenant_registry()
        .register_tenant("TenantA".to_string(), TenantQuota::default());

    let manifest = test_manifest("Fighter1");
    let fighter_id = ring
        .spawn_fighter_for_tenant(&tenant.id, manifest)
        .await
        .unwrap();

    // Fighter should exist and be scoped to tenant.
    let entry = ring.get_fighter(&fighter_id).unwrap();
    assert_eq!(entry.manifest.tenant_id, Some(tenant.id));
}

#[tokio::test]
async fn test_list_fighters_returns_only_tenant_fighters() {
    let ring = create_ring().await;

    let t1 = ring
        .tenant_registry()
        .register_tenant("TenantA".to_string(), TenantQuota::default());
    let t2 = ring
        .tenant_registry()
        .register_tenant("TenantB".to_string(), TenantQuota::default());

    // Spawn one fighter for each tenant.
    ring.spawn_fighter_for_tenant(&t1.id, test_manifest("A-Fighter"))
        .await
        .unwrap();
    ring.spawn_fighter_for_tenant(&t2.id, test_manifest("B-Fighter"))
        .await
        .unwrap();

    // Also spawn a global fighter (no tenant).
    ring.spawn_fighter(test_manifest("Global")).await;

    // Tenant A should only see A-Fighter.
    let t1_fighters = ring.list_fighters_for_tenant(&t1.id);
    assert_eq!(t1_fighters.len(), 1);
    assert_eq!(t1_fighters[0].1.name, "A-Fighter");

    // Tenant B should only see B-Fighter.
    let t2_fighters = ring.list_fighters_for_tenant(&t2.id);
    assert_eq!(t2_fighters.len(), 1);
    assert_eq!(t2_fighters[0].1.name, "B-Fighter");

    // Global list_fighters shows all 3.
    let all = ring.list_fighters();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn test_quota_enforcement_blocks_when_limit_reached() {
    let ring = create_ring().await;

    let quota = TenantQuota {
        max_fighters: 2,
        ..TenantQuota::default()
    };
    let tenant = ring
        .tenant_registry()
        .register_tenant("SmallTenant".to_string(), quota);

    // Spawn 2 fighters (should succeed).
    ring.spawn_fighter_for_tenant(&tenant.id, test_manifest("F1"))
        .await
        .unwrap();
    ring.spawn_fighter_for_tenant(&tenant.id, test_manifest("F2"))
        .await
        .unwrap();

    // Spawn a 3rd should fail.
    let result = ring
        .spawn_fighter_for_tenant(&tenant.id, test_manifest("F3"))
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("max fighters"),
        "expected quota error, got: {}",
        err
    );
}

#[tokio::test]
async fn test_suspend_tenant_blocks_all_requests() {
    let ring = create_ring().await;

    let tenant = ring
        .tenant_registry()
        .register_tenant("SuspendMe".to_string(), TenantQuota::default());

    ring.tenant_registry().suspend_tenant(&tenant.id).unwrap();

    // Spawning a fighter for a suspended tenant should fail.
    let result = ring
        .spawn_fighter_for_tenant(&tenant.id, test_manifest("Blocked"))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("suspended"));
}

#[tokio::test]
async fn test_delete_tenant_cleans_up_resources() {
    let ring = create_ring().await;

    let tenant = ring
        .tenant_registry()
        .register_tenant("ToDelete".to_string(), TenantQuota::default());

    // Spawn a fighter for this tenant.
    let fighter_id = ring
        .spawn_fighter_for_tenant(&tenant.id, test_manifest("DeleteMe"))
        .await
        .unwrap();

    // Kill the fighters belonging to the tenant, then delete tenant.
    let tenant_fighters = ring.list_fighters_for_tenant(&tenant.id);
    for (fid, _, _) in &tenant_fighters {
        ring.kill_fighter(fid);
    }

    ring.tenant_registry().delete_tenant(&tenant.id);

    // Tenant should be gone.
    assert!(ring.tenant_registry().get_tenant(&tenant.id).is_none());

    // Fighter should also be gone.
    assert!(ring.get_fighter(&fighter_id).is_none());
}

#[tokio::test]
async fn test_two_tenants_same_fighter_name_namespace_isolation() {
    let ring = create_ring().await;

    let t1 = ring
        .tenant_registry()
        .register_tenant("TenantA".to_string(), TenantQuota::default());
    let t2 = ring
        .tenant_registry()
        .register_tenant("TenantB".to_string(), TenantQuota::default());

    // Both tenants can have fighters named "Alpha".
    let f1 = ring
        .spawn_fighter_for_tenant(&t1.id, test_manifest("Alpha"))
        .await
        .unwrap();
    let f2 = ring
        .spawn_fighter_for_tenant(&t2.id, test_manifest("Alpha"))
        .await
        .unwrap();

    // They should have different IDs.
    assert_ne!(f1, f2);

    // Each tenant's list should show only their "Alpha".
    let t1_list = ring.list_fighters_for_tenant(&t1.id);
    assert_eq!(t1_list.len(), 1);
    assert_eq!(t1_list[0].0, f1);

    let t2_list = ring.list_fighters_for_tenant(&t2.id);
    assert_eq!(t2_list.len(), 1);
    assert_eq!(t2_list[0].0, f2);
}

#[tokio::test]
async fn test_kill_fighter_validates_tenant_ownership() {
    let ring = create_ring().await;

    let t1 = ring
        .tenant_registry()
        .register_tenant("TenantA".to_string(), TenantQuota::default());
    let t2 = ring
        .tenant_registry()
        .register_tenant("TenantB".to_string(), TenantQuota::default());

    let fighter = ring
        .spawn_fighter_for_tenant(&t1.id, test_manifest("Owned"))
        .await
        .unwrap();

    // Tenant B should not be able to kill Tenant A's fighter.
    let result = ring.kill_fighter_for_tenant(&fighter, &t2.id);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("does not belong"));

    // Tenant A can kill their own fighter.
    let result = ring.kill_fighter_for_tenant(&fighter, &t1.id);
    assert!(result.is_ok());
    assert!(ring.get_fighter(&fighter).is_none());
}

#[tokio::test]
async fn test_backward_compat_single_tenant_mode() {
    let ring = create_ring().await;

    // Spawning without tenant should still work (single-tenant mode).
    let manifest = test_manifest("SingleTenant");
    let fighter_id = ring.spawn_fighter(manifest).await;

    let entry = ring.get_fighter(&fighter_id).unwrap();
    assert!(entry.manifest.tenant_id.is_none());

    // list_fighters returns all fighters including tenant-less ones.
    let all = ring.list_fighters();
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn test_tenant_tool_access_check() {
    let ring = create_ring().await;

    // Tenant with restricted tools.
    let restricted_quota = TenantQuota {
        max_tools: vec!["read_file".to_string(), "web_fetch".to_string()],
        ..TenantQuota::default()
    };
    let restricted = ring
        .tenant_registry()
        .register_tenant("Restricted".to_string(), restricted_quota);

    // Tenant with no restrictions.
    let unrestricted = ring
        .tenant_registry()
        .register_tenant("Unrestricted".to_string(), TenantQuota::default());

    // Restricted tenant: allowed tools pass, disallowed tools fail.
    assert!(ring.check_tenant_tool_access(&restricted.id, "read_file"));
    assert!(ring.check_tenant_tool_access(&restricted.id, "web_fetch"));
    assert!(!ring.check_tenant_tool_access(&restricted.id, "shell_exec"));

    // Unrestricted tenant: all tools pass.
    assert!(ring.check_tenant_tool_access(&unrestricted.id, "read_file"));
    assert!(ring.check_tenant_tool_access(&unrestricted.id, "shell_exec"));
    assert!(ring.check_tenant_tool_access(&unrestricted.id, "anything"));
}

#[tokio::test]
async fn test_admin_can_see_all_tenants() {
    let ring = create_ring().await;

    ring.tenant_registry()
        .register_tenant("A".to_string(), TenantQuota::default());
    ring.tenant_registry()
        .register_tenant("B".to_string(), TenantQuota::default());
    ring.tenant_registry()
        .register_tenant("C".to_string(), TenantQuota::default());

    let all = ring.tenant_registry().list_tenants();
    assert_eq!(all.len(), 3);
}
