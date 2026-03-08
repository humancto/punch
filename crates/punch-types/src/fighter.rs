use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::capability::Capability;
use crate::config::ModelConfig;

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
