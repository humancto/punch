//! Provider health monitoring and circuit breaker failover.
//!
//! Every trainer (LLM provider) needs a corner team watching their vitals.
//! This module tracks each trainer's health — latency, error rates, availability —
//! and implements the circuit breaker pattern so the fight can continue even when
//! a trainer goes down. When a trainer is knocked out, the corner team calls in
//! the next available backup from the failover chain.

use std::collections::VecDeque;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::config::{ModelConfig, Provider};

// ---------------------------------------------------------------------------
// Health status
// ---------------------------------------------------------------------------

/// The current health status of a trainer (provider).
///
/// Like a ringside doctor's assessment — ranges from fighting fit to
/// pulled from the bout entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// Trainer is in peak condition — all systems nominal.
    Healthy,
    /// Trainer is showing signs of fatigue — elevated errors or latency.
    Degraded,
    /// Trainer is down for the count — circuit breaker has tripped.
    Unhealthy,
    /// No data yet — trainer hasn't entered the ring.
    Unknown,
}

// ---------------------------------------------------------------------------
// Provider health snapshot
// ---------------------------------------------------------------------------

/// Health snapshot for a single trainer (provider).
///
/// Captures the vital signs: latency percentiles, error rates, and
/// circuit breaker state. Think of it as the trainer's medical chart
/// between rounds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    /// Which trainer this health record belongs to.
    pub provider: Provider,
    /// Current health assessment from the corner team.
    pub status: HealthStatus,
    /// Median response latency in milliseconds (50th percentile).
    pub latency_p50_ms: f64,
    /// Tail latency in milliseconds (99th percentile).
    pub latency_p99_ms: f64,
    /// Error rate as a fraction from 0.0 (flawless) to 1.0 (total knockout).
    pub error_rate: f64,
    /// Total requests sent to this trainer.
    pub total_requests: u64,
    /// Total errors from this trainer.
    pub total_errors: u64,
    /// When the trainer last delivered a successful response.
    pub last_success: Option<DateTime<Utc>>,
    /// When the trainer last threw an error.
    pub last_error: Option<DateTime<Utc>>,
    /// The last error message from this trainer.
    pub last_error_message: Option<String>,
    /// How many errors in a row — a sign the trainer is fading.
    pub consecutive_errors: u32,
    /// Whether the circuit breaker is open (trainer pulled from the fight).
    pub circuit_open: bool,
    /// When the circuit breaker was tripped.
    pub circuit_opened_at: Option<DateTime<Utc>>,
}

