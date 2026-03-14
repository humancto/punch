use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::capability::Capability;
use crate::config::ModelConfig;
use crate::tenant::TenantId;

/// Unique identifier for a Fighter (conversational agent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FighterId(pub Uuid);

impl FighterId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for FighterId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FighterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Model tier determining the weight class of a Fighter.
///
/// Higher weight classes use more powerful (and expensive) models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightClass {
    /// Lightweight, fast models (e.g. Haiku, GPT-4o-mini)
    Featherweight,
    /// Balanced models (e.g. Sonnet, GPT-4o)
    Middleweight,
    /// High-capability models (e.g. Opus, o1)
    Heavyweight,
    /// Top-tier, unrestricted models
    Champion,
}

impl std::fmt::Display for WeightClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Featherweight => write!(f, "featherweight"),
            Self::Middleweight => write!(f, "middleweight"),
            Self::Heavyweight => write!(f, "heavyweight"),
            Self::Champion => write!(f, "champion"),
        }
    }
}

/// The manifest describing a Fighter's configuration and identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FighterManifest {
    /// Human-readable name for this Fighter.
    pub name: String,
    /// Description of the Fighter's purpose and specialty.
    pub description: String,
    /// Model configuration for this Fighter.
    pub model: ModelConfig,
    /// System prompt that shapes the Fighter's behavior.
    pub system_prompt: String,
    /// Capabilities granted to this Fighter.
    pub capabilities: Vec<Capability>,
    /// The model tier / weight class.
    pub weight_class: WeightClass,
    /// The tenant that owns this fighter. None for single-tenant / backward compat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<TenantId>,
}

/// Current operational status of a Fighter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FighterStatus {
    /// Ready and waiting for a bout.
    Idle,
    /// Actively engaged in a conversation / task.
    Fighting,
    /// Temporarily paused (e.g. rate-limited).
    Resting,
    /// Encountered a fatal error and is no longer operational.
    KnockedOut,
    /// Undergoing fine-tuning or calibration.
    Training,
}

impl std::fmt::Display for FighterStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Fighting => write!(f, "fighting"),
            Self::Resting => write!(f, "resting"),
            Self::KnockedOut => write!(f, "knocked_out"),
            Self::Training => write!(f, "training"),
        }
    }
}

/// Runtime statistics for a Fighter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FighterStats {
    /// Total messages sent by this Fighter.
    pub messages_sent: u64,
    /// Total tokens consumed.
    pub tokens_used: u64,
    /// Number of bouts won (tasks completed successfully).
    pub bouts_won: u64,
    /// Number of knockouts (unrecoverable errors).
    pub knockouts: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelConfig, Provider};

    #[test]
    fn test_fighter_id_display() {
        let uuid = Uuid::nil();
        let id = FighterId(uuid);
        assert_eq!(id.to_string(), uuid.to_string());
    }

    #[test]
    fn test_fighter_id_new_is_unique() {
        let id1 = FighterId::new();
        let id2 = FighterId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_fighter_id_default() {
        let id = FighterId::default();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_fighter_id_serde_transparent() {
        let uuid = Uuid::new_v4();
        let id = FighterId(uuid);
        let json = serde_json::to_string(&id).expect("serialize");
        // transparent means it serializes as just the UUID string
        assert_eq!(json, format!("\"{}\"", uuid));
        let deser: FighterId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser, id);
    }

    #[test]
    fn test_fighter_id_copy_clone() {
        let id = FighterId::new();
        let copied = id; // Copy
        let cloned = id.clone();
        assert_eq!(id, copied);
        assert_eq!(id, cloned);
    }

    #[test]
    fn test_fighter_id_hash() {
        let id = FighterId::new();
        let mut set = std::collections::HashSet::new();
        set.insert(id);
        set.insert(id); // duplicate
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_weight_class_display() {
        assert_eq!(WeightClass::Featherweight.to_string(), "featherweight");
        assert_eq!(WeightClass::Middleweight.to_string(), "middleweight");
        assert_eq!(WeightClass::Heavyweight.to_string(), "heavyweight");
        assert_eq!(WeightClass::Champion.to_string(), "champion");
    }

    #[test]
    fn test_weight_class_serde_roundtrip() {
        let classes = vec![
            WeightClass::Featherweight,
            WeightClass::Middleweight,
            WeightClass::Heavyweight,
            WeightClass::Champion,
        ];
        for wc in &classes {
            let json = serde_json::to_string(wc).expect("serialize");
            let deser: WeightClass = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, wc);
        }
    }

    #[test]
    fn test_weight_class_serde_values() {
        assert_eq!(
            serde_json::to_string(&WeightClass::Featherweight).unwrap(),
            "\"featherweight\""
        );
        assert_eq!(
            serde_json::to_string(&WeightClass::Champion).unwrap(),
            "\"champion\""
        );
    }

    #[test]
    fn test_fighter_status_display() {
        assert_eq!(FighterStatus::Idle.to_string(), "idle");
        assert_eq!(FighterStatus::Fighting.to_string(), "fighting");
        assert_eq!(FighterStatus::Resting.to_string(), "resting");
        assert_eq!(FighterStatus::KnockedOut.to_string(), "knocked_out");
        assert_eq!(FighterStatus::Training.to_string(), "training");
    }

    #[test]
    fn test_fighter_status_serde_roundtrip() {
        let statuses = vec![
            FighterStatus::Idle,
            FighterStatus::Fighting,
            FighterStatus::Resting,
            FighterStatus::KnockedOut,
            FighterStatus::Training,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).expect("serialize");
            let deser: FighterStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, status);
        }
    }

    #[test]
    fn test_fighter_status_equality() {
        assert_eq!(FighterStatus::Idle, FighterStatus::Idle);
        assert_ne!(FighterStatus::Idle, FighterStatus::Fighting);
    }

    #[test]
    fn test_fighter_stats_default() {
        let stats = FighterStats::default();
        assert_eq!(stats.messages_sent, 0);
        assert_eq!(stats.tokens_used, 0);
        assert_eq!(stats.bouts_won, 0);
        assert_eq!(stats.knockouts, 0);
    }

    #[test]
    fn test_fighter_stats_serde_roundtrip() {
        let stats = FighterStats {
            messages_sent: 100,
            tokens_used: 50000,
            bouts_won: 10,
            knockouts: 2,
        };
        let json = serde_json::to_string(&stats).expect("serialize");
        let deser: FighterStats = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.messages_sent, 100);
        assert_eq!(deser.tokens_used, 50000);
        assert_eq!(deser.bouts_won, 10);
        assert_eq!(deser.knockouts, 2);
    }

    #[test]
    fn test_fighter_manifest_serde() {
        let manifest = FighterManifest {
            name: "TestFighter".to_string(),
            description: "A test fighter".to_string(),
            model: ModelConfig {
                provider: Provider::Anthropic,
                model: "claude-sonnet-4-20250514".to_string(),
                api_key_env: None,
                base_url: None,
                max_tokens: None,
                temperature: None,
            },
            system_prompt: "You are helpful".to_string(),
            capabilities: vec![Capability::Memory],
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        };
        let json = serde_json::to_string(&manifest).expect("serialize");
        let deser: FighterManifest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.name, "TestFighter");
        assert_eq!(deser.weight_class, WeightClass::Middleweight);
        assert_eq!(deser.capabilities.len(), 1);
        assert!(deser.tenant_id.is_none());
    }
}
