//! **The Ring** — the central kernel and coordinator for the Punch system.
//!
//! The [`Ring`] owns every fighter and gorilla, wires them to the memory
//! substrate, the LLM driver, the event bus, the scheduler, the background
//! executor, the workflow engine, and the metering engine. All mutations
//! go through the Ring so that invariants (quotas, capabilities, lifecycle
//! events) are enforced in a single place.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, instrument, warn};

use punch_memory::{BoutId, MemorySubstrate};
use punch_runtime::{
    FighterLoopParams, FighterLoopResult, LlmDriver, run_fighter_loop, tools_for_capabilities,
};
use punch_types::{
    AgentCoordinator, AgentInfo, AgentMessageResult, FighterId, FighterManifest, FighterStatus,
    GorillaId, GorillaManifest, GorillaMetrics, GorillaStatus, PunchConfig, PunchError, PunchEvent,
    PunchResult,
};

use crate::background::BackgroundExecutor;
use crate::event_bus::EventBus;
use crate::metering::MeteringEngine;
use crate::scheduler::{QuotaConfig, Scheduler};
use crate::triggers::{Trigger, TriggerEngine, TriggerId, TriggerSummary};
use crate::workflow::{Workflow, WorkflowEngine, WorkflowId, WorkflowRunId};

// ---------------------------------------------------------------------------
// Entry types
// ---------------------------------------------------------------------------

/// Everything the Ring tracks about a single fighter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FighterEntry {
    /// The fighter's manifest (identity, model, capabilities, etc.).
    pub manifest: FighterManifest,
    /// Current operational status.
    pub status: FighterStatus,
    /// The active bout (conversation session) ID, if any.
    pub current_bout: Option<BoutId>,
}

/// Everything the Ring tracks about a single gorilla.
///
/// The `task_handle` is behind a `Mutex` because [`JoinHandle`] is not `Clone`
/// and we need interior mutability when starting / stopping background tasks.
pub struct GorillaEntry {
    /// The gorilla's manifest.
    pub manifest: GorillaManifest,
    /// Current operational status.
    pub status: GorillaStatus,
    /// Runtime metrics.
    pub metrics: GorillaMetrics,
    /// Handle to the background task (if running).
    task_handle: Option<JoinHandle<()>>,
}

