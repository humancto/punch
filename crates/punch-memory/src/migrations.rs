//! Database migration engine for the Punch memory substrate.
//!
//! Tracks applied migrations in a `_punch_migrations` table and supports
//! forward (up) and backward (down) migration with SHA-256 checksum
//! verification for integrity.

use std::sync::Arc;

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tracing::info;

use punch_types::{PunchError, PunchResult};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single database migration with up and down SQL.
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: u64,
    pub name: String,
    pub up_sql: String,
    pub down_sql: String,
}

impl Migration {
    /// Compute the SHA-256 hex digest of the `up_sql` content.
    pub fn checksum(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.up_sql.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

/// Status of a single migration (applied or pending).
#[derive(Debug, Clone)]
pub struct MigrationStatus {
    pub version: u64,
    pub name: String,
    pub applied: bool,
    pub applied_at: Option<String>,
}

/// The migration engine manages schema versioning for a SQLite database.
pub struct MigrationEngine {
    /// Database connection.
    conn: Arc<std::sync::Mutex<Connection>>,
}

impl MigrationEngine {
    /// Create a new engine and ensure the tracking table exists.
    pub fn new(conn: Arc<std::sync::Mutex<Connection>>) -> PunchResult<Self> {
        {
            let c = conn
                .lock()
                .map_err(|e| PunchError::Memory(format!("failed to lock connection: {e}")))?;
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS _punch_migrations (
                    id         INTEGER PRIMARY KEY,
                    version    INTEGER NOT NULL UNIQUE,
                    name       TEXT NOT NULL,
                    applied_at TEXT NOT NULL,
                    checksum   TEXT NOT NULL
                );",
            )
            .map_err(|e| PunchError::Memory(format!("failed to create migrations table: {e}")))?;
        }
        Ok(Self { conn })
    }

    /// Return the highest applied migration version, or 0 if none.
    pub fn current_version(&self) -> PunchResult<u64> {
        let c = self
            .conn
            .lock()
            .map_err(|e| PunchError::Memory(format!("failed to lock connection: {e}")))?;
        let version: Option<u64> = c
            .query_row("SELECT MAX(version) FROM _punch_migrations", [], |row| {
                row.get(0)
            })
            .map_err(|e| PunchError::Memory(format!("failed to query current version: {e}")))?;
        Ok(version.unwrap_or(0))
    }

    /// Return migrations from `all` that have not yet been applied, sorted by
    /// version ascending.
    pub fn pending_migrations<'a>(&self, all: &'a [Migration]) -> PunchResult<Vec<&'a Migration>> {
        let applied = self.applied_versions()?;
        let mut pending: Vec<&Migration> = all
            .iter()
            .filter(|m| !applied.contains(&m.version))
            .collect();
        pending.sort_by_key(|m| m.version);
        Ok(pending)
    }

    /// Apply all pending migrations in order. Each migration runs inside its
    /// own transaction. Returns the versions that were applied.
    pub fn migrate_up(&self, migrations: &[Migration]) -> PunchResult<Vec<u64>> {
        let pending = self.pending_migrations(migrations)?;
        let mut applied = Vec::new();

        for migration in pending {
            let c = self
                .conn
                .lock()
                .map_err(|e| PunchError::Memory(format!("failed to lock connection: {e}")))?;
            let tx = c
                .unchecked_transaction()
                .map_err(|e| PunchError::Memory(format!("failed to begin transaction: {e}")))?;

            tx.execute_batch(&migration.up_sql).map_err(|e| {
                PunchError::Memory(format!(
                    "migration v{} ({}) failed: {e}",
                    migration.version, migration.name
                ))
            })?;

            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            tx.execute(
                "INSERT INTO _punch_migrations (version, name, applied_at, checksum)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![migration.version, migration.name, now, migration.checksum(),],
            )
            .map_err(|e| {
                PunchError::Memory(format!(
                    "failed to record migration v{}: {e}",
                    migration.version
                ))
            })?;

            tx.commit().map_err(|e| {
                PunchError::Memory(format!(
                    "failed to commit migration v{}: {e}",
                    migration.version
                ))
            })?;

            info!(version = migration.version, name = %migration.name, "applied migration");
            applied.push(migration.version);
        }

        Ok(applied)
    }

    /// Roll back applied migrations whose version is greater than
    /// `target_version`, in reverse order. Returns the versions that were
    /// rolled back.
    pub fn migrate_down(
        &self,
        migrations: &[Migration],
        target_version: u64,
    ) -> PunchResult<Vec<u64>> {
        let applied = self.applied_versions()?;

        // Collect migrations that need to be rolled back, sorted descending.
        let mut to_rollback: Vec<&Migration> = migrations
            .iter()
            .filter(|m| m.version > target_version && applied.contains(&m.version))
            .collect();
        to_rollback.sort_by(|a, b| b.version.cmp(&a.version));

        let mut rolled_back = Vec::new();

        for migration in to_rollback {
            let c = self
                .conn
                .lock()
                .map_err(|e| PunchError::Memory(format!("failed to lock connection: {e}")))?;
            let tx = c
                .unchecked_transaction()
                .map_err(|e| PunchError::Memory(format!("failed to begin transaction: {e}")))?;

            tx.execute_batch(&migration.down_sql).map_err(|e| {
                PunchError::Memory(format!(
                    "down migration v{} ({}) failed: {e}",
                    migration.version, migration.name
                ))
            })?;

            tx.execute(
                "DELETE FROM _punch_migrations WHERE version = ?1",
                [migration.version],
            )
            .map_err(|e| {
                PunchError::Memory(format!(
                    "failed to remove migration record v{}: {e}",
                    migration.version
                ))
            })?;

            tx.commit().map_err(|e| {
                PunchError::Memory(format!(
                    "failed to commit down migration v{}: {e}",
                    migration.version
                ))
            })?;

            info!(version = migration.version, name = %migration.name, "rolled back migration");
            rolled_back.push(migration.version);
        }

        Ok(rolled_back)
    }

    /// Show the status (applied / pending) of every known migration.
    pub fn migration_status(&self, migrations: &[Migration]) -> PunchResult<Vec<MigrationStatus>> {
        let c = self
            .conn
            .lock()
            .map_err(|e| PunchError::Memory(format!("failed to lock connection: {e}")))?;

        let mut stmt = c
            .prepare("SELECT version, applied_at FROM _punch_migrations")
            .map_err(|e| PunchError::Memory(format!("failed to query migration status: {e}")))?;

        let rows: Vec<(u64, String)> = stmt
            .query_map([], |row| {
                let version: u64 = row.get(0)?;
                let applied_at: String = row.get(1)?;
                Ok((version, applied_at))
            })
            .map_err(|e| PunchError::Memory(format!("failed to read migration rows: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        let mut statuses: Vec<MigrationStatus> = migrations
            .iter()
            .map(|m| {
                let applied_row = rows.iter().find(|(v, _)| *v == m.version);
                MigrationStatus {
                    version: m.version,
                    name: m.name.clone(),
                    applied: applied_row.is_some(),
                    applied_at: applied_row.map(|(_, at)| at.clone()),
                }
            })
            .collect();
        statuses.sort_by_key(|s| s.version);
        Ok(statuses)
    }

    /// Verify that every applied migration's stored checksum matches the
    /// current `up_sql` content. Returns an error on the first mismatch.
    pub fn verify_checksums(&self, migrations: &[Migration]) -> PunchResult<()> {
        let c = self
            .conn
            .lock()
            .map_err(|e| PunchError::Memory(format!("failed to lock connection: {e}")))?;

        let mut stmt = c
            .prepare("SELECT version, checksum FROM _punch_migrations")
            .map_err(|e| PunchError::Memory(format!("failed to query checksums: {e}")))?;

        let rows: Vec<(u64, String)> = stmt
            .query_map([], |row| {
                let version: u64 = row.get(0)?;
                let checksum: String = row.get(1)?;
                Ok((version, checksum))
            })
            .map_err(|e| PunchError::Memory(format!("failed to read checksum rows: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        for (version, stored_checksum) in &rows {
            if let Some(migration) = migrations.iter().find(|m| m.version == *version) {
                let current_checksum = migration.checksum();
                if *stored_checksum != current_checksum {
                    return Err(PunchError::Memory(format!(
                        "checksum mismatch for migration v{} ({}): stored={}, current={}",
                        version, migration.name, stored_checksum, current_checksum
                    )));
                }
            }
        }

        Ok(())
    }

    /// Return the 6 built-in migrations that define the Punch schema.
    pub fn builtin_migrations() -> Vec<Migration> {
        vec![
            Migration {
                version: 1,
                name: "create_memories_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS memories (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    fighter_id  TEXT NOT NULL,
                    key         TEXT NOT NULL,
                    value       TEXT NOT NULL,
                    confidence  REAL NOT NULL DEFAULT 1.0,
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                    accessed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                    UNIQUE(fighter_id, key)
                );"
                .into(),
                down_sql: "DROP TABLE IF EXISTS memories;".into(),
            },
            Migration {
                version: 2,
                name: "create_entities_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS knowledge_entities (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    fighter_id  TEXT NOT NULL,
                    name        TEXT NOT NULL,
                    entity_type TEXT NOT NULL,
                    properties  TEXT NOT NULL DEFAULT '{}',
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                    UNIQUE(fighter_id, name, entity_type)
                );"
                .into(),
                down_sql: "DROP TABLE IF EXISTS knowledge_entities;".into(),
            },
            Migration {
                version: 3,
                name: "create_relations_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS knowledge_relations (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    fighter_id  TEXT NOT NULL,
                    from_entity TEXT NOT NULL,
                    relation    TEXT NOT NULL,
                    to_entity   TEXT NOT NULL,
                    properties  TEXT NOT NULL DEFAULT '{}',
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                    UNIQUE(fighter_id, from_entity, relation, to_entity)
                );"
                .into(),
                down_sql: "DROP TABLE IF EXISTS knowledge_relations;".into(),
            },
            Migration {
                version: 4,
                name: "create_bouts_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS bouts (
                    id          TEXT PRIMARY KEY,
                    fighter_id  TEXT NOT NULL,
                    title       TEXT,
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );"
                .into(),
                down_sql: "DROP TABLE IF EXISTS bouts;".into(),
            },
            Migration {
                version: 5,
                name: "create_bout_messages_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS messages (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    bout_id     TEXT NOT NULL,
                    role        TEXT NOT NULL,
                    content     TEXT NOT NULL DEFAULT '',
                    metadata    TEXT,
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );"
                .into(),
                down_sql: "DROP TABLE IF EXISTS messages;".into(),
            },
            Migration {
                version: 6,
                name: "add_indexes".into(),
                up_sql: "
                    CREATE INDEX IF NOT EXISTS idx_memories_fighter ON memories(fighter_id);
                    CREATE INDEX IF NOT EXISTS idx_ke_fighter ON knowledge_entities(fighter_id);
                    CREATE INDEX IF NOT EXISTS idx_kr_fighter ON knowledge_relations(fighter_id);
                    CREATE INDEX IF NOT EXISTS idx_bouts_fighter ON bouts(fighter_id);
                    CREATE INDEX IF NOT EXISTS idx_messages_bout ON messages(bout_id);
                "
                .into(),
                down_sql: "
                    DROP INDEX IF EXISTS idx_memories_fighter;
                    DROP INDEX IF EXISTS idx_ke_fighter;
                    DROP INDEX IF EXISTS idx_kr_fighter;
                    DROP INDEX IF EXISTS idx_bouts_fighter;
                    DROP INDEX IF EXISTS idx_messages_bout;
                "
                .into(),
            },
            Migration {
                version: 7,
                name: "create_fighters_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS fighters (
                    id          TEXT PRIMARY KEY,
                    name        TEXT NOT NULL,
                    manifest    TEXT NOT NULL,
                    status      TEXT NOT NULL DEFAULT 'idle',
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );"
                .into(),
                down_sql: "DROP TABLE IF EXISTS fighters;".into(),
            },
            Migration {
                version: 8,
                name: "create_usage_events_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS usage_events (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    fighter_id      TEXT NOT NULL,
                    model           TEXT NOT NULL,
                    input_tokens    INTEGER NOT NULL,
                    output_tokens   INTEGER NOT NULL,
                    cost_usd        REAL NOT NULL DEFAULT 0.0,
                    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );
                CREATE INDEX IF NOT EXISTS idx_usage_fighter ON usage_events(fighter_id);"
                    .into(),
                down_sql: "DROP INDEX IF EXISTS idx_usage_fighter;
                DROP TABLE IF EXISTS usage_events;"
                    .into(),
            },
            Migration {
                version: 9,
                name: "create_gorilla_state_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS gorilla_state (
                    gorilla_id  TEXT PRIMARY KEY,
                    state       TEXT NOT NULL DEFAULT '{}',
                    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );"
                .into(),
                down_sql: "DROP TABLE IF EXISTS gorilla_state;".into(),
            },
            Migration {
                version: 10,
                name: "create_embeddings_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS embeddings (
                    id         TEXT PRIMARY KEY,
                    text       TEXT NOT NULL,
                    vector     BLOB NOT NULL,
                    metadata   TEXT NOT NULL DEFAULT '{}',
                    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );"
                .into(),
                down_sql: "DROP TABLE IF EXISTS embeddings;".into(),
            },
            Migration {
                version: 11,
                name: "create_creeds_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS creeds (
                    id          TEXT PRIMARY KEY,
                    fighter_name TEXT NOT NULL,
                    fighter_id  TEXT,
                    creed_data  TEXT NOT NULL,
                    version     INTEGER NOT NULL DEFAULT 1,
                    bout_count  INTEGER NOT NULL DEFAULT 0,
                    message_count INTEGER NOT NULL DEFAULT 0,
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );
                CREATE UNIQUE INDEX IF NOT EXISTS idx_creeds_fighter_name ON creeds(fighter_name);
                CREATE INDEX IF NOT EXISTS idx_creeds_fighter_id ON creeds(fighter_id);"
                    .into(),
                down_sql: "DROP INDEX IF EXISTS idx_creeds_fighter_id;
                DROP INDEX IF EXISTS idx_creeds_fighter_name;
                DROP TABLE IF EXISTS creeds;"
                    .into(),
            },
            Migration {
                version: 12,
                name: "create_channels_table".into(),
                up_sql: "CREATE TABLE IF NOT EXISTS channels (
                    id              TEXT PRIMARY KEY,
                    name            TEXT NOT NULL UNIQUE,
                    platform        TEXT NOT NULL,
                    credentials     TEXT NOT NULL DEFAULT '{}',
                    settings        TEXT NOT NULL DEFAULT '{}',
                    status          TEXT NOT NULL DEFAULT 'disconnected',
                    validated_at    TEXT,
                    created_at      TEXT NOT NULL,
                    updated_at      TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_channels_platform ON channels(platform);"
                    .into(),
                down_sql: "DROP INDEX IF EXISTS idx_channels_platform;
                DROP TABLE IF EXISTS channels;"
                    .into(),
            },
        ]
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn applied_versions(&self) -> PunchResult<Vec<u64>> {
        let c = self
            .conn
            .lock()
            .map_err(|e| PunchError::Memory(format!("failed to lock connection: {e}")))?;
        let mut stmt = c
            .prepare("SELECT version FROM _punch_migrations ORDER BY version")
            .map_err(|e| PunchError::Memory(format!("failed to query applied versions: {e}")))?;

        let versions: Vec<u64> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| PunchError::Memory(format!("failed to read version rows: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(versions)
    }
}

// ---------------------------------------------------------------------------
// Legacy entry point — used by substrate.rs
// ---------------------------------------------------------------------------

/// Run all pending built-in migrations against `conn`.
///
/// This is the entry point called from [`crate::substrate::MemorySubstrate`]
/// during initialisation. It also handles migration from the old `_punch_meta`
/// version-tracking table if present.
pub fn migrate(conn: &Connection) -> PunchResult<()> {
    // If the old _punch_meta table exists, drop it — the new engine tracks
    // state in _punch_migrations.
    conn.execute_batch("DROP TABLE IF EXISTS _punch_meta;")
        .map_err(|e| PunchError::Memory(format!("failed to drop legacy meta table: {e}")))?;

    // Create the tracking table.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _punch_migrations (
            id         INTEGER PRIMARY KEY,
            version    INTEGER NOT NULL UNIQUE,
            name       TEXT NOT NULL,
            applied_at TEXT NOT NULL,
            checksum   TEXT NOT NULL
        );",
    )
    .map_err(|e| PunchError::Memory(format!("failed to create migrations table: {e}")))?;

    // Determine which versions have already been applied.
    let applied_versions = {
        let mut stmt = conn
            .prepare("SELECT version FROM _punch_migrations ORDER BY version")
            .map_err(|e| PunchError::Memory(format!("failed to query applied versions: {e}")))?;
        let versions: Vec<u64> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| PunchError::Memory(format!("failed to read version rows: {e}")))?
            .filter_map(|r| r.ok())
            .collect();
        versions
    };

    let builtins = MigrationEngine::builtin_migrations();
    let mut count = 0usize;

    for migration in &builtins {
        if applied_versions.contains(&migration.version) {
            continue;
        }

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| PunchError::Memory(format!("failed to begin transaction: {e}")))?;

        tx.execute_batch(&migration.up_sql).map_err(|e| {
            PunchError::Memory(format!(
                "migration v{} ({}) failed: {e}",
                migration.version, migration.name
            ))
        })?;

        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        tx.execute(
            "INSERT INTO _punch_migrations (version, name, applied_at, checksum)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![migration.version, migration.name, now, migration.checksum(),],
        )
        .map_err(|e| {
            PunchError::Memory(format!(
                "failed to record migration v{}: {e}",
                migration.version
            ))
        })?;

        tx.commit().map_err(|e| {
            PunchError::Memory(format!(
                "failed to commit migration v{}: {e}",
                migration.version
            ))
        })?;

        info!(version = migration.version, name = %migration.name, "applied migration");
        count += 1;
    }

    if count > 0 {
        info!(count, "migrations applied");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine() -> (MigrationEngine, Arc<std::sync::Mutex<Connection>>) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let arc = Arc::new(std::sync::Mutex::new(conn));
        let engine = MigrationEngine::new(Arc::clone(&arc)).unwrap();
        (engine, arc)
    }

    fn simple_migrations() -> Vec<Migration> {
        vec![
            Migration {
                version: 1,
                name: "create_alpha".into(),
                up_sql: "CREATE TABLE alpha (id INTEGER PRIMARY KEY, name TEXT);".into(),
                down_sql: "DROP TABLE IF EXISTS alpha;".into(),
            },
            Migration {
                version: 2,
                name: "create_beta".into(),
                up_sql: "CREATE TABLE beta (id INTEGER PRIMARY KEY, value TEXT);".into(),
                down_sql: "DROP TABLE IF EXISTS beta;".into(),
            },
            Migration {
                version: 3,
                name: "create_gamma".into(),
                up_sql: "CREATE TABLE gamma (id INTEGER PRIMARY KEY, score REAL);".into(),
                down_sql: "DROP TABLE IF EXISTS gamma;".into(),
            },
        ]
    }

    #[test]
    fn test_migration_table_creation() {
        let (engine, arc) = test_engine();
        // The tracking table should exist after new().
        {
            let c = arc.lock().unwrap();
            let count: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_punch_migrations'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }
        // No migrations applied yet.
        assert_eq!(engine.current_version().unwrap(), 0);
    }

    #[test]
    fn test_apply_single_migration() {
        let (engine, arc) = test_engine();
        let migrations = vec![simple_migrations().remove(0)];
        let applied = engine.migrate_up(&migrations).unwrap();
        assert_eq!(applied, vec![1]);

        let c = arc.lock().unwrap();
        let count: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='alpha'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_apply_multiple_migrations_in_order() {
        let (engine, _arc) = test_engine();
        let migrations = simple_migrations();
        let applied = engine.migrate_up(&migrations).unwrap();
        assert_eq!(applied, vec![1, 2, 3]);
        assert_eq!(engine.current_version().unwrap(), 3);
    }

    #[test]
    fn test_skip_already_applied_migrations() {
        let (engine, _arc) = test_engine();
        let migrations = simple_migrations();

        engine.migrate_up(&migrations).unwrap();
        let applied_again = engine.migrate_up(&migrations).unwrap();
        assert!(applied_again.is_empty());
    }

    #[test]
    fn test_rollback_to_specific_version() {
        let (engine, arc) = test_engine();
        let migrations = simple_migrations();

        engine.migrate_up(&migrations).unwrap();
        assert_eq!(engine.current_version().unwrap(), 3);

        let rolled_back = engine.migrate_down(&migrations, 1).unwrap();
        assert_eq!(rolled_back, vec![3, 2]);
        assert_eq!(engine.current_version().unwrap(), 1);

        // gamma and beta tables should be gone.
        let c = arc.lock().unwrap();
        let tables: Vec<String> = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name IN ('alpha','beta','gamma')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(tables, vec!["alpha".to_string()]);
    }

    #[test]
    fn test_current_version_tracking() {
        let (engine, _arc) = test_engine();
        assert_eq!(engine.current_version().unwrap(), 0);

        let migrations = simple_migrations();
        engine.migrate_up(&migrations[..1]).unwrap();
        assert_eq!(engine.current_version().unwrap(), 1);

        engine.migrate_up(&migrations).unwrap();
        assert_eq!(engine.current_version().unwrap(), 3);
    }

    #[test]
    fn test_pending_migration_detection() {
        let (engine, _arc) = test_engine();
        let migrations = simple_migrations();

        let pending = engine.pending_migrations(&migrations).unwrap();
        assert_eq!(pending.len(), 3);

        engine.migrate_up(&migrations[..2]).unwrap();

        let pending = engine.pending_migrations(&migrations).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].version, 3);
    }

    #[test]
    fn test_checksum_verification_passes() {
        let (engine, _arc) = test_engine();
        let migrations = simple_migrations();
        engine.migrate_up(&migrations).unwrap();

        // Verify with the same migrations — should succeed.
        engine.verify_checksums(&migrations).unwrap();
    }

    #[test]
    fn test_checksum_verification_fails_for_tampered() {
        let (engine, _arc) = test_engine();
        let migrations = simple_migrations();
        engine.migrate_up(&migrations).unwrap();

        // Tamper with a migration's up_sql.
        let mut tampered = simple_migrations();
        tampered[0].up_sql =
            "CREATE TABLE alpha (id INTEGER PRIMARY KEY, name TEXT, extra TEXT);".into();

        let result = engine.verify_checksums(&tampered);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("checksum mismatch"));
    }

    #[test]
    fn test_migration_status_listing() {
        let (engine, _arc) = test_engine();
        let migrations = simple_migrations();
        engine.migrate_up(&migrations[..2]).unwrap();

        let statuses = engine.migration_status(&migrations).unwrap();
        assert_eq!(statuses.len(), 3);

        assert!(statuses[0].applied);
        assert!(statuses[0].applied_at.is_some());
        assert_eq!(statuses[0].version, 1);

        assert!(statuses[1].applied);
        assert_eq!(statuses[1].version, 2);

        assert!(!statuses[2].applied);
        assert!(statuses[2].applied_at.is_none());
        assert_eq!(statuses[2].version, 3);
    }

    #[test]
    fn test_builtin_migrations_are_valid_sql() {
        let (engine, _arc) = test_engine();
        let builtins = MigrationEngine::builtin_migrations();

        // All built-in migrations should apply without error.
        let applied = engine.migrate_up(&builtins).unwrap();
        assert_eq!(applied.len(), 12);
        assert_eq!(engine.current_version().unwrap(), 12);
    }

    #[test]
    fn test_idempotent_migrate_up() {
        let (engine, _arc) = test_engine();
        let migrations = simple_migrations();

        let first = engine.migrate_up(&migrations).unwrap();
        assert_eq!(first.len(), 3);

        let second = engine.migrate_up(&migrations).unwrap();
        assert!(second.is_empty());

        // State unchanged.
        assert_eq!(engine.current_version().unwrap(), 3);
    }

    #[test]
    fn test_transaction_rollback_on_sql_error() {
        let (engine, _arc) = test_engine();

        let bad_migrations = vec![
            Migration {
                version: 1,
                name: "good".into(),
                up_sql: "CREATE TABLE good (id INTEGER PRIMARY KEY);".into(),
                down_sql: "DROP TABLE IF EXISTS good;".into(),
            },
            Migration {
                version: 2,
                name: "bad".into(),
                up_sql: "THIS IS NOT VALID SQL;".into(),
                down_sql: "SELECT 1;".into(),
            },
        ];

        // First migration succeeds, second fails.
        let result = engine.migrate_up(&bad_migrations);
        assert!(result.is_err());

        // Only version 1 should be applied.
        assert_eq!(engine.current_version().unwrap(), 1);
    }

    #[test]
    fn test_down_migration_ordering_reverse() {
        let (engine, _arc) = test_engine();
        let migrations = simple_migrations();

        engine.migrate_up(&migrations).unwrap();

        // Rolling back to 0 should go 3, 2, 1.
        let rolled = engine.migrate_down(&migrations, 0).unwrap();
        assert_eq!(rolled, vec![3, 2, 1]);
        assert_eq!(engine.current_version().unwrap(), 0);
    }

    #[test]
    fn test_empty_migration_list_handling() {
        let (engine, _arc) = test_engine();
        let empty: Vec<Migration> = vec![];

        let applied = engine.migrate_up(&empty).unwrap();
        assert!(applied.is_empty());

        let pending = engine.pending_migrations(&empty).unwrap();
        assert!(pending.is_empty());

        let statuses = engine.migration_status(&empty).unwrap();
        assert!(statuses.is_empty());

        let rolled = engine.migrate_down(&empty, 0).unwrap();
        assert!(rolled.is_empty());

        engine.verify_checksums(&empty).unwrap();
    }

    #[test]
    fn test_checksum_deterministic() {
        let m = Migration {
            version: 1,
            name: "test".into(),
            up_sql: "CREATE TABLE test (id INTEGER);".into(),
            down_sql: "DROP TABLE test;".into(),
        };
        let c1 = m.checksum();
        let c2 = m.checksum();
        assert_eq!(c1, c2);
        assert_eq!(c1.len(), 64); // SHA-256 hex is 64 chars
    }

    #[test]
    fn test_legacy_migrate_function() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        // The legacy migrate() function should work.
        migrate(&conn).unwrap();

        // Core tables from built-in migrations should exist.
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"memories".to_string()));
        assert!(tables.contains(&"knowledge_entities".to_string()));
        assert!(tables.contains(&"knowledge_relations".to_string()));
        assert!(tables.contains(&"bouts".to_string()));
        assert!(tables.contains(&"messages".to_string()));
        assert!(tables.contains(&"_punch_migrations".to_string()));
    }

    #[test]
    fn test_legacy_migrate_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();

        // Should still be version 9.
        let version: Option<u64> = conn
            .query_row("SELECT MAX(version) FROM _punch_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version.unwrap_or(0), 12);
    }
}
