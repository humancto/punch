//! Gorilla Executor — the execution engine for autonomous gorilla tasks.
//!
//! Manages gorilla lifecycle transitions (Dormant → Rampaging → Resting → Dormant),
//! implements retry logic with exponential backoff, tracks execution history,
//! and provides a circuit breaker for runaway gorillas.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use punch_types::{GorillaId, GorillaManifest, GorillaStatus, PunchError, PunchResult};

use crate::GorillaOutput;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the gorilla executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    /// Maximum consecutive failures before the circuit breaker trips.
    pub circuit_breaker_threshold: u32,
    /// How long to wait before resetting the circuit breaker.
    pub circuit_breaker_reset: Duration,
    /// Maximum retry attempts per execution.
    pub max_retries: u32,
    /// Base delay for exponential backoff (doubles each retry).
    pub retry_base_delay: Duration,
    /// Maximum retry delay cap.
    pub retry_max_delay: Duration,
    /// Maximum execution time before a gorilla is killed.
    pub execution_timeout: Duration,
    /// Maximum execution history entries to keep per gorilla.
    pub max_history_entries: usize,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            circuit_breaker_threshold: 5,
            circuit_breaker_reset: Duration::from_secs(3600),
            max_retries: 3,
            retry_base_delay: Duration::from_secs(5),
            retry_max_delay: Duration::from_secs(300),
            execution_timeout: Duration::from_secs(600),
            max_history_entries: 100,
        }
    }
}

// ---------------------------------------------------------------------------
// Execution record
// ---------------------------------------------------------------------------

/// A record of a single gorilla execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// When the execution started.
    pub started_at: DateTime<Utc>,
    /// When the execution completed (or failed).
    pub completed_at: DateTime<Utc>,
    /// Whether the execution succeeded.
    pub success: bool,
    /// Duration of the execution.
    pub duration: Duration,
    /// Error message if the execution failed.
    pub error: Option<String>,
    /// Output summary if execution succeeded.
    pub summary: Option<String>,
    /// Number of retry attempts used.
    pub retries: u32,
}

// ---------------------------------------------------------------------------
// Circuit breaker state
// ---------------------------------------------------------------------------

/// Circuit breaker state for a gorilla.
#[derive(Debug, Clone)]
struct CircuitBreaker {
    /// Consecutive failure count.
    consecutive_failures: u32,
    /// Whether the circuit is open (tripped).
    is_open: bool,
    /// When the circuit breaker was last tripped.
    tripped_at: Option<DateTime<Utc>>,
}

impl CircuitBreaker {
    fn new() -> Self {
        Self {
            consecutive_failures: 0,
            is_open: false,
            tripped_at: None,
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.is_open = false;
        self.tripped_at = None;
    }

    fn record_failure(&mut self, threshold: u32) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= threshold {
            self.is_open = true;
            self.tripped_at = Some(Utc::now());
        }
    }

