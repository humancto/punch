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
        let entry = gorillas.get_mut(&id).ok_or_else(|| PunchError::Gorilla(
            format!("gorilla {} not found", id),
        ))?;
        info!(gorilla_id = %id, name = %entry.manifest.name, "unleashing gorilla");
        entry.status = GorillaStatus::Unleashed;
        Ok(())
    }

    /// Stop (cage) a gorilla by ID.
    pub async fn cage(&self, id: GorillaId) -> PunchResult<()> {
        let mut gorillas = self.gorillas.write().await;
        let entry = gorillas.get_mut(&id).ok_or_else(|| PunchError::Gorilla(
            format!("gorilla {} not found", id),
        ))?;
        info!(gorilla_id = %id, name = %entry.manifest.name, "caging gorilla");
        entry.status = GorillaStatus::Caged;
        Ok(())
    }

    /// Get the current status of a gorilla.
    pub async fn get_status(&self, id: GorillaId) -> PunchResult<GorillaStatus> {
        let gorillas = self.gorillas.read().await;
        let entry = gorillas.get(&id).ok_or_else(|| PunchError::Gorilla(
            format!("gorilla {} not found", id),
        ))?;
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
