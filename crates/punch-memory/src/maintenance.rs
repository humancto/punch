//! Maintenance operations for the memory substrate.
//!
//! Provides cleanup, compaction, vacuum, and statistical query operations
//! used by the gorilla execution engine (Data Sweeper, Report Generator, etc.).

use chrono::{DateTime, Utc};
use tracing::{debug, info};

use punch_types::{PunchError, PunchResult};

use crate::MemorySubstrate;

impl MemorySubstrate {
    /// Delete bout messages older than the given cutoff date.
    ///
    /// Returns the number of messages deleted.
    pub async fn cleanup_old_messages(&self, cutoff: DateTime<Utc>) -> PunchResult<usize> {
        let cutoff_str = cutoff.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let conn = self.conn.lock().await;

        let count = conn
            .execute(
                "DELETE FROM messages WHERE created_at < ?1",
                rusqlite::params![cutoff_str],
            )
            .map_err(|e| PunchError::Memory(format!("failed to cleanup old messages: {e}")))?;

        info!(deleted = count, cutoff = %cutoff_str, "cleaned up old messages");
        Ok(count)
    }

    /// Compact memory entries by removing low-confidence entries when a fighter
    /// exceeds the maximum number of memories.
    ///
    /// Returns the number of entries removed.
    pub async fn compact_memories(&self, max_per_fighter: usize) -> PunchResult<usize> {
        let conn = self.conn.lock().await;

        // Find fighters that exceed the limit.
        let mut stmt = conn
            .prepare(
                "SELECT fighter_id, COUNT(*) as cnt FROM memories \
                 GROUP BY fighter_id HAVING cnt > ?1",
            )
            .map_err(|e| PunchError::Memory(format!("failed to query memory counts: {e}")))?;

        let fighters: Vec<(String, usize)> = stmt
            .query_map(rusqlite::params![max_per_fighter as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
            })
            .map_err(|e| PunchError::Memory(format!("failed to list fighter memories: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        let mut total_removed = 0;

        for (fighter_id, count) in &fighters {
            let excess = count - max_per_fighter;
            if excess > 0 {
                // Delete the lowest-confidence entries for this fighter.
                let deleted = conn
                    .execute(
                        "DELETE FROM memories WHERE rowid IN (\
                             SELECT rowid FROM memories \
                             WHERE fighter_id = ?1 \
                             ORDER BY confidence ASC \
                             LIMIT ?2\
                         )",
                        rusqlite::params![fighter_id, excess],
                    )
                    .map_err(|e| PunchError::Memory(format!("failed to compact memories: {e}")))?;
                total_removed += deleted;
                debug!(
                    fighter_id = %fighter_id,
                    removed = deleted,
                    "compacted memories for fighter"
                );
            }
        }

        info!(total_removed, "memory compaction complete");
        Ok(total_removed)
    }

    /// Run SQLite VACUUM to reclaim disk space.
    pub async fn vacuum(&self) -> PunchResult<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("VACUUM")
            .map_err(|e| PunchError::Memory(format!("vacuum failed: {e}")))?;
        info!("database vacuumed");
        Ok(())
    }

    /// Count bouts created within a time period.
    pub async fn count_bouts_in_period(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> PunchResult<usize> {
        let start_str = start.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let end_str = end.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let conn = self.conn.lock().await;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bouts WHERE created_at >= ?1 AND created_at <= ?2",
                rusqlite::params![start_str, end_str],
                |row| row.get(0),
            )
            .map_err(|e| PunchError::Memory(format!("failed to count bouts: {e}")))?;

        Ok(count as usize)
    }

    /// Count messages created within a time period.
    pub async fn count_messages_in_period(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> PunchResult<usize> {
        let start_str = start.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let end_str = end.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let conn = self.conn.lock().await;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE created_at >= ?1 AND created_at <= ?2",
                rusqlite::params![start_str, end_str],
                |row| row.get(0),
            )
            .map_err(|e| PunchError::Memory(format!("failed to count messages: {e}")))?;

        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use punch_types::{
        FighterId, FighterManifest, FighterStatus, Message, ModelConfig, Provider, Role,
        WeightClass,
    };

    use crate::MemorySubstrate;

    fn test_manifest() -> FighterManifest {
        FighterManifest {
            name: "Test".into(),
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
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        }
    }

    #[tokio::test]
    async fn test_cleanup_old_messages() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = FighterId::new();
        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();
        let bout_id = substrate.create_bout(&fighter_id).await.unwrap();

        substrate
            .save_message(&bout_id, &Message::new(Role::User, "old msg"))
            .await
            .unwrap();

        // Cutoff in the future should delete everything.
        let cutoff = chrono::Utc::now() + chrono::Duration::hours(1);
        let deleted = substrate.cleanup_old_messages(cutoff).await.unwrap();
        assert!(deleted >= 1);
    }

    #[tokio::test]
    async fn test_cleanup_old_messages_none_deleted() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = FighterId::new();
        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();
        let bout_id = substrate.create_bout(&fighter_id).await.unwrap();

        substrate
            .save_message(&bout_id, &Message::new(Role::User, "recent msg"))
            .await
            .unwrap();

        // Cutoff in the past should delete nothing.
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);
        let deleted = substrate.cleanup_old_messages(cutoff).await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn test_compact_memories() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = FighterId::new();

        // Store several memories.
        for i in 0..5 {
            substrate
                .store_memory(
                    &fighter_id,
                    &format!("key_{}", i),
                    &format!("value_{}", i),
                    (i as f64) * 0.2,
                )
                .await
                .unwrap();
        }

        // Compact to max 3.
        let removed = substrate.compact_memories(3).await.unwrap();
        assert_eq!(removed, 2);
    }

    #[tokio::test]
    async fn test_compact_memories_no_excess() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = FighterId::new();

        substrate
            .store_memory(&fighter_id, "key", "value", 0.9)
            .await
            .unwrap();

        let removed = substrate.compact_memories(10).await.unwrap();
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn test_vacuum() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        // Vacuum on in-memory should succeed.
        substrate.vacuum().await.unwrap();
    }

    #[tokio::test]
    async fn test_count_bouts_in_period() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = FighterId::new();
        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        let before = chrono::Utc::now() - chrono::Duration::seconds(1);
        substrate.create_bout(&fighter_id).await.unwrap();
        substrate.create_bout(&fighter_id).await.unwrap();
        let after = chrono::Utc::now() + chrono::Duration::seconds(1);

        let count = substrate
            .count_bouts_in_period(before, after)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_count_messages_in_period() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = FighterId::new();
        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();
        let bout_id = substrate.create_bout(&fighter_id).await.unwrap();

        let before = chrono::Utc::now() - chrono::Duration::seconds(1);
        substrate
            .save_message(&bout_id, &Message::new(Role::User, "a"))
            .await
            .unwrap();
        substrate
            .save_message(&bout_id, &Message::new(Role::Assistant, "b"))
            .await
            .unwrap();
        let after = chrono::Utc::now() + chrono::Duration::seconds(1);

        let count = substrate
            .count_messages_in_period(before, after)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_count_bouts_empty_period() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let start = chrono::Utc::now() - chrono::Duration::days(365);
        let end = chrono::Utc::now() - chrono::Duration::days(364);
        let count = substrate.count_bouts_in_period(start, end).await.unwrap();
        assert_eq!(count, 0);
    }
}
