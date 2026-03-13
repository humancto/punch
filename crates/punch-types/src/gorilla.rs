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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelConfig, Provider};

    #[test]
    fn test_gorilla_id_display() {
        let uuid = Uuid::nil();
        let id = GorillaId(uuid);
        assert_eq!(id.to_string(), uuid.to_string());
    }

    #[test]
    fn test_gorilla_id_new_unique() {
        let id1 = GorillaId::new();
        let id2 = GorillaId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_gorilla_id_default() {
        let id = GorillaId::default();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_gorilla_id_serde_transparent() {
        let uuid = Uuid::new_v4();
        let id = GorillaId(uuid);
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{}\"", uuid));
        let deser: GorillaId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser, id);
    }

    #[test]
    fn test_gorilla_status_display() {
        assert_eq!(GorillaStatus::Caged.to_string(), "caged");
        assert_eq!(GorillaStatus::Unleashed.to_string(), "unleashed");
        assert_eq!(GorillaStatus::Rampaging.to_string(), "rampaging");
        assert_eq!(GorillaStatus::Resting.to_string(), "resting");
        assert_eq!(GorillaStatus::Injured.to_string(), "injured");
    }

    #[test]
    fn test_gorilla_status_serde_roundtrip() {
        let statuses = vec![
            GorillaStatus::Caged,
            GorillaStatus::Unleashed,
            GorillaStatus::Rampaging,
            GorillaStatus::Resting,
            GorillaStatus::Injured,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).expect("serialize");
            let deser: GorillaStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, status);
        }
    }

    #[test]
    fn test_gorilla_metrics_default() {
        let metrics = GorillaMetrics::default();
        assert_eq!(metrics.tasks_completed, 0);
        assert_eq!(metrics.uptime_secs, 0);
        assert!(metrics.last_rampage.is_none());
    }

    #[test]
    fn test_gorilla_metrics_serde() {
        let metrics = GorillaMetrics {
            tasks_completed: 50,
            uptime_secs: 3600,
            last_rampage: Some(Utc::now()),
        };
        let json = serde_json::to_string(&metrics).expect("serialize");
        let deser: GorillaMetrics = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.tasks_completed, 50);
        assert_eq!(deser.uptime_secs, 3600);
        assert!(deser.last_rampage.is_some());
    }

    fn make_test_manifest() -> GorillaManifest {
        GorillaManifest {
            name: "TestGorilla".to_string(),
            description: "A test gorilla".to_string(),
            schedule: "*/5 * * * *".to_string(),
            moves_required: vec!["read_file".to_string(), "shell_exec".to_string()],
            settings_schema: None,
            dashboard_metrics: vec!["uptime".to_string()],
            system_prompt: None,
            model: None,
            capabilities: vec![],
            weight_class: None,
        }
    }

    #[test]
    fn test_effective_system_prompt_fallback() {
        let manifest = make_test_manifest();
        assert_eq!(manifest.effective_system_prompt(), "A test gorilla");
    }

    #[test]
    fn test_effective_system_prompt_explicit() {
        let mut manifest = make_test_manifest();
        manifest.system_prompt = Some("Custom prompt".to_string());
        assert_eq!(manifest.effective_system_prompt(), "Custom prompt");
    }

    #[test]
    fn test_effective_capabilities_derived() {
        let manifest = make_test_manifest();
        let caps = manifest.effective_capabilities();
        assert!(caps.contains(&Capability::FileRead("**".to_string())));
        assert!(caps.contains(&Capability::ShellExec("*".to_string())));
    }

    #[test]
    fn test_effective_capabilities_no_duplicates() {
        let mut manifest = make_test_manifest();
        manifest.capabilities = vec![Capability::FileRead("**".to_string())];
        let caps = manifest.effective_capabilities();
        let file_read_count = caps
            .iter()
            .filter(|c| matches!(c, Capability::FileRead(_)))
            .count();
        assert_eq!(file_read_count, 1);
    }

    #[test]
    fn test_capabilities_from_move_read_file() {
        let caps = capabilities_from_move("read_file");
        assert_eq!(caps, vec![Capability::FileRead("**".to_string())]);
    }

    #[test]
    fn test_capabilities_from_move_file_read() {
        let caps = capabilities_from_move("file_read");
        assert_eq!(caps, vec![Capability::FileRead("**".to_string())]);
    }

    #[test]
    fn test_capabilities_from_move_write_file() {
        let caps = capabilities_from_move("write_file");
        assert_eq!(caps, vec![Capability::FileWrite("**".to_string())]);
    }

    #[test]
    fn test_capabilities_from_move_file_list() {
        let caps = capabilities_from_move("file_list");
        assert_eq!(caps, vec![Capability::FileRead("**".to_string())]);
    }

    #[test]
    fn test_capabilities_from_move_shell() {
        let caps = capabilities_from_move("shell_exec");
        assert_eq!(caps, vec![Capability::ShellExec("*".to_string())]);
    }

    #[test]
    fn test_capabilities_from_move_web() {
        let caps1 = capabilities_from_move("web_fetch");
        let caps2 = capabilities_from_move("web_search");
        assert_eq!(caps1, vec![Capability::Network("*".to_string())]);
        assert_eq!(caps2, vec![Capability::Network("*".to_string())]);
    }

    #[test]
    fn test_capabilities_from_move_memory() {
        let caps1 = capabilities_from_move("memory_store");
        let caps2 = capabilities_from_move("memory_recall");
        assert_eq!(caps1, vec![Capability::Memory]);
        assert_eq!(caps2, vec![Capability::Memory]);
    }

    #[test]
    fn test_capabilities_from_move_knowledge() {
        for name in &[
            "knowledge_add_entity",
            "knowledge_add_relation",
            "knowledge_query",
        ] {
            let caps = capabilities_from_move(name);
            assert_eq!(caps, vec![Capability::KnowledgeGraph]);
        }
    }

    #[test]
    fn test_capabilities_from_move_unknown() {
        let caps = capabilities_from_move("nonexistent_tool");
        assert!(caps.is_empty());
    }

    #[test]
    fn test_gorilla_manifest_serde() {
        let manifest = GorillaManifest {
            name: "Watcher".to_string(),
            description: "Monitors logs".to_string(),
            schedule: "every 5m".to_string(),
            moves_required: vec!["read_file".to_string()],
            settings_schema: Some(serde_json::json!({"type": "object"})),
            dashboard_metrics: vec![],
            system_prompt: Some("Watch carefully".to_string()),
            model: Some(ModelConfig {
                provider: Provider::Anthropic,
                model: "claude-sonnet-4-20250514".to_string(),
                api_key_env: None,
                base_url: None,
                max_tokens: None,
                temperature: None,
            }),
            capabilities: vec![Capability::Memory],
            weight_class: Some(WeightClass::Heavyweight),
        };
        let json = serde_json::to_string(&manifest).expect("serialize");
        let deser: GorillaManifest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.name, "Watcher");
        assert_eq!(deser.weight_class, Some(WeightClass::Heavyweight));
    }
}
