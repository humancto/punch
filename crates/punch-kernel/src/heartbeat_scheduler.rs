//! Proactive heartbeat scheduler for fighter agents.
//!
//! Wakes fighters on a timer to execute due heartbeat tasks without requiring
//! a user message. Each fighter with active timed heartbeats gets its own
//! monitoring tokio task that sleeps until the next due time, runs a bout,
//! and lets the fighter notify users via `channel_notify`.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use punch_memory::MemorySubstrate;
use punch_runtime::{FighterLoopParams, LlmDriver, run_fighter_loop, tools_for_capabilities};
use punch_types::config::ModelRoutingConfig;
use punch_types::creed::Creed;
use punch_types::{ChannelNotifier, FighterId, FighterManifest, PunchEvent};

use crate::background::BackgroundExecutor;
use crate::event_bus::EventBus;

/// A running heartbeat monitoring task for a single fighter.
struct HeartbeatMonitor {
    handle: JoinHandle<()>,
    #[allow(dead_code)]
    started_at: DateTime<Utc>,
}

/// Manages per-fighter heartbeat monitoring tasks.
///
/// For each fighter with active timed heartbeats, spawns a tokio task that:
/// 1. Computes next wake time from the finest active cadence
/// 2. Sleeps until wake time (or shutdown signal)
/// 3. Loads the creed, checks for due tasks
/// 4. Runs a heartbeat bout so the fighter can execute tasks and notify users
/// 5. Marks tasks as checked, saves creed, loops
pub struct HeartbeatScheduler {
    /// Running monitors, keyed by fighter ID.
    monitors: DashMap<FighterId, HeartbeatMonitor>,
    /// Shutdown signal sender.
    _shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver (cloned per monitor).
    shutdown_rx: watch::Receiver<bool>,
}

/// Configuration for starting heartbeat monitoring on a fighter.
pub struct HeartbeatStartConfig {
    pub fighter_id: FighterId,
    pub fighter_name: String,
    pub manifest: FighterManifest,
    pub memory: Arc<MemorySubstrate>,
    pub driver: Arc<dyn LlmDriver>,
    pub event_bus: EventBus,
    pub channel_notifier: Option<Arc<dyn ChannelNotifier>>,
    pub model_routing: Option<ModelRoutingConfig>,
    pub chat_id_hint: Option<String>,
}

