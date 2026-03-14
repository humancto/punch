//! Tenant registry for multi-tenant isolation.
//!
//! Manages tenant lifecycle: registration, lookup, quota management,
//! suspension, and deletion. Stores tenants in a concurrent [`DashMap`]
//! for thread-safe access without external locking.

use dashmap::DashMap;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use punch_types::{PunchError, PunchResult, Tenant, TenantId, TenantQuota, TenantStatus};

/// Thread-safe in-memory tenant registry.
pub struct TenantRegistry {
    /// All registered tenants, keyed by their unique ID.
    tenants: DashMap<TenantId, Tenant>,
    /// Reverse index: API key -> TenantId for fast auth lookups.
    api_key_index: DashMap<String, TenantId>,
}

impl TenantRegistry {
    /// Create a new, empty tenant registry.
    pub fn new() -> Self {
        Self {
            tenants: DashMap::new(),
            api_key_index: DashMap::new(),
        }
    }

    /// Register a new tenant with the given name and quota.
    ///
    /// Generates a unique API key for the tenant. Returns the newly
    /// created [`Tenant`].
    #[instrument(skip(self, quota), fields(tenant_name = %name))]
    pub fn register_tenant(&self, name: String, quota: TenantQuota) -> Tenant {
        let id = TenantId::new();
        let api_key = format!("pk_{}", Uuid::new_v4().to_string().replace('-', ""));

        let tenant = Tenant {
            id,
            name: name.clone(),
            api_key: api_key.clone(),
            status: TenantStatus::Active,
            quota,
            created_at: chrono::Utc::now(),
        };

        self.api_key_index.insert(api_key, id);
        self.tenants.insert(id, tenant.clone());

        info!(%id, name, "tenant registered");
        tenant
    }

    /// Register a tenant with a specific API key (useful for testing/migration).
    #[instrument(skip(self, quota), fields(tenant_name = %name))]
    pub fn register_tenant_with_key(
        &self,
        name: String,
        api_key: String,
        quota: TenantQuota,
    ) -> Tenant {
        let id = TenantId::new();

        let tenant = Tenant {
            id,
            name: name.clone(),
            api_key: api_key.clone(),
            status: TenantStatus::Active,
            quota,
            created_at: chrono::Utc::now(),
        };

        self.api_key_index.insert(api_key, id);
        self.tenants.insert(id, tenant.clone());

        info!(%id, name, "tenant registered with custom key");
        tenant
    }

    /// Look up a tenant by ID.
    pub fn get_tenant(&self, id: &TenantId) -> Option<Tenant> {
        self.tenants.get(id).map(|t| t.value().clone())
    }

    /// Look up a tenant by their API key.
    pub fn get_tenant_by_api_key(&self, api_key: &str) -> Option<Tenant> {
        let tenant_id = self.api_key_index.get(api_key)?;
        self.tenants
            .get(tenant_id.value())
            .map(|t| t.value().clone())
    }

    /// Update a tenant's quota.
    #[instrument(skip(self, quota))]
    pub fn update_quota(&self, id: &TenantId, quota: TenantQuota) -> PunchResult<()> {
        let mut entry = self
            .tenants
            .get_mut(id)
            .ok_or_else(|| PunchError::Tenant(format!("tenant {} not found", id)))?;

        entry.quota = quota;
        info!(%id, "tenant quota updated");
        Ok(())
    }

    /// Suspend a tenant, blocking all their requests.
    #[instrument(skip(self))]
    pub fn suspend_tenant(&self, id: &TenantId) -> PunchResult<()> {
        let mut entry = self
            .tenants
            .get_mut(id)
            .ok_or_else(|| PunchError::Tenant(format!("tenant {} not found", id)))?;

        entry.status = TenantStatus::Suspended;
        info!(%id, "tenant suspended");
        Ok(())
    }

    /// Activate a tenant (restore from suspended or trial status).
    #[instrument(skip(self))]
    pub fn activate_tenant(&self, id: &TenantId) -> PunchResult<()> {
        let mut entry = self
            .tenants
            .get_mut(id)
            .ok_or_else(|| PunchError::Tenant(format!("tenant {} not found", id)))?;

        entry.status = TenantStatus::Active;
        info!(%id, "tenant activated");
        Ok(())
    }

    /// List all registered tenants.
    pub fn list_tenants(&self) -> Vec<Tenant> {
        self.tenants.iter().map(|e| e.value().clone()).collect()
    }

    /// Delete a tenant and remove their API key index.
    ///
    /// Returns the deleted tenant if it existed.
    #[instrument(skip(self))]
    pub fn delete_tenant(&self, id: &TenantId) -> Option<Tenant> {
        if let Some((_, tenant)) = self.tenants.remove(id) {
            self.api_key_index.remove(&tenant.api_key);
            info!(%id, name = %tenant.name, "tenant deleted");
            Some(tenant)
        } else {
            warn!(%id, "attempted to delete unknown tenant");
            None
        }
    }

    /// Check whether a tenant is active (not suspended).
    pub fn is_tenant_active(&self, id: &TenantId) -> bool {
        self.tenants
            .get(id)
            .map(|t| t.status != TenantStatus::Suspended)
            .unwrap_or(false)
    }

    /// Count the number of registered tenants.
    pub fn tenant_count(&self) -> usize {
        self.tenants.len()
    }
}

impl Default for TenantRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_tenant() {
        let registry = TenantRegistry::new();
        let tenant = registry.register_tenant("Acme Corp".to_string(), TenantQuota::default());

