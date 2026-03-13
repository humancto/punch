//! # punch-gorillas
//!
//! Autonomous agent (Gorilla) system for the Punch Agent Combat System.
//!
//! Gorillas are autonomous agents that run on schedules without direct user
//! interaction. They perform tasks like research, lead generation, monitoring,
//! forecasting, and content creation.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use punch_memory::MemorySubstrate;
use punch_runtime::LlmDriver;
use punch_types::{GorillaId, GorillaManifest, GorillaStatus, PunchError, PunchResult};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Output produced by a gorilla execution run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GorillaOutput {
    /// Human-readable summary of what the gorilla accomplished.
    pub summary: String,
    /// File paths, URLs, or other artifacts produced.
    pub artifacts: Vec<String>,
    /// Suggested next run time (if the gorilla wants to override its schedule).
    pub next_run: Option<DateTime<Utc>>,
}

/// Status of a single requirement check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementStatus {
    /// Name of the requirement.
    pub name: String,
    /// Whether the requirement is currently met.
    pub met: bool,
    /// Human-readable status message.
    pub message: String,
}

// ---------------------------------------------------------------------------
// GorillaRunner trait
// ---------------------------------------------------------------------------

/// Trait for executing a gorilla's autonomous task.
#[async_trait]
pub trait GorillaRunner: Send + Sync + 'static {
    /// Return this gorilla's manifest.
    fn manifest(&self) -> &GorillaManifest;

    /// Execute the gorilla's task.
    async fn execute(
        &self,
        memory: &MemorySubstrate,
        driver: Arc<dyn LlmDriver>,
    ) -> PunchResult<GorillaOutput>;

    /// Check whether all requirements for this gorilla are met.
    fn check_requirements(&self) -> Vec<RequirementStatus>;
}

// ---------------------------------------------------------------------------
// GorillaLifecycle
// ---------------------------------------------------------------------------

/// Internal state for a registered gorilla.
struct GorillaEntry {
    manifest: GorillaManifest,
    status: GorillaStatus,
    #[allow(dead_code)]
    registered_at: DateTime<Utc>,
}

/// Manages the lifecycle of gorillas: registration, starting, stopping.
pub struct GorillaLifecycle {
    gorillas: RwLock<HashMap<GorillaId, GorillaEntry>>,
}

impl GorillaLifecycle {
    /// Create a new lifecycle manager.
    pub fn new() -> Self {
        Self {
            gorillas: RwLock::new(HashMap::new()),
        }
    }

    /// Register a gorilla manifest and return its assigned ID.
    pub async fn register(&self, manifest: GorillaManifest) -> GorillaId {
        let id = GorillaId::new();
        info!(gorilla_id = %id, name = %manifest.name, "registering gorilla");
        let entry = GorillaEntry {
            manifest,
            status: GorillaStatus::Caged,
            registered_at: Utc::now(),
        };
        self.gorillas.write().await.insert(id, entry);
        id
    }

