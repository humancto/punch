use rusqlite::Connection;
use tracing::info;

use punch_types::{PunchError, PunchResult};

/// Current schema version.
const CURRENT_VERSION: u32 = 1;

/// Run all pending migrations against `conn`.
pub fn migrate(conn: &Connection) -> PunchResult<()> {
    // Ensure the meta table exists so we can track the schema version.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _punch_meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| PunchError::Memory(format!("failed to create meta table: {e}")))?;

    let current = get_version(conn)?;

    if current >= CURRENT_VERSION {
        return Ok(());
    }

    if current < 1 {
        apply_v1(conn)?;
    }

    set_version(conn, CURRENT_VERSION)?;
    info!(version = CURRENT_VERSION, "migrations complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Version helpers
// ---------------------------------------------------------------------------

fn get_version(conn: &Connection) -> PunchResult<u32> {
    let mut stmt = conn
        .prepare("SELECT value FROM _punch_meta WHERE key = 'schema_version'")
        .map_err(|e| PunchError::Memory(format!("failed to query schema version: {e}")))?;

    let version: Option<String> = stmt.query_row([], |row| row.get(0)).ok();

    match version {
        Some(v) => v
            .parse::<u32>()
            .map_err(|e| PunchError::Memory(format!("invalid schema version: {e}"))),
        None => Ok(0),
    }
}

fn set_version(conn: &Connection, version: u32) -> PunchResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO _punch_meta (key, value) VALUES ('schema_version', ?1)",
        [version.to_string()],
    )
    .map_err(|e| PunchError::Memory(format!("failed to set schema version: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// V1 — initial schema
// ---------------------------------------------------------------------------

fn apply_v1(conn: &Connection) -> PunchResult<()> {
    info!("applying migration v1");

    conn.execute_batch(
        "
        -- Fighters (agents)
        CREATE TABLE IF NOT EXISTS fighters (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            manifest    TEXT NOT NULL,   -- JSON-serialised FighterManifest
            status      TEXT NOT NULL DEFAULT 'idle',
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        -- Bouts (sessions / conversations)
        CREATE TABLE IF NOT EXISTS bouts (
            id          TEXT PRIMARY KEY,
            fighter_id  TEXT NOT NULL REFERENCES fighters(id) ON DELETE CASCADE,
            title       TEXT,
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );
        CREATE INDEX IF NOT EXISTS idx_bouts_fighter ON bouts(fighter_id);

        -- Messages within a bout
        CREATE TABLE IF NOT EXISTS messages (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            bout_id     TEXT NOT NULL REFERENCES bouts(id) ON DELETE CASCADE,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL DEFAULT '',
            metadata    TEXT,           -- JSON for tool_calls / tool_results
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );
        CREATE INDEX IF NOT EXISTS idx_messages_bout ON messages(bout_id);

        -- Key-value memories per fighter
        CREATE TABLE IF NOT EXISTS memories (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            fighter_id  TEXT NOT NULL REFERENCES fighters(id) ON DELETE CASCADE,
            key         TEXT NOT NULL,
            value       TEXT NOT NULL,
            confidence  REAL NOT NULL DEFAULT 1.0,
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            accessed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            UNIQUE(fighter_id, key)
        );
        CREATE INDEX IF NOT EXISTS idx_memories_fighter ON memories(fighter_id);

        -- Knowledge graph: entities
        CREATE TABLE IF NOT EXISTS knowledge_entities (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            fighter_id  TEXT NOT NULL REFERENCES fighters(id) ON DELETE CASCADE,
            name        TEXT NOT NULL,
            entity_type TEXT NOT NULL,
            properties  TEXT NOT NULL DEFAULT '{}',  -- JSON
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            UNIQUE(fighter_id, name, entity_type)
        );
        CREATE INDEX IF NOT EXISTS idx_ke_fighter ON knowledge_entities(fighter_id);

        -- Knowledge graph: relations
        CREATE TABLE IF NOT EXISTS knowledge_relations (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            fighter_id  TEXT NOT NULL REFERENCES fighters(id) ON DELETE CASCADE,
            from_entity TEXT NOT NULL,
            relation    TEXT NOT NULL,
            to_entity   TEXT NOT NULL,
            properties  TEXT NOT NULL DEFAULT '{}',  -- JSON
            created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            UNIQUE(fighter_id, from_entity, relation, to_entity)
        );
        CREATE INDEX IF NOT EXISTS idx_kr_fighter ON knowledge_relations(fighter_id);

        -- Usage / metering events
        CREATE TABLE IF NOT EXISTS usage_events (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            fighter_id      TEXT NOT NULL REFERENCES fighters(id) ON DELETE CASCADE,
            model           TEXT NOT NULL,
            input_tokens    INTEGER NOT NULL,
            output_tokens   INTEGER NOT NULL,
            cost_usd        REAL NOT NULL DEFAULT 0.0,
            created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );
        CREATE INDEX IF NOT EXISTS idx_usage_fighter ON usage_events(fighter_id);

        -- Gorilla (autonomous agent) persistent state
        CREATE TABLE IF NOT EXISTS gorilla_state (
            gorilla_id  TEXT PRIMARY KEY,
            state       TEXT NOT NULL DEFAULT '{}',  -- arbitrary JSON blob
            updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );
        ",
    )
    .map_err(|e| PunchError::Memory(format!("migration v1 failed: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrate_fresh_db() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        assert_eq!(get_version(&conn).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn test_migrate_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(get_version(&conn).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn test_tables_exist_after_migration() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"fighters".to_string()));
        assert!(tables.contains(&"bouts".to_string()));
        assert!(tables.contains(&"messages".to_string()));
        assert!(tables.contains(&"memories".to_string()));
        assert!(tables.contains(&"knowledge_entities".to_string()));
        assert!(tables.contains(&"knowledge_relations".to_string()));
        assert!(tables.contains(&"usage_events".to_string()));
        assert!(tables.contains(&"gorilla_state".to_string()));
    }
}
