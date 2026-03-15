//! Lock file management for the skills marketplace.
//!
//! The lock file (`punch-moves.lock`) records the exact versions, checksums,
//! and sources of installed marketplace skills. It lives at the workspace root
//! and should be committed to version control for reproducible environments.

use std::path::Path;

use serde::{Deserialize, Serialize};

use punch_types::PunchResult;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The lock file format — pinned versions of all marketplace-installed skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveLockfile {
    /// Lock file format version.
    pub version: u32,
    /// All locked move entries.
    pub moves: Vec<LockedMove>,
}

/// A single locked move entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedMove {
    /// Skill name (slug).
    pub name: String,
    /// Exact pinned version.
    pub version: String,
    /// SHA-256 checksum of the tarball.
    pub checksum: String,
    /// Source URL where the tarball was fetched from.
    pub source: String,
    /// Ed25519 public key of the publisher (hex-encoded).
    pub public_key: String,
}

impl MoveLockfile {
    /// Create a new empty lock file.
    pub fn new() -> Self {
        Self {
            version: 1,
            moves: Vec::new(),
        }
    }
}

impl Default for MoveLockfile {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Read / Write
// ---------------------------------------------------------------------------

/// Read a lock file from disk. Returns `None` if the file doesn't exist.
pub fn read_lockfile(path: &Path) -> PunchResult<Option<MoveLockfile>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    let lockfile: MoveLockfile = serde_json::from_str(&content)
        .map_err(|e| punch_types::PunchError::Config(format!("invalid lock file: {}", e)))?;
    Ok(Some(lockfile))
}

/// Write a lock file to disk (pretty-printed JSON).
pub fn write_lockfile(path: &Path, lockfile: &MoveLockfile) -> PunchResult<()> {
    let content = serde_json::to_string_pretty(lockfile).map_err(|e| {
        punch_types::PunchError::Config(format!("failed to serialize lock file: {}", e))
    })?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Add or update a move entry in the lock file.
///
/// If an entry with the same name exists, it is replaced. Otherwise a new
/// entry is appended.
pub fn add_or_update(lockfile: &mut MoveLockfile, entry: LockedMove) {
    if let Some(existing) = lockfile.moves.iter_mut().find(|m| m.name == entry.name) {
        *existing = entry;
    } else {
        lockfile.moves.push(entry);
    }
    // Keep sorted for deterministic output
    lockfile.moves.sort_by(|a, b| a.name.cmp(&b.name));
}

/// Remove a move entry from the lock file by name.
///
/// Returns `true` if the entry was found and removed.
pub fn remove_entry(lockfile: &mut MoveLockfile, name: &str) -> bool {
    let before = lockfile.moves.len();
    lockfile.moves.retain(|m| m.name != name);
    lockfile.moves.len() < before
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(name: &str) -> LockedMove {
        LockedMove {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            checksum: "abc123".to_string(),
            source: "https://example.com/skill.tar.gz".to_string(),
            public_key: "deadbeef".to_string(),
        }
    }

    #[test]
    fn test_new_lockfile() {
        let lf = MoveLockfile::new();
        assert_eq!(lf.version, 1);
        assert!(lf.moves.is_empty());
    }

    #[test]
    fn test_default_lockfile() {
        let lf = MoveLockfile::default();
        assert_eq!(lf.version, 1);
    }

    #[test]
    fn test_add_entry() {
        let mut lf = MoveLockfile::new();
        add_or_update(&mut lf, sample_entry("my-skill"));
        assert_eq!(lf.moves.len(), 1);
        assert_eq!(lf.moves[0].name, "my-skill");
    }

    #[test]
    fn test_update_entry() {
        let mut lf = MoveLockfile::new();
        add_or_update(&mut lf, sample_entry("my-skill"));

        let mut updated = sample_entry("my-skill");
        updated.version = "2.0.0".to_string();
        add_or_update(&mut lf, updated);

        assert_eq!(lf.moves.len(), 1);
        assert_eq!(lf.moves[0].version, "2.0.0");
    }

    #[test]
    fn test_add_multiple_entries_sorted() {
        let mut lf = MoveLockfile::new();
        add_or_update(&mut lf, sample_entry("zebra"));
        add_or_update(&mut lf, sample_entry("alpha"));
        add_or_update(&mut lf, sample_entry("mid"));

        assert_eq!(lf.moves[0].name, "alpha");
        assert_eq!(lf.moves[1].name, "mid");
        assert_eq!(lf.moves[2].name, "zebra");
    }

    #[test]
    fn test_remove_entry() {
        let mut lf = MoveLockfile::new();
        add_or_update(&mut lf, sample_entry("skill-a"));
        add_or_update(&mut lf, sample_entry("skill-b"));

        assert!(remove_entry(&mut lf, "skill-a"));
        assert_eq!(lf.moves.len(), 1);
        assert_eq!(lf.moves[0].name, "skill-b");
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut lf = MoveLockfile::new();
        assert!(!remove_entry(&mut lf, "missing"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut lf = MoveLockfile::new();
        add_or_update(&mut lf, sample_entry("test"));
        let json = serde_json::to_string(&lf).unwrap();
        let restored: MoveLockfile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.version, 1);
        assert_eq!(restored.moves.len(), 1);
        assert_eq!(restored.moves[0].name, "test");
    }

    #[test]
    fn test_read_lockfile_missing() {
        let path = std::path::PathBuf::from("/tmp/nonexistent-lockfile.json");
        let result = read_lockfile(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_read_write_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("punch-moves.lock");

        let mut lf = MoveLockfile::new();
        add_or_update(&mut lf, sample_entry("round-trip-test"));
        write_lockfile(&path, &lf).unwrap();

        let restored = read_lockfile(&path).unwrap().unwrap();
        assert_eq!(restored.moves.len(), 1);
        assert_eq!(restored.moves[0].name, "round-trip-test");
    }

    #[test]
    fn test_locked_move_equality() {
        let a = sample_entry("skill");
        let b = sample_entry("skill");
        assert_eq!(a, b);
    }
}
