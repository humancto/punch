//! Shared HTTP connection pool for outbound LLM and tool calls.
//!
//! The [`HttpPool`] wraps a single [`reqwest::Client`] configured with
//! connection-pool settings so that all drivers share one pool instead of
//! each creating their own.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Configuration for the shared HTTP connection pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpPoolConfig {
    /// Maximum idle connections kept per host (default: 10).
    pub pool_max_idle_per_host: usize,
    /// How long an idle connection stays in the pool in seconds (default: 90).
    pub pool_idle_timeout_secs: u64,
    /// TCP connect timeout in seconds (default: 10).
    pub connect_timeout_secs: u64,
    /// Overall request timeout in seconds (default: 120).
    pub request_timeout_secs: u64,
    /// User-Agent header sent with every request (default: "Punch/0.1").
    pub user_agent: String,
}

impl Default for HttpPoolConfig {
    fn default() -> Self {
        Self {
            pool_max_idle_per_host: 10,
            pool_idle_timeout_secs: 90,
            connect_timeout_secs: 10,
            request_timeout_secs: 120,
            user_agent: "Punch/0.1".to_string(),
        }
    }
}

/// A shared HTTP connection pool backed by a single [`reqwest::Client`].
///
/// Create one of these at startup and pass the inner client to each driver
/// so they all share the same connection pool and timeout settings.
#[derive(Debug, Clone)]
pub struct HttpPool {
    client: reqwest::Client,
    config: HttpPoolConfig,
}

impl HttpPool {
    /// Build a new pool from the given configuration.
    pub fn new(config: HttpPoolConfig) -> Self {
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .pool_idle_timeout(Duration::from_secs(config.pool_idle_timeout_secs))
            .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .user_agent(&config.user_agent)
            .build()
            .unwrap_or_default();

        tracing::info!(
            pool_max_idle = config.pool_max_idle_per_host,
            idle_timeout_secs = config.pool_idle_timeout_secs,
            connect_timeout_secs = config.connect_timeout_secs,
            request_timeout_secs = config.request_timeout_secs,
            user_agent = %config.user_agent,
            "HTTP connection pool initialized"
        );

        Self { client, config }
    }

    /// Return a reference to the shared [`reqwest::Client`].
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Return a reference to the pool configuration.
    pub fn config(&self) -> &HttpPoolConfig {
        &self.config
    }
}

impl Default for HttpPool {
    fn default() -> Self {
        Self::new(HttpPoolConfig::default())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_creates_client_with_defaults() {
        let pool = HttpPool::default();
        let _client = pool.client();
    }

    #[test]
    fn pool_creates_client_with_custom_config() {
        let config = HttpPoolConfig {
            pool_max_idle_per_host: 20,
            pool_idle_timeout_secs: 30,
            connect_timeout_secs: 5,
            request_timeout_secs: 60,
            user_agent: "TestAgent/1.0".to_string(),
        };
        let pool = HttpPool::new(config);
        let _client = pool.client();
    }

    #[test]
    fn default_config_values_are_sensible() {
        let config = HttpPoolConfig::default();
        assert_eq!(config.pool_max_idle_per_host, 10);
        assert_eq!(config.pool_idle_timeout_secs, 90);
        assert_eq!(config.connect_timeout_secs, 10);
        assert_eq!(config.request_timeout_secs, 120);
        assert_eq!(config.user_agent, "Punch/0.1");
    }

    #[test]
    fn pool_is_clone() {
        let pool = HttpPool::default();
        let pool2 = pool.clone();
        // Both clones share the same underlying connection pool
        // (reqwest::Client uses Arc internally).
        let _c1 = pool.client();
        let _c2 = pool2.client();
    }

    #[test]
    fn custom_config_overrides_work() {
        let config = HttpPoolConfig {
            pool_max_idle_per_host: 1,
            pool_idle_timeout_secs: 1,
            connect_timeout_secs: 1,
            request_timeout_secs: 1,
            user_agent: "Custom/2.0".to_string(),
        };
        let pool = HttpPool::new(config.clone());
        assert_eq!(pool.config().pool_max_idle_per_host, 1);
        assert_eq!(pool.config().request_timeout_secs, 1);
        assert_eq!(pool.config().user_agent, "Custom/2.0");
    }

    #[test]
    fn config_accessor_returns_stored_values() {
        let pool = HttpPool::default();
        assert_eq!(pool.config().pool_max_idle_per_host, 10);
    }
}
