use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use punch_types::{FighterId, Message, PunchError, PunchResult, Role};
use tracing::debug;

use crate::MemorySubstrate;

/// Unique identifier for a Bout (session / conversation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BoutId(pub Uuid);

impl BoutId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for BoutId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for BoutId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lightweight summary of a bout for listing purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoutSummary {
    pub id: BoutId,
    pub fighter_id: FighterId,
    pub title: Option<String>,
    pub message_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

impl MemorySubstrate {
    /// Create a new bout for the given fighter and return its ID.
    pub async fn create_bout(&self, fighter_id: &FighterId) -> PunchResult<BoutId> {
        let bout_id = BoutId::new();
        let bout_str = bout_id.to_string();
        let fighter_str = fighter_id.to_string();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO bouts (id, fighter_id) VALUES (?1, ?2)",
            rusqlite::params![bout_str, fighter_str],
        )
        .map_err(|e| PunchError::Bout(format!("failed to create bout: {e}")))?;

        debug!(bout_id = %bout_id, fighter_id = %fighter_id, "bout created");
        Ok(bout_id)
    }

    /// Append a message to an existing bout.
    pub async fn save_message(&self, bout_id: &BoutId, message: &Message) -> PunchResult<()> {
        let bout_str = bout_id.to_string();
        let role_str = message.role.to_string();

        // Pack tool_calls and tool_results into a metadata JSON blob.
        let metadata = if message.tool_calls.is_empty() && message.tool_results.is_empty() {
            None
        } else {
            Some(serde_json::json!({
                "tool_calls": message.tool_calls,
                "tool_results": message.tool_results,
            }))
        };
        let metadata_str = metadata.map(|m| m.to_string());
        let ts = message.timestamp.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO messages (bout_id, role, content, metadata, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![bout_str, role_str, message.content, metadata_str, ts],
        )
        .map_err(|e| PunchError::Bout(format!("failed to save message: {e}")))?;

        // Touch the bout's updated_at timestamp.
        conn.execute(
            "UPDATE bouts SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
            [&bout_str],
        )
        .map_err(|e| PunchError::Bout(format!("failed to touch bout: {e}")))?;

        Ok(())
    }

    /// Load all messages for a bout in chronological order.
    pub async fn load_messages(&self, bout_id: &BoutId) -> PunchResult<Vec<Message>> {
        let bout_str = bout_id.to_string();
        let conn = self.conn.lock().await;

        let mut stmt = conn
            .prepare(
                "SELECT role, content, metadata, created_at FROM messages WHERE bout_id = ?1 ORDER BY id",
            )
            .map_err(|e| PunchError::Bout(format!("failed to prepare message query: {e}")))?;

        let rows = stmt
            .query_map([&bout_str], |row| {
                let role_str: String = row.get(0)?;
                let content: String = row.get(1)?;
                let metadata: Option<String> = row.get(2)?;
                let created_at: String = row.get(3)?;
                Ok((role_str, content, metadata, created_at))
            })
            .map_err(|e| PunchError::Bout(format!("failed to query messages: {e}")))?;

        let mut messages = Vec::new();
        for row in rows {
            let (role_str, content, metadata, created_at) =
                row.map_err(|e| PunchError::Bout(format!("failed to read message row: {e}")))?;

            let role = parse_role(&role_str)?;
            let timestamp = parse_timestamp(&created_at)?;

            let (tool_calls, tool_results) = match metadata {
                Some(json) => {
                    let v: serde_json::Value = serde_json::from_str(&json)
                        .map_err(|e| PunchError::Bout(format!("corrupt message metadata: {e}")))?;
                    let tc = serde_json::from_value(
                        v.get("tool_calls")
                            .cloned()
                            .unwrap_or(serde_json::Value::Array(vec![])),
                    )
                    .unwrap_or_default();
                    let tr = serde_json::from_value(
                        v.get("tool_results")
                            .cloned()
                            .unwrap_or(serde_json::Value::Array(vec![])),
                    )
                    .unwrap_or_default();
                    (tc, tr)
                }
                None => (Vec::new(), Vec::new()),
            };

            messages.push(Message {
                role,
                content,
                tool_calls,
                tool_results,
                timestamp,
            });
        }

        Ok(messages)
    }

    /// List all bouts for a fighter, most recent first.
    pub async fn list_bouts(&self, fighter_id: &FighterId) -> PunchResult<Vec<BoutSummary>> {
        let fighter_str = fighter_id.to_string();
        let conn = self.conn.lock().await;

        let mut stmt = conn
            .prepare(
                "SELECT b.id, b.title, b.created_at, b.updated_at,
                        (SELECT COUNT(*) FROM messages m WHERE m.bout_id = b.id)
                 FROM bouts b
                 WHERE b.fighter_id = ?1
                 ORDER BY b.updated_at DESC",
            )
            .map_err(|e| PunchError::Bout(format!("failed to list bouts: {e}")))?;

        let rows = stmt
            .query_map([&fighter_str], |row| {
                let id: String = row.get(0)?;
                let title: Option<String> = row.get(1)?;
                let created_at: String = row.get(2)?;
                let updated_at: String = row.get(3)?;
                let message_count: u64 = row.get(4)?;
                Ok((id, title, created_at, updated_at, message_count))
            })
            .map_err(|e| PunchError::Bout(format!("failed to list bouts: {e}")))?;

        let mut summaries = Vec::new();
        for row in rows {
            let (id, title, created_at, updated_at, message_count) =
                row.map_err(|e| PunchError::Bout(format!("failed to read bout row: {e}")))?;

            let bout_id = BoutId(
                Uuid::parse_str(&id)
                    .map_err(|e| PunchError::Bout(format!("invalid bout id: {e}")))?,
            );

            summaries.push(BoutSummary {
                id: bout_id,
                fighter_id: *fighter_id,
                title,
                message_count,
                created_at,
                updated_at,
            });
        }

        Ok(summaries)
    }

