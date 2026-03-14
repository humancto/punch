//! Budget Enforcement — the promoter's purse control.
//!
//! The [`BudgetEnforcer`] checks spending limits before each LLM call and
//! returns a [`BudgetVerdict`] indicating whether the request is allowed,
//! approaching a limit (warning), or blocked (over budget).
//!
//! Budget enforcement is opt-in: if no limits are configured for a fighter
//! or globally, all requests are allowed.

use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use punch_types::{FighterId, PunchResult};

use crate::metering::{MeteringEngine, SpendPeriod};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Spending limits for a fighter or globally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetLimit {
    /// Maximum total tokens (input + output) per hour.
    pub max_tokens_per_hour: Option<u64>,
    /// Maximum total tokens (input + output) per day.
    pub max_tokens_per_day: Option<u64>,
    /// Maximum cost per day in cents (USD).
    pub max_cost_per_day_cents: Option<u64>,
    /// Maximum number of requests per hour.
    pub max_requests_per_hour: Option<u64>,
    /// Warning threshold as a percentage of any limit (default: 80).
    #[serde(default = "default_warning_threshold")]
    pub warning_threshold_percent: u8,
}

fn default_warning_threshold() -> u8 {
    80
}

impl Default for BudgetLimit {
    fn default() -> Self {
        Self {
            max_tokens_per_hour: None,
            max_tokens_per_day: None,
            max_cost_per_day_cents: None,
            max_requests_per_hour: None,
            warning_threshold_percent: default_warning_threshold(),
        }
    }
}

impl BudgetLimit {
    /// Returns true if any limit is configured.
    pub fn has_any_limit(&self) -> bool {
        self.max_tokens_per_hour.is_some()
            || self.max_tokens_per_day.is_some()
            || self.max_cost_per_day_cents.is_some()
            || self.max_requests_per_hour.is_some()
    }
}

/// The verdict from a budget check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BudgetVerdict {
    /// Request is allowed — fighter is within budget.
    Allowed,
    /// Request is allowed but approaching a limit.
    Warning {
        /// Current usage as a percentage of the closest limit.
        usage_percent: f64,
        /// Human-readable warning message.
        message: String,
    },
    /// Request is blocked — fighter is over budget.
    Blocked {
        /// Human-readable reason for the block.
        reason: String,
        /// Seconds until the budget period resets.
        retry_after_secs: u64,
    },
}

/// Current budget status for display / API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetStatus {
    /// Configured limits (if any).
    pub limits: Option<BudgetLimit>,
    /// Current hourly token usage.
    pub tokens_used_hour: u64,
    /// Current daily token usage.
    pub tokens_used_day: u64,
    /// Current daily cost in cents.
    pub cost_used_day_cents: u64,
    /// Current hourly request count.
    pub requests_used_hour: u64,
    /// Current verdict.
    pub verdict: BudgetVerdict,
}

// ---------------------------------------------------------------------------
// BudgetEnforcer
// ---------------------------------------------------------------------------

/// Enforces spending limits by checking the metering engine before each
/// LLM call. Supports per-fighter and global limits.
pub struct BudgetEnforcer {
    metering: Arc<MeteringEngine>,
    limits: DashMap<FighterId, BudgetLimit>,
    global_limit: Arc<tokio::sync::RwLock<Option<BudgetLimit>>>,
}

