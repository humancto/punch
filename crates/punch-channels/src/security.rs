//! Channel security gateway — signature verification, allowlisting, rate limiting.
//!
//! Every inbound webhook request passes through this module BEFORE touching
//! any fighter or router state. This prevents the class of attacks that
//! plagued OpenClaw's ClawHub: webhook spoofing, unauthorized access, DoS.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Instant;

use dashmap::DashMap;
use tracing::{info, warn};

use punch_types::config::ChannelConfig;

/// Per-user rate limiter using a simple sliding-window token bucket.
struct UserBucket {
    /// Remaining tokens in the current window.
    tokens: u32,
    /// When the current window started.
    window_start: Instant,
}

/// Security gateway for a single channel.
///
/// Created once per channel at daemon startup and shared across requests.
pub struct ChannelGateway {
    /// Channel name (e.g. "telegram", "slack").
    pub channel_name: String,
    /// Webhook signing secret (resolved from env at startup).
    pub webhook_secret: Option<String>,
    /// Set of allowed user IDs. Empty = open access.
    allowed_users: HashSet<String>,
    /// Whether the allowlist is enforced (non-empty).
    allowlist_enforced: bool,
    /// Per-user rate limit buckets.
    rate_buckets: DashMap<String, Mutex<UserBucket>>,
    /// Max messages per user per minute.
    rate_limit: u32,
}

impl ChannelGateway {
    /// Create a gateway from channel configuration.
    ///
    /// Resolves the webhook secret from the environment variable specified
    /// in `webhook_secret_env`. Logs warnings for insecure configurations.
    pub fn from_config(channel_name: &str, config: &ChannelConfig) -> Self {
        let webhook_secret = config
            .webhook_secret_env
            .as_ref()
            .and_then(|env_var| std::env::var(env_var).ok().filter(|v| !v.is_empty()));

        if webhook_secret.is_none() {
            warn!(
                channel = %channel_name,
                "no webhook secret configured — webhook signature verification DISABLED"
            );
        }

        let allowed_users: HashSet<String> = config.allowed_user_ids.iter().cloned().collect();
        let allowlist_enforced = !allowed_users.is_empty();

        if !allowlist_enforced {
            warn!(
                channel = %channel_name,
                "no allowed_user_ids configured — ALL users can interact (open access)"
            );
        } else {
            info!(
                channel = %channel_name,
                allowed_count = allowed_users.len(),
                "user allowlist enforced"
            );
        }

        Self {
            channel_name: channel_name.to_string(),
            webhook_secret,
            allowed_users,
            allowlist_enforced,
            rate_buckets: DashMap::new(),
            rate_limit: config.rate_limit_per_user,
        }
    }

    /// Check if a user is allowed to send messages on this channel.
    ///
    /// Returns `Ok(())` if allowed, `Err(reason)` if denied.
    pub fn check_user_allowed(&self, user_id: &str) -> Result<(), String> {
        if !self.allowlist_enforced {
            return Ok(());
        }

        if self.allowed_users.contains(user_id) {
            Ok(())
        } else {
            warn!(
                channel = %self.channel_name,
                user_id = %user_id,
                "BLOCKED: user not in allowlist"
            );
            Err(format!(
                "User {} is not authorized on this channel",
                user_id
            ))
        }
    }

    /// Check per-user rate limit.
    ///
    /// Returns `Ok(())` if within limits, `Err(reason)` if rate-limited.
    pub fn check_rate_limit(&self, user_id: &str) -> Result<(), String> {
        if self.rate_limit == 0 {
            return Ok(()); // Rate limiting disabled.
        }

        let now = Instant::now();
        let window_duration = std::time::Duration::from_secs(60);

        let entry = self
            .rate_buckets
            .entry(user_id.to_string())
            .or_insert_with(|| {
                Mutex::new(UserBucket {
                    tokens: self.rate_limit,
                    window_start: now,
                })
            });

        let mut bucket = entry.lock().unwrap();

        // Reset window if expired.
        if now.duration_since(bucket.window_start) >= window_duration {
            bucket.tokens = self.rate_limit;
            bucket.window_start = now;
        }

        if bucket.tokens == 0 {
            warn!(
                channel = %self.channel_name,
                user_id = %user_id,
                limit = self.rate_limit,
                "RATE LIMITED: user exceeded per-minute limit"
            );
            return Err(format!(
                "Rate limited: max {} messages per minute",
                self.rate_limit
            ));
        }

        bucket.tokens -= 1;
        Ok(())
    }

    /// Run all security checks for an incoming message.
    ///
    /// Call this after signature verification but before routing.
    pub fn authorize_request(&self, user_id: &str) -> Result<(), String> {
        self.check_user_allowed(user_id)?;
        self.check_rate_limit(user_id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_config(allowed_ids: Vec<&str>, rate_limit: u32) -> ChannelConfig {
        ChannelConfig {
            channel_type: "telegram".to_string(),
            token_env: None,
            webhook_secret_env: None,
            allowed_user_ids: allowed_ids.into_iter().map(String::from).collect(),
            rate_limit_per_user: rate_limit,
            settings: HashMap::new(),
        }
    }

    #[test]
    fn test_open_access_allows_anyone() {
        let gw = ChannelGateway::from_config("test", &test_config(vec![], 20));
        assert!(gw.check_user_allowed("any_user").is_ok());
        assert!(gw.check_user_allowed("another_user").is_ok());
    }

    #[test]
    fn test_allowlist_blocks_unknown_users() {
        let gw = ChannelGateway::from_config("test", &test_config(vec!["user1", "user2"], 20));
        assert!(gw.check_user_allowed("user1").is_ok());
        assert!(gw.check_user_allowed("user2").is_ok());
        assert!(gw.check_user_allowed("user3").is_err());
        assert!(gw.check_user_allowed("attacker").is_err());
    }

    #[test]
    fn test_rate_limit_blocks_after_exhaustion() {
        let gw = ChannelGateway::from_config("test", &test_config(vec![], 3));

        assert!(gw.check_rate_limit("user1").is_ok()); // 1
        assert!(gw.check_rate_limit("user1").is_ok()); // 2
        assert!(gw.check_rate_limit("user1").is_ok()); // 3
        assert!(gw.check_rate_limit("user1").is_err()); // blocked

        // Different user is fine.
        assert!(gw.check_rate_limit("user2").is_ok());
    }

    #[test]
    fn test_rate_limit_zero_disables() {
        let gw = ChannelGateway::from_config("test", &test_config(vec![], 0));
        for _ in 0..100 {
            assert!(gw.check_rate_limit("user1").is_ok());
        }
    }

    #[test]
    fn test_authorize_request_checks_both() {
        let gw = ChannelGateway::from_config("test", &test_config(vec!["user1"], 2));

        // Allowed user, within rate limit.
        assert!(gw.authorize_request("user1").is_ok());
        assert!(gw.authorize_request("user1").is_ok());

        // Rate limited.
        assert!(gw.authorize_request("user1").is_err());

        // Unknown user blocked by allowlist (never hits rate limit).
        assert!(gw.authorize_request("attacker").is_err());
    }

    #[test]
    fn test_per_user_isolation() {
        let gw = ChannelGateway::from_config("test", &test_config(vec![], 2));

        // Each user gets their own bucket.
        assert!(gw.check_rate_limit("user1").is_ok());
        assert!(gw.check_rate_limit("user1").is_ok());
        assert!(gw.check_rate_limit("user1").is_err());

        assert!(gw.check_rate_limit("user2").is_ok());
        assert!(gw.check_rate_limit("user2").is_ok());
        assert!(gw.check_rate_limit("user2").is_err());
    }
}
