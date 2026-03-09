use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::capability::Capability;
use crate::config::ModelConfig;
use crate::fighter::WeightClass;

/// Unique identifier for a Gorilla (autonomous agent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GorillaId(pub Uuid);

impl GorillaId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for GorillaId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for GorillaId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The manifest describing a Gorilla's configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GorillaManifest {
    /// Human-readable name for this Gorilla.
    pub name: String,
    /// Description of the Gorilla's autonomous purpose.
    pub description: String,
    /// Cron-style schedule expression (e.g. "*/5 * * * *" or "every 5m").
    pub schedule: String,
    /// Moves (tools) this Gorilla requires to operate.
    #[serde(default)]
    pub moves_required: Vec<String>,
    /// JSON Schema describing the Gorilla's configurable settings.
    #[serde(alias = "settings")]
    pub settings_schema: Option<serde_json::Value>,
    /// Metric keys exposed on the Gorilla's dashboard.
    #[serde(default)]
    pub dashboard_metrics: Vec<String>,
    /// System prompt for the gorilla's autonomous behavior.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Model configuration override. Falls back to the global default_model if None.
    #[serde(default)]
    pub model: Option<ModelConfig>,
    /// Explicit capability grants. If empty, derived from `moves_required`.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Weight class override. Defaults to Middleweight.
    #[serde(default)]
    pub weight_class: Option<WeightClass>,
}

impl GorillaManifest {
    /// Resolve the effective system prompt, falling back to the description.
    pub fn effective_system_prompt(&self) -> String {
        self.system_prompt
            .clone()
            .unwrap_or_else(|| self.description.clone())
    }

    /// Resolve the effective capabilities by combining explicit capabilities
    /// with those derived from `moves_required`.
    pub fn effective_capabilities(&self) -> Vec<Capability> {
        let mut caps = self.capabilities.clone();
        for move_name in &self.moves_required {
            let derived = capabilities_from_move(move_name);
            for cap in derived {
                if !caps.contains(&cap) {
                    caps.push(cap);
                }
            }
        }
        caps
    }
}

/// Map a move name (from GORILLA.toml `moves_required`) to the corresponding
/// capabilities. A single move may grant multiple capabilities.
pub fn capabilities_from_move(move_name: &str) -> Vec<Capability> {
    match move_name {
        "read_file" | "file_read" => vec![Capability::FileRead("**".to_string())],
        "write_file" | "file_write" => vec![Capability::FileWrite("**".to_string())],
        "file_list" => vec![Capability::FileRead("**".to_string())],
        "shell_exec" => vec![Capability::ShellExec("*".to_string())],
        "web_fetch" | "web_search" => vec![Capability::Network("*".to_string())],
        "memory_store" | "memory_recall" => vec![Capability::Memory],
        "knowledge_add_entity" | "knowledge_add_relation" | "knowledge_query" => {
            vec![Capability::KnowledgeGraph]
        }
        _ => Vec::new(),
    }
}

/// Current operational status of a Gorilla.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GorillaStatus {
    /// Registered but not running.
    Caged,
    /// Active and awaiting the next scheduled run.
    Unleashed,
    /// Currently executing a task.
    Rampaging,
    /// Temporarily paused by an operator.
    Resting,
    /// Disabled due to repeated failures.
    Injured,
}

impl std::fmt::Display for GorillaStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Caged => write!(f, "caged"),
            Self::Unleashed => write!(f, "unleashed"),
            Self::Rampaging => write!(f, "rampaging"),
            Self::Resting => write!(f, "resting"),
            Self::Injured => write!(f, "injured"),
        }
    }
}

/// Runtime metrics for a Gorilla.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GorillaMetrics {
    /// Total tasks completed successfully.
    pub tasks_completed: u64,
    /// Cumulative uptime in seconds.
    pub uptime_secs: u64,
    /// Timestamp of the last execution ("rampage").
    pub last_rampage: Option<DateTime<Utc>>,
}
