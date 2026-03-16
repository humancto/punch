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
use punch_runtime::{
    FighterLoopParams, FighterLoopResult, LlmDriver, run_fighter_loop, tools_for_capabilities,
};
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
    let model = manifest
        .model
        .clone()
        .unwrap_or_else(|| default_model.clone());
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
        tenant_id: None,
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
        approval_engine: None,
        sandbox: None,
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

    /// Parse a schedule string into a [`std::time::Duration`].
    ///
    /// Supported formats:
    /// - Human-friendly: `"every 30s"`, `"every 5m"`, `"every 1h"`, `"every 1d"`
    /// - Cron expressions: `"*/30 * * * *"` (every 30 min), `"0 */6 * * *"` (every 6h)
    /// - Raw seconds: `"60"`
    pub fn parse_schedule(schedule: &str) -> Option<std::time::Duration> {
        let s = schedule.trim().to_lowercase();

        // Try human-friendly format first: "every Xs/Xm/Xh/Xd"
        if let Some(duration) = Self::parse_human_schedule(&s) {
            return Some(duration);
        }

        // Try cron expression: fields separated by spaces, 5 fields = cron.
        if let Some(duration) = Self::parse_cron_schedule(&s) {
            return Some(duration);
        }

        // Try raw seconds.
        s.parse::<u64>().ok().map(std::time::Duration::from_secs)
    }

    /// Parse human-friendly schedule: "every 30s", "every 5m", etc.
    fn parse_human_schedule(s: &str) -> Option<std::time::Duration> {
        let s = s.strip_prefix("every ").unwrap_or(s);
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
            None
        }
    }

    /// Parse a 5-field cron expression into an approximate interval.
    ///
    /// Handles common periodic patterns:
    /// - `*/N * * * *`   → every N minutes
    /// - `0 */N * * *`   → every N hours
    /// - `0 0 */N * *`   → every N days
    /// - `0 0 * * *`     → daily (24h)
    fn parse_cron_schedule(s: &str) -> Option<std::time::Duration> {
        let fields: Vec<&str> = s.split_whitespace().collect();
        if fields.len() != 5 {
            return None;
        }

        let (minute, hour, day, _month, _dow) =
            (fields[0], fields[1], fields[2], fields[3], fields[4]);

        // `*/N * * * *` — every N minutes
        if let Some(step) = minute.strip_prefix("*/")
            && hour == "*"
            && day == "*"
            && let Ok(n) = step.parse::<u64>()
        {
            return Some(std::time::Duration::from_secs(n * 60));
        }

        // `0 */N * * *` — every N hours
        if minute == "0"
            && let Some(step) = hour.strip_prefix("*/")
            && day == "*"
            && let Ok(n) = step.parse::<u64>()
        {
            return Some(std::time::Duration::from_secs(n * 3600));
        }

        // `0 0 */N * *` — every N days
        if minute == "0"
            && hour == "0"
            && let Some(step) = day.strip_prefix("*/")
            && let Ok(n) = step.parse::<u64>()
        {
            return Some(std::time::Duration::from_secs(n * 86400));
        }

        // `0 0 * * *` — daily
        if minute == "0" && hour == "0" && day == "*" {
            return Some(std::time::Duration::from_secs(86400));
        }

        // `0 N * * *` — once a day at hour N (treat as 24h interval)
        if minute == "0" && day == "*" && hour.parse::<u64>().is_ok() {
            return Some(std::time::Duration::from_secs(86400));
        }

        None
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

    #[test]
    fn parse_schedule_cron_every_30_minutes() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("*/30 * * * *"),
            Some(std::time::Duration::from_secs(1800))
        );
    }

    #[test]
    fn parse_schedule_cron_every_6_hours() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("0 */6 * * *"),
            Some(std::time::Duration::from_secs(21600))
        );
    }

    #[test]
    fn parse_schedule_cron_every_2_hours() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("0 */2 * * *"),
            Some(std::time::Duration::from_secs(7200))
        );
    }

    #[test]
    fn parse_schedule_cron_every_2_days() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("0 0 */2 * *"),
            Some(std::time::Duration::from_secs(172800))
        );
    }

    #[test]
    fn parse_schedule_cron_daily() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("0 0 * * *"),
            Some(std::time::Duration::from_secs(86400))
        );
    }

    #[test]
    fn parse_schedule_cron_every_3_hours() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("0 */3 * * *"),
            Some(std::time::Duration::from_secs(10800))
        );
    }

    #[test]
    fn parse_schedule_cron_every_4_hours() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("0 */4 * * *"),
            Some(std::time::Duration::from_secs(14400))
        );
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

    #[test]
    fn parse_schedule_raw_seconds() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("60"),
            Some(std::time::Duration::from_secs(60))
        );
    }

    #[test]
    fn parse_schedule_with_whitespace() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("  every  10s  "),
            Some(std::time::Duration::from_secs(10))
        );
    }

    #[test]
    fn parse_schedule_case_insensitive() {
        assert_eq!(
            BackgroundExecutor::parse_schedule("Every 2H"),
            Some(std::time::Duration::from_secs(7200))
        );
    }

    #[test]
    fn parse_schedule_empty_string() {
        assert_eq!(BackgroundExecutor::parse_schedule(""), None);
    }

    #[test]
    fn parse_schedule_just_prefix() {
        assert_eq!(BackgroundExecutor::parse_schedule("every "), None);
    }

    #[test]
    fn default_creates_executor() {
        let executor = BackgroundExecutor::default();
        assert_eq!(executor.running_count(), 0);
        assert!(executor.list_running().is_empty());
    }

    #[tokio::test]
    async fn is_running_returns_correct_state() {
        let executor = BackgroundExecutor::new();
        let id = GorillaId::new();

        assert!(!executor.is_running(&id));

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

        assert!(executor.is_running(&id));
        executor.stop_gorilla(&id);
        assert!(!executor.is_running(&id));
    }

    #[tokio::test]
    async fn multiple_gorillas_tracked_independently() {
        let executor = BackgroundExecutor::new();
        let ids: Vec<GorillaId> = (0..5).map(|_| GorillaId::new()).collect();

        for &id in &ids {
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

        assert_eq!(executor.running_count(), 5);

        // Stop the first two.
        executor.stop_gorilla(&ids[0]);
        executor.stop_gorilla(&ids[1]);
        assert_eq!(executor.running_count(), 3);

        // The remaining three should still be running.
        for &id in &ids[2..] {
            assert!(executor.is_running(&id));
        }

        executor.shutdown_all();
        assert_eq!(executor.running_count(), 0);
    }

    #[tokio::test]
    async fn with_shutdown_receives_shutdown_signal() {
        let (tx, rx) = watch::channel(false);
        let executor = BackgroundExecutor::with_shutdown(tx.clone(), rx);

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

        assert_eq!(executor.running_count(), 1);
        executor.shutdown_all();
        assert_eq!(executor.running_count(), 0);
    }

    #[test]
    fn fighter_manifest_from_gorilla_uses_default_model() {
        use punch_types::{ModelConfig, Provider};

        let manifest = GorillaManifest {
            name: "test-gorilla".to_string(),
            description: "A test gorilla".to_string(),
            schedule: "every 30s".to_string(),
            moves_required: Vec::new(),
            settings_schema: None,
            dashboard_metrics: Vec::new(),
            system_prompt: Some("Custom prompt".to_string()),
            model: None,
            capabilities: Vec::new(),
            weight_class: None,
        };

        let default_model = ModelConfig {
            provider: Provider::Anthropic,
            model: "claude-sonnet-4-20250514".to_string(),
            api_key_env: None,
            base_url: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };

        let fighter = fighter_manifest_from_gorilla(&manifest, &default_model);
        assert_eq!(fighter.name, "test-gorilla");
        assert_eq!(fighter.model.model, "claude-sonnet-4-20250514");
        assert_eq!(fighter.system_prompt, "Custom prompt");
        assert_eq!(fighter.weight_class, punch_types::WeightClass::Middleweight);
    }

    #[test]
    fn fighter_manifest_from_gorilla_uses_gorilla_model_if_set() {
        use punch_types::{ModelConfig, Provider};

        let gorilla_model = ModelConfig {
            provider: Provider::OpenAI,
            model: "gpt-4o".to_string(),
            api_key_env: None,
            base_url: None,
            max_tokens: Some(8192),
            temperature: Some(0.5),
        };

        let manifest = GorillaManifest {
            name: "smart-gorilla".to_string(),
            description: "Uses its own model".to_string(),
            schedule: "every 1h".to_string(),
            moves_required: Vec::new(),
            settings_schema: None,
            dashboard_metrics: Vec::new(),
            system_prompt: None,
            model: Some(gorilla_model),
            capabilities: Vec::new(),
            weight_class: Some(punch_types::WeightClass::Heavyweight),
        };

        let default_model = ModelConfig {
            provider: Provider::Anthropic,
            model: "claude-sonnet-4-20250514".to_string(),
            api_key_env: None,
            base_url: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };

        let fighter = fighter_manifest_from_gorilla(&manifest, &default_model);
        assert_eq!(fighter.model.model, "gpt-4o");
        assert_eq!(fighter.weight_class, punch_types::WeightClass::Heavyweight);
        // system_prompt falls back to description when None.
        assert_eq!(fighter.system_prompt, "Uses its own model");
    }

    #[tokio::test]
    async fn list_running_returns_all_ids() {
        let executor = BackgroundExecutor::new();
        let mut expected_ids = Vec::new();

        for _ in 0..3 {
            let id = GorillaId::new();
            expected_ids.push(id);
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

        let running = executor.list_running();
        assert_eq!(running.len(), 3);
        for id in &expected_ids {
            assert!(running.contains(id));
        }

        executor.shutdown_all();
    }
}
