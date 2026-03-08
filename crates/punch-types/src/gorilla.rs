use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    /// Cron-style schedule expression (e.g. "*/5 * * * *").
    pub schedule: String,
    /// Moves (tools) this Gorilla requires to operate.
    pub moves_required: Vec<String>,
    /// JSON Schema describing the Gorilla's configurable settings.
    pub settings_schema: Option<serde_json::Value>,
    /// Metric keys exposed on the Gorilla's dashboard.
    pub dashboard_metrics: Vec<String>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GorillaMetrics {
    /// Total tasks completed successfully.
    pub tasks_completed: u64,
    /// Cumulative uptime in seconds.
    pub uptime_secs: u64,
    /// Timestamp of the last execution ("rampage").
    pub last_rampage: Option<DateTime<Utc>>,
}

impl Default for GorillaMetrics {
    fn default() -> Self {
        Self {
            tasks_completed: 0,
            uptime_secs: 0,
            last_rampage: None,
        }
    }
}
