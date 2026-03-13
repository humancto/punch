use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use punch_types::{FighterId, PunchError, PunchResult};
use tracing::debug;

use crate::MemorySubstrate;

/// A single memory entry stored for a fighter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub key: String,
    pub value: String,
    pub confidence: f64,
    pub created_at: DateTime<Utc>,
    pub accessed_at: DateTime<Utc>,
}

impl MemorySubstrate {
    /// Store (or overwrite) a key-value memory for a fighter.
    pub async fn store_memory(
        &self,
        fighter_id: &FighterId,
        key: &str,
        value: &str,
        confidence: f64,
    ) -> PunchResult<()> {
        let fighter_str = fighter_id.to_string();
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO memories (fighter_id, key, value, confidence, created_at, accessed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(fighter_id, key) DO UPDATE SET
                value = excluded.value,
                confidence = excluded.confidence,
                accessed_at = excluded.accessed_at",
            rusqlite::params![fighter_str, key, value, confidence, now],
        )
        .map_err(|e| PunchError::Memory(format!("failed to store memory: {e}")))?;

        debug!(fighter_id = %fighter_id, key = key, "memory stored");
        Ok(())
    }

    /// Recall memories matching a query substring, ordered by confidence descending.
    pub async fn recall_memories(
        &self,
        fighter_id: &FighterId,
        query: &str,
        limit: u32,
    ) -> PunchResult<Vec<MemoryEntry>> {
        let fighter_str = fighter_id.to_string();
        let pattern = format!("%{query}%");

        let conn = self.conn.lock().await;

        // Update accessed_at for matched rows.
        conn.execute(
            "UPDATE memories SET accessed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
             WHERE fighter_id = ?1 AND (key LIKE ?2 OR value LIKE ?2)",
            rusqlite::params![fighter_str, pattern],
        )
        .map_err(|e| PunchError::Memory(format!("failed to touch memory: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT key, value, confidence, created_at, accessed_at
                 FROM memories
                 WHERE fighter_id = ?1 AND (key LIKE ?2 OR value LIKE ?2)
                 ORDER BY confidence DESC
                 LIMIT ?3",
            )
            .map_err(|e| PunchError::Memory(format!("failed to recall memories: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![fighter_str, pattern, limit], |row| {
                let key: String = row.get(0)?;
                let value: String = row.get(1)?;
                let confidence: f64 = row.get(2)?;
                let created_at: String = row.get(3)?;
                let accessed_at: String = row.get(4)?;
                Ok((key, value, confidence, created_at, accessed_at))
            })
            .map_err(|e| PunchError::Memory(format!("failed to recall memories: {e}")))?;

        let mut entries = Vec::new();
        for row in rows {
            let (key, value, confidence, created_at, accessed_at) =
                row.map_err(|e| PunchError::Memory(format!("failed to read memory row: {e}")))?;

            entries.push(MemoryEntry {
                key,
                value,
                confidence,
                created_at: parse_ts(&created_at)?,
                accessed_at: parse_ts(&accessed_at)?,
            });
        }

        Ok(entries)
    }

    /// Decay all memory confidences for a fighter by a multiplicative rate.
    ///
    /// Each memory's confidence becomes `confidence * (1.0 - rate)`. Memories
    /// that decay below a threshold (0.01) are automatically deleted.
    pub async fn decay_memories(&self, fighter_id: &FighterId, rate: f64) -> PunchResult<()> {
        let fighter_str = fighter_id.to_string();
        let factor = 1.0 - rate;

        let conn = self.conn.lock().await;

        conn.execute(
            "UPDATE memories SET confidence = confidence * ?1 WHERE fighter_id = ?2",
            rusqlite::params![factor, fighter_str],
        )
        .map_err(|e| PunchError::Memory(format!("failed to decay memories: {e}")))?;

        // Prune near-zero memories.
        conn.execute(
            "DELETE FROM memories WHERE fighter_id = ?1 AND confidence < 0.01",
            [&fighter_str],
        )
        .map_err(|e| PunchError::Memory(format!("failed to prune memories: {e}")))?;

        debug!(fighter_id = %fighter_id, rate = rate, "memories decayed");
        Ok(())
    }

    /// Delete a specific memory by key.
    pub async fn delete_memory(&self, fighter_id: &FighterId, key: &str) -> PunchResult<()> {
        let fighter_str = fighter_id.to_string();
        let conn = self.conn.lock().await;

        conn.execute(
            "DELETE FROM memories WHERE fighter_id = ?1 AND key = ?2",
            rusqlite::params![fighter_str, key],
        )
        .map_err(|e| PunchError::Memory(format!("failed to delete memory: {e}")))?;

        debug!(fighter_id = %fighter_id, key = key, "memory deleted");
        Ok(())
    }
}

fn parse_ts(s: &str) -> PunchResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ").map(|ndt| ndt.and_utc())
        })
        .map_err(|e| PunchError::Memory(format!("invalid timestamp '{s}': {e}")))
}