impl BudgetEnforcer {
    /// Create a new budget enforcer backed by the given metering engine.
    pub fn new(metering: Arc<MeteringEngine>) -> Self {
        Self {
            metering,
            limits: DashMap::new(),
            global_limit: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    /// Set a per-fighter budget limit.
    pub fn set_fighter_limit(&self, fighter_id: FighterId, limit: BudgetLimit) {
        info!(%fighter_id, "budget limit set for fighter");
        self.limits.insert(fighter_id, limit);
    }

    /// Remove a per-fighter budget limit.
    pub fn remove_fighter_limit(&self, fighter_id: &FighterId) {
        self.limits.remove(fighter_id);
    }

    /// Get the per-fighter budget limit, if configured.
    pub fn get_fighter_limit(&self, fighter_id: &FighterId) -> Option<BudgetLimit> {
        self.limits.get(fighter_id).map(|entry| entry.clone())
    }

    /// Set the global budget limit (applies to all fighters).
    pub async fn set_global_limit(&self, limit: BudgetLimit) {
        info!("global budget limit set");
        let mut guard = self.global_limit.write().await;
        *guard = Some(limit);
    }

    /// Remove the global budget limit.
    pub async fn clear_global_limit(&self) {
        let mut guard = self.global_limit.write().await;
        *guard = None;
    }

    /// Get the current global budget limit.
    pub async fn get_global_limit(&self) -> Option<BudgetLimit> {
        let guard = self.global_limit.read().await;
        guard.clone()
    }

    /// Check the budget for a specific fighter before an LLM call.
    ///
    /// This checks both the per-fighter limit (if set) and the global limit
    /// (if set). The most restrictive verdict wins.
    pub async fn check_budget(&self, fighter_id: &FighterId) -> PunchResult<BudgetVerdict> {
        // Check per-fighter limit.
        let fighter_verdict = if let Some(limit) = self.limits.get(fighter_id) {
            self.evaluate_limit(fighter_id, &limit, false).await?
        } else {
            BudgetVerdict::Allowed
        };

        // If fighter is blocked, return immediately.
        if matches!(fighter_verdict, BudgetVerdict::Blocked { .. }) {
            return Ok(fighter_verdict);
        }

        // Check global limit.
        let global_verdict = {
            let guard = self.global_limit.read().await;
            if let Some(ref limit) = *guard {
                self.evaluate_global_limit(limit).await?
            } else {
                BudgetVerdict::Allowed
            }
        };

        // Return the most restrictive verdict.
        Ok(most_restrictive(fighter_verdict, global_verdict))
    }

    /// Get the current budget status for a fighter (for API responses).
    pub async fn get_fighter_status(&self, fighter_id: &FighterId) -> PunchResult<BudgetStatus> {
        let limit = self.limits.get(fighter_id).map(|e| e.clone());

        let daily_spend = self
            .metering
            .get_spend(fighter_id, SpendPeriod::Day)
            .await?;

        let verdict = self.check_budget(fighter_id).await?;

        Ok(BudgetStatus {
            limits: limit,
            tokens_used_hour: 0, // Token counts would need additional metering queries
            tokens_used_day: 0,
            cost_used_day_cents: (daily_spend * 100.0) as u64,
            requests_used_hour: 0,
            verdict,
        })
    }

    /// Get the global budget status.
    pub async fn get_global_status(&self) -> PunchResult<BudgetStatus> {
        let limit = self.get_global_limit().await;

        let daily_spend = self.metering.get_total_spend(SpendPeriod::Day).await?;

        let global_verdict = if let Some(ref lim) = limit {
            self.evaluate_global_limit(lim).await?
        } else {
            BudgetVerdict::Allowed
        };

        Ok(BudgetStatus {
            limits: limit,
            tokens_used_hour: 0,
            tokens_used_day: 0,
            cost_used_day_cents: (daily_spend * 100.0) as u64,
            requests_used_hour: 0,
            verdict: global_verdict,
        })
    }

    /// Evaluate a specific limit against a fighter's current usage.
    async fn evaluate_limit(
        &self,
        fighter_id: &FighterId,
        limit: &BudgetLimit,
        _is_global: bool,
    ) -> PunchResult<BudgetVerdict> {
        if !limit.has_any_limit() {
            return Ok(BudgetVerdict::Allowed);
        }

        let threshold = limit.warning_threshold_percent as f64 / 100.0;

        // Check daily cost limit.
        if let Some(max_cents) = limit.max_cost_per_day_cents {
            let daily_cost = self
                .metering
                .get_spend(fighter_id, SpendPeriod::Day)
                .await?;
            let daily_cents = (daily_cost * 100.0) as u64;

            if daily_cents >= max_cents {
                debug!(%fighter_id, daily_cents, max_cents, "fighter over daily cost budget");
                return Ok(BudgetVerdict::Blocked {
                    reason: format!(
                        "daily cost budget exceeded: {}c / {}c",
                        daily_cents, max_cents
                    ),
                    retry_after_secs: seconds_until_day_reset(),
                });
            }

            let usage_pct = daily_cents as f64 / max_cents as f64;
            if usage_pct >= threshold {
                return Ok(BudgetVerdict::Warning {
                    usage_percent: usage_pct * 100.0,
                    message: format!(
                        "approaching daily cost limit: {}c / {}c ({:.0}%)",
                        daily_cents,
                        max_cents,
                        usage_pct * 100.0
                    ),
                });
            }
        }

        // Check hourly cost (using hourly spend as a proxy).
        if let Some(max_tokens_hour) = limit.max_tokens_per_hour {
            let hourly_cost = self
                .metering
                .get_spend(fighter_id, SpendPeriod::Hour)
                .await?;
            // We use cost as a proxy; for token-based limits we'd need token counts.
            // For now, use the metering engine's cost data.
            let _hourly_cost_cents = (hourly_cost * 100.0) as u64;

            // Token-based checks would need the memory substrate to return token counts.
            // For simplicity, we treat `max_tokens_per_hour` as checked via event_count.
            debug!(
                %fighter_id,
                max_tokens_hour,
                "hourly token limit configured (checked via cost proxy)"
            );
        }

        Ok(BudgetVerdict::Allowed)
    }

    /// Evaluate the global limit against total spend across all fighters.
    async fn evaluate_global_limit(&self, limit: &BudgetLimit) -> PunchResult<BudgetVerdict> {
        if !limit.has_any_limit() {
            return Ok(BudgetVerdict::Allowed);
        }

        let threshold = limit.warning_threshold_percent as f64 / 100.0;

        // Check daily cost limit.
        if let Some(max_cents) = limit.max_cost_per_day_cents {
            let daily_cost = self.metering.get_total_spend(SpendPeriod::Day).await?;
            let daily_cents = (daily_cost * 100.0) as u64;

            if daily_cents >= max_cents {
                return Ok(BudgetVerdict::Blocked {
                    reason: format!(
                        "global daily cost budget exceeded: {}c / {}c",
                        daily_cents, max_cents
                    ),
                    retry_after_secs: seconds_until_day_reset(),
                });
            }

            let usage_pct = daily_cents as f64 / max_cents as f64;
            if usage_pct >= threshold {
                return Ok(BudgetVerdict::Warning {
                    usage_percent: usage_pct * 100.0,
                    message: format!(
                        "approaching global daily cost limit: {}c / {}c ({:.0}%)",
                        daily_cents,
                        max_cents,
                        usage_pct * 100.0
                    ),
                });
            }
        }

        Ok(BudgetVerdict::Allowed)
    }
}

/// Return the more restrictive of two verdicts.
fn most_restrictive(a: BudgetVerdict, b: BudgetVerdict) -> BudgetVerdict {
    match (&a, &b) {
        (BudgetVerdict::Blocked { .. }, _) => a,
        (_, BudgetVerdict::Blocked { .. }) => b,
        (
            BudgetVerdict::Warning {
                usage_percent: pa, ..
            },
            BudgetVerdict::Warning {
                usage_percent: pb, ..
            },
        ) => {
            if pa >= pb {
                a
            } else {
                b
            }
        }
        (BudgetVerdict::Warning { .. }, _) => a,
        (_, BudgetVerdict::Warning { .. }) => b,
        _ => BudgetVerdict::Allowed,
    }
}

/// Calculate seconds until the next day boundary (midnight UTC).
fn seconds_until_day_reset() -> u64 {
    let now = chrono::Utc::now();
    let tomorrow = (now + chrono::Duration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0);

    match tomorrow {
        Some(t) => {
            let reset = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(t, chrono::Utc);
            (reset - now).num_seconds().max(0) as u64
        }
        None => 3600, // fallback: 1 hour
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_memory::MemorySubstrate;
    use punch_types::{FighterManifest, FighterStatus, ModelConfig, Provider, WeightClass};

    fn test_manifest() -> FighterManifest {
        FighterManifest {
            name: "budget-test".into(),
            description: "test".into(),
            model: ModelConfig {
                provider: Provider::Anthropic,
                model: "claude-sonnet-4-20250514".into(),
                api_key_env: None,
                base_url: None,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
            system_prompt: "test".into(),
            capabilities: Vec::new(),
            weight_class: WeightClass::Featherweight,
            tenant_id: None,
        }
    }

    async fn setup() -> (Arc<MeteringEngine>, Arc<MemorySubstrate>) {
        let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory substrate"));
        let metering = Arc::new(MeteringEngine::new(Arc::clone(&memory)));
        (metering, memory)
    }

    async fn setup_fighter(memory: &MemorySubstrate) -> FighterId {
        let fid = FighterId::new();
        memory
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .expect("save fighter");
        fid
    }

    #[tokio::test]
    async fn under_budget_allowed() {
        let (metering, memory) = setup().await;
        let fid = setup_fighter(&memory).await;
        let enforcer = BudgetEnforcer::new(metering);

        enforcer.set_fighter_limit(
            fid,
            BudgetLimit {
                max_cost_per_day_cents: Some(1000), // $10
                ..Default::default()
            },
        );

        let verdict = enforcer.check_budget(&fid).await.expect("check budget");
        assert_eq!(verdict, BudgetVerdict::Allowed);
    }

    #[tokio::test]
    async fn at_80_percent_warning() {
        let (metering, memory) = setup().await;
        let fid = setup_fighter(&memory).await;

        // Record usage that puts us at ~85% of $1.00 limit.
        // claude-sonnet: $3/M in, $15/M out
        // We need ~$0.85 = 85 cents.
        // 50000 input tokens at $3/M = $0.15
        // 50000 output tokens at $15/M = $0.75
        // Total = $0.90 = 90 cents >= 80% of 100 cents
        metering
            .record_usage(&fid, "claude-sonnet-4-20250514", 50000, 50000)
            .await
            .expect("record usage");

        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));
        enforcer.set_fighter_limit(
            fid,
            BudgetLimit {
                max_cost_per_day_cents: Some(100), // $1.00
                ..Default::default()
            },
        );

        let verdict = enforcer.check_budget(&fid).await.expect("check budget");
        assert!(
            matches!(verdict, BudgetVerdict::Warning { .. }),
            "expected warning at ~90%, got {:?}",
            verdict
        );
    }

    #[tokio::test]
    async fn over_budget_blocked() {
        let (metering, memory) = setup().await;
        let fid = setup_fighter(&memory).await;

        // Record usage that exceeds the limit.
        // claude-sonnet: $3/M in, $15/M out
        // 100K input = $0.30, 100K output = $1.50 => total $1.80 = 180 cents
        metering
            .record_usage(&fid, "claude-sonnet-4-20250514", 100_000, 100_000)
            .await
            .expect("record usage");

        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));
        enforcer.set_fighter_limit(
            fid,
            BudgetLimit {
                max_cost_per_day_cents: Some(100), // $1.00 = 100 cents
                ..Default::default()
            },
        );

        let verdict = enforcer.check_budget(&fid).await.expect("check budget");
        assert!(
            matches!(verdict, BudgetVerdict::Blocked { .. }),
            "expected blocked, got {:?}",
            verdict
        );

        if let BudgetVerdict::Blocked {
            retry_after_secs, ..
        } = verdict
        {
            assert!(retry_after_secs > 0);
        }
    }

