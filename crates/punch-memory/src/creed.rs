//! Creed persistence — stores and loads fighter identity documents.
//!
//! Creeds are stored as JSON in the `creeds` table, keyed by fighter_name
//! so they survive fighter kill/respawn cycles.

use punch_types::{Creed, FighterId, PunchError, PunchResult};
use tracing::debug;

use crate::MemorySubstrate;

impl MemorySubstrate {
    /// Save or update a creed. Uses fighter_name as the natural key.
    pub async fn save_creed(&self, creed: &Creed) -> PunchResult<()> {
        let creed_data = serde_json::to_string(creed)
            .map_err(|e| PunchError::Memory(format!("failed to serialize creed: {e}")))?;
        let fighter_id_str = creed.fighter_id.map(|id| id.to_string());
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO creeds (id, fighter_name, fighter_id, creed_data, version, bout_count, message_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
             ON CONFLICT(fighter_name) DO UPDATE SET
                fighter_id = excluded.fighter_id,
                creed_data = excluded.creed_data,
                version = excluded.version,
                bout_count = excluded.bout_count,
                message_count = excluded.message_count,
                updated_at = excluded.updated_at",
            rusqlite::params![
                creed.id.to_string(),
                creed.fighter_name,
                fighter_id_str,
                creed_data,
                creed.version,
                creed.bout_count,
                creed.message_count,
                now,
            ],
        )
        .map_err(|e| PunchError::Memory(format!("failed to save creed: {e}")))?;

        debug!(fighter_name = %creed.fighter_name, version = creed.version, "creed saved");
        Ok(())
    }

    /// Load a creed by fighter name. Returns None if no creed exists.
    pub async fn load_creed_by_name(&self, fighter_name: &str) -> PunchResult<Option<Creed>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT creed_data FROM creeds WHERE fighter_name = ?1")
            .map_err(|e| PunchError::Memory(format!("failed to prepare creed query: {e}")))?;

        let result: Option<String> = stmt.query_row([fighter_name], |row| row.get(0)).ok();

