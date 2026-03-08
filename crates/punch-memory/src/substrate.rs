use std::path::Path;

use rusqlite::Connection;
use tokio::sync::Mutex;
use tracing::info;

use punch_types::PunchResult;

use crate::migrations;

/// The core persistence handle for Punch.
///
/// Wraps a SQLite [`Connection`] behind a [`tokio::sync::Mutex`] so it can be
/// shared across async tasks without blocking the executor.
pub struct MemorySubstrate {
    pub(crate) conn: Mutex<Connection>,
}

impl MemorySubstrate {
    /// Open (or create) a SQLite database at `path` and run pending migrations.
    pub fn new(path: &Path) -> PunchResult<Self> {
        let conn = Connection::open(path).map_err(|e| {
            punch_types::PunchError::Memory(format!("failed to open database: {e}"))
        })?;

        // Enable WAL mode for better concurrent-read performance.
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(|e| punch_types::PunchError::Memory(format!("failed to set pragmas: {e}")))?;

        migrations::migrate(&conn)?;

        info!(path = %path.display(), "memory substrate initialized");

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Get a lock on the underlying database connection.
    ///
    /// This is intended for advanced queries that don't have a dedicated method.
    /// Prefer using the higher-level methods on `MemorySubstrate` when possible.
    pub async fn conn(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        self.conn.lock().await
    }

    /// Create an in-memory substrate (useful for testing).
    pub fn in_memory() -> PunchResult<Self> {
        let conn = Connection::open_in_memory().map_err(|e| {
            punch_types::PunchError::Memory(format!("failed to open in-memory database: {e}"))
        })?;

        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|e| punch_types::PunchError::Memory(format!("failed to set pragmas: {e}")))?;

        migrations::migrate(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}