#[cfg(test)]
mod tests {
    use punch_types::{FighterManifest, FighterStatus, ModelConfig, Provider, WeightClass};

    use crate::MemorySubstrate;

    fn test_manifest() -> FighterManifest {
        FighterManifest {
            name: "Mem Fighter".into(),
            description: "memory test".into(),
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
        }
    }

    #[tokio::test]
    async fn test_store_and_recall() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .store_memory(&fid, "user_name", "Alice", 0.9)
            .await
            .unwrap();
        substrate
            .store_memory(&fid, "user_lang", "Rust", 0.8)
            .await
            .unwrap();

        let results = substrate.recall_memories(&fid, "user", 10).await.unwrap();
        assert_eq!(results.len(), 2);
        // Highest confidence first.
        assert_eq!(results[0].key, "user_name");
    }

    #[tokio::test]
    async fn test_decay_memories() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .store_memory(&fid, "fact", "sky is blue", 0.05)
            .await
            .unwrap();

        // A heavy decay should prune a low-confidence memory.
        substrate.decay_memories(&fid, 0.9).await.unwrap();

        let results = substrate.recall_memories(&fid, "fact", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_store_memory_overwrites() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate.store_memory(&fid, "key", "old_value", 0.5).await.unwrap();
        substrate.store_memory(&fid, "key", "new_value", 0.9).await.unwrap();

        let results = substrate.recall_memories(&fid, "key", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, "new_value");
        assert!((results[0].confidence - 0.9).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_recall_empty() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        let results = substrate.recall_memories(&fid, "nothing", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_recall_limit() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        for i in 0..10 {
            substrate
                .store_memory(&fid, &format!("item_{i}"), &format!("val_{i}"), 0.5)
                .await
                .unwrap();
        }

        let results = substrate.recall_memories(&fid, "item", 3).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_recall_ordered_by_confidence() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate.store_memory(&fid, "low_prio", "data", 0.2).await.unwrap();
        substrate.store_memory(&fid, "high_prio", "data", 0.9).await.unwrap();
        substrate.store_memory(&fid, "mid_prio", "data", 0.5).await.unwrap();

        let results = substrate.recall_memories(&fid, "prio", 10).await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].key, "high_prio");
        assert_eq!(results[2].key, "low_prio");
    }

    #[tokio::test]
    async fn test_decay_preserves_high_confidence() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate.store_memory(&fid, "strong", "data", 1.0).await.unwrap();

        // Light decay should not prune a 1.0 confidence memory
        substrate.decay_memories(&fid, 0.1).await.unwrap();

        let results = substrate.recall_memories(&fid, "strong", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].confidence > 0.5);
    }

    #[tokio::test]
    async fn test_delete_nonexistent_memory() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        // Should not error
        substrate.delete_memory(&fid, "nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn test_delete_memory() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .store_memory(&fid, "temp", "data", 1.0)
            .await
            .unwrap();
        substrate.delete_memory(&fid, "temp").await.unwrap();

        let results = substrate.recall_memories(&fid, "temp", 10).await.unwrap();
        assert!(results.is_empty());
    }
}