impl HeartbeatScheduler {
    /// Create a new scheduler that shares the given shutdown signal.
    pub fn with_shutdown(
        shutdown_tx: watch::Sender<bool>,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Self {
        Self {
            monitors: DashMap::new(),
            _shutdown_tx: shutdown_tx,
            shutdown_rx,
        }
    }

    /// Start heartbeat monitoring for a fighter.
    ///
    /// If the fighter has no timed heartbeats (only `every_bout` or `on_wake`),
    /// monitoring is skipped since those only fire on user messages or spawn.
    pub fn start_monitoring(&self, cfg: HeartbeatStartConfig) {
        // Don't double-monitor.
        if self.monitors.contains_key(&cfg.fighter_id) {
            debug!(fighter = %cfg.fighter_name, "heartbeat monitoring already active, skipping");
            return;
        }

        let fighter_id = cfg.fighter_id;
        let fighter_name = cfg.fighter_name.clone();

        let handle = tokio::spawn(heartbeat_loop(HeartbeatLoopConfig {
            fighter_id: cfg.fighter_id,
            fighter_name: cfg.fighter_name,
            manifest: cfg.manifest,
            memory: cfg.memory,
            driver: cfg.driver,
            event_bus: cfg.event_bus,
            channel_notifier: cfg.channel_notifier,
            model_routing: cfg.model_routing,
            chat_id_hint: cfg.chat_id_hint,
            shutdown_rx: self.shutdown_rx.clone(),
        }));

        self.monitors.insert(
            fighter_id,
            HeartbeatMonitor {
                handle,
                started_at: Utc::now(),
            },
        );

        info!(fighter = %fighter_name, "heartbeat monitoring started");
    }

    /// Stop heartbeat monitoring for a fighter.
    pub fn stop_monitoring(&self, fighter_id: &FighterId) {
        if let Some((_, monitor)) = self.monitors.remove(fighter_id) {
            monitor.handle.abort();
            info!(%fighter_id, "heartbeat monitoring stopped");
        }
    }

    /// Refresh monitoring for a fighter after heartbeat config changes.
    ///
    /// Stops and restarts the monitoring task so it picks up new cadences.
    pub fn refresh(&self, cfg: HeartbeatStartConfig) {
        self.stop_monitoring(&cfg.fighter_id);
        self.start_monitoring(cfg);
    }

    /// Check if a fighter is being monitored.
    pub fn is_monitoring(&self, fighter_id: &FighterId) -> bool {
        self.monitors.contains_key(fighter_id)
    }

    /// Number of fighters being monitored.
    pub fn monitoring_count(&self) -> usize {
        self.monitors.len()
    }

    /// Shut down all monitoring tasks.
    pub fn shutdown_all(&self) {
        for entry in self.monitors.iter() {
            entry.value().handle.abort();
        }
        self.monitors.clear();
        info!("all heartbeat monitors shut down");
    }
}

/// Configuration bundle for a heartbeat monitoring loop.
struct HeartbeatLoopConfig {
    fighter_id: FighterId,
    fighter_name: String,
    manifest: FighterManifest,
    memory: Arc<MemorySubstrate>,
    driver: Arc<dyn LlmDriver>,
    event_bus: EventBus,
    channel_notifier: Option<Arc<dyn ChannelNotifier>>,
    model_routing: Option<ModelRoutingConfig>,
    chat_id_hint: Option<String>,
    shutdown_rx: watch::Receiver<bool>,
}

/// The per-fighter monitoring loop.
async fn heartbeat_loop(mut cfg: HeartbeatLoopConfig) {
    let HeartbeatLoopConfig {
        fighter_id,
        ref fighter_name,
        ref manifest,
        ref memory,
        ref driver,
        ref event_bus,
        ref channel_notifier,
        ref model_routing,
        ref chat_id_hint,
        ref mut shutdown_rx,
    } = cfg;
    info!(fighter = %fighter_name, "heartbeat loop started");

    loop {
        // 1. Load creed to compute sleep duration.
        let creed = match memory.load_creed_by_name(fighter_name).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                debug!(fighter = %fighter_name, "no creed found, sleeping 60s");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(60)) => continue,
                    _ = shutdown_rx.changed() => break,
                }
            }
            Err(e) => {
                warn!(fighter = %fighter_name, error = %e, "failed to load creed, sleeping 60s");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(60)) => continue,
                    _ = shutdown_rx.changed() => break,
                }
            }
        };

        // 2. Check for already-due tasks before sleeping (handles overdue
        //    heartbeats after restart or monitor start).
        let mut creed = creed;

        // 3. Check due tasks (timed only — skip every_bout and on_wake).
        let due_indices: Vec<usize> = creed
            .heartbeat
            .iter()
            .enumerate()
            .filter(|(_, h)| {
                if !h.active {
                    return false;
                }
                is_timed_cadence_due(h)
            })
            .map(|(i, _)| i)
            .collect();

        // 4. Execute due tasks if any.
        if !due_indices.is_empty() {
            let due_tasks: Vec<String> = due_indices
                .iter()
                .map(|&i| creed.heartbeat[i].task.clone())
                .collect();

            info!(
                fighter = %fighter_name,
                tasks = due_tasks.len(),
                "executing heartbeat bout"
            );

            // Build the heartbeat prompt.
            let mut prompt = format!(
                "[HEARTBEAT] You are {}. The following proactive tasks are due:\n{}",
                fighter_name,
                due_tasks
                    .iter()
                    .map(|t| format!("- {t}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );

            prompt.push_str(
                "\n\nExecute each task using your available tools. \
                 If you have results to share with the user, use channel_notify to message them.",
            );

            // Inject chat_id hint if available.
            if let Some(cid) = chat_id_hint {
                prompt.push_str(&format!(
                    "\nTo notify the user, use channel_notify with channel=\"telegram\" and chat_id=\"{cid}\"."
                ));
            }

            // Run a heartbeat bout.
            let bout_id = match memory.create_bout(&fighter_id).await {
                Ok(id) => id,
                Err(e) => {
                    warn!(fighter = %fighter_name, error = %e, "failed to create heartbeat bout");
                    // Still sleep before retrying.
                    let sleep_dur = compute_next_wake(&creed);
                    tokio::select! {
                        _ = tokio::time::sleep(sleep_dur) => continue,
                        _ = shutdown_rx.changed() => break,
                    }
                }
            };

            let params = FighterLoopParams {
                manifest: manifest.clone(),
                user_message: prompt,
                bout_id,
                fighter_id,
                memory: Arc::clone(memory),
                driver: Arc::clone(driver),
                available_tools: tools_for_capabilities(&manifest.capabilities),
                max_iterations: Some(10),
                context_window: None,
                tool_timeout_secs: Some(60),
                coordinator: None,
                approval_engine: None,
                sandbox: None,
                mcp_clients: None,
                model_routing: model_routing.clone(),
                channel_notifier: channel_notifier.clone(),
                user_content_parts: vec![],
            };

            match run_fighter_loop(params).await {
                Ok(_) => {
                    // Mark heartbeat tasks checked.
                    for &idx in &due_indices {
                        creed.mark_heartbeat_checked(idx);
                    }
                    if let Err(e) = memory.save_creed(&creed).await {
                        warn!(fighter = %fighter_name, error = %e, "failed to save creed after heartbeat bout");
                    }

                    // Publish events.
                    for task in &due_tasks {
                        event_bus.publish(PunchEvent::HeartbeatExecuted {
                            fighter_id,
                            task_description: task.clone(),
                        });
                    }

                    info!(
                        fighter = %fighter_name,
                        tasks = due_tasks.len(),
                        "heartbeat bout completed"
                    );
                }
                Err(e) => {
                    error!(fighter = %fighter_name, error = %e, "heartbeat bout failed");
                }
            }
        }

        // 5. Compute sleep duration and wait for next cycle.
        let sleep_dur = compute_next_wake(&creed);

        // If no timed heartbeats, sleep for 5 minutes and re-check
        // (fighter may add heartbeats later via self-config tools).
        if sleep_dur == Duration::from_secs(300) && !has_timed_heartbeats(&creed) {
            tokio::select! {
                _ = tokio::time::sleep(sleep_dur) => continue,
                _ = shutdown_rx.changed() => break,
            }
        }

        debug!(
            fighter = %fighter_name,
            sleep_secs = sleep_dur.as_secs(),
            "heartbeat sleeping until next wake"
        );

        tokio::select! {
            _ = tokio::time::sleep(sleep_dur) => {},
            _ = shutdown_rx.changed() => break,
        }
    }

    info!(fighter = %fighter_name, "heartbeat loop exiting");
}

/// Compute the sleep duration until the next heartbeat wake.
///
/// Finds the shortest timed cadence among active heartbeat tasks. Falls back
/// to 5 minutes if no timed heartbeats are active (the loop re-checks in case
/// the fighter adds heartbeats later).
pub fn compute_next_wake(creed: &Creed) -> Duration {
    let mut min_secs = u64::MAX;

    for h in &creed.heartbeat {
        if !h.active {
            continue;
        }
        let secs = match h.cadence.as_str() {
            "every_bout" | "on_wake" => continue, // Not timer-driven
            "hourly" => 3600,
            "daily" => 86400,
            "weekly" => 604800,
            other => {
                // Try parsing via BackgroundExecutor's schedule parser.
                BackgroundExecutor::parse_schedule(other)
                    .map(|d| d.as_secs())
                    .unwrap_or(u64::MAX)
            }
        };
        min_secs = min_secs.min(secs);
    }

    if min_secs == u64::MAX {
        Duration::from_secs(300) // Default 5 minute check-in
    } else {
        Duration::from_secs(min_secs)
    }
}

/// Check if a heartbeat task with a timed cadence is due.
fn is_timed_cadence_due(h: &punch_types::creed::HeartbeatTask) -> bool {
    let now = Utc::now();
    let required_secs = match h.cadence.as_str() {
        "every_bout" | "on_wake" => return false, // Not timer-driven
        "hourly" => 3600i64,
        "daily" => 86400,
        "weekly" => 604800,
        other => BackgroundExecutor::parse_schedule(other)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    };

    if required_secs == 0 {
        return false;
    }

    match h.last_checked {
        None => true, // Never run before
        Some(t) => (now - t).num_seconds() >= required_secs,
    }
}

/// Check if a creed has any timed (not `every_bout`/`on_wake`) heartbeats.
fn has_timed_heartbeats(creed: &Creed) -> bool {
    creed
        .heartbeat
        .iter()
        .any(|h| h.active && !matches!(h.cadence.as_str(), "every_bout" | "on_wake"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::creed::HeartbeatTask;

    fn make_creed_with_heartbeats(cadences: &[&str]) -> Creed {
        let mut creed = Creed::new("test-fighter");
        for cadence in cadences {
            creed.heartbeat.push(HeartbeatTask {
                task: format!("Task with cadence {cadence}"),
                cadence: cadence.to_string(),
                active: true,
                execution_count: 0,
                last_checked: None,
            });
        }
        creed
    }

    // -----------------------------------------------------------------------
    // compute_next_wake tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_compute_next_wake_hourly() {
        let creed = make_creed_with_heartbeats(&["hourly"]);
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(3600));
    }

    #[test]
    fn test_compute_next_wake_daily() {
        let creed = make_creed_with_heartbeats(&["daily"]);
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(86400));
    }

    #[test]
    fn test_compute_next_wake_weekly() {
        let creed = make_creed_with_heartbeats(&["weekly"]);
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(604800));
    }

    #[test]
    fn test_compute_next_wake_every_30m() {
        let creed = make_creed_with_heartbeats(&["every 30m"]);
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(1800));
    }

    #[test]
    fn test_compute_next_wake_every_5m() {
        let creed = make_creed_with_heartbeats(&["every 5m"]);
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(300));
    }

    #[test]
    fn test_compute_next_wake_cron_every_10min() {
        let creed = make_creed_with_heartbeats(&["*/10 * * * *"]);
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(600));
    }

    #[test]
    fn test_compute_next_wake_picks_finest_cadence() {
        let creed = make_creed_with_heartbeats(&["daily", "hourly", "every 15m"]);
        // 15 minutes is the shortest
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(900));
    }

    #[test]
    fn test_compute_next_wake_ignores_every_bout_and_on_wake() {
        let creed = make_creed_with_heartbeats(&["every_bout", "on_wake"]);
        // No timed heartbeats → default 5 min
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(300));
    }

    #[test]
    fn test_compute_next_wake_empty_heartbeats() {
        let creed = Creed::new("empty");
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(300));
    }

    #[test]
    fn test_compute_next_wake_inactive_heartbeats() {
        let mut creed = Creed::new("test");
        creed.heartbeat.push(HeartbeatTask {
            task: "inactive".to_string(),
            cadence: "hourly".to_string(),
            active: false,
            execution_count: 0,
            last_checked: None,
        });
        assert_eq!(compute_next_wake(&creed), Duration::from_secs(300));
    }

    // -----------------------------------------------------------------------
    // is_timed_cadence_due tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_timed_cadence_due_never_checked() {
        let task = HeartbeatTask {
            task: "test".to_string(),
            cadence: "hourly".to_string(),
            active: true,
            execution_count: 0,
            last_checked: None,
        };
        assert!(is_timed_cadence_due(&task));
    }

    #[test]
    fn test_timed_cadence_due_recently_checked() {
        let task = HeartbeatTask {
            task: "test".to_string(),
            cadence: "hourly".to_string(),
            active: true,
            execution_count: 1,
            last_checked: Some(Utc::now()),
        };
        assert!(!is_timed_cadence_due(&task));
    }

    #[test]
    fn test_timed_cadence_due_elapsed() {
        let task = HeartbeatTask {
            task: "test".to_string(),
            cadence: "hourly".to_string(),
            active: true,
            execution_count: 1,
            last_checked: Some(Utc::now() - chrono::Duration::hours(2)),
        };
        assert!(is_timed_cadence_due(&task));
    }

    #[test]
    fn test_timed_cadence_every_bout_not_timed() {
        let task = HeartbeatTask {
            task: "test".to_string(),
            cadence: "every_bout".to_string(),
            active: true,
            execution_count: 0,
            last_checked: None,
        };
        assert!(!is_timed_cadence_due(&task));
    }

    #[test]
    fn test_timed_cadence_on_wake_not_timed() {
        let task = HeartbeatTask {
            task: "test".to_string(),
            cadence: "on_wake".to_string(),
            active: true,
            execution_count: 0,
            last_checked: None,
        };
        assert!(!is_timed_cadence_due(&task));
    }

    #[test]
    fn test_timed_cadence_custom_schedule() {
        let task = HeartbeatTask {
            task: "test".to_string(),
            cadence: "every 30m".to_string(),
            active: true,
            execution_count: 1,
            last_checked: Some(Utc::now() - chrono::Duration::minutes(31)),
        };
        assert!(is_timed_cadence_due(&task));
    }

    #[test]
    fn test_timed_cadence_custom_schedule_not_due() {
        let task = HeartbeatTask {
            task: "test".to_string(),
            cadence: "every 30m".to_string(),
            active: true,
            execution_count: 1,
            last_checked: Some(Utc::now() - chrono::Duration::minutes(10)),
        };
        assert!(!is_timed_cadence_due(&task));
    }

    #[test]
    fn test_timed_cadence_weekly_due() {
        let task = HeartbeatTask {
            task: "test".to_string(),
            cadence: "weekly".to_string(),
            active: true,
            execution_count: 1,
            last_checked: Some(Utc::now() - chrono::Duration::days(8)),
        };
        assert!(is_timed_cadence_due(&task));
    }

    #[test]
    fn test_timed_cadence_weekly_not_due() {
        let task = HeartbeatTask {
            task: "test".to_string(),
            cadence: "weekly".to_string(),
            active: true,
            execution_count: 1,
            last_checked: Some(Utc::now() - chrono::Duration::days(3)),
        };
        assert!(!is_timed_cadence_due(&task));
    }

    // -----------------------------------------------------------------------
    // has_timed_heartbeats tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_has_timed_heartbeats_true() {
        let creed = make_creed_with_heartbeats(&["hourly"]);
        assert!(has_timed_heartbeats(&creed));
    }

    #[test]
    fn test_has_timed_heartbeats_false_only_bout() {
        let creed = make_creed_with_heartbeats(&["every_bout", "on_wake"]);
        assert!(!has_timed_heartbeats(&creed));
    }

    #[test]
    fn test_has_timed_heartbeats_empty() {
        let creed = Creed::new("empty");
        assert!(!has_timed_heartbeats(&creed));
    }

    #[test]
    fn test_has_timed_heartbeats_mixed() {
        let creed = make_creed_with_heartbeats(&["every_bout", "every 5m"]);
        assert!(has_timed_heartbeats(&creed));
    }

    // -----------------------------------------------------------------------
    // Scheduler lifecycle tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_scheduler_monitoring_count_starts_at_zero() {
        let (tx, rx) = watch::channel(false);
        let sched = HeartbeatScheduler::with_shutdown(tx, rx);
        assert_eq!(sched.monitoring_count(), 0);
    }

    #[test]
    fn test_scheduler_is_monitoring_false_by_default() {
        let (tx, rx) = watch::channel(false);
        let sched = HeartbeatScheduler::with_shutdown(tx, rx);
        let id = FighterId::new();
        assert!(!sched.is_monitoring(&id));
    }
}