// Manual Debug impl because JoinHandle doesn't implement Debug in a useful way.
impl std::fmt::Debug for GorillaEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GorillaEntry")
            .field("manifest", &self.manifest)
            .field("status", &self.status)
            .field("metrics", &self.metrics)
            .field("has_task", &self.task_handle.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// The Ring
// ---------------------------------------------------------------------------

/// The Ring — the central coordinator for the Punch Agent Combat System.
///
/// Thread-safe by design: all collections use [`DashMap`] and all shared state
/// is behind `Arc`. Wrap the `Ring` itself in an `Arc` to share across tasks.
pub struct Ring {
    /// All active fighters, keyed by their unique ID.
    fighters: DashMap<FighterId, FighterEntry>,
    /// All registered gorillas, keyed by their unique ID.
    gorillas: DashMap<GorillaId, Mutex<GorillaEntry>>,
    /// Shared memory substrate (SQLite persistence).
    memory: Arc<MemorySubstrate>,
    /// The LLM driver used for completions.
    driver: Arc<dyn LlmDriver>,
    /// System-wide event bus.
    event_bus: EventBus,
    /// Per-fighter quota scheduler.
    scheduler: Scheduler,
    /// Top-level Punch configuration.
    config: PunchConfig,
    /// Background executor for autonomous gorilla tasks.
    background: BackgroundExecutor,
    /// Multi-step workflow engine.
    workflow_engine: WorkflowEngine,
    /// Cost tracking and metering engine.
    metering: MeteringEngine,
    /// Event-driven trigger engine.
    trigger_engine: TriggerEngine,
    /// Shutdown signal sender.
    shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver.
    _shutdown_rx: watch::Receiver<bool>,
}

impl Ring {
    /// Create a new Ring.
    ///
    /// The caller provides the already-initialised memory substrate, LLM
    /// driver, and configuration. The Ring will create its own event bus and
    /// scheduler internally.
    pub fn new(
        config: PunchConfig,
        memory: Arc<MemorySubstrate>,
        driver: Arc<dyn LlmDriver>,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let background =
            BackgroundExecutor::with_shutdown(shutdown_tx.clone(), shutdown_rx.clone());
        let metering = MeteringEngine::new(Arc::clone(&memory));

        Self {
            fighters: DashMap::new(),
            gorillas: DashMap::new(),
            memory,
            driver,
            event_bus: EventBus::new(),
            scheduler: Scheduler::new(QuotaConfig::default()),
            config,
            background,
            workflow_engine: WorkflowEngine::new(),
            metering,
            trigger_engine: TriggerEngine::new(),
            shutdown_tx,
            _shutdown_rx: shutdown_rx,
        }
    }

    /// Create a new Ring with a custom quota configuration.
    pub fn with_quota_config(
        config: PunchConfig,
        memory: Arc<MemorySubstrate>,
        driver: Arc<dyn LlmDriver>,
        quota_config: QuotaConfig,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let background =
            BackgroundExecutor::with_shutdown(shutdown_tx.clone(), shutdown_rx.clone());
        let metering = MeteringEngine::new(Arc::clone(&memory));

        Self {
            fighters: DashMap::new(),
            gorillas: DashMap::new(),
            memory,
            driver,
            event_bus: EventBus::new(),
            scheduler: Scheduler::new(quota_config),
            config,
            background,
            workflow_engine: WorkflowEngine::new(),
            metering,
            trigger_engine: TriggerEngine::new(),
            shutdown_tx,
            _shutdown_rx: shutdown_rx,
        }
    }

    // -- Accessors -----------------------------------------------------------

    /// Get a reference to the event bus.
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// Get a reference to the scheduler.
    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    /// Get a reference to the memory substrate.
    pub fn memory(&self) -> &Arc<MemorySubstrate> {
        &self.memory
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &PunchConfig {
        &self.config
    }

    /// Get a reference to the background executor.
    pub fn background(&self) -> &BackgroundExecutor {
        &self.background
    }

    /// Get a reference to the workflow engine.
    pub fn workflow_engine(&self) -> &WorkflowEngine {
        &self.workflow_engine
    }

    /// Get a reference to the metering engine.
    pub fn metering(&self) -> &MeteringEngine {
        &self.metering
    }

    /// Get a reference to the trigger engine.
    pub fn trigger_engine(&self) -> &TriggerEngine {
        &self.trigger_engine
    }

    // -- Trigger operations --------------------------------------------------

    /// Register a trigger with the engine.
    pub fn register_trigger(&self, trigger: Trigger) -> TriggerId {
        self.trigger_engine.register_trigger(trigger)
    }

    /// Remove a trigger by ID.
    pub fn remove_trigger(&self, id: &TriggerId) {
        self.trigger_engine.remove_trigger(id);
    }

    /// List all triggers with summary information.
    pub fn list_triggers(&self) -> Vec<(TriggerId, TriggerSummary)> {
        self.trigger_engine.list_triggers()
    }

    // -- Fighter operations --------------------------------------------------

    /// Spawn a new fighter from a manifest.
    ///
    /// Returns the newly-assigned [`FighterId`]. The fighter starts in
    /// [`FighterStatus::Idle`] and is persisted to the memory substrate.
    #[instrument(skip(self, manifest), fields(fighter_name = %manifest.name))]
    pub async fn spawn_fighter(&self, manifest: FighterManifest) -> FighterId {
        let id = FighterId::new();
        let name = manifest.name.clone();

        // Persist to the database first so FK constraints work.
        if let Err(e) = self
            .memory
            .save_fighter(&id, &manifest, FighterStatus::Idle)
            .await
        {
            warn!(error = %e, "failed to persist fighter to database (continuing in-memory only)");
        }

        let entry = FighterEntry {
            manifest,
            status: FighterStatus::Idle,
            current_bout: None,
        };

        self.fighters.insert(id, entry);

        self.event_bus.publish(PunchEvent::FighterSpawned {
            fighter_id: id,
            name: name.clone(),
        });

        info!(%id, name, "fighter spawned");
        id
    }

    /// Send a user message to a fighter and run the agent loop (without coordinator).
    ///
    /// Convenience wrapper around [`send_message_with_coordinator`] that passes
    /// `None`, meaning the fighter will not have access to inter-agent tools.
    #[instrument(skip(self, message), fields(%fighter_id))]
    pub async fn send_message(
        &self,
        fighter_id: &FighterId,
        message: String,
    ) -> PunchResult<FighterLoopResult> {
        self.send_message_with_coordinator(fighter_id, message, None)
            .await
    }

    /// Send a user message to a fighter and run the agent loop.
    ///
    /// This creates (or reuses) a bout for the fighter, checks quotas, then
    /// delegates to [`run_fighter_loop`]. Usage is recorded through the
    /// metering engine after a successful completion.
    ///
    /// If `coordinator` is provided, the fighter can use inter-agent tools
    /// (`agent_spawn`, `agent_message`, `agent_list`).
    #[instrument(skip(self, message, coordinator), fields(%fighter_id))]
    pub async fn send_message_with_coordinator(
        &self,
        fighter_id: &FighterId,
        message: String,
        coordinator: Option<Arc<dyn AgentCoordinator>>,
    ) -> PunchResult<FighterLoopResult> {
        // Look up the fighter.
        let mut entry = self
            .fighters
            .get_mut(fighter_id)
            .ok_or_else(|| PunchError::Fighter(format!("fighter {} not found", fighter_id)))?;

        // Check quota.
        if !self.scheduler.check_quota(fighter_id) {
            entry.status = FighterStatus::Resting;
            return Err(PunchError::RateLimited {
                provider: "scheduler".to_string(),
                retry_after_ms: 60_000,
            });
        }

        // Ensure the fighter has an active bout.
        let bout_id = match entry.current_bout {
            Some(id) => id,
            None => {
                // Create the bout in the database.
                let id = self
                    .memory
                    .create_bout(fighter_id)
                    .await
                    .map_err(|e| PunchError::Bout(format!("failed to create bout: {e}")))?;
                entry.current_bout = Some(id);

                self.event_bus.publish(PunchEvent::BoutStarted {
                    bout_id: id.0,
                    fighter_id: *fighter_id,
                });

                id
            }
        };

        // Mark as fighting.
        entry.status = FighterStatus::Fighting;
        let manifest = entry.manifest.clone();
        let available_tools = tools_for_capabilities(&manifest.capabilities);
        drop(entry); // Release the DashMap guard before the async call.

        // Run the fighter loop.
        let params = FighterLoopParams {
            manifest: manifest.clone(),
            user_message: message,
            bout_id,
            fighter_id: *fighter_id,
            memory: Arc::clone(&self.memory),
            driver: Arc::clone(&self.driver),
            available_tools,
            max_iterations: None,
            context_window: None,
            tool_timeout_secs: None,
            coordinator,
            approval_engine: None,
            sandbox: None,
        };

        let result = run_fighter_loop(params).await;

        // Update state based on the outcome.
        if let Some(mut entry) = self.fighters.get_mut(fighter_id) {
            match &result {
                Ok(loop_result) => {
                    entry.status = FighterStatus::Idle;
                    self.scheduler
                        .record_usage(fighter_id, loop_result.usage.total());

                    // Record usage through the metering engine.
                    if let Err(e) = self
                        .metering
                        .record_usage(
                            fighter_id,
                            &manifest.model.model,
                            loop_result.usage.input_tokens,
                            loop_result.usage.output_tokens,
                        )
                        .await
                    {
                        warn!(error = %e, "failed to record metering usage");
                    }
                }
                Err(_) => {
                    entry.status = FighterStatus::KnockedOut;
                }
            }
        }

        result
    }

    /// Kill (remove) a fighter.
    #[instrument(skip(self), fields(%fighter_id))]
    pub fn kill_fighter(&self, fighter_id: &FighterId) {
        if let Some((_, entry)) = self.fighters.remove(fighter_id) {
            self.scheduler.remove_fighter(fighter_id);
            info!(name = %entry.manifest.name, "fighter killed");
        } else {
            warn!("attempted to kill unknown fighter");
        }
    }

    /// List all fighters with their current status.
    pub fn list_fighters(&self) -> Vec<(FighterId, FighterManifest, FighterStatus)> {
        self.fighters
            .iter()
            .map(|entry| {
                let id = *entry.key();
                let e = entry.value();
                (id, e.manifest.clone(), e.status)
            })
            .collect()
    }

    /// Get a snapshot of a single fighter's entry.
    pub fn get_fighter(&self, fighter_id: &FighterId) -> Option<FighterEntry> {
        self.fighters.get(fighter_id).map(|e| e.value().clone())
    }

    // -- Gorilla operations --------------------------------------------------

    /// Register a gorilla with the Ring.
    ///
    /// Returns the newly-assigned [`GorillaId`]. The gorilla starts in
    /// [`GorillaStatus::Caged`].
    #[instrument(skip(self, manifest), fields(gorilla_name = %manifest.name))]
    pub fn register_gorilla(&self, manifest: GorillaManifest) -> GorillaId {
        let id = GorillaId::new();
        let name = manifest.name.clone();

        let entry = GorillaEntry {
            manifest,
            status: GorillaStatus::Caged,
            metrics: GorillaMetrics::default(),
            task_handle: None,
        };

        self.gorillas.insert(id, Mutex::new(entry));
        info!(%id, name, "gorilla registered");
        id
    }

    /// Unleash (start) a gorilla's background task.
    ///
    /// This uses the [`BackgroundExecutor`] to spawn the gorilla's autonomous
    /// loop, which will run the fighter loop on the gorilla's schedule.
    #[instrument(skip(self), fields(%gorilla_id))]
    pub async fn unleash_gorilla(&self, gorilla_id: &GorillaId) -> PunchResult<()> {
        let entry_ref = self
            .gorillas
            .get(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;

        let mut entry = entry_ref.value().lock().await;

        if entry.status == GorillaStatus::Unleashed || entry.status == GorillaStatus::Rampaging {
            return Err(PunchError::Gorilla(format!(
                "gorilla {} is already active",
                gorilla_id
            )));
        }

        let gorilla_id_owned = *gorilla_id;
        let name = entry.manifest.name.clone();
        let manifest = entry.manifest.clone();

        // Start the gorilla via the background executor.
        self.background.start_gorilla(
            gorilla_id_owned,
            manifest,
            self.config.default_model.clone(),
            Arc::clone(&self.memory),
            Arc::clone(&self.driver),
        )?;

        entry.status = GorillaStatus::Unleashed;
        drop(entry);
        drop(entry_ref);

        self.event_bus.publish(PunchEvent::GorillaUnleashed {
            gorilla_id: gorilla_id_owned,
            name,
        });

        Ok(())
    }

    /// Cage (stop) a gorilla's background task.
    #[instrument(skip(self), fields(%gorilla_id))]
    pub async fn cage_gorilla(&self, gorilla_id: &GorillaId) -> PunchResult<()> {
        let entry_ref = self
            .gorillas
            .get(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;

        let mut entry = entry_ref.value().lock().await;

        // Stop via background executor.
        self.background.stop_gorilla(gorilla_id);

        // Also abort any legacy task handle.
        if let Some(handle) = entry.task_handle.take() {
            handle.abort();
        }

        let name = entry.manifest.name.clone();
        entry.status = GorillaStatus::Caged;
        drop(entry);
        drop(entry_ref);

        self.event_bus.publish(PunchEvent::GorillaPaused {
            gorilla_id: *gorilla_id,
            reason: "manually caged".to_string(),
        });

        info!(name, "gorilla caged");
        Ok(())
    }

    /// List all gorillas with their current status.
    pub async fn list_gorillas(&self) -> Vec<(GorillaId, GorillaManifest, GorillaStatus)> {
        let mut result = Vec::new();

        for entry in self.gorillas.iter() {
            let id = *entry.key();
            let inner = entry.value().lock().await;
            result.push((id, inner.manifest.clone(), inner.status));
        }

        result
    }

    /// Get a gorilla's manifest by ID.
    pub async fn get_gorilla_manifest(&self, gorilla_id: &GorillaId) -> Option<GorillaManifest> {
        let entry_ref = self.gorillas.get(gorilla_id)?;
        let entry = entry_ref.value().lock().await;
        Some(entry.manifest.clone())
    }

    /// Find a gorilla ID by name (case-insensitive).
    pub async fn find_gorilla_by_name(&self, name: &str) -> Option<GorillaId> {
        for entry in self.gorillas.iter() {
            let inner = entry.value().lock().await;
            if inner.manifest.name.eq_ignore_ascii_case(name) {
                return Some(*entry.key());
            }
        }
        None
    }

    /// Run a single autonomous tick for a gorilla (for testing/debugging).
    ///
    /// This executes the gorilla's autonomous prompt once, without starting
    /// the background scheduler. Useful for verifying configuration.
    #[instrument(skip(self), fields(%gorilla_id))]
    pub async fn run_gorilla_tick(
        &self,
        gorilla_id: &GorillaId,
    ) -> PunchResult<punch_runtime::FighterLoopResult> {
        let entry_ref = self
            .gorillas
            .get(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;

        let entry = entry_ref.value().lock().await;
        let manifest = entry.manifest.clone();
        drop(entry);
        drop(entry_ref);

        crate::background::run_gorilla_tick(
            *gorilla_id,
            &manifest,
            &self.config.default_model,
            &self.memory,
            &self.driver,
        )
        .await
    }

    /// Get the LLM driver (useful for CLI commands that need to run ticks directly).
    pub fn driver(&self) -> &Arc<dyn LlmDriver> {
        &self.driver
    }

    // -- Workflow operations -------------------------------------------------

    /// Register a workflow with the engine.
    pub fn register_workflow(&self, workflow: Workflow) -> WorkflowId {
        self.workflow_engine.register_workflow(workflow)
    }

    /// Execute a workflow by ID with the given input.
    pub async fn execute_workflow(
        &self,
        workflow_id: &WorkflowId,
        input: String,
    ) -> PunchResult<WorkflowRunId> {
        self.workflow_engine
            .execute_workflow(
                workflow_id,
                input,
                Arc::clone(&self.memory),
                Arc::clone(&self.driver),
                &self.config.default_model,
            )
            .await
    }

    // -- Shutdown ------------------------------------------------------------

    /// Gracefully shut down the Ring, stopping all gorillas and background tasks.
    pub fn shutdown(&self) {
        info!("Ring shutdown initiated");

        // Signal shutdown to all background tasks.
        let _ = self.shutdown_tx.send(true);

        // Stop all gorilla tasks via the background executor.
        self.background.shutdown_all();

        info!("Ring shutdown complete");
    }
}

// ---------------------------------------------------------------------------
// AgentCoordinator implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl AgentCoordinator for Ring {
    async fn spawn_fighter(&self, manifest: FighterManifest) -> PunchResult<FighterId> {
        Ok(Ring::spawn_fighter(self, manifest).await)
    }

    async fn send_message_to_agent(
        &self,
        target: &FighterId,
        message: String,
    ) -> PunchResult<AgentMessageResult> {
        let result = self.send_message(target, message).await?;
        Ok(AgentMessageResult {
            response: result.response,
            tokens_used: result.usage.total(),
        })
    }

    async fn find_fighter_by_name(&self, name: &str) -> PunchResult<Option<FighterId>> {
        let found = self.fighters.iter().find_map(|entry| {
            if entry.value().manifest.name.eq_ignore_ascii_case(name) {
                Some(*entry.key())
            } else {
                None
            }
        });
        Ok(found)
    }

    async fn list_fighters(&self) -> PunchResult<Vec<AgentInfo>> {
        let fighters = self
            .fighters
            .iter()
            .map(|entry| AgentInfo {
                id: *entry.key(),
                name: entry.value().manifest.name.clone(),
                status: entry.value().status,
            })
            .collect();
        Ok(fighters)
    }
}

// ---------------------------------------------------------------------------
// Compile-time Send + Sync assertion
// ---------------------------------------------------------------------------

/// Compile-time assertion that `Ring` is `Send + Sync`.
const _: () = {
    fn _assert_send<T: Send>() {}
    fn _assert_sync<T: Sync>() {}
    fn _assert() {
        _assert_send::<Ring>();
        _assert_sync::<Ring>();
    }
};
