use punch_types::{FighterId, FighterManifest, FighterStatus, PunchError, PunchResult};
use tracing::debug;

use crate::MemorySubstrate;

impl MemorySubstrate {
    /// Persist a fighter's manifest and status.
    pub async fn save_fighter(
        &self,
        id: &FighterId,
        manifest: &FighterManifest,
        status: FighterStatus,
    ) -> PunchResult<()> {
        let manifest_json = serde_json::to_string(manifest)
            .map_err(|e| PunchError::Memory(format!("failed to serialize manifest: {e}")))?;
        let id_str = id.to_string();
        let status_str = status.to_string();
        let name = manifest.name.clone();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO fighters (id, name, manifest, status, updated_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
            rusqlite::params![id_str, name, manifest_json, status_str],
        )
        .map_err(|e| PunchError::Memory(format!("failed to save fighter: {e}")))?;

        debug!(fighter_id = %id, "fighter saved");
        Ok(())
    }

    /// Load a fighter manifest by ID.
    pub async fn load_fighter(&self, id: &FighterId) -> PunchResult<Option<FighterManifest>> {
        let id_str = id.to_string();
        let conn = self.conn.lock().await;

        let result = conn.query_row(
            "SELECT manifest FROM fighters WHERE id = ?1",
            [&id_str],
            |row| {
                let json: String = row.get(0)?;
                Ok(json)
            },
        );

        match result {
            Ok(json) => {
                let manifest: FighterManifest = serde_json::from_str(&json)
                    .map_err(|e| PunchError::Memory(format!("corrupt fighter manifest: {e}")))?;
                Ok(Some(manifest))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(PunchError::Memory(format!("failed to load fighter: {e}"))),
        }
    }

    /// List all stored fighters as `(FighterId, name, FighterStatus)` tuples.
    pub async fn list_fighters(&self) -> PunchResult<Vec<(FighterId, String, FighterStatus)>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT id, name, status FROM fighters ORDER BY name")
            .map_err(|e| PunchError::Memory(format!("failed to list fighters: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let id_str: String = row.get(0)?;
                let name: String = row.get(1)?;
                let status_str: String = row.get(2)?;
                Ok((id_str, name, status_str))
            })
            .map_err(|e| PunchError::Memory(format!("failed to list fighters: {e}")))?;

        let mut fighters = Vec::new();
        for row in rows {
            let (id_str, name, status_str) =
                row.map_err(|e| PunchError::Memory(format!("failed to read fighter row: {e}")))?;

            let id = FighterId(
                uuid::Uuid::parse_str(&id_str)
                    .map_err(|e| PunchError::Memory(format!("invalid fighter id: {e}")))?,
            );
            let status = parse_fighter_status(&status_str)?;
            fighters.push((id, name, status));
        }

        Ok(fighters)
    }

    /// Update a fighter's operational status.
    pub async fn update_fighter_status(
        &self,
        id: &FighterId,
        status: FighterStatus,
    ) -> PunchResult<()> {
        let id_str = id.to_string();
        let status_str = status.to_string();
        let conn = self.conn.lock().await;

        let changed = conn
            .execute(
                "UPDATE fighters SET status = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?2",
                rusqlite::params![status_str, id_str],
            )
            .map_err(|e| PunchError::Memory(format!("failed to update fighter status: {e}")))?;

        if changed == 0 {
            return Err(PunchError::Fighter(format!("fighter {id} not found")));
        }

        debug!(fighter_id = %id, %status, "fighter status updated");
        Ok(())
    }

    /// Delete a fighter and all related data (cascading).
    pub async fn delete_fighter(&self, id: &FighterId) -> PunchResult<()> {
        let id_str = id.to_string();
        let conn = self.conn.lock().await;

        conn.execute("DELETE FROM fighters WHERE id = ?1", [&id_str])
            .map_err(|e| PunchError::Memory(format!("failed to delete fighter: {e}")))?;

        debug!(fighter_id = %id, "fighter deleted");
        Ok(())
    }
}

fn parse_fighter_status(s: &str) -> PunchResult<FighterStatus> {
    match s {
        "idle" => Ok(FighterStatus::Idle),
        "fighting" => Ok(FighterStatus::Fighting),
        "resting" => Ok(FighterStatus::Resting),
        "knocked_out" => Ok(FighterStatus::KnockedOut),
        "training" => Ok(FighterStatus::Training),
        other => Err(PunchError::Memory(format!(
            "unknown fighter status: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use punch_types::{FighterManifest, FighterStatus, ModelConfig, Provider, WeightClass};

    use crate::MemorySubstrate;

    fn test_manifest() -> FighterManifest {
        FighterManifest {
            name: "Test Fighter".into(),
            description: "A test fighter".into(),
            model: ModelConfig {
                provider: Provider::Anthropic,
                model: "claude-sonnet-4-20250514".into(),
                api_key_env: None,
                base_url: None,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
            system_prompt: "You are a test fighter.".into(),
            capabilities: Vec::new(),
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        }
    }

    #[tokio::test]
    async fn test_save_and_load_fighter() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let id = punch_types::FighterId::new();
        let manifest = test_manifest();

        substrate
            .save_fighter(&id, &manifest, FighterStatus::Idle)
            .await
            .unwrap();

        let loaded = substrate.load_fighter(&id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().name, "Test Fighter");
    }

    #[tokio::test]
    async fn test_list_fighters() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let id = punch_types::FighterId::new();
        let manifest = test_manifest();

        substrate
            .save_fighter(&id, &manifest, FighterStatus::Idle)
            .await
            .unwrap();

        let fighters = substrate.list_fighters().await.unwrap();
        assert_eq!(fighters.len(), 1);
        assert_eq!(fighters[0].1, "Test Fighter");
    }

    #[tokio::test]
    async fn test_load_nonexistent_fighter() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let id = punch_types::FighterId::new();
        let loaded = substrate.load_fighter(&id).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_list_multiple_fighters() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let id1 = punch_types::FighterId::new();
        let id2 = punch_types::FighterId::new();

        let mut m1 = test_manifest();
        m1.name = "Alpha Fighter".into();
        let mut m2 = test_manifest();
        m2.name = "Beta Fighter".into();

        substrate.save_fighter(&id1, &m1, FighterStatus::Idle).await.unwrap();
        substrate.save_fighter(&id2, &m2, FighterStatus::Fighting).await.unwrap();

        let fighters = substrate.list_fighters().await.unwrap();
        assert_eq!(fighters.len(), 2);
    }

    #[tokio::test]
    async fn test_update_nonexistent_fighter_status() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let id = punch_types::FighterId::new();
        let result = substrate.update_fighter_status(&id, FighterStatus::Fighting).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_and_verify_gone() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let id = punch_types::FighterId::new();
        substrate.save_fighter(&id, &test_manifest(), FighterStatus::Idle).await.unwrap();

        substrate.delete_fighter(&id).await.unwrap();
        let loaded = substrate.load_fighter(&id).await.unwrap();
        assert!(loaded.is_none());

        let fighters = substrate.list_fighters().await.unwrap();
        assert!(fighters.is_empty());
    }

    #[tokio::test]
    async fn test_save_fighter_overwrites() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let id = punch_types::FighterId::new();

        let mut m1 = test_manifest();
        m1.name = "Original".into();
        substrate.save_fighter(&id, &m1, FighterStatus::Idle).await.unwrap();

        let mut m2 = test_manifest();
        m2.name = "Updated".into();
        substrate.save_fighter(&id, &m2, FighterStatus::Fighting).await.unwrap();

        let loaded = substrate.load_fighter(&id).await.unwrap().unwrap();
        assert_eq!(loaded.name, "Updated");

        // Should still only be 1 fighter
        let fighters = substrate.list_fighters().await.unwrap();
        assert_eq!(fighters.len(), 1);
    }

    #[tokio::test]
    async fn test_update_and_delete_fighter() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let id = punch_types::FighterId::new();
        let manifest = test_manifest();

        substrate
            .save_fighter(&id, &manifest, FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .update_fighter_status(&id, FighterStatus::Fighting)
            .await
            .unwrap();

        substrate.delete_fighter(&id).await.unwrap();

        let loaded = substrate.load_fighter(&id).await.unwrap();
        assert!(loaded.is_none());
    }
}
