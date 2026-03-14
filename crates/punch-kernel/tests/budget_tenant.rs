//! Integration tests for budget enforcement and tenant registry.
//!
//! Tests cover per-fighter budget limits, global limits, verdict logic,
//! tenant lifecycle (register, suspend, activate, delete), and API key lookup.

use std::sync::Arc;

use punch_kernel::{
    BudgetEnforcer, BudgetLimit, BudgetVerdict, MeteringEngine, TenantRegistry,
};
use punch_memory::MemorySubstrate;
use punch_types::{
    FighterId, FighterManifest, FighterStatus, ModelConfig, Provider, TenantQuota, TenantStatus,
    WeightClass,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_manifest() -> FighterManifest {
    FighterManifest {
        name: "budget-test".into(),
        description: "test".into(),
        model: ModelConfig {
            provider: Provider::Anthropic,
            model: "claude-sonnet-4-20250514".into(),
            api_key_env: None,
            base_url: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        },
        system_prompt: "test".into(),
        capabilities: Vec::new(),
        weight_class: WeightClass::Featherweight,
        tenant_id: None,
    }
}

async fn setup() -> (Arc<MeteringEngine>, Arc<MemorySubstrate>) {
    let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory substrate"));
    let metering = Arc::new(MeteringEngine::new(Arc::clone(&memory)));
    (metering, memory)
}

async fn setup_fighter(memory: &MemorySubstrate) -> FighterId {
    let fid = FighterId::new();
    memory
        .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
        .await
        .expect("save fighter");
    fid
}

// ===========================================================================
// Budget enforcement tests
// ===========================================================================

/// Fighter with no usage and a budget limit should be allowed.
#[tokio::test]
async fn test_budget_under_limit_allowed() {
    let (metering, memory) = setup().await;
    let fid = setup_fighter(&memory).await;
    let enforcer = BudgetEnforcer::new(metering);

    enforcer.set_fighter_limit(
        fid,
        BudgetLimit {
            max_cost_per_day_cents: Some(1000),
            ..Default::default()
        },
    );

    let verdict = enforcer.check_budget(&fid).await.unwrap();
    assert_eq!(verdict, BudgetVerdict::Allowed);
}

/// Fighter with no budget limits is always allowed regardless of usage.
#[tokio::test]
async fn test_budget_no_limit_always_allowed() {
    let (metering, memory) = setup().await;
    let fid = setup_fighter(&memory).await;

    // Record heavy usage.
    metering
        .record_usage(&fid, "claude-sonnet-4-20250514", 1_000_000, 1_000_000)
        .await
        .unwrap();

    let enforcer = BudgetEnforcer::new(metering);
    let verdict = enforcer.check_budget(&fid).await.unwrap();
    assert_eq!(verdict, BudgetVerdict::Allowed);
}

/// Fighter over budget is blocked.
#[tokio::test]
async fn test_budget_over_limit_blocked() {
    let (metering, memory) = setup().await;
    let fid = setup_fighter(&memory).await;

    // Record enough usage to exceed $1.00 limit.
    metering
        .record_usage(&fid, "claude-sonnet-4-20250514", 100_000, 100_000)
        .await
        .unwrap();

    let enforcer = BudgetEnforcer::new(metering);
    enforcer.set_fighter_limit(
        fid,
        BudgetLimit {
            max_cost_per_day_cents: Some(100), // $1.00
            ..Default::default()
        },
    );

    let verdict = enforcer.check_budget(&fid).await.unwrap();
    assert!(matches!(verdict, BudgetVerdict::Blocked { .. }));
}

/// Per-fighter budgets are independent.
#[tokio::test]
async fn test_budget_per_fighter_independent() {
    let (metering, memory) = setup().await;
    let fid1 = setup_fighter(&memory).await;
    let fid2 = setup_fighter(&memory).await;

    // Only fid1 has usage.
    metering
        .record_usage(&fid1, "claude-sonnet-4-20250514", 100_000, 100_000)
        .await
        .unwrap();

    let enforcer = BudgetEnforcer::new(Arc::clone(&metering));
    enforcer.set_fighter_limit(
        fid1,
        BudgetLimit {
            max_cost_per_day_cents: Some(100),
            ..Default::default()
        },
    );
    enforcer.set_fighter_limit(
        fid2,
        BudgetLimit {
            max_cost_per_day_cents: Some(100),
            ..Default::default()
        },
    );

    let v1 = enforcer.check_budget(&fid1).await.unwrap();
    let v2 = enforcer.check_budget(&fid2).await.unwrap();

    assert!(matches!(v1, BudgetVerdict::Blocked { .. }));
    assert_eq!(v2, BudgetVerdict::Allowed);
}

/// Global budget limit blocks all fighters.
#[tokio::test]
async fn test_budget_global_limit_blocks_all() {
    let (metering, memory) = setup().await;
    let fid = setup_fighter(&memory).await;

    metering
        .record_usage(&fid, "claude-sonnet-4-20250514", 100_000, 100_000)
        .await
        .unwrap();

    let enforcer = BudgetEnforcer::new(Arc::clone(&metering));
    enforcer
        .set_global_limit(BudgetLimit {
            max_cost_per_day_cents: Some(100),
            ..Default::default()
        })
        .await;

    // Even a different fighter should be blocked.
    let fid2 = setup_fighter(&memory).await;
    let verdict = enforcer.check_budget(&fid2).await.unwrap();
    assert!(
        matches!(verdict, BudgetVerdict::Blocked { .. }),
        "global limit should block: {:?}",
        verdict
    );
}

/// BudgetLimit::default has no limits set.
#[test]
fn test_budget_limit_default_has_no_limits() {
    let limit = BudgetLimit::default();
    assert!(!limit.has_any_limit());
    assert_eq!(limit.warning_threshold_percent, 80);
}

/// Set and remove a fighter limit.
#[tokio::test]
async fn test_budget_set_and_remove_fighter_limit() {
    let (metering, _memory) = setup().await;
    let fid = FighterId::new();
    let enforcer = BudgetEnforcer::new(metering);

    enforcer.set_fighter_limit(
        fid,
        BudgetLimit {
            max_cost_per_day_cents: Some(100),
            ..Default::default()
        },
    );

    assert!(enforcer.get_fighter_limit(&fid).is_some());

    enforcer.remove_fighter_limit(&fid);
    assert!(enforcer.get_fighter_limit(&fid).is_none());
}

/// Set and clear a global limit.
#[tokio::test]
async fn test_budget_set_and_clear_global_limit() {
    let (metering, _memory) = setup().await;
    let enforcer = BudgetEnforcer::new(metering);

    enforcer
        .set_global_limit(BudgetLimit {
            max_cost_per_day_cents: Some(500),
            ..Default::default()
        })
        .await;

    assert!(enforcer.get_global_limit().await.is_some());

    enforcer.clear_global_limit().await;
    assert!(enforcer.get_global_limit().await.is_none());
}

// ===========================================================================
// Tenant registry tests
// ===========================================================================

/// Register a tenant and verify it appears in the registry.
#[test]
fn test_tenant_register_and_get() {
    let registry = TenantRegistry::new();
    let tenant = registry.register_tenant("Acme Corp".to_string(), TenantQuota::default());

    assert_eq!(tenant.name, "Acme Corp");
    assert_eq!(tenant.status, TenantStatus::Active);
    assert!(tenant.api_key.starts_with("pk_"));

    let found = registry.get_tenant(&tenant.id).unwrap();
    assert_eq!(found.id, tenant.id);
}

/// Tenant lookup by API key works.
#[test]
fn test_tenant_lookup_by_api_key() {
    let registry = TenantRegistry::new();
    let tenant = registry.register_tenant("KeyTest".to_string(), TenantQuota::default());

    let found = registry.get_tenant_by_api_key(&tenant.api_key).unwrap();
    assert_eq!(found.id, tenant.id);
}

/// Invalid API key returns None.
#[test]
fn test_tenant_invalid_api_key_returns_none() {
    let registry = TenantRegistry::new();
    assert!(registry.get_tenant_by_api_key("invalid-key").is_none());
}

/// Suspend and reactivate a tenant.
#[test]
fn test_tenant_suspend_and_activate() {
    let registry = TenantRegistry::new();
    let tenant = registry.register_tenant("SuspendTest".to_string(), TenantQuota::default());

    registry.suspend_tenant(&tenant.id).unwrap();
    assert!(!registry.is_tenant_active(&tenant.id));
    assert_eq!(
        registry.get_tenant(&tenant.id).unwrap().status,
        TenantStatus::Suspended
    );

    registry.activate_tenant(&tenant.id).unwrap();
    assert!(registry.is_tenant_active(&tenant.id));
    assert_eq!(
        registry.get_tenant(&tenant.id).unwrap().status,
        TenantStatus::Active
    );
}

/// Delete a tenant removes it and its API key index.
#[test]
fn test_tenant_delete() {
    let registry = TenantRegistry::new();
    let tenant = registry.register_tenant("DeleteMe".to_string(), TenantQuota::default());
    let api_key = tenant.api_key.clone();

    let deleted = registry.delete_tenant(&tenant.id);
    assert!(deleted.is_some());
    assert!(registry.get_tenant(&tenant.id).is_none());
    assert!(registry.get_tenant_by_api_key(&api_key).is_none());
}

/// Delete non-existent tenant returns None.
#[test]
fn test_tenant_delete_nonexistent() {
    let registry = TenantRegistry::new();
    assert!(registry.delete_tenant(&punch_types::TenantId::new()).is_none());
}

/// List tenants returns all registered tenants.
#[test]
fn test_tenant_list() {
    let registry = TenantRegistry::new();
    registry.register_tenant("A".to_string(), TenantQuota::default());
    registry.register_tenant("B".to_string(), TenantQuota::default());
    registry.register_tenant("C".to_string(), TenantQuota::default());

    assert_eq!(registry.list_tenants().len(), 3);
    assert_eq!(registry.tenant_count(), 3);
}

/// Two tenants are fully isolated.
#[test]
fn test_tenant_isolation() {
    let registry = TenantRegistry::new();
    let t1 = registry.register_tenant("TenantA".to_string(), TenantQuota::default());
    let t2 = registry.register_tenant("TenantB".to_string(), TenantQuota::default());

    assert_ne!(t1.id, t2.id);
    assert_ne!(t1.api_key, t2.api_key);

    // Deleting one does not affect the other.
    registry.delete_tenant(&t1.id);
    assert!(registry.get_tenant(&t1.id).is_none());
    assert!(registry.get_tenant(&t2.id).is_some());
}

/// Update a tenant's quota.
#[test]
fn test_tenant_update_quota() {
    let registry = TenantRegistry::new();
    let tenant = registry.register_tenant("QuotaTest".to_string(), TenantQuota::default());

    let new_quota = TenantQuota {
        max_fighters: 50,
        ..TenantQuota::default()
    };
    registry.update_quota(&tenant.id, new_quota).unwrap();

    let updated = registry.get_tenant(&tenant.id).unwrap();
    assert_eq!(updated.quota.max_fighters, 50);
}

/// Suspend/activate non-existent tenant returns error.
#[test]
fn test_tenant_suspend_nonexistent_errors() {
    let registry = TenantRegistry::new();
    let fake_id = punch_types::TenantId::new();
    assert!(registry.suspend_tenant(&fake_id).is_err());
    assert!(registry.activate_tenant(&fake_id).is_err());
}

/// Register tenant with custom API key.
#[test]
fn test_tenant_register_with_custom_key() {
    let registry = TenantRegistry::new();
    let tenant = registry.register_tenant_with_key(
        "CustomKey".to_string(),
        "my-custom-key-123".to_string(),
        TenantQuota::default(),
    );

    assert_eq!(tenant.api_key, "my-custom-key-123");
    let found = registry.get_tenant_by_api_key("my-custom-key-123").unwrap();
    assert_eq!(found.id, tenant.id);
}

/// Unknown tenant is not active.
#[test]
fn test_tenant_unknown_not_active() {
    let registry = TenantRegistry::new();
    assert!(!registry.is_tenant_active(&punch_types::TenantId::new()));
}

/// Tenant count starts at zero.
#[test]
fn test_tenant_count_starts_zero() {
    let registry = TenantRegistry::new();
    assert_eq!(registry.tenant_count(), 0);
}
