use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use rusqlite::Connection;
use tokio::sync::Mutex;
use tracing::info;

use punch_types::PunchResult;

use crate::embeddings::{EmbeddingStore, BuiltInEmbedder, Embedder};
use crate::migrations;

/// The core persistence handle for Punch.
///
/// Wraps a SQLite [`Connection`] behind a [`tokio::sync::Mutex`] so it can be
/// shared across async tasks without blocking the executor. Optionally includes
/// an [`EmbeddingStore`] for semantic search over stored memories.
pub struct MemorySubstrate {
    pub(crate) conn: Mutex<Connection>,
    /// Optional embedding store for semantic recall.
    embedding_store: Option<StdMutex<EmbeddingStore>>,
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
            embedding_store: None,
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
            embedding_store: None,
        })
    }

    /// Attach an embedding store with the given embedder for semantic recall.
    ///
    /// The embedding store shares a *separate* SQLite connection (via
    /// `std::sync::Mutex`) since it operates synchronously.
    pub fn with_embedding_store(
        mut self,
        conn: Arc<StdMutex<Connection>>,
        embedder: Box<dyn Embedder>,
    ) -> PunchResult<Self> {
        let store = EmbeddingStore::new(conn, embedder)?;
        self.embedding_store = Some(StdMutex::new(store));
        Ok(self)
    }

    /// Attach a default built-in (TF-IDF) embedding store using an in-memory
    /// SQLite connection. Useful for testing and offline operation.
    pub fn with_builtin_embeddings(mut self) -> PunchResult<Self> {
        let conn = Connection::open_in_memory().map_err(|e| {
            punch_types::PunchError::Memory(format!(
                "failed to open embedding db: {e}"
            ))
        })?;
        let arc = Arc::new(StdMutex::new(conn));
        let embedder = BuiltInEmbedder::new();
        let store = EmbeddingStore::new(arc, Box::new(embedder))?;
        self.embedding_store = Some(StdMutex::new(store));
        Ok(self)
    }

    /// Returns whether an embedding store is attached.
    pub fn has_embedding_store(&self) -> bool {
        self.embedding_store.is_some()
    }

    /// Store a text embedding (if the embedding store is attached).
    pub fn embed_and_store(
        &self,
        text: &str,
        metadata: HashMap<String, String>,
    ) -> PunchResult<Option<String>> {
        if let Some(ref store_mutex) = self.embedding_store {
            let store = store_mutex
                .lock()
                .map_err(|e| punch_types::PunchError::Memory(format!("lock failed: {e}")))?;
            let id = store.store(text, metadata)?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// Perform semantic search over stored embeddings. Falls back to `None`
    /// if no embedding store is attached.
    pub fn semantic_search(
        &self,
        query: &str,
        k: usize,
    ) -> PunchResult<Option<Vec<(f32, crate::embeddings::Embedding)>>> {
        if let Some(ref store_mutex) = self.embedding_store {
            let store = store_mutex
                .lock()
                .map_err(|e| punch_types::PunchError::Memory(format!("lock failed: {e}")))?;
            let results = store.search(query, k)?;
            Ok(Some(results))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_creation() {
        let substrate = MemorySubstrate::in_memory();
        assert!(substrate.is_ok());
    }

    #[test]
    fn test_no_embedding_store_by_default() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        assert!(!substrate.has_embedding_store());
    }

    #[test]
    fn test_with_builtin_embeddings() {
        let substrate = MemorySubstrate::in_memory()
            .unwrap()
            .with_builtin_embeddings()
            .unwrap();
        assert!(substrate.has_embedding_store());
    }

    #[test]
    fn test_embed_and_store_without_store() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let result = substrate.embed_and_store("hello", HashMap::new()).unwrap();
        assert!(result.is_none(), "no embedding store means None");
    }

    #[test]
    fn test_semantic_search_without_store() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let result = substrate.semantic_search("hello", 5).unwrap();
        assert!(result.is_none(), "no embedding store means None");
    }

    #[test]
    fn test_embed_and_store_with_builtin() {
        let substrate = MemorySubstrate::in_memory()
            .unwrap()
            .with_builtin_embeddings()
            .unwrap();
        let result = substrate.embed_and_store("test text", HashMap::new()).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_semantic_search_with_builtin() {
        let substrate = MemorySubstrate::in_memory()
            .unwrap()
            .with_builtin_embeddings()
            .unwrap();
        substrate.embed_and_store("hello world", HashMap::new()).unwrap();
        let results = substrate.semantic_search("hello", 5).unwrap();
        assert!(results.is_some());
    }

    #[tokio::test]
    async fn test_conn_access() {
        let substrate = MemorySubstrate::in_memory().unwrap();
        let conn = substrate.conn().await;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0, "should have tables from migrations");
    }
}