    fn should_allow(&self, reset_duration: Duration) -> bool {
        if !self.is_open {
            return true;
        }
        // Allow if enough time has passed since the trip.
        if let Some(tripped) = self.tripped_at {
            let elapsed = Utc::now() - tripped;
            if let Ok(reset_chrono) = chrono::Duration::from_std(reset_duration) {
                return elapsed > reset_chrono;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Per-gorilla execution state
// ---------------------------------------------------------------------------

/// Internal execution state for a single gorilla.
struct GorillaExecutionState {
    /// Current lifecycle status.
    status: GorillaStatus,
    /// Execution history (most recent first).
    history: VecDeque<ExecutionRecord>,
    /// Circuit breaker.
    circuit_breaker: CircuitBreaker,
    /// Total run count.
    run_count: u64,
    /// Total error count.
    error_count: u64,
    /// Last run time.
    last_run: Option<DateTime<Utc>>,
    /// The gorilla's manifest.
    manifest: GorillaManifest,
    /// Stored output/artifacts from the last successful run.
    last_output: Option<GorillaOutput>,
}

// ---------------------------------------------------------------------------
// GorillaExecutor
// ---------------------------------------------------------------------------

/// The gorilla execution engine.
///
/// Manages lifecycle, retries, circuit breaking, and execution history
/// for all registered gorillas.
pub struct GorillaExecutor {
    /// Per-gorilla execution state.
    states: DashMap<GorillaId, GorillaExecutionState>,
    /// Executor configuration.
    config: ExecutorConfig,
    /// Notification for state changes.
    notify: Arc<Notify>,
}

impl GorillaExecutor {
    /// Create a new gorilla executor with default configuration.
    pub fn new() -> Self {
        Self {
            states: DashMap::new(),
            config: ExecutorConfig::default(),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Create a new gorilla executor with custom configuration.
    pub fn with_config(config: ExecutorConfig) -> Self {
        Self {
            states: DashMap::new(),
            config,
            notify: Arc::new(Notify::new()),
        }
    }

    /// Register a gorilla with the executor.
    pub fn register(&self, gorilla_id: GorillaId, manifest: GorillaManifest) {
        let state = GorillaExecutionState {
            status: GorillaStatus::Caged,
            history: VecDeque::new(),
            circuit_breaker: CircuitBreaker::new(),
            run_count: 0,
            error_count: 0,
            last_run: None,
            manifest,
            last_output: None,
        };
        self.states.insert(gorilla_id, state);
        info!(gorilla_id = %gorilla_id, "gorilla registered with executor");
    }

    /// Unregister a gorilla from the executor.
    pub fn unregister(&self, gorilla_id: &GorillaId) {
        self.states.remove(gorilla_id);
    }

    /// Transition a gorilla to Unleashed (ready to run).
    pub fn unleash(&self, gorilla_id: &GorillaId) -> PunchResult<()> {
        let mut state = self
            .states
            .get_mut(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;
        state.status = GorillaStatus::Unleashed;
        info!(gorilla_id = %gorilla_id, "gorilla unleashed");
        Ok(())
    }

    /// Transition a gorilla to Caged (stopped).
    pub fn cage(&self, gorilla_id: &GorillaId) -> PunchResult<()> {
        let mut state = self
            .states
            .get_mut(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;
        state.status = GorillaStatus::Caged;
        info!(gorilla_id = %gorilla_id, "gorilla caged");
        Ok(())
    }

    /// Check if a gorilla is allowed to execute (circuit breaker check).
    pub fn can_execute(&self, gorilla_id: &GorillaId) -> PunchResult<bool> {
        let state = self
            .states
            .get(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;

        if state.status == GorillaStatus::Caged || state.status == GorillaStatus::Injured {
            return Ok(false);
        }

        Ok(state
            .circuit_breaker
            .should_allow(self.config.circuit_breaker_reset))
    }

    /// Begin execution of a gorilla (transition to Rampaging).
    ///
    /// Returns an error if the gorilla cannot execute (circuit breaker tripped,
    /// wrong status, etc.).
    pub fn begin_execution(&self, gorilla_id: &GorillaId) -> PunchResult<GorillaManifest> {
        let mut state = self
            .states
            .get_mut(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;

        if !state
            .circuit_breaker
            .should_allow(self.config.circuit_breaker_reset)
        {
            return Err(PunchError::Gorilla(format!(
                "gorilla {} circuit breaker is open ({} consecutive failures)",
                gorilla_id, state.circuit_breaker.consecutive_failures
            )));
        }

        if state.status == GorillaStatus::Injured {
            return Err(PunchError::Gorilla(format!(
                "gorilla {} is injured and cannot execute",
                gorilla_id
            )));
        }

        state.status = GorillaStatus::Rampaging;
        debug!(gorilla_id = %gorilla_id, "gorilla began rampaging");
        Ok(state.manifest.clone())
    }

    /// Record a successful execution.
    pub fn record_success(
        &self,
        gorilla_id: &GorillaId,
        started_at: DateTime<Utc>,
        output: GorillaOutput,
        retries: u32,
    ) -> PunchResult<()> {
        let mut state = self
            .states
            .get_mut(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;

        let now = Utc::now();
        let duration = (now - started_at)
            .to_std()
            .unwrap_or(Duration::from_secs(0));

        let record = ExecutionRecord {
            started_at,
            completed_at: now,
            success: true,
            duration,
            error: None,
            summary: Some(output.summary.clone()),
            retries,
        };

        state.history.push_front(record);
        if state.history.len() > self.config.max_history_entries {
            state.history.pop_back();
        }

        state.circuit_breaker.record_success();
        state.run_count += 1;
        state.last_run = Some(now);
        state.last_output = Some(output);
        state.status = GorillaStatus::Unleashed;

        info!(
            gorilla_id = %gorilla_id,
            run_count = state.run_count,
            duration_secs = duration.as_secs(),
            "gorilla execution succeeded"
        );
        Ok(())
    }

    /// Record a failed execution.
    pub fn record_failure(
        &self,
        gorilla_id: &GorillaId,
        started_at: DateTime<Utc>,
        error: &str,
        retries: u32,
    ) -> PunchResult<()> {
        let mut state = self
            .states
            .get_mut(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;

        let now = Utc::now();
        let duration = (now - started_at)
            .to_std()
            .unwrap_or(Duration::from_secs(0));

        let record = ExecutionRecord {
            started_at,
            completed_at: now,
            success: false,
            duration,
            error: Some(error.to_string()),
            summary: None,
            retries,
        };

        state.history.push_front(record);
        if state.history.len() > self.config.max_history_entries {
            state.history.pop_back();
        }

        state
            .circuit_breaker
            .record_failure(self.config.circuit_breaker_threshold);
        state.error_count += 1;
        state.last_run = Some(now);

        if state.circuit_breaker.is_open {
            state.status = GorillaStatus::Injured;
            warn!(
                gorilla_id = %gorilla_id,
                consecutive_failures = state.circuit_breaker.consecutive_failures,
                "gorilla circuit breaker tripped, marking as injured"
            );
        } else {
            state.status = GorillaStatus::Unleashed;
        }

        error!(
            gorilla_id = %gorilla_id,
            error = %error,
            error_count = state.error_count,
            retries,
            "gorilla execution failed"
        );
        Ok(())
    }

    /// Calculate the retry delay for a given attempt number using exponential backoff.
    pub fn retry_delay(&self, attempt: u32) -> Duration {
        let delay = self
            .config
            .retry_base_delay
            .saturating_mul(2u32.saturating_pow(attempt));
        delay.min(self.config.retry_max_delay)
    }

    /// Get the maximum number of retries.
    pub fn max_retries(&self) -> u32 {
        self.config.max_retries
    }

    /// Get the execution timeout.
    pub fn execution_timeout(&self) -> Duration {
        self.config.execution_timeout
    }

    /// Get the current status of a gorilla.
    pub fn get_status(&self, gorilla_id: &GorillaId) -> Option<GorillaStatus> {
        self.states.get(gorilla_id).map(|s| s.status)
    }

    /// Get execution history for a gorilla.
    pub fn get_history(&self, gorilla_id: &GorillaId) -> Option<Vec<ExecutionRecord>> {
        self.states
            .get(gorilla_id)
            .map(|s| s.history.iter().cloned().collect())
    }

    /// Get execution statistics for a gorilla.
    pub fn get_stats(&self, gorilla_id: &GorillaId) -> Option<ExecutionStats> {
        self.states.get(gorilla_id).map(|s| ExecutionStats {
            run_count: s.run_count,
            error_count: s.error_count,
            success_rate: if s.run_count + s.error_count == 0 {
                0.0
            } else {
                s.run_count as f64 / (s.run_count + s.error_count) as f64
            },
            last_run: s.last_run,
            circuit_breaker_open: s.circuit_breaker.is_open,
            consecutive_failures: s.circuit_breaker.consecutive_failures,
        })
    }

    /// Get the last output from a gorilla.
    pub fn get_last_output(&self, gorilla_id: &GorillaId) -> Option<GorillaOutput> {
        self.states
            .get(gorilla_id)
            .and_then(|s| s.last_output.clone())
    }

    /// Reset a gorilla's circuit breaker (e.g., after manual investigation).
    pub fn reset_circuit_breaker(&self, gorilla_id: &GorillaId) -> PunchResult<()> {
        let mut state = self
            .states
            .get_mut(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not found", gorilla_id)))?;
        state.circuit_breaker = CircuitBreaker::new();
        if state.status == GorillaStatus::Injured {
            state.status = GorillaStatus::Caged;
        }
        info!(gorilla_id = %gorilla_id, "circuit breaker reset");
        Ok(())
    }

    /// Get the notification handle.
    pub fn notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    /// List all registered gorilla IDs and their statuses.
    pub fn list(&self) -> Vec<(GorillaId, GorillaStatus)> {
        self.states.iter().map(|e| (*e.key(), e.status)).collect()
    }
}

impl Default for GorillaExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary statistics for a gorilla's execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStats {
    /// Total successful runs.
    pub run_count: u64,
    /// Total failed runs.
    pub error_count: u64,
    /// Success rate (0.0 - 1.0).
    pub success_rate: f64,
    /// Last run timestamp.
    pub last_run: Option<DateTime<Utc>>,
    /// Whether the circuit breaker is currently open.
    pub circuit_breaker_open: bool,
    /// Current consecutive failure count.
    pub consecutive_failures: u32,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    fn test_output() -> GorillaOutput {
        GorillaOutput {
            summary: "Test completed".to_string(),
            artifacts: vec!["artifact.txt".to_string()],
            next_run: None,
        }
    }

    #[test]
    fn executor_register_and_list() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));

        let list = executor.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, id);
        assert_eq!(list[0].1, GorillaStatus::Caged);
    }

    #[test]
    fn executor_unregister() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));
        executor.unregister(&id);
        assert!(executor.get_status(&id).is_none());
    }

    #[test]
    fn executor_unleash_and_cage() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));

        executor.unleash(&id).unwrap();
        assert_eq!(executor.get_status(&id), Some(GorillaStatus::Unleashed));

        executor.cage(&id).unwrap();
        assert_eq!(executor.get_status(&id), Some(GorillaStatus::Caged));
    }

    #[test]
    fn executor_unleash_nonexistent() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        assert!(executor.unleash(&id).is_err());
    }

    #[test]
    fn executor_can_execute_caged() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));
        assert!(!executor.can_execute(&id).unwrap());
    }

    #[test]
    fn executor_can_execute_unleashed() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));
        executor.unleash(&id).unwrap();
        assert!(executor.can_execute(&id).unwrap());
    }

    #[test]
    fn executor_begin_execution() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));
        executor.unleash(&id).unwrap();

        let manifest = executor.begin_execution(&id).unwrap();
        assert_eq!(manifest.name, "test");
        assert_eq!(executor.get_status(&id), Some(GorillaStatus::Rampaging));
    }

    #[test]
    fn executor_begin_execution_injured() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));

        // Force injured state.
        if let Some(mut state) = executor.states.get_mut(&id) {
            state.status = GorillaStatus::Injured;
        }

        assert!(executor.begin_execution(&id).is_err());
    }

    #[test]
    fn executor_record_success() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));
        executor.unleash(&id).unwrap();
        executor.begin_execution(&id).unwrap();

        let started = Utc::now() - chrono::Duration::seconds(5);
        executor
            .record_success(&id, started, test_output(), 0)
            .unwrap();

        assert_eq!(executor.get_status(&id), Some(GorillaStatus::Unleashed));
        let stats = executor.get_stats(&id).unwrap();
        assert_eq!(stats.run_count, 1);
        assert_eq!(stats.error_count, 0);
    }

    #[test]
    fn executor_record_failure() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));
        executor.unleash(&id).unwrap();
        executor.begin_execution(&id).unwrap();

        let started = Utc::now();
        executor
            .record_failure(&id, started, "test error", 0)
            .unwrap();

        let stats = executor.get_stats(&id).unwrap();
        assert_eq!(stats.error_count, 1);
    }

    #[test]
    fn executor_circuit_breaker_trips() {
        let config = ExecutorConfig {
            circuit_breaker_threshold: 3,
            ..Default::default()
        };
        let executor = GorillaExecutor::with_config(config);
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));
        executor.unleash(&id).unwrap();

        let started = Utc::now();
        for i in 0..3 {
            // Reset to rampaging for each failure.
            if let Some(mut state) = executor.states.get_mut(&id) {
                state.status = GorillaStatus::Rampaging;
            }
            executor
                .record_failure(&id, started, &format!("error {}", i), 0)
                .unwrap();
        }

        assert_eq!(executor.get_status(&id), Some(GorillaStatus::Injured));
        assert!(!executor.can_execute(&id).unwrap());

        let stats = executor.get_stats(&id).unwrap();
        assert!(stats.circuit_breaker_open);
        assert_eq!(stats.consecutive_failures, 3);
    }

    #[test]
    fn executor_reset_circuit_breaker() {
        let config = ExecutorConfig {
            circuit_breaker_threshold: 1,
            ..Default::default()
        };
        let executor = GorillaExecutor::with_config(config);
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));
        executor.unleash(&id).unwrap();

        if let Some(mut state) = executor.states.get_mut(&id) {
            state.status = GorillaStatus::Rampaging;
        }
        executor
            .record_failure(&id, Utc::now(), "error", 0)
            .unwrap();
        assert_eq!(executor.get_status(&id), Some(GorillaStatus::Injured));

        executor.reset_circuit_breaker(&id).unwrap();
        assert_eq!(executor.get_status(&id), Some(GorillaStatus::Caged));
        let stats = executor.get_stats(&id).unwrap();
        assert!(!stats.circuit_breaker_open);
    }

    #[test]
    fn executor_retry_delay_exponential() {
        let executor = GorillaExecutor::new();
        let d0 = executor.retry_delay(0);
        let d1 = executor.retry_delay(1);
        let d2 = executor.retry_delay(2);

        assert_eq!(d0, Duration::from_secs(5));
        assert_eq!(d1, Duration::from_secs(10));
        assert_eq!(d2, Duration::from_secs(20));
    }

    #[test]
    fn executor_retry_delay_capped() {
        let executor = GorillaExecutor::new();
        let d10 = executor.retry_delay(10);
        assert!(d10 <= executor.config.retry_max_delay);
    }

    #[test]
    fn executor_execution_history() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));
        executor.unleash(&id).unwrap();

        for i in 0..5 {
            if let Some(mut state) = executor.states.get_mut(&id) {
                state.status = GorillaStatus::Rampaging;
            }
            executor
                .record_success(
                    &id,
                    Utc::now(),
                    GorillaOutput {
                        summary: format!("run {}", i),
                        artifacts: Vec::new(),
                        next_run: None,
                    },
                    0,
                )
                .unwrap();
        }

        let history = executor.get_history(&id).unwrap();
        assert_eq!(history.len(), 5);
        // Most recent first.
        assert!(history[0].summary.as_deref() == Some("run 4"));
    }

    #[test]
    fn executor_history_capped() {
        let config = ExecutorConfig {
            max_history_entries: 3,
            ..Default::default()
        };
        let executor = GorillaExecutor::with_config(config);
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));

        for i in 0..5 {
            if let Some(mut state) = executor.states.get_mut(&id) {
                state.status = GorillaStatus::Rampaging;
            }
            executor
                .record_success(
                    &id,
                    Utc::now(),
                    GorillaOutput {
                        summary: format!("run {}", i),
                        artifacts: Vec::new(),
                        next_run: None,
                    },
                    0,
                )
                .unwrap();
        }

        let history = executor.get_history(&id).unwrap();
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn executor_get_last_output() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));

        assert!(executor.get_last_output(&id).is_none());

        if let Some(mut state) = executor.states.get_mut(&id) {
            state.status = GorillaStatus::Rampaging;
        }
        executor
            .record_success(&id, Utc::now(), test_output(), 0)
            .unwrap();

        let output = executor.get_last_output(&id).unwrap();
        assert_eq!(output.summary, "Test completed");
    }

    #[test]
    fn executor_stats_success_rate() {
        let executor = GorillaExecutor::new();
        let id = GorillaId::new();
        executor.register(id, test_manifest("test"));

        // 3 successes, 1 failure.
        for _ in 0..3 {
            if let Some(mut state) = executor.states.get_mut(&id) {
                state.status = GorillaStatus::Rampaging;
            }
            executor
                .record_success(&id, Utc::now(), test_output(), 0)
                .unwrap();
        }
        if let Some(mut state) = executor.states.get_mut(&id) {
            state.status = GorillaStatus::Rampaging;
        }
        executor.record_failure(&id, Utc::now(), "oops", 0).unwrap();

        let stats = executor.get_stats(&id).unwrap();
        assert_eq!(stats.run_count, 3);
        assert_eq!(stats.error_count, 1);
        assert!((stats.success_rate - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn executor_default() {
        let executor = GorillaExecutor::default();
        assert!(executor.list().is_empty());
    }

    #[test]
    fn circuit_breaker_new_allows() {
        let cb = CircuitBreaker::new();
        assert!(cb.should_allow(Duration::from_secs(60)));
    }

    #[test]
    fn circuit_breaker_success_resets() {
        let mut cb = CircuitBreaker::new();
        cb.record_failure(3);
        cb.record_failure(3);
        cb.record_success();
        assert_eq!(cb.consecutive_failures, 0);
        assert!(!cb.is_open);
    }

    #[test]
    fn executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.circuit_breaker_threshold, 5);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn execution_record_serialization() {
        let record = ExecutionRecord {
            started_at: Utc::now(),
            completed_at: Utc::now(),
            success: true,
            duration: Duration::from_secs(10),
            error: None,
            summary: Some("done".to_string()),
            retries: 0,
        };
        let json = serde_json::to_string(&record).unwrap();
        let deser: ExecutionRecord = serde_json::from_str(&json).unwrap();
        assert!(deser.success);
    }
}
