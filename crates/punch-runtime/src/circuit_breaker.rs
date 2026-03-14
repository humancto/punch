//! Provider-level circuit breaker for LLM calls.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// The status of a circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitStatus {
    Closed,
    Open,
    HalfOpen,
}

/// Internal state for a single provider's circuit breaker.
#[derive(Debug)]
pub struct CircuitState {
    pub status: CircuitStatus,
    pub consecutive_failures: u64,
    pub last_failure: Option<Instant>,
    pub last_success: Option<Instant>,
}

impl Default for CircuitState {
    fn default() -> Self {
        Self {
            status: CircuitStatus::Closed,
            consecutive_failures: 0,
            last_failure: None,
            last_success: None,
        }
    }
}

/// A circuit breaker that tracks provider health across calls.
pub struct ProviderCircuitBreaker {
    states: DashMap<String, CircuitState>,
    failure_threshold: u64,
    cooldown: Duration,
}

impl ProviderCircuitBreaker {
    pub fn new() -> Self {
        Self {
            states: DashMap::new(),
            failure_threshold: 5,
            cooldown: Duration::from_secs(60),
        }
    }

    pub fn with_config(failure_threshold: u64, cooldown: Duration) -> Self {
        Self {
            states: DashMap::new(),
            failure_threshold,
            cooldown,
        }
    }

    pub fn should_allow(&self, provider: &str) -> bool {
        let entry = self.states.entry(provider.to_string()).or_default();
        let state = entry.value();

        match state.status {
            CircuitStatus::Closed | CircuitStatus::HalfOpen => true,
            CircuitStatus::Open => {
                if let Some(last_failure) = state.last_failure {
                    if last_failure.elapsed() >= self.cooldown {
                        drop(entry);
                        if let Some(mut s) = self.states.get_mut(provider) {
                            s.status = CircuitStatus::HalfOpen;
                            debug!(provider, "circuit breaker transitioning to half-open");
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    true
                }
            }
        }
    }

    pub fn record_success(&self, provider: &str) {
        let mut entry = self.states.entry(provider.to_string()).or_default();
        let state = entry.value_mut();
        state.consecutive_failures = 0;
        state.last_success = Some(Instant::now());
        if state.status != CircuitStatus::Closed {
            info!(provider, "circuit breaker closed after successful probe");
            state.status = CircuitStatus::Closed;
        }
    }

    pub fn record_failure(&self, provider: &str) {
        let mut entry = self.states.entry(provider.to_string()).or_default();
        let state = entry.value_mut();
        state.consecutive_failures += 1;
        state.last_failure = Some(Instant::now());

        if state.consecutive_failures >= self.failure_threshold
            && state.status != CircuitStatus::Open
        {
            warn!(
                provider,
                failures = state.consecutive_failures,
                "circuit breaker tripped"
            );
            state.status = CircuitStatus::Open;
        }
    }

    pub fn get_status(&self, provider: &str) -> CircuitStatus {
        self.states
            .get(provider)
            .map(|e| e.status)
            .unwrap_or(CircuitStatus::Closed)
    }
}

impl Default for ProviderCircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_circuit_is_closed() {
        let cb = ProviderCircuitBreaker::new();
        assert_eq!(cb.get_status("anthropic"), CircuitStatus::Closed);
        assert!(cb.should_allow("anthropic"));
    }

    #[test]
    fn trips_after_threshold() {
        let cb = ProviderCircuitBreaker::with_config(3, Duration::from_secs(60));
        for _ in 0..3 {
            cb.record_failure("test");
        }
        assert_eq!(cb.get_status("test"), CircuitStatus::Open);
        assert!(!cb.should_allow("test"));
    }

    #[test]
    fn resets_on_success() {
        let cb = ProviderCircuitBreaker::with_config(3, Duration::from_secs(60));
        cb.record_failure("test");
        cb.record_failure("test");
        cb.record_success("test");
        assert_eq!(cb.get_status("test"), CircuitStatus::Closed);
    }
}
