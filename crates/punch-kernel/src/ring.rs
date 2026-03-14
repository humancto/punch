//! **The Ring** — the central kernel and coordinator for the Punch system.
//!
//! The [`Ring`] owns every fighter and gorilla, wires them to the memory
//! substrate, the LLM driver, the event bus, the scheduler, the background
//! executor, the workflow engine, and the metering engine. All mutations
//! go through the Ring so that invariants (quotas, capabilities, lifecycle
//! events) are enforced in a single place.

use std::sync::Arc;

use async_trait::async_trait;
use chrono;
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
    AgentCoordinator, AgentInfo, AgentMessageResult, CoordinationStrategy, FighterId,
    FighterManifest, FighterStatus, GorillaId, GorillaManifest, GorillaMetrics, GorillaStatus,
    PunchConfig, PunchError, PunchEvent, PunchResult, TenantId, TenantStatus, Troop, TroopId,
};

use punch_skills::{SkillMarketplace, builtin_skills};

use crate::agent_messaging::MessageRouter;
use crate::background::BackgroundExecutor;
use crate::budget::BudgetEnforcer;
use crate::event_bus::EventBus;
use crate::metering::MeteringEngine;
use crate::metrics::{self, MetricsRegistry};
use crate::scheduler::{QuotaConfig, Scheduler};
use crate::swarm::SwarmCoordinator;
use crate::tenant_registry::TenantRegistry;
use crate::triggers::{Trigger, TriggerEngine, TriggerId, TriggerSummary};
use crate::troop::TroopManager;
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
    /// Budget enforcement layer (opt-in spending limits).
    budget_enforcer: Arc<BudgetEnforcer>,
    /// Event-driven trigger engine.
    trigger_engine: TriggerEngine,
    /// Troop manager for multi-agent coordination.
    troop_manager: TroopManager,
    /// Swarm coordinator for emergent behavior tasks.
    swarm_coordinator: SwarmCoordinator,
    /// Inter-agent message router.
    message_router: MessageRouter,
    /// Production observability metrics.
    metrics: Arc<MetricsRegistry>,
    /// Multi-tenant registry.
    tenant_registry: TenantRegistry,
    /// Skill marketplace for discovering and installing moves.
    marketplace: SkillMarketplace,
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
        let metering_arc = Arc::new(MeteringEngine::new(Arc::clone(&memory)));
        let budget_enforcer = Arc::new(BudgetEnforcer::new(Arc::clone(&metering_arc)));
        let metrics_registry = Arc::new(MetricsRegistry::new());
        metrics::register_default_metrics(&metrics_registry);

        let marketplace = SkillMarketplace::new();
        for listing in builtin_skills() {
            marketplace.publish(listing);
        }

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
            budget_enforcer,
            trigger_engine: TriggerEngine::new(),
            troop_manager: TroopManager::new(),
            swarm_coordinator: SwarmCoordinator::new(),
            message_router: MessageRouter::new(),
            metrics: metrics_registry,
            tenant_registry: TenantRegistry::new(),
            marketplace,
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
        let metering_arc = Arc::new(MeteringEngine::new(Arc::clone(&memory)));
        let budget_enforcer = Arc::new(BudgetEnforcer::new(Arc::clone(&metering_arc)));
        let metrics_registry = Arc::new(MetricsRegistry::new());
        metrics::register_default_metrics(&metrics_registry);

        let marketplace = SkillMarketplace::new();
        for listing in builtin_skills() {
            marketplace.publish(listing);
        }

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
            budget_enforcer,
            trigger_engine: TriggerEngine::new(),
            troop_manager: TroopManager::new(),
            swarm_coordinator: SwarmCoordinator::new(),
            message_router: MessageRouter::new(),
            metrics: metrics_registry,
            tenant_registry: TenantRegistry::new(),
            marketplace,
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

    /// Get a reference to the budget enforcer.
    pub fn budget_enforcer(&self) -> &Arc<BudgetEnforcer> {
        &self.budget_enforcer
    }

    /// Get a reference to the trigger engine.
    pub fn trigger_engine(&self) -> &TriggerEngine {
        &self.trigger_engine
    }

    /// Get a reference to the metrics registry.
    pub fn metrics(&self) -> &Arc<MetricsRegistry> {
        &self.metrics
    }

    /// Get a reference to the tenant registry.
    pub fn tenant_registry(&self) -> &TenantRegistry {
        &self.tenant_registry
    }

    /// Access the skill marketplace.
    pub fn marketplace(&self) -> &SkillMarketplace {
        &self.marketplace
    }

    // -- Tenant-scoped operations --------------------------------------------

    /// Spawn a fighter scoped to a tenant, enforcing quota limits.
    ///
    /// Returns an error if the tenant is suspended or the fighter quota is
    /// exceeded.
    #[instrument(skip(self, manifest), fields(fighter_name = %manifest.name))]
    pub async fn spawn_fighter_for_tenant(
        &self,
        tenant_id: &TenantId,
        mut manifest: FighterManifest,
    ) -> PunchResult<FighterId> {
        // Verify tenant exists and is active.
        let tenant = self
            .tenant_registry
            .get_tenant(tenant_id)
            .ok_or_else(|| PunchError::Tenant(format!("tenant {} not found", tenant_id)))?;

        if tenant.status == TenantStatus::Suspended {
            return Err(PunchError::Tenant(format!(
                "tenant {} is suspended",
                tenant_id
            )));
        }

        // Check fighter quota.
        let current_count = self
            .fighters
            .iter()
            .filter(|e| e.value().manifest.tenant_id.as_ref() == Some(tenant_id))
            .count();

        if current_count >= tenant.quota.max_fighters {
            return Err(PunchError::QuotaExceeded(format!(
                "tenant {} has reached max fighters limit ({})",
                tenant_id, tenant.quota.max_fighters
            )));
        }

        // Stamp the manifest with the tenant ID.
        manifest.tenant_id = Some(*tenant_id);
        Ok(self.spawn_fighter(manifest).await)
    }

    /// List fighters that belong to a specific tenant.
    pub fn list_fighters_for_tenant(
        &self,
        tenant_id: &TenantId,
    ) -> Vec<(FighterId, FighterManifest, FighterStatus)> {
        self.fighters
            .iter()
            .filter(|entry| entry.value().manifest.tenant_id.as_ref() == Some(tenant_id))
            .map(|entry| {
                let id = *entry.key();
                let e = entry.value();
                (id, e.manifest.clone(), e.status)
            })
            .collect()
    }

    /// Kill a fighter, validating that the caller tenant owns it.
    ///
    /// Returns an error if the fighter doesn't belong to the given tenant.
    #[instrument(skip(self), fields(%fighter_id, %tenant_id))]
    pub fn kill_fighter_for_tenant(
        &self,
        fighter_id: &FighterId,
        tenant_id: &TenantId,
    ) -> PunchResult<()> {
        let entry = self
            .fighters
            .get(fighter_id)
            .ok_or_else(|| PunchError::Fighter(format!("fighter {} not found", fighter_id)))?;

        if entry.manifest.tenant_id.as_ref() != Some(tenant_id) {
            return Err(PunchError::Auth(format!(
                "fighter {} does not belong to tenant {}",
                fighter_id, tenant_id
            )));
        }

        drop(entry);
        self.kill_fighter(fighter_id);
        Ok(())
    }

    /// Check whether a tenant's tool access is allowed for the given tool name.
    ///
    /// Returns `true` if the tenant has no tool restrictions (empty list) or
    /// the tool is in the allowed list.
    pub fn check_tenant_tool_access(&self, tenant_id: &TenantId, tool_name: &str) -> bool {
        match self.tenant_registry.get_tenant(tenant_id) {
            Some(tenant) => {
                tenant.quota.max_tools.is_empty()
                    || tenant.quota.max_tools.iter().any(|t| t == tool_name)
            }
            None => false,
        }
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

        // Record metrics.
        self.metrics.counter_inc(metrics::FIGHTER_SPAWNS_TOTAL);
        self.metrics
            .gauge_set(metrics::ACTIVE_FIGHTERS, self.fighters.len() as i64);

        self.event_bus.publish(PunchEvent::FighterSpawned {
            fighter_id: id,
            name: name.clone(),
        });

        info!(%id, name, "fighter spawned");

        // --- Creed binding ---
        // If a creed exists for this fighter name, bind it to the new instance.
        // This ensures the creed persists across kill/respawn cycles.
        {
            let memory = Arc::clone(&self.memory);
            let creed_name = name.clone();
            let fid = id;
            tokio::spawn(async move {
                if let Ok(Some(_)) = memory.load_creed_by_name(&creed_name).await {
                    if let Err(e) = memory.bind_creed_to_fighter(&creed_name, &fid).await {
                        warn!(error = %e, fighter = %creed_name, "failed to bind creed on spawn");
                    } else {
                        info!(fighter = %creed_name, id = %fid, "creed bound to fighter on spawn");
                    }
                }
            });
        }

        id
    }

    /// Create a default creed for a fighter if none exists.
    /// The default creed includes self-awareness from the manifest.
    pub async fn ensure_creed(&self, fighter_name: &str, manifest: &FighterManifest) {
        match self.memory.load_creed_by_name(fighter_name).await {
            Ok(Some(_)) => {
                // Creed already exists.
            }
            Ok(None) => {
                // Create a default creed with self-awareness.
                let creed = punch_types::Creed::new(fighter_name).with_self_awareness(manifest);
                if let Err(e) = self.memory.save_creed(&creed).await {
                    warn!(error = %e, "failed to create default creed");
                } else {
                    info!(fighter = %fighter_name, "default creed created with self-awareness");
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to check for existing creed");
            }
        }
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

        // Check budget enforcement (opt-in — only blocks if limits are configured).
        match self.budget_enforcer.check_budget(fighter_id).await {
            Ok(crate::budget::BudgetVerdict::Blocked {
                reason,
                retry_after_secs,
            }) => {
                entry.status = FighterStatus::Resting;
                return Err(PunchError::RateLimited {
                    provider: format!("budget: {}", reason),
                    retry_after_ms: retry_after_secs * 1000,
                });
            }
            Ok(crate::budget::BudgetVerdict::Warning { message, .. }) => {
                info!(warning = %message, "budget warning for fighter");
            }
            Ok(crate::budget::BudgetVerdict::Allowed) => {}
            Err(e) => {
                warn!(error = %e, "budget check failed, allowing request");
            }
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

        // Record message metric.
        self.metrics.counter_inc(metrics::MESSAGES_TOTAL);

        let result = run_fighter_loop(params).await;

        // Update state based on the outcome.
        if let Some(mut entry) = self.fighters.get_mut(fighter_id) {
            match &result {
                Ok(loop_result) => {
                    entry.status = FighterStatus::Idle;
                    self.scheduler
                        .record_usage(fighter_id, loop_result.usage.total());

                    // Record token usage metrics.
                    self.metrics
                        .counter_add(metrics::TOKENS_INPUT_TOTAL, loop_result.usage.input_tokens);
                    self.metrics.counter_add(
                        metrics::TOKENS_OUTPUT_TOTAL,
                        loop_result.usage.output_tokens,
                    );

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
                    self.metrics.counter_inc(metrics::ERRORS_TOTAL);
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
            self.metrics
                .gauge_set(metrics::ACTIVE_FIGHTERS, self.fighters.len() as i64);
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

        // Record gorilla metrics.
        self.metrics.counter_inc(metrics::GORILLA_RUNS_TOTAL);
        self.metrics.gauge_inc(metrics::ACTIVE_GORILLAS);

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

        self.metrics.gauge_dec(metrics::ACTIVE_GORILLAS);

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

    // -- Inter-agent communication -------------------------------------------

    /// Send a message from one fighter to another.
    ///
    /// The source fighter's message becomes the target fighter's input,
    /// enriched with source context so the target knows who is speaking.
    /// The target processes it through its own fighter loop (with its own creed)
    /// and the response is returned.
    #[instrument(skip(self, message), fields(%source_id, %target_id))]
    pub async fn fighter_to_fighter(
        &self,
        source_id: &FighterId,
        target_id: &FighterId,
        message: String,
    ) -> PunchResult<FighterLoopResult> {
        // Get source fighter name for context.
        let source_name = self
            .fighters
            .get(source_id)
            .map(|entry| entry.value().manifest.name.clone())
            .ok_or_else(|| {
                PunchError::Fighter(format!("source fighter {} not found", source_id))
            })?;

        // Verify target exists.
        if self.fighters.get(target_id).is_none() {
            return Err(PunchError::Fighter(format!(
                "target fighter {} not found",
                target_id
            )));
        }

        // Wrap the message with source context so the target knows who is speaking.
        let enriched_message = format!(
            "[Message from fighter '{}' (id: {})]\n\n{}",
            source_name, source_id, message
        );

        // Send to target through normal message flow (uses target's creed).
        self.send_message(target_id, enriched_message).await
    }

    /// Find a fighter by name (case-insensitive).
    ///
    /// Returns the fighter ID and manifest if found.
    pub fn find_fighter_by_name_sync(&self, name: &str) -> Option<(FighterId, FighterManifest)> {
        self.fighters.iter().find_map(|entry| {
            if entry.value().manifest.name.eq_ignore_ascii_case(name) {
                Some((*entry.key(), entry.value().manifest.clone()))
            } else {
                None
            }
        })
    }

    /// Update relationship tracking in both fighters' creeds after inter-agent
    /// communication.
    ///
    /// Loads both creeds, adds or updates the peer relationship entry
    /// (incrementing interaction_count), and saves them back.
    pub async fn update_fighter_relationships(&self, fighter_a_name: &str, fighter_b_name: &str) {
        let memory = Arc::clone(&self.memory);
        let a_name = fighter_a_name.to_string();
        let b_name = fighter_b_name.to_string();

        // Update in a spawned task to avoid blocking the caller.
        tokio::spawn(async move {
            // Update A's creed with relationship to B.
            if let Ok(Some(mut creed_a)) = memory.load_creed_by_name(&a_name).await {
                update_relationship(&mut creed_a, &b_name, None);
                if let Err(e) = memory.save_creed(&creed_a).await {
                    warn!(error = %e, fighter = %a_name, "failed to save creed relationship update");
                }
            }

            // Update B's creed with relationship to A.
            if let Ok(Some(mut creed_b)) = memory.load_creed_by_name(&b_name).await {
                update_relationship(&mut creed_b, &a_name, None);
                if let Err(e) = memory.save_creed(&creed_b).await {
                    warn!(error = %e, fighter = %b_name, "failed to save creed relationship update");
                }
            }
        });
    }

    // -- Troop / Swarm / Messaging accessors ---------------------------------

    /// Get a reference to the troop manager.
    pub fn troop_manager(&self) -> &TroopManager {
        &self.troop_manager
    }

    /// Get a reference to the swarm coordinator.
    pub fn swarm_coordinator(&self) -> &SwarmCoordinator {
        &self.swarm_coordinator
    }

    /// Get a reference to the message router.
    pub fn message_router(&self) -> &MessageRouter {
        &self.message_router
    }

    // -- Troop operations ----------------------------------------------------

    /// Form a new troop with a leader and initial members.
    #[instrument(skip(self, members), fields(troop_name = %name))]
    pub fn form_troop(
        &self,
        name: String,
        leader: FighterId,
        members: Vec<FighterId>,
        strategy: CoordinationStrategy,
    ) -> PunchResult<TroopId> {
        // Verify the leader exists.
        if self.fighters.get(&leader).is_none() {
            return Err(PunchError::Troop(format!(
                "leader fighter {} not found",
                leader
            )));
        }

        // Verify all members exist.
        for member in &members {
            if self.fighters.get(member).is_none() {
                return Err(PunchError::Troop(format!(
                    "member fighter {} not found",
                    member
                )));
            }
        }

        let member_count = members.len() + 1; // +1 for leader if not in list
        let troop_id = self
            .troop_manager
            .form_troop(name.clone(), leader, members, strategy);

        self.event_bus.publish(PunchEvent::TroopFormed {
            troop_id,
            name,
            member_count,
        });

        Ok(troop_id)
    }

    /// Disband (dissolve) a troop.
    #[instrument(skip(self), fields(%troop_id))]
    pub fn disband_troop(&self, troop_id: &TroopId) -> PunchResult<()> {
        let name = self.troop_manager.disband_troop(troop_id)?;

        self.event_bus.publish(PunchEvent::TroopDisbanded {
            troop_id: *troop_id,
            name,
        });

        Ok(())
    }

    /// Assign a task to a troop, returning the fighters that should handle it.
    pub fn assign_troop_task(
        &self,
        troop_id: &TroopId,
        task_description: &str,
    ) -> PunchResult<Vec<FighterId>> {
        self.troop_manager.assign_task(troop_id, task_description)
    }

    /// Get the current status of a troop.
    pub fn get_troop_status(&self, troop_id: &TroopId) -> Option<Troop> {
        self.troop_manager.get_troop(troop_id)
    }

    /// List all troops.
    pub fn list_troops(&self) -> Vec<Troop> {
        self.troop_manager.list_troops()
    }

    /// Recruit a fighter into a troop.
    pub fn recruit_to_troop(&self, troop_id: &TroopId, fighter_id: FighterId) -> PunchResult<()> {
        // Verify the fighter exists.
        if self.fighters.get(&fighter_id).is_none() {
            return Err(PunchError::Troop(format!(
                "fighter {} not found",
                fighter_id
            )));
        }
        self.troop_manager.recruit(troop_id, fighter_id)
    }

    /// Dismiss a fighter from a troop.
    pub fn dismiss_from_troop(
        &self,
        troop_id: &TroopId,
        fighter_id: &FighterId,
    ) -> PunchResult<()> {
        self.troop_manager.dismiss(troop_id, fighter_id)
    }

    // -- Troop-aware fighter lifecycle ---------------------------------------

    /// Kill a fighter, warning if they're in a troop.
    ///
    /// Unlike [`kill_fighter`], this checks troop membership and dismisses
    /// the fighter from all troops before killing them.
    #[instrument(skip(self), fields(%fighter_id))]
    pub fn kill_fighter_safe(&self, fighter_id: &FighterId) {
        // Dismiss from any troops first.
        let troop_ids = self.troop_manager.get_fighter_troops(fighter_id);
        for troop_id in troop_ids {
            if let Err(e) = self.troop_manager.dismiss(&troop_id, fighter_id) {
                warn!(
                    %troop_id,
                    %fighter_id,
                    error = %e,
                    "failed to dismiss fighter from troop before kill"
                );
            }
        }
        self.kill_fighter(fighter_id);
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
// Relationship tracking helper
// ---------------------------------------------------------------------------

/// Update or insert a peer relationship in a creed.
fn update_relationship(creed: &mut punch_types::Creed, peer_name: &str, trust_nudge: Option<f64>) {
    if let Some(rel) = creed
        .relationships
        .iter_mut()
        .find(|r| r.entity == peer_name && r.entity_type == "fighter")
    {
        rel.interaction_count += 1;
        if let Some(nudge) = trust_nudge {
            rel.trust = (rel.trust * 0.9 + nudge * 0.1).clamp(0.0, 1.0);
        }
        rel.notes = format!(
            "Last interaction: {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")
        );
    } else {
        creed.relationships.push(punch_types::Relationship {
            entity: peer_name.to_string(),
            entity_type: "fighter".to_string(),
            nature: "peer".to_string(),
            trust: trust_nudge.unwrap_or(0.5),
            interaction_count: 1,
            notes: format!(
                "First interaction: {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")
            ),
        });
    }
    creed.updated_at = chrono::Utc::now();
    creed.version += 1;
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
