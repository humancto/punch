//! Database backup and restore for the SQLite-backed memory substrate.
//!
//! Supports hot backups (no downtime), optional gzip compression, automatic
//! rotation of old backups, and integrity verification.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use tracing::{info, warn};

use punch_types::{PunchError, PunchResult};

/// Metadata about a single backup.
#[derive(Debug, Clone)]
pub struct BackupInfo {
    /// Unique identifier derived from the filename.
    pub id: String,
    /// Full path to the backup file.
    pub path: PathBuf,
    /// Size in bytes.
    pub size_bytes: u64,
    /// When the backup was created.
    pub created_at: DateTime<Utc>,
    /// Database schema version (from `user_version` pragma).
    pub db_version: String,
}

/// Manages creation, listing, rotation, and restoration of database backups.
pub struct BackupManager {
    /// Path to the live database file.
    db_path: PathBuf,
    /// Directory where backups are stored.
    backup_dir: PathBuf,
    /// Maximum number of backups to keep (oldest are pruned).
    max_backups: usize,
    /// Whether to gzip-compress backup files.
    compress: bool,
}

impl BackupManager {
    /// Create a new backup manager.
    ///
    /// `db_path` is the live SQLite database. `backup_dir` is the directory
    /// where backups will be written. The directory will be created if it does
    /// not exist.
    pub fn new(db_path: PathBuf, backup_dir: PathBuf) -> Self {
        Self {
            db_path,
            backup_dir,
            max_backups: 10,
            compress: false,
        }
    }

    /// Set the maximum number of backups to retain.
    pub fn with_max_backups(mut self, max: usize) -> Self {
        self.max_backups = max;
        self
    }

    /// Enable or disable gzip compression for backups.
    pub fn with_compression(mut self, compress: bool) -> Self {
        self.compress = compress;
        self
    }

    /// Create a new backup of the live database.
    ///
    /// Uses SQLite's `VACUUM INTO` to produce a consistent snapshot without
    /// interrupting readers/writers on the live database.
    pub async fn create_backup(&self) -> PunchResult<BackupInfo> {
        std::fs::create_dir_all(&self.backup_dir).map_err(|e| {
            PunchError::Memory(format!(
                "failed to create backup directory {}: {e}",
                self.backup_dir.display()
            ))
        })?;

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let base_name = format!("punch_backup_{}.db", timestamp);
        let backup_path = self.backup_dir.join(&base_name);

        // Perform VACUUM INTO on a blocking thread (SQLite is not async).
        let db_path = self.db_path.clone();
        let dest = backup_path.clone();
        let compress = self.compress;

        let final_path = tokio::task::spawn_blocking(move || -> PunchResult<PathBuf> {
            let conn = Connection::open(&db_path).map_err(|e| {
                PunchError::Memory(format!("failed to open database for backup: {e}"))
            })?;

            let dest_str = dest.to_string_lossy().to_string();
            conn.execute_batch(&format!("VACUUM INTO '{}'", dest_str.replace('\'', "''")))
                .map_err(|e| PunchError::Memory(format!("VACUUM INTO failed: {e}")))?;

            // Verify the backup.
            verify_backup(&dest)?;

            if compress {
                let gz_path = compress_backup(&dest)?;
                // Remove uncompressed file after successful compression.
                let _ = std::fs::remove_file(&dest);
                Ok(gz_path)
            } else {
                Ok(dest)
            }
        })
        .await
        .map_err(|e| PunchError::Memory(format!("backup task panicked: {e}")))??;

        let metadata = std::fs::metadata(&final_path)
            .map_err(|e| PunchError::Memory(format!("failed to stat backup file: {e}")))?;

        let id = final_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&base_name)
            .to_string();

        let info = BackupInfo {
            id,
            path: final_path.clone(),
            size_bytes: metadata.len(),
            created_at: Utc::now(),
            db_version: read_db_version(&self.db_path)?,
        };

        info!(
            path = %final_path.display(),
            size_bytes = info.size_bytes,
            "database backup created"
        );