        assert_eq!(tenant.name, "Acme Corp");
        assert_eq!(tenant.status, TenantStatus::Active);
        assert!(tenant.api_key.starts_with("pk_"));
    }

    #[test]
    fn test_get_tenant() {
        let registry = TenantRegistry::new();
        let tenant = registry.register_tenant("Acme Corp".to_string(), TenantQuota::default());

        let found = registry.get_tenant(&tenant.id).unwrap();
        assert_eq!(found.id, tenant.id);
        assert_eq!(found.name, "Acme Corp");
    }

    #[test]
    fn test_get_tenant_not_found() {
        let registry = TenantRegistry::new();
        assert!(registry.get_tenant(&TenantId::new()).is_none());
    }

    #[test]
    fn test_get_tenant_by_api_key() {
        let registry = TenantRegistry::new();
        let tenant = registry.register_tenant("Acme Corp".to_string(), TenantQuota::default());

        let found = registry.get_tenant_by_api_key(&tenant.api_key).unwrap();
        assert_eq!(found.id, tenant.id);
    }

    #[test]
    fn test_get_tenant_by_api_key_invalid() {
        let registry = TenantRegistry::new();
        assert!(registry.get_tenant_by_api_key("invalid-key").is_none());
    }

    #[test]
    fn test_update_quota() {
        let registry = TenantRegistry::new();
        let tenant = registry.register_tenant("Acme Corp".to_string(), TenantQuota::default());

        let new_quota = TenantQuota {
            max_fighters: 50,
            ..TenantQuota::default()
        };
        registry.update_quota(&tenant.id, new_quota).unwrap();

        let updated = registry.get_tenant(&tenant.id).unwrap();
        assert_eq!(updated.quota.max_fighters, 50);
    }

    #[test]
    fn test_update_quota_not_found() {
        let registry = TenantRegistry::new();
        let result = registry.update_quota(&TenantId::new(), TenantQuota::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_suspend_tenant() {
        let registry = TenantRegistry::new();
        let tenant = registry.register_tenant("Acme Corp".to_string(), TenantQuota::default());

        registry.suspend_tenant(&tenant.id).unwrap();

        let suspended = registry.get_tenant(&tenant.id).unwrap();
        assert_eq!(suspended.status, TenantStatus::Suspended);
        assert!(!registry.is_tenant_active(&tenant.id));
    }

    #[test]
    fn test_activate_tenant() {
        let registry = TenantRegistry::new();
        let tenant = registry.register_tenant("Acme Corp".to_string(), TenantQuota::default());
        registry.suspend_tenant(&tenant.id).unwrap();
        registry.activate_tenant(&tenant.id).unwrap();

        let active = registry.get_tenant(&tenant.id).unwrap();
        assert_eq!(active.status, TenantStatus::Active);
        assert!(registry.is_tenant_active(&tenant.id));
    }

    #[test]
    fn test_list_tenants() {
        let registry = TenantRegistry::new();
        registry.register_tenant("Tenant A".to_string(), TenantQuota::default());
        registry.register_tenant("Tenant B".to_string(), TenantQuota::default());

        let tenants = registry.list_tenants();
        assert_eq!(tenants.len(), 2);
    }

    #[test]
    fn test_delete_tenant() {
        let registry = TenantRegistry::new();
        let tenant = registry.register_tenant("Acme Corp".to_string(), TenantQuota::default());
        let api_key = tenant.api_key.clone();

        let deleted = registry.delete_tenant(&tenant.id);
        assert!(deleted.is_some());
        assert!(registry.get_tenant(&tenant.id).is_none());
        assert!(registry.get_tenant_by_api_key(&api_key).is_none());
    }

    #[test]
    fn test_delete_tenant_not_found() {
        let registry = TenantRegistry::new();
        assert!(registry.delete_tenant(&TenantId::new()).is_none());
    }

    #[test]
    fn test_tenant_count() {
        let registry = TenantRegistry::new();
        assert_eq!(registry.tenant_count(), 0);
        registry.register_tenant("A".to_string(), TenantQuota::default());
        assert_eq!(registry.tenant_count(), 1);
        registry.register_tenant("B".to_string(), TenantQuota::default());
        assert_eq!(registry.tenant_count(), 2);
    }

    #[test]
    fn test_register_tenant_with_key() {
        let registry = TenantRegistry::new();
        let tenant = registry.register_tenant_with_key(
            "Custom Tenant".to_string(),
            "custom-api-key-123".to_string(),
            TenantQuota::default(),
        );

        assert_eq!(tenant.api_key, "custom-api-key-123");
        let found = registry
            .get_tenant_by_api_key("custom-api-key-123")
            .unwrap();
        assert_eq!(found.id, tenant.id);
    }

    #[test]
    fn test_is_tenant_active_unknown() {
        let registry = TenantRegistry::new();
        assert!(!registry.is_tenant_active(&TenantId::new()));
    }

    #[test]
    fn test_two_tenants_isolated() {
        let registry = TenantRegistry::new();
        let t1 = registry.register_tenant("Tenant A".to_string(), TenantQuota::default());
        let t2 = registry.register_tenant("Tenant B".to_string(), TenantQuota::default());

        assert_ne!(t1.id, t2.id);
        assert_ne!(t1.api_key, t2.api_key);

        // Each is retrievable independently.
        assert!(registry.get_tenant(&t1.id).is_some());
        assert!(registry.get_tenant(&t2.id).is_some());

        // Deleting one doesn't affect the other.
        registry.delete_tenant(&t1.id);
        assert!(registry.get_tenant(&t1.id).is_none());
        assert!(registry.get_tenant(&t2.id).is_some());
    }
}
