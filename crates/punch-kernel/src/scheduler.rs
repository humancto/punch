//! Per-fighter quota tracking and rate limiting.
//!
//! The [`Scheduler`] enforces tokens-per-hour and messages-per-hour limits on a
//! per-fighter basis. Quota windows are sliding and old entries are periodically
//! cleaned up.

use std::collections::VecDeque;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use tracing::{debug, info, instrument};

use punch_types::FighterId;

/// A single recorded usage event within a quota window.
#[derive(Debug, Clone)]
struct UsageRecord {
    /// When this usage was recorded.
    timestamp: DateTime<Utc>,
    /// Number of tokens consumed (0 for message-only records).
    tokens: u64,
}

/// Per-fighter usage tracking.
#[derive(Debug)]
struct FighterQuota {
    /// Rolling window of usage records.
    records: VecDeque<UsageRecord>,
}

impl FighterQuota {
    fn new() -> Self {
        Self {
            records: VecDeque::new(),
        }
    }

    /// Remove records older than `window`.
    fn evict_before(&mut self, cutoff: DateTime<Utc>) {
        while let Some(front) = self.records.front() {
            if front.timestamp < cutoff {
                self.records.pop_front();
            } else {
                break;
            }
        }
    }

    /// Sum of tokens in the current window.
    fn tokens_in_window(&self, cutoff: DateTime<Utc>) -> u64 {
        self.records
            .iter()
            .filter(|r| r.timestamp >= cutoff)
            .map(|r| r.tokens)
            .sum()
    }

    /// Number of messages (records) in the current window.
    fn messages_in_window(&self, cutoff: DateTime<Utc>) -> usize {
        self.records
            .iter()
            .filter(|r| r.timestamp >= cutoff)
            .count()
    }
}

/// Quota configuration.
#[derive(Debug, Clone)]
pub struct QuotaConfig {
    /// Maximum tokens a single fighter may consume per hour.
    pub tokens_per_hour: u64,
    /// Maximum messages a single fighter may send per hour.
    pub messages_per_hour: u64,
    /// Length of the sliding window.
    pub window: Duration,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            tokens_per_hour: 1_000_000,
            messages_per_hour: 500,
            window: Duration::hours(1),
        }
    }
}

/// Agent scheduler that enforces per-fighter quotas.
///
/// Thread-safe: all internal state is behind `DashMap` entries with inner
/// `Mutex` guards so the `Scheduler` can be shared via `Arc`.
pub struct Scheduler {
    quotas: DashMap<FighterId, Mutex<FighterQuota>>,
    config: QuotaConfig,
}

impl Scheduler {
    /// Create a new scheduler with the given quota configuration.
    pub fn new(config: QuotaConfig) -> Self {
        Self {
            quotas: DashMap::new(),
            config,
        }
    }

    /// Check whether `fighter_id` is within its quota limits.
    ///
    /// Returns `true` if the fighter may proceed, `false` if it has been
    /// rate-limited.
    #[instrument(skip(self), fields(%fighter_id))]
    pub fn check_quota(&self, fighter_id: &FighterId) -> bool {
        let now = Utc::now();
        let cutoff = now - self.config.window;

        let entry = self
            .quotas
            .entry(*fighter_id)
            .or_insert_with(|| Mutex::new(FighterQuota::new()));
        let quota = entry.value().lock().expect("quota lock poisoned");

        let tokens = quota.tokens_in_window(cutoff);
        let messages = quota.messages_in_window(cutoff);

        let within_limits = tokens < self.config.tokens_per_hour
            && messages < self.config.messages_per_hour as usize;

        if !within_limits {
            debug!(
                tokens,
                messages,
                tokens_limit = self.config.tokens_per_hour,
                messages_limit = self.config.messages_per_hour,
                "fighter quota exceeded"
            );
        }

        within_limits
    }

    /// Record token usage for a fighter.
    #[instrument(skip(self), fields(%fighter_id, tokens))]
    pub fn record_usage(&self, fighter_id: &FighterId, tokens: u64) {
        let entry = self
            .quotas
            .entry(*fighter_id)
            .or_insert_with(|| Mutex::new(FighterQuota::new()));
        let mut quota = entry.value().lock().expect("quota lock poisoned");

        quota.records.push_back(UsageRecord {
            timestamp: Utc::now(),
            tokens,
        });

        debug!("usage recorded");
    }

    /// Evict stale records outside the sliding window for all tracked fighters.
    ///
    /// Call this periodically (e.g. every few minutes) to keep memory bounded.
    #[instrument(skip(self))]
    pub fn cleanup(&self) {
        let cutoff = Utc::now() - self.config.window;
        let mut cleaned = 0usize;

        self.quotas.iter().for_each(|entry| {
            let mut quota = entry.value().lock().expect("quota lock poisoned");
            let before = quota.records.len();
            quota.evict_before(cutoff);
            cleaned += before - quota.records.len();
        });

        // Remove fighters that have no records left.
        self.quotas.retain(|_, v| {
            let quota = v.get_mut().expect("quota lock poisoned");
            !quota.records.is_empty()
        });

        info!(evicted_records = cleaned, "scheduler cleanup complete");
    }

    /// Remove all quota state for a given fighter (e.g. when the fighter is killed).
    pub fn remove_fighter(&self, fighter_id: &FighterId) {
        self.quotas.remove(fighter_id);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> QuotaConfig {
        QuotaConfig {
            tokens_per_hour: 1000,
            messages_per_hour: 5,
            window: Duration::hours(1),
        }
    }

    #[test]
    fn fresh_fighter_within_quota() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();
        assert!(scheduler.check_quota(&id));
    }