        Ok(info)
    }

    /// List all existing backups, newest first.
    pub async fn list_backups(&self) -> PunchResult<Vec<BackupInfo>> {
        let backup_dir = self.backup_dir.clone();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || -> PunchResult<Vec<BackupInfo>> {
            let mut backups = Vec::new();

            let entries = match std::fs::read_dir(&backup_dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(backups),
                Err(e) => {
                    return Err(PunchError::Memory(format!(
                        "failed to read backup directory: {e}"
                    )));
                }
            };

            for entry in entries {
                let entry = entry.map_err(|e| {
                    PunchError::Memory(format!("failed to read directory entry: {e}"))
                })?;

                let path = entry.path();
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();

                if !name.starts_with("punch_backup_") {
                    continue;
                }

                let metadata = entry
                    .metadata()
                    .map_err(|e| PunchError::Memory(format!("failed to stat backup file: {e}")))?;

                let created_at = metadata
                    .created()
                    .or_else(|_| metadata.modified())
                    .map(DateTime::<Utc>::from)
                    .unwrap_or_else(|_| Utc::now());

                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name)
                    .to_string();

                let db_version =
                    read_db_version(&db_path).unwrap_or_else(|_| "unknown".to_string());

                backups.push(BackupInfo {
                    id,
                    path,
                    size_bytes: metadata.len(),
                    created_at,
                    db_version,
                });
            }

            // Sort newest first.
            backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));

            Ok(backups)
        })
        .await
        .map_err(|e| PunchError::Memory(format!("list backups task panicked: {e}")))?
    }

    /// Restore the live database from a backup identified by `backup_id`.
    ///
    /// The `backup_id` is the file stem (e.g. `punch_backup_20260101_120000`).
    /// If the backup is gzip-compressed it will be decompressed first.
    pub async fn restore_backup(&self, backup_id: &str) -> PunchResult<()> {
        let backups = self.list_backups().await?;
        let backup = backups
            .iter()
            .find(|b| b.id == backup_id)
            .ok_or_else(|| PunchError::Memory(format!("backup not found: {backup_id}")))?;

        let backup_path = backup.path.clone();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || -> PunchResult<()> {
            let source = if backup_path.extension().and_then(|e| e.to_str()) == Some("gz") {
                decompress_backup(&backup_path)?
            } else {
                backup_path.clone()
            };

            // Verify backup integrity before restoring.
            verify_backup(&source)?;

            // Copy the backup over the live database.
            std::fs::copy(&source, &db_path)
                .map_err(|e| PunchError::Memory(format!("failed to restore backup: {e}")))?;

            // Clean up decompressed temp file if we made one.
            if source != backup_path {
                let _ = std::fs::remove_file(&source);
            }

            info!(
                backup = %backup_path.display(),
                target = %db_path.display(),
                "database restored from backup"
            );

            Ok(())
        })
        .await
        .map_err(|e| PunchError::Memory(format!("restore task panicked: {e}")))?
    }

    /// Remove old backups, keeping only the most recent `max_backups`.
    ///
    /// Returns the number of backups removed.
    pub async fn cleanup_old_backups(&self) -> PunchResult<usize> {
        let backups = self.list_backups().await?;

        if backups.len() <= self.max_backups {
            return Ok(0);
        }

        let to_remove = &backups[self.max_backups..];
        let mut removed = 0;

        for backup in to_remove {
            match std::fs::remove_file(&backup.path) {
                Ok(()) => {
                    info!(path = %backup.path.display(), "removed old backup");
                    removed += 1;
                }
                Err(e) => {
                    warn!(
                        path = %backup.path.display(),
                        error = %e,
                        "failed to remove old backup"
                    );
                }
            }
        }

        Ok(removed)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read the `user_version` pragma from a SQLite database.
fn read_db_version(path: &Path) -> PunchResult<String> {
    let conn = Connection::open(path).map_err(|e| {
        PunchError::Memory(format!("failed to open database for version check: {e}"))
    })?;

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| PunchError::Memory(format!("failed to read user_version: {e}")))?;

    Ok(version.to_string())
}

/// Verify a backup file by opening it and running `PRAGMA integrity_check`.
fn verify_backup(path: &Path) -> PunchResult<()> {
    let conn = Connection::open(path)
        .map_err(|e| PunchError::Memory(format!("failed to open backup for verification: {e}")))?;

    let result: String = conn
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(|e| PunchError::Memory(format!("integrity check failed: {e}")))?;

    if result != "ok" {
        return Err(PunchError::Memory(format!(
            "backup integrity check failed: {result}"
        )));
    }

    Ok(())
}

/// Compress a backup file with gzip, returning the path to the `.gz` file.
fn compress_backup(path: &Path) -> PunchResult<PathBuf> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::{Read, Write};

    let gz_path = path.with_extension("db.gz");
    let mut input = std::fs::File::open(path)
        .map_err(|e| PunchError::Memory(format!("failed to open backup for compression: {e}")))?;

    let output = std::fs::File::create(&gz_path)
        .map_err(|e| PunchError::Memory(format!("failed to create compressed backup: {e}")))?;

    let mut encoder = GzEncoder::new(output, Compression::default());
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = input
            .read(&mut buf)
            .map_err(|e| PunchError::Memory(format!("read error during compression: {e}")))?;
        if n == 0 {
            break;
        }
        encoder
            .write_all(&buf[..n])
            .map_err(|e| PunchError::Memory(format!("write error during compression: {e}")))?;
    }

    encoder
        .finish()
        .map_err(|e| PunchError::Memory(format!("failed to finalize compressed backup: {e}")))?;

    Ok(gz_path)
}

