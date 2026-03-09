//! Background executor for autonomous gorilla tasks.
//!
//! The [`BackgroundExecutor`] manages tokio tasks that run gorillas on their
//! configured schedules. Each gorilla gets its own spawned task that sleeps
//! for the configured interval, acquires a global LLM concurrency semaphore,
//! and then runs the fighter loop with an autonomous prompt.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use punch_memory::MemorySubstrate;
use punch_runtime::{FighterLoopParams, FighterLoopResult, LlmDriver, run_fighter_loop, tools_for_capabilities};
use punch_types::{
    FighterId, FighterManifest, GorillaId, GorillaManifest, ModelConfig, PunchResult, WeightClass,
};

/// Maximum concurrent LLM calls across all gorillas.
const DEFAULT_LLM_CONCURRENCY: usize = 3;

/// A running gorilla background task.
struct GorillaTask {
    handle: JoinHandle<()>,
    #[allow(dead_code)]
    started_at: DateTime<Utc>,
}

/// Manages background gorilla tasks that run autonomously on schedules.
pub struct BackgroundExecutor {
    /// Running gorilla tasks.
    tasks: DashMap<GorillaId, GorillaTask>,
    /// Global LLM concurrency limiter.
    llm_semaphore: Arc<Semaphore>,
    /// Shutdown signal sender (kept alive to prevent channel closure).
    _shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver (cloned for each gorilla task).
    shutdown_rx: watch::Receiver<bool>,
}

/// Build a [`FighterManifest`] from a [`GorillaManifest`], using the provided
/// `default_model` as a fallback when the gorilla does not specify its own model.
pub fn fighter_manifest_from_gorilla(
    manifest: &GorillaManifest,
    default_model: &ModelConfig,
) -> FighterManifest {
    let model = manifest.model.clone().unwrap_or_else(|| default_model.clone());
    let capabilities = manifest.effective_capabilities();
    let weight_class = manifest.weight_class.unwrap_or(WeightClass::Middleweight);
    let system_prompt = manifest.effective_system_prompt();

    FighterManifest {
        name: manifest.name.clone(),
        description: format!("Autonomous gorilla: {}", manifest.name),
        model,
        system_prompt,
        capabilities,
        weight_class,
    }
}

/// Run a single autonomous tick for a gorilla. This is the reusable core that
/// both the background scheduler and the CLI `gorilla test` command invoke.
pub async fn run_gorilla_tick(
    gorilla_id: GorillaId,
    manifest: &GorillaManifest,
    default_model: &ModelConfig,
    memory: &Arc<MemorySubstrate>,
    driver: &Arc<dyn LlmDriver>,
) -> PunchResult<FighterLoopResult> {
    let fighter_manifest = fighter_manifest_from_gorilla(manifest, default_model);
    let gorilla_name = &manifest.name;
    let system_prompt = fighter_manifest.system_prompt.clone();

    // Build the autonomous prompt.
    let autonomous_prompt = format!(
        "[AUTONOMOUS TICK] You are {}. Review your memory, check your goals, and take the next action. {}",
        gorilla_name, system_prompt
    );

    // Create a temporary fighter identity for this gorilla tick.
    let fighter_id = FighterId::new();

    // Save the fighter first (required for FK constraint on bout creation).
    if let Err(e) = memory
        .save_fighter(
            &fighter_id,
            &fighter_manifest,
            punch_types::FighterStatus::Idle,
        )
        .await
    {
        warn!(gorilla_id = %gorilla_id, error = %e, "failed to persist gorilla fighter");
    }

    // Create a bout for this tick.
    let bout_id = memory.create_bout(&fighter_id).await?;

    let available_tools = tools_for_capabilities(&fighter_manifest.capabilities);

    let params = FighterLoopParams {
        manifest: fighter_manifest,
        user_message: autonomous_prompt,
        bout_id,
        fighter_id,
        memory: Arc::clone(memory),
        driver: Arc::clone(driver),
        available_tools,
        max_iterations: Some(10),
        context_window: None,
        tool_timeout_secs: None,
        coordinator: None,
    };

    run_fighter_loop(params).await
}