        match result {
            Some(data) => {
                let creed: Creed = serde_json::from_str(&data)
                    .map_err(|e| PunchError::Memory(format!("failed to deserialize creed: {e}")))?;
                Ok(Some(creed))
            }
            None => Ok(None),
        }
    }

    /// Load a creed by fighter ID. Returns None if no creed exists.
    pub async fn load_creed_by_fighter(
        &self,
        fighter_id: &FighterId,
    ) -> PunchResult<Option<Creed>> {
        let fighter_str = fighter_id.to_string();
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT creed_data FROM creeds WHERE fighter_id = ?1")
            .map_err(|e| PunchError::Memory(format!("failed to prepare creed query: {e}")))?;

        let result: Option<String> = stmt.query_row([&fighter_str], |row| row.get(0)).ok();

        match result {
            Some(data) => {
                let creed: Creed = serde_json::from_str(&data)
                    .map_err(|e| PunchError::Memory(format!("failed to deserialize creed: {e}")))?;
                Ok(Some(creed))
            }
            None => Ok(None),
        }
    }

    /// List all creeds.
    pub async fn list_creeds(&self) -> PunchResult<Vec<Creed>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT creed_data FROM creeds ORDER BY updated_at DESC")
            .map_err(|e| PunchError::Memory(format!("failed to list creeds: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let data: String = row.get(0)?;
                Ok(data)
            })
            .map_err(|e| PunchError::Memory(format!("failed to read creed rows: {e}")))?;

        let mut creeds = Vec::new();
        for row in rows {
            let data = row.map_err(|e| PunchError::Memory(format!("failed to read creed: {e}")))?;
            let creed: Creed = serde_json::from_str(&data)
                .map_err(|e| PunchError::Memory(format!("failed to deserialize creed: {e}")))?;
            creeds.push(creed);
        }
        Ok(creeds)
    }

    /// Delete a creed by fighter name.
    pub async fn delete_creed(&self, fighter_name: &str) -> PunchResult<()> {
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM creeds WHERE fighter_name = ?1", [fighter_name])
            .map_err(|e| PunchError::Memory(format!("failed to delete creed: {e}")))?;
        debug!(fighter_name = fighter_name, "creed deleted");
        Ok(())
    }

    /// Bind a creed to a specific fighter instance (after spawn/respawn).
    pub async fn bind_creed_to_fighter(
        &self,
        fighter_name: &str,
        fighter_id: &FighterId,
    ) -> PunchResult<()> {
        let fighter_str = fighter_id.to_string();
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE creeds SET fighter_id = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE fighter_name = ?2",
            rusqlite::params![fighter_str, fighter_name],
        )
        .map_err(|e| PunchError::Memory(format!("failed to bind creed: {e}")))?;
        debug!(fighter_name = fighter_name, fighter_id = %fighter_id, "creed bound to fighter");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use punch_types::{Creed, FighterId};

    use crate::MemorySubstrate;

    fn make_creed(name: &str) -> Creed {
        Creed::new(name)
    }

    #[tokio::test]
    async fn test_save_and_load_creed_by_name() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let creed = make_creed("atlas");

        substrate.save_creed(&creed).await.unwrap();
        let loaded = substrate.load_creed_by_name("atlas").await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.fighter_name, "atlas");
        assert_eq!(loaded.id, creed.id);
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.bout_count, 0);
        assert_eq!(loaded.message_count, 0);
    }

    #[tokio::test]
    async fn test_load_creed_by_fighter() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = FighterId::new();
        let mut creed = make_creed("bravo");
        creed.fighter_id = Some(fighter_id);

        substrate.save_creed(&creed).await.unwrap();
        let loaded = substrate.load_creed_by_fighter(&fighter_id).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.fighter_name, "bravo");
        assert_eq!(loaded.fighter_id, Some(fighter_id));
    }

    #[tokio::test]
    async fn test_save_creed_upsert() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let mut creed = make_creed("charlie");
        creed.identity = "original".into();
        substrate.save_creed(&creed).await.unwrap();

        // Update the creed and save again — should upsert by fighter_name.
        creed.identity = "evolved".into();
        creed.version = 2;
        creed.bout_count = 5;
        substrate.save_creed(&creed).await.unwrap();

        let loaded = substrate
            .load_creed_by_name("charlie")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.identity, "evolved");
        assert_eq!(loaded.version, 2);
        assert_eq!(loaded.bout_count, 5);
    }

    #[tokio::test]
    async fn test_list_creeds() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        substrate.save_creed(&make_creed("delta")).await.unwrap();
        substrate.save_creed(&make_creed("echo")).await.unwrap();
        substrate.save_creed(&make_creed("foxtrot")).await.unwrap();

        let creeds = substrate.list_creeds().await.unwrap();
        assert_eq!(creeds.len(), 3);

        let names: Vec<&str> = creeds.iter().map(|c| c.fighter_name.as_str()).collect();
        assert!(names.contains(&"delta"));
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"foxtrot"));
    }

    #[tokio::test]
    async fn test_delete_creed() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        substrate.save_creed(&make_creed("golf")).await.unwrap();

        // Verify it exists.
        assert!(
            substrate
                .load_creed_by_name("golf")
                .await
                .unwrap()
                .is_some()
        );

        // Delete it.
        substrate.delete_creed("golf").await.unwrap();
        assert!(
            substrate
                .load_creed_by_name("golf")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_bind_creed_to_fighter() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let creed = make_creed("hotel");
        substrate.save_creed(&creed).await.unwrap();

        // Initially no fighter_id.
        let loaded = substrate
            .load_creed_by_name("hotel")
            .await
            .unwrap()
            .unwrap();
        assert!(loaded.fighter_id.is_none());

        // Bind to a fighter.
        let fighter_id = FighterId::new();
        substrate
            .bind_creed_to_fighter("hotel", &fighter_id)
            .await
            .unwrap();

        // The creed_data JSON is not updated by bind, only the fighter_id column.
        // So load_creed_by_fighter should find it via the index column.
        let loaded = substrate.load_creed_by_fighter(&fighter_id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().fighter_name, "hotel");
    }

    #[tokio::test]
    async fn test_load_nonexistent_creed_returns_none() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let by_name = substrate.load_creed_by_name("nonexistent").await.unwrap();
        assert!(by_name.is_none());

        let by_id = substrate
            .load_creed_by_fighter(&FighterId::new())
            .await
            .unwrap();
        assert!(by_id.is_none());
    }
}
