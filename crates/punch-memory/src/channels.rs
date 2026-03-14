//! Channel persistence -- stores and loads channel configuration records.
//!
//! Channel records are stored in the `channels` table, keyed by name
//! so they survive restarts and can be managed via the CLI or API.

use serde::{Deserialize, Serialize};
use tracing::debug;

use punch_types::{PunchError, PunchResult};

use crate::MemorySubstrate;

/// A persisted channel configuration record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    /// Unique identifier for this channel.
    pub id: String,
    /// Human-readable name for this channel (unique).
    pub name: String,
    /// Platform identifier (e.g., "telegram", "slack").
    pub platform: String,
    /// JSON-encoded credentials.
    pub credentials: String,
    /// JSON-encoded settings.
    pub settings: String,
    /// Connection status (e.g., "connected", "disconnected", "error").
    pub status: String,
    /// When credentials were last validated.
    pub validated_at: Option<String>,
    /// When the record was created.
    pub created_at: String,
    /// When the record was last updated.
    pub updated_at: String,
}

impl MemorySubstrate {
    /// Save or update a channel record. Uses name as the natural key.
    pub async fn save_channel(&self, record: &ChannelRecord) -> PunchResult<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO channels (id, name, platform, credentials, settings, status, validated_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(name) DO UPDATE SET
                platform = excluded.platform,
                credentials = excluded.credentials,
                settings = excluded.settings,
                status = excluded.status,
                validated_at = excluded.validated_at,
                updated_at = excluded.updated_at",
            rusqlite::params![
                record.id,
                record.name,
                record.platform,
                record.credentials,
                record.settings,
                record.status,
                record.validated_at,
                record.created_at,
                record.updated_at,
            ],
        )
        .map_err(|e| PunchError::Memory(format!("failed to save channel: {e}")))?;

        debug!(name = %record.name, platform = %record.platform, "channel saved");
        Ok(())
    }

    /// Load a channel by name. Returns None if no channel exists.
    pub async fn load_channel(&self, name: &str) -> PunchResult<Option<ChannelRecord>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, platform, credentials, settings, status, validated_at, created_at, updated_at
                 FROM channels WHERE name = ?1",
            )
            .map_err(|e| PunchError::Memory(format!("failed to prepare channel query: {e}")))?;

        let result = stmt
            .query_row([name], |row| {
                Ok(ChannelRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    platform: row.get(2)?,
                    credentials: row.get(3)?,
                    settings: row.get(4)?,
                    status: row.get(5)?,
                    validated_at: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .ok();

        Ok(result)
    }

    /// List all channel records.
    pub async fn list_channels(&self) -> PunchResult<Vec<ChannelRecord>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, platform, credentials, settings, status, validated_at, created_at, updated_at
                 FROM channels ORDER BY created_at DESC",
            )
            .map_err(|e| PunchError::Memory(format!("failed to list channels: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(ChannelRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    platform: row.get(2)?,
                    credentials: row.get(3)?,
                    settings: row.get(4)?,
                    status: row.get(5)?,
                    validated_at: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .map_err(|e| PunchError::Memory(format!("failed to read channel rows: {e}")))?;

        let mut channels = Vec::new();
        for row in rows {
            let record =
                row.map_err(|e| PunchError::Memory(format!("failed to read channel: {e}")))?;
            channels.push(record);
        }
        Ok(channels)
    }

    /// Delete a channel by name.
    pub async fn delete_channel(&self, name: &str) -> PunchResult<()> {
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM channels WHERE name = ?1", [name])
            .map_err(|e| PunchError::Memory(format!("failed to delete channel: {e}")))?;
        debug!(name = name, "channel deleted");
        Ok(())
    }

    /// Update the status of a channel by name.
    pub async fn update_channel_status(&self, name: &str, status: &str) -> PunchResult<()> {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE channels SET status = ?1, updated_at = ?2 WHERE name = ?3",
            rusqlite::params![status, now, name],
        )
        .map_err(|e| PunchError::Memory(format!("failed to update channel status: {e}")))?;
        debug!(name = name, status = status, "channel status updated");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySubstrate;

    fn make_record(name: &str, platform: &str) -> ChannelRecord {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        ChannelRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            platform: platform.to_string(),
            credentials: "{}".to_string(),
            settings: "{}".to_string(),
            status: "disconnected".to_string(),
            validated_at: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_save_and_load_channel() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let record = make_record("my-telegram", "telegram");

        substrate.save_channel(&record).await.unwrap();
        let loaded = substrate.load_channel("my-telegram").await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.name, "my-telegram");
        assert_eq!(loaded.platform, "telegram");
        assert_eq!(loaded.status, "disconnected");
    }

    #[tokio::test]
    async fn test_save_channel_upsert() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let mut record = make_record("my-slack", "slack");
        substrate.save_channel(&record).await.unwrap();

        record.status = "connected".to_string();
        substrate.save_channel(&record).await.unwrap();

        let loaded = substrate.load_channel("my-slack").await.unwrap().unwrap();
        assert_eq!(loaded.status, "connected");
    }

    #[tokio::test]
    async fn test_list_channels() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        substrate
            .save_channel(&make_record("ch-1", "telegram"))
            .await
            .unwrap();
        substrate
            .save_channel(&make_record("ch-2", "slack"))
            .await
            .unwrap();
        substrate
            .save_channel(&make_record("ch-3", "discord"))
            .await
            .unwrap();

        let channels = substrate.list_channels().await.unwrap();
        assert_eq!(channels.len(), 3);
    }

    #[tokio::test]
    async fn test_delete_channel() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        substrate
            .save_channel(&make_record("to-delete", "telegram"))
            .await
            .unwrap();

        assert!(substrate.load_channel("to-delete").await.unwrap().is_some());

        substrate.delete_channel("to-delete").await.unwrap();
        assert!(substrate.load_channel("to-delete").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_update_channel_status() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        substrate
            .save_channel(&make_record("status-test", "discord"))
            .await
            .unwrap();

        substrate
            .update_channel_status("status-test", "connected")
            .await
            .unwrap();

        let loaded = substrate
            .load_channel("status-test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.status, "connected");
    }

    #[tokio::test]
    async fn test_load_nonexistent_channel() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let result = substrate.load_channel("nonexistent").await.unwrap();
        assert!(result.is_none());
    }
}