impl BackgroundExecutor {
    /// Create a new background executor.
    pub fn new() -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            tasks: DashMap::new(),
            llm_semaphore: Arc::new(Semaphore::new(DEFAULT_LLM_CONCURRENCY)),
            _shutdown_tx: shutdown_tx,
            shutdown_rx,
        }
    }

    /// Create a new background executor with a custom shutdown channel.
    pub fn with_shutdown(
        shutdown_tx: watch::Sender<bool>,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Self {
        Self {
            tasks: DashMap::new(),
            llm_semaphore: Arc::new(Semaphore::new(DEFAULT_LLM_CONCURRENCY)),
            _shutdown_tx: shutdown_tx,
            shutdown_rx,
        }
    }

    /// Parse a schedule string like "every 30s", "every 5m", "every 1h", "every 1d"
    /// into a [`std::time::Duration`].
    pub fn parse_schedule(schedule: &str) -> Option<std::time::Duration> {
        let s = schedule.trim().to_lowercase();
        let s = s.strip_prefix("every ").unwrap_or(&s);
        let s = s.trim();

        if let Some(num_str) = s.strip_suffix('s') {
            num_str
                .trim()
                .parse::<u64>()
                .ok()
                .map(std::time::Duration::from_secs)
        } else if let Some(num_str) = s.strip_suffix('m') {
            num_str
                .trim()
                .parse::<u64>()
                .ok()
                .map(|m| std::time::Duration::from_secs(m * 60))
        } else if let Some(num_str) = s.strip_suffix('h') {
            num_str
                .trim()
                .parse::<u64>()
                .ok()
                .map(|h| std::time::Duration::from_secs(h * 3600))
        } else if let Some(num_str) = s.strip_suffix('d') {
            num_str
                .trim()
                .parse::<u64>()
                .ok()
                .map(|d| std::time::Duration::from_secs(d * 86400))
        } else {
            // Try to parse as raw seconds.
            s.parse::<u64>().ok().map(std::time::Duration::from_secs)
        }
    }

    /// Start a gorilla's autonomous background task.
    ///
    /// The task will loop on the gorilla's schedule, acquiring the LLM
    /// semaphore before each run, and executing the fighter loop with an
    /// autonomous prompt derived from the gorilla's manifest.
    ///
    /// `default_model` is used as a fallback when the gorilla manifest does
    /// not specify its own `model` configuration.
    pub fn start_gorilla(
        &self,
        id: GorillaId,
        manifest: GorillaManifest,
        default_model: ModelConfig,
        memory: Arc<MemorySubstrate>,
        driver: Arc<dyn LlmDriver>,
    ) -> PunchResult<()> {
        if self.tasks.contains_key(&id) {
            return Err(punch_types::PunchError::Gorilla(format!(
                "gorilla {} is already running",
                id
            )));
        }

        let interval = Self::parse_schedule(&manifest.schedule).unwrap_or_else(|| {
            warn!(
                gorilla_id = %id,
                schedule = %manifest.schedule,
                "could not parse schedule, defaulting to 5m"
            );
            std::time::Duration::from_secs(300)
        });

        let semaphore = Arc::clone(&self.llm_semaphore);
        let mut shutdown_rx = self.shutdown_rx.clone();
        let gorilla_name = manifest.name.clone();

        let handle = tokio::spawn(async move {
            info!(
                gorilla_id = %id,
                name = %gorilla_name,
                interval_secs = interval.as_secs(),
                "gorilla background task started"
            );

            let mut tasks_completed: u64 = 0;
            let mut error_count: u64 = 0;

            loop {
                // Sleep for the interval, checking shutdown signal.
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {},
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!(gorilla_id = %id, "gorilla received shutdown signal");
                            break;
                        }
                    }
                }

                // Check shutdown before proceeding.
                if *shutdown_rx.borrow() {
                    break;
                }

                // Acquire semaphore permit.
                let _permit = match semaphore.acquire().await {
                    Ok(permit) => permit,
                    Err(_) => {
                        warn!(gorilla_id = %id, "semaphore closed, stopping gorilla");
                        break;
                    }
                };

                match run_gorilla_tick(id, &manifest, &default_model, &memory, &driver).await {
                    Ok(result) => {
                        tasks_completed += 1;
                        info!(
                            gorilla_id = %id,
                            tasks_completed,
                            tokens = result.usage.total(),
                            "gorilla tick completed successfully"
                        );
                    }
                    Err(e) => {
                        error_count += 1;
                        error!(
                            gorilla_id = %id,
                            error = %e,
                            error_count,
                            "gorilla tick failed"
                        );
                    }
                }
            }

            info!(
                gorilla_id = %id,
                tasks_completed,
                "gorilla background task stopped"
            );
        });

        self.tasks.insert(
            id,
            GorillaTask {
                handle,
                started_at: Utc::now(),
            },
        );

        Ok(())
    }

    /// Stop a gorilla's background task by aborting it.
    pub fn stop_gorilla(&self, id: &GorillaId) -> bool {
        if let Some((_, task)) = self.tasks.remove(id) {
            task.handle.abort();
            info!(gorilla_id = %id, "gorilla task stopped");
            true
        } else {
            false
        }
    }

    /// Check whether a gorilla is currently running.
    pub fn is_running(&self, id: &GorillaId) -> bool {
        self.tasks.contains_key(id)
    }

    /// List all currently running gorilla IDs.
    pub fn list_running(&self) -> Vec<GorillaId> {
        self.tasks.iter().map(|entry| *entry.key()).collect()
    }

    /// Shutdown all running gorilla tasks.
    pub fn shutdown_all(&self) {
        let ids: Vec<GorillaId> = self.tasks.iter().map(|e| *e.key()).collect();
        for id in &ids {
            if let Some((_, task)) = self.tasks.remove(id) {
                task.handle.abort();
            }
        }
        info!(count = ids.len(), "all gorilla tasks shut down");
    }

    /// Returns the number of currently running gorilla tasks.
    pub fn running_count(&self) -> usize {
        self.tasks.len()
    }
}