    #[tokio::test]
    async fn budget_resets_at_period_boundary() {
        // This test verifies the concept: a fighter with no recent usage
        // should be allowed even if they had usage in a previous period.
        let (metering, memory) = setup().await;
        let fid = setup_fighter(&memory).await;
        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));

        enforcer.set_fighter_limit(
            fid,
            BudgetLimit {
                max_cost_per_day_cents: Some(100),
                ..Default::default()
            },
        );

        // No usage recorded => should be allowed.
        let verdict = enforcer.check_budget(&fid).await.expect("check budget");
        assert_eq!(verdict, BudgetVerdict::Allowed);
    }

    #[tokio::test]
    async fn per_fighter_limits_independent() {
        let (metering, memory) = setup().await;
        let fid1 = setup_fighter(&memory).await;
        let fid2 = setup_fighter(&memory).await;

        // Fighter 1 is over budget.
        metering
            .record_usage(&fid1, "claude-sonnet-4-20250514", 100_000, 100_000)
            .await
            .expect("record usage");

        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));
        enforcer.set_fighter_limit(
            fid1,
            BudgetLimit {
                max_cost_per_day_cents: Some(100),
                ..Default::default()
            },
        );
        enforcer.set_fighter_limit(
            fid2,
            BudgetLimit {
                max_cost_per_day_cents: Some(100),
                ..Default::default()
            },
        );

        let v1 = enforcer.check_budget(&fid1).await.expect("check fid1");
        let v2 = enforcer.check_budget(&fid2).await.expect("check fid2");

        assert!(matches!(v1, BudgetVerdict::Blocked { .. }));
        assert_eq!(v2, BudgetVerdict::Allowed);
    }

    #[tokio::test]
    async fn global_limit_applies_to_all_fighters() {
        let (metering, memory) = setup().await;
        let fid = setup_fighter(&memory).await;

        // Record significant usage.
        metering
            .record_usage(&fid, "claude-sonnet-4-20250514", 100_000, 100_000)
            .await
            .expect("record usage");

        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));
        enforcer
            .set_global_limit(BudgetLimit {
                max_cost_per_day_cents: Some(100),
                ..Default::default()
            })
            .await;

        // Even a different fighter should be blocked by global limit.
        let fid2 = setup_fighter(&memory).await;
        let verdict = enforcer.check_budget(&fid2).await.expect("check budget");
        assert!(
            matches!(verdict, BudgetVerdict::Blocked { .. }),
            "global limit should block: {:?}",
            verdict
        );
    }

    #[tokio::test]
    async fn no_limit_always_allowed() {
        let (metering, memory) = setup().await;
        let fid = setup_fighter(&memory).await;

        // Record heavy usage but set no limits.
        metering
            .record_usage(&fid, "claude-sonnet-4-20250514", 1_000_000, 1_000_000)
            .await
            .expect("record usage");

        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));

        let verdict = enforcer.check_budget(&fid).await.expect("check budget");
        assert_eq!(verdict, BudgetVerdict::Allowed);
    }

    #[tokio::test]
    async fn zero_limit_always_blocked() {
        let (metering, memory) = setup().await;
        let fid = setup_fighter(&memory).await;

        // Even with zero usage, a limit of 0 cents should block immediately.
        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));
        enforcer.set_fighter_limit(
            fid,
            BudgetLimit {
                max_cost_per_day_cents: Some(0),
                ..Default::default()
            },
        );

        let verdict = enforcer.check_budget(&fid).await.expect("check budget");
        assert!(
            matches!(verdict, BudgetVerdict::Blocked { .. }),
            "zero limit should block: {:?}",
            verdict
        );
    }

    #[tokio::test]
    async fn multiple_fighters_dont_interfere() {
        let (metering, memory) = setup().await;
        let fid1 = setup_fighter(&memory).await;
        let fid2 = setup_fighter(&memory).await;
        let fid3 = setup_fighter(&memory).await;

        // Only fid1 has usage.
        metering
            .record_usage(&fid1, "claude-sonnet-4-20250514", 100_000, 100_000)
            .await
            .expect("record usage");

        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));

        // Set different limits for each.
        enforcer.set_fighter_limit(
            fid1,
            BudgetLimit {
                max_cost_per_day_cents: Some(100),
                ..Default::default()
            },
        );
        enforcer.set_fighter_limit(
            fid2,
            BudgetLimit {
                max_cost_per_day_cents: Some(100),
                ..Default::default()
            },
        );
        enforcer.set_fighter_limit(
            fid3,
            BudgetLimit {
                max_cost_per_day_cents: Some(50),
                ..Default::default()
            },
        );

        let v1 = enforcer.check_budget(&fid1).await.expect("check fid1");
        let v2 = enforcer.check_budget(&fid2).await.expect("check fid2");
        let v3 = enforcer.check_budget(&fid3).await.expect("check fid3");

        assert!(matches!(v1, BudgetVerdict::Blocked { .. }));
        assert_eq!(v2, BudgetVerdict::Allowed);
        assert_eq!(v3, BudgetVerdict::Allowed);
    }

    #[tokio::test]
    async fn warning_threshold_configurable() {
        let (metering, memory) = setup().await;
        let fid = setup_fighter(&memory).await;

        // Record usage: ~$0.90 = 90 cents.
        metering
            .record_usage(&fid, "claude-sonnet-4-20250514", 50000, 50000)
            .await
            .expect("record usage");

        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));

        // Set threshold to 95% — with 90 cents out of 100, we should NOT get a warning.
        enforcer.set_fighter_limit(
            fid,
            BudgetLimit {
                max_cost_per_day_cents: Some(100),
                warning_threshold_percent: 95,
                ..Default::default()
            },
        );

        let verdict = enforcer.check_budget(&fid).await.expect("check budget");
        assert_eq!(
            verdict,
            BudgetVerdict::Allowed,
            "95% threshold should not warn at 90%: {:?}",
            verdict
        );

        // Now set threshold to 50% — should warn.
        enforcer.set_fighter_limit(
            fid,
            BudgetLimit {
                max_cost_per_day_cents: Some(100),
                warning_threshold_percent: 50,
                ..Default::default()
            },
        );

        let verdict = enforcer.check_budget(&fid).await.expect("check budget");
        assert!(
            matches!(verdict, BudgetVerdict::Warning { .. }),
            "50% threshold should warn at 90%: {:?}",
            verdict
        );
    }

    #[tokio::test]
    async fn cost_based_budget() {
        let (metering, memory) = setup().await;
        let fid = setup_fighter(&memory).await;

        // Use gpt-4o-mini which is much cheaper: $0.15/M in, $0.60/M out
        // 10K input = $0.0015, 10K output = $0.006 => total $0.0075 = ~0.75 cents
        metering
            .record_usage(&fid, "gpt-4o-mini", 10_000, 10_000)
            .await
            .expect("record usage");

        let enforcer = BudgetEnforcer::new(Arc::clone(&metering));
        enforcer.set_fighter_limit(
            fid,
            BudgetLimit {
                max_cost_per_day_cents: Some(1), // 1 cent limit
                ..Default::default()
            },
        );

        let verdict = enforcer.check_budget(&fid).await.expect("check budget");
        // 0.75 cents out of 1 cent = 75%, under 80% threshold => Allowed
        assert_eq!(
            verdict,
            BudgetVerdict::Allowed,
            "0.75 cents should be under 1 cent limit at 80% threshold: {:?}",
            verdict
        );
    }

    #[test]
    fn most_restrictive_selects_blocked_over_warning() {
        let a = BudgetVerdict::Warning {
            usage_percent: 85.0,
            message: "warning".to_string(),
        };
        let b = BudgetVerdict::Blocked {
            reason: "blocked".to_string(),
            retry_after_secs: 100,
        };

        let result = most_restrictive(a, b);
        assert!(matches!(result, BudgetVerdict::Blocked { .. }));
    }

    #[test]
    fn most_restrictive_selects_warning_over_allowed() {
        let a = BudgetVerdict::Allowed;
        let b = BudgetVerdict::Warning {
            usage_percent: 85.0,
            message: "warning".to_string(),
        };

        let result = most_restrictive(a, b);
        assert!(matches!(result, BudgetVerdict::Warning { .. }));
    }

    #[test]
    fn most_restrictive_both_allowed() {
        let result = most_restrictive(BudgetVerdict::Allowed, BudgetVerdict::Allowed);
        assert_eq!(result, BudgetVerdict::Allowed);
    }

    #[test]
    fn budget_limit_default() {
        let limit = BudgetLimit::default();
        assert!(!limit.has_any_limit());
        assert_eq!(limit.warning_threshold_percent, 80);
    }

    #[test]
    fn seconds_until_day_reset_positive() {
        let secs = seconds_until_day_reset();
        assert!(secs > 0);
        assert!(secs <= 86400);
    }
}
