use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use punch_types::{FighterId, PunchError, PunchResult};
use tracing::debug;

use crate::MemorySubstrate;

/// A single usage / metering event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub id: i64,
    pub fighter_id: FighterId,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub created_at: String,
}

/// Aggregated usage summary for a fighter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub event_count: u64,
}

/// Per-model usage breakdown row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsageBreakdown {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub request_count: u64,
}

/// Per-fighter usage breakdown row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FighterUsageBreakdown {
    pub fighter_id: FighterId,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub request_count: u64,
}

impl MemorySubstrate {
    /// Record a usage event for a fighter.
    pub async fn record_usage(
        &self,
        fighter_id: &FighterId,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    ) -> PunchResult<()> {
        let fighter_str = fighter_id.to_string();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO usage_events (fighter_id, model, input_tokens, output_tokens, cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![fighter_str, model, input_tokens, output_tokens, cost_usd],
        )
        .map_err(|e| PunchError::Memory(format!("failed to record usage: {e}")))?;

        debug!(
            fighter_id = %fighter_id,
            model = model,
            input_tokens = input_tokens,
            output_tokens = output_tokens,
            "usage recorded"
        );
        Ok(())
    }

    /// Get an aggregated usage summary for a fighter since the given timestamp.
    pub async fn get_usage_summary(
        &self,
        fighter_id: &FighterId,
        since: DateTime<Utc>,
    ) -> PunchResult<UsageSummary> {
        let fighter_str = fighter_id.to_string();
        let since_str = since.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let conn = self.conn.lock().await;

        let result = conn
            .query_row(
                "SELECT COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    COUNT(*)
             FROM usage_events
             WHERE fighter_id = ?1 AND created_at >= ?2",
                rusqlite::params![fighter_str, since_str],
                |row| {
                    let total_input_tokens: u64 = row.get(0)?;
                    let total_output_tokens: u64 = row.get(1)?;
                    let total_cost_usd: f64 = row.get(2)?;
                    let event_count: u64 = row.get(3)?;
                    Ok(UsageSummary {
                        total_input_tokens,
                        total_output_tokens,
                        total_cost_usd,
                        event_count,
                    })
                },
            )
            .map_err(|e| PunchError::Memory(format!("failed to get usage summary: {e}")))?;

        Ok(result)
    }

    /// Get per-model usage breakdown for a fighter since the given timestamp.
    pub async fn get_model_breakdown(
        &self,
        fighter_id: &FighterId,
        since: DateTime<Utc>,
    ) -> PunchResult<Vec<ModelUsageBreakdown>> {
        let fighter_str = fighter_id.to_string();
        let since_str = since.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let conn = self.conn.lock().await;

        let mut stmt = conn
            .prepare(
                "SELECT model,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    COUNT(*)
                 FROM usage_events
                 WHERE fighter_id = ?1 AND created_at >= ?2
                 GROUP BY model
                 ORDER BY SUM(cost_usd) DESC",
            )
            .map_err(|e| PunchError::Memory(format!("failed to prepare model breakdown: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![fighter_str, since_str], |row| {
                Ok(ModelUsageBreakdown {
                    model: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cost_usd: row.get(3)?,
                    request_count: row.get(4)?,
                })
            })
            .map_err(|e| PunchError::Memory(format!("failed to query model breakdown: {e}")))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(
                row.map_err(|e| PunchError::Memory(format!("failed to read breakdown row: {e}")))?,
            );
        }
        Ok(result)
    }

    /// Get per-model usage breakdown across ALL fighters since the given timestamp.
    pub async fn get_total_model_breakdown(
        &self,
        since: DateTime<Utc>,
    ) -> PunchResult<Vec<ModelUsageBreakdown>> {
        let since_str = since.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let conn = self.conn.lock().await;

        let mut stmt = conn
            .prepare(
                "SELECT model,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    COUNT(*)
                 FROM usage_events
                 WHERE created_at >= ?1
                 GROUP BY model
                 ORDER BY SUM(cost_usd) DESC",
            )
            .map_err(|e| PunchError::Memory(format!("failed to prepare model breakdown: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![since_str], |row| {
                Ok(ModelUsageBreakdown {
                    model: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cost_usd: row.get(3)?,
                    request_count: row.get(4)?,
                })
            })
            .map_err(|e| PunchError::Memory(format!("failed to query model breakdown: {e}")))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(
                row.map_err(|e| PunchError::Memory(format!("failed to read breakdown row: {e}")))?,
            );
        }
        Ok(result)
    }

    /// Get per-fighter usage breakdown across all fighters since the given timestamp.
    pub async fn get_fighter_breakdown(
        &self,
        since: DateTime<Utc>,
    ) -> PunchResult<Vec<FighterUsageBreakdown>> {
        let since_str = since.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let conn = self.conn.lock().await;

        let mut stmt = conn
            .prepare(
                "SELECT fighter_id,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    COUNT(*)
                 FROM usage_events
                 WHERE created_at >= ?1
                 GROUP BY fighter_id
                 ORDER BY SUM(cost_usd) DESC",
            )
            .map_err(|e| PunchError::Memory(format!("failed to prepare fighter breakdown: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![since_str], |row| {
                let id_str: String = row.get(0)?;
                let fighter_id = id_str
                    .parse::<uuid::Uuid>()
                    .map(FighterId)
                    .unwrap_or_else(|_| FighterId::new());
                Ok(FighterUsageBreakdown {
                    fighter_id,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cost_usd: row.get(3)?,
                    request_count: row.get(4)?,
                })
            })
            .map_err(|e| PunchError::Memory(format!("failed to query fighter breakdown: {e}")))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(
                row.map_err(|e| PunchError::Memory(format!("failed to read breakdown row: {e}")))?,
            );
        }
        Ok(result)
    }