impl Default for BackgroundExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_schedule_seconds() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("every 30s"),
            Some(std::time::Duration::from_secs(30))
        );
    }

    #[test]
    fn parse_schedule_minutes() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("every 5m"),
            Some(std::time::Duration::from_secs(300))
        );
    }

    #[test]
    fn parse_schedule_hours() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("every 1h"),
            Some(std::time::Duration::from_secs(3600))
        );
    }

    #[test]
    fn parse_schedule_days() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("every 1d"),
            Some(std::time::Duration::from_secs(86400))
        );
    }

    #[test]
    fn parse_schedule_invalid() {
        assert_eq!(BackgroundExecutor::parse_schedule("invalid"), None);
    }

    #[tokio::test]
    async fn start_and_stop_gorilla() {
        let executor = BackgroundExecutor::new();
        let id = GorillaId::new();
        let _manifest = GorillaManifest {
            name: "test-gorilla".to_string(),
            description: "test".to_string(),
            schedule: "every 30s".to_string(),
            moves_required: Vec::new(),
            settings_schema: None,
            dashboard_metrics: Vec::new(),
            system_prompt: None,
            model: None,
            capabilities: Vec::new(),
            weight_class: None,
        };

        // We can't actually run the gorilla loop without a real driver/memory,
        // but we can test the task management.
        let handle = tokio::spawn(async {
            futures::future::pending::<()>().await;
        });

        executor.tasks.insert(
            id,
            GorillaTask {
                handle,
                started_at: Utc::now(),
            },
        );

        assert_eq!(executor.running_count(), 1);
        assert!(executor.list_running().contains(&id));

        assert!(executor.stop_gorilla(&id));
        assert_eq!(executor.running_count(), 0);
    }

    #[tokio::test]
    async fn shutdown_all_stops_everything() {
        let executor = BackgroundExecutor::new();

        for _ in 0..3 {
            let id = GorillaId::new();
            let handle = tokio::spawn(async {
                futures::future::pending::<()>().await;
            });
            executor.tasks.insert(
                id,
                GorillaTask {
                    handle,
                    started_at: Utc::now(),
                },
            );
        }

        assert_eq!(executor.running_count(), 3);
        executor.shutdown_all();
        assert_eq!(executor.running_count(), 0);
    }

    #[tokio::test]
    async fn stop_nonexistent_gorilla_returns_false() {
        let executor = BackgroundExecutor::new();
        let id = GorillaId::new();
        assert!(!executor.stop_gorilla(&id));
    }
}