    /// Delete a bout and all its messages (cascading).
    pub async fn delete_bout(&self, bout_id: &BoutId) -> PunchResult<()> {
        let bout_str = bout_id.to_string();
        let conn = self.conn.lock().await;

        conn.execute("DELETE FROM bouts WHERE id = ?1", [&bout_str])
            .map_err(|e| PunchError::Bout(format!("failed to delete bout: {e}")))?;

        debug!(bout_id = %bout_id, "bout deleted");
        Ok(())
    }
}

fn parse_role(s: &str) -> PunchResult<Role> {
    match s {
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        "system" => Ok(Role::System),
        "tool" => Ok(Role::Tool),
        other => Err(PunchError::Bout(format!("unknown role: {other}"))),
    }
}

fn parse_timestamp(s: &str) -> PunchResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ").map(|ndt| ndt.and_utc())
        })
        .map_err(|e| PunchError::Bout(format!("invalid timestamp '{s}': {e}")))
}

#[cfg(test)]
mod tests {
    use punch_types::{
        FighterManifest, FighterStatus, Message, ModelConfig, Provider, Role, WeightClass,
    };

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
    async fn test_create_bout_and_messages() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = punch_types::FighterId::new();

        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        let bout_id = substrate.create_bout(&fighter_id).await.unwrap();

        let msg = Message::new(Role::User, "Hello, fighter!");
        substrate.save_message(&bout_id, &msg).await.unwrap();

        let messages = substrate.load_messages(&bout_id).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello, fighter!");
        assert_eq!(messages[0].role, Role::User);
    }

    #[tokio::test]
    async fn test_multiple_messages_in_bout() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = punch_types::FighterId::new();
        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();
        let bout_id = substrate.create_bout(&fighter_id).await.unwrap();

        substrate
            .save_message(&bout_id, &Message::new(Role::User, "Hello"))
            .await
            .unwrap();
        substrate
            .save_message(&bout_id, &Message::new(Role::Assistant, "Hi there"))
            .await
            .unwrap();
        substrate
            .save_message(&bout_id, &Message::new(Role::User, "How are you?"))
            .await
            .unwrap();

        let messages = substrate.load_messages(&bout_id).await.unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].role, Role::Assistant);
        assert_eq!(messages[2].content, "How are you?");
    }

    #[tokio::test]
    async fn test_load_messages_empty_bout() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = punch_types::FighterId::new();
        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();
        let bout_id = substrate.create_bout(&fighter_id).await.unwrap();

        let messages = substrate.load_messages(&bout_id).await.unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_bouts_for_fighter() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = punch_types::FighterId::new();
        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate.create_bout(&fighter_id).await.unwrap();
        substrate.create_bout(&fighter_id).await.unwrap();
        substrate.create_bout(&fighter_id).await.unwrap();

        let bouts = substrate.list_bouts(&fighter_id).await.unwrap();
        assert_eq!(bouts.len(), 3);
    }

    #[tokio::test]
    async fn test_bout_summary_message_count() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = punch_types::FighterId::new();
        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();
        let bout_id = substrate.create_bout(&fighter_id).await.unwrap();

        substrate
            .save_message(&bout_id, &Message::new(Role::User, "a"))
            .await
            .unwrap();
        substrate
            .save_message(&bout_id, &Message::new(Role::Assistant, "b"))
            .await
            .unwrap();

        let bouts = substrate.list_bouts(&fighter_id).await.unwrap();
        assert_eq!(bouts[0].message_count, 2);
    }

    #[tokio::test]
    async fn test_bout_id_display() {
        let bout_id = super::BoutId::new();
        let s = bout_id.to_string();
        assert!(!s.is_empty());
        // Should be a valid UUID string
        assert!(uuid::Uuid::parse_str(&s).is_ok());
    }

    #[tokio::test]
    async fn test_delete_bout_cascades_messages() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = punch_types::FighterId::new();
        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();
        let bout_id = substrate.create_bout(&fighter_id).await.unwrap();
        substrate
            .save_message(&bout_id, &Message::new(Role::User, "msg"))
            .await
            .unwrap();

        substrate.delete_bout(&bout_id).await.unwrap();
        let bouts = substrate.list_bouts(&fighter_id).await.unwrap();
        assert!(bouts.is_empty());
    }

    #[tokio::test]
    async fn test_list_and_delete_bouts() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fighter_id = punch_types::FighterId::new();

        substrate
            .save_fighter(&fighter_id, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        let bout_id = substrate.create_bout(&fighter_id).await.unwrap();

        let bouts = substrate.list_bouts(&fighter_id).await.unwrap();
        assert_eq!(bouts.len(), 1);

        substrate.delete_bout(&bout_id).await.unwrap();

        let bouts = substrate.list_bouts(&fighter_id).await.unwrap();
        assert!(bouts.is_empty());
    }
}