impl ProviderHealth {
    /// Create a fresh health record for a trainer entering the ring.
    fn new(provider: Provider) -> Self {
        Self {
            provider,
            status: HealthStatus::Unknown,
            latency_p50_ms: 0.0,
            latency_p99_ms: 0.0,
            error_rate: 0.0,
            total_requests: 0,
            total_errors: 0,
            last_success: None,
            last_error: None,
            last_error_message: None,
            consecutive_errors: 0,
            circuit_open: false,
            circuit_opened_at: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Circuit breaker config
// ---------------------------------------------------------------------------

/// Configuration for the circuit breaker — the rules the corner team
/// uses to decide when to pull a trainer from the fight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Consecutive errors before the corner throws in the towel (opens circuit).
    pub error_threshold: u32,
    /// Seconds to wait before letting the trainer try a half-open round.
    pub recovery_timeout_secs: u64,
    /// Maximum requests allowed during the half-open trial round.
    pub half_open_max_requests: u32,
    /// Error rate (0.0–1.0) that triggers the circuit breaker.
    pub error_rate_threshold: f64,
    /// Minimum requests before error rate is considered meaningful.
    pub min_requests_for_rate: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            error_threshold: 5,
            recovery_timeout_secs: 60,
            half_open_max_requests: 3,
            error_rate_threshold: 0.5,
            min_requests_for_rate: 10,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal mutable state per provider
// ---------------------------------------------------------------------------

/// Internal tracking state for a single provider, kept behind a Mutex
/// inside the DashMap so we can mutate latency windows safely.
struct ProviderState {
    health: ProviderHealth,
    /// Rolling window of recent latencies for percentile calculation.
    latency_window: VecDeque<u64>,
    /// Counter of requests allowed through during half-open state.
    half_open_requests: u32,
}

impl ProviderState {
    fn new(provider: Provider) -> Self {
        Self {
            health: ProviderHealth::new(provider),
            latency_window: VecDeque::with_capacity(LATENCY_WINDOW_SIZE),
            half_open_requests: 0,
        }
    }
}

/// Maximum number of latency samples kept in the rolling window.
const LATENCY_WINDOW_SIZE: usize = 100;

// ---------------------------------------------------------------------------
// Provider health monitor
// ---------------------------------------------------------------------------

/// The corner team — monitors every trainer's health and manages failover.
///
/// Thread-safe by design: uses `DashMap` for concurrent provider tracking
/// and `Mutex` for internal mutable state within each provider entry.
/// When a trainer goes down, the corner team consults the failover chain
/// to find the next available backup.
pub struct ProviderHealthMonitor {
    /// Per-provider mutable state, keyed by provider display name.
    providers: DashMap<String, Mutex<ProviderState>>,
    /// Circuit breaker rules for the corner team.
    config: CircuitBreakerConfig,
    /// Ordered list of backup trainers to call when the primary goes down.
    failover_chain: Vec<(Provider, ModelConfig)>,
}

impl ProviderHealthMonitor {
    /// Assemble a new corner team with the given circuit breaker rules.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            providers: DashMap::new(),
            config,
            failover_chain: Vec::new(),
        }
    }

    /// Assemble a corner team with sensible default rules.
    pub fn with_defaults() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    /// Record a successful hit — the trainer delivered a clean response.
    ///
    /// Updates latency stats, resets consecutive error count, and may
    /// close a half-open circuit if the trainer is proving reliable again.
    pub fn record_success(&self, provider: &Provider, latency_ms: u64) {
        let key = provider.to_string();
        self.providers
            .entry(key)
            .or_insert_with(|| Mutex::new(ProviderState::new(provider.clone())));

        if let Some(ref_entry) = self.providers.get(&provider.to_string())
            && let Ok(mut state) = ref_entry.value().lock()
        {
            state.health.total_requests += 1;
            state.health.consecutive_errors = 0;
            state.health.last_success = Some(Utc::now());

            // Record latency in the rolling window.
            if state.latency_window.len() >= LATENCY_WINDOW_SIZE {
                state.latency_window.pop_front();
            }
            state.latency_window.push_back(latency_ms);

            // Recompute percentiles.
            Self::recompute_percentiles(&mut state);

            // Recompute error rate.
            Self::recompute_error_rate(&mut state);

            // If we were in half-open, a success moves us toward closing.
            if state.health.circuit_open {
                state.half_open_requests += 1;
                if state.half_open_requests >= self.config.half_open_max_requests {
                    // Trainer proved they can handle the load — close the circuit.
                    state.health.circuit_open = false;
                    state.health.circuit_opened_at = None;
                    state.half_open_requests = 0;
                }
            }

            // Update overall status.
            Self::update_status(&mut state, &self.config);
        }
    }

    /// Record a miss — the trainer threw an error.
    ///
    /// Increments error counters and may trip the circuit breaker if the
    /// trainer has taken too many consecutive hits.
    pub fn record_error(&self, provider: &Provider, error: &str) {
        let key = provider.to_string();
        self.providers
            .entry(key)
            .or_insert_with(|| Mutex::new(ProviderState::new(provider.clone())));

        if let Some(ref_entry) = self.providers.get(&provider.to_string())
            && let Ok(mut state) = ref_entry.value().lock()
        {
            state.health.total_requests += 1;
            state.health.total_errors += 1;
            state.health.consecutive_errors += 1;
            state.health.last_error = Some(Utc::now());
            state.health.last_error_message = Some(error.to_string());

            // Recompute error rate.
            Self::recompute_error_rate(&mut state);

            // Check if the circuit breaker should trip.
            let consecutive_tripped =
                state.health.consecutive_errors >= self.config.error_threshold;
            let rate_tripped = state.health.total_requests >= self.config.min_requests_for_rate
                && state.health.error_rate >= self.config.error_rate_threshold;

            if (consecutive_tripped || rate_tripped) && !state.health.circuit_open {
                state.health.circuit_open = true;
                state.health.circuit_opened_at = Some(Utc::now());
                state.half_open_requests = 0;
            }

            // If already in half-open and we get an error, re-open fully.
            if state.health.circuit_open && state.half_open_requests > 0 {
                state.health.circuit_opened_at = Some(Utc::now());
                state.half_open_requests = 0;
            }

            // Update overall status.
            Self::update_status(&mut state, &self.config);
        }
    }

    /// Get the current health snapshot for a trainer.
    pub fn get_health(&self, provider: &Provider) -> ProviderHealth {
        let key = provider.to_string();
        match self.providers.get(&key) {
            Some(entry) => match entry.value().lock() {
                Ok(state) => state.health.clone(),
                Err(_) => ProviderHealth::new(provider.clone()),
            },
            None => ProviderHealth::new(provider.clone()),
        }
    }

    /// Check if a trainer is available to take requests (circuit not open,
    /// or half-open and accepting trial requests).
    pub fn is_available(&self, provider: &Provider) -> bool {
        let key = provider.to_string();
        match self.providers.get(&key) {
            Some(entry) => match entry.value().lock() {
                Ok(state) => !state.health.circuit_open,
                Err(_) => false,
            },
            // Unknown provider is assumed available (no bad history).
            None => true,
        }
    }

    /// Get health snapshots for all tracked trainers.
    pub fn all_health(&self) -> Vec<ProviderHealth> {
        let mut results = Vec::new();
        for entry in self.providers.iter() {
            if let Ok(state) = entry.value().lock() {
                results.push(state.health.clone());
            }
        }
        results
    }

    /// Add a backup trainer to the failover chain — the corner team's
    /// bench of substitutes ready to step in.
    pub fn add_failover(&mut self, provider: Provider, config: ModelConfig) {
        self.failover_chain.push((provider, config));
    }

    /// Get the next available backup trainer from the failover chain.
    ///
    /// Skips any trainers whose circuits are currently open — no point
    /// sending a fighter to a corner that's already throwing in the towel.
    pub fn get_failover(&self, failed_provider: &Provider) -> Option<&(Provider, ModelConfig)> {
        self.failover_chain
            .iter()
            .find(|(p, _)| p != failed_provider && self.is_available(p))
    }

    /// Manually reset a trainer's circuit breaker — the ringside doctor
    /// clears them to fight again.
    pub fn reset_circuit(&self, provider: &Provider) {
        let key = provider.to_string();
        if let Some(entry) = self.providers.get(&key)
            && let Ok(mut state) = entry.value().lock()
        {
            state.health.circuit_open = false;
            state.health.circuit_opened_at = None;
            state.health.consecutive_errors = 0;
            state.half_open_requests = 0;
            Self::update_status(&mut state, &self.config);
        }
    }

    /// Check if a trainer's circuit should transition to half-open.
    ///
    /// After the recovery timeout expires, the corner team lets the trainer
    /// take a few trial punches to see if they've recovered.
    pub fn check_half_open(&self, provider: &Provider) -> bool {
        let key = provider.to_string();
        match self.providers.get(&key) {
            Some(entry) => match entry.value().lock() {
                Ok(state) => {
                    if !state.health.circuit_open {
                        return false;
                    }
                    match state.health.circuit_opened_at {
                        Some(opened_at) => {
                            let elapsed = Utc::now().signed_duration_since(opened_at).num_seconds();
                            elapsed >= self.config.recovery_timeout_secs as i64
                        }
                        None => false,
                    }
                }
                Err(_) => false,
            },
            None => false,
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Recompute latency percentiles from the rolling window.
    fn recompute_percentiles(state: &mut ProviderState) {
        if state.latency_window.is_empty() {
            state.health.latency_p50_ms = 0.0;
            state.health.latency_p99_ms = 0.0;
            return;
        }

        let mut sorted: Vec<u64> = state.latency_window.iter().copied().collect();
        sorted.sort_unstable();

        let len = sorted.len();
        let p50_idx = len / 2;
        state.health.latency_p50_ms = sorted[p50_idx] as f64;

        // For p99, use the element at the 99th percentile index.
        let p99_idx = ((len as f64) * 0.99).ceil() as usize;
        let p99_idx = if p99_idx >= len { len - 1 } else { p99_idx };
        state.health.latency_p99_ms = sorted[p99_idx] as f64;
    }

    /// Recompute error rate from total requests and errors.
    fn recompute_error_rate(state: &mut ProviderState) {
        if state.health.total_requests == 0 {
            state.health.error_rate = 0.0;
        } else {
            state.health.error_rate =
                state.health.total_errors as f64 / state.health.total_requests as f64;
        }
    }

    /// Update the overall health status based on current metrics.
    fn update_status(state: &mut ProviderState, config: &CircuitBreakerConfig) {
        if state.health.circuit_open {
            state.health.status = HealthStatus::Unhealthy;
        } else if state.health.total_requests == 0 {
            state.health.status = HealthStatus::Unknown;
        } else if state.health.total_requests >= config.min_requests_for_rate
            && state.health.error_rate >= config.error_rate_threshold * 0.5
        {
            // Error rate is elevated but not yet tripping the breaker.
            state.health.status = HealthStatus::Degraded;
        } else if state.health.consecutive_errors > 0 {
            state.health.status = HealthStatus::Degraded;
        } else {
            state.health.status = HealthStatus::Healthy;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Provider;

    fn test_provider() -> Provider {
        Provider::Anthropic
    }

    fn test_provider_b() -> Provider {
        Provider::OpenAI
    }

    fn test_model_config(provider: Provider) -> ModelConfig {
        ModelConfig {
            provider: provider.clone(),
            model: "test-model".to_string(),
            api_key_env: None,
            base_url: None,
            max_tokens: Some(1024),
            temperature: None,
        }
    }

    // 1. Record success updates stats
    #[test]
    fn record_success_updates_stats() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider = test_provider();

        monitor.record_success(&provider, 150);

        let health = monitor.get_health(&provider);
        assert_eq!(health.total_requests, 1);
        assert_eq!(health.total_errors, 0);
        assert_eq!(health.consecutive_errors, 0);
        assert!(health.last_success.is_some());
        assert_eq!(health.latency_p50_ms, 150.0);
    }

    // 2. Record error increments counters
    #[test]
    fn record_error_increments_counters() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider = test_provider();

        monitor.record_error(&provider, "timeout");

        let health = monitor.get_health(&provider);
        assert_eq!(health.total_requests, 1);
        assert_eq!(health.total_errors, 1);
        assert_eq!(health.consecutive_errors, 1);
        assert!(health.last_error.is_some());
        assert_eq!(health.last_error_message.as_deref(), Some("timeout"));
    }

    // 3. Consecutive errors opens circuit
    #[test]
    fn consecutive_errors_opens_circuit() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider = test_provider();

        // Default threshold is 5 consecutive errors.
        for i in 0..5 {
            monitor.record_error(&provider, &format!("error {}", i));
        }

        let health = monitor.get_health(&provider);
        assert!(
            health.circuit_open,
            "Circuit should be open after 5 consecutive errors"
        );
        assert!(health.circuit_opened_at.is_some());
        assert_eq!(health.status, HealthStatus::Unhealthy);
    }

    // 4. Circuit open blocks availability
    #[test]
    fn circuit_open_blocks_availability() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider = test_provider();

        // Before any errors, provider should be available.
        assert!(monitor.is_available(&provider));

        // Trip the circuit.
        for _ in 0..5 {
            monitor.record_error(&provider, "error");
        }

        assert!(
            !monitor.is_available(&provider),
            "Provider should not be available with open circuit"
        );
    }

    // 5. Recovery timeout triggers half-open check
    #[test]
    fn recovery_timeout_triggers_half_open() {
        let config = CircuitBreakerConfig {
            error_threshold: 2,
            recovery_timeout_secs: 0, // Immediate recovery for testing.
            ..Default::default()
        };
        let monitor = ProviderHealthMonitor::new(config);
        let provider = test_provider();

        // Trip the circuit.
        monitor.record_error(&provider, "error 1");
        monitor.record_error(&provider, "error 2");

        assert!(monitor.get_health(&provider).circuit_open);

        // With recovery_timeout_secs = 0, half-open should be true immediately.
        assert!(
            monitor.check_half_open(&provider),
            "Half-open check should pass after recovery timeout"
        );
    }

    // 6. Successful half-open closes circuit
    #[test]
    fn successful_half_open_closes_circuit() {
        let config = CircuitBreakerConfig {
            error_threshold: 2,
            half_open_max_requests: 2,
            recovery_timeout_secs: 0,
            ..Default::default()
        };
        let monitor = ProviderHealthMonitor::new(config);
        let provider = test_provider();

        // Trip the circuit.
        monitor.record_error(&provider, "e1");
        monitor.record_error(&provider, "e2");
        assert!(monitor.get_health(&provider).circuit_open);

        // Send successful requests through the half-open circuit.
        monitor.record_success(&provider, 100);
        // After 1 success, circuit should still be open (need 2).
        assert!(monitor.get_health(&provider).circuit_open);

        monitor.record_success(&provider, 100);
        // After 2 successes (matching half_open_max_requests), circuit closes.
        assert!(
            !monitor.get_health(&provider).circuit_open,
            "Circuit should close after enough half-open successes"
        );
    }

    // 7. Failed half-open re-opens circuit
    #[test]
    fn failed_half_open_reopens_circuit() {
        let config = CircuitBreakerConfig {
            error_threshold: 2,
            half_open_max_requests: 3,
            recovery_timeout_secs: 0,
            ..Default::default()
        };
        let monitor = ProviderHealthMonitor::new(config);
        let provider = test_provider();

        // Trip the circuit.
        monitor.record_error(&provider, "e1");
        monitor.record_error(&provider, "e2");
        assert!(monitor.get_health(&provider).circuit_open);

        // One success in half-open.
        monitor.record_success(&provider, 100);
        assert!(monitor.get_health(&provider).circuit_open);

        // Then an error — should re-open the circuit fully.
        monitor.record_error(&provider, "half-open fail");
        let health = monitor.get_health(&provider);
        assert!(
            health.circuit_open,
            "Circuit should re-open after half-open failure"
        );
        assert!(health.circuit_opened_at.is_some());
    }

    // 8. Error rate tracking
    #[test]
    fn error_rate_tracking() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider = test_provider();

        // 3 successes, 2 errors = 40% error rate.
        monitor.record_success(&provider, 100);
        monitor.record_success(&provider, 100);
        monitor.record_success(&provider, 100);
        monitor.record_error(&provider, "e1");
        monitor.record_error(&provider, "e2");

        let health = monitor.get_health(&provider);
        assert_eq!(health.total_requests, 5);
        assert_eq!(health.total_errors, 2);
        let expected_rate = 2.0 / 5.0;
        assert!(
            (health.error_rate - expected_rate).abs() < f64::EPSILON,
            "Error rate should be {}, got {}",
            expected_rate,
            health.error_rate
        );
    }

