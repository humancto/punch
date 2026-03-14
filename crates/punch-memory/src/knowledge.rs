use serde::{Deserialize, Serialize};

use punch_types::{FighterId, PunchError, PunchResult};
use tracing::debug;

use crate::MemorySubstrate;

/// An entity in a fighter's knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntity {
    pub id: i64,
    pub name: String,
    pub entity_type: String,
    pub properties: serde_json::Value,
    pub created_at: String,
}

/// A directed relation between two entities in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeRelation {
    pub id: i64,
    pub from_entity: String,
    pub relation: String,
    pub to_entity: String,
    pub properties: serde_json::Value,
    pub created_at: String,
}

impl MemorySubstrate {
    /// Add (or upsert) an entity to a fighter's knowledge graph.
    pub async fn add_entity(
        &self,
        fighter_id: &FighterId,
        name: &str,
        entity_type: &str,
        properties: &serde_json::Value,
    ) -> PunchResult<()> {
        let fighter_str = fighter_id.to_string();
        let props_str = properties.to_string();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO knowledge_entities (fighter_id, name, entity_type, properties)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(fighter_id, name, entity_type) DO UPDATE SET
                properties = excluded.properties",
            rusqlite::params![fighter_str, name, entity_type, props_str],
        )
        .map_err(|e| PunchError::KnowledgeGraph(format!("failed to add entity: {e}")))?;

        debug!(fighter_id = %fighter_id, name = name, "knowledge entity added");
        Ok(())
    }

    /// Add (or upsert) a relation between two entities.
    pub async fn add_relation(
        &self,
        fighter_id: &FighterId,
        from: &str,
        relation: &str,
        to: &str,
        properties: &serde_json::Value,
    ) -> PunchResult<()> {
        let fighter_str = fighter_id.to_string();
        let props_str = properties.to_string();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO knowledge_relations (fighter_id, from_entity, relation, to_entity, properties)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(fighter_id, from_entity, relation, to_entity) DO UPDATE SET
                properties = excluded.properties",
            rusqlite::params![fighter_str, from, relation, to, props_str],
        )
        .map_err(|e| PunchError::KnowledgeGraph(format!("failed to add relation: {e}")))?;

        debug!(fighter_id = %fighter_id, from = from, relation = relation, to = to, "knowledge relation added");
        Ok(())
    }

    /// Query entities matching a name or type substring.
    pub async fn query_entities(
        &self,
        fighter_id: &FighterId,
        query: &str,
    ) -> PunchResult<Vec<KnowledgeEntity>> {
        let fighter_str = fighter_id.to_string();
        let pattern = format!("%{query}%");

        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, entity_type, properties, created_at
                 FROM knowledge_entities
                 WHERE fighter_id = ?1 AND (name LIKE ?2 OR entity_type LIKE ?2)
                 ORDER BY name",
            )
            .map_err(|e| PunchError::KnowledgeGraph(format!("failed to query entities: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![fighter_str, pattern], |row| {
                let id: i64 = row.get(0)?;
                let name: String = row.get(1)?;
                let entity_type: String = row.get(2)?;
                let props: String = row.get(3)?;
                let created_at: String = row.get(4)?;
                Ok((id, name, entity_type, props, created_at))
            })
            .map_err(|e| PunchError::KnowledgeGraph(format!("failed to query entities: {e}")))?;

        let mut entities = Vec::new();
        for row in rows {
            let (id, name, entity_type, props, created_at) = row.map_err(|e| {
                PunchError::KnowledgeGraph(format!("failed to read entity row: {e}"))
            })?;

            let properties: serde_json::Value = serde_json::from_str(&props).map_err(|e| {
                PunchError::KnowledgeGraph(format!("corrupt entity properties: {e}"))
            })?;

            entities.push(KnowledgeEntity {
                id,
                name,
                entity_type,
                properties,
                created_at,
            });
        }

        Ok(entities)
    }

    /// Query all relations involving a given entity (as source or target).
    pub async fn query_relations(
        &self,
        fighter_id: &FighterId,
        entity: &str,
    ) -> PunchResult<Vec<KnowledgeRelation>> {
        let fighter_str = fighter_id.to_string();

        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, from_entity, relation, to_entity, properties, created_at
                 FROM knowledge_relations
                 WHERE fighter_id = ?1 AND (from_entity = ?2 OR to_entity = ?2)
                 ORDER BY relation",
            )
            .map_err(|e| PunchError::KnowledgeGraph(format!("failed to query relations: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![fighter_str, entity], |row| {
                let id: i64 = row.get(0)?;
                let from_entity: String = row.get(1)?;
                let relation: String = row.get(2)?;
                let to_entity: String = row.get(3)?;
                let props: String = row.get(4)?;
                let created_at: String = row.get(5)?;
                Ok((id, from_entity, relation, to_entity, props, created_at))
            })
            .map_err(|e| PunchError::KnowledgeGraph(format!("failed to query relations: {e}")))?;

        let mut relations = Vec::new();
        for row in rows {
            let (id, from_entity, relation, to_entity, props, created_at) = row.map_err(|e| {
                PunchError::KnowledgeGraph(format!("failed to read relation row: {e}"))
            })?;

            let properties: serde_json::Value = serde_json::from_str(&props).map_err(|e| {
                PunchError::KnowledgeGraph(format!("corrupt relation properties: {e}"))
            })?;

            relations.push(KnowledgeRelation {
                id,
                from_entity,
                relation,
                to_entity,
                properties,
                created_at,
            });
        }

        Ok(relations)
    }
}

#[cfg(test)]
mod tests {
    use punch_types::{FighterManifest, FighterStatus, ModelConfig, Provider, WeightClass};

    use crate::MemorySubstrate;

    fn test_manifest() -> FighterManifest {
        FighterManifest {
            name: "KG Fighter".into(),
            description: "knowledge graph test".into(),
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
    async fn test_add_and_query_entities() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .add_entity(&fid, "Rust", "language", &serde_json::json!({"year": 2010}))
            .await
            .unwrap();

        let entities = substrate.query_entities(&fid, "Rust").await.unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].entity_type, "language");
    }

    #[tokio::test]
    async fn test_add_and_query_relations() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let fid = punch_types::FighterId::new();
        substrate
            .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
            .await
            .unwrap();

        substrate
            .add_entity(&fid, "Alice", "person", &serde_json::json!({}))
            .await
            .unwrap();
        substrate
            .add_entity(&fid, "Bob", "person", &serde_json::json!({}))
            .await
            .unwrap();
        substrate
            .add_relation(
                &fid,
                "Alice",
                "knows",
                "Bob",
                &serde_json::json!({"since": 2020}),
            )
            .await
            .unwrap();

        let relations = substrate.query_relations(&fid, "Alice").await.unwrap();
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].relation, "knows");
        assert_eq!(relations[0].to_entity, "Bob");
    }
}