/// Decompress a `.gz` backup, returning the path to the decompressed file.
fn decompress_backup(gz_path: &Path) -> PunchResult<PathBuf> {
    use flate2::read::GzDecoder;
    use std::io::{Read, Write};

    let stem = gz_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("backup");
    let out_path = gz_path.with_file_name(format!("{}_restored.db", stem));

    let input = std::fs::File::open(gz_path)
        .map_err(|e| PunchError::Memory(format!("failed to open compressed backup: {e}")))?;

    let mut decoder = GzDecoder::new(input);
    let mut output = std::fs::File::create(&out_path)
        .map_err(|e| PunchError::Memory(format!("failed to create decompressed file: {e}")))?;

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = decoder
            .read(&mut buf)
            .map_err(|e| PunchError::Memory(format!("read error during decompression: {e}")))?;
        if n == 0 {
            break;
        }
        output
            .write_all(&buf[..n])
            .map_err(|e| PunchError::Memory(format!("write error during decompression: {e}")))?;
    }

    Ok(out_path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a minimal SQLite database for testing.
    fn create_test_db(path: &Path) {
        let conn = Connection::open(path).expect("create test db");
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT);
             INSERT INTO test (value) VALUES ('hello');",
        )
        .expect("init test db");
    }

    #[tokio::test]
    async fn create_backup_produces_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let backup_dir = dir.path().join("backups");

        create_test_db(&db_path);

        let mgr = BackupManager::new(db_path, backup_dir.clone());
        let info = mgr.create_backup().await.expect("create backup");

        assert!(info.path.exists());
        assert!(info.size_bytes > 0);
        assert!(info.id.starts_with("punch_backup_"));
    }

    #[tokio::test]
    async fn backup_is_valid_sqlite() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let backup_dir = dir.path().join("backups");

        create_test_db(&db_path);

        let mgr = BackupManager::new(db_path, backup_dir);
        let info = mgr.create_backup().await.expect("create backup");

        // Should be openable and pass integrity check.
        verify_backup(&info.path).expect("integrity check should pass");

        // Should contain our test data.
        let conn = Connection::open(&info.path).expect("open backup");
        let value: String = conn
            .query_row("SELECT value FROM test WHERE id = 1", [], |row| row.get(0))
            .expect("query backup");
        assert_eq!(value, "hello");
    }

    #[tokio::test]
    async fn list_backups_returns_created_backups() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let backup_dir = dir.path().join("backups");

        create_test_db(&db_path);

        let mgr = BackupManager::new(db_path, backup_dir);

        // Empty initially.
        let list = mgr.list_backups().await.expect("list");
        assert!(list.is_empty());

        mgr.create_backup().await.expect("backup 1");
        let list = mgr.list_backups().await.expect("list");
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn cleanup_removes_old_backups() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let backup_dir = dir.path().join("backups");

        create_test_db(&db_path);

        let mgr = BackupManager::new(db_path, backup_dir.clone()).with_max_backups(2);

        // Create 4 backups with distinct timestamps.
        for i in 0..4 {
            let name = format!("punch_backup_20260101_12000{}.db", i);
            let path = backup_dir.join(&name);
            fs::create_dir_all(&backup_dir).expect("mkdir");
            // Create a minimal valid SQLite file by copying.
            let conn = Connection::open(&path).expect("create");
            conn.execute_batch("CREATE TABLE t (id INTEGER);")
                .expect("init");
        }

        let removed = mgr.cleanup_old_backups().await.expect("cleanup");
        assert_eq!(removed, 2);

        let remaining = mgr.list_backups().await.expect("list");
        assert_eq!(remaining.len(), 2);
    }

    #[tokio::test]
    async fn backup_naming_follows_pattern() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let backup_dir = dir.path().join("backups");

        create_test_db(&db_path);

        let mgr = BackupManager::new(db_path, backup_dir);
        let info = mgr.create_backup().await.expect("backup");

        let filename = info
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("filename");
        assert!(
            filename.starts_with("punch_backup_"),
            "expected punch_backup_ prefix, got: {filename}"
        );
        assert!(
            filename.ends_with(".db"),
            "expected .db suffix, got: {filename}"
        );
    }

    #[tokio::test]
    async fn compressed_backup_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let backup_dir = dir.path().join("backups");

        create_test_db(&db_path);

        let mgr = BackupManager::new(db_path.clone(), backup_dir).with_compression(true);
        let info = mgr.create_backup().await.expect("backup");

        let filename = info
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("filename");
        assert!(
            filename.ends_with(".db.gz"),
            "expected .db.gz suffix, got: {filename}"
        );

        // Decompress and verify.
        let restored = decompress_backup(&info.path).expect("decompress");
        verify_backup(&restored).expect("integrity check on decompressed");
    }
}