    // 9. Latency percentile tracking (approximate)
    #[test]
    fn latency_percentile_tracking() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider = test_provider();

        // Record latencies from 1 to 100.
        for i in 1..=100 {
            monitor.record_success(&provider, i);
        }

        let health = monitor.get_health(&provider);
        // p50 should be around 50 (index 50 of 100 sorted values).
        assert!(
            (health.latency_p50_ms - 51.0).abs() < 2.0,
            "p50 should be ~51, got {}",
            health.latency_p50_ms
        );
        // p99 should be near 100.
        assert!(
            health.latency_p99_ms >= 99.0,
            "p99 should be >= 99, got {}",
            health.latency_p99_ms
        );
    }

    // 10. Failover chain returns next provider
    #[test]
    fn failover_chain_returns_next_provider() {
        let mut monitor = ProviderHealthMonitor::with_defaults();
        let primary = test_provider();
        let backup = test_provider_b();
        let backup_config = test_model_config(backup.clone());

        monitor.add_failover(backup.clone(), backup_config);

        let failover = monitor.get_failover(&primary);
        assert!(
            failover.is_some(),
            "Should return a failover for a different provider"
        );
        assert_eq!(failover.map(|(p, _)| p), Some(&backup));
    }

    // 11. Failover skips unhealthy providers
    #[test]
    fn failover_skips_unhealthy_providers() {
        let mut monitor = ProviderHealthMonitor::with_defaults();
        let primary = test_provider();
        let backup_a = test_provider_b();
        let backup_b = Provider::Groq;

        monitor.add_failover(backup_a.clone(), test_model_config(backup_a.clone()));
        monitor.add_failover(backup_b.clone(), test_model_config(backup_b.clone()));

        // Make backup_a unhealthy by tripping its circuit.
        for _ in 0..5 {
            monitor.record_error(&backup_a, "down");
        }
        assert!(!monitor.is_available(&backup_a));

        // Failover should skip backup_a and return backup_b.
        let failover = monitor.get_failover(&primary);
        assert!(failover.is_some(), "Should find a healthy failover");
        assert_eq!(failover.map(|(p, _)| p), Some(&backup_b));
    }

    // 12. Manual circuit reset
    #[test]
    fn manual_circuit_reset() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider = test_provider();

        // Trip the circuit.
        for _ in 0..5 {
            monitor.record_error(&provider, "error");
        }
        assert!(monitor.get_health(&provider).circuit_open);
        assert!(!monitor.is_available(&provider));

        // Reset the circuit manually — the ringside doctor clears them.
        monitor.reset_circuit(&provider);

        let health = monitor.get_health(&provider);
        assert!(!health.circuit_open, "Circuit should be closed after reset");
        assert!(health.circuit_opened_at.is_none());
        assert_eq!(health.consecutive_errors, 0);
        assert!(monitor.is_available(&provider));
    }

    // 13. Default config has sensible values
    #[test]
    fn default_config_has_sensible_values() {
        let config = CircuitBreakerConfig::default();
        assert_eq!(config.error_threshold, 5);
        assert_eq!(config.recovery_timeout_secs, 60);
        assert_eq!(config.half_open_max_requests, 3);
        assert!((config.error_rate_threshold - 0.5).abs() < f64::EPSILON);
        assert_eq!(config.min_requests_for_rate, 10);
    }

    // 14. Multiple providers tracked independently
    #[test]
    fn multiple_providers_tracked_independently() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider_a = test_provider();
        let provider_b = test_provider_b();

        monitor.record_success(&provider_a, 100);
        monitor.record_success(&provider_a, 200);
        monitor.record_error(&provider_b, "timeout");

        let health_a = monitor.get_health(&provider_a);
        let health_b = monitor.get_health(&provider_b);

        assert_eq!(health_a.total_requests, 2);
        assert_eq!(health_a.total_errors, 0);
        assert_eq!(health_b.total_requests, 1);
        assert_eq!(health_b.total_errors, 1);

        // All health should return both.
        let all = monitor.all_health();
        assert_eq!(all.len(), 2);
    }

    // 15. Error rate threshold triggers circuit breaker
    #[test]
    fn error_rate_threshold_triggers_circuit() {
        let config = CircuitBreakerConfig {
            error_threshold: 100, // High consecutive threshold so only rate triggers.
            error_rate_threshold: 0.5,
            min_requests_for_rate: 4,
            ..Default::default()
        };
        let monitor = ProviderHealthMonitor::new(config);
        let provider = test_provider();

        // 2 successes then 2 errors = 50% error rate at 4 requests.
        monitor.record_success(&provider, 100);
        monitor.record_success(&provider, 100);
        monitor.record_error(&provider, "e1");
        // At 3 requests, below min_requests_for_rate — should not trip.
        assert!(!monitor.get_health(&provider).circuit_open);

        monitor.record_error(&provider, "e2");
        // At 4 requests with 50% error rate — should trip.
        assert!(
            monitor.get_health(&provider).circuit_open,
            "Circuit should open when error rate hits threshold"
        );
    }

    // 16. Unknown provider is available by default
    #[test]
    fn unknown_provider_is_available() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider = Provider::Mistral;
        assert!(monitor.is_available(&provider));
    }

    // 17. Health status transitions correctly
    #[test]
    fn health_status_transitions() {
        let monitor = ProviderHealthMonitor::with_defaults();
        let provider = test_provider();

        // Initially unknown.
        assert_eq!(monitor.get_health(&provider).status, HealthStatus::Unknown);

        // After success, healthy.
        monitor.record_success(&provider, 100);
        assert_eq!(monitor.get_health(&provider).status, HealthStatus::Healthy);

        // After an error, degraded (has consecutive_errors > 0).
        monitor.record_error(&provider, "oops");
        assert_eq!(monitor.get_health(&provider).status, HealthStatus::Degraded);

        // After success, back to healthy.
        monitor.record_success(&provider, 100);
        assert_eq!(monitor.get_health(&provider).status, HealthStatus::Healthy);
    }
}