    /// Get an aggregated usage summary across ALL fighters since the given timestamp.
    pub async fn get_total_usage_summary(&self, since: DateTime<Utc>) -> PunchResult<UsageSummary> {
        let since_str = since.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let conn = self.conn.lock().await;

        let result = conn
            .query_row(
                "SELECT COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cost_usd), 0.0),
                    COUNT(*)
             FROM usage_events
             WHERE created_at >= ?1",
                rusqlite::params![since_str],
                |row| {
                    let total_input_tokens: u64 = row.get(0)?;
                    let total_output_tokens: u64 = row.get(1)?;
                    let total_cost_usd: f64 = row.get(2)?;
                    let event_count: u64 = row.get(3)?;
                    Ok(UsageSummary {
                        total_input_tokens,
                        total_output_tokens,
                        total_cost_usd,
                        event_count,
                    })
                },
            )
            .map_err(|e| PunchError::Memory(format!("failed to get total usage summary: {e}")))?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use punch_types::{FighterManifest, FighterStatus, ModelConfig, Provider, WeightClass};

    use crate::MemorySubstrate;

    fn test_manifest() -> FighterManifest {
        FighterManifest {
            name: "Usage Fighter".into(),
            description: "usage test".into(),
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

    #[tokio::test]
    async fn test_record_and_summarize_usage() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .record_usage(&fid, "claude-sonnet-4-20250514", 1000, 500, 0.015)
            .await
            .unwrap();
        substrate
            .record_usage(&fid, "claude-sonnet-4-20250514", 2000, 800, 0.028)
            .await
            .unwrap();

        let since = Utc::now() - Duration::hours(1);
        let summary = substrate.get_usage_summary(&fid, since).await.unwrap();

        assert_eq!(summary.event_count, 2);
        assert_eq!(summary.total_input_tokens, 3000);
        assert_eq!(summary.total_output_tokens, 1300);
        assert!((summary.total_cost_usd - 0.043).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_model_breakdown() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .record_usage(&fid, "claude-sonnet-4-20250514", 1000, 500, 0.015)
            .await
            .unwrap();
        substrate
            .record_usage(&fid, "gpt-4o-mini", 2000, 800, 0.002)
            .await
            .unwrap();
        substrate
            .record_usage(&fid, "claude-sonnet-4-20250514", 3000, 1000, 0.030)
            .await
            .unwrap();

        let since = Utc::now() - Duration::hours(1);
        let breakdown = substrate.get_model_breakdown(&fid, since).await.unwrap();

        assert_eq!(breakdown.len(), 2);
        // Ordered by cost DESC, so sonnet ($0.045) first, then gpt-4o-mini ($0.002)
        assert_eq!(breakdown[0].model, "claude-sonnet-4-20250514");
        assert_eq!(breakdown[0].input_tokens, 4000);
        assert_eq!(breakdown[0].output_tokens, 1500);
        assert_eq!(breakdown[0].request_count, 2);
        assert_eq!(breakdown[1].model, "gpt-4o-mini");
        assert_eq!(breakdown[1].request_count, 1);
    }

    #[tokio::test]
    async fn test_total_model_breakdown() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid1 = punch_types::FighterId::new();
        let fid2 = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid1, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();
        substrate
            .save_fighter(&fid2, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .record_usage(&fid1, "claude-sonnet-4-20250514", 1000, 500, 0.015)
            .await
            .unwrap();
        substrate
            .record_usage(&fid2, "claude-sonnet-4-20250514", 2000, 800, 0.028)
            .await
            .unwrap();

        let since = Utc::now() - Duration::hours(1);
        let breakdown = substrate.get_total_model_breakdown(since).await.unwrap();

        assert_eq!(breakdown.len(), 1);
        assert_eq!(breakdown[0].input_tokens, 3000);
        assert_eq!(breakdown[0].request_count, 2);
    }

    #[tokio::test]
    async fn test_fighter_breakdown() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid1 = punch_types::FighterId::new();
        let fid2 = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid1, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();
        substrate
            .save_fighter(&fid2, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .record_usage(&fid1, "claude-sonnet-4-20250514", 1000, 500, 0.015)
            .await
            .unwrap();
        substrate
            .record_usage(&fid2, "gpt-4o-mini", 5000, 2000, 0.004)
            .await
            .unwrap();

        let since = Utc::now() - Duration::hours(1);
        let breakdown = substrate.get_fighter_breakdown(since).await.unwrap();

        assert_eq!(breakdown.len(), 2);
        // Ordered by cost DESC: sonnet ($0.015) first
        assert_eq!(breakdown[0].fighter_id, fid1);
        assert_eq!(breakdown[1].fighter_id, fid2);
    }

    #[tokio::test]
    async fn test_usage_summary_empty() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        let since = Utc::now() - Duration::hours(1);
        let summary = substrate.get_usage_summary(&fid, since).await.unwrap();

        assert_eq!(summary.event_count, 0);
        assert_eq!(summary.total_input_tokens, 0);
    }
}