    /// Start (unleash) a gorilla by ID.
    pub async fn unleash(&self, id: GorillaId) -> PunchResult<()> {
        let mut gorillas = self.gorillas.write().await;
        let entry = gorillas
            .get_mut(&id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", id)))?;
        info!(gorilla_id = %id, name = %entry.manifest.name, "unleashing gorilla");
        entry.status = GorillaStatus::Unleashed;
        Ok(())
    }

    /// Stop (cage) a gorilla by ID.
    pub async fn cage(&self, id: GorillaId) -> PunchResult<()> {
        let mut gorillas = self.gorillas.write().await;
        let entry = gorillas
            .get_mut(&id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", id)))?;
        info!(gorilla_id = %id, name = %entry.manifest.name, "caging gorilla");
        entry.status = GorillaStatus::Caged;
        Ok(())
    }

    /// Get the current status of a gorilla.
    pub async fn get_status(&self, id: GorillaId) -> PunchResult<GorillaStatus> {
        let gorillas = self.gorillas.read().await;
        let entry = gorillas
            .get(&id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", id)))?;
        Ok(entry.status)
    }

    /// List all registered gorilla IDs and their names.
    pub async fn list(&self) -> Vec<(GorillaId, String, GorillaStatus)> {
        self.gorillas
            .read()
            .await
            .iter()
            .map(|(id, entry)| (*id, entry.manifest.name.clone(), entry.status))
            .collect()
    }
}

impl Default for GorillaLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manifest(name: &str) -> GorillaManifest {
        GorillaManifest {
            name: name.to_string(),
            description: format!("{name} description"),
            schedule: "*/5 * * * *".to_string(),
            moves_required: Vec::new(),
            settings_schema: None,
            dashboard_metrics: Vec::new(),
            system_prompt: None,
            model: None,
            capabilities: Vec::new(),
            weight_class: None,
        }
    }

    #[tokio::test]
    async fn test_register_gorilla() {
        let lifecycle = GorillaLifecycle::new();
        let id = lifecycle.register(test_manifest("alpha")).await;
        let gorillas = lifecycle.list().await;
        assert_eq!(gorillas.len(), 1);
        assert_eq!(gorillas[0].1, "alpha");
        assert_eq!(gorillas[0].0, id);
    }

    #[tokio::test]
    async fn test_gorilla_initial_status_is_caged() {
        let lifecycle = GorillaLifecycle::new();
        let id = lifecycle.register(test_manifest("beta")).await;
        let status = lifecycle.get_status(id).await.unwrap();
        assert_eq!(status, GorillaStatus::Caged);
    }

    #[tokio::test]
    async fn test_unleash_gorilla() {
        let lifecycle = GorillaLifecycle::new();
        let id = lifecycle.register(test_manifest("charlie")).await;
        lifecycle.unleash(id).await.unwrap();
        let status = lifecycle.get_status(id).await.unwrap();
        assert_eq!(status, GorillaStatus::Unleashed);
    }

    #[tokio::test]
    async fn test_cage_gorilla() {
        let lifecycle = GorillaLifecycle::new();
        let id = lifecycle.register(test_manifest("delta")).await;
        lifecycle.unleash(id).await.unwrap();
        lifecycle.cage(id).await.unwrap();
        let status = lifecycle.get_status(id).await.unwrap();
        assert_eq!(status, GorillaStatus::Caged);
    }

    #[tokio::test]
    async fn test_unleash_nonexistent_gorilla() {
        let lifecycle = GorillaLifecycle::new();
        let fake_id = GorillaId::new();
        let result = lifecycle.unleash(fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cage_nonexistent_gorilla() {
        let lifecycle = GorillaLifecycle::new();
        let fake_id = GorillaId::new();
        let result = lifecycle.cage(fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_status_nonexistent() {
        let lifecycle = GorillaLifecycle::new();
        let fake_id = GorillaId::new();
        let result = lifecycle.get_status(fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_register_multiple_gorillas() {
        let lifecycle = GorillaLifecycle::new();
        for i in 0..5 {
            lifecycle.register(test_manifest(&format!("gorilla-{i}"))).await;
        }
        let gorillas = lifecycle.list().await;
        assert_eq!(gorillas.len(), 5);
    }

    #[tokio::test]
    async fn test_lifecycle_default() {
        let lifecycle = GorillaLifecycle::default();
        let gorillas = lifecycle.list().await;
        assert!(gorillas.is_empty());
    }

    #[test]
    fn test_gorilla_output_serialization() {
        let output = GorillaOutput {
            summary: "Completed task".to_string(),
            artifacts: vec!["report.pdf".to_string()],
            next_run: None,
        };
        let json = serde_json::to_string(&output).unwrap();
        let deserialized: GorillaOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.summary, "Completed task");
        assert_eq!(deserialized.artifacts.len(), 1);
        assert!(deserialized.next_run.is_none());
    }

    #[test]
    fn test_gorilla_output_with_next_run() {
        let output = GorillaOutput {
            summary: "Done".to_string(),
            artifacts: Vec::new(),
            next_run: Some(Utc::now()),
        };
        let json = serde_json::to_string(&output).unwrap();
        let deserialized: GorillaOutput = serde_json::from_str(&json).unwrap();
        assert!(deserialized.next_run.is_some());
    }

    #[test]
    fn test_requirement_status_serialization() {
        let req = RequirementStatus {
            name: "api_key".to_string(),
            met: true,
            message: "API key is configured".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: RequirementStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "api_key");
        assert!(deserialized.met);
    }

    #[test]
    fn test_requirement_status_not_met() {
        let req = RequirementStatus {
            name: "database".to_string(),
            met: false,
            message: "Database not reachable".to_string(),
        };
        assert!(!req.met);
        assert!(req.message.contains("not reachable"));
    }

    #[test]
    fn test_gorilla_manifest_schedule_parsing() {
        let manifest = GorillaManifest {
            name: "scheduler-test".to_string(),
            description: "test".to_string(),
            schedule: "0 */6 * * *".to_string(),
            moves_required: Vec::new(),
            settings_schema: None,
            dashboard_metrics: Vec::new(),
            system_prompt: None,
            model: None,
            capabilities: Vec::new(),
            weight_class: None,
        };
        assert_eq!(manifest.schedule, "0 */6 * * *");
    }

    #[test]
    fn test_gorilla_manifest_with_moves() {
        let manifest = GorillaManifest {
            name: "worker".to_string(),
            description: "worker gorilla".to_string(),
            schedule: "*/10 * * * *".to_string(),
            moves_required: vec!["read_file".to_string(), "web_fetch".to_string()],
            settings_schema: None,
            dashboard_metrics: vec!["files_processed".to_string()],
            system_prompt: Some("You are a worker.".to_string()),
            model: None,
            capabilities: Vec::new(),
            weight_class: None,
        };
        assert_eq!(manifest.moves_required.len(), 2);
        assert_eq!(manifest.dashboard_metrics.len(), 1);
        assert_eq!(manifest.effective_system_prompt(), "You are a worker.");
    }

    #[test]
    fn test_gorilla_manifest_effective_prompt_fallback() {
        let manifest = test_manifest("fallback");
        // With no system_prompt, falls back to description
        assert_eq!(manifest.effective_system_prompt(), "fallback description");
    }
}
