use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a tenant/organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(pub Uuid);

impl TenantId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TenantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A tenant represents an organization or user account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: TenantId,
    pub name: String,
    pub api_key: String,
    pub status: TenantStatus,
    pub quota: TenantQuota,
    pub created_at: DateTime<Utc>,
}

/// The operational status of a tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    Active,
    Suspended,
    Trial,
}

impl std::fmt::Display for TenantStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Suspended => write!(f, "suspended"),
            Self::Trial => write!(f, "trial"),
        }
    }
}

/// Resource quotas for a tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantQuota {
    /// Maximum number of fighters the tenant can spawn.
    pub max_fighters: usize,
    /// Maximum number of gorillas the tenant can register.
    pub max_gorillas: usize,
    /// Maximum number of concurrent bouts.
    pub max_bouts: usize,
    /// Maximum tokens the tenant can consume per day.
    pub max_tokens_per_day: u64,
    /// Allowed tool names. Empty means all tools are allowed.
    #[serde(default)]
    pub max_tools: Vec<String>,
}

impl Default for TenantQuota {
    fn default() -> Self {
        Self {
            max_fighters: 10,
            max_gorillas: 5,
            max_bouts: 50,
            max_tokens_per_day: 1_000_000,
            max_tools: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_id_display() {
        let uuid = Uuid::nil();
        let id = TenantId(uuid);
        assert_eq!(id.to_string(), uuid.to_string());
    }

    #[test]
    fn test_tenant_id_new_is_unique() {
        let id1 = TenantId::new();
        let id2 = TenantId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_tenant_id_default() {
        let id = TenantId::default();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_tenant_id_serde_transparent() {
        let uuid = Uuid::new_v4();
        let id = TenantId(uuid);
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{}\"", uuid));
        let deser: TenantId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser, id);
    }

    #[test]
    fn test_tenant_id_copy_clone() {
        let id = TenantId::new();
        let copied = id;
        let cloned = id.clone();
        assert_eq!(id, copied);
        assert_eq!(id, cloned);
    }

    #[test]
    fn test_tenant_id_hash() {
        let id = TenantId::new();
        let mut set = std::collections::HashSet::new();
        set.insert(id);
        set.insert(id);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_tenant_status_display() {
        assert_eq!(TenantStatus::Active.to_string(), "active");
        assert_eq!(TenantStatus::Suspended.to_string(), "suspended");
        assert_eq!(TenantStatus::Trial.to_string(), "trial");
    }

    #[test]
    fn test_tenant_status_serde_roundtrip() {
        let statuses = vec![
            TenantStatus::Active,
            TenantStatus::Suspended,
            TenantStatus::Trial,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).expect("serialize");
            let deser: TenantStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, status);
        }
    }

    #[test]
    fn test_tenant_quota_default() {
        let quota = TenantQuota::default();
        assert_eq!(quota.max_fighters, 10);
        assert_eq!(quota.max_gorillas, 5);
        assert_eq!(quota.max_bouts, 50);
        assert_eq!(quota.max_tokens_per_day, 1_000_000);
        assert!(quota.max_tools.is_empty());
    }

    #[test]
    fn test_tenant_quota_serde_roundtrip() {
        let quota = TenantQuota {
            max_fighters: 20,
            max_gorillas: 10,
            max_bouts: 100,
            max_tokens_per_day: 5_000_000,
            max_tools: vec!["read_file".to_string(), "web_fetch".to_string()],
        };
        let json = serde_json::to_string(&quota).expect("serialize");
        let deser: TenantQuota = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.max_fighters, 20);
        assert_eq!(deser.max_tools.len(), 2);
    }

    #[test]
    fn test_tenant_serde_roundtrip() {
        let tenant = Tenant {
            id: TenantId::new(),
            name: "Acme Corp".to_string(),
            api_key: "pk_test_abc123".to_string(),
            status: TenantStatus::Active,
            quota: TenantQuota::default(),
            created_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&tenant).expect("serialize");
        let deser: Tenant = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.id, tenant.id);
        assert_eq!(deser.name, "Acme Corp");
        assert_eq!(deser.api_key, "pk_test_abc123");
        assert_eq!(deser.status, TenantStatus::Active);
    }
}