    #[test]
    fn token_quota_exceeded() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();

        // Record usage just at the limit.
        scheduler.record_usage(&id, 1000);
        assert!(!scheduler.check_quota(&id));
    }

    #[test]
    fn message_quota_exceeded() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();

        for _ in 0..5 {
            scheduler.record_usage(&id, 0);
        }

        assert!(!scheduler.check_quota(&id));
    }

    #[test]
    fn cleanup_removes_stale_entries() {
        let config = QuotaConfig {
            tokens_per_hour: 1000,
            messages_per_hour: 100,
            window: Duration::zero(),
        };
        let scheduler = Scheduler::new(config);
        let id = FighterId::new();

        scheduler.record_usage(&id, 500);
        // With a zero-width window every record is immediately stale.
        scheduler.cleanup();

        // Fighter entry should have been removed entirely.
        assert!(scheduler.check_quota(&id));
    }

    #[test]
    fn remove_fighter_clears_quota() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();

        scheduler.record_usage(&id, 999);
        scheduler.remove_fighter(&id);

        // After removal the fighter starts fresh.
        assert!(scheduler.check_quota(&id));
    }

    #[test]
    fn independent_fighters_have_separate_quotas() {
        let scheduler = Scheduler::new(test_config());
        let a = FighterId::new();
        let b = FighterId::new();

        scheduler.record_usage(&a, 1000);

        assert!(!scheduler.check_quota(&a));
        assert!(scheduler.check_quota(&b));
    }

    #[test]
    fn default_quota_config() {
        let config = QuotaConfig::default();
        assert_eq!(config.tokens_per_hour, 1_000_000);
        assert_eq!(config.messages_per_hour, 500);
    }

    #[test]
    fn check_quota_creates_entry_if_missing() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();
        // First check should create entry and return true (within quota).
        assert!(scheduler.check_quota(&id));
        // Second check should still work.
        assert!(scheduler.check_quota(&id));
    }

    #[test]
    fn token_quota_just_under_limit_passes() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();

        scheduler.record_usage(&id, 999);
        assert!(scheduler.check_quota(&id));
    }

    #[test]
    fn message_quota_just_under_limit_passes() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();

        for _ in 0..4 {
            scheduler.record_usage(&id, 0);
        }

        assert!(scheduler.check_quota(&id));
    }

    #[test]
    fn both_quotas_can_exceed_independently() {
        // Exceed only tokens.
        let scheduler = Scheduler::new(test_config());
        let id_tok = FighterId::new();
        scheduler.record_usage(&id_tok, 1001);
        assert!(!scheduler.check_quota(&id_tok));

        // Exceed only messages (with zero tokens each).
        let scheduler2 = Scheduler::new(test_config());
        let id_msg = FighterId::new();
        for _ in 0..5 {
            scheduler2.record_usage(&id_msg, 0);
        }
        assert!(!scheduler2.check_quota(&id_msg));
    }

    #[test]
    fn remove_fighter_allows_reuse_of_quota() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();

        scheduler.record_usage(&id, 1000);
        assert!(!scheduler.check_quota(&id));

        scheduler.remove_fighter(&id);
        assert!(scheduler.check_quota(&id));

        // Can record usage again.
        scheduler.record_usage(&id, 500);
        assert!(scheduler.check_quota(&id));
    }

    #[test]
    fn cleanup_with_no_entries_does_not_panic() {
        let scheduler = Scheduler::new(test_config());
        scheduler.cleanup();
    }

    #[test]
    fn multiple_fighters_cleanup() {
        let config = QuotaConfig {
            tokens_per_hour: 1000,
            messages_per_hour: 100,
            window: Duration::zero(),
        };
        let scheduler = Scheduler::new(config);

        let ids: Vec<FighterId> = (0..5).map(|_| FighterId::new()).collect();
        for id in &ids {
            scheduler.record_usage(id, 100);
        }

        scheduler.cleanup();

        // All should be cleaned up since window is zero.
        for id in &ids {
            assert!(scheduler.check_quota(id));
        }
    }

    #[test]
    fn concurrent_quota_checks_are_safe() {
        use std::sync::Arc;
        use std::thread;

        // Use generous limits so concurrent access doesn't exceed them.
        let config = QuotaConfig {
            tokens_per_hour: 1_000_000,
            messages_per_hour: 1_000,
            window: Duration::hours(1),
        };
        let scheduler = Arc::new(Scheduler::new(config));
        let id = FighterId::new();

        let mut handles = Vec::new();
        for _ in 0..10 {
            let sched = Arc::clone(&scheduler);
            let fid = id;
            handles.push(thread::spawn(move || {
                sched.record_usage(&fid, 10);
                sched.check_quota(&fid);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // Should still be within quota (10 records of 10 tokens = 100 tokens, 10 messages).
        assert!(scheduler.check_quota(&id));
    }

    #[test]
    fn remove_nonexistent_fighter_does_not_panic() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();
        scheduler.remove_fighter(&id); // Should not panic.
    }

    #[test]
    fn accumulating_usage_reaches_limit() {
        let scheduler = Scheduler::new(test_config());
        let id = FighterId::new();

        // Record 200 tokens 5 times = 1000, which should hit the limit.
        for _ in 0..5 {
            scheduler.record_usage(&id, 200);
        }
        assert!(!scheduler.check_quota(&id));
    }
}
